use std::{fs, process::Command};

#[test]
fn fake_runs_and_reports_all_seven_meetings() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("run");
    let binary = env!("CARGO_BIN_EXE_s2-consolidate");
    let run = Command::new(binary)
        .args(["run", "--backend", "fake", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(!run.status.success());
    let stderr = String::from_utf8(run.stderr).unwrap();
    assert_eq!(stderr.lines().count(), 4);
    assert_eq!(
        stderr.matches("could not fit consolidation input").count(),
        3
    );
    let report = Command::new(binary)
        .args(["report", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(report.status.success());
    let report = fs::read_to_string(out.join("RESULTS.md")).unwrap();
    let rows: Vec<_> = report
        .lines()
        .filter(|line| line.contains("| 1.000 |"))
        .collect();
    assert_eq!(rows.len(), 42);
    assert!(
        rows.iter()
            .all(|line| line.split(" | ").nth(6) == Some("1.000"))
    );
    let long: Vec<_> = rows
        .iter()
        .filter(|line| line.starts_with("| long | two-stage |"))
        .collect();
    assert_eq!(long.len(), 3);
    assert!(
        long.iter()
            .all(|line| { line.split(" | ").nth(13).unwrap().parse::<usize>().unwrap() > 1 })
    );
    assert!(run.stdout.is_empty());
    assert_eq!(
        rows.iter()
            .filter(|row| row.ends_with("| complete |"))
            .count(),
        39
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.starts_with("| long | baseline |"))
            .filter(|row| row.ends_with("| consolidation stopped |"))
            .count(),
        3
    );
    assert!(report.contains("Not evaluated."));
    let retry = Command::new(binary)
        .args(["run", "--backend", "fake", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(!retry.status.success());
    let calls = fs::read_to_string(out.join("calls.jsonl")).unwrap();
    assert!(!calls.is_empty());
    for line in calls.lines() {
        let call: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(call["input_chars"].as_u64().unwrap() <= 24_000);
        assert!(call["parsed"].as_bool().unwrap());
        assert_eq!(call["dropped_ops"], 0);
        assert!(line.is_ascii());
    }
}

#[test]
fn call_cap_stops_before_the_next_call() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("run");
    let run = Command::new(env!("CARGO_BIN_EXE_s2-consolidate"))
        .args(["run", "--backend", "fake", "--max-calls", "2", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("--max-calls reached"));
    let calls = fs::read_to_string(out.join("calls.jsonl")).unwrap();
    assert_eq!(calls.lines().count(), 2);
    assert!(!out.join("m1/flush.json").exists());
}

#[cfg(unix)]
#[test]
fn three_failures_skip_consolidation_and_keep_cli_body_out_of_logs() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let stub = dir.path().join("claude");
    let marker = "synthetic-private-body";
    fs::write(
        &stub,
        format!("#!/bin/sh\nprintf '%s\\n' '{marker}' >&2\nexit 1\n"),
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700)).unwrap();
    let out = dir.path().join("run");
    let run = Command::new(env!("CARGO_BIN_EXE_s2-consolidate"))
        .env("PATH", dir.path())
        .args(["run", "--backend", "claude-cli", "--short-only", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(!run.status.success());
    let stderr = String::from_utf8(run.stderr).unwrap();
    assert_eq!(stderr.matches("flush stopped").count(), 6);
    assert!(!stderr.contains(marker));
    assert!(!String::from_utf8_lossy(&run.stdout).contains(marker));
    let calls = fs::read_to_string(out.join("calls.jsonl")).unwrap();
    assert!(!calls.contains(marker));
    let mut counts = std::collections::BTreeMap::new();
    for line in calls.lines() {
        let call: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(call["phase"], "flush");
        assert_eq!(call["parsed"], false);
        assert_eq!(call["error"], "non_zero_exit");
        *counts
            .entry(call["meeting"].as_str().unwrap().to_owned())
            .or_insert(0) += 1;
    }
    assert_eq!(counts.len(), 6);
    assert!(counts.values().all(|&count| count == 3));
}

#[cfg(unix)]
#[test]
fn matrix_estimates_first_and_continues_after_failed_runs() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let cargo = dir.path().join("cargo");
    fs::write(
        &cargo,
        r#"#!/bin/bash
case "$1" in
    build) exit 0 ;;
esac
shift 4
printf '%s\n' "$*" >> "$MATRIX_TRACE"
case "$1" in
    run)
        shift
        while [[ "$1" != --out ]]; do shift; done
        mkdir -p "$2"
        touch "$2/config.json"
        exit 1 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o700)).unwrap();

    let trace_path = dir.path().join("trace");
    let out = dir.path().join("results");
    let mut paths = vec![dir.path().to_path_buf()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let run = || {
        Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/run-matrix.sh"))
            .env("PATH", std::env::join_paths(&paths).unwrap())
            .env("MATRIX_TRACE", &trace_path)
            .env("MAX_CALLS", "25")
            .arg(&out)
            .output()
            .unwrap()
    };

    assert!(!run().status.success());
    let trace = fs::read_to_string(&trace_path).unwrap();
    let lines: Vec<_> = trace.lines().collect();
    assert_eq!(lines.len(), 9);
    for (index, (name, model)) in [
        ("sonnet", "sonnet"),
        ("haiku", "haiku"),
        ("sonnet-wide", "sonnet"),
    ]
    .into_iter()
    .enumerate()
    {
        let destination = out.join(name);
        assert!(lines[index].starts_with("estimate --backend claude-cli --max-calls 25"));
        assert!(lines[index].contains(&format!("--model {model}")));
        assert!(lines[3 + 2 * index].starts_with("run --backend claude-cli --max-calls 25"));
        assert!(lines[3 + 2 * index].contains(&format!("--out {}", destination.display())));
        assert_eq!(
            lines[4 + 2 * index],
            format!("report --out {}", destination.display())
        );
    }
    assert!(lines[2].contains("--window 12/90 --short-only --two-stage-only --repeats 1"));
    assert!(!run().status.success());
    assert_eq!(fs::read_to_string(trace_path).unwrap().lines().count(), 12);
}
