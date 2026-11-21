use discussion_graph::{apply_patch, parse_ops, Graph, Rejection};

#[test]
fn merge_rejects_different_kinds() {
    let ops = parse_ops(r#"{"ops":[
        {"op":"add_node","tmp":"a","kind":"issue","text":"Question","confidence":1,"grounds":[{"uid":"u1"}]},
        {"op":"add_node","tmp":"b","kind":"position","text":"Proposal","confidence":1,"grounds":[{"uid":"u1"}]},
        {"op":"merge","from":"a","into":"b","grounds":[{"uid":"u1"}]}
    ]}"#).unwrap();
    let result = apply_patch(&Graph::default(), &ops, &["u1"]);
    assert_eq!(result.applied.len(), 2);
    assert_eq!(result.rejected[&Rejection::MergeKind], 1);
}

use discussion_graph::{AppliedOp, Op, ParseError, Review, Status};
use serde_json::{json, Value};

fn add(tmp: &str, kind: &str) -> Value {
    json!({"op":"add_node","tmp":tmp,"kind":kind,"text":"Synthetic",
           "confidence":0.5,"grounds":[{"uid":"u1"}]})
}

fn edge(src: &str, dst: &str, kind: &str) -> Value {
    json!({"op":"add_edge","src":src,"dst":dst,"kind":kind,
           "confidence":1,"grounds":[{"uid":"u1"}]})
}

fn merge(from: &str, into: &str) -> Value {
    json!({"op":"merge","from":from,"into":into,"grounds":[{"uid":"u2","act":"rephrases"}]})
}

fn status(nid: &str, status: &str) -> Value {
    json!({"op":"set_status","nid":nid,"status":status,"grounds":[{"uid":"u2"}]})
}

fn apply(graph: &Graph, ops: impl IntoIterator<Item = Value>) -> discussion_graph::PatchResult {
    apply_patch(
        graph,
        &ops.into_iter().map(Op).collect::<Vec<_>>(),
        &["u1", "u2"],
    )
}

fn reject(graph: &Graph, op: Value, reason: Rejection) {
    let result = apply(graph, [op]);
    assert!(result.applied.is_empty());
    assert!(result.touched.is_empty());
    assert_eq!(result.rejected, [(reason, 1)].into());
}

fn with_field(mut op: Value, key: &str, value: Value) -> Value {
    op[key] = value;
    op
}

#[test]
fn parses_first_object_with_fences_prose_and_braces_in_text() {
    let parsed = parse_ops(
        "先に {JSON} の説明\n```json\n{\"ops\":[{\"text\":\"}\"}]}\n```\n補足 {\"ops\":[]}",
    )
    .unwrap();
    assert_eq!(parsed, vec![Op(json!({"text":"}"}))]);
    assert_eq!(parse_ops("nothing here"), Err(ParseError::NoObject));
    assert_eq!(parse_ops("{\"ops\":[}"), Err(ParseError::InvalidJson));
    assert_eq!(parse_ops("{\"ops\":null}"), Err(ParseError::InvalidOps));
    assert_eq!(parse_ops("{} {\"ops\":[]}"), Err(ParseError::InvalidOps));
}

macro_rules! rejects_add {
    ($name:ident, $field:literal, $value:expr, $reason:ident) => {
        #[test]
        fn $name() {
            reject(
                &Graph::default(),
                with_field(add("a", "issue"), $field, $value),
                Rejection::$reason,
            );
        }
    };
}

rejects_add!(rejects_empty_grounds, "grounds", json!([]), EmptyGrounds);
rejects_add!(
    rejects_grounds_outside_input,
    "grounds",
    json!([{"uid":"u42"}]),
    GroundOutsideInput
);
rejects_add!(
    rejects_unknown_act,
    "grounds",
    json!([{"uid":"u1","act":"quotes"}]),
    UnknownAct
);
rejects_add!(
    rejects_unknown_operation,
    "op",
    json!("remove_node"),
    UnknownOperation
);
rejects_add!(
    rejects_confidence_above_one,
    "confidence",
    json!(1.1),
    Confidence
);
rejects_add!(
    rejects_negative_confidence,
    "confidence",
    json!(-0.1),
    Confidence
);
rejects_add!(
    rejects_unknown_node_kind,
    "kind",
    json!("cause"),
    UnknownNodeKind
);
rejects_add!(rejects_blank_text, "text", json!("  "), EmptyText);

