mod common;

use discussion_graph::{
    apply_patch, build_consolidate, build_consolidate_groups, build_consolidate_issues,
    build_incremental, parse_ops, ConsolidateStyle, Edge, EdgeKind, Graph, Ground,
    IssuePromptStage, NodeKind, Prompt, PromptError, Review, Status, DEFAULT_INPUT_LIMIT,
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
    let limit = 4_200;
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
    graph.nodes.get_mut("p1").unwrap().created_rev = 5;

    for (kind, edge_kind) in [
        (NodeKind::Decision, EdgeKind::Resolves),
        (NodeKind::Position, EdgeKind::RespondsTo),
    ] {
        for (index, created_rev, review, status, link_kind) in [
            (0, 4, Review::Auto, Status::Open, edge_kind),
            (1, 3, Review::Auto, Status::Open, edge_kind),
            (2, 2, Review::Auto, Status::Open, edge_kind),
            (3, 0, Review::Rejected, Status::Open, edge_kind),
            (4, 0, Review::Auto, Status::Merged, edge_kind),
            (5, 0, Review::Auto, Status::Open, EdgeKind::Supports),
        ] {
            let id = format!("{kind:?}{index}");
            let mut annotation = node(&id, kind);
            annotation.created_rev = created_rev;
            annotation.review = review;
            annotation.status = status;
            annotation.text = format!("{index}{}", "添".repeat(70));
            graph.nodes.insert(id.clone(), annotation);
            graph.edges.insert(
                id.clone(),
                Edge {
                    eid: id.clone(),
                    src: id,
                    dst: "n1".into(),
                    kind: link_kind,
                    confidence: 1.0,
                    review: Review::Auto,
                    created_rev: 1,
                },
            );
        }
    }

    graph
        .nodes
        .insert("objection".into(), node("objection", NodeKind::Argument));
    graph.edges.insert(
        "objection".into(),
        Edge {
            eid: "objection".into(),
            src: "objection".into(),
            dst: "p1".into(),
            kind: EdgeKind::ObjectsTo,
            confidence: 1.0,
            review: Review::Auto,
            created_rev: 1,
        },
    );

    let build = |graph: &Graph, limit| build_consolidate_issues(graph, limit).unwrap().unwrap();
    let full = build(&graph, DEFAULT_INPUT_LIMIT);
    assert_graph_prompt(&full, 0, 3, DEFAULT_INPUT_LIMIT);
    assert!(full.system.contains("issue 同士の merge"));
    assert!(!full.system.contains("reference"));
    assert!(!full.system.contains("add_node"));
    assert!(full.user.find("graph edges").unwrap() < full.user.find("graph nodes").unwrap());
    assert_eq!(full.allowed_uids, ["u1", "u3", "u4"]);
    let issues = section(&full, "graph nodes");
    assert_eq!(issues[0]["ground_count"], 2);
    assert_eq!(issues[0]["grounds"][0]["text"], "根".repeat(100));

    for key in ["decisions", "positions"] {
        assert_eq!(
            issues[0][key],
            serde_json::json!([
                format!("2{}", "添".repeat(59)),
                format!("1{}", "添".repeat(59))
            ])
        );
    }

    let limit = size(&full) - 1;
    let short = build(&graph, limit);
    assert_eq!(short.issue_stage, Some(IssuePromptStage::ShortDecision));
    let issues = section(&short, "graph nodes");
    assert_eq!(
        issues[0]["decisions"],
        serde_json::json!([format!("2{}", "添".repeat(39))])
    );
    assert!(issues[0].get("positions").is_none());

    let limit = size(&short) - 1;
    let without_annotations = build(&graph, limit);
    assert_graph_prompt(&without_annotations, 0, 3, limit);
    assert_eq!(without_annotations.omitted_issues, 0);
    assert_eq!(without_annotations.allowed_uids, ["u1", "u3", "u4"]);
    let issues = section(&without_annotations, "graph nodes");
    assert_eq!(issues[0]["grounds"][0]["text"], "根".repeat(100));
    assert!(issues
        .iter()
        .all(|issue| issue.get("decisions").is_none() && issue.get("positions").is_none()));

    let limit = size(&without_annotations) - 1;
    let without_evidence = build(&graph, limit);
    assert_graph_prompt(&without_evidence, 0, 3, limit);
    assert_eq!(without_evidence.omitted_issues, 0);
    assert_eq!(without_evidence.allowed_uids, ["u1", "u2", "u3", "u4"]);
    let issues = section(&without_evidence, "graph nodes");
    assert_eq!(
        issues[0]["grounds"],
        serde_json::json!([{"uid":"u1"}, {"uid":"u2"}])
    );
    assert!(issues
        .iter()
        .all(|issue| issue.get("decisions").is_none() && issue.get("positions").is_none()));

    let limit = size(&without_evidence) - 1;
    let trimmed = build(&graph, limit);
    assert_graph_prompt(&trimmed, 0, 2, limit);
    assert_eq!(trimmed.omitted_issues, 1);
    for (prompt, stage) in [
        (&full, IssuePromptStage::FullAnnotations),
        (&short, IssuePromptStage::ShortDecision),
        (&without_annotations, IssuePromptStage::NoAnnotations),
        (&without_evidence, IssuePromptStage::UidOnly),
        (&trimmed, IssuePromptStage::OmittedIssues),
    ] {
        assert_eq!(prompt.issue_stage, Some(stage));
        let issues = section(prompt, "graph nodes");
        assert_eq!(issues[0]["position_count"], 4);
        assert_eq!(issues[0]["has_objection"], true);
        assert_eq!(issues[1]["position_count"], 1);
        assert_eq!(issues[1]["has_objection"], false);
    }
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
    let prompts = build_consolidate_groups(&graph, DEFAULT_INPUT_LIMIT).0;
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
    let build = |limit| build_consolidate_groups(&graph, limit).0.remove(0);
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
    assert_eq!(build_consolidate_groups(&graph, 1), (vec![], 2));
}

