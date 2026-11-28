use std::{sync::mpsc, thread, time::Duration};

use chrono::Utc;
use recorder_domain::{
    AudioCodec, AudioManifest, AudioSegment, CaptureManifest, ConsentManifest, RemoteSourceKind,
    SessionManifest, UploadState,
};
use session_store::{SessionStore, TranscriptSegment};
use tempfile::TempDir;

use super::*;
use crate::{parse_ops, AnalysisKind, Status};

fn create_session(store: &SessionStore) -> SessionId {
    let session_id = SessionId::new();
    store
        .create_session(&SessionManifest {
            schema_version: 1,
            session_id,
            started_at: Utc::now(),
            ended_at: None,
            platform: "test".into(),
            app_version: "0.1.0".into(),
            capture: CaptureManifest {
                microphone_device_id: "synthetic-mic".into(),
                remote_source_id: "synthetic-output".into(),
                remote_source_kind: RemoteSourceKind::EndpointLoopback,
            },
            audio: AudioManifest {
                sample_rate: 48_000,
                segment_duration_ms: 30_000,
                tracks: vec![TrackKind::SelfMic],
            },
            consent: ConsentManifest {
                confirmed_by_user: true,
                confirmed_at: Utc::now(),
            },
        })
        .unwrap();
    session_id
}

fn stores() -> (TempDir, SessionStore, GraphStore) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite");
    let session = SessionStore::open(&path).unwrap();
    let graph = GraphStore::open(&path).unwrap();
    (dir, session, graph)
}

fn append(store: &SessionStore, session_id: SessionId, text: &str) {
    store
        .insert_transcript_segment(&TranscriptSegment {
            session_id,
            track: Some(TrackKind::SelfMic),
            speaker: None,
            text: text.into(),
            start_ms: None,
            end_ms: None,
            is_final: true,
            is_retranscribed: false,
        })
        .unwrap();
}

fn add_issue() -> Vec<Op> {
    parse_ops(
        r#"{"ops":[{"op":"add_node","tmp":"issue","kind":"issue",
        "text":"Synthetic question","confidence":0.75,"grounds":[{"uid":"u1","act":"asks"}]}]}"#,
    )
    .unwrap()
}

#[test]
fn failed_patch_rolls_back_and_recording_store_can_still_write() {
    let (_dir, session, mut graph) = stores();
    let sid = create_session(&session);
    append(&session, sid, "Synthetic first turn");
    append(&session, sid, "Synthetic second turn");
    graph.import_utterances(sid).unwrap();
    // Fail after writing patch, node, ground and the first extraction marker.
    graph
        .conn
        .execute_batch(
            "CREATE TRIGGER fail_extraction BEFORE UPDATE OF extracted_rev ON graph_utterances
         WHEN OLD.uid = 'u2' BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
        )
        .unwrap();
    assert!(matches!(
        graph.apply_patch(
            sid,
            PatchSource::Live,
            None,
            &add_issue(),
            &["u1", "u2"],
            &[]
        ),
        Err(StoreError::Sql(_))
    ));
    let snapshot = graph.load_graph(sid).unwrap();
    assert_eq!(snapshot.rev, 0);
    assert!(snapshot.nodes.is_empty());
    assert!(snapshot.grounds.is_empty());
    assert_eq!(graph.pending_count(sid).unwrap(), 2);

    append(&session, sid, "Synthetic third turn");
    assert_eq!(graph.import_utterances(sid).unwrap(), 1);
}

#[test]
fn recording_writer_waits_for_graph_write_transaction() {
    let (_dir, session, mut graph) = stores();
    let sid = create_session(&session);
    let segment = AudioSegment {
        session_id: sid,
        track: TrackKind::SelfMic,
        sequence: 0,
        timeline_start_ms: 0,
        duration_ms: 30_000,
        codec: AudioCodec::Opus,
        sample_rate: 48_000,
        channels: 1,
        sha256: "0".repeat(64),
        local_path: "synthetic.opus".into(),
        byte_len: 1,
    };
    // BEGIN IMMEDIATE must reserve the write lock before issuing any SQL.
    let tx = graph.begin_write().unwrap();
    let (started, ready) = mpsc::channel();
    let (completed, result) = mpsc::channel();
    let writer = thread::spawn(move || {
        started.send(()).unwrap();
        completed.send(session.register_segment(&segment)).unwrap();
        session
    });
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let while_locked = result.recv_timeout(Duration::from_millis(200));
    tx.commit().unwrap();
    assert!(matches!(while_locked, Err(mpsc::RecvTimeoutError::Timeout)));
    result
        .recv_timeout(Duration::from_secs(6))
        .unwrap()
        .unwrap();
    let session = writer.join().unwrap();
    assert_eq!(
        session.upload_state(sid, TrackKind::SelfMic, 0).unwrap(),
        UploadState::NotStarted
    );
}

