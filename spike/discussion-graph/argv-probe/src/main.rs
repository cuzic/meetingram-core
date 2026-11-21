// Spike S1 (meetingram ADR 0001): how long a prompt can Rust's Command pass on Windows,
// to a .exe vs an npm-style .cmd shim, and whether bare names resolve .cmd shims.
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

fn payload(n: usize) -> String {
    let unit = "会議の発言「品質を優先すべき」と\"引用\"、100% の確率 & A^B <x> | y\n";
    let mut s = String::new();
    while s.chars().count() < n {
        s.push_str(unit);
    }
    s.chars().take(n).collect()
}

fn expect(s: &str) -> String {
    let sum: u64 = s.chars().fold(0u64, |h, c| h.wrapping_mul(31).wrapping_add(c as u64));
    format!("{} {}", s.chars().count(), sum)
}

async fn try_arg(prog: &str, n: usize) -> String {
    let p = payload(n);
    match Command::new(prog).arg(&p).output().await {
        Ok(o) => {
            let got = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if got == expect(&p) { "ok".into() } else { format!("mismatch(exit={:?}, got='{}')", o.status.code(), got.chars().take(40).collect::<String>()) }
        }
        Err(e) => format!("err({:?}: {})", e.kind(), e),
    }
}

async fn try_stdin(prog: &str, n: usize) -> String {
    let p = payload(n);
    let mut c = match Command::new(prog).arg("-").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return format!("err({:?})", e.kind()),
    };
    let mut si = c.stdin.take().unwrap();
    let p2 = p.clone();
    let w = tokio::spawn(async move { si.write_all(p2.as_bytes()).await.ok(); drop(si); });
    let o = c.wait_with_output().await.unwrap();
    w.await.ok();
    let got = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if got == expect(&p) { "ok".into() } else { "mismatch".into() }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    let exe = dir.join(if cfg!(windows) { "echo_arg.exe" } else { "echo_arg" });
    let exe_s = exe.to_string_lossy().to_string();
    let shim = dir.join("echo_shim.cmd");
    if cfg!(windows) {
        std::fs::write(&shim, "@ECHO off\r\n\"%~dp0echo_arg.exe\" %*\r\n").unwrap();
    }
    let shim_s = shim.to_string_lossy().to_string();
    println!("| chars | .exe argv | .cmd shim argv | .exe stdin |");
    println!("| --- | --- | --- | --- |");
    for n in [1000, 4000, 8000, 12000, 16000, 20000, 24000, 28000, 32000, 100000] {
        let a = try_arg(&exe_s, n).await;
        let b = if cfg!(windows) { try_arg(&shim_s, n).await } else { "-".into() };
        let c = try_stdin(&exe_s, n).await;
        println!("| {n} | {a} | {b} | {c} |");
    }
    // bare-name resolution like CliBackend::binary()
    for name in ["claude", "codex", "echo_shim"] {
        let r = Command::new(name).arg("--version").current_dir(&dir).output().await;
        let r = match r { Ok(o) => format!("spawned exit={:?} out='{}'", o.status.code(), String::from_utf8_lossy(&o.stdout).trim().chars().take(60).collect::<String>()), Err(e) => format!("err({:?})", e.kind()) };
        println!("bare `{name}`: {r}");
    }
}
