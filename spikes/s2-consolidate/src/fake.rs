use std::collections::{BTreeMap, BTreeSet};

use discussion_graph::{Graph, Prompt, Status, eval::GoldGraph};
use serde_json::json;

use crate::data::uid;

/// Play each annotation when its first ground arrives. Add edges when the second
/// endpoint becomes available. Consolidation preserves the annotated graph.
pub(crate) fn response(graph: &Graph, gold: &GoldGraph, prompt: &Prompt) -> String {
    let new: BTreeSet<_> = prompt.new_uids.iter().map(String::as_str).collect();
    let existing: BTreeMap<_, _> = graph
        .grounds
        .iter()
        .filter_map(|ground| {
            graph.nodes.get(&ground.target).map(|node| {
                (
                    (ground.uid.as_str(), node.kind, node.text.as_str()),
                    node.nid.as_str(),
                )
            })
        })
        .collect();
    let mut ids = BTreeMap::new();
    let mut ops = Vec::new();
    for node in &gold.nodes {
        let Some(anchor) = node.grounds.iter().min().map(|&row| uid(row)) else {
            continue;
        };
        if new.contains(anchor.as_str()) {
            ops.push(json!({
                "op":"add_node", "tmp":node.id, "kind":node.kind, "text":node.text,
                "confidence":1.0, "grounds":[{"uid":anchor}],
            }));
            if node.status != Status::Open {
                ops.push(json!({
                    "op":"set_status", "nid":node.id, "status":node.status, "grounds":[{"uid":anchor}],
                }));
            }
            ids.insert(node.id.as_str(), (node.id.as_str(), Some(anchor)));
        } else if let Some(&nid) = existing.get(&(anchor.as_str(), node.kind, node.text.as_str())) {
            ids.insert(node.id.as_str(), (nid, None));
        }
    }
    for edge in &gold.edges {
        if let (Some((src, src_new)), Some((dst, dst_new))) =
            (ids.get(edge.src.as_str()), ids.get(edge.dst.as_str()))
            && let Some(anchor) = src_new.as_ref().or(dst_new.as_ref())
        {
            ops.push(json!({
                "op":"add_edge", "src":src, "dst":dst, "kind":edge.kind,
                "confidence":1.0, "grounds":[{"uid":anchor}],
            }));
        }
    }
    json!({"ops":ops}).to_string()
}
