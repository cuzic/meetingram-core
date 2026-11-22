use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    fs,
    path::Path,
};

use anyhow::Result;
use discussion_graph::{
    EdgeKind, Graph, Node, NodeKind, Review,
    eval::{Evaluation, evaluate, gold_agenda_count},
};

use crate::{
    Config, Style,
    data::{self, Gold},
    run::{RecordedIssueStage, Trial},
};

#[derive(Clone)]
struct Measurement {
    scores: Evaluation,
    pairs: usize,
    gold_agendas: usize,
    complete: bool,
    baseline_not_applicable: bool,
}

pub(crate) fn report(out: &Path) -> Result<()> {
    let config: Config = data::read_json(&out.join("config.json"))?;
    let mut results = String::from(
        "# S2 results\n\n         | Meeting | Style | Trial | Agendas | Gold agendas | Sub-questions | Recall | Precision | Edge F1 | Wrong merges | Duplicate pairs merged | ms | Timeouts | Calls | Omitted issues | Omitted group nodes | Undetermined merges | Issue prompt stage | Parented contested issues | Status |\n         | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | --- |\n",
    );
    let mut appendix = String::new();
    let mut all = BTreeMap::new();
    let mut flush_recalls = BTreeMap::new();
    for &meeting in config.meetings() {
        let (_, gold) = data::load(meeting)?;
        let gold_agendas = gold_agenda_count(&gold.graph);
        let flush = out.join(meeting).join("flush.json");
        let before: Option<Graph> = read_if_present(&flush)?;
        if let Some(before) = &before {
            flush_recalls.insert(
                meeting,
                evaluate(before, &gold.graph, before, &[]).node_recall,
            );
        }
        for &style in config.styles() {
            for repeat in 1..=config.repeats {
                let path = data::trial_path(out, meeting, style.label(), repeat);
                if let (Some(before), Some(trial)) = (&before, read_if_present::<Trial>(&path)?) {
                    let scores = evaluate(&trial.graph, &gold.graph, before, &trial.applied);
                    let pairs = merged_pairs(before, &trial.graph, &gold);
                    let baseline_not_applicable = trial.baseline_not_applicable(style);
                    for (from, into) in &scores.undetermined_merges {
                        let node_text = |id| {
                            before
                                .nodes
                                .get(id)
                                .or_else(|| trial.graph.nodes.get(id))
                                .map(|node| node.text.as_str())
                                .unwrap_or("missing node")
                        };
                        writeln!(
                            appendix,
                            "- {meeting} / {} / trial {repeat}, `{from}` → `{into}`\n    - From: {}\n    - Into: {}",
                            style.label(),
                            serde_json::to_string(node_text(from))?,
                            serde_json::to_string(node_text(into))?,
                        )?;
                    }
                    writeln!(
                        results,
                        "| {meeting} | {} | {repeat} | {} | {gold_agendas} | {} | {:.3} | {:.3} | {:.3} | {} | {}/{} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                        style.label(),
                        scores.agendas,
                        scores.sub_questions,
                        scores.node_recall,
                        scores.node_precision,
                        scores.edge_f1,
                        scores.wrong_merges,
                        pairs,
                        gold.duplicate_issue_pairs.len(),
                        trial.elapsed_ms,
                        trial.timeouts,
                        trial.calls,
                        trial.omitted_issues,
                        trial.omitted_group_nodes,
                        scores.undetermined_merges.len(),
                        trial.issue_stage.map_or("—", RecordedIssueStage::label),
                        parented_contested_issues(&trial.graph),
                        if baseline_not_applicable {
                            "baseline not applicable"
                        } else if trial.complete {
                            "complete"
                        } else {
                            "consolidation stopped"
                        },
                    )?;
                    all.entry((meeting, style.label()))
                        .or_insert_with(Vec::new)
                        .push(Measurement {
                            scores,
                            pairs,
                            gold_agendas,
                            complete: trial.complete,
                            baseline_not_applicable,
                        });
                } else {
                    let status = if before.is_some() {
                        "trial missing"
                    } else {
                        "flush incomplete"
                    };
                    writeln!(
                        results,
                        "| {meeting} | {} | {repeat} | — | {gold_agendas} | — | — | — | — | — | — | — | — | — | — | — | — | — | — | {status} |",
                        style.label()
                    )?;
                }
            }
        }
    }

    writeln!(results, "\n## Task 4b gate (ADR r13)\n")?;
    if !config.is_sonnet_gate() {
        writeln!(
            results,
            "Not evaluated. The gate requires Claude CLI Sonnet, 6/45 windows, all seven meetings, both styles and three trials."
        )?;
    } else {
        write_gate(&mut results, &all, &flush_recalls)?;
    }
    if !appendix.is_empty() {
        writeln!(
            results,
            "\n## Appendix: Undetermined merges\n\nReview each pair and add any wrong merges to its trial's count before finalizing condition (2).\n\n{appendix}"
        )?;
    }
    fs::write(out.join("RESULTS.md"), results)?;
    Ok(())
}

