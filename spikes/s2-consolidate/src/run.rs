use std::{
    collections::BTreeSet,
    fs,
    io::{BufWriter, Write},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Result, bail, ensure};
use discussion_graph::{
    AppliedOp, ConsolidateStyle, DEFAULT_INPUT_LIMIT, Graph, PatchResult, Prompt, Utterance,
    apply_patch, build_consolidate, build_consolidate_groups, build_consolidate_issues,
    build_incremental, eval::GoldGraph, parse_ops,
};
use serde::{Deserialize, Serialize};
use summarize::{CliSummarizer, CompletionRequest, Summarizer, cli_backend::CliBackend};

use crate::{Backend, Config, Style, Window, data, fake};

const FLUSH_TIMEOUT: Duration = Duration::from_secs(60);
const CONSOLIDATE_TIMEOUT: Duration = Duration::from_secs(120);
const RECENT_PATCHES: usize = 3;

#[derive(Serialize)]
struct Call {
    meeting: &'static str,
    phase: &'static str,
    repeat: Option<u16>,
    elapsed_ms: u128,
    input_chars: usize,
    output_chars: usize,
    parsed: bool,
    dropped_ops: usize,
    error: Option<&'static str>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Trial {
    pub graph: Graph,
    pub applied: Vec<AppliedOp>,
    pub elapsed_ms: u128,
    pub timeouts: usize,
    pub calls: usize,
    pub omitted_issues: usize,
    pub omitted_group_nodes: usize,
    pub complete: bool,
}

struct Runner<'a> {
    config: &'a Config,
    cli: Option<CliSummarizer>,
    log: BufWriter<fs::File>,
    remaining: u32,
}

struct CallResult {
    patch: Option<PatchResult>,
    timed_out: bool,
}