#[test]
fn patches_consume_only_input_and_reviews_protect_human_decisions() {
    let (_dir, session, mut graph) = stores();
    let sid = create_session(&session);
    append(&session, sid, "Synthetic context");
    append(&session, sid, "Synthetic input");
    graph.import_utterances(sid).unwrap();

    let empty = graph
        .apply_patch(sid, PatchSource::Live, None, &[], &["u1"], &[])
        .unwrap();
    assert_eq!(empty.patch_rev(), None);
    assert_eq!(graph.revision(sid).unwrap(), 1);
    assert_eq!(graph.pending_count(sid).unwrap(), 1);

    let mut ops = add_issue();
    ops.push(Op(json!({"op":"invalid"})));
    let result = graph
        .apply_patch(
            sid,
            PatchSource::Flush,
            Some("synthetic/model"),
            &ops,
            &["u1"],
            &["u1", "u2"],
        )
        .unwrap();
    assert_eq!(result.patch_rev(), Some(2));
    assert_eq!(graph.pending_count(sid).unwrap(), 1);
    assert_eq!(graph.load_graph(sid).unwrap(), result.graph);
    let (provider, rejected, ops_json): (String, usize, String) = graph
        .conn
        .query_row(
            "SELECT provider_model, rejected_ops, ops_json FROM graph_patches WHERE rev = 2",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(provider, "synthetic/model");
    assert_eq!(rejected, 1);
    assert_eq!(
        serde_json::from_str::<Vec<crate::AppliedOp>>(&ops_json).unwrap(),
        result.applied
    );

    assert_eq!(
        graph
            .set_review(sid, ReviewTarget::Node("n1"), ReviewDecision::Approve)
            .unwrap(),
        3
    );
    let update = parse_ops(
        r#"{"ops":[{"op":"set_status","nid":"n1","status":"resolved",
        "grounds":[{"uid":"u2"}]}]}"#,
    )
    .unwrap();
    let result = graph
        .apply_patch(sid, PatchSource::Consolidate, None, &update, &["u2"], &[])
        .unwrap();
    assert_eq!(result.patch_rev(), None);
    assert_eq!(result.graph.nodes["n1"].review, Review::Approved);
    assert_eq!(result.graph.nodes["n1"].status, Status::Open);
    assert_eq!(graph.revision(sid).unwrap(), 4);
    assert_eq!(graph.pending_count(sid).unwrap(), 0);
    let source: String = graph
        .conn
        .query_row(
            "SELECT source FROM graph_patches WHERE rev = 3",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(source, "review");
    let (ops, rejected): (String, usize) = graph
        .conn
        .query_row(
            "SELECT ops_json, rejected_ops FROM graph_patches WHERE rev = 4",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(ops, "[]");
    assert_eq!(rejected, 1);
    let mut stmt = graph
        .conn
        .prepare("SELECT extracted_rev FROM graph_utterances ORDER BY uid")
        .unwrap();
    let extracted = stmt
        .query_map([], |row| row.get::<_, u64>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(extracted, [1, 4]);
}

#[test]
fn consolidation_is_derived_from_the_saved_patch_history() {
    let (dir, session, mut graph) = stores();
    let sid = create_session(&session);
    let other = create_session(&session);
    assert!(!graph.is_consolidated(sid).unwrap());

    graph
        .apply_patch(other, PatchSource::Consolidate, None, &[], &[], &[])
        .unwrap();
    assert!(graph.is_consolidated(other).unwrap());
    assert!(!graph.is_consolidated(sid).unwrap());

    append(&session, sid, "Synthetic turn");
    graph.import_utterances(sid).unwrap();
    graph
        .apply_patch(sid, PatchSource::Live, None, &add_issue(), &["u1"], &[])
        .unwrap();
    assert!(!graph.is_consolidated(sid).unwrap());

    for source in [PatchSource::Live, PatchSource::Flush] {
        graph
            .apply_patch(sid, PatchSource::Consolidate, None, &[], &[], &[])
            .unwrap();
        assert!(graph.is_consolidated(sid).unwrap());
        graph.apply_patch(sid, source, None, &[], &[], &[]).unwrap();
        assert!(!graph.is_consolidated(sid).unwrap());
    }

    graph
        .apply_patch(sid, PatchSource::Consolidate, None, &[], &[], &[])
        .unwrap();
    graph
        .set_review(sid, ReviewTarget::Node("n1"), ReviewDecision::Approve)
        .unwrap();
    assert!(graph.is_consolidated(sid).unwrap());

    drop(graph);
    let mut graph = GraphStore::open(&dir.path().join("sessions.sqlite")).unwrap();
    assert!(graph.is_consolidated(sid).unwrap());
    graph.delete_session(sid).unwrap();
    assert!(!graph.is_consolidated(sid).unwrap());
    assert!(graph.is_consolidated(other).unwrap());
}

#[test]
fn deleting_one_session_keeps_the_other_and_allows_reimport() {
    let (_dir, session, mut graph) = stores();
    let first = create_session(&session);
    let second = create_session(&session);
    for sid in [first, second] {
        append(&session, sid, "Synthetic turn");
        graph.import_utterances(sid).unwrap();
        graph
            .apply_patch(sid, PatchSource::Live, None, &add_issue(), &["u1"], &[])
            .unwrap();
    }
    let preserved = graph.load_graph(second).unwrap();
    graph.delete_session(first).unwrap();
    assert_eq!(graph.load_graph(first).unwrap(), Graph::default());
    assert_eq!(graph.load_graph(second).unwrap(), preserved);
    assert_eq!(graph.import_utterances(first).unwrap(), 1);
    assert_eq!(graph.pending_utterances(first).unwrap()[0].uid, "u1");
}

#[test]
#[ignore = "synthetic meeting evaluation"]
fn stored_budget_meeting_matches_gold_graph() {
    use crate::eval::{evaluate, GoldGraph};

    let (dir, session, mut graph) = stores();
    let sid = create_session(&session);
    let mut input_ids = Vec::new();
    for (i, line) in include_str!("../../testdata/budget.jsonl")
        .lines()
        .enumerate()
    {
        #[derive(serde::Deserialize)]
        struct FixtureTurn {
            row_id: usize,
            text: String,
            start_ms: u64,
            end_ms: u64,
        }
        let turn: FixtureTurn = serde_json::from_str(line).unwrap();
        assert_eq!(turn.row_id, i + 1);
        session
            .insert_transcript_segment(&TranscriptSegment {
                session_id: sid,
                track: None,
                speaker: None,
                text: turn.text,
                start_ms: Some(turn.start_ms),
                end_ms: Some(turn.end_ms),
                is_final: true,
                is_retranscribed: false,
            })
            .unwrap();
        input_ids.push(format!("u{}", turn.row_id));
    }
    assert_eq!(graph.import_utterances(sid).unwrap(), input_ids.len());

    let gold: GoldGraph =
        serde_json::from_str(include_str!("../../testdata/budget.gold.json")).unwrap();
    let mut ops = Vec::new();
    for node in &gold.nodes {
        let grounds: Vec<_> = node
            .grounds
            .iter()
            .map(|row| json!({"uid": format!("u{row}")}))
            .collect();
        ops.push(Op(json!({
            "op": "add_node", "tmp": node.id, "kind": node.kind, "text": node.text,
            "confidence": 1.0, "grounds": grounds,
        })));
    }
    for edge in &gold.edges {
        let source = gold.nodes.iter().find(|node| node.id == edge.src).unwrap();
        let row = source.grounds.first().unwrap();
        ops.push(Op(json!({
            "op": "add_edge", "src": edge.src, "dst": edge.dst, "kind": edge.kind,
            "confidence": 1.0, "grounds": [{"uid": format!("u{row}")}],
        })));
    }
    let inputs: Vec<_> = input_ids.iter().map(String::as_str).collect();
    let result = graph
        .apply_patch(sid, PatchSource::Flush, None, &ops, &inputs, &[])
        .unwrap();
    assert!(result.rejected.is_empty());
    drop(graph);

    let graph = GraphStore::open(&dir.path().join("sessions.sqlite")).unwrap();
    let persisted = graph.load_graph(sid).unwrap();
    let scores = evaluate(&persisted, &gold, &persisted, &[]);
    assert_eq!(scores.node_precision, 1.0);
    assert_eq!(scores.node_recall, 1.0);
    assert_eq!(scores.edge_f1, 1.0);
    assert_eq!(graph.pending_count(sid).unwrap(), 0);
}

#[test]
fn imports_batch_in_transcript_row_order() {
    let (_dir, session, mut graph) = stores();
    let sid = create_session(&session);
    for text in ["Synthetic first", "Synthetic second", "Synthetic third"] {
        append(&session, sid, text);
    }
    graph
        .conn
        .execute(
            "UPDATE transcript_segments SET is_retranscribed = 1 WHERE id = 2",
            [],
        )
        .unwrap();
    assert_eq!(graph.import_utterances(sid).unwrap(), 3);
    let pending = graph.pending_utterances(sid).unwrap();
    let ids: Vec<_> = pending.iter().map(|u| (u.uid.as_str(), u.row_id)).collect();
    assert_eq!(ids, [("u1", 1), ("u2", 2), ("u3", 3)]);
    let retranscribed: Vec<_> = pending.iter().map(|u| u.is_retranscribed).collect();
    assert_eq!(retranscribed, [false, true, false]);
}

#[test]
fn loads_utterance_without_transcript_row() {
    let (_dir, session, mut graph) = stores();
    let sid = create_session(&session);
    append(&session, sid, "Synthetic turn");
    graph.import_utterances(sid).unwrap();
    graph
        .conn
        .execute(
            "DELETE FROM transcript_segments WHERE session_id = ?1",
            [sid.to_string()],
        )
        .unwrap();
    assert!(graph.load_graph(sid).unwrap().utterances["u1"]
        .text
        .is_empty());
}

fn seed_merge_graph(session: &SessionStore, graph: &mut GraphStore) -> SessionId {
    let sid = create_session(session);
    append(session, sid, "Synthetic first");
    append(session, sid, "Synthetic second");
    graph.import_utterances(sid).unwrap();
    let ops = parse_ops(r#"{"ops":[
        {"op":"add_node","tmp":"from","kind":"issue","text":"First question","confidence":1,"grounds":[{"uid":"u1"}]},
        {"op":"add_node","tmp":"into","kind":"issue","text":"Second question","confidence":1,"grounds":[{"uid":"u2"}]},
        {"op":"add_node","tmp":"position","kind":"position","text":"Synthetic proposal","confidence":1,"grounds":[{"uid":"u1"}]},
        {"op":"add_edge","src":"position","dst":"from","kind":"responds_to","confidence":1,"grounds":[{"uid":"u1"}]},
        {"op":"add_edge","src":"position","dst":"into","kind":"responds_to","confidence":1,"grounds":[{"uid":"u2"}]}
    ]}"#).unwrap();
    let result = graph
        .apply_patch(sid, PatchSource::Live, None, &ops, &["u1", "u2"], &[])
        .unwrap();
    assert_eq!(result.applied.len(), 5, "{:?}", result.rejected);
    sid
}

#[test]
fn persists_merge_status_redirect_and_grounds() {
    let (dir, session, mut graph) = stores();
    let sid = seed_merge_graph(&session, &mut graph);
    let ops = parse_ops(
        r#"{"ops":[
        {"op":"merge","from":"n1","into":"n2","grounds":[{"uid":"u2"}]},
        {"op":"set_status","nid":"n2","status":"resolved","grounds":[{"uid":"u2"}]}
    ]}"#,
    )
    .unwrap();
    let result = graph
        .apply_patch(sid, PatchSource::Consolidate, None, &ops, &[], &["u2"])
        .unwrap();
    assert_eq!(result.applied.len(), 2, "{:?}", result.rejected);
    drop(graph);
    let persisted = GraphStore::open(&dir.path().join("sessions.sqlite"))
        .unwrap()
        .load_graph(sid)
        .unwrap();
    let mut expected = result.graph;
    // SQLite returns grounds ordered by target and utterance ID.
    expected
        .grounds
        .sort_by(|a, b| (&a.target, &a.uid).cmp(&(&b.target, &b.uid)));
    assert_eq!(persisted, expected);
    assert_eq!(persisted.nodes["n1"].status, Status::Merged);
    assert_eq!(persisted.nodes["n1"].merged_into.as_deref(), Some("n2"));
    assert_eq!(persisted.nodes["n2"].status, Status::Resolved);
    assert_eq!(persisted.edges.len(), 1);
    assert_eq!(persisted.edges["e1"].dst, "n2");
    assert!(persisted
        .grounds
        .iter()
        .any(|g| g.target == "n2" && g.uid == "u1"));
    assert!(persisted
        .grounds
        .iter()
        .all(|g| g.target != "n1" && g.target != "e2"));
}

