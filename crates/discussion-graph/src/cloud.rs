//! Conflict cloud (ADR 0006): candidates, prompt input and response validation.
//!
//! Everything here is pure. Node, utterance and response text must never reach
//! logs or error messages.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    prompt::{chars, prefix},
    Edge, EdgeKind, Graph, Node, NodeKind, Status, Utterance, DEFAULT_INPUT_LIMIT,
};

#[cfg(test)]
mod tests;

/// Maximum body length of a cloud element in UTF-16 units. svg-diagram allows 120
/// and the renderer may prepend a 4-unit mark.
pub const CLOUD_TEXT_UNITS: usize = 116;
const MAX_ASSUMPTIONS: usize = 8;
const MAX_PER_ARROW: usize = 2;
const OTHER_ISSUES: usize = 5;
const OTHER_ISSUE_CHARS: usize = 60;

/// The fixed system prompt. It never contains meeting text.
pub const CLOUD_SYSTEM: &str = include_str!("cloud-system.txt");

/// How an element relates to the meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Basis {
    /// Backed by at least one utterance in `grounds`.
    Stated,
    /// Supplied by the model.
    Inferred,
}

/// The arrow an assumption sits behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssumptionOn {
    Ab,
    Ac,
    Bd,
    Cd,
    Dd,
}

impl AssumptionOn {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "ab" => Self::Ab,
            "ac" => Self::Ac,
            "bd" => Self::Bd,
            "cd" => Self::Cd,
            "dd" => Self::Dd,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudElement {
    pub text: String,
    pub basis: Basis,
    /// Utterance IDs from the prompt. A `stated` element has at least one.
    #[serde(default)]
    pub grounds: Vec<String>,
    /// Related graph nodes. They never make an element `stated`.
    #[serde(default)]
    pub nids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudAssumption {
    pub on: AssumptionOn,
    pub text: String,
    pub basis: Basis,
    #[serde(default)]
    pub grounds: Vec<String>,
    #[serde(default)]
    pub nids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub injection: Option<CloudElement>,
}

/// A validated conflict cloud. This is the `body_json` of a saved analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudDiagram {
    pub a: CloudElement,
    pub b: CloudElement,
    pub c: CloudElement,
    pub d: CloudElement,
    pub d2: CloudElement,
    pub assumptions: Vec<CloudAssumption>,
}

/// A validated cloud with the counts to store beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValidatedCloud {
    pub diagram: CloudDiagram,
    /// Dropped elements, assumptions, injections, grounds and node IDs.
    pub dropped: usize,
    /// `stated` elements that lost every ground.
    pub downgraded: usize,
}

/// A candidate subject for a conflict cloud.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConflictCandidate {
    pub issue: String,
    /// Live positions answering the issue, in nid order.
    pub positions: Vec<String>,
    /// The default pair: the two positions with the most supports and
    /// objections (ties by older `created_rev`, then nid), in nid order.
    pub default_pair: [String; 2],
    pub decided: bool,
    pub has_objection: bool,
    /// Supports plus objections on the default pair.
    pub reasons: usize,
}

impl ConflictCandidate {
    pub fn subject(&self) -> String {
        conflict_subject(&self.issue, &self.default_pair[0], &self.default_pair[1])
    }
}

/// The key of a conflict cloud: `"{issue}:{pos}:{pos}"` with positions in
/// ascending string order.
pub fn conflict_subject(issue: &str, first: &str, second: &str) -> String {
    let (low, high) = if first <= second {
        (first, second)
    } else {
        (second, first)
    };
    format!("{issue}:{low}:{high}")
}

fn is_live_position(node: &Node) -> bool {
    node.kind == NodeKind::Position && node.is_live() && node.status != Status::Retracted
}

fn is_live_reason(node: &Node) -> bool {
    node.kind == NodeKind::Argument && node.is_live() && node.status != Status::Retracted
}

fn is_live_decision(node: &Node) -> bool {
    node.kind == NodeKind::Decision && node.is_live() && node.status != Status::Retracted
}

