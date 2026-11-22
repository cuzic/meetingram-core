//! Compare a prediction against annotated nodes and edges using ground row IDs.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{AppliedOp, EdgeKind, Graph, NodeKind, Status};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldGraph {
    pub nodes: Vec<GoldNode>,
    pub edges: Vec<GoldEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldNode {
    pub id: String,
    pub kind: NodeKind,
    pub text: String,
    pub status: Status,
    pub grounds: Vec<i64>,
    /// Parent agenda or question of a gold sub-question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldEdge {
    pub src: String,
    pub dst: String,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Evaluation {
    pub node_recall: f64,
    pub node_precision: f64,
    pub edge_f1: f64,
    pub live_nodes: usize,
    /// Live issues without a parent.
    pub agendas: usize,
    /// Live issues with a parent, including nested questions.
    pub sub_questions: usize,
    /// Merges whose endpoints each share a ground row with at least one
    /// same-kind gold node before consolidation, with disjoint sets of matches.
    pub wrong_merges: usize,
    /// Merge endpoints in response order where either side has no gold match.
    pub undetermined_merges: Vec<(String, String)>,
}

/// Count gold issues without a parent.
pub fn gold_agenda_count(gold: &GoldGraph) -> usize {
    gold.nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Issue && node.parent.is_none())
        .count()
}

/// Evaluate live nodes and edges against agenda-level gold annotations.
/// Exclude sub-questions and nodes attached only to sub-questions from scoring.
/// Pass the applied consolidation operations in order.
pub fn evaluate(
    prediction: &Graph,
    gold: &GoldGraph,
    before_consolidation: &Graph,
    consolidation_ops: &[AppliedOp],
) -> Evaluation {
    let parents = prediction.issue_parents();
    let agendas: Vec<_> = prediction
        .nodes
        .values()
        .filter(|node| {
            node.is_live()
                && node.kind == NodeKind::Issue
                && !parents.contains_key(node.nid.as_str())
        })
        .map(|node| node.nid.as_str())
        .collect();
    let links = prediction
        .live_edges()
        .filter(|edge| edge.kind != EdgeKind::SubIssueOf)
        .map(|edge| (edge.src.as_str(), edge.dst.as_str()));
    let excluded = sub_question_only(links, agendas.clone(), parents.keys().copied().collect());
    let (gold_agendas, gold_questions): (Vec<_>, Vec<_>) = gold
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Issue)
        .partition(|node| node.parent.is_none());
    let gold_links = gold
        .edges
        .iter()
        .filter(|edge| edge.kind != EdgeKind::SubIssueOf)
        .map(|edge| (edge.src.as_str(), edge.dst.as_str()));
    let gold_excluded = sub_question_only(
        gold_links,
        gold_agendas
            .into_iter()
            .map(|node| node.id.as_str())
            .collect(),
        gold_questions
            .into_iter()
            .map(|node| node.id.as_str())
            .collect(),
    );
    let matched = matching(
        prediction,
        gold,
        |node| node.is_live() && !excluded.contains(node.nid.as_str()),
        &gold_excluded,
    );
    let live_nodes = prediction.nodes.values().filter(|n| n.is_live()).count();
    let gold_edges: BTreeSet<_> = gold
        .edges
        .iter()
        .filter(|edge| {
            !gold_excluded.contains(edge.src.as_str()) && !gold_excluded.contains(edge.dst.as_str())
        })
        .map(|e| (e.src.as_str(), e.dst.as_str(), e.kind))
        .collect();
    let mut edge_count = 0;
    let mut matched_edges = BTreeSet::new();
    for edge in prediction.live_edges() {
        if excluded.contains(edge.src.as_str()) || excluded.contains(edge.dst.as_str()) {
            continue;
        }
        edge_count += 1;
        if let (Some(&src), Some(&dst)) = (
            matched.get(edge.src.as_str()),
            matched.get(edge.dst.as_str()),
        ) {
            let key = (src, dst, edge.kind);
            if gold_edges.contains(&key) {
                matched_edges.insert(key);
            }
        }
    }

    let mut before = BTreeMap::<_, BTreeSet<_>>::new();
    for (_, prediction_id, gold_id) in candidates(before_consolidation, gold, |_| true) {
        before.entry(prediction_id).or_default().insert(gold_id);
    }
    let mut wrong_merges = 0;
    let mut undetermined_merges = Vec::new();
    for op in consolidation_ops {
        if let AppliedOp::Merge { from, into, .. } = op {
            match before.get(from.as_str()).zip(before.get(into.as_str())) {
                Some((source, target)) if source.is_disjoint(target) => wrong_merges += 1,
                Some(_) => {}
                None => undetermined_merges.push((from.clone(), into.clone())),
            }
        }
    }
    Evaluation {
        node_recall: ratio(
            matched.len(),
            gold.nodes
                .iter()
                .filter(|node| !gold_excluded.contains(node.id.as_str()))
                .count(),
        ),
        node_precision: ratio(matched.len(), live_nodes - excluded.len()),
        edge_f1: ratio(2 * matched_edges.len(), edge_count + gold_edges.len()),
        live_nodes,
        agendas: agendas.len(),
        sub_questions: parents.len(),
        wrong_merges,
        undetermined_merges,
    }
}

