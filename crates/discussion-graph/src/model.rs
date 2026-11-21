use std::{collections::BTreeMap, fmt, str::FromStr};

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
    Resolves => "resolves", Assigns => "assigns",
});
stored_enum!(Act {
    Asserts => "asserts", Asks => "asks", Challenges => "challenges", Agrees => "agrees",
    Retracts => "retracts", Rephrases => "rephrases",
});
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
    pub(crate) fn live_edges(&self) -> impl Iterator<Item = &Edge> {
        self.edges.values().filter(|edge| {
            edge.review != Review::Rejected
                && self.nodes.get(&edge.src).is_some_and(Node::is_live)
                && self.nodes.get(&edge.dst).is_some_and(Node::is_live)
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gaps {
    pub unanswered_issues: Vec<String>,
    pub one_sided_positions: Vec<String>,
    pub issues_without_decision: Vec<String>,
    pub decisions_without_action: Vec<String>,
}

pub fn detect_gaps(graph: &Graph, flush_complete: bool) -> Gaps {
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
                if !has(EdgeKind::RespondsTo) {
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
