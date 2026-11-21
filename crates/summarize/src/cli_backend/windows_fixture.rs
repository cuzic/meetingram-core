// Compiled by the Windows test so the .cmd shim forwards to a native CLI, as
// the npm shim does. Failure codes identify which synthetic input was lost.
use std::io::Read;
use std::path::Path;
use std::process::{exit, Command};
use std::time::Duration;

fn require(check: bool, code: i32) {
    if !check {
        exit(code);
    }
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--sleep"] {
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    let mut user = String::new();
    std::io::stdin().read_to_string(&mut user).unwrap();
    require(user == "abcdefghij\n".repeat(3000), 33);

    let dir = std::env::current_dir().unwrap();
    if args[0] == "exec" {
        require(Path::new(&args[8]) == dir, 21);
        require(Path::new(&args[10]) == dir.join("out.txt"), 22);
        require(args[12] == r#"model_instructions_file="system.txt""#, 23);
        require(args[18] == r#"web_search="disabled""#, 24);
        std::fs::write(&args[10], "synthetic summary").unwrap();
        println!(r#"{{"type":"item.completed","item":{{"type":"agent_message"}}}}"#);
        return;
    }

    require(args[4..7] == ["--tools", "", "--strict-mcp-config"], 31);
    require(
        std::fs::read_to_string(&args[8]).unwrap() == "synthetic instruction",
        32,
    );
    let binary = std::env::current_exe().unwrap();
    let fixture = binary.parent().unwrap();
    if fixture.join("sleep").exists() {
        let mut child = Command::new(&binary).arg("--sleep").spawn().unwrap();
        std::fs::write(fixture.join("grandchild"), child.id().to_string()).unwrap();
        child.wait().unwrap();
    }
    println!(r#"{{"result":"synthetic summary"}}"#);
}
