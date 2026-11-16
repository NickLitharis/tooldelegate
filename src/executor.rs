//! Linux-only bubblewrap boundary. Missing isolation is an error, never a fallback.
use crate::filesystem_task::snapshot;
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
pub const ALLOWLIST: &[&str] = &["python3", "sort", "mv", "mkdir", "cat", "sed"];

#[derive(Clone)]
pub struct Limits {
    pub wall: Duration,
    pub cpu_seconds: u64,
    pub memory_bytes: u64,
    pub file_bytes: u64,
    pub tree_bytes: u64,
    pub tree_entries: u64,
    pub processes: u64,
    pub open_files: u64,
    pub feedback_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            wall: Duration::from_secs(60),
            cpu_seconds: 30,
            memory_bytes: 512 * 1024 * 1024,
            file_bytes: 128 * 1024 * 1024,
            tree_bytes: 128 * 1024 * 1024,
            tree_entries: 4096,
            processes: 128,
            open_files: 64,
            feedback_bytes: 4096,
        }
    }
}

pub struct Execution {
    pub status: Option<i32>,
    pub timed_out: bool,
    pub resource_limited: bool,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
    pub before: BTreeMap<String, String>,
    pub after: BTreeMap<String, String>,
    pub after_error: Option<String>,
}

pub(crate) fn drain(mut source: impl Read, cap: usize) -> std::io::Result<(u64, Vec<u8>)> {
    let mut count = 0;
    let mut saved = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let n = source.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        saved.extend_from_slice(&buffer[..n.min(cap.saturating_sub(saved.len()))]);
    }
    Ok((count, saved))
}

