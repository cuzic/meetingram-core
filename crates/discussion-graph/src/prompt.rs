use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use crate::{Edge, EdgeKind, Graph, Node, NodeKind, Status, Utterance};

pub const DEFAULT_INPUT_LIMIT: usize = 24_000;

const EXTRACT: &str = concat!(
    include_str!("extraction.txt"),
    include_str!("extraction-r11.txt"),
    include_str!("extraction-r12.txt"),
    include_str!("extraction-r13.txt"),
    "増分では後から案が出るかは分かりません。明らかに事実・数値・可否の確認の問いのときだけ親を付けてください。付けた親が後で合わなくなったら整理の段 1 で外します。\n"
);
const BASELINE: &str = concat!(
    include_str!("extraction.txt"),
    "\n会議は終わりました。今回は graph と根拠の発話を読みます。\n",
    "重複の merge、抜けたエッジの add_edge、状態の訂正の set_status を返してください。\n",
    "新規ノードは原則作らず、提示された根拠の uid を使ってください。\n"
);
const UID_ONLY: (usize, usize) = (0, 3);
const GROUP_EVIDENCE_TIERS: [(usize, usize); 4] = [
    (usize::MAX, usize::MAX),
    (200, usize::MAX),
    (200, 1),
    UID_ONLY,
];
const CONSOLIDATE: &str = include_str!("consolidation.txt");
const CONSOLIDATE_EDITS: &str = concat!(
    include_str!("consolidation.txt"),
    include_str!("consolidation-edits.txt")
);
const ISSUE_OPERATIONS: &str = concat!(
    r#"
全 issue の組を比較し、同じ問いの言い換えは issue 同士の merge で統合してください。ground_count が多い方を into にしてください。
parent_nid は既存の親です。親の無い細かな問いは、子を src、親の議題を dst にして sub_issue_of の add_edge でつないでください。親は 1 つまでで、循環させないでください。
{"op":"add_edge","src":"n8","dst":"n5","kind":"sub_issue_of","confidence":0.8,"grounds":[{"uid":"u3"}]}
既存の親子も見直してください。細かな問いに当たらない issue の親と、進め方の問いへの親は remove_edge で外してください。必要なら同じ応答の後続の add_edge で別の親に付け直せます。remove_edge は段 1 の sub_issue_of に限ります。
{"op":"remove_edge","src":"n8","dst":"n5","kind":"sub_issue_of","grounds":[{"uid":"u3"}]}
position_count はその issue に responds_to する position の数です。has_objection はその中に objects_to を受けた position があることを示します。
decisions はその issue に resolves する決定、positions はその issue に responds_to する案の本文です。issue の本文が違っても、決定や案が同じなら同じ問いの言い換えとして merge を検討してください。
"#,
    include_str!("extraction-r13.txt")
);
const GROUP_OPERATIONS: &str = "\ngraph groups はまとまりごとの nid の一覧です。同じまとまりの中で重複を比較して merge し、不足するエッジを add_edge、状態の訂正を set_status で返してください。external のエッジも既に存在するので、add_edge で出し直さないでください。\n";

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Prompt {
    pub system: String,
    pub user: String,
    /// Only these utterances can be used as grounds for this request.
    pub allowed_uids: Vec<String>,
    /// New utterances included in this incremental request, in row order.
    pub new_uids: Vec<String>,
    /// Number of live issues omitted to fit the issue consolidation prompt.
    pub omitted_issues: usize,
    /// The input reduction needed for stage one, if this is an issue prompt.
    pub issue_stage: Option<IssuePromptStage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IssuePromptStage {
    FullAnnotations,
    ShortDecision,
    NoAnnotations,
    UidOnly,
    OmittedIssues,
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
    let parents = graph.issue_parents();
    let mut context: Vec<_> = context_nodes(graph, recent_touched)
        .into_iter()
        .enumerate()
        .map(|(index, node)| {
            let revision = if node.updated_rev == 0 {
                node.created_rev
            } else {
                node.updated_rev
            };
            let mut line = json!({
                "nid": node.nid, "kind": node.kind, "status": node.status, "text": node.text,
            });
            if let Some(parent) = parents.get(node.nid.as_str()) {
                line["parent_nid"] = json!(parent);
            }
            (index, revision, node, line.to_string())
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
        issue_stage: None,
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
                    BASELINE,
                    &all,
                    Layout::Baseline,
                    [(200, 3), (200, 1), UID_ONLY],
                    limit,
                )
                .map(|(_, prompt)| vec![prompt])
        }
    }
}

