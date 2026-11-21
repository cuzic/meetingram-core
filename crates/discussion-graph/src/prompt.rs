use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use crate::{Edge, Graph, Node, NodeKind, Status, Utterance};

pub const DEFAULT_INPUT_LIMIT: usize = 24_000;

const EXTRACT: &str = include_str!("extraction.txt");
const CONSOLIDATE: &str = include_str!("consolidation.txt");
const CONSOLIDATE_EDITS: &str = concat!(
    include_str!("consolidation.txt"),
    include_str!("consolidation-edits.txt")
);
const ISSUE_MERGES: &str = "\n全 issue の組を比較し、同じ問いの言い換えは issue 同士の merge だけで統合してください。ground_count が多い方を into にしてください。\n";
const GROUP_OPERATIONS: &str = "\ngraph groups はまとまりごとの nid の一覧です。同じまとまりの中で重複を比較して merge し、不足するエッジを add_edge、状態の訂正を set_status で返してください。\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub system: String,
    pub user: String,
    /// Only these utterances can be used as grounds for this request.
    pub allowed_uids: Vec<String>,
    /// New utterances included in this incremental request, in row order.
    pub new_uids: Vec<String>,
    /// Number of live issues omitted to fit the issue consolidation prompt.
    pub omitted_issues: usize,
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
    let mut context: Vec<_> = context_nodes(graph, recent_touched)
        .into_iter()
        .enumerate()
        .map(|(index, node)| {
            let revision = if node.updated_rev == 0 {
                node.created_rev
            } else {
                node.updated_rev
            };
            let line = json!({
                "nid": node.nid, "kind": node.kind, "status": node.status, "text": node.text,
            })
            .to_string();
            (index, revision, node, line)
        })
        .collect();
    // Trim by age across both groups, then restore their presentation order.
    context.sort_by_key(|(_, revision, node, _)| {
        std::cmp::Reverse((*revision, node.created_rev, &node.nid))
    });

    let frame = chars(&incremental_user(&[], &[], &[]));
    let budget = limit
        .checked_sub(chars(EXTRACT) + frame)
        .ok_or(PromptError::InputLimit)?;
    let third = limit / 3;
    let mut context_len: usize = context.iter().map(|(_, _, _, line)| chars(line) + 1).sum();
    while context_len > third.min(budget) {
        if let Some((_, _, _, line)) = context.pop() {
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
        let Some((_, _, _, line)) = context.pop() else {
            break;
        };
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
    context.sort_by_key(|(index, _, _, _)| *index);
    let context: Vec<_> = context.into_iter().map(|(_, _, _, line)| line).collect();
    Ok(Prompt {
        system: EXTRACT.to_owned(),
        user: incremental_user(&context, &ref_lines, &new_lines),
        allowed_uids,
        new_uids,
        omitted_issues: 0,
    })
}

fn context_nodes<'a>(graph: &'a Graph, recent_touched: &[BTreeSet<String>]) -> Vec<&'a Node> {
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
    let mut add = |node: &'a Node| {
        if seen.insert(&node.nid) {
            context.push(node);
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
}

/// Build the single baseline prompt for comparison with two-stage consolidation.
pub fn build_consolidate(
    graph: &Graph,
    style: ConsolidateStyle,
    limit: usize,
) -> Result<Vec<Prompt>, PromptError> {
    match style {
        ConsolidateStyle::Baseline => {
            let renderer = Consolidation::new(graph);
            let all = renderer.nodes.keys().copied().collect();
            renderer
                .fit_group(
                    CONSOLIDATE_EDITS,
                    &all,
                    false,
                    [(200, 3), (200, 1), (0, 0)],
                    limit,
                )
                .map(|(_, prompt)| vec![prompt])
        }
    }
}

/// Build one issue-only request. Apply its patch before building stage two.
/// Skip graphs with fewer than two live issues. Return an input-limit error
/// when there is no room to compare two issues even without evidence.
pub fn build_consolidate_issues(
    graph: &Graph,
    limit: usize,
) -> Result<Option<Prompt>, PromptError> {
    let renderer = Consolidation::new(graph);
    let mut issues: Vec<_> = renderer.issues().collect();
    if issues.len() < 2 {
        return Ok(None);
    }
    // Node IDs break ties when a single patch creates multiple issues.
    issues.sort_by_key(|node| {
        (
            node.created_rev,
            node.nid
                .strip_prefix('n')
                .and_then(|id| id.parse::<u64>().ok()),
            &node.nid,
        )
    });
    let system = format!("{CONSOLIDATE}{ISSUE_MERGES}");
    let issue_count = issues.len();
    let render = |issues: &[&Node], evidence| {
        let nodes = issues
            .iter()
            .map(|node| (node.nid.as_str(), evidence))
            .collect();
        renderer.render(&system, &nodes, &[], true)
    };
    let full = render(&issues, (100, 1));
    if full.fits(limit) {
        return Ok(Some(full));
    }
    while issues.len() >= 2 {
        let mut prompt = render(&issues, (0, 0));
        if prompt.fits(limit) {
            prompt.omitted_issues = issue_count - issues.len();
            return Ok(Some(prompt));
        }
        issues.pop();
    }
    Err(PromptError::InputLimit)
}