#[test]
fn cannot_review_a_decided_node_or_edge_again() {
    let (_dir, session, mut graph) = stores();
    let sid = seed_merge_graph(&session, &mut graph);
    for target in [ReviewTarget::Node("n1"), ReviewTarget::Edge("e1")] {
        let rev = graph
            .set_review(sid, target, ReviewDecision::Approve)
            .unwrap();
        let approved = graph.load_graph(sid).unwrap();
        assert!(matches!(
            graph.set_review(sid, target, ReviewDecision::Reject),
            Err(StoreError::UnreviewableTarget)
        ));
        assert_eq!(graph.revision(sid).unwrap(), rev);
        assert_eq!(graph.load_graph(sid).unwrap(), approved);
    }
}

#[test]
fn stores_and_reloads_sub_issue_edges() {
    let (_dir, session, mut store) = stores();
    let sid = create_session(&session);
    append(&session, sid, "Synthetic agenda and detail");
    store.import_utterances(sid).unwrap();
    let ops = parse_ops(
        r#"{"ops":[
            {"op":"add_node","tmp":"agenda","kind":"issue","text":"Synthetic agenda","confidence":1,"grounds":[{"uid":"u1"}]},
            {"op":"add_node","tmp":"detail","kind":"issue","text":"Synthetic detail","confidence":1,"grounds":[{"uid":"u1"}]},
            {"op":"add_edge","src":"detail","dst":"agenda","kind":"sub_issue_of","confidence":1,"grounds":[{"uid":"u1"}]}
        ]}"#,
    )
    .unwrap();
    store
        .apply_patch(sid, PatchSource::Live, None, &ops, &["u1"], &[])
        .unwrap();

    let graph = store.load_graph(sid).unwrap();
    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges["e1"].kind, crate::EdgeKind::SubIssueOf);
    assert_eq!(graph.issue_parents(), [("n2", "n1")].into());
}