/// Build one issue-only request. Apply its patch before building stage two.
/// Skip graphs with fewer than two live issues. Return an input-limit error
/// when there is no room to compare two issues even with UID-only evidence.
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
    let system = format!("{CONSOLIDATE}{ISSUE_OPERATIONS}");
    let issue_count = issues.len();
    let render = |issues: &[&Node], evidence, stage| {
        let nodes = issues
            .iter()
            .map(|node| (node.nid.as_str(), evidence))
            .collect();
        renderer.render(&system, &nodes, &[], Layout::Issues(stage))
    };
    for (evidence, stage) in [
        ((100, 1), IssuePromptStage::FullAnnotations),
        ((100, 1), IssuePromptStage::ShortDecision),
        ((100, 1), IssuePromptStage::NoAnnotations),
        (UID_ONLY, IssuePromptStage::UidOnly),
    ] {
        let prompt = render(&issues, evidence, stage);
        if prompt.fits(limit) {
            return Ok(Some(prompt));
        }
    }
    while issues.len() > 2 {
        issues.pop();
        let mut prompt = render(&issues, UID_ONLY, IssuePromptStage::OmittedIssues);
        if prompt.fits(limit) {
            prompt.omitted_issues = issue_count - issues.len();
            return Ok(Some(prompt));
        }
    }
    Err(PromptError::InputLimit)
}

/// Pack issue groups from the graph after stage one's patch has been applied.
/// Split oversized groups by node count. Return the prompts and the number
/// of nodes omitted because they could not fit even with UID-only evidence.
pub fn build_consolidate_groups(graph: &Graph, limit: usize) -> (Vec<Prompt>, usize) {
    let renderer = Consolidation::new(graph);
    let all = renderer.nodes.keys().copied().collect();
    let system = format!("{CONSOLIDATE_EDITS}{GROUP_OPERATIONS}");
    let mut fitted = Vec::new();
    let mut omitted = 0;
    for group in renderer.groups(&all) {
        omitted += renderer.fit_or_split(&system, group, &GROUP_EVIDENCE_TIERS, limit, &mut fitted);
    }

    let mut batches: Vec<GroupBatch<'_>> = Vec::new();
    'groups: for (group, evidence, prompt) in fitted {
        for batch in &mut batches {
            let candidate_groups: Vec<_> =
                batch.groups.iter().chain(std::iter::once(&group)).collect();
            let mut candidate_evidence = batch.evidence.clone();
            candidate_evidence.extend(evidence.iter());
            let candidate = renderer.render(
                &system,
                &candidate_evidence,
                &candidate_groups,
                Layout::Groups,
            );
            if candidate.fits(limit) {
                batch.groups.push(group);
                batch.evidence = candidate_evidence;
                batch.prompt = candidate;
                continue 'groups;
            }
        }
        batches.push(GroupBatch {
            groups: vec![group],
            evidence,
            prompt,
        });
    }
    (
        batches.into_iter().map(|batch| batch.prompt).collect(),
        omitted,
    )
}

impl Prompt {
    fn fits(&self, limit: usize) -> bool {
        chars(&self.system) + chars(&self.user) <= limit
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    Baseline,
    Issues(IssuePromptStage),
    Groups,
}

type EvidenceLimits<'a> = BTreeMap<&'a str, (usize, usize)>;

type FittedGroup<'a> = (BTreeSet<&'a str>, EvidenceLimits<'a>, Prompt);

struct GroupBatch<'a> {
    groups: Vec<BTreeSet<&'a str>>,
    evidence: EvidenceLimits<'a>,
    prompt: Prompt,
}

