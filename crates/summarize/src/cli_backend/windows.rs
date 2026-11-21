use super::*;
use std::fs::File;
use std::mem::{size_of, size_of_val};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::ExitStatusExt;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::*;

pub(super) fn resolve(name: &OsStr) -> io::Result<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        for extension in [".exe", ".cmd"] {
            let mut file = name.to_owned();
            file.push(extension);
            let candidate = dir.join(file);
            if candidate.is_file() {
                return std::path::absolute(candidate);
            }
        }
    }
    Err(io::ErrorKind::NotFound.into())
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let value: Vec<_> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(value)
}

fn system_cmd() -> io::Result<PathBuf> {
    let mut directory = [0; 261];
    // SAFETY: directory is writable for the supplied length.
    let len = unsafe { GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) };
    if len == 0 || len as usize >= directory.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(OsString::from_wide(&directory[..len as usize])).join("cmd.exe"))
}

fn command_line(binary: &Path, args: &[OsString]) -> io::Result<(PathBuf, Vec<u16>)> {
    let batch = binary
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd"));
    let binary_wide = wide(binary.as_os_str())?;
    let (application, mut line) = if batch {
        if binary_wide.iter().any(|&c| matches!(c, 34 | 37 | 94)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported character in batch path",
            ));
        }
        let mut line: Vec<_> = "cmd.exe /e:on /v:off /d /s /c \"\""
            .encode_utf16()
            .collect();
        line.extend(binary_wide);
        line.push(b'"'.into());
        (system_cmd()?, line)
    } else {
        let mut line = Vec::new();
        quote_arg(&mut line, &binary_wide, false)?;
        (binary.to_owned(), line)
    };
    for arg in args {
        line.push(b' '.into());
        quote_arg(&mut line, &wide(arg)?, batch)?;
    }
    if batch {
        line.push(b'"'.into());
    }
    line.push(0);
    Ok((application, line))
}

// Match Windows CRT backslash quoting for executables. For batch shims use
// cmd.exe's doubled quotes and prevent percent expansion, as Rust's Command does.
fn quote_arg(line: &mut Vec<u16>, arg: &[u16], batch: bool) -> io::Result<()> {
    let mut backslashes = 0;
    line.push(b'"'.into());
    for &unit in arg {
        match unit {
            92 => {
                backslashes += 1;
                continue;
            }
            10 | 13 if batch => return Err(io::ErrorKind::InvalidInput.into()),
            _ => {}
        }
        let slash_count = if unit == 34 {
            2 * backslashes
        } else {
            backslashes
        };
        line.extend(std::iter::repeat_n(u16::from(b'\\'), slash_count));
        backslashes = 0;
        match unit {
            34 => line.push(if batch { b'"' } else { b'\\' }.into()),
            37 if batch => line.extend("%%cd:~,".encode_utf16()),
            _ => {}
        }
        line.push(unit);
    }
    line.extend(std::iter::repeat_n(u16::from(b'\\'), 2 * backslashes));
    line.push(b'"'.into());
    Ok(())
}

fn check(value: i32) -> io::Result<()> {
    if value == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn pipe(parent_reads: bool) -> io::Result<(tokio::fs::File, OwnedHandle)> {
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        bInheritHandle: 1,
        ..Default::default()
    };
    let mut reader = null_mut();
    let mut writer = null_mut();
    // SAFETY: both output pointers and the security attributes are valid.
    unsafe {
        check(CreatePipe(&mut reader, &mut writer, &security, 0))?;
        let reader = OwnedHandle::from_raw_handle(reader);
        let writer = OwnedHandle::from_raw_handle(writer);
        let (parent, child) = if parent_reads {
            (reader, writer)
        } else {
            (writer, reader)
        };
        check(SetHandleInformation(
            parent.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            0,
        ))?;
        Ok((tokio::fs::File::from_std(File::from(parent)), child))
    }
}

struct Attributes {
    // A word-aligned allocation for the opaque Windows attribute list.
    _storage: Vec<usize>,
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
}

