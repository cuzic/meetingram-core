use std::{error::Error, ffi::OsString, path::Path, process::Stdio, time::Duration};
use tokio::{process::Command, time::timeout};

async fn matches_length(program: &Path, prefix: &[OsString], unit: &str, len: usize) -> bool {
    let result = timeout(
        Duration::from_secs(10),
        Command::new(program)
            .args(prefix)
            .arg("--")
            .arg(unit.repeat(len))
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    matches!(result, Ok(Ok(output)) if output.status.success() &&
        String::from_utf8_lossy(&output.stdout).trim().parse::<usize>() == Ok(len))
}

async fn max_length(program: &Path, prefix: &[OsString], unit: &str) -> Option<usize> {
    if !matches_length(program, prefix, unit, 1).await {
        return None;
    }
    let (mut low, mut high) = (1, 32_768);
    while high - low > 1 {
        let mid = (low + high) / 2;
        if matches_length(program, prefix, unit, mid).await {
            low = mid;
        } else {
            high = mid;
        }
    }
    Some(low)
}

async fn versions() {
    for name in ["claude", "codex"] {
        let result = timeout(
            Duration::from_secs(20),
            Command::new(name)
                .arg("--version")
                .kill_on_drop(true)
                .stdin(Stdio::null())
                .output(),
        )
        .await;
        let outcome = match result {
            Ok(Ok(output)) if output.status.success() => {
                format!("成功: {}", String::from_utf8_lossy(&output.stdout).trim())
            }
            Ok(Ok(output)) => format!("終了コード {:?}", output.status.code()),
            Ok(Err(error)) => format!("{:?}", error.kind()),
            Err(_) => "時間切れ".into(),
        };
        println!(
            "| {name} | {} |",
            outcome.replace('|', "/").replace(['\r', '\n'], " ")
        );
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().nth(1).as_deref() == Some("versions") {
        versions().await;
        return Ok(());
    }
    let executable = std::env::current_exe()?;
    let dir = executable.parent().ok_or("executable directory missing")?;
    let exe = dir.join("echo_arg.exe");
    let shim = dir.join("echo_shim.cmd");
    std::fs::write(&shim, "@echo off\r\n\"%~dp0echo_arg.exe\" %*\r\n")?;
    let claude: Vec<_> = [
        "--print",
        "--output-format",
        "json",
        "--no-session-persistence",
        "--disallowedTools",
        "Bash,Edit,Write,Read,WebFetch,WebSearch,Task,Glob,Grep,NotebookEdit",
        "--model",
        "sonnet",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    let mut codex: Vec<_> = [
        "exec",
        "-s",
        "read-only",
        "--skip-git-repo-check",
        "--ephemeral",
        "-C",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    codex.extend([
        dir.as_os_str().to_owned(),
        "-o".into(),
        dir.join("codex-output.txt").into_os_string(),
    ]);
    println!("| CLI の前置き | 受け手 | 文字種 | 最大文字数 |\n| --- | --- | --- | --- |");
    for (cli, prefix) in [("Claude", &claude), ("Codex", &codex)] {
        for (label, path) in [(".exe", &exe), (".cmd", &shim)] {
            for (alphabet, unit) in [("日本語 (BMP)", "会"), ("ASCII", "a")] {
                let outcome = max_length(path, prefix, unit)
                    .await
                    .map_or("1 文字でも失敗".into(), |n| n.to_string());
                println!("| {cli} | {label} | {alphabet} | {outcome} |");
            }
        }
    }
    Ok(())
}
