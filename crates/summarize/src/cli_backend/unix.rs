use super::*;
use std::process::Stdio;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

pub(super) fn resolve(name: &OsStr) -> io::Result<PathBuf> {
    Ok(PathBuf::from(name))
}

pub(super) struct Group {
    child: Child,
    id: libc::pid_t,
}

impl Drop for Group {
    fn drop(&mut self) {
        // SAFETY: id is the positive process group ID of the spawned child.
        unsafe { libc::killpg(self.id, libc::SIGKILL) };
    }
}

pub(super) struct Process {
    pub(super) group: Group,
    pub(super) stdin: Option<ChildStdin>,
    pub(super) stdout: ChildStdout,
    pub(super) stderr: ChildStderr,
}

impl Process {
    pub(super) fn spawn(binary: &Path, args: &[OsString], dir: Option<&Path>) -> io::Result<Self> {
        let mut command = Command::new(binary);
        command
            .args(args)
            .process_group(0)
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = dir {
            command.current_dir(dir);
        }
        let mut child = command.spawn()?;
        let id = child.id().expect("new child has a process ID") as libc::pid_t;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        Ok(Self {
            group: Group { child, id },
            stdin,
            stdout,
            stderr,
        })
    }
}

impl Group {
    pub(super) async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }
}