fn process_filter() -> Result<fs::File> {
    use std::io::{Seek, SeekFrom};
    use std::os::fd::FromRawFd;
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err("sandbox requires Linux x86_64".into());
    }
    let statement = |code, k| libc::sock_filter {
        code,
        jt: 0,
        jf: 0,
        k,
    };
    let jump = |k| libc::sock_filter {
        code: 0x15,
        jt: 0,
        jf: 1,
        k,
    };
    let mut filters = vec![
        statement(0x20, 4),
        libc::sock_filter {
            code: 0x15,
            jt: 1,
            jf: 0,
            k: 0xc000003e,
        },
        statement(0x06, 0x80000000),
        statement(0x20, 0),
        libc::sock_filter {
            code: 0x35,
            jt: 0,
            jf: 1,
            k: 0x40000000,
        },
        statement(0x06, 0x80000000),
    ];
    // Native x86_64 syscall numbers; deny alternate ABIs above before interpreting them.
    for syscall in [56, 57, 58, 435] {
        filters.push(jump(syscall));
        filters.push(statement(0x06, 0x00050000 | libc::EPERM as u32));
    }
    filters.push(statement(0x06, 0x7fff0000));
    let fd = unsafe { libc::memfd_create(c"benchmark-seccomp".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut file = unsafe { fs::File::from_raw_fd(fd) };
    for filter in filters {
        file.write_all(&filter.code.to_ne_bytes())?;
        file.write_all(&[filter.jt, filter.jf])?;
        file.write_all(&filter.k.to_ne_bytes())?;
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

fn command(work: &Path, args: &[String], limits: &Limits, filter_fd: i32) -> Command {
    let mut command = Command::new("/usr/bin/bwrap");
    command.args([
        "--unshare-all",
        "--unshare-user",
        "--disable-userns",
        "--assert-userns-disabled",
        "--die-with-parent",
        "--new-session",
        "--cap-drop",
        "ALL",
    ]);
    for path in ["/usr/bin", "/usr/lib", "/lib", "/lib64"] {
        command.args(["--ro-bind", path, path]);
    }
    command.args([
        "--proc", "/proc", "--dev", "/dev", "--size", "16777216", "--tmpfs", "/tmp", "--bind",
    ]);
    command.arg(work).arg("/work");
    command.arg("--seccomp").arg(filter_fd.to_string());
    command.args([
        "--chdir",
        "/work",
        "--clearenv",
        "--setenv",
        "PATH",
        "/usr/bin",
        "--setenv",
        "LANG",
        "C.UTF-8",
        "--setenv",
        "HOME",
        "/tmp",
        "--",
    ]);
    command.arg("/usr/bin/prlimit").args([
        format!("--cpu={0}:{0}", limits.cpu_seconds),
        format!("--as={0}:{0}", limits.memory_bytes),
        format!("--fsize={0}:{0}", limits.file_bytes),
        format!("--nproc={0}:{0}", limits.processes),
        format!("--nofile={0}:{0}", limits.open_files),
        "--core=0:0".into(),
        "--".into(),
    ]);
    command
        .arg(format!("/usr/bin/{}", args[0]))
        .args(&args[1..]);
    command
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    unsafe {
        command.pre_exec(move || {
            // Only the seccomp descriptor may survive exec beyond standard I/O.
            if libc::syscall(
                libc::SYS_close_range,
                3u32,
                u32::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            ) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(filter_fd, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}

pub(crate) fn tree_budget(root: &Path, limits: &Limits) -> Result<()> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    };
    fn visit(
        dir: &fs::File,
        bytes: &mut u64,
        entries: &mut u64,
        limits: &Limits,
        depth: u64,
    ) -> Result<()> {
        if depth > 64 {
            return Err("workspace nesting limit exceeded".into());
        }
        // Anchor every lookup to an open directory; a rename/symlink race cannot escape.
        for entry in fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd()))? {
            let entry = entry?;
            *entries += 1;
            if *entries > limits.tree_entries {
                return Err("workspace entry limit exceeded".into());
            }
            let name = std::ffi::CString::new(entry.file_name().as_bytes())?;
            let fd = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::NotFound {
                    continue;
                }
                return Err(error.into());
            }
            let file = unsafe { fs::File::from_raw_fd(fd) };
            let metadata = file.metadata()?;
            if metadata.is_dir() {
                visit(&file, bytes, entries, limits, depth + 1)?;
            } else if metadata.is_file() {
                *bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or("workspace size overflow")?;
                if *bytes > limits.tree_bytes {
                    return Err("workspace byte limit exceeded".into());
                }
            } else {
                return Err("workspace contains links or special files".into());
            }
        }
        Ok(())
    }
    visit(&fs::File::open(root)?, &mut 0, &mut 0, limits, 0)
}

pub fn execute(work: &Path, args: &[String], limits: &Limits) -> Result<Execution> {
    if args.is_empty() || !ALLOWLIST.contains(&args[0].as_str()) {
        return Err("unauthorized command".into());
    }
    if limits.wall.is_zero()
        || [
            limits.cpu_seconds,
            limits.memory_bytes,
            limits.file_bytes,
            limits.tree_bytes,
            limits.tree_entries,
            limits.processes,
            limits.open_files,
        ]
        .contains(&0)
        || limits.feedback_bytes > 1_048_576
    {
        return Err("invalid execution limits".into());
    }
    let work = fs::canonicalize(work)?;
    tree_budget(&work, limits)?;
    let before = snapshot(&work)?;
    let start = Instant::now();
    use std::os::fd::AsRawFd;
    let filter = process_filter()?;
    let mut child = command(&work, args, limits, filter.as_raw_fd()).spawn()?;
    let pid = child.id();
    let stdout = child.stdout.take().ok_or("missing stdout")?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;
    let cap = limits.feedback_bytes;
    let out_thread = thread::spawn(move || drain(stdout, cap));
    let err_thread = thread::spawn(move || drain(stderr, cap));
    let mut timed_out = false;
    let mut resource_limited = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let exhausted = tree_budget(&work, limits).is_err();
        if start.elapsed() >= limits.wall || exhausted {
            timed_out = start.elapsed() >= limits.wall;
            resource_limited = exhausted;
            // Kill the monitor's group; bubblewrap tears down its PID namespace too.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            child.kill().ok();
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(5));
    };
    let (stdout_bytes, stdout) = out_thread.join().map_err(|_| "stdout reader panicked")??;
    let (stderr_bytes, stderr) = err_thread.join().map_err(|_| "stderr reader panicked")??;
    let post = tree_budget(&work, limits).and_then(|()| snapshot(&work));
    resource_limited |= post.is_err();
    let (after, after_error) = match post {
        Ok(tree) => (tree, None),
        Err(error) => (BTreeMap::new(), Some(error.to_string())),
    };
    Ok(Execution {
        status: status.code(),
        timed_out,
        resource_limited,
        stdout_bytes,
        stderr_bytes,
        stdout,
        stderr,
        elapsed: start.elapsed(),
        before,
        after,
        after_error,
    })
}

/// Save host-only traces exclusively, outside the model's filesystem and context.
pub fn save_trace(run: &Execution, path: &Path) -> Result<()> {
    use serde_json::json;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(&serde_json::to_vec_pretty(&json!({"status":run.status,"timed_out":run.timed_out,"resource_limited":run.resource_limited,"stdout_bytes":run.stdout_bytes,"stderr_bytes":run.stderr_bytes,"stdout_bounded":String::from_utf8_lossy(&run.stdout),"stderr_bounded":String::from_utf8_lossy(&run.stderr),"stdout_truncated":run.stdout_bytes>run.stdout.len() as u64,"stderr_truncated":run.stderr_bytes>run.stderr.len() as u64,"elapsed_seconds":run.elapsed.as_secs_f64(),"before":run.before,"after":run.after,"after_error":run.after_error}))?)?;
    Ok(())
}

pub fn fresh_work(task: &Path, destination: &Path) -> Result<PathBuf> {
    crate::suite::copy_tree(&task.join("input"), destination)?;
    Ok(fs::canonicalize(destination)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "sandbox-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        path
    }
    fn python(root: &Path, code: &str, limits: &Limits) -> Execution {
        execute(
            root,
            &["python3".into(), "-I".into(), "-c".into(), code.into()],
            limits,
        )
        .unwrap()
    }
    #[test]
    fn isolation_and_output_accounting() {
        let root = root();
        let work = root.join("work");
        fs::create_dir(&work).unwrap();
        fs::write(root.join("host-secret"), "hidden").unwrap();
        let limits = Limits {
            feedback_bytes: 64,
            ..Limits::default()
        };
        let code = format!(
            "import os, socket\nassert not os.path.exists({:?})\nassert not os.path.exists('/home')\nassert 'SECRET' not in os.environ\ntry:\n open('/usr/bin/forbidden','w')\n raise AssertionError('writable runtime')\nexcept OSError: pass\ns=socket.socket(); s.settimeout(.1)\nassert s.connect_ex(('1.1.1.1',443)) != 0\nopen('local','w').write('ok')\nprint('x'*10000)",
            root.join("host-secret").to_str().unwrap()
        );
        let run = python(&work, &code, &limits);
        assert_eq!(
            run.status,
            Some(0),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(run.stdout_bytes, 10001);
        assert_eq!(run.stdout.len(), 64);
        assert!(run.after.contains_key("local"));
        assert!(run.before.is_empty());
        assert!(execute(&work, &["sh".into(), "-c".into(), "true".into()], &limits).is_err());
        save_trace(&run, &root.join("trace.json")).unwrap();
        assert!(save_trace(&run, &root.join("trace.json")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn timeout_and_limits_are_hard() {
        let root = root();
        let mut limits = Limits {
            wall: Duration::from_millis(250),
            ..Limits::default()
        };
        let run = python(
            &root,
            "import time\ntime.sleep(5)\nopen('escaped-child','w').write('bad')",
            &limits,
        );
        assert!(run.timed_out);
        assert!(run.elapsed < Duration::from_secs(3));
        thread::sleep(Duration::from_millis(1100));
        assert!(!root.join("escaped-child").exists());
        limits.wall = Duration::from_secs(5);
        limits.file_bytes = 1024;
        let run = python(&root, "open('large','wb').write(b'x'*8192)", &limits);
        assert_ne!(run.status, Some(0));
        assert!(fs::metadata(root.join("large")).unwrap().len() <= 1024);
        let run = python(
            &root,
            "import resource\nassert resource.getrlimit(resource.RLIMIT_AS)==(536870912,536870912)\nassert resource.getrlimit(resource.RLIMIT_NPROC)==(128,128)\nassert resource.getrlimit(resource.RLIMIT_NOFILE)==(64,64)\nassert resource.getrlimit(resource.RLIMIT_CPU)==(30,30)\ntry:\n x=bytearray(1024*1024*1024)\n raise AssertionError('memory unlimited')\nexcept MemoryError: pass",
            &limits,
        );
        assert_eq!(
            run.status,
            Some(0),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        limits.cpu_seconds = 1;
        assert_ne!(python(&root, "while True: pass", &limits).status, Some(0));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn namespace_identity_links_and_workspace_budget() {
        let root = root();
        let host_net = fs::read_link("/proc/self/ns/net").unwrap();
        let code = format!(
            "import os\nassert os.readlink('/proc/self/ns/net') != {:?}\nassert not os.path.exists('/proc/1/root/home')\nprint('ISOLATED',flush=True)\nos.symlink('/etc/passwd','link')",
            host_net.to_str().unwrap()
        );
        let run = python(&root, &code, &Limits::default());
        assert_eq!(run.stdout, b"ISOLATED\n");
        assert!(run.after_error.is_some());
        assert!(run.resource_limited);
        assert!(execute(&root, &["cat".into(), "link".into()], &Limits::default()).is_err());
        fs::remove_file(root.join("link")).unwrap();
        let limits = Limits {
            tree_bytes: 1024,
            tree_entries: 16,
            ..Limits::default()
        };
        let run = python(&root, "open('oversize','wb').write(b'x'*8192)", &limits);
        assert!(run.resource_limited);
        assert!(run.after_error.is_some());
        fs::remove_file(root.join("oversize")).unwrap();
        let run = python(
            &root,
            "import os,errno,threading\ntry:\n os.fork()\n raise AssertionError('fork permitted')\nexcept OSError as e: assert e.errno==errno.EPERM\ntry:\n threading.Thread(target=lambda:None).start()\n raise AssertionError('thread permitted')\nexcept RuntimeError: pass",
            &Limits::default(),
        );
        assert_eq!(
            run.status,
            Some(0),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }
}
