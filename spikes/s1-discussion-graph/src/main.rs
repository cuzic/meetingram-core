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
    fs::write(
        args.out.join("score.json"),
        serde_json::to_vec_pretty(&report::evaluate(&graph, &gold))?,
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
