//! SQLite storage using a connection independent of the recording store.

mod records;
#[cfg(test)]
mod tests;

use std::path::Path;

use recorder_domain::{speaker_label, SessionId, TrackKind};
use rusqlite::{params, Connection, Transaction, TransactionBehavior};
use serde_json::json;

use self::records::{load_graph, read_utterances, revision, write_changes};
use crate::{apply_patch, Graph, Op, PatchResult, PatchSource, Review, Utterance};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    #[error("could not open the session database")]
    Open(#[from] session_store::StoreError),
    #[error("graph database operation failed")]
    Sql(#[from] rusqlite::Error),
    #[error("invalid transcript track")]
    Track(#[from] recorder_domain::ParseTrackKindError),
    #[error("could not encode graph operations")]
    Json(#[from] serde_json::Error),
    #[error("patch input refers to an unknown utterance")]
    UnknownUtterance,
    #[error("review target does not exist or has already been reviewed")]
    UnreviewableTarget,
    #[error("review patches must use set_review")]
    InvalidPatchSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewTarget<'a> {
    Node(&'a str),
    Edge(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    Approve,
    Reject,
}

pub struct GraphStore {
    conn: Connection,
}

impl GraphStore {
    fn begin_write(&mut self) -> rusqlite::Result<Transaction<'_>> {
        self.conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
    }

    /// Open a second connection to the same on-disk session database.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let mut conn = session_store::open_with_pragmas(path)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(include_str!("schema.sql"))?;
        tx.commit()?;
        Ok(Self { conn })
    }

    /// Import new final transcript rows in insertion order and return their count.
    pub fn import_utterances(&mut self, session_id: SessionId) -> Result<usize, StoreError> {
        let sid = session_id.to_string();
        let tx = self.begin_write()?;
        let (last_row, last_uid): (i64, u64) = tx.query_row(
            "SELECT COALESCE(MAX(row_id), 0), COALESCE(MAX(CAST(substr(uid, 2) AS INTEGER)), 0)
             FROM graph_utterances WHERE session_id = ?1",
            [&sid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let mut read = tx.prepare(
            "SELECT id, track, speaker, start_ms, end_ms
             FROM transcript_segments
             WHERE session_id = ?1 AND is_final = 1 AND id > ?2 ORDER BY id",
        )?;
        let mut rows = read.query(params![sid, last_row])?;
        let mut insert = tx.prepare(
            "INSERT INTO graph_utterances (session_id, uid, row_id, track, speaker_label, start_ms, end_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        let mut next_uid = last_uid;
        while let Some(row) = rows.next()? {
            let track: Option<String> = row.get(1)?;
            let label = speaker_label(
                track.as_deref().map(str::parse::<TrackKind>).transpose()?,
                row.get(2)?,
            );
            next_uid += 1;
            insert.execute(params![
                sid,
                format!("u{next_uid}"),
                row.get::<_, i64>(0)?,
                track,
                label,
                row.get::<_, Option<u64>>(3)?,
                row.get::<_, Option<u64>>(4)?
            ])?;
        }
        drop(rows);
        drop((read, insert));
        tx.commit()?;
        Ok((next_uid - last_uid) as usize)
    }

    /// Read a consistent snapshot, including its revision.
    pub fn load_graph(&self, session_id: SessionId) -> Result<Graph, StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let graph = load_graph(&tx, &session_id.to_string())?;
        tx.commit()?;
        Ok(graph)
    }

    pub fn pending_utterances(&self, session_id: SessionId) -> Result<Vec<Utterance>, StoreError> {
        Ok(read_utterances(&self.conn, &session_id.to_string(), true)?)
    }

    pub fn pending_count(&self, session_id: SessionId) -> Result<usize, StoreError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM graph_utterances WHERE session_id = ?1 AND extracted_rev IS NULL",
            [session_id.to_string()],
            |row| row.get(0),
        )?)
    }

    pub fn revision(&self, session_id: SessionId) -> Result<u64, StoreError> {
        Ok(revision(&self.conn, &session_id.to_string())?)
    }

    /// Apply parsed operations. Only `input_uids` are consumed. Reference IDs
    /// can supply grounds without changing their extraction status.
    /// Every stored patch advances `result.graph.rev`, including empty patches.
    /// Use `result.applied.is_empty()` to detect a patch without graph changes.
    pub fn apply_patch(
        &mut self,
        session_id: SessionId,
        source: PatchSource,
        provider_model: Option<&str>,
        ops: &[Op],
        input_uids: &[&str],
        reference_uids: &[&str],
    ) -> Result<PatchResult, StoreError> {
        if source == PatchSource::Review {
            return Err(StoreError::InvalidPatchSource);
        }
        let sid = session_id.to_string();
        let tx = self.begin_write()?;
        let before = load_graph(&tx, &sid)?;
        let allowed: Vec<_> = input_uids.iter().chain(reference_uids).copied().collect();
        if allowed
            .iter()
            .any(|uid| !before.utterances.contains_key(*uid))
        {
            return Err(StoreError::UnknownUtterance);
        }
        let mut result = apply_patch(&before, ops, &allowed);
        let rev = before.rev + 1;
        result.graph.rev = rev;
        let ops_json = serde_json::to_string(&result.applied)?;
        let rejected_ops: usize = result.rejected.values().sum();
        tx.execute(
            "INSERT INTO graph_patches (session_id, rev, source, provider_model, ops_json, rejected_ops)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![sid, rev, source.as_str(), provider_model, ops_json, rejected_ops],
        )?;
        if !result.applied.is_empty() {
            write_changes(&tx, &sid, &before, &result.graph)?;
        }
        {
            let mut mark = tx.prepare(
                "UPDATE graph_utterances SET extracted_rev = ?3
                 WHERE session_id = ?1 AND uid = ?2 AND extracted_rev IS NULL",
            )?;
            for uid in input_uids {
                mark.execute(params![sid, uid, rev])?;
            }
        }
        tx.commit()?;
        Ok(result)
    }

    /// Record a human decision for an automatic node or edge and return its revision.
    pub fn set_review(
        &mut self,
        session_id: SessionId,
        target: ReviewTarget<'_>,
        decision: ReviewDecision,
    ) -> Result<u64, StoreError> {
        let (id, kind) = match target {
            ReviewTarget::Node(id) => (id, "node"),
            ReviewTarget::Edge(id) => (id, "edge"),
        };
        let review = match decision {
            ReviewDecision::Approve => Review::Approved,
            ReviewDecision::Reject => Review::Rejected,
        };
        let ops_json =
            json!([{ "op": "set_review", "target": id, "kind": kind, "review": review }])
                .to_string();
        let sid = session_id.to_string();
        let tx = self.begin_write()?;
        let rev = revision(&tx, &sid)? + 1;
        let changed = match target {
            ReviewTarget::Node(_) => tx.execute(
                "UPDATE graph_nodes SET review = ?3, updated_rev = ?4
                 WHERE session_id = ?1 AND nid = ?2 AND review = 'auto' AND status <> 'merged'",
                params![sid, id, review.as_str(), rev],
            )?,
            ReviewTarget::Edge(_) => tx.execute(
                "UPDATE graph_edges SET review = ?3
                 WHERE session_id = ?1 AND eid = ?2 AND review = 'auto'",
                params![sid, id, review.as_str()],
            )?,
        };
        if changed == 0 {
            return Err(StoreError::UnreviewableTarget);
        }
        tx.execute(
            "INSERT INTO graph_patches (session_id, rev, source, ops_json, rejected_ops)
             VALUES (?1, ?2, 'review', ?3, 0)",
            params![sid, rev, ops_json],
        )?;
        tx.commit()?;
        Ok(rev)
    }

    /// Delete the graph and its import cursor so this session can be rebuilt.
    pub fn delete_session(&mut self, session_id: SessionId) -> Result<(), StoreError> {
        let sid = session_id.to_string();
        let tx = self.begin_write()?;
        for sql in [
            "DELETE FROM graph_grounds WHERE session_id = ?1",
            "DELETE FROM graph_edges WHERE session_id = ?1",
            "DELETE FROM graph_nodes WHERE session_id = ?1",
            "DELETE FROM graph_utterances WHERE session_id = ?1",
            "DELETE FROM graph_patches WHERE session_id = ?1",
        ] {
            tx.execute(sql, [&sid])?;
        }
        tx.commit()?;
        Ok(())
    }
}