fn reason_edges(graph: &Graph) -> BTreeMap<&str, Vec<&Edge>> {
    let mut incoming = BTreeMap::<_, Vec<_>>::new();
    for edge in graph.live_edges() {
        if matches!(edge.kind, EdgeKind::Supports | EdgeKind::ObjectsTo)
            && graph.nodes.get(&edge.src).is_some_and(is_live_reason)
        {
            incoming.entry(edge.dst.as_str()).or_default().push(edge);
        }
    }
    incoming
}

/// Issues with two or more live positions, each with a default pair.
/// Undecided issues come first, then those with an objection, then those with
/// more reasons on the pair, then older issues.
pub fn conflict_candidates(graph: &Graph) -> Vec<ConflictCandidate> {
    let reasons = reason_edges(graph);
    let mut answers = BTreeMap::<&str, BTreeSet<&str>>::new();
    let mut decided = BTreeSet::new();
    for edge in graph.live_edges() {
        match edge.kind {
            EdgeKind::RespondsTo if graph.nodes.get(&edge.src).is_some_and(is_live_position) => {
                answers
                    .entry(edge.dst.as_str())
                    .or_default()
                    .insert(edge.src.as_str());
            }
            EdgeKind::Resolves if graph.nodes.get(&edge.src).is_some_and(is_live_decision) => {
                decided.insert(edge.dst.as_str());
            }
            _ => {}
        }
    }
    let count = |nid: &str| reasons.get(nid).map_or(0, Vec::len);
    let mut found = Vec::new();
    for (issue, positions) in answers {
        let Some(node) = graph.nodes.get(issue) else {
            continue;
        };
        if node.kind != NodeKind::Issue || positions.len() < 2 {
            continue;
        }
        let mut ranked: Vec<&str> = positions.iter().copied().collect();
        ranked.sort_by_key(|nid| (Reverse(count(nid)), graph.nodes[*nid].created_rev, *nid));
        let mut pair = [ranked[0], ranked[1]];
        pair.sort_unstable();
        let has_objection = positions.iter().any(|nid| {
            reasons
                .get(nid)
                .is_some_and(|edges| edges.iter().any(|edge| edge.kind == EdgeKind::ObjectsTo))
        });
        found.push((
            node.created_rev,
            ConflictCandidate {
                issue: issue.to_owned(),
                positions: positions.iter().map(|nid| (*nid).to_owned()).collect(),
                default_pair: pair.map(str::to_owned),
                decided: decided.contains(issue),
                has_objection,
                reasons: count(pair[0]) + count(pair[1]),
            },
        ));
    }
    found.sort_by(|(left_rev, left), (right_rev, right)| {
        (
            left.decided,
            !left.has_objection,
            Reverse(left.reasons),
            left_rev,
            &left.issue,
        )
            .cmp(&(
                right.decided,
                !right.has_objection,
                Reverse(right.reasons),
                right_rev,
                &right.issue,
            ))
    });
    found.into_iter().map(|(_, candidate)| candidate).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CloudInputError {
    /// The issue or a position is missing, not live, or not an answer to the issue.
    #[error("the conflict cloud subject is not valid")]
    InvalidSubject,
    /// Even the smallest input exceeds the limit. Do not call the model.
    #[error("the question is too large to draw as a conflict cloud")]
    TooLarge,
}

/// The reduction applied to fit the input limit, from none to the last step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum CloudInputStage {
    Full,
    OneUtterancePerNode,
    ShortUtterances,
    NoOtherIssues,
    NoSecondLevelReasons,
}

const STAGES: [CloudInputStage; 5] = [
    CloudInputStage::Full,
    CloudInputStage::OneUtterancePerNode,
    CloudInputStage::ShortUtterances,
    CloudInputStage::NoOtherIssues,
    CloudInputStage::NoSecondLevelReasons,
];

/// A position copied into the diagram as D or D2.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CloudAnchor {
    pub nid: String,
    pub text: String,
    /// Every utterance grounding the position.
    pub grounds: Vec<String>,
}