#[test]
fn group_stage_keeps_unassigned_nodes_and_cross_group_edges() {
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
    let full = build_consolidate_groups(&graph, DEFAULT_INPUT_LIMIT).0;
    assert_eq!(full.len(), 1);
    assert_graph_prompt(&full[0], 3, 4, DEFAULT_INPUT_LIMIT);
    assert!(full[0].system.contains("external のエッジも既に存在"));
    let limit = size(&full[0]) - 1;
    let split = build_consolidate_groups(&graph, limit).0;
    assert_eq!(split.len(), 2);
    assert_graph_prompt(&split[0], 3, 3, limit);
    assert_graph_prompt(&split[1], 1, 1, limit);
    let external_edge = serde_json::json!({
        "eid":"e3", "src":"a", "dst":"p2", "kind":"supports", "external":true
    });
    assert_eq!(section(&split[0], "graph edges")[2], external_edge);
    assert_eq!(section(&split[1], "graph edges"), [external_edge]);
    assert_eq!(section(&split[1], "graph nodes")[0]["nid"], "p2");
}

#[test]
fn group_stage_assigns_shared_nodes_to_the_first_issue_only() {
    let mut graph = graph_with_groups(2);
    for (nid, kind) in [
        ("n5", NodeKind::Argument),
        ("n3", NodeKind::Position),
        ("n4", NodeKind::Position),
    ] {
        let mut record = node(nid, kind);
        record.text = "案".repeat(1000);
        graph.nodes.insert(nid.into(), record);
    }
    graph.nodes.remove("p1");
    graph.nodes.remove("p2");
    graph.edges.get_mut("e1").unwrap().src = "n3".into();
    graph.edges.get_mut("e2").unwrap().src = "n4".into();
    for (eid, dst) in [("e3", "n3"), ("e4", "n4")] {
        graph.edges.insert(
            eid.into(),
            Edge {
                eid: eid.into(),
                src: "n5".into(),
                dst: dst.into(),
                kind: EdgeKind::Supports,
                confidence: 1.0,
                review: Review::Auto,
                created_rev: 1,
            },
        );
    }
    let full = build_consolidate_groups(&graph, DEFAULT_INPUT_LIMIT).0;
    assert_eq!(full.len(), 1);
    assert_graph_prompt(&full[0], 4, 5, DEFAULT_INPUT_LIMIT);
    let edges = section(&full[0], "graph edges");
    assert_eq!(edges[3]["eid"], "e4");
    assert!(edges[3].get("external").is_none());
    let limit = size(&full[0]) - 1;
    let split = build_consolidate_groups(&graph, limit).0;
    assert_eq!(split.len(), 2);
    assert_graph_prompt(&split[0], 3, 3, limit);
    assert_graph_prompt(&split[1], 2, 2, limit);
    assert_eq!(
        section(&split[0], "graph groups")[0]["nids"],
        serde_json::json!(["n1", "n3", "n5"])
    );
    assert_eq!(
        section(&split[1], "graph groups")[0]["nids"],
        serde_json::json!(["n2", "n4"])
    );
    assert_eq!(section(&split[1], "graph edges")[0]["eid"], "e2");
}

