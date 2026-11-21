mod backend;
mod graph;
mod replay;
mod report;

use anyhow::{Context, Result, ensure};
use backend::Backend;
use clap::{Args, Parser, Subcommand};
use graph::Graph;
use report::Attempt;
use std::{
    collections::BTreeSet,
    fs,
    io::{BufWriter, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Count {
        #[command(flatten)]
        window: Window,
    },
    Run(Run),
    Aggregate {
        #[arg(long)]
        out: PathBuf,
        #[arg(required = true)]
        runs: Vec<PathBuf>,
    },
}

#[derive(Args)]
struct Window {
    #[arg(long)]
    input: PathBuf,
    #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(u32).range(1..))]
    count: u32,
    #[arg(long, default_value_t = 45, value_parser = clap::value_parser!(u32).range(1..))]
    seconds: u32,
}

#[derive(Args)]
struct Run {
    #[command(flatten)]
    window: Window,
    #[arg(long)]
    gold: PathBuf,
    #[arg(long)]
    backend: Backend,
    #[arg(long)]
    model: Option<String>,
    #[arg(long, default_value_t = 3)]
    k: usize,
    #[arg(long, default_value_t = 24_000)]
    input_limit: usize,
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u32).range(1..))]
    timeout: u32,
    #[arg(long)]
    out: PathBuf,
    // Shared by sequential matrix runs, including retries and budget-induced splits.
    #[arg(long)]
    call_budget: Option<PathBuf>,
}

impl Window {
    fn load(&self) -> Result<(Vec<replay::Row>, Vec<usize>)> {
        let rows =
            replay::load(&self.input).with_context(|| format!("入力: {}", self.input.display()))?;
        let windows = replay::windows(&rows, self.count as usize, u64::from(self.seconds) * 1_000);
        Ok((rows, windows))
    }
}

fn spend(path: Option<&PathBuf>) -> Result<()> {
    if let Some(path) = path {
        let remaining: u32 = fs::read_to_string(path)?.trim().parse()?;
        ensure!(
            remaining > 0,
            "マトリクスの 400 回の呼び出し上限に達しました"
        );
        fs::write(path, (remaining - 1).to_string())?;
    }
    Ok(())
}

async fn run(args: Run) -> Result<()> {
    let (rows, boundaries) = args.window.load()?;
    let gold: Graph = serde_json::from_slice(&fs::read(&args.gold)?)?;
    fs::create_dir_all(&args.out)?;
    let mut calls = BufWriter::new(fs::File::create(args.out.join("calls.jsonl"))?);
    let mut graph = Graph::default();
    let mut recent = Vec::<BTreeSet<usize>>::new();
    let mut cursor = 0;
    for (index, end) in boundaries.into_iter().enumerate() {
        while cursor < end {
            let input = replay::input(&rows, cursor..end, &graph, &recent, args.input_limit);
            let mut consumed = false;
            let mut parse_error = None;
            for retry in [false, true] {
                let prompt = match parse_error {
                    Some(error) => format!(
                        "{}\n前回の解析エラー: {error}。正しい JSON オブジェクトを返してください。",
                        input.prompt
                    ),
                    None => input.prompt.clone(),
                };
                let input_chars = prompt.chars().count();
                let allowed = input_chars <= args.input_limit && input.consumed > 0;
                let start = Instant::now();
                let reply = if allowed {
                    spend(args.call_budget.as_ref())?;
                    backend::call(
                        args.backend,
                        args.model.as_deref(),
                        &prompt,
                        Duration::from_secs(args.timeout.into()),
                    )
                    .await
                } else {
                    backend::Reply {
                        error: Some("input_limit"),
                        ..backend::Reply::default()
                    }
                };
                let parsed = reply.text.as_deref().map(replay::parse).transpose();
                let mut attempt = Attempt {
                    window: index + 1,
                    retry,
                    input_chars,
                    called: allowed,
                    input_tokens: reply.input_tokens,
                    output_tokens: reply.output_tokens,
                    elapsed_ms: start.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                    parsed: matches!(parsed, Ok(Some(_))),
                    error: reply
                        .error
                        .or_else(|| parsed.as_ref().err().copied())
                        .map(str::to_owned),
                    raw: reply.raw,
                    dropped: Default::default(),
                    applied: Vec::new(),
                };
                parse_error = parsed
                    .as_ref()
                    .err()
                    .copied()
                    .or_else(|| reply.error.filter(|&error| error == "invalid_envelope"));
                if let Ok(Some(ops)) = parsed {
                    let patch = graph.apply(ops, &input.grounds);
                    attempt.dropped = patch
                        .dropped
                        .into_iter()
                        .map(|(key, count)| (key.to_owned(), count))
                        .collect();
                    attempt.applied = patch.applied;
                    recent.push(patch.touched);
                    if recent.len() > args.k {
                        recent.remove(0);
                    }
                    consumed = true;
                }
                serde_json::to_writer(&mut calls, &attempt)?;
                writeln!(calls)?;
                calls.flush()?;
                if parse_error.is_none() {
                    break;
                }
            }
            if !consumed {
                break;
            }
            cursor += input.consumed;
        }
    }
    fs::write(
        args.out.join("graph.json"),
        serde_json::to_vec_pretty(&graph)?,
    )?;
    let mut score = report::evaluate(&graph, &gold);
    score.duration_ms = rows.first().map(|first| {
        rows.iter()
            .map(|row| row.end_ms)
            .max()
            .unwrap_or(first.start_ms)
            - first.start_ms
    });
    fs::write(
        args.out.join("score.json"),
        serde_json::to_vec_pretty(&score)?,
    )?;
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Count { window } => {
            println!("{}", window.load()?.1.len());
            Ok(())
        }
        Command::Run(args) => run(args).await,
        Command::Aggregate { out, runs } => report::aggregate(out, &runs),
    }
}

