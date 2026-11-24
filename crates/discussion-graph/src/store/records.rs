use std::{collections::BTreeMap, str::FromStr};

use rusqlite::{params, types::Type, Connection, Row};

use crate::{Edge, Graph, Ground, Node, UnknownValue, Utterance};

pub(super) fn revision(conn: &Connection, sid: &str) -> rusqlite::Result<u64> {
    conn.query_row(
        "SELECT COALESCE(MAX(rev), 0) FROM graph_patches WHERE session_id = ?1",
        [sid],
        |row| row.get(0),
    )
}

pub(super) fn read_utterances(
    conn: &Connection,
    sid: &str,
    pending_only: bool,
) -> rusqlite::Result<Vec<Utterance>> {
    let mut stmt = conn.prepare(
        "SELECT u.uid, u.row_id, u.track, u.speaker_label, u.start_ms, u.end_ms,
                COALESCE(t.text, ''), COALESCE(t.is_retranscribed, 0)
         FROM graph_utterances u
         LEFT JOIN transcript_segments t ON t.id = u.row_id AND t.session_id = u.session_id
         WHERE u.session_id = ?1 AND (NOT ?2 OR u.extracted_rev IS NULL)
         ORDER BY u.row_id",
    )?;
    let rows = stmt.query_map(params![sid, pending_only], |row| {
        Ok(Utterance {
            uid: row.get(0)?,
            row_id: row.get(1)?,
            track: row.get(2)?,
            speaker_label: row.get(3)?,
            start_ms: row.get(4)?,
            end_ms: row.get(5)?,
            text: row.get(6)?,
            is_retranscribed: row.get(7)?,
        })
    })?;
    rows.collect()
}

pub(super) fn load_graph(conn: &Connection, sid: &str) -> rusqlite::Result<Graph> {
    let rev = revision(conn, sid)?;
    let utterances = read_utterances(conn, sid, false)?
        .into_iter()
        .map(|utterance| (utterance.uid.clone(), utterance))
        .collect();
    let mut stmt = conn.prepare(
        "SELECT nid, kind, text, status, merged_into, confidence, review, created_rev, updated_rev
         FROM graph_nodes WHERE session_id = ?1",
    )?;
    let nodes = stmt
        .query_map([sid], |row| {
            let node = Node {
                nid: row.get(0)?,
                kind: parse_column(row, 1)?,
                text: row.get(2)?,
                status: parse_column(row, 3)?,
                merged_into: row.get(4)?,
                confidence: row.get(5)?,
                review: parse_column(row, 6)?,
                created_rev: row.get(7)?,
                updated_rev: row.get(8)?,
            };
            Ok((node.nid.clone(), node))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare(
        "SELECT eid, src, dst, kind, confidence, review, created_rev
         FROM graph_edges WHERE session_id = ?1",
    )?;
    let edges = stmt
        .query_map([sid], |row| {
            let edge = Edge {
                eid: row.get(0)?,
                src: row.get(1)?,
                dst: row.get(2)?,
                kind: parse_column(row, 3)?,
                confidence: row.get(4)?,
                review: parse_column(row, 5)?,
                created_rev: row.get(6)?,
            };
            Ok((edge.eid.clone(), edge))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare(
        "SELECT target, uid, act FROM graph_grounds WHERE session_id = ?1 ORDER BY target, uid",
    )?;
    let grounds = stmt
        .query_map([sid], |row| {
            Ok(Ground {
                target: row.get(0)?,
                uid: row.get(1)?,
                act: row
                    .get::<_, Option<String>>(2)?
                    .map(|value| parse_value(&value, 2))
                    .transpose()?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Graph {
        rev,
        utterances,
        nodes,
        edges,
        grounds,
    })
}

pub(super) fn parse_column<T: FromStr<Err = UnknownValue>>(row: &Row<'_>, col: usize) -> rusqlite::Result<T> {
    parse_value(&row.get::<_, String>(col)?, col)
}

fn parse_value<T: FromStr<Err = UnknownValue>>(value: &str, col: usize) -> rusqlite::Result<T> {
    value
        .parse()
        .map_err(|err| rusqlite::Error::FromSqlConversionFailure(col, Type::Text, Box::new(err)))
}

pub(super) fn write_changes(
    conn: &Connection,
    sid: &str,
    before: &Graph,
    after: &Graph,
) -> rusqlite::Result<()> {
    let mut nodes = conn.prepare(
        "INSERT INTO graph_nodes (session_id, nid, kind, text, status, merged_into, confidence,
                                 review, created_rev, updated_rev)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (session_id, nid) DO UPDATE SET
           status = excluded.status, merged_into = excluded.merged_into, updated_rev = excluded.updated_rev",
    )?;
    for node in after
        .nodes
        .values()
        .filter(|node| before.nodes.get(&node.nid) != Some(*node))
    {
        nodes.execute(params![
            sid,
            node.nid,
            node.kind.as_str(),
            node.text,
            node.status.as_str(),
            node.merged_into,
            node.confidence,
            node.review.as_str(),
            node.created_rev,
            node.updated_rev
        ])?;
    }

    // Remove old keys before inserting redirected edges, since a merge can
    // replace an edge with an older edge that has moved onto the same key.
    let mut delete = conn.prepare("DELETE FROM graph_edges WHERE session_id = ?1 AND eid = ?2")?;
    for edge in before
        .edges
        .values()
        .filter(|edge| after.edges.get(&edge.eid) != Some(*edge))
    {
        delete.execute(params![sid, edge.eid])?;
    }
    let mut insert = conn.prepare(
        "INSERT INTO graph_edges (session_id, eid, src, dst, kind, confidence, review, created_rev)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for edge in after
        .edges
        .values()
        .filter(|edge| before.edges.get(&edge.eid) != Some(*edge))
    {
        insert.execute(params![
            sid,
            edge.eid,
            edge.src,
            edge.dst,
            edge.kind.as_str(),
            edge.confidence,
            edge.review.as_str(),
            edge.created_rev
        ])?;
    }

    let old_grounds: BTreeMap<_, _> = before
        .grounds
        .iter()
        .map(|g| ((g.target.as_str(), g.uid.as_str()), g.act))
        .collect();
    let new_grounds: BTreeMap<_, _> = after
        .grounds
        .iter()
        .map(|g| ((g.target.as_str(), g.uid.as_str()), g.act))
        .collect();
    let mut delete = conn
        .prepare("DELETE FROM graph_grounds WHERE session_id = ?1 AND target = ?2 AND uid = ?3")?;
    for &(target, uid) in old_grounds
        .keys()
        .filter(|key| !new_grounds.contains_key(key))
    {
        delete.execute(params![sid, target, uid])?;
    }
    let mut upsert = conn.prepare(
        "INSERT INTO graph_grounds (session_id, target, uid, act) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (session_id, target, uid) DO UPDATE SET act = excluded.act",
    )?;
    for (&(target, uid), &act) in new_grounds
        .iter()
        .filter(|(key, act)| old_grounds.get(*key) != Some(*act))
    {
        upsert.execute(params![sid, target, uid, act.map(crate::Act::as_str)])?;
    }
    Ok(())
}
