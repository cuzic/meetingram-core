mod common;

use discussion_graph::{
    build_consolidate, build_incremental, ConsolidateStyle, Graph, Ground, NodeKind, Prompt,
    PromptError, Review, Status, DEFAULT_INPUT_LIMIT,
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
        graph.nodes.insert(id, node);
    }
    graph.nodes.get_mut("n8").unwrap().review = Review::Rejected;
    graph.nodes.get_mut("n7").unwrap().status = Status::Merged;
    graph.nodes.get_mut("n6").unwrap().status = Status::Decided;
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
        ["n5", "n4"]
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

#[test]
fn consolidation_splits_and_caps_text_before_reducing_ground_count() {
    let mut graph = Graph::default();
    for i in 1..=2 {
        let id = format!("n{i}");
        graph.nodes.insert(id.clone(), node(&id, NodeKind::Issue));
        for k in 0..2 {
            let u = utterance(i * 10 + k, "根".repeat(if i == 1 { 650 } else { 250 }));
            graph.grounds.push(Ground {
                target: id.clone(),
                uid: u.uid.clone(),
                act: None,
            });
            graph.utterances.insert(u.uid.clone(), u);
        }
    }
    let style = ConsolidateStyle::Strengthened;
    let full = build_consolidate(&graph, style, DEFAULT_INPUT_LIMIT).unwrap();
    assert_eq!(full.len(), 1);
    assert!(full[0].system.contains("すべての組"));
    let system_size = full[0].system.chars().count();

    let limit = system_size + 760;
    let split = build_consolidate(&graph, style, limit).unwrap();
    assert_eq!(split.len(), 2);
    for prompt in &split {
        assert!(size(prompt) <= limit);
        assert_eq!(
            section(prompt, "issues")
                .iter()
                .map(|n| n["nid"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["n1", "n2"]
        );
        let nodes = section(prompt, "graph nodes");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0]["grounds"].as_array().unwrap().len(), 2);
        assert!(nodes[0]["grounds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|ground| ground["text"].as_str().unwrap().chars().count() == 200));
        assert_eq!(nodes[0]["ground_count"], 2);
    }
    let limit = system_size + 510;
    let reduced = build_consolidate(&graph, style, limit).unwrap();
    for prompt in &reduced {
        assert!(size(prompt) <= limit);
        assert_eq!(section(prompt, "issues").len(), 2);
        assert_eq!(prompt.allowed_uids.len(), 1);
        let nodes = section(prompt, "graph nodes");
        assert_eq!(nodes[0]["grounds"].as_array().unwrap().len(), 1);
        assert_eq!(nodes[0]["ground_count"], 2);
    }
    let baseline =
        build_consolidate(&graph, ConsolidateStyle::Baseline, DEFAULT_INPUT_LIMIT).unwrap();
    assert_eq!(baseline.len(), 1);
    assert!(!baseline[0].system.contains("すべての組"));
    assert_eq!(baseline[0].allowed_uids.len(), 4);
    assert_eq!(
        build_consolidate(&graph, style, system_size + 20),
        Err(PromptError::InputLimit)
    );
}