#[test]
fn rejects_duplicate_tmp_and_references_to_dropped_tmp() {
    let result = apply(
        &Graph::default(),
        [
            add("a", "issue"),
            add("a", "position"),
            add("b", "unknown"),
            edge("b", "a", "responds_to"),
            add("c", "position"),
            edge("c", "a", "responds_to"),
            status("b", "open"),
            merge("b", "c"),
        ],
    );
    assert_eq!(result.applied.len(), 3);
    assert_eq!(
        result.rejected,
        [
            (Rejection::DuplicateTmp, 1),
            (Rejection::UnknownNodeKind, 1),
            (Rejection::EdgeEndpoint, 1),
            (Rejection::StatusEndpoint, 1),
            (Rejection::MergeEndpoint, 1),
        ]
        .into()
    );
    assert_eq!(result.graph.edges["e1"].src, "n2");
    assert_eq!(result.graph.edges["e1"].dst, "n1");
}

fn graph() -> Graph {
    apply(
        &Graph::default(),
        [
            add("a", "issue"),
            add("b", "position"),
            add("c", "argument"),
            add("d", "argument"),
            add("e", "argument"),
            add("f", "decision"),
            add("g", "action"),
            edge("b", "a", "responds_to"),
        ],
    )
    .graph
}

#[test]
fn rejects_missing_merged_and_rejected_edge_endpoints() {
    let mut graph = graph();
    reject(
        &graph,
        edge("missing", "n2", "supports"),
        Rejection::EdgeEndpoint,
    );
    graph.nodes.get_mut("n3").unwrap().status = Status::Merged;
    reject(
        &graph,
        edge("n3", "n2", "supports"),
        Rejection::EdgeEndpoint,
    );
    graph.nodes.get_mut("n2").unwrap().review = Review::Rejected;
    reject(
        &graph,
        edge("n4", "n2", "supports"),
        Rejection::EdgeEndpoint,
    );
}

#[test]
fn rejects_invalid_edge_kind_pair() {
    let graph = graph();
    for (src, dst, kind) in [
        ("n1", "n2", "responds_to"),
        ("n3", "n1", "supports"),
        ("n2", "n3", "objects_to"),
        ("n6", "n2", "resolves"),
        ("n7", "n3", "assigns"),
        ("n3", "n2", "causes"),
    ] {
        reject(&graph, edge(src, dst, kind), Rejection::EdgeKind);
    }
}

#[test]
fn accepts_each_edge_kind_and_endpoint_pair() {
    let result = apply(
        &graph(),
        [
            edge("n3", "n2", "supports"),
            edge("n4", "n3", "supports"),
            edge("n3", "n2", "objects_to"),
            edge("n4", "n3", "objects_to"),
            edge("n6", "n1", "resolves"),
            edge("n7", "n1", "assigns"),
            edge("n7", "n6", "assigns"),
        ],
    );
    assert_eq!(result.applied.len(), 7);
    assert!(result.rejected.is_empty());
}

#[test]
fn rejects_self_loop() {
    reject(
        &graph(),
        edge("n3", "n3", "supports"),
        Rejection::EdgeSelfLoop,
    );
}

#[test]
fn rejects_duplicate_edge() {
    reject(
        &graph(),
        edge("n2", "n1", "responds_to"),
        Rejection::EdgeDuplicate,
    );
}

#[test]
fn rejects_self_merge() {
    reject(&graph(), merge("n3", "n3"), Rejection::MergeSameNode);
}

#[test]
fn rejects_merge_to_or_from_missing_or_merged_node() {
    let result = apply(
        &graph(),
        [
            merge("n3", "n4"),
            merge("n3", "n5"),
            merge("n4", "n3"),
            merge("missing", "n4"),
            merge("n4", "missing"),
        ],
    );
    assert_eq!(result.rejected, [(Rejection::MergeEndpoint, 4)].into());
    assert_eq!(result.applied.len(), 1);
}

