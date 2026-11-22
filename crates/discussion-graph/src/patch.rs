use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Act, Edge, EdgeKind, Graph, Ground, Node, NodeKind, Review, Status};

/// An operation as returned by the model. Individual entries remain unvalidated
/// so that an invalid entry cannot prevent the rest of a response from applying.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Op(pub Value);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    #[error("no JSON object found")]
    NoObject,
    #[error("invalid JSON object")]
    InvalidJson,
    #[error("JSON object must contain an ops array")]
    InvalidOps,
}

pub fn parse_ops(response: &str) -> Result<Vec<Op>, ParseError> {
    let start = response
        .match_indices('{')
        .find(|(start, _)| {
            let next = response[start + 1..].trim_start().chars().next();
            matches!(next, Some('"' | '}') | None)
        })
        .map(|(start, _)| start)
        .ok_or_else(|| {
            if response.contains('{') {
                ParseError::InvalidJson
            } else {
                ParseError::NoObject
            }
        })?;
    // The stream parser consumes only the first value, ignoring trailing prose.
    let object = serde_json::Deserializer::from_str(&response[start..])
        .into_iter::<Value>()
        .next()
        .and_then(Result::ok)
        .ok_or(ParseError::InvalidJson)?;
    let Value::Object(mut fields) = object else {
        return Err(ParseError::InvalidJson);
    };
    match fields.remove("ops") {
        Some(Value::Array(ops)) => Ok(ops.into_iter().map(Op).collect()),
        _ => Err(ParseError::InvalidOps),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroundRef {
    pub uid: String,
    pub act: Option<Act>,
}

/// Applied operations use resolved IDs, including the ID allocated for each addition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppliedOp {
    AddNode {
        node: Node,
        grounds: Vec<GroundRef>,
    },
    AddEdge {
        edge: Edge,
        grounds: Vec<GroundRef>,
    },
    RemoveEdge {
        edge: Edge,
        grounds: Vec<GroundRef>,
    },
    Merge {
        from: String,
        into: String,
        grounds: Vec<GroundRef>,
    },
    SetStatus {
        nid: String,
        status: Status,
        grounds: Vec<GroundRef>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Rejection {
    MalformedOperation,
    UnknownOperation,
    EmptyGrounds,
    GroundOutsideInput,
    UnknownAct,
    Confidence,
    UnknownNodeKind,
    EmptyText,
    DuplicateTmp,
    TmpMatchesNodeId,
    EdgeEndpoint,
    EdgeKind,
    EdgeSelfLoop,
    EdgeDuplicate,
    RemoveEdgeContext,
    RemoveEdgeKind,
    RemoveEdgeMissing,
    RemoveEdgeReviewed,
    IssueAlreadyHasParent,
    IssueParentCycle,
    MergeSameNode,
    MergeEndpoint,
    MergeKind,
    MergeReviewedNode,
    MergeReviewedEdge,
    MergeRejectedTarget,
    StatusEndpoint,
    StatusReviewedNode,
    StatusKind,
    StatusMerged,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PatchResult {
    pub graph: Graph,
    pub applied: Vec<AppliedOp>,
    pub rejected: BTreeMap<Rejection, usize>,
    pub touched: BTreeSet<String>,
}

impl PatchResult {
    /// Graph revision when at least one operation applied.
    pub fn patch_rev(&self) -> Option<u64> {
        (!self.applied.is_empty()).then_some(self.graph.rev)
    }
}

/// Validate in response order. The IDs in `allowed_uids` are those actually
/// presented to the model, including any reference utterances.
pub fn apply_patch(graph: &Graph, ops: &[Op], allowed_uids: &[&str]) -> PatchResult {
    apply_patch_with_context(graph, ops, allowed_uids, PatchContext::Other)
}

/// Identifies whether a response came from issue consolidation (stage one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PatchContext {
    Other,
    IssueConsolidation,
}

/// Validate and apply a response using the caller's consolidation context.
pub fn apply_patch_with_context(
    graph: &Graph,
    ops: &[Op],
    allowed_uids: &[&str],
    context: PatchContext,
) -> PatchResult {
    let mut patch = Patch {
        context,
        next_node: last_id(graph.nodes.keys(), 'n'),
        next_edge: last_id(graph.edges.keys(), 'e'),
        graph: graph.clone(),
        allowed: allowed_uids.iter().copied().collect(),
        tmp: BTreeMap::new(),
        seen_tmp: BTreeSet::new(),
        touched: BTreeSet::new(),
    };
    let mut applied = Vec::new();
    let mut rejected = BTreeMap::new();
    for Op(value) in ops {
        match patch.apply(value) {
            Ok(op) => applied.push(op),
            Err(reason) => *rejected.entry(reason).or_default() += 1,
        }
    }
    if !applied.is_empty() {
        patch.graph.rev += 1;
    }
    for id in &patch.touched {
        if let Some(node) = patch.graph.nodes.get_mut(id) {
            node.updated_rev = patch.graph.rev;
        }
    }
    PatchResult {
        graph: patch.graph,
        applied,
        rejected,
        touched: patch.touched,
    }
}

fn last_id<'a>(ids: impl Iterator<Item = &'a String>, prefix: char) -> u64 {
    ids.filter_map(|id| id.strip_prefix(prefix)?.parse().ok())
        .max()
        .unwrap_or(0)
}

struct Patch<'a> {
    context: PatchContext,
    graph: Graph,
    allowed: BTreeSet<&'a str>,
    tmp: BTreeMap<String, String>,
    seen_tmp: BTreeSet<String>,
    touched: BTreeSet<String>,
    next_node: u64,
    next_edge: u64,
}

impl Patch<'_> {
    fn apply(&mut self, value: &Value) -> Result<AppliedOp, Rejection> {
        let field = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .ok_or(Rejection::MalformedOperation)
        };
        let op = field("op")?;
        let tmp = (op == "add_node")
            .then(|| value.get("tmp").and_then(Value::as_str))
            .flatten();
        let duplicate = tmp.is_some_and(|id| !self.seen_tmp.insert(id.to_owned()));
        if !matches!(
            op,
            "add_node" | "add_edge" | "remove_edge" | "merge" | "set_status"
        ) {
            return Err(Rejection::UnknownOperation);
        }
        let grounds = self.grounds(value)?;
        let confidence = match value.get("confidence") {
            Some(raw) => raw
                .as_f64()
                .filter(|c| (0.0..=1.0).contains(c))
                .ok_or(Rejection::Confidence)?,
            None if matches!(op, "add_node" | "add_edge") => {
                return Err(Rejection::MalformedOperation)
            }
            None => 1.0,
        };
        match op {
            "add_node" => {
                let kind = field("kind")?
                    .parse()
                    .map_err(|_| Rejection::UnknownNodeKind)?;
                let text = field("text")?.trim();
                if text.is_empty() {
                    return Err(Rejection::EmptyText);
                }
                if duplicate {
                    return Err(Rejection::DuplicateTmp);
                }
                let tmp = field("tmp")?;
                if self.graph.nodes.contains_key(tmp) {
                    return Err(Rejection::TmpMatchesNodeId);
                }
                self.next_node += 1;
                let nid = format!("n{}", self.next_node);
                let node = Node {
                    nid: nid.clone(),
                    kind,
                    text: text.to_owned(),
                    status: Status::Open,
                    merged_into: None,
                    confidence,
                    review: Review::Auto,
                    created_rev: self.graph.rev + 1,
                    updated_rev: self.graph.rev + 1,
                };
                self.tmp.insert(tmp.to_owned(), nid.clone());
                self.touched.insert(nid.clone());
                self.add_grounds(&nid, &grounds);
                self.graph.nodes.insert(nid, node.clone());
                Ok(AppliedOp::AddNode { node, grounds })
            }
            "add_edge" => self.add_edge(
                field("src")?,
                field("dst")?,
                field("kind")?,
                confidence,
                grounds,
            ),
            "remove_edge" => {
                self.remove_edge(field("src")?, field("dst")?, field("kind")?, grounds)
            }
            "merge" => self.merge(field("from")?, field("into")?, grounds),
            "set_status" => self.set_status(field("nid")?, field("status")?, grounds),
            _ => Err(Rejection::UnknownOperation),
        }
    }

    fn grounds(&self, value: &Value) -> Result<Vec<GroundRef>, Rejection> {
        let grounds = value
            .get("grounds")
            .and_then(Value::as_array)
            .filter(|values| !values.is_empty())
            .ok_or(Rejection::EmptyGrounds)?;
        grounds
            .iter()
            .map(|ground| {
                let uid = ground
                    .get("uid")
                    .and_then(Value::as_str)
                    .filter(|uid| self.allowed.contains(uid))
                    .ok_or(Rejection::GroundOutsideInput)?;
                let act = match ground.get("act") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(act)) => {
                        Some(act.parse().map_err(|_| Rejection::UnknownAct)?)
                    }
                    _ => return Err(Rejection::UnknownAct),
                };
                Ok(GroundRef {
                    uid: uid.to_owned(),
                    act,
                })
            })
            .collect()
    }

    fn node(&self, id: &str) -> Option<&Node> {
        if let Some(nid) = self.tmp.get(id) {
            return self.graph.nodes.get(nid);
        }
        if self.seen_tmp.contains(id) {
            return None;
        }
        self.graph.nodes.get(id)
    }

    fn add_grounds(&mut self, target: &str, grounds: &[GroundRef]) {
        for ground in grounds {
            if !self
                .graph
                .grounds
                .iter()
                .any(|g| g.target == target && g.uid == ground.uid)
            {
                self.graph.grounds.push(Ground {
                    target: target.to_owned(),
                    uid: ground.uid.clone(),
                    act: ground.act,
                });
            }
        }
    }

    fn add_edge(
        &mut self,
        src: &str,
        dst: &str,
        kind: &str,
        confidence: f64,
        grounds: Vec<GroundRef>,
    ) -> Result<AppliedOp, Rejection> {
        let src = self
            .node(src)
            .filter(|node| node.is_live())
            .ok_or(Rejection::EdgeEndpoint)?;
        let dst = self
            .node(dst)
            .filter(|node| node.is_live())
            .ok_or(Rejection::EdgeEndpoint)?;
        if src.nid == dst.nid {
            return Err(Rejection::EdgeSelfLoop);
        }
        let kind = kind.parse().map_err(|_| Rejection::EdgeKind)?;
        if !valid_edge(kind, src.kind, dst.kind) {
            return Err(Rejection::EdgeKind);
        }
        if self
            .graph
            .edges
            .values()
            .any(|e| e.key() == (&src.nid, &dst.nid, kind))
        {
            return Err(Rejection::EdgeDuplicate);
        }
        if kind == EdgeKind::SubIssueOf {
            validate_issue_parent(&self.graph, &src.nid, &dst.nid)?;
        }
        let src = src.nid.clone();
        let dst = dst.nid.clone();
        self.next_edge += 1;
        let edge = Edge {
            eid: format!("e{}", self.next_edge),
            src,
            dst,
            kind,
            confidence,
            review: Review::Auto,
            created_rev: self.graph.rev + 1,
        };
        self.touched.extend([edge.src.clone(), edge.dst.clone()]);
        self.add_grounds(&edge.eid, &grounds);
        self.graph.edges.insert(edge.eid.clone(), edge.clone());
        Ok(AppliedOp::AddEdge { edge, grounds })
    }

    fn remove_edge(
        &mut self,
        src: &str,
        dst: &str,
        kind: &str,
        grounds: Vec<GroundRef>,
    ) -> Result<AppliedOp, Rejection> {
        if self.context != PatchContext::IssueConsolidation {
            return Err(Rejection::RemoveEdgeContext);
        }
        if kind.parse() != Ok(EdgeKind::SubIssueOf) {
            return Err(Rejection::RemoveEdgeKind);
        }
        let src = self.node(src).ok_or(Rejection::RemoveEdgeMissing)?;
        let dst = self.node(dst).ok_or(Rejection::RemoveEdgeMissing)?;
        let edge = self
            .graph
            .live_edges()
            .find(|edge| edge.key() == (src.nid.as_str(), dst.nid.as_str(), EdgeKind::SubIssueOf))
            .ok_or(Rejection::RemoveEdgeMissing)?;
        if edge.review != Review::Auto {
            return Err(Rejection::RemoveEdgeReviewed);
        }
        let edge = edge.clone();
        self.graph.edges.remove(&edge.eid);
        self.graph
            .grounds
            .retain(|ground| ground.target != edge.eid);
        self.touched.extend([edge.src.clone(), edge.dst.clone()]);
        Ok(AppliedOp::RemoveEdge { edge, grounds })
    }

    fn merge(
        &mut self,
        from: &str,
        into: &str,
        grounds: Vec<GroundRef>,
    ) -> Result<AppliedOp, Rejection> {
        let lookup = |id| {
            self.node(id)
                .filter(|node| node.status != Status::Merged)
                .ok_or(Rejection::MergeEndpoint)
        };
        let source = lookup(from)?;
        let target = lookup(into)?;
        if source.nid == target.nid {
            return Err(Rejection::MergeSameNode);
        }
        if source.kind != target.kind {
            return Err(Rejection::MergeKind);
        }
        if source.review != Review::Auto {
            return Err(Rejection::MergeReviewedNode);
        }
        if target.review == Review::Rejected {
            return Err(Rejection::MergeRejectedTarget);
        }
        let from = source.nid.clone();
        let into = target.nid.clone();
        if self
            .graph
            .edges
            .values()
            .any(|e| e.review != Review::Auto && (e.src == from || e.dst == from))
        {
            return Err(Rejection::MergeReviewedEdge);
        }
        if let Some(node) = self.graph.nodes.get_mut(&from) {
            node.status = Status::Merged;
            node.merged_into = Some(into.clone());
        }
        for node in self.graph.nodes.values_mut() {
            if node.merged_into.as_deref() == Some(&from) {
                node.merged_into = Some(into.clone());
                self.touched.insert(node.nid.clone());
            }
        }
        let mut former_grounds = Vec::new();
        self.graph.grounds.retain(|ground| {
            if ground.target == from {
                former_grounds.push(GroundRef {
                    uid: ground.uid.clone(),
                    act: ground.act,
                });
                false
            } else {
                true
            }
        });
        self.add_grounds(&into, &former_grounds);
        self.add_grounds(&into, &grounds);
        self.redirect_edges(&from, &into);
        self.touched.extend([from.clone(), into.clone()]);
        Ok(AppliedOp::Merge {
            from,
            into,
            grounds,
        })
    }

    fn redirect_edges(&mut self, from: &str, into: &str) {
        let mut keys = BTreeMap::new();
        let mut moving = Vec::new();
        self.graph.edges.retain(|_, edge| {
            if edge.src == from || edge.dst == from {
                moving.push(edge.clone());
                false
            } else {
                keys.insert(
                    (edge.src.clone(), edge.dst.clone(), edge.kind),
                    edge.eid.clone(),
                );
                true
            }
        });
        let mut removed = BTreeSet::new();
        for mut edge in moving {
            if edge.src == from {
                edge.src = into.to_owned();
            }
            if edge.dst == from {
                edge.dst = into.to_owned();
            }
            self.touched.extend([edge.src.clone(), edge.dst.clone()]);
            if edge.src == edge.dst {
                removed.insert(edge.eid);
                continue;
            }
            let key = (edge.src.clone(), edge.dst.clone(), edge.kind);
            if let Some(existing) = keys.get(&key).and_then(|eid| self.graph.edges.get(eid)) {
                let existing_id = existing.eid.clone();
                if existing.review != Review::Auto || edge_age(existing) <= edge_age(&edge) {
                    removed.insert(edge.eid);
                    continue;
                }
                self.graph.edges.remove(&existing_id);
                removed.insert(existing_id);
            } else if edge.kind == EdgeKind::SubIssueOf
                && validate_issue_parent(&self.graph, &edge.src, &edge.dst).is_err()
            {
                removed.insert(edge.eid);
                continue;
            }
            keys.insert(key, edge.eid.clone());
            self.graph.edges.insert(edge.eid.clone(), edge);
        }
        self.graph
            .grounds
            .retain(|ground| !removed.contains(&ground.target));
    }

    fn set_status(
        &mut self,
        nid: &str,
        status: &str,
        grounds: Vec<GroundRef>,
    ) -> Result<AppliedOp, Rejection> {
        let node = self
            .node(nid)
            .filter(|n| n.is_live())
            .ok_or(Rejection::StatusEndpoint)?;
        if node.review != Review::Auto {
            return Err(Rejection::StatusReviewedNode);
        }
        let status = status.parse().map_err(|_| Rejection::StatusKind)?;
        match status {
            Status::Merged => return Err(Rejection::StatusMerged),
            Status::Decided | Status::Resolved if node.kind != NodeKind::Issue => {
                return Err(Rejection::StatusKind)
            }
            Status::Retracted if !matches!(node.kind, NodeKind::Position | NodeKind::Argument) => {
                return Err(Rejection::StatusKind)
            }
            _ => {}
        }
        let nid = node.nid.clone();
        if let Some(node) = self.graph.nodes.get_mut(&nid) {
            node.status = status;
        }
        self.touched.insert(nid.clone());
        self.add_grounds(&nid, &grounds);
        Ok(AppliedOp::SetStatus {
            nid,
            status,
            grounds,
        })
    }
}

