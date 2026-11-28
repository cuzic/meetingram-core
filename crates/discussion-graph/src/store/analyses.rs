//! Saved TOC diagram analyses (ADR 0006 section 2). Bodies are stored only here
//! and never logged.

use std::str::FromStr;

use recorder_domain::SessionId;
use rusqlite::{params, types::Type, Row};

use super::{GraphStore, StoreError};
use crate::{AnalysisKind, UnknownValue};

/// A validated analysis to save. A later save with the same key replaces the row.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NewAnalysis<'a> {
    pub kind: AnalysisKind,
    pub subject: &'a str,
    /// The graph revision the input was built from.
    pub based_on_rev: u64,
    pub body_json: &'a str,
    pub provider_model: Option<&'a str>,
    pub dropped: usize,
    pub downgraded: usize,
}

impl<'a> NewAnalysis<'a> {
    pub fn new(
        kind: AnalysisKind,
        subject: &'a str,
        based_on_rev: u64,
        body_json: &'a str,
    ) -> Self {
        Self {
            kind,
            subject,
            based_on_rev,
            body_json,
            provider_model: None,
            dropped: 0,
            downgraded: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StoredAnalysis {
    pub kind: AnalysisKind,
    pub subject: String,
    pub based_on_rev: u64,
    pub body_json: String,
    pub provider_model: Option<String>,
    pub dropped: usize,
    pub downgraded: usize,
    pub created_at: String,
}

const COLUMNS: &str =
    "kind, subject, based_on_rev, body_json, provider_model, dropped, downgraded, created_at";

fn read(row: &Row<'_>) -> rusqlite::Result<StoredAnalysis> {
    let kind: String = row.get(0)?;
    Ok(StoredAnalysis {
        kind: AnalysisKind::from_str(&kind).map_err(|error: UnknownValue| {
            rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(error))
        })?,
        subject: row.get(1)?,
        based_on_rev: row.get(2)?,
        body_json: row.get(3)?,
        provider_model: row.get(4)?,
        dropped: row.get(5)?,
        downgraded: row.get(6)?,
        created_at: row.get(7)?,
    })
}

impl GraphStore {
    /// Save the latest analysis for its key, replacing any earlier one.
    /// Call this inside the same `db()` section that checks the graph generation.
    pub fn save_analysis(
        &mut self,
        session_id: SessionId,
        analysis: &NewAnalysis<'_>,
    ) -> Result<(), StoreError> {
        let tx = self.begin_write()?;
        tx.execute(
            "INSERT INTO graph_analyses
                 (session_id, kind, subject, based_on_rev, body_json, provider_model, dropped, downgraded)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (session_id, kind, subject) DO UPDATE SET
                 based_on_rev = excluded.based_on_rev,
                 body_json = excluded.body_json,
                 provider_model = excluded.provider_model,
                 dropped = excluded.dropped,
                 downgraded = excluded.downgraded,
                 created_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                session_id.to_string(),
                analysis.kind.as_str(),
                analysis.subject,
                analysis.based_on_rev,
                analysis.body_json,
                analysis.provider_model,
                analysis.dropped,
                analysis.downgraded
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn analysis(
        &self,
        session_id: SessionId,
        kind: AnalysisKind,
        subject: &str,
    ) -> Result<Option<StoredAnalysis>, StoreError> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM graph_analyses
                     WHERE session_id = ?1 AND kind = ?2 AND subject = ?3"
                ),
                params![session_id.to_string(), kind.as_str(), subject],
                read,
            )
            .optional()?)
    }

    /// All saved analyses of one kind for a session, ordered by subject.
    pub fn analyses(
        &self,
        session_id: SessionId,
        kind: AnalysisKind,
    ) -> Result<Vec<StoredAnalysis>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM graph_analyses
             WHERE session_id = ?1 AND kind = ?2 ORDER BY subject"
        ))?;
        let rows = stmt.query_map(params![session_id.to_string(), kind.as_str()], read)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Delete one analysis and report whether a row existed.
    pub fn delete_analysis(
        &mut self,
        session_id: SessionId,
        kind: AnalysisKind,
        subject: &str,
    ) -> Result<bool, StoreError> {
        let tx = self.begin_write()?;
        let changed = tx.execute(
            "DELETE FROM graph_analyses WHERE session_id = ?1 AND kind = ?2 AND subject = ?3",
            params![session_id.to_string(), kind.as_str(), subject],
        )?;
        tx.commit()?;
        Ok(changed > 0)
    }
}