fn read_if_present<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    if path.try_exists()? {
        return data::read_json(path).map(Some);
    }
    Ok(None)
}

fn write_gate(
    results: &mut String,
    all: &BTreeMap<(&str, &str), Vec<Measurement>>,
    flush_recalls: &BTreeMap<&str, f64>,
) -> std::fmt::Result {
    let baseline = Style::Baseline.label();
    let two_stage = Style::TwoStage.label();
    let complete = |meeting, style| {
        all.get(&(meeting, style))
            .filter(|rows| rows.len() == 3 && rows.iter().all(|row| row.complete))
    };
    let mut agenda_ratio_sum = 0.0;
    let mut each_agenda_ok = true;
    let mut agenda_ratios = Vec::new();
    let mut recall_details = Vec::new();
    let mut wrong_merges = 0;
    let mut recall_ok = true;
    for &meeting in crate::SHORT_MEETINGS {
        let Some(two_stage) = complete(meeting, two_stage) else {
            return write_inconclusive(results);
        };
        let gold_agendas = two_stage[0].gold_agendas;
        if gold_agendas == 0 {
            return writeln!(results, "Inconclusive. {meeting} has no gold agendas.");
        }
        let ratio = median(two_stage, |row| row.scores.agendas) as f64 / gold_agendas as f64;
        agenda_ratio_sum += ratio;
        each_agenda_ok &= (0.5..=2.5).contains(&ratio);
        agenda_ratios.push((meeting, ratio));
        wrong_merges += median(two_stage, |row| row.scores.wrong_merges);

        let (reference, source) = if let Some(base) = complete(meeting, baseline) {
            (median(base, |row| row.scores.node_recall), "baseline")
        } else if all.get(&(meeting, baseline)).is_some_and(|rows| {
            rows.len() == 3 && rows.iter().all(|row| row.baseline_not_applicable)
        }) {
            let Some(&recall) = flush_recalls.get(meeting) else {
                return write_inconclusive(results);
            };
            (recall, "flush")
        } else {
            return write_inconclusive(results);
        };
        let recall = median(two_stage, |row| row.scores.node_recall);
        recall_ok &= recall >= reference;
        recall_details.push(format!(
            "{meeting} {recall:.3} vs {reference:.3} ({source})"
        ));
    }
    let Some(long) = complete("long", two_stage) else {
        return write_inconclusive(results);
    };
    let mean_ratio = agenda_ratio_sum / crate::SHORT_MEETINGS.len() as f64;
    let long_pairs = median(long, |row| row.pairs);
    let agenda_ok = mean_ratio <= 1.5;
    let merges_ok = wrong_merges <= 2;
    let long_ok = long_pairs == 3;
    let pass = agenda_ok && each_agenda_ok && merges_ok && recall_ok && long_ok;
    let status = |ok| if ok { "PASS" } else { "FAIL" };
    writeln!(results, "**{}**\n", status(pass))?;
    writeln!(
        results,
        "- Mean agenda-to-gold-agenda ratio (target ≤1.5): {} ({mean_ratio:.3}).",
        status(agenda_ok)
    )?;
    let agenda_details: Vec<_> = agenda_ratios
        .iter()
        .map(|(meeting, ratio)| format!("{meeting} {ratio:.3}"))
        .collect();
    writeln!(
        results,
        "- Agenda ratio for each meeting (target 0.5–2.5): {} ({}).",
        status(each_agenda_ok),
        agenda_details.join(", ")
    )?;
    writeln!(
        results,
        "- Sum of wrong-merge medians (target ≤2): {} ({wrong_merges}).",
        status(merges_ok)
    )?;
    writeln!(
        results,
        "- Recall across six meetings: {} ({}).",
        status(recall_ok),
        recall_details.join(", ")
    )?;
    writeln!(
        results,
        "- Long-meeting correct duplicate pairs merged (median, target 3/3): {} ({long_pairs}/3).",
        status(long_ok)
    )?;
    writeln!(
        results,
        "\n| Meeting | Median agenda ratio |\n| --- | ---: |"
    )?;
    for (meeting, ratio) in agenda_ratios {
        writeln!(results, "| {meeting} | {ratio:.3} |")?;
    }
    Ok(())
}