impl Runner<'_> {
    async fn call(
        &mut self,
        meeting: &'static str,
        style: Option<(Style, u16)>,
        graph: &Graph,
        gold: &GoldGraph,
        prompt: Prompt,
    ) -> Result<CallResult> {
        ensure!(
            self.remaining > 0,
            "--max-calls reached before the next call"
        );
        self.remaining -= 1;
        let (phase, repeat, timeout) = match style {
            Some((style, repeat)) => (style.label(), Some(repeat), CONSOLIDATE_TIMEOUT),
            None => ("flush", None, FLUSH_TIMEOUT),
        };
        let input_chars = prompt.system.chars().count() + prompt.user.chars().count();
        ensure!(
            input_chars <= DEFAULT_INPUT_LIMIT,
            "prompt exceeded 24,000 characters"
        );
        let start = Instant::now();
        let output = if let Some(cli) = &self.cli {
            let request =
                CompletionRequest::new(prompt.system, prompt.user, &self.config.model, timeout);
            cli.complete(&request).await.map_err(|err| err.kind())
        } else {
            Ok(fake::response(graph, gold, &prompt))
        };
        let elapsed_ms = start.elapsed().as_millis();
        let output_chars = output.as_ref().map_or(0, |text| text.chars().count());
        let parsed = output.and_then(|text| parse_ops(&text).map_err(|_| "parse"));
        let error = parsed.as_ref().err().copied();
        let patch = parsed.ok().map(|ops| {
            let allowed: Vec<_> = prompt.allowed_uids.iter().map(String::as_str).collect();
            apply_patch(graph, &ops, &allowed)
        });
        let dropped_ops = patch.as_ref().map_or(0, |p| p.rejected.values().sum());
        serde_json::to_writer(
            &mut self.log,
            &Call {
                meeting,
                phase,
                repeat,
                elapsed_ms,
                input_chars,
                output_chars,
                parsed: patch.is_some(),
                dropped_ops,
                error,
            },
        )?;
        self.log.write_all(b"\n")?;
        self.log.flush()?;
        Ok(CallResult {
            patch,
            timed_out: error == Some("timeout"),
        })
    }

    async fn flush(
        &mut self,
        meeting: &'static str,
        utterances: Vec<Utterance>,
        gold: &GoldGraph,
    ) -> Result<Option<Graph>> {
        let mut graph = Graph::default();
        let windows = windows(&utterances, self.config.window);
        let total = utterances.len();
        let uids: Vec<_> = utterances.iter().map(|u| u.uid.clone()).collect();
        graph.utterances = utterances.into_iter().map(|u| (u.uid.clone(), u)).collect();

        let mut processed = 0;
        let mut consecutive_failures = 0;
        let mut recent: Vec<BTreeSet<String>> = Vec::new();
        for range in windows {
            while processed < range.end {
                let pending: Vec<_> = uids[processed..range.end]
                    .iter()
                    .map(String::as_str)
                    .collect();
                let prompt =
                    build_incremental(&graph, &pending, &recent, false, DEFAULT_INPUT_LIMIT)?;
                let included = prompt.new_uids.len();
                ensure!(included > 0, "flush prompt included no pending utterances");
                if let Some(patch) = self.call(meeting, None, &graph, gold, prompt).await?.patch {
                    processed += included;
                    consecutive_failures = 0;
                    if patch.patch_rev().is_some() {
                        if recent.len() == RECENT_PATCHES {
                            recent.remove(0);
                        }
                        recent.push(patch.touched);
                    }
                    graph = patch.graph;
                } else {
                    consecutive_failures += 1;
                    if consecutive_failures == 3 {
                        eprintln!(
                            "{meeting}: flush stopped with {} pending utterances",
                            total - processed
                        );
                        return Ok(None);
                    }
                }
            }
        }
        Ok(Some(graph))
    }

    async fn consolidate(
        &mut self,
        meeting: &'static str,
        before: &Graph,
        gold: &GoldGraph,
        style: Style,
        repeat: u16,
    ) -> Result<Trial> {
        let start = Instant::now();
        let mut trial = Trial {
            graph: before.clone(),
            applied: Vec::new(),
            elapsed_ms: 0,
            timeouts: 0,
            calls: 0,
            omitted_issues: 0,
            omitted_group_nodes: 0,
            complete: false,
        };
        let first = match style {
            Style::Baseline => {
                build_consolidate(before, ConsolidateStyle::Baseline, DEFAULT_INPUT_LIMIT)
            }
            Style::TwoStage => build_consolidate_issues(before, DEFAULT_INPUT_LIMIT)
                .map(|prompt| prompt.into_iter().collect()),
        };
        let Ok(first) = first else {
            eprintln!(
                "{meeting}: {} trial {repeat} could not fit consolidation input",
                style.label()
            );
            trial.elapsed_ms = start.elapsed().as_millis();
            return Ok(trial);
        };

        for prompt in first {
            trial.omitted_issues += prompt.omitted_issues;
            if !self
                .consolidation_call(meeting, style, repeat, &mut trial, gold, prompt)
                .await?
            {
                return Ok(incomplete_trial(trial, before, start));
            }
        }
        if style == Style::TwoStage {
            let (prompts, omitted) = build_consolidate_groups(&trial.graph, DEFAULT_INPUT_LIMIT);
            trial.omitted_group_nodes = omitted;
            for prompt in prompts {
                if !self
                    .consolidation_call(meeting, style, repeat, &mut trial, gold, prompt)
                    .await?
                {
                    return Ok(incomplete_trial(trial, before, start));
                }
            }
        }
        trial.complete = true;
        trial.elapsed_ms = start.elapsed().as_millis();
        Ok(trial)
    }

    async fn consolidation_call(
        &mut self,
        meeting: &'static str,
        style: Style,
        repeat: u16,
        trial: &mut Trial,
        gold: &GoldGraph,
        prompt: Prompt,
    ) -> Result<bool> {
        let call = self
            .call(meeting, Some((style, repeat)), &trial.graph, gold, prompt)
            .await?;
        trial.calls += 1;
        let Some(patch) = call.patch else {
            trial.timeouts += usize::from(call.timed_out);
            return Ok(false);
        };
        trial.applied.extend(patch.applied);
        trial.graph = patch.graph;
        Ok(true)
    }
}