/// Pack issue groups from the graph after stage one's patch has been applied.
pub fn build_consolidate_groups(graph: &Graph, limit: usize) -> Result<Vec<Prompt>, PromptError> {
    let renderer = Consolidation::new(graph);
    let all = renderer.nodes.keys().copied().collect();
    let system = format!("{CONSOLIDATE_EDITS}{GROUP_OPERATIONS}");
    let mut batches: Vec<GroupBatch<'_>> = Vec::new();
    let groups = renderer.groups(&all);
    for group in &groups {
        let (evidence, prompt) = renderer.fit_group(
            &system,
            group,
            true,
            [
                (usize::MAX, usize::MAX),
                (200, usize::MAX),
                (200, 1),
                (0, 0),
            ],
            limit,
        )?;
        let mut packed = false;
        for batch in &mut batches {
            let mut candidate_groups = batch.groups.clone();
            candidate_groups.push(group);
            let mut candidate_evidence = batch.evidence.clone();
            for (&nid, &limits) in &evidence {
                let entry = candidate_evidence.entry(nid).or_insert(limits);
                *entry = (*entry).max(limits);
            }
            let candidate = renderer.render(&system, &candidate_evidence, &candidate_groups, false);
            if candidate.fits(limit) {
                *batch = GroupBatch {
                    groups: candidate_groups,
                    evidence: candidate_evidence,
                    prompt: candidate,
                };
                packed = true;
                break;
            }
        }
        if !packed {
            batches.push(GroupBatch {
                groups: vec![group],
                evidence,
                prompt,
            });
        }
    }
    Ok(batches.into_iter().map(|batch| batch.prompt).collect())
}

impl Prompt {
    fn fits(&self, limit: usize) -> bool {
        chars(&self.system) + chars(&self.user) <= limit
    }
}

type EvidenceLimits<'a> = BTreeMap<&'a str, (usize, usize)>;

struct GroupBatch<'a> {
    groups: Vec<&'a BTreeSet<&'a str>>,
    evidence: EvidenceLimits<'a>,
    prompt: Prompt,
}

struct Consolidation<'a> {
    nodes: BTreeMap<&'a str, &'a Node>,
    edges: Vec<&'a Edge>,
    evidence: BTreeMap<&'a str, Vec<&'a Utterance>>,
}

impl<'a> Consolidation<'a> {
    fn new(graph: &'a Graph) -> Self {
        let nodes = graph
            .nodes
            .values()
            .filter(|node| node.is_live())
            .map(|node| (node.nid.as_str(), node))
            .collect();
        let edges = graph.live_edges().collect();
        let mut evidence: BTreeMap<&str, Vec<&Utterance>> = BTreeMap::new();
        for ground in &graph.grounds {
            if let Some(utterance) = graph.utterances.get(&ground.uid) {
                evidence.entry(&ground.target).or_default().push(utterance);
            }
        }
        for utterances in evidence.values_mut() {
            utterances.sort_by_key(|u| (u.row_id, &u.uid));
            utterances.dedup_by_key(|u| &u.uid);
        }
        Self {
            nodes,
            edges,
            evidence,
        }
    }

    fn issues(&self) -> impl Iterator<Item = &'a Node> + '_ {
        self.nodes
            .values()
            .copied()
            .filter(|node| node.kind == NodeKind::Issue)
    }

    fn groups(&self, all: &BTreeSet<&'a str>) -> Vec<BTreeSet<&'a str>> {
        let mut incoming: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for edge in &self.edges {
            incoming.entry(&edge.dst).or_default().push(&edge.src);
        }
        let mut groups = Vec::new();
        let mut assigned = BTreeSet::new();
        for issue in self.issues() {
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
        if !unassigned.is_empty() {
            groups.push(unassigned);
        }
        groups
    }

    fn fit_group(
        &self,
        system: &str,
        group: &BTreeSet<&'a str>,
        grouped: bool,
        tiers: impl IntoIterator<Item = (usize, usize)>,
        limit: usize,
    ) -> Result<(EvidenceLimits<'a>, Prompt), PromptError> {
        let groups = if grouped {
            std::slice::from_ref(&group)
        } else {
            &[]
        };
        for limits in tiers {
            let evidence = group.iter().map(|&nid| (nid, limits)).collect();
            let prompt = self.render(system, &evidence, groups, false);
            if prompt.fits(limit) {
                return Ok((evidence, prompt));
            }
        }
        Err(PromptError::InputLimit)
    }

    fn render(
        &self,
        system: &str,
        evidence: &EvidenceLimits<'_>,
        groups: &[&BTreeSet<&str>],
        issues_only: bool,
    ) -> Prompt {
        let mut user = String::from("graph edges\n");
        for edge in &self.edges {
            let src = evidence.contains_key(edge.src.as_str());
            let dst = evidence.contains_key(edge.dst.as_str());
            // Include cross-group edges in stage two, even if the other
            // endpoint belongs to a different request.
            if (src && dst) || (!groups.is_empty() && (src || dst)) {
                push_line(
                    &mut user,
                    json!({
                        "eid":edge.eid, "src":edge.src, "dst":edge.dst,
                        "kind":edge.kind, "confidence":edge.confidence,
                    }),
                );
            }
        }
        if !groups.is_empty() {
            user.push_str("graph groups\n");
            for group in groups {
                push_line(&mut user, json!({"nids": group}));
            }
        }
        user.push_str("graph nodes\n");
        let mut allowed = BTreeSet::new();
        for (&nid, &(cap, count)) in evidence {
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
            let line = if issues_only {
                json!({"nid":nid, "text":node.text, "ground_count":utterances.len(), "grounds":grounds})
            } else {
                json!({
                    "nid":nid, "kind":node.kind, "status":node.status, "text":node.text,
                    "confidence":node.confidence, "ground_count":utterances.len(), "grounds":grounds,
                })
            };
            push_line(&mut user, line);
        }
        Prompt {
            system: system.to_owned(),
            user,
            allowed_uids: allowed.into_iter().collect(),
            new_uids: Vec::new(),
            omitted_issues: 0,
        }
    }
}

fn push_line(user: &mut String, line: serde_json::Value) {
    user.push_str(&line.to_string());
    user.push('\n');
}