fn write_inconclusive(results: &mut String) -> std::fmt::Result {
    writeln!(
        results,
        "Inconclusive. At least one required trial did not complete."
    )
}

fn median<T: Copy + PartialOrd>(rows: &[Measurement], metric: impl Fn(&Measurement) -> T) -> T {
    let mut values = [metric(&rows[0]), metric(&rows[1]), metric(&rows[2])];
    values.sort_by(|a, b| a.partial_cmp(b).expect("metrics are finite"));
    values[1]
}

fn parented_contested_issues(graph: &Graph) -> usize {
    let live_edges = graph.edges.values().filter(|edge| {
        edge.review != Review::Rejected
            && graph.nodes.get(&edge.src).is_some_and(Node::is_live)
            && graph.nodes.get(&edge.dst).is_some_and(Node::is_live)
    });
    let mut parents = BTreeSet::new();
    let mut objections = BTreeSet::new();
    let mut positions = BTreeMap::<_, BTreeSet<_>>::new();
    for edge in live_edges {
        match edge.kind {
            EdgeKind::SubIssueOf => {
                parents.insert(&edge.src);
            }
            EdgeKind::ObjectsTo => {
                objections.insert(&edge.dst);
            }
            EdgeKind::RespondsTo if graph.nodes[&edge.src].kind == NodeKind::Position => {
                positions.entry(&edge.dst).or_default().insert(&edge.src);
            }
            _ => {}
        }
    }
    parents
        .into_iter()
        .filter(|issue| {
            positions.get(issue).is_some_and(|positions| {
                positions.len() >= 2
                    || positions
                        .iter()
                        .any(|position| objections.contains(position))
            })
        })
        .count()
}