impl Attributes {
    fn new(handles: &mut [HANDLE; 3]) -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: Windows supplies the required size on the first call. Storage
        // remains allocated for the initialized list and handles outlive it.
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
            let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
            let list = storage.as_mut_ptr().cast();
            check(InitializeProcThreadAttributeList(list, 1, 0, &mut bytes))?;
            let attributes = Self {
                _storage: storage,
                list,
            };
            check(UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_mut_ptr().cast(),
                size_of_val(handles),
                null_mut(),
                null_mut(),
            ))?;
            Ok(attributes)
        }
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: list was initialized and its backing allocation is still live.
        unsafe { DeleteProcThreadAttributeList(self.list) };
    }
}

pub(super) struct Group {
    job: OwnedHandle,
    process: OwnedHandle,
}

impl Group {
    pub(super) async fn wait(&mut self) -> io::Result<ExitStatus> {
        loop {
            // SAFETY: process is a live owned handle; the zero timeout never blocks.
            match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    // SAFETY: code is writable and the process handle is live.
                    unsafe { check(GetExitCodeProcess(self.process.as_raw_handle(), &mut code))? };
                    return Ok(ExitStatus::from_raw(code));
                }
                WAIT_TIMEOUT => tokio::time::sleep(Duration::from_millis(10)).await,
                _ => return Err(io::Error::last_os_error()),
            }
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        // SAFETY: job and process are live. Kill-on-close also covers errors.
        unsafe {
            if TerminateJobObject(self.job.as_raw_handle(), 1) == 0 {
                return;
            }
            let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            while QueryInformationJobObject(
                self.job.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of_val(&accounting) as u32,
                null_mut(),
            ) != 0
                && accounting.ActiveProcesses != 0
            {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

pub(super) struct Process {
    pub(super) group: Group,
    pub(super) stdin: Option<tokio::fs::File>,
    pub(super) stdout: tokio::fs::File,
    pub(super) stderr: tokio::fs::File,
}

impl Process {
    pub(super) fn spawn(binary: &Path, args: &[OsString], dir: Option<&Path>) -> io::Result<Self> {
        let (application, mut command) = command_line(binary, args)?;
        let mut application = wide(application.as_os_str())?;
        application.push(0);
        let directory = dir
            .map(|dir| {
                let mut path = wide(dir.as_os_str())?;
                path.push(0);
                Ok::<_, io::Error>(path)
            })
            .transpose()?;
        let (stdin, child_stdin) = pipe(false)?;
        let (stdout, child_stdout) = pipe(true)?;
        let (stderr, child_stderr) = pipe(true)?;
        let mut handles = [
            child_stdin.as_raw_handle(),
            child_stdout.as_raw_handle(),
            child_stderr.as_raw_handle(),
        ];
        let attributes = Attributes::new(&mut handles)?;
        let startup = STARTUPINFOEXW {
            StartupInfo: STARTUPINFOW {
                cb: size_of::<STARTUPINFOEXW>() as u32,
                dwFlags: STARTF_USESTDHANDLES,
                hStdInput: handles[0],
                hStdOutput: handles[1],
                hStdError: handles[2],
                ..Default::default()
            },
            lpAttributeList: attributes.list,
        };
        // SAFETY: all pointers passed to Win32 refer to initialized, live
        // buffers. The child stays suspended until the job owns it.
        let group = unsafe {
            let job = CreateJobObjectW(null(), null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = OwnedHandle::from_raw_handle(job);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            check(SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of_val(&limits) as u32,
            ))?;
            let mut info = PROCESS_INFORMATION::default();
            check(CreateProcessW(
                application.as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_SUSPENDED | CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                null(),
                directory.as_ref().map_or(null(), |path| path.as_ptr()),
                &startup.StartupInfo,
                &mut info,
            ))?;
            let process = OwnedHandle::from_raw_handle(info.hProcess);
            let thread = OwnedHandle::from_raw_handle(info.hThread);
            if let Err(error) = check(AssignProcessToJobObject(
                job.as_raw_handle(),
                process.as_raw_handle(),
            )) {
                TerminateProcess(process.as_raw_handle(), 1);
                WaitForSingleObject(process.as_raw_handle(), INFINITE);
                return Err(error);
            }
            let group = Group { job, process };
            if ResumeThread(thread.as_raw_handle()) == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            group
        };
        Ok(Self {
            group,
            stdin: Some(stdin),
            stdout,
            stderr,
        })
    }
}
