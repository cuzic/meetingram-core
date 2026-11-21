use crate::graph::{Graph, RULES};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Write as _,
    fs,
    path::PathBuf,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct Score {
    pub recall: f64,
    pub precision: f64,
    pub edge_f1: f64,
    pub duplicates: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

fn fraction(n: usize, d: usize) -> f64 {
    if d == 0 { 0.0 } else { n as f64 / d as f64 }
}

pub fn evaluate(predicted: &Graph, gold: &Graph) -> Score {
    let active: HashSet<_> = predicted
        .nodes
        .iter()
        .filter(|node| node.active())
        .map(|node| node.id.as_str())
        .collect();
    let mut candidates = Vec::new();
    for pred in predicted.nodes.iter().filter(|node| node.active()) {
        for answer in &gold.nodes {
            if pred.kind == answer.kind {
                let overlap = pred.grounds.intersection(&answer.grounds).count();
                if overlap > 0 {
                    candidates.push((
                        std::cmp::Reverse(overlap),
                        pred.id.as_str(),
                        answer.id.as_str(),
                    ));
                }
            }
        }
    }
    candidates.sort_unstable();
    let mut matched = HashMap::new();
    let mut used = HashSet::new();
    for &(_, pred, answer) in &candidates {
        if !matched.contains_key(pred) && used.insert(answer) {
            matched.insert(pred, answer);
        }
    }
    let duplicates = candidates
        .into_iter()
        .map(|(_, pred, _)| pred)
        .filter(|pred| !matched.contains_key(pred))
        .collect::<HashSet<_>>()
        .len();
    let gold_edges: HashSet<_> = gold.edges.iter().map(|edge| edge.key()).collect();
    let predicted_edges: HashSet<_> = predicted
        .edges
        .iter()
        .filter(|edge| {
            edge.review != crate::graph::Review::Rejected
                && active.contains(edge.src.as_str())
                && active.contains(edge.dst.as_str())
        })
        .map(|edge| edge.key())
        .collect();
    let correct = predicted_edges
        .iter()
        .filter(|&&(src, dst, kind)| {
            matched
                .get(src)
                .zip(matched.get(dst))
                .is_some_and(|(&src, &dst)| gold_edges.contains(&(src, dst, kind)))
        })
        .count();
    Score {
        recall: fraction(matched.len(), gold.nodes.len()),
        precision: fraction(matched.len(), active.len()),
        edge_f1: fraction(2 * correct, gold_edges.len() + predicted_edges.len()),
        duplicates,
        duration_ms: None,
    }
}

#[derive(Serialize, Deserialize)]
pub struct Attempt {
    pub window: usize,
    pub retry: bool,
    pub input_chars: usize,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub elapsed_ms: u64,
    pub called: bool,
    pub parsed: bool,
    pub error: Option<String>,
    pub raw: Option<String>,
    pub dropped: BTreeMap<String, usize>,
    pub applied: Vec<serde_json::Value>,
}

pub fn aggregate(out: PathBuf, dirs: &[PathBuf]) -> Result<()> {
    let mut report = String::from(
        "# S1 計測結果\n\n各行は 1 会議の実行。トークンの平均と合計は取得できた呼び出し分。取得できた件数を併記する。時間換算は会議の開始から最終発話の終了までの長さを使う。拒否率の分母は解析できた応答の全操作数。上限超過で呼ばなかった入力は呼び出し数・遅延・解析率から除く。\n\n",
    );
    write!(
        report,
        "| 実行 | 呼び出し | 上限超過 | 入力トークン/回 (件数) | 出力トークン/回 (件数) | p50 ms | p95 ms | 解析率 |"
    )?;
    for rule in RULES {
        write!(report, " {rule} 拒否率 |")?;
    }
    writeln!(
        report,
        " 再現率 | 適合率 | エッジ F1 | 重複 | 入力合計 | 出力合計 | 会議の長さ (分) | 呼び出し/時 | 入力トークン/時 | 出力トークン/時 |"
    )?;
    writeln!(report, "|{}", " --- |".repeat(18 + RULES.len()))?;
    for dir in dirs {
        let score: Score = serde_json::from_slice(&fs::read(dir.join("score.json"))?)?;
        let attempts: Vec<Attempt> = fs::read_to_string(dir.join("calls.jsonl"))?
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
        let calls: Vec<_> = attempts.iter().filter(|attempt| attempt.called).collect();
        let mut times: Vec<_> = calls.iter().map(|attempt| attempt.elapsed_ms).collect();
        times.sort_unstable();
        let percentile = |p: usize| {
            times
                .get((times.len() * p).div_ceil(100).saturating_sub(1))
                .map_or("—".into(), u64::to_string)
        };
        let rate = |n, d| {
            if d == 0 {
                "—".into()
            } else {
                format!("{:.1}%", 100.0 * fraction(n, d))
            }
        };
        let total_ops: usize = calls
            .iter()
            .map(|attempt| attempt.applied.len() + attempt.dropped.values().sum::<usize>())
            .sum();
        let minutes = score.duration_ms.map(|ms| ms as f64 / 60_000.0);
        let per_hour = |count: u64| {
            minutes
                .filter(|&mins| mins > 0.0)
                .map_or("—".into(), |mins| {
                    format!("{:.1}", count as f64 * 60.0 / mins)
                })
        };
        let tokens = |select: fn(&Attempt) -> Option<u64>| {
            let values: Vec<_> = calls.iter().filter_map(|attempt| select(attempt)).collect();
            let total: u64 = values.iter().sum();
            if values.is_empty() {
                ("— (0)".into(), "—".into(), "—".into())
            } else {
                (
                    format!(
                        "{:.1} ({})",
                        total as f64 / values.len() as f64,
                        values.len()
                    ),
                    total.to_string(),
                    per_hour(total),
                )
            }
        };
        let (input, total_in, hourly_in) = tokens(|attempt| attempt.input_tokens);
        let (output, total_out, hourly_out) = tokens(|attempt| attempt.output_tokens);
        let label = dir
            .file_name()
            .unwrap_or(dir.as_os_str())
            .to_string_lossy()
            .replace('|', "\\|");
        write!(
            report,
            "| {label} | {} | {} | {input} | {output} | {} | {} | {} |",
            calls.len(),
            attempts.len() - calls.len(),
            percentile(50),
            percentile(95),
            rate(
                calls.iter().filter(|attempt| attempt.parsed).count(),
                calls.len()
            )
        )?;
        for rule in RULES {
            let rejected = calls
                .iter()
                .map(|attempt| attempt.dropped.get(rule).copied().unwrap_or(0))
                .sum();
            write!(report, " {} |", rate(rejected, total_ops))?;
        }
        writeln!(
            report,
            " {:.3} | {:.3} | {:.3} | {} | {total_in} | {total_out} | {} | {} | {hourly_in} | {hourly_out} |",
            score.recall,
            score.precision,
            score.edge_f1,
            score.duplicates,
            minutes.map_or("—".into(), |mins| format!("{mins:.2}")),
            per_hour(calls.len() as u64)
        )?;
    }
    fs::write(out, report)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_normalizes_observed_usage_to_one_hour() {
        let run = tempfile::tempdir().unwrap();
        let dir = run.path();
        fs::write(
            dir.join("score.json"),
            r#"{"recall":0,"precision":0,"edge_f1":0,"duplicates":0,"duration_ms":900000}"#,
        )
        .unwrap();
        fs::write(
            dir.join("calls.jsonl"),
            r#"{"window":1,"retry":false,"input_chars":50,"input_tokens":10,"output_tokens":null,"elapsed_ms":1,"called":true,"parsed":true,"error":null,"raw":null,"dropped":{},"applied":[]}"#,
        )
        .unwrap();
        let out = dir.join("RESULTS.md");
        aggregate(out.clone(), &[dir.to_path_buf()]).unwrap();
        let table = fs::read_to_string(out).unwrap();
        assert!(
            table.contains("| 会議の長さ (分) | 呼び出し/時 | 入力トークン/時 | 出力トークン/時 |")
        );
        assert!(table.contains("| 15.00 | 4.0 | 40.0 | — |"));
    }

    #[test]
    fn matches_by_overlap_and_counts_unmatched_predictions_once() {
        let pred: Graph = serde_json::from_value(serde_json::json!({
            "nodes":[
                {"id":"p1","kind":"issue","text":"a","grounds":[1]},
                {"id":"p2","kind":"issue","text":"b","grounds":[1,2]},
                {"id":"p3","kind":"position","text":"c","grounds":[3]}
            ],
            "edges":[
                {"src":"p3","dst":"p1","kind":"responds_to"},
                {"src":"p3","dst":"p2","kind":"responds_to"}
            ]
        }))
        .unwrap();
        let gold: Graph = serde_json::from_value(serde_json::json!({
            "nodes":[
                {"id":"g1","kind":"issue","text":"a","grounds":[1,2]},
                {"id":"g2","kind":"position","text":"c","grounds":[3]}
            ],
            "edges":[{"src":"g2","dst":"g1","kind":"responds_to"}]
        }))
        .unwrap();
        let score = evaluate(&pred, &gold);
        assert_eq!(score.precision, 2.0 / 3.0);
        assert_eq!(score.recall, 1.0);
        assert_eq!(score.edge_f1, 2.0 / 3.0);
        assert_eq!(score.duplicates, 1);
    }
}
