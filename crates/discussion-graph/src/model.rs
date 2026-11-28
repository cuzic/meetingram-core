use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    str::FromStr,
};

use serde::{Deserialize, Serialize};

macro_rules! stored_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[non_exhaustive]
        pub enum $name {
            $(#[serde(rename = $value)] $variant),+
        }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $value),+ }
            }
        }

        impl FromStr for $name {
            type Err = UnknownValue;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($value => Ok(Self::$variant)),+,
                    _ => Err(UnknownValue),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unknown graph enum value")]
pub struct UnknownValue;

stored_enum!(NodeKind {
    Issue => "issue", Position => "position", Argument => "argument",
    Decision => "decision", Action => "action",
});
stored_enum!(Status {
    Open => "open", Resolved => "resolved", Decided => "decided",
    Retracted => "retracted", Merged => "merged",
});
stored_enum!(Review { Auto => "auto", Approved => "approved", Rejected => "rejected" });
stored_enum!(EdgeKind {
    RespondsTo => "responds_to", Supports => "supports", ObjectsTo => "objects_to",
    Resolves => "resolves", Assigns => "assigns", SubIssueOf => "sub_issue_of",
});
stored_enum!(Act {
    Asserts => "asserts", Asks => "asks", Challenges => "challenges", Agrees => "agrees",
    Retracts => "retracts", Rephrases => "rephrases",
});
stored_enum!(AnalysisKind { Cloud => "cloud", Crt => "crt" });
stored_enum!(PatchSource {
    Live => "live", Flush => "flush", Consolidate => "consolidate", Review => "review",
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Utterance {
    pub uid: String,
    pub row_id: i64,
    pub track: Option<String>,
    pub speaker_label: String,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub text: String,
    #[serde(default)]
    pub is_retranscribed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub nid: String,
    pub kind: NodeKind,
    pub text: String,
    pub status: Status,
    pub merged_into: Option<String>,
    pub confidence: f64,
    pub review: Review,
    pub created_rev: u64,
    pub updated_rev: u64,
}

impl Node {
    pub fn is_live(&self) -> bool {
        self.status != Status::Merged && self.review != Review::Rejected
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub eid: String,
    pub src: String,
    pub dst: String,
    pub kind: EdgeKind,
    pub confidence: f64,
    pub review: Review,
    pub created_rev: u64,
}

impl Edge {
    pub(crate) fn key(&self) -> (&str, &str, EdgeKind) {
        (&self.src, &self.dst, self.kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ground {
    pub target: String,
    pub uid: String,
    pub act: Option<Act>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub rev: u64,
    /// Records keyed by their session-local IDs.
    pub utterances: BTreeMap<String, Utterance>,
    pub nodes: BTreeMap<String, Node>,
    pub edges: BTreeMap<String, Edge>,
    pub grounds: Vec<Ground>,
}

impl Graph {
    pub(crate) fn issue_parents(&self) -> BTreeMap<&str, &str> {
        self.live_edges()
            .filter(|edge| edge.kind == EdgeKind::SubIssueOf)
            .map(|edge| (edge.src.as_str(), edge.dst.as_str()))
            .collect()
    }

    pub(crate) fn live_edges(&self) -> impl Iterator<Item = &Edge> {
        self.edges.values().filter(|edge| {
            edge.review != Review::Rejected
                && self.nodes.get(&edge.src).is_some_and(Node::is_live)
                && self.nodes.get(&edge.dst).is_some_and(Node::is_live)
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Gaps {
    pub unanswered_issues: Vec<String>,
    pub one_sided_positions: Vec<String>,
    pub issues_without_decision: Vec<String>,
    pub decisions_without_action: Vec<String>,
}

pub fn detect_gaps(graph: &Graph, flush_complete: bool) -> Gaps {
    let parents = graph.issue_parents();
    let mut incoming = BTreeMap::<_, Vec<_>>::new();
    for edge in graph.live_edges() {
        incoming
            .entry(edge.dst.as_str())
            .or_default()
            .push(edge.kind);
    }
    let mut gaps = Gaps::default();
    for node in graph.nodes.values().filter(|node| node.is_live()) {
        let has = |kind| {
            incoming
                .get(node.nid.as_str())
                .is_some_and(|kinds| kinds.contains(&kind))
        };
        match node.kind {
            NodeKind::Issue => {
                if !has(EdgeKind::RespondsTo)
                    && (!parents.contains_key(node.nid.as_str())
                        || (node.status == Status::Open && !has(EdgeKind::Resolves)))
                {
                    gaps.unanswered_issues.push(node.nid.clone());
                }
                if flush_complete && !has(EdgeKind::Resolves) {
                    gaps.issues_without_decision.push(node.nid.clone());
                }
            }
            NodeKind::Position if has(EdgeKind::Supports) ^ has(EdgeKind::ObjectsTo) => {
                gaps.one_sided_positions.push(node.nid.clone());
            }
            NodeKind::Decision if !has(EdgeKind::Assigns) => {
                gaps.decisions_without_action.push(node.nid.clone());
            }
            _ => {}
        }
    }
    gaps
}

/// Count unanswered issues, including nested questions, by their root agenda issue.
pub fn unanswered_by_agenda(graph: &Graph) -> BTreeMap<String, usize> {
    let parents = graph.issue_parents();
    let mut counts = BTreeMap::new();
    for issue in detect_gaps(graph, false).unanswered_issues {
        let mut root = issue.as_str();
        let mut visited = BTreeSet::new();
        while let Some(&parent) = parents.get(root) {
            if !visited.insert(root) {
                break;
            }
            root = parent;
        }
        *counts.entry(root.to_owned()).or_default() += 1;
    }
    counts
}

/// A live node in an agenda outline. The incoming relation is absent at the root.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct OutlineNode<'a> {
    pub node: &'a Node,
    pub relation: Option<EdgeKind>,
    /// True for a repeated occurrence. References have no children.
    pub is_reference: bool,
    pub children: Vec<OutlineNode<'a>>,
}

/// Agenda trees and live nodes that cannot be reached from any agenda.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Outline<'a> {
    pub agendas: Vec<OutlineNode<'a>>,
    /// Sorted by node kind and then by node ID.
    pub unattached: Vec<&'a Node>,
}

/// Build an outline rooted at each agenda issue, following live edges toward
/// questions, positions, arguments, decisions and actions. Each node expands
/// at its first occurrence across the outline. Later occurrences are references.
pub fn build_outline(graph: &Graph) -> Outline<'_> {
    fn branch<'a>(
        graph: &'a Graph,
        incoming: &BTreeMap<&str, Vec<&'a Edge>>,
        node: &'a Node,
        relation: Option<EdgeKind>,
        path: &mut BTreeSet<&'a str>,
        attached: &mut BTreeSet<&'a str>,
    ) -> OutlineNode<'a> {
        let is_reference = !attached.insert(node.nid.as_str());
        let mut children = Vec::new();
        if !is_reference {
            path.insert(node.nid.as_str());
            for edge in incoming.get(node.nid.as_str()).into_iter().flatten() {
                if !path.contains(edge.src.as_str()) {
                    children.push(branch(
                        graph,
                        incoming,
                        &graph.nodes[&edge.src],
                        Some(edge.kind),
                        path,
                        attached,
                    ));
                }
            }
            path.remove(node.nid.as_str());
        }
        OutlineNode {
            node,
            relation,
            is_reference,
            children,
        }
    }

    let parents = graph.issue_parents();
    let mut incoming = BTreeMap::<_, Vec<_>>::new();
    for edge in graph.live_edges() {
        incoming.entry(edge.dst.as_str()).or_default().push(edge);
    }
    for edges in incoming.values_mut() {
        edges.sort_by_key(|edge| (edge.kind != EdgeKind::SubIssueOf, &edge.src, edge.kind));
    }
    let mut attached = BTreeSet::new();
    let agendas = graph
        .nodes
        .values()
        .filter(|node| {
            node.is_live()
                && node.kind == NodeKind::Issue
                && !parents.contains_key(node.nid.as_str())
        })
        .map(|node| {
            branch(
                graph,
                &incoming,
                node,
                None,
                &mut BTreeSet::new(),
                &mut attached,
            )
        })
        .collect();
    let mut unattached: Vec<_> = graph
        .nodes
        .values()
        .filter(|node| node.is_live() && !attached.contains(node.nid.as_str()))
        .collect();
    unattached.sort_by_key(|node| (node.kind, node.nid.as_str()));
    Outline {
        agendas,
        unattached,
    }
}

pub fn graph_updated_subject(sid: impl fmt::Display) -> String {
    format!("graph.{sid}.updated")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphUpdated {
    pub rev: u64,
    pub source: PatchSource,
    pub added: usize,
    pub merged: usize,
    pub rejected: usize,
}

#[cfg(test)]
mod outline_tests {
    use super::*;

    #[test]
    fn outline_expands_shared_children_once() {
        for depth in [2, 20] {
            let mut graph = Graph::default();
            let mut add_node = |id: &str, kind| {
                graph.nodes.insert(
                    id.to_owned(),
                    Node {
                        nid: id.to_owned(),
                        kind,
                        text: id.to_owned(),
                        status: Status::Open,
                        merged_into: None,
                        confidence: 1.0,
                        review: Review::Auto,
                        created_rev: 1,
                        updated_rev: 1,
                    },
                );
            };
            let mut add_edge = |src: &str, dst: &str, kind| {
                let eid = format!("{src}:{dst}");
                graph.edges.insert(
                    eid.clone(),
                    Edge {
                        eid,
                        src: src.to_owned(),
                        dst: dst.to_owned(),
                        kind,
                        confidence: 1.0,
                        review: Review::Auto,
                        created_rev: 1,
                    },
                );
            };
            add_node("agenda-a", NodeKind::Issue);
            add_node("agenda-b", NodeKind::Issue);
            add_node("position", NodeKind::Position);
            add_edge("position", "agenda-a", EdgeKind::RespondsTo);
            add_edge("position", "agenda-b", EdgeKind::RespondsTo);
            let mut parent = "position".to_owned();
            for level in 0..depth {
                let left = format!("{level}-left");
                let right = format!("{level}-right");
                let shared = format!("{level}-shared");
                for id in [&left, &right, &shared] {
                    add_node(id, NodeKind::Argument);
                }
                add_edge(&left, &parent, EdgeKind::Supports);
                add_edge(&right, &parent, EdgeKind::ObjectsTo);
                add_edge(&shared, &left, EdgeKind::Supports);
                add_edge(&shared, &right, EdgeKind::ObjectsTo);
                parent = shared;
            }

            let outline = build_outline(&graph);
            assert!(outline.unattached.is_empty());
            let mut pending: Vec<_> = outline.agendas.iter().collect();
            let mut count = 0;
            while let Some(node) = pending.pop() {
                count += 1;
                pending.extend(&node.children);
            }
            assert_eq!(count, graph.nodes.len() + depth + 1);

            let position_reference = &outline.agendas[1].children[0];
            assert_eq!(position_reference.node.nid, "position");
            assert!(position_reference.is_reference);
            assert!(position_reference.children.is_empty());

            let mut branch = &outline.agendas[0].children[0];
            assert!(!branch.is_reference);
            for level in 0..depth {
                let [left, right] = branch.children.as_slice() else {
                    panic!("expected two argument branches at level {level}");
                };
                let expanded = &left.children[0];
                let reference = &right.children[0];
                assert_eq!(expanded.node.nid, format!("{level}-shared"));
                assert_eq!(reference.node.nid, expanded.node.nid);
                assert_eq!(reference.relation, Some(EdgeKind::ObjectsTo));
                assert!(!expanded.is_reference);
                assert!(reference.is_reference);
                assert!(reference.children.is_empty());
                branch = expanded;
            }
        }
    }
}