#[test]
fn issue_consolidation_rejects_live_and_flush_without_writing() {
    let (_dir, session, mut store) = stores();
    let sid = create_session(&session);
    append(&session, sid, "Synthetic question");
    store.import_utterances(sid).unwrap();
    let before = store.load_graph(sid).unwrap();
    let ops = add_issue();

    for source in [PatchSource::Live, PatchSource::Flush] {
        let result = store.apply_patch_with_context(
            sid,
            source,
            None,
            &ops,
            &["u1"],
            &[],
            PatchContext::IssueConsolidation,
        );
        assert!(
            matches!(result, Err(StoreError::InvalidPatchSource)),
            "{source:?}"
        );
        assert_eq!(store.load_graph(sid).unwrap(), before);
        assert_eq!(store.pending_count(sid).unwrap(), 1);
    }
}

#[test]
fn issue_consolidation_replaces_a_stored_parent() {
    let (dir, session, mut store) = stores();
    let sid = create_session(&session);
    append(&session, sid, "Synthetic first turn");
    store.import_utterances(sid).unwrap();
    let initial = parse_ops(
        r#"{"ops":[
            {"op":"add_node","tmp":"old","kind":"issue","text":"Old parent","confidence":0.8,"grounds":[{"uid":"u1"}]},
            {"op":"add_node","tmp":"new","kind":"issue","text":"New parent","confidence":0.8,"grounds":[{"uid":"u1"}]},
            {"op":"add_node","tmp":"child","kind":"issue","text":"Child","confidence":0.8,"grounds":[{"uid":"u1"}]},
            {"op":"add_edge","src":"child","dst":"old","kind":"sub_issue_of","confidence":0.8,"grounds":[{"uid":"u1"}]}
        ]}"#,
    )
    .unwrap();
    let initial = store
        .apply_patch(sid, PatchSource::Flush, None, &initial, &["u1"], &[])
        .unwrap();
    assert!(initial.rejected.is_empty());
    let ops = parse_ops(
        r#"{"ops":[
            {"op":"remove_edge","src":"n3","dst":"n1","kind":"sub_issue_of","grounds":[{"uid":"u1"}]},
            {"op":"add_edge","src":"n3","dst":"n2","kind":"sub_issue_of","confidence":0.8,"grounds":[{"uid":"u1"}]}
        ]}"#,
    )
    .unwrap();
    let result = store
        .apply_patch_with_context(
            sid,
            PatchSource::Consolidate,
            None,
            &ops,
            &[],
            &["u1"],
            PatchContext::IssueConsolidation,
        )
        .unwrap();
    assert!(result.rejected.is_empty());
    assert_eq!(result.applied.len(), 2);
    drop(store);

    let reopened = GraphStore::open(&dir.path().join("sessions.sqlite")).unwrap();
    let snapshot = reopened.load_graph(sid).unwrap();
    assert_eq!(snapshot.rev, result.graph.rev);
    assert_eq!(snapshot.issue_parents().get("n3"), Some(&"n2"));
    assert!(!snapshot.edges.contains_key("e1"));
    assert!(snapshot.grounds.iter().all(|ground| ground.target != "e1"));
}

