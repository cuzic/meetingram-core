mod common;

use discussion_graph::{
    apply_patch, build_consolidate, build_consolidate_groups, build_consolidate_issues,
    build_incremental, parse_ops, ConsolidateStyle, Edge, EdgeKind, Graph, Ground, NodeKind,
    Prompt, PromptError, Review, Status, DEFAULT_INPUT_LIMIT,
};
use serde_json::Value;

use common::{node, utterance};

fn size(prompt: &Prompt) -> usize {
    prompt.system.chars().count() + prompt.user.chars().count()
}

fn section(prompt: &Prompt, name: &str) -> Vec<Value> {
    prompt
        .user
        .lines()
        .skip_while(|line| *line != name)
        .skip(1)
        .take_while(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn incremental_limits_context_and_drops_oldest_and_hidden_nodes() {
    let mut graph = Graph::default();
    for i in 1..=8 {
        let id = format!("n{i}");
        let mut node = node(&id, NodeKind::Issue);
        node.text = "議".repeat(480);
        node.created_rev = i;
        node.updated_rev = 0;
        graph.nodes.insert(id, node);
    }
    graph.nodes.get_mut("n8").unwrap().review = Review::Rejected;
    graph.nodes.get_mut("n7").unwrap().status = Status::Merged;
    graph.nodes.get_mut("n6").unwrap().status = Status::Decided;
    graph.nodes.get_mut("n1").unwrap().updated_rev = 9;
    let recent = [["n6".to_owned(), "n7".to_owned(), "n8".to_owned()].into()];
    let limit = 3_500;
    let prompt = build_incremental(&graph, &[], &recent, false, limit).unwrap();
    let context = section(&prompt, "context");
    let context_chars: usize = context
        .iter()
        .map(|n| n.to_string().chars().count() + 1)
        .sum();
    assert!(size(&prompt) <= limit);
    assert!(context_chars <= limit / 3);
    assert_eq!(
        context
            .iter()
            .map(|n| n["nid"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["n1", "n6"]
    );
    let roomy = build_incremental(&graph, &[], &recent, false, DEFAULT_INPUT_LIMIT).unwrap();
    let context = section(&roomy, "context");
    assert_eq!(context.last().unwrap()["nid"], "n6");
    assert!(!context.iter().any(|n| n["nid"] == "n7" || n["nid"] == "n8"));
}

#[test]
fn incremental_caps_long_new_and_reference_utterances_and_tracks_actual_input() {
    let mut graph = Graph::default();
    for i in 6..=12 {
        let text = if i >= 10 {
            "新しい".repeat(1_000)
        } else {
            "参照用".repeat(100)
        };
        let u = utterance(i, text);
        graph.utterances.insert(u.uid.clone(), u);
    }
    let limit = 4_000;
    let prompt = build_incremental(&graph, &["u12", "u10", "u11"], &[], false, limit).unwrap();
    assert!(size(&prompt) <= limit);
    assert_eq!(prompt.new_uids, ["u10"]);
    assert_eq!(prompt.allowed_uids, ["u7", "u8", "u9", "u10"]);
    assert_eq!(
        section(&prompt, "new")[0]["text"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        limit / 3
    );
    let references = section(&prompt, "reference");
    assert_eq!(references.len(), 3);
    assert!(references
        .iter()
        .all(|r| r["text"].as_str().unwrap().chars().count() == 200));
    assert!(!prompt.system.contains("新しい新しい"));
    assert!(!prompt.system.contains("参照用参照用"));
    let reprint = build_incremental(&graph, &["u10"], &[], true, limit).unwrap();
    assert_eq!(reprint.allowed_uids, ["u10"]);
    assert!(section(&reprint, "reference").is_empty());
    assert_eq!(prompt.system, reprint.system);
}

#[test]
fn incremental_still_includes_a_long_first_utterance_when_space_is_tight() {
    let mut graph = Graph::default();
    let u = utterance(1, "\"".repeat(1000));
    graph.utterances.insert(u.uid.clone(), u);
    let frame = build_incremental(&graph, &[], &[], false, DEFAULT_INPUT_LIMIT).unwrap();
    let limit = size(&frame) + 100;
    let prompt = build_incremental(&graph, &["u1"], &[], false, limit).unwrap();
    assert_eq!(prompt.new_uids, ["u1"]);
    assert!(size(&prompt) <= limit && size(&prompt) >= limit - 1);
    assert_eq!(
        build_incremental(&graph, &["u1"], &[], false, 10),
        Err(PromptError::InputLimit)
    );
}

fn add_ground(graph: &mut Graph, nid: &str, row: i64, length: usize) {
    let u = utterance(row, "根".repeat(length));
    graph.grounds.push(Ground {
        target: nid.into(),
        uid: u.uid.clone(),
        act: None,
    });
    graph.utterances.insert(u.uid.clone(), u);
}

fn graph_with_groups(count: usize) -> Graph {
    let mut graph = Graph::default();
    for i in 1..=count {
        let issue = format!("n{i}");
        let position = format!("p{i}");
        let eid = format!("e{i}");
        let mut question = node(&issue, NodeKind::Issue);
        question.created_rev = i as u64;
        graph.nodes.insert(issue.clone(), question);
        graph
            .nodes
            .insert(position.clone(), node(&position, NodeKind::Position));
        graph.edges.insert(
            eid.clone(),
            Edge {
                eid,
                src: position,
                dst: issue,
                kind: EdgeKind::RespondsTo,
                confidence: 1.0,
                review: Review::Auto,
                created_rev: 1,
            },
        );
    }
    graph
}

fn assert_graph_prompt(prompt: &Prompt, edges: usize, nodes: usize, limit: usize) {
    assert!(size(prompt) <= limit);
    assert_eq!(section(prompt, "graph edges").len(), edges);

    let nodes_in_prompt = section(prompt, "graph nodes");
    assert_eq!(nodes_in_prompt.len(), nodes);
    let uids: std::collections::BTreeSet<_> = nodes_in_prompt
        .iter()
        .flat_map(|n| n["grounds"].as_array().unwrap())
        .map(|g| g["uid"].as_str().unwrap())
        .collect();
    assert_eq!(prompt.allowed_uids, uids.into_iter().collect::<Vec<_>>());
    assert!(prompt.new_uids.is_empty());
    for text in ["Synthetic", "根根"] {
        assert!(!prompt.system.contains(text), "{text}");
    }
}

#[test]
fn baseline_matches_original_prompt_when_it_fits() {
    let mut graph = graph_with_groups(1);
    add_ground(&mut graph, "n1", 1, 1);
    let prompt = build_consolidate(&graph, ConsolidateStyle::Baseline, DEFAULT_INPUT_LIMIT)
        .unwrap()
        .remove(0);
    assert_eq!(prompt.system, include_str!("baseline-system.txt"));
    // Use the original field order with or without serde_json's preserve_order.
    let issue = serde_json::json!({
        "nid":"n1", "kind":"issue", "status":"open", "text":"Synthetic n1",
        "confidence":1.0, "ground_count":1,
        "grounds":[{"uid":"u1","speaker_label":"A","text":"根"}],
    });
    let position = serde_json::json!({
        "nid":"p1", "kind":"position", "status":"open", "text":"Synthetic p1",
        "confidence":1.0, "ground_count":0, "grounds":[],
    });
    let edge = serde_json::json!({
        "eid":"e1", "src":"p1", "dst":"n1", "kind":"responds_to", "confidence":1.0,
    });
    assert_eq!(
        prompt.user,
        format!("graph nodes\n{issue}\n{position}\ngraph edges\n{edge}\n")
    );
    assert_eq!(prompt.allowed_uids, ["u1"]);
}

#[test]
fn issue_stage_uid_only_evidence_still_allows_a_merge() {
    let mut graph = Graph::default();
    for row in 1..=120 {
        let id = format!("n{row}");
        let mut issue = node(&id, NodeKind::Issue);
        issue.text = "問".repeat(40);
        graph.nodes.insert(id.clone(), issue);
        add_ground(&mut graph, &id, row, 400);
    }
    let prompt = build_consolidate_issues(&graph, DEFAULT_INPUT_LIMIT)
        .unwrap()
        .unwrap();
    assert_eq!(prompt.omitted_issues, 0);
    assert_eq!(
        section(&prompt, "graph nodes")[0]["grounds"],
        serde_json::json!([{"uid":"u1"}])
    );
    let allowed: Vec<_> = prompt.allowed_uids.iter().map(String::as_str).collect();
    let ops =
        parse_ops(r#"{"ops":[{"op":"merge","from":"n2","into":"n1","grounds":[{"uid":"u1"}]}]}"#)
            .unwrap();
    let result = apply_patch(&graph, &ops, &allowed);
    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
    assert_eq!(result.applied.len(), 1);
}

#[test]
fn baseline_reduces_evidence_without_losing_edges_or_nodes() {
    let mut graph = graph_with_groups(2);
    for row in 1..=3 {
        add_ground(&mut graph, "n1", row, 400);
    }
    let build = |limit| {
        build_consolidate(&graph, ConsolidateStyle::Baseline, limit)
            .unwrap()
            .remove(0)
    };
    let full = build(DEFAULT_INPUT_LIMIT);
    assert_graph_prompt(&full, 2, 4, DEFAULT_INPUT_LIMIT);
    let limit = size(&full) - 450;
    let reduced = build(limit);
    assert_graph_prompt(&reduced, 2, 4, limit);
    assert_eq!(reduced.allowed_uids, ["u1"]);
    let limit = size(&reduced) - 1;
    assert_graph_prompt(&build(limit), 2, 4, limit);
    assert_eq!(build(limit).allowed_uids, ["u1", "u2", "u3"]);
}

#[test]
fn issue_stage_uses_only_live_issues_and_drops_newest_after_evidence() {
    let mut graph = graph_with_groups(3);
    let mut hidden = node("hidden", NodeKind::Issue);
    hidden.review = Review::Rejected;
    graph.nodes.insert(hidden.nid.clone(), hidden);
    let mut merged = node("merged", NodeKind::Issue);
    merged.status = Status::Merged;
    graph.nodes.insert(merged.nid.clone(), merged);
    add_ground(&mut graph, "n1", 2, 400);
    add_ground(&mut graph, "n1", 1, 400);
    add_ground(&mut graph, "n2", 3, 400);
    add_ground(&mut graph, "n3", 4, 400);
    add_ground(&mut graph, "p1", 5, 400);

    let build = |graph: &Graph, limit| build_consolidate_issues(graph, limit).unwrap().unwrap();
    let full = build(&graph, DEFAULT_INPUT_LIMIT);
    assert_graph_prompt(&full, 0, 3, DEFAULT_INPUT_LIMIT);
    assert!(full.system.contains("issue 同士の merge だけ"));
    assert!(!full.system.contains("reference"));
    assert!(!full.system.contains("add_node"));
    assert!(full.user.find("graph edges").unwrap() < full.user.find("graph nodes").unwrap());
    assert_eq!(full.allowed_uids, ["u1", "u3", "u4"]);
    let issues = section(&full, "graph nodes");
    assert_eq!(issues[0]["ground_count"], 2);
    assert_eq!(issues[0]["grounds"][0]["text"], "根".repeat(100));

    let limit = size(&full) - 1;
    let without_evidence = build(&graph, limit);
    assert_graph_prompt(&without_evidence, 0, 3, limit);
    assert_eq!(without_evidence.omitted_issues, 0);
    assert_eq!(without_evidence.allowed_uids, ["u1", "u2", "u3", "u4"]);

    let limit = size(&without_evidence) - 1;
    let trimmed = build(&graph, limit);
    assert_graph_prompt(&trimmed, 0, 2, limit);
    assert_eq!(trimmed.omitted_issues, 1);
    let issues = section(&trimmed, "graph nodes");
    assert_eq!(issues[0]["nid"], "n1");
    assert_eq!(issues[1]["nid"], "n2");
    assert_eq!(
        build_consolidate_issues(&graph, 1),
        Err(PromptError::InputLimit)
    );
    for id in ["n1", "n2"] {
        graph.nodes.remove(id);
    }
    assert_eq!(build_consolidate_issues(&graph, 1).unwrap(), None);
}

#[test]
fn group_stage_packs_thirty_small_groups_into_one_prompt() {
    let graph = graph_with_groups(30);
    let prompts = build_consolidate_groups(&graph, DEFAULT_INPUT_LIMIT).unwrap();
    assert_eq!(prompts.len(), 1);
    assert_graph_prompt(&prompts[0], 30, 60, DEFAULT_INPUT_LIMIT);
    assert_eq!(section(&prompts[0], "graph groups").len(), 30);
    assert!(!prompts[0].system.contains("reference"));
    assert!(!prompts[0].system.contains("add_node"));
    assert!(
        prompts[0].user.find("graph edges").unwrap() < prompts[0].user.find("graph nodes").unwrap()
    );
}

#[test]
fn group_stage_reduces_evidence_in_order_and_preserves_edges_and_node_text() {
    let mut graph = graph_with_groups(1);
    graph.nodes.get_mut("p1").unwrap().text = "案".repeat(400);
    for row in 1..=3 {
        add_ground(&mut graph, "n1", row, 400);
    }
    let build = |limit| build_consolidate_groups(&graph, limit).unwrap().remove(0);
    let mut limit = DEFAULT_INPUT_LIMIT;
    for (count, length) in [(3, 400), (3, 200), (1, 200), (3, 0)] {
        let prompt = build(limit);
        assert_graph_prompt(&prompt, 1, 2, limit);
        let nodes = section(&prompt, "graph nodes");
        let grounds = nodes[0]["grounds"].as_array().unwrap();
        assert_eq!(grounds.len(), count);
        if length == 0 {
            assert_eq!(
                grounds,
                &vec![
                    serde_json::json!({"uid":"u1"}),
                    serde_json::json!({"uid":"u2"}),
                    serde_json::json!({"uid":"u3"})
                ]
            );
        } else {
            assert!(grounds.iter().all(|g| g["text"] == "根".repeat(length)));
        }
        assert_eq!(nodes[0]["ground_count"], 3);
        assert_eq!(nodes[1]["text"], "案".repeat(400));
        assert!(!prompt.system.contains("案案"));
        limit = size(&prompt) - 1;
    }
    assert_eq!(
        build_consolidate_groups(&graph, 1),
        Err(PromptError::InputLimit)
    );
}

#[test]
fn group_stage_keeps_unassigned_nodes_and_omits_cross_group_edges() {
    let mut graph = graph_with_groups(1);
    for (id, kind) in [("a", NodeKind::Argument), ("p2", NodeKind::Position)] {
        let mut record = node(id, kind);
        record.text = "合".repeat(500);
        graph.nodes.insert(id.into(), record);
    }
    for (eid, dst) in [("e2", "p1"), ("e3", "p2")] {
        graph.edges.insert(
            eid.into(),
            Edge {
                eid: eid.into(),
                src: "a".into(),
                dst: dst.into(),
                kind: EdgeKind::Supports,
                confidence: 1.0,
                review: Review::Auto,
                created_rev: 1,
            },
        );
    }
    let full = build_consolidate_groups(&graph, DEFAULT_INPUT_LIMIT).unwrap();
    assert_eq!(full.len(), 1);
    assert_graph_prompt(&full[0], 2, 4, DEFAULT_INPUT_LIMIT);
    let limit = size(&full[0]) - 1;
    let split = build_consolidate_groups(&graph, limit).unwrap();
    assert_eq!(split.len(), 2);
    assert_graph_prompt(&split[0], 2, 3, limit);
    assert_graph_prompt(&split[1], 0, 1, limit);
    assert_eq!(section(&split[1], "graph nodes")[0]["nid"], "p2");
}

#[test]
fn group_stage_assigns_shared_nodes_to_the_first_issue_only() {
    let mut graph = graph_with_groups(2);
    for nid in ["a", "p1", "p2"] {
        let mut record = node(nid, NodeKind::Argument);
        if nid != "a" {
            record.kind = NodeKind::Position;
        }
        record.text = "案".repeat(1000);
        graph.nodes.insert(nid.into(), record);
    }
    for (eid, dst) in [("e3", "p1"), ("e4", "p2")] {
        graph.edges.insert(
            eid.into(),
            Edge {
                eid: eid.into(),
                src: "a".into(),
                dst: dst.into(),
                kind: EdgeKind::Supports,
                confidence: 1.0,
                review: Review::Auto,
                created_rev: 1,
            },
        );
    }
    let full = build_consolidate_groups(&graph, DEFAULT_INPUT_LIMIT).unwrap();
    assert_graph_prompt(&full[0], 3, 5, DEFAULT_INPUT_LIMIT);
    let limit = size(&full[0]) - 1;
    let split = build_consolidate_groups(&graph, limit).unwrap();
    assert_eq!(split.len(), 2);
    assert_graph_prompt(&split[0], 2, 3, limit);
    assert_graph_prompt(&split[1], 1, 2, limit);
    assert_eq!(
        section(&split[0], "graph groups")[0]["nids"],
        serde_json::json!(["a", "n1", "p1"])
    );
    assert_eq!(
        section(&split[1], "graph groups")[0]["nids"],
        serde_json::json!(["n2", "p2"])
    );
    assert_eq!(section(&split[1], "graph edges")[0]["eid"], "e2");
}

#[test]
fn group_stage_splits_only_the_oversized_group() {
    let small = graph_with_groups(1);
    let expected = build_consolidate_groups(&small, DEFAULT_INPUT_LIMIT).unwrap();
    let mut graph = small;
    for nid in ["a", "b", "c", "d"] {
        let mut record = node(nid, NodeKind::Argument);
        record.text = "案".repeat(1000);
        graph.nodes.insert(nid.into(), record);
    }
    let limit = size(&expected[0]) + 2500;
    let prompts = build_consolidate_groups(&graph, limit).unwrap();
    assert_eq!(prompts.len(), 3);
    assert_eq!(prompts[0], expected[0]);
    for (prompt, expected_ids) in prompts[1..].iter().zip([["a", "b"], ["c", "d"]]) {
        assert_graph_prompt(prompt, 0, 2, limit);
        assert_eq!(
            section(prompt, "graph groups")[0]["nids"],
            serde_json::json!(expected_ids)
        );
    }
}

#[test]
fn incremental_prompt_limits_issue_and_action_creation() {
    let prompt =
        build_incremental(&Graph::default(), &[], &[], false, DEFAULT_INPUT_LIMIT).unwrap();
    for rule in [
        "context の open の issue に当てはまる場合は、新しい issue を作らず、その issue に responds_to",
        "新しい issue は context のどの問いとも異なる問いに限ります",
        "誰かが引き受けた作業や誰かに頼まれた作業が発話にある場合だけ",
    ] {
        assert!(prompt.system.contains(rule));
    }
}
