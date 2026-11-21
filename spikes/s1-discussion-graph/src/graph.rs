use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub const RULES: [&str; 5] = ["common", "add_node", "add_edge", "merge", "set_status"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub grounds: BTreeSet<u64>,
    #[serde(default = "open")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_into: Option<String>,
    #[serde(default)]
    pub review: Review,
    #[serde(default)]
    pub confidence: f64,
}

fn open() -> String {
    "open".into()
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Review {
    #[default]
    Auto,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub src: String,
    pub dst: String,
    pub kind: String,
    #[serde(default)]
    pub grounds: BTreeSet<u64>,
    #[serde(default)]
    pub review: Review,
    #[serde(default)]
    pub confidence: f64,
}

impl Edge {
    pub fn key(&self) -> (&str, &str, &str) {
        (&self.src, &self.dst, &self.kind)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl Node {
    pub fn active(&self) -> bool {
        self.status != "merged" && self.review != Review::Rejected
    }
}

#[derive(Default, Serialize)]
pub struct Patch {
    pub dropped: BTreeMap<&'static str, usize>,
    pub applied: Vec<Value>,
    #[serde(skip)]
    pub touched: BTreeSet<usize>,
}

#[derive(Deserialize)]
struct Ground {
    uid: String,
    act: Option<String>,
}

#[derive(Deserialize)]
struct Common {
    op: String,
    grounds: Vec<Ground>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Operation {
    AddNode {
        tmp: String,
        kind: String,
        text: String,
        confidence: f64,
    },
    AddEdge {
        src: String,
        dst: String,
        kind: String,
        confidence: f64,
    },
    Merge {
        from: String,
        into: String,
    },
    SetStatus {
        nid: String,
        status: String,
    },
}

fn field<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name)?.as_str()
}

impl Graph {
    pub fn apply(&mut self, ops: Vec<Value>, rows: &HashMap<String, u64>) -> Patch {
        let mut patch = Patch {
            dropped: RULES.into_iter().map(|rule| (rule, 0)).collect(),
            ..Patch::default()
        };
        let mut seen = HashSet::new();
        let mut temps = HashMap::new();
        for raw in ops {
            let tmp = (field(&raw, "op") == Some("add_node"))
                .then(|| field(&raw, "tmp"))
                .flatten();
            let duplicate = tmp.is_some_and(|name| !seen.insert(name.to_owned()));
            let result = self.apply_one(&raw, rows, &mut temps, duplicate);
            match result {
                Ok(touched) => {
                    patch.touched.extend(touched);
                    patch.applied.push(raw);
                }
                Err(rule) => {
                    if let Some(tmp) = tmp {
                        temps.insert(tmp.to_owned(), None);
                    }
                    *patch.dropped.entry(rule).or_default() += 1;
                }
            }
        }
        patch
    }

    fn apply_one(
        &mut self,
        raw: &Value,
        rows: &HashMap<String, u64>,
        temps: &mut HashMap<String, Option<usize>>,
        duplicate: bool,
    ) -> Result<Vec<usize>, &'static str> {
        let common: Common = serde_json::from_value(raw.clone()).map_err(|_| "common")?;
        let op = RULES[1..]
            .iter()
            .copied()
            .find(|rule| *rule == common.op)
            .ok_or("common")?;
        let grounds: BTreeSet<u64> = common
            .grounds
            .iter()
            .map(|ground| match ground.act.as_deref() {
                None
                | Some("asserts" | "asks" | "challenges" | "agrees" | "retracts" | "rephrases") => {
                    rows.get(&ground.uid).copied().ok_or("common")
                }
                _ => Err("common"),
            })
            .collect::<Result<_, _>>()?;
        if grounds.is_empty()
            || raw.get("confidence").is_some_and(|confidence| {
                !confidence
                    .as_f64()
                    .is_some_and(|n| (0.0..=1.0).contains(&n))
            })
        {
            return Err("common");
        }
        let find = |id: &str| {
            match temps.get(id) {
                Some(index) => *index,
                None => self.nodes.iter().position(|node| node.id == id),
            }
            .filter(|index| self.nodes[*index].active())
            .ok_or(op)
        };
        let operation = serde_json::from_value(raw.clone()).map_err(|_| op)?;
        match operation {
            Operation::AddNode {
                tmp,
                kind,
                text,
                confidence,
            } => {
                if duplicate
                    || tmp.trim().is_empty()
                    || text.trim().is_empty()
                    || !matches!(
                        kind.as_str(),
                        "issue" | "position" | "argument" | "decision" | "action"
                    )
                {
                    return Err(op);
                }
                let index = self.nodes.len();
                self.nodes.push(Node {
                    id: format!("n{}", index + 1),
                    kind,
                    text,
                    grounds,
                    confidence,
                    status: open(),
                    merged_into: None,
                    review: Review::Auto,
                });
                temps.insert(tmp, Some(index));
                Ok(vec![index])
            }
            Operation::AddEdge {
                src,
                dst,
                kind,
                confidence,
            } => {
                let (src, dst) = (find(&src)?, find(&dst)?);
                let (a, b) = (&self.nodes[src], &self.nodes[dst]);
                if src == dst
                    || !matches!(
                        (kind.as_str(), a.kind.as_str(), b.kind.as_str()),
                        ("responds_to", "position", "issue")
                            | (
                                "supports" | "objects_to",
                                "argument",
                                "position" | "argument"
                            )
                            | ("resolves", "decision", "issue")
                            | ("assigns", "action", "decision" | "issue")
                    )
                    || self
                        .edges
                        .iter()
                        .any(|edge| edge.key() == (&a.id, &b.id, &kind))
                {
                    return Err(op);
                }
                self.edges.push(Edge {
                    src: a.id.clone(),
                    dst: b.id.clone(),
                    kind,
                    grounds,
                    review: Review::Auto,
                    confidence,
                });
                Ok(vec![src, dst])
            }
            Operation::Merge { from, into } => {
                let (from, into) = (find(&from)?, find(&into)?);
                let a = &self.nodes[from];
                if from == into
                    || a.kind != self.nodes[into].kind
                    || a.review != Review::Auto
                    || self.edges.iter().any(|edge| {
                        edge.review != Review::Auto && (edge.src == a.id || edge.dst == a.id)
                    })
                {
                    return Err(op);
                }
                Ok(self.merge(from, into, grounds))
            }
            Operation::SetStatus { nid, status } => {
                let index = find(&nid)?;
                let node = &mut self.nodes[index];
                if node.review != Review::Auto
                    || !matches!(
                        (status.as_str(), node.kind.as_str()),
                        ("open", _)
                            | ("decided" | "resolved", "issue")
                            | ("retracted", "position" | "argument")
                    )
                {
                    return Err(op);
                }
                node.status = status;
                node.grounds.extend(grounds);
                Ok(vec![index])
            }
        }
    }

    fn merge(&mut self, from: usize, into: usize, grounds: BTreeSet<u64>) -> Vec<usize> {
        let source = self.nodes[from].id.clone();
        let target = self.nodes[into].id.clone();
        let old_grounds = self.nodes[from].grounds.clone();
        self.nodes[into].grounds.extend(old_grounds);
        self.nodes[into].grounds.extend(grounds);
        self.nodes[from].status = "merged".into();
        self.nodes[from].merged_into = Some(target.clone());
        let mut touched = vec![from, into];
        for (index, node) in self.nodes.iter_mut().enumerate() {
            if node.merged_into.as_deref() == Some(&source) {
                node.merged_into = Some(target.clone());
                touched.push(index);
            }
        }
        let (moving, retained): (Vec<_>, Vec<_>) = std::mem::take(&mut self.edges)
            .into_iter()
            .partition(|edge| edge.src == source || edge.dst == source);
        self.edges = retained;
        for mut edge in moving {
            if edge.src == source {
                edge.src.clone_from(&target);
            }
            if edge.dst == source {
                edge.dst.clone_from(&target);
            }
            if edge.src != edge.dst
                && !self
                    .edges
                    .iter()
                    .any(|existing| existing.key() == edge.key())
            {
                self.edges.push(edge);
            }
        }
        touched
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn add(tmp: &str, kind: &str) -> Value {
        json!({"op":"add_node","tmp":tmp,"kind":kind,"text":"合成の命題","confidence":0.8,"grounds":[{"uid":"u1"}]})
    }

    fn edge(src: &str, dst: &str, kind: &str) -> Value {
        json!({"op":"add_edge","src":src,"dst":dst,"kind":kind,"confidence":1,"grounds":[{"uid":"u1"}]})
    }

    fn merge(from: &str, into: &str) -> Value {
        json!({"op":"merge","from":from,"into":into,"grounds":[{"uid":"u2","act":"rephrases"}]})
    }

    fn status(nid: &str, status: &str) -> Value {
        json!({"op":"set_status","nid":nid,"status":status,"grounds":[{"uid":"u1"}]})
    }

    fn apply(graph: &mut Graph, ops: Vec<Value>) -> Patch {
        graph.apply(ops, &HashMap::from([("u1".into(), 1), ("u2".into(), 2)]))
    }

    #[test]
    fn common_rejects_unsupported_evidence_confidence_and_op() {
        for (field, invalid) in [
            ("grounds", json!([])),
            ("grounds", json!([{"uid":"u9"}])),
            ("grounds", json!([{"uid":"u1","act":"invented"}])),
            ("confidence", json!(1.1)),
            ("op", json!("unknown")),
        ] {
            let mut op = add("a", "issue");
            op[field] = invalid;
            let patch = apply(&mut Graph::default(), vec![op]);
            assert_eq!(patch.dropped["common"], 1, "{field}");
        }
    }

    #[test]
    fn rejects_invalid_nodes_and_references_to_failed_temporaries() {
        let mut graph = Graph::default();
        let mut empty = add("c", "issue");
        empty["text"] = json!(" ");
        let patch = apply(
            &mut graph,
            vec![
                add("a", "position"),
                add("b", "invented"),
                empty,
                add("a", "position"),
                add("d", "issue"),
                edge("a", "d", "responds_to"),
                edge("b", "d", "responds_to"),
            ],
        );
        assert_eq!(patch.dropped["add_node"], 3);
        assert_eq!(patch.dropped["add_edge"], 2);
        assert_eq!(graph.nodes.len(), 2);
    }

    #[test]
    fn rejects_invalid_edges_and_duplicates() {
        let mut graph = Graph::default();
        apply(
            &mut graph,
            vec![
                add("a", "argument"),
                add("b", "position"),
                add("c", "issue"),
                add("d", "position"),
            ],
        );
        graph.nodes[3].review = Review::Rejected;
        let valid = edge("n1", "n2", "supports");
        let patch = apply(
            &mut graph,
            vec![
                valid.clone(),
                valid,
                edge("n1", "n1", "supports"),
                edge("n1", "n3", "supports"),
                edge("n1", "missing", "supports"),
                edge("n1", "n4", "supports"),
            ],
        );
        assert_eq!(patch.dropped["add_edge"], 5);
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn merge_rewires_edges_evidence_and_chains() {
        let mut graph = Graph::default();
        apply(
            &mut graph,
            vec![
                add("a", "argument"),
                add("b", "argument"),
                add("c", "argument"),
                add("d", "position"),
                add("e", "argument"),
                edge("a", "b", "supports"),
                edge("a", "d", "supports"),
                edge("c", "d", "supports"),
                edge("e", "a", "objects_to"),
            ],
        );
        let patch = apply(
            &mut graph,
            vec![merge("n1", "n2"), merge("n1", "n3"), merge("n2", "n3")],
        );
        assert_eq!(patch.dropped["merge"], 1);
        assert_eq!(graph.nodes[0].merged_into.as_deref(), Some("n3"));
        assert_eq!(graph.nodes[1].merged_into.as_deref(), Some("n3"));
        assert_eq!(graph.nodes[2].grounds, BTreeSet::from([1, 2]));
        assert_eq!(
            graph.edges.iter().map(Edge::key).collect::<Vec<_>>(),
            [("n3", "n4", "supports"), ("n5", "n3", "objects_to"),]
        );
    }

    #[test]
    fn merge_protects_reviewed_nodes_and_edges() {
        let mut graph = Graph::default();
        apply(
            &mut graph,
            vec![
                add("a", "argument"),
                add("b", "argument"),
                add("c", "argument"),
                add("d", "position"),
                add("e", "argument"),
                add("f", "argument"),
                edge("a", "d", "supports"),
            ],
        );
        graph.edges[0].review = Review::Approved;
        graph.nodes[1].review = Review::Approved;
        graph.nodes[2].review = Review::Rejected;
        let patch = apply(
            &mut graph,
            vec![
                merge("n1", "n5"),
                merge("n2", "n5"),
                merge("n5", "n3"),
                merge("n3", "n5"),
                merge("n5", "n4"),
                merge("n5", "n5"),
                merge("n5", "n99"),
                merge("n5", "n6"),
                edge("n5", "n4", "supports"),
            ],
        );
        assert_eq!(patch.dropped["merge"], 7);
        assert_eq!(patch.dropped["add_edge"], 1);
    }

    #[test]
    fn set_status_checks_kind_and_review() {
        let mut graph = Graph::default();
        apply(
            &mut graph,
            vec![
                add("a", "position"),
                add("b", "issue"),
                add("c", "issue"),
                add("d", "issue"),
                add("e", "issue"),
            ],
        );
        graph.nodes[2].review = Review::Approved;
        graph.nodes[3].review = Review::Rejected;
        graph.nodes[4].status = "merged".into();
        let patch = apply(
            &mut graph,
            vec![
                status("n1", "decided"),
                status("n1", "resolved"),
                status("n2", "retracted"),
                status("n2", "merged"),
                status("n3", "decided"),
                status("n4", "decided"),
                status("n5", "decided"),
                status("n6", "decided"),
                status("n2", "decided"),
                status("n1", "retracted"),
            ],
        );
        assert_eq!(patch.dropped["set_status"], 8);
        assert_eq!(graph.nodes[1].status, "decided");
        assert_eq!(graph.nodes[0].status, "retracted");
    }
}