#[test]
fn consolidation_completion_survives_flush_and_reopening_until_deletion() {
    let (dir, session, mut graph) = stores();
    let sid = create_session(&session);
    assert_eq!(graph.consolidation_completed(sid).unwrap(), None);

    graph
        .apply_patch(sid, PatchSource::Consolidate, None, &[], &[], &[])
        .unwrap();
    assert!(graph.is_consolidated(sid).unwrap());
    assert_eq!(graph.consolidation_completed(sid).unwrap(), None);

    graph
        .apply_patch(sid, PatchSource::Consolidate, None, &[], &[], &[])
        .unwrap();
    graph.mark_consolidation_completed(sid).unwrap();
    assert_eq!(graph.consolidation_completed(sid).unwrap(), Some(2));

    graph
        .apply_patch(sid, PatchSource::Flush, None, &[], &[], &[])
        .unwrap();
    assert!(!graph.is_consolidated(sid).unwrap());
    assert_eq!(graph.consolidation_completed(sid).unwrap(), Some(2));

    let path = dir.path().join("sessions.sqlite");
    drop(graph);
    let mut graph = GraphStore::open(&path).unwrap();
    assert_eq!(graph.consolidation_completed(sid).unwrap(), Some(2));

    graph
        .apply_patch(sid, PatchSource::Consolidate, None, &[], &[], &[])
        .unwrap();
    graph.mark_consolidation_completed(sid).unwrap();
    assert_eq!(graph.consolidation_completed(sid).unwrap(), Some(4));

    graph.delete_session(sid).unwrap();
    assert_eq!(graph.consolidation_completed(sid).unwrap(), None);
    drop(graph);
    let graph = GraphStore::open(&path).unwrap();
    assert_eq!(graph.consolidation_completed(sid).unwrap(), None);
}

