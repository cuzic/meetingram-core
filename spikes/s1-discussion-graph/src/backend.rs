use crate::replay::Prompt;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{process::Command, time::timeout};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Backend {
    Fake,
    Claude,
    Codex,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum CliMode {
    #[default]
    Lean,
    Full,
}

impl CliMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lean => "lean",
            Self::Full => "full",
        }
    }
}

const SYSTEM_FILE: &str = "system.txt";
const OUTPUT_FILE: &str = "codex-output.txt";

#[derive(Default)]
pub struct Reply {
    pub raw: Option<String>,
    pub text: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub error: Option<&'static str>,
}

pub async fn call(
    backend: Backend,
    mode: CliMode,
    model: Option<&str>,
    prompt: Prompt<'_>,
    limit: Duration,
) -> Reply {
    if matches!(backend, Backend::Fake) {
        let text = r#"{"ops":[]}"#.to_owned();
        return Reply {
            raw: Some(text.clone()),
            text: Some(text),
            ..Reply::default()
        };
    }
    let result = timeout(limit, command(backend, mode, model, prompt))
        .await
        .unwrap_or(Err("timeout"));
    result.unwrap_or_else(|error| Reply {
        error: Some(error),
        ..Reply::default()
    })
}

fn cli_command(
    backend: Backend,
    mode: CliMode,
    model: Option<&str>,
    prompt: Prompt<'_>,
    dir: &Path,
) -> Command {
    let (mut command, model_flag) = match backend {
        Backend::Claude => {
            let mut command = Command::new("claude");
            command.args([
                "--print",
                "--output-format",
                "json",
                "--no-session-persistence",
            ]);
            match mode {
                CliMode::Lean => command.args([
                    "--tools",
                    "",
                    "--strict-mcp-config",
                    "--setting-sources",
                    "",
                    "--system-prompt",
                    prompt.system,
                ]),
                CliMode::Full => command.args([
                    "--disallowedTools",
                    "Bash,Edit,Write,Read,WebFetch,WebSearch,Task,Glob,Grep,NotebookEdit",
                ]),
            };
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
                .arg(dir)
                .arg("-o")
                .arg(dir.join(OUTPUT_FILE));
            if matches!(mode, CliMode::Lean) {
                command.args([
                    "--ignore-user-config",
                    "-c",
                    &format!("model_instructions_file=\"{SYSTEM_FILE}\""),
                    "-c",
                    "project_doc_max_bytes=0",
                    "-c",
                    "mcp_servers={}",
                    "-c",
                    "features.shell_tool=false",
                    "-c",
                    "features.view_image=false",
                    "-c",
                    "web_search=\"disabled\"",
                ]);
            }
            (command, "-m")
        }
        Backend::Fake => unreachable!(),
    };
    if let Some(model) = model {
        command.args([model_flag, model]);
    }
    command.arg("--");
    match mode {
        CliMode::Lean => command.arg(prompt.user),
        CliMode::Full => command.arg(prompt.combined()),
    };
    command
}

async fn command(
    backend: Backend,
    mode: CliMode,
    model: Option<&str>,
    prompt: Prompt<'_>,
) -> Result<Reply, &'static str> {
    let dir = tempfile::tempdir().map_err(|_| "tempdir")?;
    if matches!((backend, mode), (Backend::Codex, CliMode::Lean)) {
        std::fs::write(dir.path().join(SYSTEM_FILE), prompt.system).map_err(|_| "system_file")?;
    }
    let output = cli_command(backend, mode, model, prompt, dir.path())
        .current_dir(dir.path())
        .kill_on_drop(true)
        .stdin(Stdio::null())
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
        let text = std::fs::read_to_string(dir.path().join(OUTPUT_FILE)).ok();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_arguments_match_backend_and_mode() {
        let dir = Path::new("work");
        let prompt = Prompt {
            system: "規則",
            user: "発話",
        };
        let claude = [
            "--print",
            "--output-format",
            "json",
            "--no-session-persistence",
        ];
        let output = dir.join("codex-output.txt");
        let codex = [
            "exec",
            "--json",
            "-s",
            "read-only",
            "--skip-git-repo-check",
            "--ephemeral",
            "-C",
            "work",
            "-o",
            output.to_str().unwrap(),
        ];
        for (backend, mode, expected) in [
            (
                Backend::Claude,
                CliMode::Full,
                [
                    claude.as_slice(),
                    &[
                        "--disallowedTools",
                        "Bash,Edit,Write,Read,WebFetch,WebSearch,Task,Glob,Grep,NotebookEdit",
                        "--model",
                        "example",
                        "--",
                        "規則\n発話",
                    ],
                ]
                .concat(),
            ),
            (
                Backend::Claude,
                CliMode::Lean,
                [
                    claude.as_slice(),
                    &[
                        "--tools",
                        "",
                        "--strict-mcp-config",
                        "--setting-sources",
                        "",
                        "--system-prompt",
                        "規則",
                        "--model",
                        "example",
                        "--",
                        "発話",
                    ],
                ]
                .concat(),
            ),
            (
                Backend::Codex,
                CliMode::Full,
                [codex.as_slice(), &["-m", "example", "--", "規則\n発話"]].concat(),
            ),
            (
                Backend::Codex,
                CliMode::Lean,
                [
                    codex.as_slice(),
                    &[
                        "--ignore-user-config",
                        "-c",
                        "model_instructions_file=\"system.txt\"",
                        "-c",
                        "project_doc_max_bytes=0",
                        "-c",
                        "mcp_servers={}",
                        "-c",
                        "features.shell_tool=false",
                        "-c",
                        "features.view_image=false",
                        "-c",
                        "web_search=\"disabled\"",
                        "-m",
                        "example",
                        "--",
                        "発話",
                    ],
                ]
                .concat(),
            ),
        ] {
            let command = cli_command(backend, mode, Some("example"), prompt, dir);
            assert_eq!(command.as_std().get_args().collect::<Vec<_>>(), expected);
        }
    }
}
