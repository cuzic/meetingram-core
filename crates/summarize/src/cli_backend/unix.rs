use super::*;
use std::process::Stdio;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

pub(super) fn resolve(name: &OsStr) -> io::Result<PathBuf> {
    Ok(PathBuf::from(name))
}

pub(super) fn process_start(pid: u32) -> ProcessStart {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return ProcessStart::Unknown;
    };
    if pid <= 0 {
        return ProcessStart::Unknown;
    }
    // SAFETY: signal 0 only checks for existence or permission.
    if unsafe { libc::kill(pid, 0) } == -1 {
        return match io::Error::last_os_error().raw_os_error() {
            Some(libc::ESRCH) => ProcessStart::Dead,
            Some(libc::EPERM) => ProcessStart::Alive,
            _ => ProcessStart::Unknown,
        };
    }

    process_start_time(pid).map_or(ProcessStart::Alive, ProcessStart::Known)
}

#[cfg(target_os = "linux")]
fn process_start_time(pid: libc::pid_t) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name can contain spaces and parentheses. Field 22 (start
    // ticks since boot) follows the final closing parenthesis by 20 fields.
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(target_os = "macos")]
fn process_start_time(pid: libc::pid_t) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: info points to a writable buffer of size bytes. A full-size
    // result means libproc initialized the structure.
    unsafe {
        if libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        ) != size
        {
            return None;
        }
        let info = info.assume_init();
        Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_start_time(_pid: libc::pid_t) -> Option<u64> {
    None
}

pub(super) struct Group {
    child: Option<Child>,
    id: libc::pid_t,
    dir: Option<Arc<WorkingDir>>,
}

impl Drop for Group {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(super) struct Process {
    pub(super) group: Group,
    pub(super) stdin: Option<ChildStdin>,
    pub(super) stdout: ChildStdout,
    pub(super) stderr: ChildStderr,
}

impl Process {
    pub(super) fn spawn(
        binary: &Path,
        args: &[OsString],
        dir: Option<&Arc<WorkingDir>>,
    ) -> io::Result<Self> {
        let mut command = Command::new(binary);
        command
            .args(args)
            .process_group(0)
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = dir {
            command.current_dir(dir.path());
        }
        let mut child = command.spawn()?;
        let id = child.id().expect("new child has a process ID") as libc::pid_t;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        Ok(Self {
            group: Group {
                child: Some(child),
                id,
                dir: dir.cloned(),
            },
            stdin,
            stdout,
            stderr,
        })
    }
}

impl Group {
    pub(super) async fn wait(&mut self) -> io::Result<ExitStatus> {
        let child = self.child.as_mut().expect("CLI child is live");
        loop {
            if leader_exited(self.id)? {
                kill_group(self.id);
                return child.wait().await;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    pub(super) fn stop(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        let mut child = self.child.take()?;
        let id = self.id;
        let dir = self.dir.take();
        if child.id().is_some() {
            kill_group(id);
        }
        spawn_cleanup(move || {
            let deadline = std::time::Instant::now() + Duration::from_millis(250);
            loop {
                let _ = child.try_wait();
                // SAFETY: signal zero only probes the process group.
                if unsafe { libc::killpg(id, 0) } == -1
                    && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    break;
                }
                // Some init processes leave terminated orphans as zombies.
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            drop(dir);
        })
    }
}

fn leader_exited(id: libc::pid_t) -> io::Result<bool> {
    // Observe exit without reaping so the leader still reserves the group ID.
    // SAFETY: info is a valid output buffer and id is our child's PID.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::waitid(
            libc::P_PID,
            id as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    } == -1
    {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(error);
    }
    Ok(info.si_signo != 0)
}

fn kill_group(id: libc::pid_t) {
    // SAFETY: id is the positive process group ID of an unreaped child.
    unsafe { libc::killpg(id, libc::SIGKILL) };
}