#[cfg(test)]
mod meeting_tests {
    use super::*;

    #[test]
    fn synthetic_meetings_cover_review_profile() {
        let mut total_calls = 0;
        for meeting in ["design", "budget", "one-on-one"] {
            let rows = replay::load(&PathBuf::from(format!("data/{meeting}.jsonl"))).unwrap();
            let duration = rows.last().unwrap().end_ms - rows[0].start_ms;
            assert!(
                (900_000..=1_200_000).contains(&duration),
                "{meeting}: {duration} ms"
            );
            assert!((150..=180).contains(&rows.len()), "{meeting}");
            let mut lengths: Vec<_> = rows.iter().map(|row| row.text.chars().count()).collect();
            lengths.sort_unstable();
            let median =
                (lengths[(lengths.len() - 1) / 2] + lengths[lengths.len() / 2]) as f64 / 2.0;
            assert!(
                (30.0..=40.0).contains(&median),
                "{meeting}: median {median}"
            );
            assert!(
                lengths.iter().filter(|&&len| len >= 100).count() >= lengths.len().div_ceil(10),
                "{meeting}"
            );
            assert!(
                lengths.iter().filter(|&&len| len >= 300).count() >= 3,
                "{meeting}"
            );
            for row in &rows {
                let milliseconds = row.end_ms - row.start_ms;
                let chars = row.text.chars().count() as u64 * 1000;
                assert!(
                    (6 * milliseconds..=8 * milliseconds).contains(&chars),
                    "{meeting}: row {}",
                    row.row_id
                );
            }
            let mut long_pauses = 0;
            for pair in rows.windows(2) {
                let pause = pair[1].start_ms.checked_sub(pair[0].end_ms).unwrap();
                assert!(
                    (300..=3000).contains(&pause) || (10_000..=60_000).contains(&pause),
                    "{meeting}: pause {pause}"
                );
                long_pauses += usize::from(pause >= 10_000);
            }
            assert!(long_pauses >= 3, "{meeting}");
            let gold: Graph =
                serde_json::from_slice(&fs::read(format!("gold/{meeting}.json")).unwrap()).unwrap();
            for (kind, minimum) in [("issue", 5), ("position", 10)] {
                assert!(
                    gold.nodes.iter().filter(|node| node.kind == kind).count() >= minimum,
                    "{meeting}: {kind}"
                );
            }
            let ids: BTreeSet<_> = rows.iter().map(|row| row.row_id).collect();
            assert!(
                gold.nodes
                    .iter()
                    .all(|node| !node.grounds.is_empty() && node.grounds.is_subset(&ids)),
                "{meeting}"
            );
            total_calls += 4 * replay::windows(&rows, 6, 45_000).len()
                + replay::windows(&rows, 12, 90_000).len();
        }
        assert!(total_calls <= 400, "{total_calls} base calls");
    }
}