/// A request for one conflict cloud.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CloudPrompt {
    pub system: String,
    pub user: String,
    pub subject: String,
    /// D then D2: the positions in ascending nid order.
    pub anchors: [CloudAnchor; 2],
    /// Utterances in the input. Only these can ground a `stated` element.
    pub allowed_uids: Vec<String>,
    /// Nodes in the input.
    pub allowed_nids: Vec<String>,
    pub stage: CloudInputStage,
}

struct Reason<'a> {
    node: &'a Node,
    relation: EdgeKind,
    on: &'a str,
    second_level: bool,
    /// Further relations of the same node, in target order. On a first-level row
    /// they are second-level relations, dropped with the second level.
    also: Vec<(EdgeKind, &'a str)>,
}

struct Material<'a> {
    graph: &'a Graph,
    issue: &'a Node,
    positions: [&'a Node; 2],
    others: Vec<&'a Node>,
    reasons: Vec<Reason<'a>>,
    decisions: Vec<&'a Node>,
    actions: Vec<&'a Node>,
}

impl<'a> Material<'a> {
    fn collect(graph: &'a Graph, issue: &str, positions: [&str; 2]) -> Option<Self> {
        let [first, second] = positions;
        if first == second {
            return None;
        }
        let issue = graph
            .nodes
            .get(issue)
            .filter(|node| node.kind == NodeKind::Issue && node.is_live())?;
        let mut pair = [first, second];
        pair.sort_unstable();
        let mut found = Vec::new();
        for nid in pair {
            let node = graph.nodes.get(nid).filter(|node| is_live_position(node))?;
            graph.live_edges().find(|edge| {
                edge.kind == EdgeKind::RespondsTo && edge.src == nid && edge.dst == issue.nid
            })?;
            found.push(node);
        }
        let positions = [found[0], found[1]];

        let parents = graph.issue_parents();
        let mut others: Vec<_> = graph
            .nodes
            .values()
            .filter(|node| {
                node.kind == NodeKind::Issue
                    && node.is_live()
                    && node.nid != issue.nid
                    && !parents.contains_key(node.nid.as_str())
            })
            .collect();
        others.sort_by_key(|node| (node.created_rev, &node.nid));
        others.truncate(OTHER_ISSUES);

        let incoming = reason_edges(graph);
        let mut reasons: Vec<Reason<'a>> = Vec::new();
        for second_level in [false, true] {
            let mut targets: Vec<&str> = if second_level {
                reasons
                    .iter()
                    .map(|reason| reason.node.nid.as_str())
                    .collect()
            } else {
                positions.iter().map(|node| node.nid.as_str()).collect()
            };
            // A reason listed on two rows is still one target.
            let mut once = BTreeSet::new();
            targets.retain(|target| once.insert(*target));
            let mut level: Vec<Reason<'a>> = Vec::new();
            for target in targets {
                for edge in incoming.get(target).into_iter().flatten() {
                    let node = &graph.nodes[&edge.src];
                    let on = target_of(graph, target);
                    if second_level {
                        if node.nid == target {
                            continue;
                        }
                        // The node stays one line, at the first level if it is
                        // there; its other relations ride on that line.
                        if let Some(row) = reasons
                            .iter_mut()
                            .chain(level.iter_mut())
                            .find(|reason| reason.node.nid == node.nid)
                        {
                            let link = (edge.kind, on);
                            if (row.relation, row.on) != link && !row.also.contains(&link) {
                                row.also.push(link);
                            }
                            continue;
                        }
                    }
                    level.push(Reason {
                        node,
                        relation: edge.kind,
                        on,
                        second_level,
                        also: Vec::new(),
                    });
                }
            }
            reasons.extend(level);
        }
        reasons.sort_by_key(|reason| {
            (
                reason.second_level,
                reason.node.created_rev,
                &reason.node.nid,
                reason.on,
            )
        });

        let mut decisions = Vec::new();
        let mut actions = Vec::new();
        for edge in graph.live_edges() {
            if edge.kind == EdgeKind::Resolves
                && edge.dst == issue.nid
                && graph.nodes.get(&edge.src).is_some_and(is_live_decision)
            {
                decisions.push(&graph.nodes[&edge.src]);
            }
        }
        decisions.sort_by_key(|node| (node.created_rev, &node.nid));
        decisions.dedup_by_key(|node| &node.nid);
        for edge in graph.live_edges() {
            if edge.kind == EdgeKind::Assigns
                && !decisions.is_empty()
                && (edge.dst == issue.nid || decisions.iter().any(|node| node.nid == edge.dst))
                && graph.nodes.get(&edge.src).is_some_and(|node| {
                    node.kind == NodeKind::Action && node.status != Status::Retracted
                })
            {
                actions.push(&graph.nodes[&edge.src]);
            }
        }
        actions.sort_by_key(|node| (node.created_rev, &node.nid));
        actions.dedup_by_key(|node| &node.nid);

        Some(Self {
            graph,
            issue,
            positions,
            others,
            reasons,
            decisions,
            actions,
        })
    }

