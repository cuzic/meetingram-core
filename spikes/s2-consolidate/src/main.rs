mod data;
mod fake;
mod report;
mod run;

use std::{path::PathBuf, str::FromStr};

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Run {
        #[command(flatten)]
        config: Config,
        #[arg(long)]
        out: PathBuf,
    },
    Report {
        #[arg(long)]
        out: PathBuf,
    },
    Estimate {
        #[command(flatten)]
        config: Config,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
enum Backend {
    Fake,
    ClaudeCli,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct Window {
    count: usize,
    seconds: u64,
}

impl FromStr for Window {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "6/45" => Ok(Self {
                count: 6,
                seconds: 45,
            }),
            "12/90" => Ok(Self {
                count: 12,
                seconds: 90,
            }),
            _ => bail!("window must be 6/45 or 12/90"),
        }
    }
}

#[derive(Args, Serialize, Deserialize)]
struct Config {
    #[arg(long, value_enum)]
    backend: Backend,
    #[arg(long, default_value = "sonnet")]
    model: String,
    #[arg(long, default_value = "6/45")]
    window: Window,
    #[arg(long, default_value = "3", value_parser = clap::value_parser!(u16).range(1..))]
    repeats: u16,
    #[arg(long, default_value = "700", value_parser = clap::value_parser!(u32).range(1..))]
    max_calls: u32,
    #[arg(long)]
    short_only: bool,
    #[arg(long)]
    strengthened_only: bool,
}

impl Config {
    fn meetings(&self) -> &'static [&'static str] {
        let short = &["m1", "m2", "m3", "budget", "design", "one-on-one"];
        if self.short_only {
            short
        } else {
            &["m1", "m2", "m3", "budget", "design", "one-on-one", "long"]
        }
    }

    fn styles(&self) -> &'static [Style] {
        if self.strengthened_only {
            &[Style::Strengthened]
        } else {
            &[Style::Baseline, Style::Strengthened]
        }
    }

    fn is_sonnet_gate(&self) -> bool {
        self.backend == Backend::ClaudeCli
            && self.model == "sonnet"
            && self.window.count == 6
            && self.repeats == 3
            && !self.short_only
            && !self.strengthened_only
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Baseline,
    Strengthened,
}

impl Style {
    fn label(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Strengthened => "strengthened",
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let result = match Cli::parse().command {
        Command::Run { config, out } => run::run(&config, &out).await,
        Command::Report { out } => report::report(&out),
        Command::Estimate { config } => run::estimate(&config),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
