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
    assert!(run.status.success());
    let stderr = String::from_utf8(run.stderr).unwrap();
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
            .all(|line| line.split(" | ").skip(6).take(2).eq(["1.000", "1.000"]))
    );
    let long: Vec<_> = rows
        .iter()
        .filter(|line| line.starts_with("| long | two-stage |"))
        .collect();
    assert_eq!(long.len(), 3);
    assert!(
        long.iter()
            .all(|line| { line.split(" | ").nth(15).unwrap().parse::<usize>().unwrap() > 1 })
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
            .filter(|row| row.ends_with("| baseline not applicable |"))
            .count(),
        3
    );
    assert!(report.contains("Not evaluated."));
    assert!(
        rows.iter()
            .any(|row| row.starts_with("| m3 | two-stage | 1 | 3 | 3 | 1 |"))
    );
    assert!(long.iter().all(|row| row.contains(" | full annotations | ")
        || row.contains(" | short decision | ")
        || row.contains(" | no annotations | ")
        || row.contains(" | UID only | ")
        || row.contains(" | omitted issues | ")));

    assert!(report.contains("| Stage 1 parent adds | Stage 1 parent removals |"));
    let m3 = rows
        .iter()
        .find(|line| line.starts_with("| m3 | two-stage | 1 |"))
        .unwrap();
    assert!(m3.ends_with("| 0 | 0 | complete |"));
    let (_, appendix) = report.split_once("## Appendix: Parent samples").unwrap();
    let (samples, _) = appendix.split_once("## Appendix: Changed matches").unwrap();
    assert!(samples.contains("m3 / two-stage / trial 1"));
    assert!(samples.contains("[flush]"));
    assert!(!samples.contains("trial 2"));

    assert!(report.contains("## Task 4b gate (ADR r15)"));

    // Use the fake artifacts to exercise the Sonnet gate. A short meeting's
    // empty flush graph has zero recall, even though its saved baseline graphs
    // still have full recall.
    let config_path = out.join("config.json");
    let mut config = read_json(&config_path);
    config["backend"] = "ClaudeCli".into();
    fs::write(config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let flush_path = out.join("m1/flush.json");
    let full_flush = read_json(&flush_path);
    let empty = serde_json::to_value(discussion_graph::Graph::default()).unwrap();
    fs::write(&flush_path, serde_json::to_vec(&empty).unwrap()).unwrap();
    for repeat in 1..=3 {
        for style in ["baseline", "two-stage"] {
            let path = out.join(format!("m1/{style}-{repeat}.json"));
            let mut trial = read_json(&path);
            if style == "baseline" {
                if repeat == 1 {
                    // Older runs did not explicitly record the input-limit skip.
                    trial
                        .as_object_mut()
                        .unwrap()
                        .remove("baseline_not_applicable");
                } else {
                    trial["baseline_not_applicable"] = true.into();
                }
                trial["complete"] = false.into();
                trial["calls"] = 0.into();
            } else {
                trial["graph"] = empty.clone();
            }
            fs::write(path, serde_json::to_vec(&trial).unwrap()).unwrap();
        }
    }
    let gate_report = || {
        assert!(
            Command::new(binary)
                .args(["report", "--out"])
                .arg(&out)
                .status()
                .unwrap()
                .success()
        );
        fs::read_to_string(out.join("RESULTS.md")).unwrap()
    };
    // Replay an undetermined merge in the saved fake output. An endpoint
    // with no matching evidence is classified using the original flush.
    let path = out.join("m2/two-stage-1.json");
    let mut trial = read_json(&path);
    let mut before = read_json(&out.join("m2/flush.json"));
    let nodes = before["nodes"].as_object_mut().unwrap();
    let into = nodes.keys().next().unwrap().clone();
    let mut unmatched = nodes[&into].clone();
    unmatched["nid"] = "n999".into();
    unmatched["text"] = "synthetic unmatched issue".into();
    nodes.insert("n999".to_owned(), unmatched);
    fs::write(
        out.join("m2/flush.json"),
        serde_json::to_vec(&before).unwrap(),
    )
    .unwrap();
    trial["applied"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "op": "merge", "from": "n999", "into": into, "grounds": [],
        }));
    fs::write(path, serde_json::to_vec(&trial).unwrap()).unwrap();

    let report = gate_report();
    assert!(report.contains("## Appendix: Undetermined merges"));
    assert!(report.contains(&format!("m2 / two-stage / trial 1, `n999` → `{into}`")));
    assert!(report.contains("From: \"synthetic unmatched issue\""));
    assert!(report.contains(&format!("Into: {}", before["nodes"][&into]["text"])));
    assert!(report.contains("**FAIL**")); // Fake consolidation merges no long-meeting pairs.
    assert!(report.contains("m1 0.000 vs 0.000 (flush)"));
    assert!(report.contains("- Content recall across six meetings (condition 3): PASS"));
    fs::write(flush_path, serde_json::to_vec(&full_flush).unwrap()).unwrap();
    let report = gate_report();
    assert!(report.contains("m1 0.000 vs 1.000 (flush)"));
    assert!(report.contains("- Content recall across six meetings (condition 3): FAIL"));

    let retry = Command::new(binary)
        .args(["run", "--backend", "fake", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(!retry.status.success());
    let calls = fs::read_to_string(out.join("calls.jsonl")).unwrap();
    assert!(!calls.is_empty());
    let mut stage_calls = 0;
    for line in calls.lines() {
        let call: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(call["input_chars"].as_u64().unwrap() <= 24_000);
        assert!(call["parsed"].as_bool().unwrap());
        assert_eq!(call["dropped_ops"], 0);
        assert!(line.is_ascii());
        if !call["issue_stage"].is_null() {
            assert_eq!(call["phase"], "two-stage");
            stage_calls += 1;
        }
    }
    assert_eq!(stage_calls, 21);
}

fn read_json(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
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
    assert_eq!(run.status.code(), Some(1));
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
    assert_eq!(run.status.code(), Some(1));
    assert!(stderr.contains("were incomplete"));
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