#[test]
fn group_stage_splits_only_the_oversized_group() {
    let small = graph_with_groups(1);
    let expected = build_consolidate_groups(&small, DEFAULT_INPUT_LIMIT).0;
    let mut graph = small;
    for nid in ["a", "b", "c", "d"] {
        let mut record = node(nid, NodeKind::Argument);
        record.text = "案".repeat(1000);
        graph.nodes.insert(nid.into(), record);
    }
    for row in 1..=3 {
        add_ground(&mut graph, "d", row, 1000);
    }
    let limit = size(&expected[0]) + 3000;
    let prompts = build_consolidate_groups(&graph, limit).0;
    assert_eq!(prompts.len(), 2);
    assert_graph_prompt(&prompts[0], 1, 4, limit);
    assert_eq!(
        section(&prompts[0], "graph groups")[1]["nids"],
        serde_json::json!(["a", "b"])
    );
    assert_graph_prompt(&prompts[1], 0, 2, limit);
    assert_eq!(
        section(&prompts[1], "graph groups")[0]["nids"],
        serde_json::json!(["c", "d"])
    );
    let nodes = section(&prompts[1], "graph nodes");
    let grounds = nodes[1]["grounds"].as_array().unwrap();
    assert_eq!(grounds.len(), 3);
    assert!(grounds
        .iter()
        .all(|ground| ground["text"] == "根".repeat(200)));
}

#[test]
fn group_stage_skips_only_an_oversized_node() {
    let mut graph = graph_with_groups(2);
    graph.nodes.get_mut("p1").unwrap().text = "案".repeat(DEFAULT_INPUT_LIMIT);
    let (prompts, omitted) = build_consolidate_groups(&graph, DEFAULT_INPUT_LIMIT);
    assert_eq!(omitted, 1);
    assert_eq!(prompts.len(), 1);
    assert_graph_prompt(&prompts[0], 2, 3, DEFAULT_INPUT_LIMIT);
    assert_eq!(section(&prompts[0], "graph edges")[0]["external"], true);
    let nodes = section(&prompts[0], "graph nodes");
    assert_eq!(
        nodes
            .iter()
            .map(|node| node["nid"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["n1", "n2", "p2"]
    );
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

#[test]
fn prompts_show_parents_and_stage_one_can_attach_a_question_using_uid_only() {
    let mut graph = graph_with_groups(3);
    add_ground(&mut graph, "n2", 2, 400);
    add_ground(&mut graph, "n3", 3, 400);
    let first = parse_ops(r#"{"ops":[
        {"op":"add_edge","src":"n2","dst":"n3","kind":"sub_issue_of","confidence":0.9,"grounds":[{"uid":"u2"}]}
    ]}"#).unwrap();
    graph = apply_patch(&graph, &first, &["u2"]).graph;
    let live = build_incremental(&graph, &[], &[], false, DEFAULT_INPUT_LIMIT).unwrap();
    assert!(live.system.contains("同じ応答で sub_issue_of"));
    assert!(section(&live, "context")
        .iter()
        .any(|line| line["nid"] == "n2" && line["parent_nid"] == "n3"));

    let full = build_consolidate_issues(&graph, DEFAULT_INPUT_LIMIT)
        .unwrap()
        .unwrap();
    let short = build_consolidate_issues(&graph, size(&full) - 1)
        .unwrap()
        .unwrap();
    let without_annotations = build_consolidate_issues(&graph, size(&short) - 1)
        .unwrap()
        .unwrap();
    let issues = build_consolidate_issues(&graph, size(&without_annotations) - 1)
        .unwrap()
        .unwrap();
    assert!(issues.system.contains("sub_issue_of の add_edge"));
    assert_eq!(issues.allowed_uids, ["u2", "u3"]);
    let nodes = section(&issues, "graph nodes");
    assert_eq!(nodes[1]["parent_nid"], "n3");
    assert_eq!(nodes[1]["grounds"][0], serde_json::json!({"uid":"u2"}));
    assert!(nodes[2]["parent_nid"].is_null());
    let ops = parse_ops(r#"{"ops":[
        {"op":"add_edge","src":"n3","dst":"n1","kind":"sub_issue_of","confidence":0.9,"grounds":[{"uid":"u3"}]}
    ]}"#).unwrap();
    let allowed = issues
        .allowed_uids
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let result = apply_patch(&graph, &ops, &allowed);
    assert_eq!(result.applied.len(), 1);
    let (groups, omitted) = build_consolidate_groups(&result.graph, DEFAULT_INPUT_LIMIT);
    assert_eq!(omitted, 0);
    assert_eq!(
        section(&groups[0], "graph groups"),
        [serde_json::json!({"nids":["n1","n2","n3","p1","p2","p3"]})]
    );
}

#[test]
fn stage_one_lists_sub_issue_edges() {
    let ops = parse_ops(r#"{"ops":[
        {"op":"add_edge","src":"n1","dst":"n2","kind":"sub_issue_of","confidence":0.9,"grounds":[{"uid":"u1"}]}
    ]}"#).unwrap();
    let graph = apply_patch(&graph_with_groups(2), &ops, &["u1"]).graph;
    let prompt = build_consolidate_issues(&graph, DEFAULT_INPUT_LIMIT)
        .unwrap()
        .unwrap();
    let edges = section(&prompt, "graph edges");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["src"], "n1");
    assert_eq!(edges[0]["dst"], "n2");
    assert_eq!(edges[0]["kind"], "sub_issue_of");
}