    /// Utterances grounding a node, oldest row first.
    fn evidence(&self, nid: &str) -> Vec<&'a Utterance> {
        let mut found: Vec<_> = self
            .graph
            .grounds
            .iter()
            .filter(|ground| ground.target == nid)
            .filter_map(|ground| self.graph.utterances.get(&ground.uid))
            .collect();
        found.sort_by_key(|utterance| (utterance.row_id, &utterance.uid));
        found.dedup_by_key(|utterance| &utterance.uid);
        found
    }

    fn render(&self, stage: CloudInputStage) -> (String, BTreeSet<String>, BTreeSet<String>) {
        let one = stage >= CloudInputStage::OneUtterancePerNode;
        let cap = if stage >= CloudInputStage::ShortUtterances {
            100
        } else {
            200
        };
        let with_others = stage < CloudInputStage::NoOtherIssues;
        let with_second = stage < CloudInputStage::NoSecondLevelReasons;

        let mut shown: BTreeMap<&str, &Utterance> = BTreeMap::new();
        let mut nids = BTreeSet::new();
        let mut node_line = |node: &Node, extra: Value| {
            nids.insert(node.nid.clone());
            let mut uids: Vec<&Utterance> = self.evidence(&node.nid);
            if one {
                uids.truncate(1);
            }
            let mut line = json!({
                "nid": node.nid,
                "text": node.text,
                "uids": uids.iter().map(|u| &u.uid).collect::<Vec<_>>(),
            });
            for (key, value) in extra.as_object().into_iter().flatten() {
                line[key] = value.clone();
            }
            shown.extend(uids.into_iter().map(|u| (u.uid.as_str(), u)));
            line.to_string()
        };

        let mut other_nids = Vec::new();
        let mut user = String::from("issue\n");
        user.push_str(&node_line(self.issue, json!({})));
        user.push('\n');
        user.push_str("other_issues\n");
        if with_others {
            for node in &self.others {
                other_nids.push(node.nid.clone());
                let line = json!({"nid": node.nid, "text": prefix(&node.text, OTHER_ISSUE_CHARS)});
                user.push_str(&format!("{line}\n"));
            }
        }
        user.push_str("positions\n");
        for (slot, node) in ["d", "d2"].into_iter().zip(self.positions) {
            user.push_str(&node_line(node, json!({ "slot": slot })));
            user.push('\n');
        }
        user.push_str("arguments\n");
        for reason in self
            .reasons
            .iter()
            .filter(|reason| with_second || !reason.second_level)
        {
            let mut extra = json!({ "relation": reason.relation, "on": reason.on });
            // `also` of a first-level row holds second-level relations.
            if with_second && !reason.also.is_empty() {
                extra["also"] = reason
                    .also
                    .iter()
                    .map(|(relation, on)| json!({ "relation": relation, "on": on }))
                    .collect();
            }
            user.push_str(&node_line(reason.node, extra));
            user.push('\n');
        }
        for (header, nodes) in [("decisions", &self.decisions), ("actions", &self.actions)] {
            user.push_str(header);
            user.push('\n');
            for node in nodes {
                user.push_str(&node_line(node, json!({})));
                user.push('\n');
            }
        }
        user.push_str("utterances\n");
        let mut ordered: Vec<_> = shown.values().copied().collect();
        ordered.sort_by_key(|u| (u.row_id, &u.uid));
        for utterance in &ordered {
            let line = json!({
                "uid": utterance.uid,
                "speaker_label": utterance.speaker_label,
                "text": prefix(&utterance.text, cap),
            });
            user.push_str(&format!("{line}\n"));
        }
        let uids = ordered.iter().map(|u| u.uid.clone()).collect();
        nids.extend(other_nids);
        (user, uids, nids)
    }
}