fn incomplete_trial(mut trial: Trial, before: &Graph, start: Instant) -> Trial {
    trial.graph = before.clone();
    trial.applied.clear();
    trial.elapsed_ms = start.elapsed().as_millis();
    trial
}

pub(crate) async fn run(config: &Config, out: &Path) -> Result<()> {
    // Refuse to mix a fresh run with existing results.
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(out)?;
    data::write_json(&out.join("config.json"), config)?;
    let cli_dir = (config.backend == Backend::ClaudeCli)
        .then(|| tempfile::Builder::new().prefix("s2-claude-").tempdir())
        .transpose()?;
    let cli = cli_dir
        .as_ref()
        .map(|dir| CliSummarizer::new(CliBackend::ClaudeCode, Some(dir.path().to_owned())));
    let mut runner = Runner {
        config,
        cli,
        log: BufWriter::new(fs::File::create(out.join("calls.jsonl"))?),
        remaining: config.max_calls,
    };
    let mut incomplete = false;
    for &meeting in config.meetings() {
        let (utterances, gold) = data::load(meeting)?;
        let Some(graph) = runner.flush(meeting, utterances, &gold.graph).await? else {
            incomplete = true;
            continue;
        };
        let dir = out.join(meeting);
        fs::create_dir(&dir)?;
        let path = dir.join("flush.json");
        data::write_json(&path, &graph)?;
        let graph = data::read_json(&path)?;
        for &style in config.styles() {
            for repeat in 1..=config.repeats {
                let trial = runner
                    .consolidate(meeting, &graph, &gold.graph, style, repeat)
                    .await?;
                incomplete |= !trial.complete;
                data::write_json(
                    &data::trial_path(out, meeting, style.label(), repeat),
                    &trial,
                )?;
            }
        }
    }
    if incomplete {
        bail!("one or more meetings or consolidation trials were incomplete");
    }
    Ok(())
}

/// Replay finalized rows in timestamp order as recorded by the fixture.
/// Each window starts at its first pending row. Missing timestamps advance the
/// simulated clock by six seconds, and a gap can close a window before its count fills.
fn windows(utterances: &[Utterance], window: Window) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut prior_end = 0;
    let mut first = 0;
    let mut deadline = 0;
    for (index, utterance) in utterances.iter().enumerate() {
        let start = utterance.start_ms.unwrap_or(prior_end);
        let end = utterance
            .end_ms
            .unwrap_or_else(|| start.saturating_add(6_000))
            .max(prior_end);
        if index > first && (index - first == window.count || end > deadline) {
            ranges.push(first..index);
            first = index;
        }
        if index == first {
            deadline = start.saturating_add(window.seconds * 1_000).max(end);
        }
        prior_end = end;
    }
    if first < utterances.len() {
        ranges.push(first..utterances.len());
    }
    ranges
}

pub(crate) fn estimate(config: &Config) -> Result<()> {
    let mut flush = 0;
    for &meeting in config.meetings() {
        let (utterances, _) = data::load(meeting)?;
        let count = windows(&utterances, config.window).len();
        println!("{meeting}: {count} extraction windows");
        flush += count;
    }
    let consolidation = config.meetings().len()
        * config
            .styles()
            .iter()
            .map(|style| match style {
                Style::Baseline => 1,
                Style::TwoStage => 2,
            })
            .sum::<usize>()
        * usize::from(config.repeats);
    println!(
        "Estimated calls without splits or retries: {} ({flush} extraction + {consolidation} consolidation)",
        flush + consolidation
    );
    println!(
        "Prompt limits and retries can add calls. Hard cap: {}",
        config.max_calls
    );
    Ok(())
}
