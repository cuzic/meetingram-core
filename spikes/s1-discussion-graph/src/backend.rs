use clap::ValueEnum;
use serde_json::Value;
use std::{process::Stdio, time::Duration};
use tokio::{process::Command, time::timeout};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Backend {
    Fake,
    Claude,
    Codex,
}

#[derive(Default)]
pub struct Reply {
    pub raw: Option<String>,
    pub text: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub error: Option<&'static str>,
}

pub async fn call(backend: Backend, model: Option<&str>, prompt: &str, limit: Duration) -> Reply {
    if matches!(backend, Backend::Fake) {
        let text = r#"{"ops":[]}"#.to_owned();
        return Reply {
            raw: Some(text.clone()),
            text: Some(text),
            ..Reply::default()
        };
    }
    let result = timeout(limit, command(backend, model, prompt))
        .await
        .unwrap_or(Err("timeout"));
    result.unwrap_or_else(|error| Reply {
        error: Some(error),
        ..Reply::default()
    })
}

async fn command(
    backend: Backend,
    model: Option<&str>,
    prompt: &str,
) -> Result<Reply, &'static str> {
    let dir = tempfile::tempdir().map_err(|_| "tempdir")?;
    let output_path = dir.path().join("codex-output.txt");
    let (mut command, model_flag) = match backend {
        Backend::Claude => {
            let mut command = Command::new("claude");
            command.args([
                "--print",
                "--output-format",
                "json",
                "--no-session-persistence",
                "--disallowedTools",
                "Bash,Edit,Write,Read,WebFetch,WebSearch,Task,Glob,Grep,NotebookEdit",
            ]);
            (command, "--model")
        }
        Backend::Codex => {
            let mut command = Command::new("codex");
            command
                .args([
                    "exec",
                    "--json",
                    "-s",
                    "read-only",
                    "--skip-git-repo-check",
                    "--ephemeral",
                    "-C",
                ])
                .arg(dir.path())
                .arg("-o")
                .arg(&output_path);
            (command, "-m")
        }
        Backend::Fake => unreachable!(),
    };
    if let Some(model) = model {
        command.args([model_flag, model]);
    }
    let output = command
        .current_dir(dir.path())
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .arg("--")
        .arg(prompt)
        .output()
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "not_found"
            } else {
                "spawn"
            }
        })?;
    let raw = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        return Ok(Reply {
            raw: Some(raw),
            error: Some("non_zero_exit"),
            ..Reply::default()
        });
    }
    let (text, usage) = if matches!(backend, Backend::Claude) {
        let envelope = serde_json::from_str::<Value>(&raw).unwrap_or_default();
        let text = envelope["result"].as_str().map(str::to_owned);
        (text, envelope["usage"].clone())
    } else {
        let text = std::fs::read_to_string(output_path).ok();
        let usage = raw
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|event| event["type"] == "turn.completed")
            .map(|event| event["usage"].clone())
            .unwrap_or_default();
        (text, usage)
    };
    let tokens = |key: &str| usage[key].as_u64();
    let input_tokens = tokens("input_tokens").map(|n| {
        n + tokens("cache_creation_input_tokens").unwrap_or(0)
            + tokens("cache_read_input_tokens").unwrap_or(0)
    });
    Ok(Reply {
        raw: Some(raw),
        error: text.is_none().then_some("invalid_envelope"),
        text,
        input_tokens,
        output_tokens: tokens("output_tokens"),
    })
}