fn target_of<'a>(graph: &'a Graph, nid: &str) -> &'a str {
    graph.nodes.get(nid).map_or("", |node| node.nid.as_str())
}

/// Build the input for the two chosen positions, reducing it in ADR order until
/// it fits `limit` characters (system and user together). Use
/// [`DEFAULT_INPUT_LIMIT`] for the standard 24,000.
pub fn build_conflict_cloud(
    graph: &Graph,
    issue: &str,
    positions: [&str; 2],
    limit: usize,
) -> Result<CloudPrompt, CloudInputError> {
    let material =
        Material::collect(graph, issue, positions).ok_or(CloudInputError::InvalidSubject)?;
    for stage in STAGES {
        let (user, uids, nids) = material.render(stage);
        if chars(CLOUD_SYSTEM) + chars(&user) > limit {
            continue;
        }
        let anchors = material.positions.map(|node| CloudAnchor {
            nid: node.nid.clone(),
            text: node.text.clone(),
            grounds: unique(
                graph
                    .grounds
                    .iter()
                    .filter(|ground| ground.target == node.nid)
                    .map(|ground| ground.uid.clone()),
            ),
        });
        return Ok(CloudPrompt {
            system: CLOUD_SYSTEM.to_owned(),
            user,
            subject: conflict_subject(issue, positions[0], positions[1]),
            anchors,
            allowed_uids: uids.into_iter().collect(),
            allowed_nids: nids.into_iter().collect(),
            stage,
        });
    }
    Err(CloudInputError::TooLarge)
}

/// Same as [`build_conflict_cloud`] with the default 24,000-character limit.
pub fn build_conflict_cloud_default(
    graph: &Graph,
    issue: &str,
    positions: [&str; 2],
) -> Result<CloudPrompt, CloudInputError> {
    build_conflict_cloud(graph, issue, positions, DEFAULT_INPUT_LIMIT)
}

fn unique(items: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    items.filter(|item| seen.insert(item.clone())).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CloudParseError {
    #[error("the response is not a JSON object")]
    NotJson,
    #[error("the response lacks a usable a, b or c")]
    MissingElement,
}

/// Cut to `units` UTF-16 units without splitting a surrogate pair.
pub fn truncate_utf16(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (index, ch) in text.char_indices() {
        used += ch.len_utf16();
        if used > units {
            return &text[..index];
        }
    }
    text
}

fn first_object(response: &str) -> Option<serde_json::Map<String, Value>> {
    let start = response.match_indices('{').find_map(|(start, _)| {
        let next = response[start + 1..].trim_start().chars().next();
        matches!(next, Some('"' | '}') | None).then_some(start)
    })?;
    match serde_json::Deserializer::from_str(&response[start..])
        .into_iter::<Value>()
        .next()?
        .ok()?
    {
        Value::Object(fields) => Some(fields),
        _ => None,
    }
}

struct Counts {
    dropped: usize,
    downgraded: usize,
}

struct Checker<'a> {
    uids: BTreeSet<&'a str>,
    nids: BTreeSet<&'a str>,
}

