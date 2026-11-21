mod common;

use discussion_graph::eval::{evaluate, GoldEdge, GoldGraph, GoldNode};
use discussion_graph::{
    detect_gaps, graph_updated_subject, AppliedOp, Edge, EdgeKind, Graph, Ground, NodeKind, Review,
    Status, Utterance,
};
use serde::Deserialize;

use common::{node, utterance};

fn gold_node(id: &str, kind: NodeKind, row: i64) -> GoldNode {
    GoldNode {
        id: id.into(),
        kind,
        text: format!("Synthetic {id}"),
        status: Status::Open,
        grounds: vec![row],
    }
}

fn gold_edge(src: &str, dst: &str, kind: EdgeKind) -> GoldEdge {
    GoldEdge {
        src: src.into(),
        dst: dst.into(),
        kind,
    }
}

fn graph_from_gold(utterances: Vec<Utterance>, gold: &GoldGraph) -> Graph {
    let mut graph = Graph {
        utterances: utterances.into_iter().map(|u| (u.uid.clone(), u)).collect(),
        ..Graph::default()
    };
    for n in &gold.nodes {
        graph.nodes.insert(
            n.id.clone(),
            discussion_graph::Node {
                text: n.text.clone(),
                status: n.status,
                ..node(&n.id, n.kind)
            },
        );
        for row in &n.grounds {
            graph.grounds.push(Ground {
                target: n.id.clone(),
                uid: format!("u{row}"),
                act: None,
            });
        }
    }
    for (i, e) in gold.edges.iter().enumerate() {
        let eid = format!("e{}", i + 1);
        graph.edges.insert(
            eid.clone(),
            Edge {
                eid,
                src: e.src.clone(),
                dst: e.dst.clone(),
                kind: e.kind,
                confidence: 1.0,
                review: Review::Auto,
                created_rev: 1,
            },
        );
    }
    graph
}

#[test]
fn metrics_match_hand_calculated_counts() {
    use EdgeKind::*;
    use NodeKind::*;
    let gold = GoldGraph {
        nodes: vec![
            gold_node("a", Issue, 1),
            gold_node("b", Position, 2),
            gold_node("c", Issue, 3),
        ],
        edges: vec![
            gold_edge("b", "a", RespondsTo),
            gold_edge("b", "c", RespondsTo),
        ],
    };
    let prediction = GoldGraph {
        nodes: vec![
            gold_node("n1", Issue, 1),
            gold_node("n2", Position, 2),
            gold_node("n3", Issue, 4),
            gold_node("n4", Argument, 3),
        ],
        edges: vec![
            gold_edge("n2", "n1", RespondsTo),
            gold_edge("n2", "n3", RespondsTo),
            gold_edge("n4", "n2", Supports),
        ],
    };
    let utterances: Vec<_> = (1..=4).map(|i| utterance(i, String::new())).collect();
    let prediction = graph_from_gold(utterances, &prediction);
    let mut before = prediction.clone();
    before.nodes.insert("n5".into(), node("n5", Issue));
    before.grounds.push(Ground {
        target: "n5".into(),
        uid: "u3".into(),
        act: None,
    });
    let merge = |from: &str, into: &str| AppliedOp::Merge {
        from: from.into(),
        into: into.into(),
        grounds: Vec::new(),
    };
    let result = evaluate(
        &prediction,
        &gold,
        &before,
        &[merge("n5", "n1"), merge("n1", "n3"), merge("n4", "n2")],
    );
    assert_eq!(result.node_precision, 0.5);
    assert_eq!(result.node_recall, 2.0 / 3.0);
    assert_eq!(result.edge_f1, 0.4);
    assert_eq!(result.live_nodes, 4);
    assert_eq!(result.wrong_merges, 2);
}

#[derive(Deserialize)]
struct RowId {
    row_id: i64,
}

macro_rules! fixture {
    ($name:ident, $file:literal) => {
        #[test]
        fn $name() {
            let gold: GoldGraph =
                serde_json::from_str(include_str!(concat!("../testdata/", $file, ".gold.json")))
                    .unwrap();
            let utterances = include_str!(concat!("../testdata/", $file, ".jsonl"))
                .lines()
                .map(|line| {
                    let id: RowId = serde_json::from_str(line).unwrap();
                    let mut row: serde_json::Value = serde_json::from_str(line).unwrap();
                    row["uid"] = format!("u{}", id.row_id).into();
                    serde_json::from_value(row).unwrap()
                })
                .collect();
            let graph = graph_from_gold(utterances, &gold);
            assert!(graph
                .grounds
                .iter()
                .all(|g| graph.utterances.contains_key(&g.uid)));
            assert!(graph
                .edges
                .values()
                .all(|e| graph.nodes.contains_key(&e.src) && graph.nodes.contains_key(&e.dst)));
            let metrics = evaluate(&graph, &gold, &graph, &[]);
            assert_eq!(metrics.node_recall, 1.0);
            assert_eq!(metrics.node_precision, 1.0);
            assert_eq!(metrics.edge_f1, 1.0);
        }
    };
}

fixture!(m1, "m1");
fixture!(m2, "m2");
fixture!(m3, "m3");
fixture!(budget, "budget");
fixture!(design, "design");
fixture!(one_on_one, "one-on-one");