#[test]
fn protects_reviewed_merge_sources_and_their_edges() {
    for review in [Review::Approved, Review::Rejected] {
        let mut graph = graph();
        graph.nodes.get_mut("n3").unwrap().review = review;
        reject(&graph, merge("n3", "n4"), Rejection::MergeReviewedNode);
        graph.nodes.get_mut("n3").unwrap().review = Review::Auto;
        graph = apply(&graph, [edge("n3", "n2", "supports")]).graph;
        graph.edges.get_mut("e2").unwrap().review = review;
        reject(&graph, merge("n3", "n4"), Rejection::MergeReviewedEdge);
    }
}

#[test]
fn rejects_rejected_merge_target_and_allows_approved_target() {
    let mut graph = graph();
    graph.nodes.get_mut("n4").unwrap().review = Review::Rejected;
    reject(&graph, merge("n3", "n4"), Rejection::MergeRejectedTarget);

    graph.nodes.get_mut("n4").unwrap().review = Review::Approved;
    let result = apply(&graph, [merge("n3", "n4")]);
    assert_eq!(result.applied.len(), 1);
    assert_eq!(result.graph.nodes["n4"].review, Review::Approved);
}

#[test]
fn merge_redirects_edges_grounds_and_earlier_aliases_without_duplicates() {
    let graph = apply(
        &graph(),
        [
            edge("n3", "n2", "supports"),
            edge("n4", "n2", "supports"),
            edge("n3", "n4", "supports"),
            edge("n5", "n3", "objects_to"),
        ],
    )
    .graph;
    let result = apply(&graph, [merge("n3", "n4"), merge("n4", "n5")]);
    assert!(result.rejected.is_empty());
    assert_eq!(result.graph.nodes["n3"].merged_into.as_deref(), Some("n5"));
    assert_eq!(result.graph.nodes["n4"].merged_into.as_deref(), Some("n5"));
    assert_eq!(
        result.graph.edges.keys().collect::<Vec<_>>(),
        vec!["e1", "e3"]
    );
    assert_eq!(result.graph.edges["e3"].src, "n5");
    assert_eq!(result.graph.edges["e3"].dst, "n2");
    assert!(result
        .graph
        .grounds
        .iter()
        .all(|g| !["e2", "e4", "e5", "n3", "n4"].contains(&g.target.as_str())));
    let grounds = result
        .graph
        .grounds
        .iter()
        .filter(|g| g.target == "n5")
        .collect::<Vec<_>>();
    assert_eq!(grounds.len(), 2);
    assert_eq!(grounds[1].act, Some(discussion_graph::Act::Rephrases));
    assert_eq!(
        result.touched,
        ["n2", "n3", "n4", "n5"].map(str::to_owned).into()
    );
    assert_eq!(result.graph.nodes["n3"].updated_rev, result.graph.rev);
    assert!(
        matches!(&result.applied[0], AppliedOp::Merge {from, into, ..} if from == "n3" && into == "n4")
    );
}

#[test]
fn rejects_status_on_missing_merged_and_reviewed_nodes() {
    let mut graph = graph();
    reject(&graph, status("missing", "open"), Rejection::StatusEndpoint);
    graph.nodes.get_mut("n1").unwrap().status = Status::Merged;
    reject(&graph, status("n1", "open"), Rejection::StatusEndpoint);
    graph.nodes.get_mut("n1").unwrap().status = Status::Open;
    graph.nodes.get_mut("n1").unwrap().review = Review::Rejected;
    reject(&graph, status("n1", "open"), Rejection::StatusEndpoint);
    graph.nodes.get_mut("n1").unwrap().review = Review::Approved;
    reject(&graph, status("n1", "open"), Rejection::StatusReviewedNode);
}

#[test]
fn checks_status_kind_and_disallows_direct_merge_status() {
    let graph = graph();
    for (id, value) in [
        ("n2", "decided"),
        ("n2", "resolved"),
        ("n1", "retracted"),
        ("n1", "unknown"),
    ] {
        reject(&graph, status(id, value), Rejection::StatusKind);
    }
    reject(&graph, status("n1", "merged"), Rejection::StatusMerged);
    let result = apply(
        &graph,
        [
            status("n1", "decided"),
            status("n1", "resolved"),
            status("n2", "retracted"),
            status("n3", "retracted"),
        ],
    );
    assert_eq!(result.applied.len(), 4);
    assert_eq!(result.graph.nodes["n1"].status, Status::Resolved);
    assert!(result
        .graph
        .grounds
        .iter()
        .any(|g| g.target == "n1" && g.uid == "u2"));
}
