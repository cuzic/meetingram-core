use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use crate::{Edge, Graph, Node, NodeKind, Status, Utterance};

pub const DEFAULT_INPUT_LIMIT: usize = 24_000;

const EXTRACT: &str = include_str!("extraction.txt");
const CONSOLIDATE: &str = concat!(
    include_str!("extraction.txt"),
    "\n会議は終わりました。今回は graph と根拠の発話を読みます。\n",
    "重複の merge、抜けたエッジの add_edge、状態の訂正の set_status を返してください。\n",
    "新規ノードは原則作らず、提示された根拠の uid を使ってください。\n"
);
const STRENGTHENED: &str = concat!(
    "\nまず issue 同士のすべての組を比較してください。次に、同じ issue に属する position 同士のすべての組を比較してください。\n",
    "同じ問い・同じ案を言い換えたものは必ず merge し、根拠が多い方を into にしてください。\n",
    "issues があれば、そこにある他のまとまりの issue も比較に含めてください。\n"
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub system: String,
    pub user: String,
    /// Only these utterances can be used as grounds for this request.
    pub allowed_uids: Vec<String>,
    /// New utterances included in this incremental request, in row order.
    pub new_uids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PromptError {
    #[error("prompt cannot fit within the input limit")]
    InputLimit,
}

/// Build a window from the pending IDs. Pass only the recent patch history
/// selected by the caller, in oldest-to-newest order.
pub fn build_incremental(
    graph: &Graph,
    pending_uids: &[&str],
    recent_touched: &[BTreeSet<String>],
    retranscribed: bool,
    limit: usize,
) -> Result<Prompt, PromptError> {
    let mut ordered: Vec<_> = graph.utterances.values().collect();
    ordered.sort_by_key(|u| (u.row_id, &u.uid));
    let pending_ids: BTreeSet<_> = pending_uids.iter().copied().collect();
    let pending: Vec<_> = ordered
        .iter()
        .copied()
        .filter(|u| pending_ids.contains(u.uid.as_str()))
        .collect();
    let mut context = context_lines(graph, recent_touched);

    let frame = chars(&incremental_user(&[], &[], &[]));
    let budget = limit
        .checked_sub(chars(EXTRACT) + frame)
        .ok_or(PromptError::InputLimit)?;
    let third = limit / 3;
    let mut context_len: usize = context.iter().map(|line| chars(line) + 1).sum();
    while context_len > third.min(budget) {
        if let Some(line) = context.pop() {
            context_len -= chars(&line) + 1;
        }
    }
    let mut refs = Vec::new();
    if !retranscribed {
        if let Some(first) = pending.first() {
            let end = ordered.partition_point(|u| u.row_id < first.row_id);
            refs.extend(
                ordered[end.saturating_sub(3)..end]
                    .iter()
                    .map(|u| (u.uid.clone(), utterance_line(u, 200))),
            );
        }
    }

    // Reserve enough space for the first new utterance before spending the
    // remaining budget on references. Context and references lose oldest first.
    let first = pending.first().map(|u| utterance_line(u, third));
    let first_len = first.as_ref().map_or(0, |line| chars(line) + 1);
    let mut ref_len: usize = refs.iter().map(|(_, line)| chars(line) + 1).sum();
    while context_len + ref_len + first_len > budget && !refs.is_empty() {
        let (_, line) = refs.remove(0);
        ref_len -= chars(&line) + 1;
    }
    while context_len + first_len > budget {
        let Some(line) = context.pop() else { break };
        context_len -= chars(&line) + 1;
    }

    let mut remaining = budget - context_len - ref_len;
    let mut new_lines = Vec::new();
    let mut new_uids = Vec::new();
    for u in pending {
        let mut line = utterance_line(u, third);
        if chars(&line) + 1 > remaining {
            if !new_lines.is_empty() {
                break;
            }
            line = fit_utterance(u, third, remaining.saturating_sub(1))
                .ok_or(PromptError::InputLimit)?;
        }
        remaining -= chars(&line) + 1;
        new_uids.push(u.uid.clone());
        new_lines.push(line);
    }
    let (mut allowed_uids, ref_lines): (Vec<_>, Vec<_>) = refs.into_iter().unzip();
    allowed_uids.extend(new_uids.iter().cloned());
    Ok(Prompt {
        system: EXTRACT.to_owned(),
        user: incremental_user(&context, &ref_lines, &new_lines),
        allowed_uids,
        new_uids,
    })
}

fn context_lines(graph: &Graph, recent_touched: &[BTreeSet<String>]) -> Vec<String> {
    let mut nodes: Vec<_> = graph.nodes.values().filter(|n| n.is_live()).collect();
    nodes.sort_by_key(|n| {
        std::cmp::Reverse((
            n.created_rev,
            n.nid
                .strip_prefix('n')
                .and_then(|id| id.parse::<u64>().ok()),
            &n.nid,
        ))
    });
    let mut context = Vec::new();
    let mut seen = BTreeSet::new();
    let mut add = |node: &Node| {
        if seen.insert(node.nid.clone()) {
            context.push(
                json!({
                    "nid":node.nid, "kind":node.kind, "status":node.status, "text":node.text,
                })
                .to_string(),
            );
        }
    };
    for node in nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Issue && n.status == Status::Open)
    {
        add(node);
    }
    for touched in recent_touched.iter().rev() {
        for node in nodes.iter().filter(|n| touched.contains(&n.nid)) {
            add(node);
        }
    }
    context
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

fn prefix(text: &str, cap: usize) -> &str {
    text.char_indices()
        .nth(cap)
        .map_or(text, |(end, _)| &text[..end])
}

fn utterance_line(utterance: &Utterance, cap: usize) -> String {
    json!({
        "uid": utterance.uid,
        "speaker_label": utterance.speaker_label,
        "text": prefix(&utterance.text, cap),
    })
    .to_string()
}

fn fit_utterance(utterance: &Utterance, cap: usize, budget: usize) -> Option<String> {
    let mut line = utterance_line(utterance, 0);
    if chars(&line) > budget {
        return None;
    }
    let mut start = 1;
    let mut end = cap.min(chars(&utterance.text));
    while start <= end {
        let middle = start + (end - start) / 2;
        let candidate = utterance_line(utterance, middle);
        if chars(&candidate) <= budget {
            line = candidate;
            start = middle + 1;
        } else {
            end = middle - 1;
        }
    }
    Some(line)
}

fn incremental_user(context: &[String], refs: &[String], new: &[String]) -> String {
    let mut output = String::new();
    for (header, lines) in [("context", context), ("reference", refs), ("new", new)] {
        output.push_str(header);
        output.push('\n');
        for line in lines {
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConsolidateStyle {
    Baseline,
    Strengthened,
}

/// Build the full consolidation prompt, or one prompt per issue group when
/// the strengthened prompt exceeds the limit.
pub fn build_consolidate(
    graph: &Graph,
    style: ConsolidateStyle,
    limit: usize,
) -> Result<Vec<Prompt>, PromptError> {
    let nodes: BTreeMap<_, _> = graph
        .nodes
        .values()
        .filter(|n| n.is_live())
        .map(|n| (n.nid.as_str(), n))
        .collect();
    let edges: Vec<_> = graph.live_edges().collect();
    let mut evidence: BTreeMap<&str, Vec<&Utterance>> = BTreeMap::new();
    for ground in &graph.grounds {
        if let Some(u) = graph.utterances.get(&ground.uid) {
            evidence.entry(&ground.target).or_default().push(u);
        }
    }
    for utterances in evidence.values_mut() {
        utterances.sort_by_key(|u| (u.row_id, &u.uid));
        utterances.dedup_by_key(|u| &u.uid);
    }
    let renderer = Consolidation {
        nodes,
        edges,
        evidence,
    };
    let all = renderer.nodes.keys().copied().collect();
    if style == ConsolidateStyle::Baseline {
        let mut prompt = renderer.render(CONSOLIDATE, &all, None, 200, 3);
        let budget = limit
            .checked_sub(chars(CONSOLIDATE))
            .ok_or(PromptError::InputLimit)?;
        const OMITTED: &str = "（以下省略）\n";
        if chars(&prompt.user) > budget {
            let body_budget = budget
                .checked_sub(chars(OMITTED))
                .ok_or(PromptError::InputLimit)?;
            let end = prefix(&prompt.user, body_budget)
                .rfind('\n')
                .map_or(0, |end| end + 1);
            prompt.user.truncate(end);
            prompt.allowed_uids.retain(|uid| {
                prompt.user.lines().any(|line| {
                    serde_json::from_str::<serde_json::Value>(line).is_ok_and(|value| {
                        value["grounds"].as_array().is_some_and(|grounds| {
                            grounds
                                .iter()
                                .any(|ground| ground["uid"].as_str() == Some(uid))
                        })
                    })
                })
            });
            prompt.user.push_str(OMITTED);
        }
        return Ok(vec![prompt]);
    }

    let system = format!("{CONSOLIDATE}{STRENGTHENED}");
    let full = renderer.render(&system, &all, None, usize::MAX, usize::MAX);
    if full.fits(limit) {
        return Ok(vec![full]);
    }
    let issues: Vec<_> = renderer
        .nodes
        .values()
        .filter(|node| node.kind == NodeKind::Issue)
        .copied()
        .collect();
    renderer
        .groups(&issues, &all)
        .iter()
        .map(|group| {
            for (cap, count) in [(usize::MAX, usize::MAX), (200, usize::MAX), (200, 1)] {
                let prompt = renderer.render(&system, group, Some(&issues), cap, count);
                if prompt.fits(limit) {
                    return Ok(prompt);
                }
            }
            Err(PromptError::InputLimit)
        })
        .collect()
}

impl Prompt {
    fn fits(&self, limit: usize) -> bool {
        chars(&self.system) + chars(&self.user) <= limit
    }
}

struct Consolidation<'a> {
    nodes: BTreeMap<&'a str, &'a Node>,
    edges: Vec<&'a Edge>,
    evidence: BTreeMap<&'a str, Vec<&'a Utterance>>,
}

impl<'a> Consolidation<'a> {
    fn groups(&self, issues: &[&'a Node], all: &BTreeSet<&'a str>) -> Vec<BTreeSet<&'a str>> {
        let mut incoming: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for edge in &self.edges {
            incoming.entry(&edge.dst).or_default().push(&edge.src);
        }
        let mut groups = Vec::new();
        let mut assigned = BTreeSet::new();
        for issue in issues {
            let id = issue.nid.as_str();
            let mut group = BTreeSet::new();
            let mut pending = vec![id];
            while let Some(nid) = pending.pop() {
                if group.insert(nid) {
                    pending.extend(incoming.get(nid).into_iter().flatten().copied());
                }
            }
            assigned.extend(group.iter().copied());
            groups.push(group);
        }
        let unassigned: BTreeSet<_> = all.difference(&assigned).copied().collect();
        if !unassigned.is_empty() || groups.is_empty() {
            groups.push(unassigned);
        }
        groups
    }

    fn render(
        &self,
        system: &str,
        group: &BTreeSet<&str>,
        issues: Option<&[&Node]>,
        cap: usize,
        count: usize,
    ) -> Prompt {
        let mut user = String::new();
        let mut allowed = BTreeSet::new();
        if let Some(issues) = issues {
            user.push_str("issues\n");
            for issue in issues {
                push_line(&mut user, json!({"nid":issue.nid, "text":issue.text}));
            }
        }
        user.push_str("graph nodes\n");
        for nid in group {
            let node = self.nodes[nid];
            let utterances = self
                .evidence
                .get(nid)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let grounds: Vec<_> = utterances.iter().take(count).map(|u| {
                allowed.insert(u.uid.clone());
                json!({"uid":u.uid, "speaker_label":u.speaker_label, "text":prefix(&u.text, cap)})
            }).collect();
            push_line(
                &mut user,
                json!({
                    "nid":nid, "kind":node.kind, "status":node.status, "text":node.text,
                    "confidence":node.confidence, "ground_count":utterances.len(), "grounds":grounds,
                }),
            );
        }
        user.push_str("graph edges\n");
        for edge in self
            .edges
            .iter()
            .filter(|e| group.contains(e.src.as_str()) && group.contains(e.dst.as_str()))
        {
            push_line(
                &mut user,
                json!({
                    "eid":edge.eid, "src":edge.src, "dst":edge.dst,
                    "kind":edge.kind, "confidence":edge.confidence,
                }),
            );
        }
        Prompt {
            system: system.to_owned(),
            user,
            allowed_uids: allowed.into_iter().collect(),
            new_uids: Vec::new(),
        }
    }
}

fn push_line(user: &mut String, line: serde_json::Value) {
    user.push_str(&line.to_string());
    user.push('\n');
}