impl Checker<'_> {
    fn list(
        &self,
        value: Option<&Value>,
        allowed: &BTreeSet<&str>,
        counts: &mut Counts,
    ) -> Vec<String> {
        let mut kept = BTreeSet::new();
        let mut out = Vec::new();
        for item in value.and_then(Value::as_array).into_iter().flatten() {
            match item.as_str() {
                Some(id) if allowed.contains(id) && kept.insert(id) => out.push(id.to_owned()),
                _ => counts.dropped += 1,
            }
        }
        out
    }

    fn has_text(value: &Value) -> bool {
        value
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty())
    }

    /// Read one element. `None` when the text is missing or empty.
    fn element(&self, value: &Value, counts: &mut Counts) -> Option<CloudElement> {
        let text = value.get("text")?.as_str()?.trim();
        if text.is_empty() {
            return None;
        }
        let grounds = self.list(value.get("grounds"), &self.uids, counts);
        let nids = self.list(value.get("nids"), &self.nids, counts);
        let mut basis = match value.get("basis").and_then(Value::as_str) {
            Some("stated") => Basis::Stated,
            _ => Basis::Inferred,
        };
        if basis == Basis::Stated && grounds.is_empty() {
            basis = Basis::Inferred;
            counts.downgraded += 1;
        }
        Some(CloudElement {
            text: truncate_utf16(text, CLOUD_TEXT_UNITS).to_owned(),
            basis,
            grounds,
            nids,
        })
    }
}

/// Validate a model response against its prompt (ADR 0006 section 5). Invalid
/// parts are dropped; a missing a, b or c fails the whole response.
pub fn validate_cloud(
    response: &str,
    prompt: &CloudPrompt,
) -> Result<ValidatedCloud, CloudParseError> {
    let fields = first_object(response).ok_or(CloudParseError::NotJson)?;
    let checker = Checker {
        uids: prompt.allowed_uids.iter().map(String::as_str).collect(),
        nids: prompt.allowed_nids.iter().map(String::as_str).collect(),
    };
    let mut counts = Counts {
        dropped: 0,
        downgraded: 0,
    };
    let required = |key: &str, counts: &mut Counts| {
        fields
            .get(key)
            .and_then(|value| checker.element(value, counts))
            .ok_or(CloudParseError::MissingElement)
    };
    let a = required("a", &mut counts)?;
    let b = required("b", &mut counts)?;
    let c = required("c", &mut counts)?;

    let mut assumptions = Vec::new();
    let mut per_arrow = BTreeMap::<AssumptionOn, usize>::new();
    for item in fields
        .get("assumptions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let on = item
            .get("on")
            .and_then(Value::as_str)
            .and_then(AssumptionOn::parse);
        // The arrow and the text come first, then the quotas, then grounds and
        // downgrades, so a dropped assumption adds one to `dropped` and nothing else.
        let (Some(on), true) = (on, Checker::has_text(item)) else {
            counts.dropped += 1;
            continue;
        };
        let used = per_arrow.entry(on).or_default();
        if *used >= MAX_PER_ARROW || assumptions.len() >= MAX_ASSUMPTIONS {
            counts.dropped += 1;
            continue;
        }
        *used += 1;
        let Some(element) = checker.element(item, &mut counts) else {
            counts.dropped += 1;
            continue;
        };
        let injection = match item.get("injection") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let injection = checker.element(value, &mut counts);
                if injection.is_none() {
                    counts.dropped += 1;
                }
                injection
            }
        };
        assumptions.push(CloudAssumption {
            on,
            text: element.text,
            basis: element.basis,
            grounds: element.grounds,
            nids: element.nids,
            injection,
        });
    }

    // D and D2 come last: the model supplies only text.
    let anchored = |key: &str, anchor: &CloudAnchor| {
        let text = fields
            .get(key)
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| anchor.text.trim());
        CloudElement {
            text: truncate_utf16(text, CLOUD_TEXT_UNITS).to_owned(),
            basis: Basis::Stated,
            grounds: anchor.grounds.clone(),
            nids: vec![anchor.nid.clone()],
        }
    };
    let d = anchored("d", &prompt.anchors[0]);
    let d2 = anchored("d2", &prompt.anchors[1]);
    Ok(ValidatedCloud {
        diagram: CloudDiagram {
            a,
            b,
            c,
            d,
            d2,
            assumptions,
        },
        dropped: counts.dropped,
        downgraded: counts.downgraded,
    })
}
