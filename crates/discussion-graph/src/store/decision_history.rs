use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::records::parse_column;
use super::{load_graph, revision, GraphStore, SessionId, StoreError};
use crate::{ChangeState, DecisionChange, DecisionChangeAction, DecisionHistory, Graph, NodeKind};

impl GraphStore {
    pub fn load_decision_history(
        &self,
        session_id: SessionId,
    ) -> Result<DecisionHistory, StoreError> {
        let sid = session_id.to_string();
        let tx = self.conn.unchecked_transaction()?;
        let graph = load_graph(&tx, &sid)?;
        let changes = load_changes(&tx, &sid)?;
        tx.commit()?;
        Ok(DecisionHistory { graph, changes })
    }

    /// Node and evidence IDs are local to the listed session.
    pub fn add_decision_change(
        &mut self,
        session_id: SessionId,
        listed_rev: u64,
        previous: &str,
        replacement: Option<&str>,
        evidence: &str,
    ) -> Result<u64, StoreError> {
        let sid = session_id.to_string();
        let tx = self.begin_write()?;
        let rev = next_revision(&tx, &sid, listed_rev)?;
        validate(&load_graph(&tx, &sid)?, previous, replacement, evidence)?;
        if let Some(existing) = load_changes(&tx, &sid)?.iter().find(|change| {
            change.previous == previous
                && change.replacement.as_deref() == replacement
                && change.evidence == evidence
        }) {
            return Ok(existing.id);
        }

        tx.execute(
            "INSERT INTO graph_decision_changes (session_id, id, previous, replacement, evidence, state)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending')",
            params![sid, rev, previous, replacement, evidence],
        )?;
        record_review(
            &tx,
            &sid,
            rev,
            json!({ "op": "add_decision_change", "id": rev }),
        )?;
        tx.commit()?;
        Ok(rev)
    }

    pub fn review_decision_change(
        &mut self,
        session_id: SessionId,
        listed_rev: u64,
        id: u64,
        action: DecisionChangeAction,
    ) -> Result<u64, StoreError> {
        let sid = session_id.to_string();
        let tx = self.begin_write()?;
        let rev = next_revision(&tx, &sid, listed_rev)?;
        let changes = load_changes(&tx, &sid)?;
        let change = changes
            .iter()
            .find(|change| change.id == id)
            .ok_or(StoreError::InvalidDecisionChange)?;
        let approved_from = |nid: &str| {
            changes.iter().find(|candidate| {
                candidate.state == ChangeState::Approved && candidate.previous == nid
            })
        };
        let state = match (change.state, action) {
            (ChangeState::Pending, DecisionChangeAction::Approve { same_subject: true }) => {
                validate(
                    &load_graph(&tx, &sid)?,
                    &change.previous,
                    change.replacement.as_deref(),
                    &change.evidence,
                )?;
                if approved_from(&change.previous).is_some() {
                    return Err(StoreError::InvalidChangeTransition);
                }
                let mut next = change.replacement.as_deref();
                while let Some(nid) = next {
                    if nid == change.previous {
                        return Err(StoreError::InvalidChangeTransition);
                    }
                    next = approved_from(nid).and_then(|entry| entry.replacement.as_deref());
                }
                ChangeState::Approved
            }
            (ChangeState::Pending, DecisionChangeAction::Reject) => ChangeState::Rejected,
            (ChangeState::Approved, DecisionChangeAction::Revoke)
                if change
                    .replacement
                    .as_deref()
                    .and_then(approved_from)
                    .is_none() =>
            {
                ChangeState::Revoked
            }
            (ChangeState::Rejected | ChangeState::Revoked, DecisionChangeAction::Reopen) => {
                validate(
                    &load_graph(&tx, &sid)?,
                    &change.previous,
                    change.replacement.as_deref(),
                    &change.evidence,
                )?;
                ChangeState::Pending
            }
            _ => return Err(StoreError::InvalidChangeTransition),
        };

        tx.execute(
            "UPDATE graph_decision_changes SET state = ?3 WHERE session_id = ?1 AND id = ?2",
            params![sid, id, state.as_str()],
        )?;
        record_review(
            &tx,
            &sid,
            rev,
            json!({ "op": "review_decision_change", "id": id, "state": state }),
        )?;
        tx.commit()?;
        Ok(rev)
    }
}

fn load_changes(conn: &Connection, sid: &str) -> rusqlite::Result<Vec<DecisionChange>> {
    let mut query = conn.prepare(
        "SELECT id, previous, replacement, evidence, state
         FROM graph_decision_changes WHERE session_id = ?1 ORDER BY id",
    )?;
    let rows = query.query_map([sid], |row| {
        Ok(DecisionChange {
            id: row.get(0)?,
            previous: row.get(1)?,
            replacement: row.get(2)?,
            evidence: row.get(3)?,
            state: parse_column(row, 4)?,
        })
    })?;
    rows.collect()
}

pub(super) fn is_active_endpoint(
    conn: &Connection,
    sid: &str,
    nid: &str,
) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM graph_decision_changes
         WHERE session_id = ?1 AND (previous = ?2 OR replacement = ?2)
         AND state IN ('pending', 'approved'))",
        params![sid, nid],
        |row| row.get(0),
    )
}

fn validate(
    graph: &Graph,
    previous: &str,
    replacement: Option<&str>,
    evidence: &str,
) -> Result<(), StoreError> {
    let kind = graph
        .nodes
        .get(previous)
        .filter(|node| node.is_live() && matches!(node.kind, NodeKind::Decision | NodeKind::Action))
        .map(|node| node.kind)
        .ok_or(StoreError::InvalidDecisionChange)?;
    if !graph.utterances.contains_key(evidence)
        || replacement.is_some_and(|nid| {
            nid == previous
                || !graph
                    .nodes
                    .get(nid)
                    .is_some_and(|node| node.is_live() && node.kind == kind)
        })
    {
        return Err(StoreError::InvalidDecisionChange);
    }
    Ok(())
}

fn next_revision(conn: &Connection, sid: &str, listed_rev: u64) -> Result<u64, StoreError> {
    let actual = revision(conn, sid)?;
    if actual != listed_rev {
        return Err(StoreError::StaleDecisionHistory);
    }
    Ok(actual + 1)
}

fn record_review(conn: &Connection, sid: &str, rev: u64, op: Value) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO graph_patches (session_id, rev, source, ops_json, rejected_ops)
         VALUES (?1, ?2, 'review', ?3, 0)",
        params![sid, rev, json!([op]).to_string()],
    )?;
    Ok(())
}
