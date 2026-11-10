//! Bounded Docker CLI calls against one explicitly selected, inode-pinned local socket.
use std::{
    collections::BTreeMap,
    io::Seek,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStderr, ChildStdout, Command},
};

use crate::{Error, Result, shellenv::ShellEnv};
use sha2::{Digest, Sha256};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StreamedOutput {
    pub bytes: u64,
    pub sha256: String,
}

pub(crate) struct DockerCommand {
    binary: PathBuf,
    vars: BTreeMap<String, String>,
    socket: PathBuf,
    stamp: (u64, u64),
}
fn unavailable() -> Error {
    Error::Unavailable("The selected Docker service could not be verified or stopped; retain the workspace fence and inspect its local Docker runtime".into())
}
fn socket_stamp(path: &Path) -> Result<(u64, u64)> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| unavailable())?;
    // SAFETY: geteuid has no arguments or preconditions.
    if !metadata.file_type().is_socket()
        || ![0, unsafe { libc::geteuid() }].contains(&metadata.uid())
    {
        return Err(unavailable());
    }
    Ok((metadata.dev(), metadata.ino()))
}
impl DockerCommand {
    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket
    }
    pub(crate) fn new(env: &ShellEnv, socket: &Path) -> Result<Self> {
        if !socket.is_absolute() {
            return Err(unavailable());
        }
        let selected = socket_stamp(socket)?;
        let socket = socket
            .parent()
            .ok_or_else(unavailable)?
            .canonicalize()
            .map_err(|_| unavailable())?
            .join(socket.file_name().ok_or_else(unavailable)?);
        let stamp = socket_stamp(&socket)?;
        if selected != stamp {
            return Err(unavailable());
        }
        let mut vars = env.vars();
        for key in [
            "DOCKER_HOST",
            "DOCKER_CONTEXT",
            "DOCKER_TLS_VERIFY",
            "DOCKER_CERT_PATH",
            "DOCKER_TLS",
        ] {
            vars.remove(key);
        }
        let binary = std::env::split_paths(vars.get("PATH").ok_or_else(unavailable)?)
            .filter(|path| path.is_absolute())
            .map(|path| path.join("docker"))
            .find(|path| {
                std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0)
            })
            .and_then(|path| path.canonicalize().ok())
            .ok_or_else(unavailable)?;
        Ok(Self {
            binary,
            vars,
            socket,
            stamp,
        })
    }
    fn check_socket(&self) -> Result<()> {
        if socket_stamp(&self.socket)? != self.stamp {
            return Err(unavailable());
        }
        Ok(())
    }
    fn spawn(
        &self,
        args: &[&str],
        timeout: Duration,
        stdin: Stdio,
        stderr: Stdio,
    ) -> Result<OwnedGroup> {
        if timeout.is_zero()
            || timeout > Duration::from_secs(300)
            || args.iter().any(|arg| {
                matches!(*arg, "--host" | "-H" | "--context" | "-c")
                    || arg.starts_with("--host=")
                    || arg.starts_with("--context=")
                    || arg.starts_with("-H")
                    || arg.starts_with("-c")
            })
        {
            return Err(unavailable());
        }
        self.check_socket()?;
        let mut command = Command::new(&self.binary);
        // The pinned path is canonical; a multi-call binary behind a `docker` link (OrbStack's
        // docker-tools) picks its tool from argv[0].
        command.arg0("docker");
        let socket = self.socket.to_str().ok_or_else(unavailable)?;
        command
            .env_clear()
            .envs(&self.vars)
            .arg("--host")
            .arg(format!("unix://{socket}"))
            .args(args)
            .stdin(stdin)
            .stderr(stderr)
            .stdout(Stdio::piped())
            .process_group(0);
        let child = command.spawn().map_err(|_| unavailable())?;
        Ok(OwnedGroup {
            child,
            reserved: true,
        })
    }
    /// The execution deadline permits at most two additional seconds for final reaping after
    /// group termination. An unfinished reap refuses and retains Drop's termination obligation.
    pub(crate) async fn run(
        &self,
        args: &[&str],
        timeout: Duration,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        if max_bytes > 1024 * 1024 {
            return Err(unavailable());
        }
        let mut owned = self.spawn(args, timeout, Stdio::null(), Stdio::null())?;
        let stdout = owned.child.stdout.take().ok_or_else(unavailable)?;
        let output = tokio::time::timeout(timeout, async {
            let mut output = Vec::new();
            stdout
                .take((max_bytes + 1) as u64)
                .read_to_end(&mut output)
                .await
                .map_err(|_| unavailable())?;
            if output.len() > max_bytes {
                return Err(unavailable());
            }
            owned.wait_unreaped().await?;
            Ok(output)
        })
        .await
        .map_err(|_| unavailable())
        .and_then(|result| result);
        self.finish(&mut owned).await?;
        output
    }

    /// For a command whose own diagnostics say why it failed (`docker compose up`): its output is
    /// discarded, and a failure carries the last lines of its standard error.
    pub(crate) async fn run_reporting(
        &self,
        args: &[&str],
        timeout: Duration,
    ) -> std::result::Result<(), String> {
        let unanswered = || "Docker did not answer".to_owned();
        let mut owned = self
            .spawn(args, timeout, Stdio::null(), Stdio::piped())
            .map_err(|_| unanswered())?;
        let stdout = owned.child.stdout.take().ok_or_else(unanswered)?;
        let stderr = owned.child.stderr.take().ok_or_else(unanswered)?;
        // Both streams are drained to their end, so compose never blocks writing; only the last
        // 64 KiB of standard error are kept, for the message.
        let read = tokio::time::timeout(timeout, async {
            let mut stdout = stdout;
            let mut stderr = stderr;
            let drain = async {
                let mut buffer = [0u8; 8192];
                while stdout.read(&mut buffer).await.unwrap_or(0) != 0 {}
            };
            let tail = async {
                let mut kept: Vec<u8> = Vec::new();
                let mut buffer = [0u8; 8192];
                loop {
                    let n = stderr.read(&mut buffer).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    kept.extend_from_slice(&buffer[..n]);
                    if kept.len() > 64 * 1024 {
                        kept.drain(..kept.len() - 64 * 1024);
                    }
                }
                kept
            };
            let ((), diagnostics) = tokio::join!(drain, tail);
            owned.wait_unreaped().await?;
            Ok::<_, Error>(diagnostics)
        })
        .await;
        let finished = self.finish(&mut owned).await;
        match (read, finished) {
            (Ok(Ok(_)), Ok(())) => Ok(()),
            (Ok(Ok(diagnostics)), Err(_)) => {
                let text = String::from_utf8_lossy(&diagnostics);
                let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
                Err(lines[lines.len().saturating_sub(8)..].join("\n"))
            }
            _ => Err(format!("it did not finish within {} s", timeout.as_secs())),
        }
    }

    /// Bounded read-only query output with null stdin; any diagnostic byte refuses its result.
    /// The execution deadline permits at most two extra seconds for final process reaping.
    pub(crate) async fn run_checked(
        &self,
        args: &[&str],
        timeout: Duration,
        max_output: usize,
    ) -> Result<Vec<u8>> {
        if max_output > 1024 * 1024 {
            return Err(unavailable());
        }
        let mut owned = self.spawn(args, timeout, Stdio::null(), Stdio::piped())?;
        let stdout = owned.child.stdout.take().ok_or_else(unavailable)?;
        let stderr = owned.child.stderr.take().ok_or_else(unavailable)?;
        let result = tokio::time::timeout(timeout, async {
            let output = checked_output(stdout, stderr, max_output).await?;
            owned.wait_unreaped().await?;
            Ok(output)
        })
        .await
        .map_err(|_| unavailable())
        .and_then(|result| result);
        self.finish(&mut owned).await?;
        result
    }

    /// Streams into the caller's fresh regular file, retaining partial bytes on every refusal.
    /// Any stderr byte refuses rather than silently discarding a database export warning.
    /// The execution deadline includes writes/sync and permits at most two extra seconds to reap.
    pub(crate) async fn run_to_file(
        &self,
        args: &[&str],
        timeout: Duration,
        max_bytes: u64,
        mut output: std::fs::File,
    ) -> Result<StreamedOutput> {
        let metadata = output.metadata().map_err(|_| unavailable())?;
        if max_bytes > 16 * 1024 * 1024 * 1024
            || !metadata.is_file()
            || metadata.len() != 0
            || output.stream_position().map_err(|_| unavailable())? != 0
        {
            return Err(unavailable());
        }
        let mut owned = self.spawn(args, timeout, Stdio::null(), Stdio::piped())?;
        let mut stdout = owned.child.stdout.take().ok_or_else(unavailable)?;
        let mut stderr = owned.child.stderr.take().ok_or_else(unavailable)?;
        let mut output = tokio::fs::File::from_std(output);
        let result = tokio::time::timeout(timeout, async {
            let capture = async {
                let mut buffer = [0u8; 64 * 1024];
                let mut bytes = 0u64;
                let mut hash = Sha256::new();
                loop {
                    let length = stdout.read(&mut buffer).await.map_err(|_| unavailable())?;
                    if length == 0 {
                        break;
                    }
                    let next = bytes.checked_add(length as u64).ok_or_else(unavailable)?;
                    if next > max_bytes {
                        return Err(unavailable());
                    }
                    output
                        .write_all(&buffer[..length])
                        .await
                        .map_err(|_| unavailable())?;
                    hash.update(&buffer[..length]);
                    bytes = next;
                }
                output.flush().await.map_err(|_| unavailable())?;
                output.sync_all().await.map_err(|_| unavailable())?;
                Ok::<_, Error>(StreamedOutput {
                    bytes,
                    sha256: hex::encode(hash.finalize()),
                })
            };
            let warnings = async {
                // Refuse on the first byte, bounding even a hostile diagnostic stream well
                // below 64 KiB without waiting for EOF or allocating its private contents.
                let mut buffer = [0u8; 4096];
                if stderr.read(&mut buffer).await.map_err(|_| unavailable())? != 0 {
                    return Err(unavailable());
                }
                Ok::<_, Error>(())
            };
            let (captured, ()) = tokio::try_join!(capture, warnings)?;
            owned.wait_unreaped().await?;
            Ok(captured)
        })
        .await
        .map_err(|_| unavailable())
        .and_then(|result| result);
        self.finish(&mut owned).await?;
        result
    }

    /// Sends the exact referenced regular file through stdin without removing or modifying it.
    /// A failure may follow partial remote effects: callers retain quarantine and recovery data.
    /// The hash is checked while sending, so callers must authenticate and privately stage
    /// untrusted input before invoking a command which can execute its contents.
    /// The deadline includes all three pipes and permits at most two extra seconds for reaping.
    pub(crate) async fn run_with_input(
        &self,
        args: &[&str],
        timeout: Duration,
        mut input: std::fs::File,
        expected_bytes: u64,
        expected_sha256: &str,
        max_output: usize,
    ) -> Result<Vec<u8>> {
        let metadata = input.metadata().map_err(|_| unavailable())?;
        if expected_bytes == 0
            || expected_bytes > 16 * 1024 * 1024 * 1024
            || expected_sha256.len() != 64
            || !expected_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || max_output > 1024 * 1024
            || !metadata.is_file()
            || metadata.len() != expected_bytes
            || input.stream_position().map_err(|_| unavailable())? != 0
        {
            return Err(unavailable());
        }
        let stamp = input_stamp(&metadata);
        let mut owned = self.spawn(args, timeout, Stdio::piped(), Stdio::piped())?;
        let mut stdin = owned.child.stdin.take().ok_or_else(unavailable)?;
        let stdout = owned.child.stdout.take().ok_or_else(unavailable)?;
        let stderr = owned.child.stderr.take().ok_or_else(unavailable)?;
        let mut input = tokio::fs::File::from_std(input);
        let result = tokio::time::timeout(timeout, async {
            let pump = async {
                let mut buffer = [0u8; 64 * 1024];
                let mut hash = Sha256::new();
                let mut bytes = 0u64;
                loop {
                    let length = input.read(&mut buffer).await.map_err(|_| unavailable())?;
                    if length == 0 {
                        break;
                    }
                    let next = bytes.checked_add(length as u64).ok_or_else(unavailable)?;
                    if next > expected_bytes {
                        return Err(unavailable());
                    }
                    stdin
                        .write_all(&buffer[..length])
                        .await
                        .map_err(|_| unavailable())?;
                    hash.update(&buffer[..length]);
                    bytes = next;
                }
                if bytes != expected_bytes
                    || hex::encode(hash.finalize()) != expected_sha256
                    || input_stamp(&input.metadata().await.map_err(|_| unavailable())?) != stamp
                {
                    return Err(unavailable());
                }
                stdin.shutdown().await.map_err(|_| unavailable())?;
                drop(stdin);
                Ok::<_, Error>(())
            };
            let ((), output) = tokio::try_join!(pump, checked_output(stdout, stderr, max_output))?;
            owned.wait_unreaped().await?;
            if input_stamp(&input.metadata().await.map_err(|_| unavailable())?) != stamp {
                return Err(unavailable());
            }
            Ok(output)
        })
        .await
        .map_err(|_| unavailable())
        .and_then(|result| result);
        self.finish(&mut owned).await?;
        result
    }

    async fn finish(&self, owned: &mut OwnedGroup) -> Result<()> {
        // The leader remains unreaped even on success: its PID still reserves this process
        // group while plugin children (including those which closed stdout) are killed before
        // the final wait. A child which deliberately creates a new session escapes this group.
        owned.kill();
        let status = tokio::time::timeout(Duration::from_secs(2), owned.child.wait()).await;
        if owned.child.id().is_none()
            || matches!(&status, Ok(Ok(_)))
            || matches!(&status, Ok(Err(error)) if error.raw_os_error() == Some(libc::ECHILD))
        {
            owned.reserved = false;
        }
        let status = status
            .map_err(|_| unavailable())?
            .map_err(|_| unavailable())?;
        self.check_socket()?;
        if !status.success() {
            return Err(unavailable());
        }
        Ok(())
    }
}