#[test]
fn detects_four_gaps_and_checks_decisions_after_flush() {
    use EdgeKind::*;
    use NodeKind::*;
    let gold = GoldGraph {
        nodes: vec![
            gold_node("i1", Issue, 1),
            gold_node("i2", Issue, 2),
            gold_node("p", Position, 3),
            gold_node("a", Argument, 4),
            gold_node("d", Decision, 5),
        ],
        edges: vec![
            gold_edge("p", "i1", RespondsTo),
            gold_edge("a", "p", ObjectsTo),
            gold_edge("d", "i1", Resolves),
        ],
    };
    let graph = graph_from_gold(Vec::new(), &gold);
    let gaps = detect_gaps(&graph, true);
    assert_eq!(gaps.unanswered_issues, ["i2"]);
    assert_eq!(gaps.one_sided_positions, ["p"]);
    assert_eq!(gaps.issues_without_decision, ["i2"]);
    assert_eq!(gaps.decisions_without_action, ["d"]);
    assert!(detect_gaps(&graph, false)
        .issues_without_decision
        .is_empty());
    assert_eq!(graph_updated_subject("test"), "graph.test.updated");
}

#[test]
fn merging_duplicates_into_the_node_with_more_grounds_is_not_an_error() {
    use discussion_graph::{apply_patch, Op};
    use serde_json::json;

    let gold = GoldGraph {
        nodes: vec![gold_node("g1", NodeKind::Issue, 1)],
        edges: vec![],
    };
    let mut larger = gold_node("n2", NodeKind::Issue, 1);
    larger.grounds.extend([2, 3]);
    let before = graph_from_gold(
        (1..=3).map(|i| utterance(i, String::new())).collect(),
        &GoldGraph {
            nodes: vec![gold_node("n1", NodeKind::Issue, 1), larger],
            edges: vec![],
        },
    );
    let merged = apply_patch(
        &before,
        &[Op(json!({
            "op": "merge", "from": "n1", "into": "n2",
            "grounds": [{"uid": "u1"}]
        }))],
        &["u1"],
    );
    assert_eq!(merged.applied.len(), 1);
    assert_eq!(
        evaluate(&merged.graph, &gold, &before, &merged.applied).wrong_merges,
        0
    );
}

#[test]
fn evaluates_agendas_and_shared_nodes_without_sub_question_branches() {
    use EdgeKind::*;
    use NodeKind::*;
    let gold = GoldGraph {
        nodes: vec![
            gold_node("agenda", Issue, 1),
            gold_node("shared", Position, 4),
        ],
        edges: vec![gold_edge("shared", "agenda", RespondsTo)],
    };
    let prediction = GoldGraph {
        nodes: vec![
            gold_node("agenda", Issue, 1),
            gold_node("detail", Issue, 1),
            gold_node("nested", Issue, 2),
            gold_node("private", Position, 4),
            gold_node("shared", Position, 4),
            gold_node("reason", Argument, 5),
        ],
        edges: vec![
            gold_edge("detail", "agenda", SubIssueOf),
            gold_edge("nested", "detail", SubIssueOf),
            gold_edge("private", "nested", RespondsTo),
            gold_edge("shared", "nested", RespondsTo),
            gold_edge("shared", "agenda", RespondsTo),
            gold_edge("reason", "private", ObjectsTo),
        ],
    };
    let graph = graph_from_gold(
        (1..=5).map(|i| utterance(i, "Synthetic".into())).collect(),
        &prediction,
    );
    let score = evaluate(&graph, &gold, &graph, &[]);
    assert_eq!(score.node_precision, 1.0);
    assert_eq!(score.node_recall, 1.0);
    assert_eq!(score.edge_f1, 1.0);
    assert_eq!(score.agendas, 1);
    assert_eq!(score.sub_questions, 2);
    assert_eq!(score.live_nodes, 6);
}

#[test]
fn outlines_nested_issues_and_counts_unanswered_questions_by_agenda() {
    use EdgeKind::*;
    use NodeKind::*;
    let data = GoldGraph {
        nodes: vec![
            gold_node("agenda", Issue, 1),
            gold_node("detail", Issue, 2),
            gold_node("nested", Issue, 3),
            gold_node("position", Position, 4),
            gold_node("argument", Argument, 5),
            gold_node("objection", Argument, 6),
        ],
        edges: vec![
            gold_edge("detail", "agenda", SubIssueOf),
            gold_edge("nested", "detail", SubIssueOf),
            gold_edge("position", "nested", RespondsTo),
            gold_edge("argument", "position", Supports),
            gold_edge("objection", "argument", ObjectsTo),
            gold_edge("argument", "objection", Supports),
        ],
    };
    let graph = graph_from_gold(vec![], &data);
    assert_eq!(
        discussion_graph::unanswered_by_agenda(&graph),
        [("agenda".into(), 2)].into()
    );
    let outline = discussion_graph::build_outline(&graph);
    assert_eq!(outline.len(), 1);
    let mut branch = &outline[0];
    for (id, relation) in [
        ("agenda", None),
        ("detail", Some(SubIssueOf)),
        ("nested", Some(SubIssueOf)),
        ("position", Some(RespondsTo)),
        ("argument", Some(Supports)),
        ("objection", Some(ObjectsTo)),
    ] {
        assert_eq!(branch.node.nid, id);
        assert_eq!(branch.relation, relation);
        if id == "objection" {
            assert!(branch.children.is_empty());
        } else {
            assert_eq!(branch.children.len(), 1);
            branch = &branch.children[0];
        }
    }
}