fn edge_age(edge: &Edge) -> (u64, u64, &str) {
    let numeric_id = edge.eid.strip_prefix('e').and_then(|id| id.parse().ok());
    (edge.created_rev, numeric_id.unwrap_or(u64::MAX), &edge.eid)
}

fn valid_edge(kind: EdgeKind, src: NodeKind, dst: NodeKind) -> bool {
    use NodeKind::*;
    match kind {
        EdgeKind::SubIssueOf => (src, dst) == (Issue, Issue),
        EdgeKind::RespondsTo => (src, dst) == (Position, Issue),
        EdgeKind::Supports | EdgeKind::ObjectsTo => {
            src == Argument && matches!(dst, Position | Argument)
        }
        EdgeKind::Resolves => (src, dst) == (Decision, Issue),
        EdgeKind::Assigns => src == Action && matches!(dst, Decision | Issue),
    }
}

fn validate_issue_parent(graph: &Graph, src: &str, dst: &str) -> Result<(), Rejection> {
    let parents = graph.issue_parents();
    if parents.contains_key(src) {
        return Err(Rejection::IssueAlreadyHasParent);
    }
    let mut ancestor = dst;
    let mut visited = BTreeSet::new();
    loop {
        if ancestor == src || !visited.insert(ancestor) {
            return Err(Rejection::IssueParentCycle);
        }
        match parents.get(ancestor) {
            Some(&parent) => ancestor = parent,
            None => return Ok(()),
        }
    }
}