#[test]
fn stage_two_keeps_an_earlier_child_in_its_agenda_group() {
    let ops = parse_ops(r#"{"ops":[
        {"op":"add_edge","src":"n1","dst":"n2","kind":"sub_issue_of","confidence":0.9,"grounds":[{"uid":"u1"}]}
    ]}"#).unwrap();
    let graph = apply_patch(&graph_with_groups(2), &ops, &["u1"]).graph;
    let (prompts, omitted) = build_consolidate_groups(&graph, DEFAULT_INPUT_LIMIT);
    assert_eq!(omitted, 0);
    assert_eq!(prompts.len(), 1);
    assert_eq!(
        section(&prompts[0], "graph groups"),
        [serde_json::json!({"nids":["n1","n2","p1","p2"]})]
    );
}

#[test]
fn extraction_and_issue_stage_distinguish_agendas_from_sub_questions() {
    let mut graph = graph_with_groups(2);
    add_ground(&mut graph, "n1", 1, 20);
    let increment = build_incremental(&graph, &["u1"], &[], false, DEFAULT_INPUT_LIMIT).unwrap();
    let stage_one = build_consolidate_issues(&graph, DEFAULT_INPUT_LIMIT)
        .unwrap()
        .unwrap();
    for system in [&increment.system, &stage_one.system] {
        for rule in [
            "案が対立する issue（responds_to する position が 2 件以上ある、または objects_to を受けた position が付いている）は、必ず議題とし、ほかの議題の判断材料になる問いでも親を付けないでください。",
            "細かな問いは、親の議題に答えるための事実・数値・可否の確認の問いで、案が対立していないものです。答えの position が 1 件あってもかまいません。",
            "それ以外の issue（案が 1 件だけでも確認の問いでないもの）は、親の無い議題のまま残してください。",
        ] {
            assert!(system.lines().any(|line| line == rule), "{rule}");
        }
        assert!(
            system.contains("会議の進め方（今日は何を話すか、順番、時間配分）は issue にしない")
        );
        assert!(system.contains("議題を進め方の問いの子にしない"));
    }
    assert!(increment
        .system
        .contains("明らかに事実・数値・可否の確認の問いのときだけ"));
    assert!(stage_one.system.lines().any(|line| {
        line == "既存の親子も見直してください。細かな問いに当たらない issue の親と、進め方の問いへの親は remove_edge で外してください。必要なら同じ応答の後続の add_edge で別の親に付け直せます。remove_edge は段 1 の sub_issue_of に限ります。"
    }));
    assert!(stage_one.system.contains(r#""op":"remove_edge""#));
    assert_eq!(increment.issue_stage, None);
    assert!(stage_one.system.contains("decisions"));
    assert!(stage_one.system.contains("positions"));
}