fn sub_question_only<'a>(
    links: impl Iterator<Item = (&'a str, &'a str)>,
    agendas: Vec<&'a str>,
    questions: Vec<&'a str>,
) -> BTreeSet<&'a str> {
    let mut incoming = BTreeMap::<_, Vec<_>>::new();
    for (src, dst) in links {
        incoming.entry(dst).or_default().push(src);
    }
    let agenda_nodes = reachable_nodes(&incoming, agendas);
    let question_nodes = reachable_nodes(&incoming, questions);
    question_nodes.difference(&agenda_nodes).copied().collect()
}

fn reachable_nodes<'a>(
    incoming: &BTreeMap<&str, Vec<&'a str>>,
    mut pending: Vec<&'a str>,
) -> BTreeSet<&'a str> {
    let mut reached = BTreeSet::new();
    while let Some(nid) = pending.pop() {
        if reached.insert(nid) {
            pending.extend(incoming.get(nid).into_iter().flatten().copied());
        }
    }
    reached
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn candidates<'a>(
    prediction: &'a Graph,
    gold: &'a GoldGraph,
    include: impl Fn(&crate::Node) -> bool,
) -> Vec<(f64, &'a str, &'a str)> {
    let mut row_ids = BTreeMap::<_, BTreeSet<_>>::new();
    for ground in &prediction.grounds {
        if let Some(utterance) = prediction.utterances.get(&ground.uid) {
            row_ids
                .entry(ground.target.as_str())
                .or_default()
                .insert(utterance.row_id);
        }
    }
    let mut candidates = Vec::new();
    for gold_node in &gold.nodes {
        let gold_rows: BTreeSet<_> = gold_node.grounds.iter().copied().collect();
        for node in prediction
            .nodes
            .values()
            .filter(|node| node.kind == gold_node.kind && include(node))
        {
            if let Some(rows) = row_ids.get(node.nid.as_str()) {
                let shared = rows.intersection(&gold_rows).count();
                if shared > 0 {
                    let union = rows.len() + gold_rows.len() - shared;
                    candidates.push((
                        ratio(shared, union),
                        node.nid.as_str(),
                        gold_node.id.as_str(),
                    ));
                }
            }
        }
    }
    // Match highest Jaccard scores first, using IDs to break ties.
    candidates.sort_by(|(left, lp, lg), (right, rp, rg)| {
        right.total_cmp(left).then_with(|| (lp, lg).cmp(&(rp, rg)))
    });
    candidates
}

fn matching<'a>(
    prediction: &'a Graph,
    gold: &'a GoldGraph,
    include: impl Fn(&crate::Node) -> bool,
    excluded_gold: &BTreeSet<&str>,
) -> BTreeMap<&'a str, &'a str> {
    let mut matched = BTreeMap::new();
    let mut used_gold = BTreeSet::new();
    for (_, prediction_id, gold_id) in candidates(prediction, gold, include) {
        if excluded_gold.contains(gold_id) {
            continue;
        }
        if !matched.contains_key(prediction_id) && used_gold.insert(gold_id) {
            matched.insert(prediction_id, gold_id);
        }
    }
    matched
}