async fn checked_output(
    stdout: ChildStdout,
    mut stderr: ChildStderr,
    max_output: usize,
) -> Result<Vec<u8>> {
    let capture = async {
        let mut output = Vec::new();
        stdout
            .take((max_output + 1) as u64)
            .read_to_end(&mut output)
            .await
            .map_err(|_| unavailable())?;
        if output.len() > max_output {
            return Err(unavailable());
        }
        Ok::<_, Error>(output)
    };
    let warnings = async {
        let mut buffer = [0u8; 4096];
        if stderr.read(&mut buffer).await.map_err(|_| unavailable())? != 0 {
            return Err(unavailable());
        }
        Ok::<_, Error>(())
    };
    let (output, ()) = tokio::try_join!(capture, warnings)?;
    Ok(output)
}

fn input_stamp(metadata: &std::fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

struct OwnedGroup {
    child: Child,
    reserved: bool,
}
impl OwnedGroup {
    fn kill(&self) {
        if self.reserved
            && let Some(pid) = self
                .child
                .id()
                .filter(|pid| *pid > 0 && *pid <= i32::MAX as u32)
        {
            // SAFETY: the dedicated group's leader is owned and has not been reaped; no
            // other task waits for it. Its PID cannot identify a historical/reused group.
            unsafe {
                libc::killpg(pid as i32, libc::SIGKILL);
            }
        }
    }
    async fn wait_unreaped(&mut self) -> Result<()> {
        let pid = self
            .child
            .id()
            .filter(|pid| *pid > 0 && *pid <= i32::MAX as u32)
            .ok_or_else(unavailable)?;
        loop {
            if self.exited_unreaped(pid)? {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    fn exited_unreaped(&mut self, pid: u32) -> Result<bool> {
        // SAFETY: siginfo_t is a C output structure; zero initialization also gives a
        // portable zero si_pid when WNOHANG observes no exited child.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: this is our owned child. WNOWAIT observes exit without releasing its
        // PID, so group cleanup can precede the one and only final reap.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                self.reserved = false;
            }
            if error.raw_os_error() != Some(libc::EINTR) {
                return Err(unavailable());
            }
        // SAFETY: waitid successfully initialized this output field.
        } else if unsafe { info.si_pid() } == pid as libc::pid_t {
            return Ok(true);
        }
        Ok(false)
    }
}
impl Drop for OwnedGroup {
    fn drop(&mut self) {
        self.kill();
        // Tokio retains a dropped child's reap obligation. Group termination happens first,
        // including when cancellation/panic drops the caller's future before it can await.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        os::unix::{
            fs::{OpenOptionsExt, PermissionsExt, symlink},
            net::UnixListener,
        },
        sync::Arc,
    };

    /// Runs a fake tool written a moment ago. `exec` fails with "Text file busy" while any process
    /// holds the file open for writing, and a test thread that forks while this one writes keeps a
    /// copy of the descriptor until its own `exec`: try again until that copy is gone.
    fn warm_up(binary: &Path) {
        for _ in 0..100 {
            match std::process::Command::new(binary).arg("--warm-up").status() {
                Ok(status) => return assert!(status.success()),
                Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("the fake tool did not start: {error}"),
            }
        }
        panic!("the fake tool stayed busy for two seconds");
    }

    #[test]
    fn a_fake_tool_still_open_for_writing_runs_once_it_is_closed() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("tool");
        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&binary)
            .unwrap();
        writer.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
        // Another thread's fork would hold the descriptor like this one, then drop it.
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(writer);
        });
        warm_up(&binary);
        release.join().unwrap();
    }

    struct Fixture {
        root: tempfile::TempDir,
        socket: PathBuf,
        _listener: UnixListener,
        env: ShellEnv,
    }
    impl Fixture {
        fn python(script: &str) -> Self {
            Self::with_binary(format!(
                "#!/usr/bin/env python3\nimport sys\nif sys.argv[1:]==['--warm-up']:sys.exit(0)\n{script}\n"
            ))
        }
        fn output(&self) -> (PathBuf, std::fs::File) {
            let path = self.root.path().join("export.bin");
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            (path, file)
        }
        fn input(&self, bytes: &[u8]) -> (PathBuf, std::fs::File, String) {
            let path = self.root.path().join("source.dump");
            std::fs::write(&path, bytes).unwrap();
            let file = std::fs::File::open(&path).unwrap();
            (path, file, hex::encode(Sha256::digest(bytes)))
        }
        fn new(script: &str) -> Self {
            Self::with_binary(format!(
                "#!/bin/sh\n[ \"$1\" = --warm-up ] && exit 0\n{script}\n"
            ))
        }
        fn with_binary(contents: String) -> Self {
            let root = tempfile::Builder::new()
                .prefix("docker runner ")
                .tempdir_in("/tmp")
                .unwrap();
            let socket = root.path().join("socket path.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            let binary = root.path().join("docker");
            std::fs::write(&binary, contents).unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
            // macOS checks a freshly written executable on its first exec, which can take
            // seconds: paid here, it stays out of the deadlines under test.
            warm_up(&binary);
            let env = ShellEnv::new();
            env.set_vars(BTreeMap::from([
                (
                    "PATH".into(),
                    format!("{}:/usr/bin:/bin", root.path().display()),
                ),
                ("HOME".into(), root.path().display().to_string()),
                ("PINNED_VALUE".into(), "original snapshot".into()),
                ("DOCKER_HOST".into(), "ambient-host".into()),
                ("DOCKER_CONTEXT".into(), "ambient-context".into()),
                ("DOCKER_TLS_VERIFY".into(), "ambient-tls-verify".into()),
                ("DOCKER_TLS".into(), "ambient-tls".into()),
                ("DOCKER_CERT_PATH".into(), "ambient-certificate".into()),
            ]));
            Self {
                root,
                socket,
                _listener: listener,
                env,
            }
        }
        fn runner(&self) -> DockerCommand {
            DockerCommand::new(&self.env, &self.socket).unwrap()
        }
        fn vars(&self) -> BTreeMap<String, String> {
            self.env.vars()
        }
    }
    fn generic(error: Error) {
        assert!(matches!(error, Error::Unavailable(_)));
        assert!(!error.to_string().contains("synthetic-private-output"));
    }
    /// How long a fixture may take to start or to be reaped on a loaded machine: shorter than
    /// the 60 s its processes sleep, so a process left running still fails the test.
    const PATIENCE: Duration = Duration::from_secs(20);
    async fn gone(pid: i32) -> bool {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        while tokio::time::Instant::now() < deadline {
            // SAFETY: signal zero observes existence only; it does not signal any process.
            if unsafe { libc::kill(pid, 0) } == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }
    async fn pid_file(path: &Path) -> i32 {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        while tokio::time::Instant::now() < deadline {
            if let Ok(value) = std::fs::read_to_string(path)
                && let Ok(pid) = value.trim().parse()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("fixture process did not publish its PID");
    }

    #[tokio::test]
    async fn selected_socket_and_environment_snapshot_override_all_ambient_docker_routing() {
        let f = Fixture::new(
            "printf '%s\\n' \"$@\"; printf '%s\\n' \"$PINNED_VALUE\"; printf '%s' \"${DOCKER_HOST-unset}|${DOCKER_CONTEXT-unset}|${DOCKER_TLS_VERIFY-unset}|${DOCKER_TLS-unset}|${DOCKER_CERT_PATH-unset}\"",
        );
        let runner = f.runner();
        let mut vars = f.vars();
        vars.insert("PINNED_VALUE".into(), "changed after selection".into());
        vars.insert("PATH".into(), "/missing".into());
        f.env.set_vars(vars);
        let output = runner
            .run(
                &["container", "inspect", "name with spaces"],
                Duration::from_secs(2),
                4096,
            )
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!(
                "--host\nunix://{}\ncontainer\ninspect\nname with spaces\noriginal snapshot\nunset|unset|unset|unset|unset",
                f.socket.canonicalize().unwrap().display()
            )
        );
    }

    #[tokio::test]
    async fn nonzero_missing_binary_output_limits_and_invalid_budgets_refuse_without_output_leaks()
    {
        let f = Fixture::new(
            "printf synthetic-private-output; printf synthetic-private-output >&2; exit 1",
        );
        generic(
            f.runner()
                .run(&["inspect"], Duration::from_secs(1), 4096)
                .await
                .unwrap_err(),
        );
        std::fs::write(
            f.root.path().join("docker"),
            "#!/bin/sh\nprintf synthetic-private-output\n",
        )
        .unwrap();
        generic(
            f.runner()
                .run(&[], Duration::from_secs(1), 3)
                .await
                .unwrap_err(),
        );
        let runner = f.runner();
        for (timeout, cap) in [
            (Duration::ZERO, 10),
            (Duration::from_secs(301), 10),
            (Duration::from_secs(1), 1024 * 1024 + 1),
        ] {
            generic(runner.run(&[], timeout, cap).await.unwrap_err());
        }
        for flag in [
            "--host=tcp://other",
            "-Htcp://other",
            "--context=other",
            "-cother",
        ] {
            generic(
                runner
                    .run(&[flag], Duration::from_secs(1), 10)
                    .await
                    .unwrap_err(),
            );
        }
        std::fs::remove_file(f.root.path().join("docker")).unwrap();
        generic(
            runner
                .run(&[], Duration::from_secs(1), 10)
                .await
                .unwrap_err(),
        );
    }

    #[tokio::test]
    async fn socket_replacement_before_or_during_a_command_refuses_the_result() {
        let f = Fixture::new("printf success");
        let runner = f.runner();
        std::fs::rename(&f.socket, f.root.path().join("retained.sock")).unwrap();
        let replacement = UnixListener::bind(&f.socket).unwrap();
        generic(
            runner
                .run(&[], Duration::from_secs(1), 100)
                .await
                .unwrap_err(),
        );
        drop(replacement);
        let f = Fixture::new("/bin/rm -- \"$SOCKET\"; printf success");
        let mut vars = f.vars();
        vars.insert("SOCKET".into(), f.socket.display().to_string());
        f.env.set_vars(vars);
        generic(
            f.runner()
                .run(&[], Duration::from_secs(1), 100)
                .await
                .unwrap_err(),
        );
    }

    #[test]
    fn missing_regular_and_symlink_socket_selections_never_start_docker() {
        let f = Fixture::new("exit 99");
        assert!(DockerCommand::new(&f.env, &f.root.path().join("missing.sock")).is_err());
        assert!(DockerCommand::new(&f.env, &f.root.path().join("docker")).is_err());
        let alias = f.root.path().join("alias.sock");
        symlink(&f.socket, &alias).unwrap();
        assert!(DockerCommand::new(&f.env, &alias).is_err());
        assert!(DockerCommand::new(&f.env, Path::new("relative.sock")).is_err());
        let mut vars = f.vars();
        vars.insert(
            "PATH".into(),
            f.root.path().join("absent-directory").display().to_string(),
        );
        f.env.set_vars(vars);
        assert!(DockerCommand::new(&f.env, &f.socket).is_err());
    }

    #[tokio::test]
    async fn timeout_kills_and_reaps_the_wrapper_and_its_plugin_group() {
        let f = Fixture::new(
            "echo $$ > \"$HOME/parent.pid\"; sleep 60 & echo $! > \"$HOME/plugin.pid\"; wait",
        );
        generic(
            f.runner()
                .run(&[], Duration::from_secs(2), 100)
                .await
                .unwrap_err(),
        );
        let parent = pid_file(&f.root.path().join("parent.pid")).await;
        let plugin = pid_file(&f.root.path().join("plugin.pid")).await;
        assert!(gone(parent).await);
        assert!(gone(plugin).await);
    }

    #[tokio::test]
    async fn normal_wrapper_exit_does_not_leave_a_plugin_with_closed_stdout() {
        let f = Fixture::new(
            "sleep 60 >/dev/null 2>&1 & echo $! > \"$HOME/plugin.pid\"; printf success",
        );
        assert_eq!(
            f.runner()
                .run(&[], Duration::from_secs(2), 100)
                .await
                .unwrap(),
            b"success"
        );
        let plugin = pid_file(&f.root.path().join("plugin.pid")).await;
        assert!(gone(plugin).await);
    }

    #[tokio::test]
    async fn caller_cancellation_kills_the_owned_wrapper_group_before_reaping_it() {
        let f = Fixture::new(
            "echo $$ > \"$HOME/parent.pid\"; sleep 60 & echo $! > \"$HOME/plugin.pid\"; wait",
        );
        let runner = Arc::new(f.runner());
        let task = tokio::spawn(async move { runner.run(&[], Duration::from_secs(60), 100).await });
        let parent = pid_file(&f.root.path().join("parent.pid")).await;
        let plugin = pid_file(&f.root.path().join("plugin.pid")).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(gone(parent).await);
        assert!(gone(plugin).await);
    }

    #[tokio::test]
    async fn streamed_binary_stdout_is_hashed_and_synced_without_a_path_based_output_capability() {
        let f = Fixture::python("import sys\nsys.stdout.buffer.write(bytes(range(256))*8193)");
        let (path, file) = f.output();
        let result = f
            .runner()
            .run_to_file(
                &["exec", "synthetic-container", "pg_dump"],
                Duration::from_secs(5),
                3 * 1024 * 1024,
                file,
            )
            .await
            .unwrap();
        let expected = (0..8193).flat_map(|_| 0..=255u8).collect::<Vec<_>>();
        assert_eq!(std::fs::read(&path).unwrap(), expected);
        assert_eq!(result.bytes, expected.len() as u64);
        assert_eq!(result.sha256, hex::encode(Sha256::digest(&expected)));
    }

    #[tokio::test]
    async fn streamed_output_never_writes_a_chunk_which_would_exceed_the_limit() {
        let f = Fixture::python("import sys\nsys.stdout.buffer.write(b'x'*262144)");
        let (path, file) = f.output();
        generic(
            f.runner()
                .run_to_file(&[], Duration::from_secs(5), 70_000, file)
                .await
                .unwrap_err(),
        );
        let partial = std::fs::read(path).unwrap();
        assert!(partial.len() <= 70_000);
        assert!(partial.iter().all(|byte| *byte == b'x'));
    }

    #[tokio::test]
    async fn stderr_warnings_and_nonzero_exit_never_return_a_usable_export_or_private_diagnostics()
    {
        for script in [
            "import sys\nsys.stdout.buffer.write(b'partial export');sys.stdout.flush()\nsys.stderr.write('synthetic-private-output')",
            "import sys\nsys.stderr.write('synthetic-private-output'*65536)\nsys.stderr.flush()\nsys.stdout.buffer.write(b'x'*262144)",
            "import sys\nsys.stdout.buffer.write(b'partial export');sys.exit(1)",
        ] {
            let f = Fixture::python(script);
            let (path, file) = f.output();
            generic(
                f.runner()
                    .run_to_file(&[], Duration::from_secs(5), 1024 * 1024, file)
                    .await
                    .unwrap_err(),
            );
            assert!(path.is_file());
            assert!(std::fs::metadata(path).unwrap().len() <= 1024 * 1024);
        }
    }

    #[tokio::test]
    async fn streaming_refuses_preexisting_data_invalid_budgets_and_read_only_output_handles() {
        let f = Fixture::python("import sys\nsys.stdout.buffer.write(b'new export')");
        let (path, mut file) = f.output();
        std::io::Write::write_all(&mut file, b"retained source").unwrap();
        generic(
            f.runner()
                .run_to_file(&[], Duration::from_secs(5), 1024, file)
                .await
                .unwrap_err(),
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"retained source");
        let empty = f.root.path().join("readonly.bin");
        std::fs::write(&empty, []).unwrap();
        generic(
            f.runner()
                .run_to_file(
                    &[],
                    Duration::from_secs(5),
                    1024,
                    std::fs::File::open(&empty).unwrap(),
                )
                .await
                .unwrap_err(),
        );
        for (index, (timeout, limit)) in [
            (Duration::ZERO, 1024),
            (Duration::from_secs(301), 1024),
            (Duration::from_secs(5), 16 * 1024 * 1024 * 1024 + 1),
        ]
        .into_iter()
        .enumerate()
        {
            let path = f.root.path().join(format!("invalid-{index}.bin"));
            generic(
                f.runner()
                    .run_to_file(&[], timeout, limit, std::fs::File::create(&path).unwrap())
                    .await
                    .unwrap_err(),
            );
            assert_eq!(std::fs::metadata(path).unwrap().len(), 0);
        }
    }

    fn waiting_export() -> Fixture {
        Fixture::python(
            "import os,pathlib,subprocess,sys,time\nroot=pathlib.Path(os.environ['HOME'])\n(root/'parent.pid').write_text(str(os.getpid()))\nchild=subprocess.Popen(['sleep','60'])\n(root/'plugin.pid').write_text(str(child.pid))\nsys.stdout.buffer.write(b'partial export');sys.stdout.flush()\ntime.sleep(60)",
        )
    }

    #[tokio::test]
    async fn streaming_timeout_retains_the_partial_file_and_kills_its_wrapper_and_descendants() {
        let f = waiting_export();
        let (path, file) = f.output();
        generic(
            f.runner()
                .run_to_file(&[], Duration::from_secs(2), 1024, file)
                .await
                .unwrap_err(),
        );
        assert!(path.is_file());
        assert!(gone(pid_file(&f.root.path().join("parent.pid")).await).await);
        assert!(gone(pid_file(&f.root.path().join("plugin.pid")).await).await);
    }

    #[tokio::test]
    async fn streaming_cancellation_retains_the_file_and_terminates_the_owned_group() {
        let f = waiting_export();
        let (path, file) = f.output();
        let runner = Arc::new(f.runner());
        let task = tokio::spawn(async move {
            runner
                .run_to_file(&[], Duration::from_secs(60), 1024, file)
                .await
        });
        let parent = pid_file(&f.root.path().join("parent.pid")).await;
        let plugin = pid_file(&f.root.path().join("plugin.pid")).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(path.is_file());
        assert!(gone(parent).await);
        assert!(gone(plugin).await);
    }

    #[tokio::test]
    async fn referenced_binary_input_streams_concurrently_with_bounded_stdout() {
        let f = Fixture::python(
            "import hashlib,sys\nsys.stdout.buffer.write(b'x'*131072);sys.stdout.flush()\nh=hashlib.sha256();n=0\nwhile True:\n data=sys.stdin.buffer.read(65536)\n if not data:break\n h.update(data);n+=len(data)\nsys.stdout.write(str(n)+':'+h.hexdigest())",
        );
        let bytes = (0..8193).flat_map(|_| 0..=255u8).collect::<Vec<_>>();
        let (path, file, hash) = f.input(&bytes);
        let output = f
            .runner()
            .run_with_input(
                &["exec", "-i", "synthetic-container", "pg_restore"],
                Duration::from_secs(5),
                file,
                bytes.len() as u64,
                &hash,
                200_000,
            )
            .await
            .unwrap();
        assert_eq!(&output[..131072], vec![b'x'; 131072]);
        assert_eq!(
            &output[131072..],
            format!("{}:{hash}", bytes.len()).as_bytes()
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[tokio::test]
    async fn strict_queries_refuse_warnings_without_changing_ordinary_run_behavior() {
        let f = Fixture::python(
            "import sys\nassert sys.stdin.buffer.read()==b''\nsys.stdout.write('query result');sys.stderr.write('synthetic-private-output')",
        );
        generic(
            f.runner()
                .run_checked(&[], Duration::from_secs(5), 1024)
                .await
                .unwrap_err(),
        );
        assert_eq!(
            f.runner()
                .run(&[], Duration::from_secs(5), 1024)
                .await
                .unwrap(),
            b"query result"
        );
        let f = Fixture::python(
            "import sys\nassert sys.stdin.buffer.read()==b''\nsys.stdout.write('query result')",
        );
        assert_eq!(
            f.runner()
                .run_checked(&[], Duration::from_secs(5), 1024)
                .await
                .unwrap(),
            b"query result"
        );
    }

    #[tokio::test]
    async fn input_bounds_hash_mismatch_and_nonzero_offsets_never_produce_success() {
        for kind in [
            "empty",
            "short",
            "long",
            "oversized",
            "invalid-hash",
            "wrong-hash",
            "offset",
            "write-only",
        ] {
            let f =
                Fixture::python("import sys\nsys.stdin.buffer.read();sys.stdout.write('accepted')");
            let (path, mut file, mut hash) = f.input(b"synthetic-private-input");
            let mut size = std::fs::metadata(&path).unwrap().len();
            match kind {
                "empty" => size = 0,
                "short" => size += 1,
                "long" => size -= 1,
                "oversized" => size = 16 * 1024 * 1024 * 1024 + 1,
                "invalid-hash" => hash = "A".repeat(64),
                "wrong-hash" => hash = "0".repeat(64),
                "offset" => {
                    file.seek(std::io::SeekFrom::Start(1)).unwrap();
                }
                _ => file = std::fs::OpenOptions::new().write(true).open(&path).unwrap(),
            }
            generic(
                f.runner()
                    .run_with_input(&[], Duration::from_secs(5), file, size, &hash, 100)
                    .await
                    .unwrap_err(),
            );
            assert_eq!(std::fs::read(path).unwrap(), b"synthetic-private-input");
        }
    }

    #[tokio::test]
    async fn input_changes_while_the_child_reads_cannot_be_accepted_as_the_selected_dump() {
        for edit in ["truncate", "append", "rewrite"] {
            let action = match edit {
                "truncate" => "path.write_bytes(b'changed')",
                "append" => "with path.open('ab') as output:output.write(b'changed')",
                _ => "with path.open('r+b') as output:output.write(b'changed')",
            };
            let script = format!(
                "import os,pathlib,sys\nsys.stdin.buffer.read(65536)\npath=pathlib.Path(os.environ['HOME'])/'source.dump'\n{action}\nwhile sys.stdin.buffer.read(65536):pass"
            );
            let f = Fixture::python(&script);
            let bytes = vec![b'a'; 2 * 1024 * 1024];
            let (path, file, hash) = f.input(&bytes);
            generic(
                f.runner()
                    .run_with_input(
                        &[],
                        Duration::from_secs(5),
                        file,
                        bytes.len() as u64,
                        &hash,
                        100,
                    )
                    .await
                    .unwrap_err(),
            );
            assert!(path.is_file());
        }
    }

    #[tokio::test]
    async fn input_delivery_refuses_stderr_stdout_flood_nonzero_exit_and_early_closed_stdin() {
        for script in [
            "import sys\nsys.stderr.write('synthetic-private-output'*65536);sys.stderr.flush()\nsys.stdin.buffer.read()",
            "import sys\nsys.stdout.buffer.write(b'x'*262144);sys.stdout.flush()\nsys.stdin.buffer.read()",
            "import sys\nsys.stdin.buffer.read();sys.exit(1)",
            "import sys\nsys.exit(0)",
        ] {
            let f = Fixture::python(script);
            let bytes = vec![b'a'; 2 * 1024 * 1024];
            let (path, file, hash) = f.input(&bytes);
            generic(
                f.runner()
                    .run_with_input(
                        &[],
                        Duration::from_secs(5),
                        file,
                        bytes.len() as u64,
                        &hash,
                        1024,
                    )
                    .await
                    .unwrap_err(),
            );
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }

    fn waiting_input() -> Fixture {
        Fixture::python(
            "import os,pathlib,subprocess,time\nroot=pathlib.Path(os.environ['HOME'])\n(root/'parent.pid').write_text(str(os.getpid()))\nchild=subprocess.Popen(['sleep','60'])\n(root/'plugin.pid').write_text(str(child.pid))\ntime.sleep(60)",
        )
    }

    #[tokio::test]
    async fn input_timeout_retains_the_source_and_kills_the_owned_process_group() {
        let f = waiting_input();
        let bytes = vec![b'a'; 2 * 1024 * 1024];
        let (path, file, hash) = f.input(&bytes);
        generic(
            f.runner()
                .run_with_input(
                    &[],
                    Duration::from_secs(2),
                    file,
                    bytes.len() as u64,
                    &hash,
                    100,
                )
                .await
                .unwrap_err(),
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert!(gone(pid_file(&f.root.path().join("parent.pid")).await).await);
        assert!(gone(pid_file(&f.root.path().join("plugin.pid")).await).await);
    }

    #[tokio::test]
    async fn input_cancellation_retains_the_source_and_terminates_its_wrapper_and_plugin() {
        let f = waiting_input();
        let bytes = vec![b'a'; 2 * 1024 * 1024];
        let (path, file, hash) = f.input(&bytes);
        let runner = Arc::new(f.runner());
        let task = tokio::spawn(async move {
            runner
                .run_with_input(
                    &[],
                    Duration::from_secs(60),
                    file,
                    2 * 1024 * 1024,
                    &hash,
                    100,
                )
                .await
        });
        let parent = pid_file(&f.root.path().join("parent.pid")).await;
        let plugin = pid_file(&f.root.path().join("plugin.pid")).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert!(gone(parent).await);
        assert!(gone(plugin).await);
    }
}