#[test]
fn opening_an_older_database_creates_the_consolidation_completion_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite");
    let session = SessionStore::open(&path).unwrap();
    let sid = create_session(&session);
    let (old_schema, _) = include_str!("schema.sql")
        .split_once("CREATE TABLE IF NOT EXISTS graph_consolidations")
        .unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(old_schema).unwrap();
    drop(conn);

    let graph = GraphStore::open(&path).unwrap();
    assert_eq!(graph.consolidation_completed(sid).unwrap(), None);
}

fn cloud_analysis<'a>(subject: &'a str, rev: u64, body: &'a str) -> NewAnalysis<'a> {
    let mut analysis = NewAnalysis::new(AnalysisKind::Cloud, subject, rev, body);
    analysis.provider_model = Some("synthetic-model");
    analysis.dropped = 2;
    analysis.downgraded = 1;
    analysis
}

#[test]
fn saving_an_analysis_with_the_same_key_replaces_it() {
    let (_dir, session, mut graph) = stores();
    let sid = create_session(&session);
    assert!(graph
        .analysis(sid, AnalysisKind::Cloud, "i1:p1:p2")
        .unwrap()
        .is_none());
    graph
        .save_analysis(sid, &cloud_analysis("i1:p1:p2", 3, r#"{"v":1}"#))
        .unwrap();
    graph
        .save_analysis(sid, &cloud_analysis("i1:p1:p3", 3, r#"{"v":"other"}"#))
        .unwrap();
    graph
        .save_analysis(
            sid,
            &NewAnalysis::new(AnalysisKind::Crt, "i1:p1:p2", 3, "{}"),
        )
        .unwrap();
    graph
        .save_analysis(sid, &cloud_analysis("i1:p1:p2", 5, r#"{"v":2}"#))
        .unwrap();

    let saved = graph
        .analysis(sid, AnalysisKind::Cloud, "i1:p1:p2")
        .unwrap()
        .unwrap();
    assert_eq!(saved.body_json, r#"{"v":2}"#);
    assert_eq!(saved.based_on_rev, 5);
    assert_eq!(saved.provider_model.as_deref(), Some("synthetic-model"));
    assert_eq!((saved.dropped, saved.downgraded), (2, 1));
    assert!(!saved.created_at.is_empty());
    let cloud: Vec<_> = graph
        .analyses(sid, AnalysisKind::Cloud)
        .unwrap()
        .into_iter()
        .map(|analysis| analysis.subject)
        .collect();
    assert_eq!(cloud, ["i1:p1:p2", "i1:p1:p3"]);
    assert_eq!(graph.analyses(sid, AnalysisKind::Crt).unwrap().len(), 1);
}

#[test]
fn deleting_an_analysis_removes_only_that_row() {
    let (_dir, session, mut graph) = stores();
    let sid = create_session(&session);
    for subject in ["i1:p1:p2", "i1:p1:p3"] {
        graph
            .save_analysis(sid, &cloud_analysis(subject, 1, "{}"))
            .unwrap();
    }
    assert!(graph
        .delete_analysis(sid, AnalysisKind::Cloud, "i1:p1:p2")
        .unwrap());
    assert!(!graph
        .delete_analysis(sid, AnalysisKind::Cloud, "i1:p1:p2")
        .unwrap());
    assert_eq!(graph.analyses(sid, AnalysisKind::Cloud).unwrap().len(), 1);
}

#[test]
fn delete_session_removes_its_analyses_and_keeps_other_sessions() {
    let (_dir, session, mut graph) = stores();
    let first = create_session(&session);
    let second = create_session(&session);
    for sid in [first, second] {
        graph
            .save_analysis(sid, &cloud_analysis("i1:p1:p2", 1, "{}"))
            .unwrap();
        graph
            .save_analysis(sid, &NewAnalysis::new(AnalysisKind::Crt, "i1", 1, "{}"))
            .unwrap();
    }
    graph.delete_session(first).unwrap();
    for kind in [AnalysisKind::Cloud, AnalysisKind::Crt] {
        assert!(graph.analyses(first, kind).unwrap().is_empty());
        assert_eq!(graph.analyses(second, kind).unwrap().len(), 1);
    }
}

#[test]
fn opening_an_older_database_creates_the_analyses_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite");
    let session = SessionStore::open(&path).unwrap();
    let sid = create_session(&session);
    let (old_schema, _) = include_str!("schema.sql")
        .split_once("CREATE TABLE IF NOT EXISTS graph_analyses")
        .unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(old_schema).unwrap();
    drop(conn);

    let mut graph = GraphStore::open(&path).unwrap();
    assert!(graph.analyses(sid, AnalysisKind::Cloud).unwrap().is_empty());
    graph
        .save_analysis(sid, &cloud_analysis("i1:p1:p2", 1, "{}"))
        .unwrap();
}