struct Consolidation<'a> {
    nodes: BTreeMap<&'a str, &'a Node>,
    edges: Vec<&'a Edge>,
    parents: BTreeMap<&'a str, &'a str>,
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
            parents: graph.issue_parents(),
            evidence,
        }
    }

    fn issues(&self) -> impl Iterator<Item = &'a Node> + '_ {
        self.nodes
            .values()
            .copied()
            .filter(|node| node.kind == NodeKind::Issue)
    }

    fn issue_links(&self, nid: &str, kind: EdgeKind, source_kind: NodeKind) -> Vec<&'a Node> {
        let mut linked: Vec<_> = self
            .edges
            .iter()
            .filter(|edge| edge.dst == nid && edge.kind == kind)
            .map(|edge| self.nodes[edge.src.as_str()])
            .filter(|node| node.kind == source_kind)
            .collect();
        linked.sort_unstable_by_key(|node| (node.created_rev, &node.nid));
        linked.dedup_by_key(|node| &node.nid);
        linked
    }

    fn groups(&self, all: &BTreeSet<&'a str>) -> Vec<BTreeSet<&'a str>> {
        let mut incoming: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for edge in &self.edges {
            incoming.entry(&edge.dst).or_default().push(&edge.src);
        }
        let mut groups = Vec::new();
        let mut assigned = BTreeSet::new();
        for issue in self
            .issues()
            .filter(|node| !self.parents.contains_key(node.nid.as_str()))
        {
            let id = issue.nid.as_str();
            let mut group = BTreeSet::new();
            let mut pending = vec![id];
            while let Some(nid) = pending.pop() {
                if assigned.insert(nid) {
                    group.insert(nid);
                    pending.extend(incoming.get(nid).into_iter().flatten().copied());
                }
            }
            if !group.is_empty() {
                groups.push(group);
            }
        }
        let unassigned: BTreeSet<_> = all.difference(&assigned).copied().collect();
        if !unassigned.is_empty() {
            groups.push(unassigned);
        }
        groups
    }

    fn fit_or_split(
        &self,
        system: &str,
        mut group: BTreeSet<&'a str>,
        tiers: &[(usize, usize)],
        limit: usize,
        fitted: &mut Vec<FittedGroup<'a>>,
    ) -> usize {
        if let Ok((evidence, prompt)) =
            self.fit_group(system, &group, Layout::Groups, tiers.iter().copied(), limit)
        {
            fitted.push((group, evidence, prompt));
            return 0;
        }
        if group.len() == 1 {
            return 1;
        }
        let midpoint = *group.iter().nth(group.len() / 2).unwrap();
        let second = group.split_off(midpoint);
        let tiers = &GROUP_EVIDENCE_TIERS[1..];
        self.fit_or_split(system, group, tiers, limit, fitted)
            + self.fit_or_split(system, second, tiers, limit, fitted)
    }

    fn fit_group(
        &self,
        system: &str,
        group: &BTreeSet<&'a str>,
        layout: Layout,
        tiers: impl IntoIterator<Item = (usize, usize)>,
        limit: usize,
    ) -> Result<(EvidenceLimits<'a>, Prompt), PromptError> {
        let groups = if layout == Layout::Groups {
            std::slice::from_ref(&group)
        } else {
            &[]
        };
        for limits in tiers {
            let evidence = group.iter().map(|&nid| (nid, limits)).collect();
            let prompt = self.render(system, &evidence, groups, layout);
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
        layout: Layout,
    ) -> Prompt {
        let mut edges = String::from("graph edges\n");
        for edge in &self.edges {
            let has_src = evidence.contains_key(edge.src.as_str());
            let has_dst = evidence.contains_key(edge.dst.as_str());
            if (!matches!(layout, Layout::Issues(_)) || edge.kind == EdgeKind::SubIssueOf)
                && has_src
                && has_dst
            {
                push_line(
                    &mut edges,
                    json!({
                        "eid":edge.eid, "src":edge.src, "dst":edge.dst,
                        "kind":edge.kind, "confidence":edge.confidence,
                    }),
                );
            } else if layout == Layout::Groups && (has_src || has_dst) {
                push_line(
                    &mut edges,
                    json!({
                        "eid":edge.eid, "src":edge.src, "dst":edge.dst,
                        "kind":edge.kind, "external":true,
                    }),
                );
            }
        }
        let mut user = String::new();
        if layout != Layout::Baseline {
            user.push_str(&edges);
        }
        if !groups.is_empty() {
            user.push_str("graph groups\n");
            for group in groups {
                push_line(&mut user, json!({"nids": group}));
            }
        }
        user.push_str("graph nodes\n");
        let mut allowed = BTreeSet::new();
        let objections: BTreeSet<_> = self
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::ObjectsTo)
            .map(|edge| edge.dst.as_str())
            .collect();
        for (&nid, &(cap, count)) in evidence {
            let node = self.nodes[nid];
            let utterances = self
                .evidence
                .get(nid)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let grounds: Vec<_> = utterances.iter().take(count).map(|u| {
                allowed.insert(u.uid.clone());
                if cap == 0 {
                    json!({"uid":u.uid})
                } else {
                    json!({"uid":u.uid, "speaker_label":u.speaker_label, "text":prefix(&u.text, cap)})
                }
            }).collect();
            let line = if let Layout::Issues(stage) = layout {
                let positions = self.issue_links(nid, EdgeKind::RespondsTo, NodeKind::Position);
                let has_objection = positions
                    .iter()
                    .any(|position| objections.contains(position.nid.as_str()));
                let mut line = json!({
                    "nid":nid, "parent_nid":self.parents.get(nid), "text":node.text,
                    "ground_count":utterances.len(), "grounds":grounds,
                    "position_count":positions.len(), "has_objection":has_objection,
                });
                let (count, cap) = match stage {
                    IssuePromptStage::FullAnnotations => (2, 60),
                    IssuePromptStage::ShortDecision => (1, 40),
                    _ => (0, 0),
                };
                if count > 0 {
                    let decisions = self.issue_links(nid, EdgeKind::Resolves, NodeKind::Decision);
                    line["decisions"] = json!(decisions
                        .iter()
                        .take(count)
                        .map(|node| prefix(&node.text, cap))
                        .collect::<Vec<_>>());
                }
                if stage == IssuePromptStage::FullAnnotations {
                    line["positions"] = json!(positions
                        .iter()
                        .take(count)
                        .map(|node| prefix(&node.text, cap))
                        .collect::<Vec<_>>());
                }
                line
            } else {
                json!({
                    "nid":nid, "kind":node.kind, "status":node.status, "text":node.text,
                    "confidence":node.confidence, "ground_count":utterances.len(), "grounds":grounds,
                })
            };
            push_line(&mut user, line);
        }
        if layout == Layout::Baseline {
            user.push_str(&edges);
        }
        Prompt {
            system: system.to_owned(),
            user,
            allowed_uids: allowed.into_iter().collect(),
            new_uids: Vec::new(),
            omitted_issues: 0,
            issue_stage: match layout {
                Layout::Issues(stage) => Some(stage),
                _ => None,
            },
        }
    }
}

fn push_line(user: &mut String, line: serde_json::Value) {
    user.push_str(&line.to_string());
    user.push('\n');
}