/// Match each pre-consolidation issue by evidence Jaccard, breaking ties by gold ID.
fn merged_pairs(before: &Graph, after: &Graph, gold: &Gold) -> usize {
    if gold.duplicate_issue_pairs.is_empty() {
        return 0;
    }
    let issues: Vec<_> = gold
        .graph
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Issue)
        .map(|node| {
            (
                &node.id,
                node.grounds.iter().copied().collect::<BTreeSet<_>>(),
            )
        })
        .collect();
    let mut rows = BTreeMap::<_, BTreeSet<_>>::new();
    for ground in &before.grounds {
        if let Some(utterance) = before.utterances.get(&ground.uid) {
            rows.entry(&ground.target)
                .or_default()
                .insert(utterance.row_id);
        }
    }
    let mut groups = BTreeMap::<_, BTreeSet<_>>::new();
    for node in before
        .nodes
        .values()
        .filter(|node| node.is_live() && node.kind == NodeKind::Issue)
    {
        let Some(evidence) = rows.get(&node.nid) else {
            continue;
        };
        let matched = issues
            .iter()
            .filter_map(|(id, grounds)| {
                let shared = evidence.intersection(grounds).count();
                (shared > 0).then(|| {
                    let union = evidence.len() + grounds.len() - shared;
                    (shared as f64 / union as f64, *id)
                })
            })
            .max_by(|(left, left_id), (right, right_id)| {
                left.total_cmp(right).then_with(|| right_id.cmp(left_id))
            });
        if let (Some((_, id)), Some(final_node)) = (matched, after.nodes.get(&node.nid)) {
            let survivor = final_node.merged_into.as_ref().unwrap_or(&node.nid);
            if after.nodes.get(survivor).is_some_and(Node::is_live) {
                groups.entry(survivor).or_default().insert(id);
            }
        }
    }
    gold.duplicate_issue_pairs
        .iter()
        .filter(|[early, late]| {
            groups
                .values()
                .any(|ids| ids.contains(early) && ids.contains(late))
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use discussion_graph::{Op, apply_patch};
    use serde_json::json;

    #[test]
    fn task_4b_checks_each_condition_using_three_trial_medians() {
        let style = Style::TwoStage.label();
        let baseline = Style::Baseline.label();
        let (_, gold) = data::load("m1").unwrap();
        let empty = Graph::default();
        let mut scores = evaluate(&empty, &gold.graph, &empty, &[]);
        scores.agendas = 3;
        scores.node_recall = 1.0;
        let row = Measurement {
            scores,
            pairs: 3,
            gold_agendas: 2,
            complete: true,
            baseline_not_applicable: false,
        };
        let mut all = BTreeMap::new();
        for &meeting in crate::SHORT_MEETINGS {
            all.insert((meeting, baseline), vec![row.clone(); 3]);
            all.insert((meeting, style), vec![row.clone(); 3]);
        }
        all.insert(("long", style), vec![row.clone(); 3]);
        let change =
            |all: &mut BTreeMap<_, Vec<Measurement>>, meeting, edit: fn(&mut Measurement)| {
                let rows = all.get_mut(&(meeting, style)).unwrap();
                for row in &mut rows[..2] {
                    edit(row);
                }
            };
        let gate = |all: &BTreeMap<_, _>| {
            let mut result = String::new();
            write_gate(&mut result, all, &BTreeMap::new()).unwrap();
            result
        };

        // A single high outlier does not move the median. Two wrong merges still pass.
        all.get_mut(&("m1", style)).unwrap()[2].scores.agendas = 300;
        change(&mut all, "m1", |row| row.scores.wrong_merges = 2);
        assert!(gate(&all).contains("**PASS**"));

        type Edit = fn(&mut Measurement);
        let cases: [(_, Edit); 5] = [
            ("m1", |row| row.scores.agendas = 4),
            ("m2", |row| row.scores.wrong_merges = 1),
            ("m3", |row| row.scores.node_recall = 0.99),
            ("long", |row| row.pairs = 2),
            ("m1", |row| row.scores.agendas = 0),
        ];
        for (meeting, edit) in cases {
            let mut trials = all.clone();
            change(&mut trials, meeting, edit);
            assert!(gate(&trials).contains("**FAIL**"), "{meeting}");
        }

        all.get_mut(&("long", style)).unwrap()[0].complete = false;
        assert!(gate(&all).contains("Inconclusive."));
    }

    #[test]
    fn counts_a_duplicate_only_if_distinct_extracted_issues_merge_during_consolidation() {
        let (utterances, gold) = data::load("long").unwrap();
        let [early, late] = &gold.duplicate_issue_pairs[0];
        let mut graph = Graph {
            utterances: utterances.into_iter().map(|u| (u.uid.clone(), u)).collect(),
            ..Graph::default()
        };
        let nodes: Vec<_> = [early, late]
            .iter()
            .map(|id| {
                gold.graph
                    .nodes
                    .iter()
                    .find(|node| &node.id == *id)
                    .unwrap()
            })
            .collect();
        let uids: Vec<_> = nodes
            .iter()
            .map(|node| data::uid(node.grounds[0]))
            .collect();
        let allowed: Vec<_> = uids.iter().map(String::as_str).collect();
        let adds = nodes
            .iter()
            .zip(&uids)
            .map(|(node, uid)| {
                Op(json!({
                    "op":"add_node", "tmp":node.id, "kind":"issue", "text":node.text,
                    "confidence":1.0, "grounds":[{"uid":uid}],
                }))
            })
            .collect::<Vec<_>>();
        graph = apply_patch(&graph, &adds, &allowed).graph;
        let merge = Op(json!({
            "op":"merge", "from":"n2", "into":"n1", "grounds":[{"uid":uids[1]}],
        }));
        let after = apply_patch(&graph, &[merge], &allowed).graph;
        assert_eq!(merged_pairs(&graph, &after, &gold), 1);
        assert_eq!(merged_pairs(&graph, &graph, &gold), 0);
        assert_eq!(merged_pairs(&after, &after, &gold), 0);
    }
}
