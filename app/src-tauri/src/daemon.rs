//! Finds the running `chauffeurd`, or starts it, and hands the UI its WebSocket address and token.
//!
//! The daemon writes `daemon.json` (its port) and `token` in its state folder, `~/.chauffeur`
//! or `$CHAUFFEUR_HOME`. The UI then talks to the daemon directly over WebSocket: nothing but the
//! connection details goes through Tauri.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

/// What the UI needs to open `ws://127.0.0.1:<port>/ws?token=<token>`.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Connection {
    pub url: String,
    pub token: String,
}

#[derive(Deserialize)]
struct DaemonInfo {
    port: u16,
    #[serde(default)]
    pid: Option<i32>,
    #[serde(default)]
    version: Option<String>,
    /// Absent from daemons before 0.2.0's rename: their version stands in for it.
    #[serde(default)]
    build: Option<String>,
}

/// This build, as the daemon's (see chauffeurd::BUILD): `scripts/package.sh` sets it for both.
const BUILD: &str = match option_env!("CHAUFFEUR_BUILD") {
    Some(build) => build,
    None => env!("CARGO_PKG_VERSION"),
};

/// The daemon's state folder, resolved like the daemon does.
pub fn chauffeur_home() -> PathBuf {
    match std::env::var_os("CHAUFFEUR_HOME") {
        Some(home) if !home.is_empty() => PathBuf::from(home),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".chauffeur"),
    }
}

/// Reads `daemon.json` and `token`; `None` when either is missing or unreadable.
pub fn read_connection(home: &Path) -> Option<(u16, Connection)> {
    let info: DaemonInfo =
        serde_json::from_str(&std::fs::read_to_string(home.join("daemon.json")).ok()?).ok()?;
    let token = std::fs::read_to_string(home.join("token")).ok()?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return None;
    }
    Some((
        info.port,
        Connection {
            url: format!("ws://127.0.0.1:{}/ws", info.port),
            token,
        },
    ))
}

/// Whether a chauffeurd answers `/health` on its Unix socket. The socket lives in the state
/// folder, which only the user can write, so an answer proves the daemon itself is up, and that
/// `daemon.json` is its own: a stale one may name a port some other program now holds, and the
/// token must never be sent there.
pub fn healthy(home: &Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(home.join("chauffeurd.sock")) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(400)));
    if stream
        .write_all(b"GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut response = String::new();
    let _ = stream.take(4096).read_to_string(&mut response);
    response.starts_with("HTTP/1.") && response.contains(" 200 ") && response.ends_with("ok")
}

/// `$CHAUFFEURD`, else `chauffeurd` next to this executable (bundled app, or `target/debug` in
/// development since the shell is a member of the Cargo workspace), else the repo's
/// `target/debug/chauffeurd` in debug builds, else the installed `~/.chauffeur/bin/chauffeurd`,
/// else `chauffeurd` on the PATH.
pub fn daemon_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("CHAUFFEURD").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(sibling) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("chauffeurd")))
        .filter(|path| path.is_file())
    {
        return sibling;
    }
    #[cfg(debug_assertions)]
    {
        let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/chauffeurd");
        if dev.is_file() {
            return dev;
        }
    }
    // Where scripts/install.sh puts it: an app opened from the Finder has no user PATH.
    let installed = chauffeur_home().join("bin").join("chauffeurd");
    if installed.is_file() {
        return installed;
    }
    PathBuf::from("chauffeurd")
}

/// Asks the running daemon to stop through the bundled CLI, and waits until it no longer answers.
/// Its chats resume in the new daemon. The CLI speaks the protocol of this build, which a daemon
/// of an older build may refuse (the handshake fails): that daemon is then stopped by signal.
fn stop_daemon(bin_dir: &Path, home: &Path) {
    let asked = Command::new(bin_dir.join("chauffeur"))
        .args(["daemon", "stop"])
        .env("CHAUFFEUR_HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !asked && healthy(home) {
        signal_daemon(home);
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while healthy(home) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The pid `daemon.json` names, when that process is a chauffeurd.
fn daemon_pid(home: &Path) -> Option<i32> {
    let info: DaemonInfo =
        serde_json::from_str(&std::fs::read_to_string(home.join("daemon.json")).ok()?).ok()?;
    let pid = info.pid.filter(|pid| *pid > 1)?;
    let command = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&command.stdout)
        .contains("chauffeurd")
        .then_some(pid)
}

/// SIGTERM, the daemon's graceful stop: it closes its sessions and the sidecar before it exits.
fn signal_daemon(home: &Path) {
    if let Some(pid) = daemon_pid(home) {
        // SAFETY: kill(2) on the pid of a process checked to be this user's chauffeurd.
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
}

/// Starts the daemon detached (its own process group, output appended to
/// `logs/chauffeurd.log`), so that closing the app never stops it.
fn spawn_daemon(home: &Path) -> anyhow::Result<PathBuf> {
    use std::os::unix::process::CommandExt;
    let logs = home.join("logs");
    std::fs::create_dir_all(&logs).with_context(|| format!("creating {}", logs.display()))?;
    let log_path = logs.join("chauffeurd.log");
    // Agents' output lands in the log: the user's alone, like the rest of the state folder.
    let log = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&log_path)
            .with_context(|| format!("opening {}", log_path.display()))?
    };
    let binary = daemon_binary();
    let mut child = Command::new(&binary)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .with_context(|| format!("starting {}", binary.display()))?;
    // Reap it if it exits while the app runs (e.g. another daemon already holds the lock).
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(log_path)
}

/// The version the running daemon wrote in `daemon.json`.
fn running_build(home: &Path) -> Option<String> {
    let info: DaemonInfo =
        serde_json::from_str(&std::fs::read_to_string(home.join("daemon.json")).ok()?).ok()?;
    info.build.or(info.version)
}

/// The running daemon's connection, starting the daemon first when none answers. A packaged app
/// replaces a daemon of another build: the app was updated while the old daemon kept running.
pub fn ensure_daemon(home: &Path, timeout: Duration) -> anyhow::Result<Connection> {
    if healthy(home)
        && let Some((_, connection)) = read_connection(home)
    {
        let bundled = crate::bundle::bundled_bin_dir();
        let replace = crate::bundle::must_replace_daemon(
            running_build(home).as_deref(),
            BUILD,
            bundled.is_some(),
        );
        match bundled {
            Some(bin_dir) if replace => stop_daemon(&bin_dir, home),
            _ => return Ok(connection),
        }
    }
    let log = spawn_daemon(home)?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        if healthy(home)
            && let Some((_, connection)) = read_connection(home)
        {
            return Ok(connection);
        }
    }
    bail!(
        "chauffeurd did not start within {} s; see {}",
        timeout.as_secs(),
        log.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_daemon_files() {
        let dir = std::env::temp_dir().join(format!("chauffeur-app-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_connection(&dir).is_none());
        std::fs::write(
            dir.join("daemon.json"),
            r#"{"pid": 1, "port": 47999, "socket": "/x"}"#,
        )
        .unwrap();
        assert!(read_connection(&dir).is_none(), "no token yet");
        std::fs::write(dir.join("token"), "abc\n").unwrap();
        let (port, connection) = read_connection(&dir).unwrap();
        assert_eq!(port, 47999);
        assert_eq!(
            connection,
            Connection {
                url: "ws://127.0.0.1:47999/ws".into(),
                token: "abc".into()
            }
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_daemon_without_a_build_id_is_known_by_its_version() {
        let dir = std::env::temp_dir().join(format!("chauffeur-app-build-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let info = |json: &str| {
            std::fs::write(dir.join("daemon.json"), json).unwrap();
            running_build(&dir)
        };
        // An older 0.2.0 does not match a packaged 0.2.0 build: it is replaced.
        let old = info(r#"{"port": 1, "version": "0.2.0"}"#);
        assert_eq!(old.as_deref(), Some("0.2.0"));
        assert!(crate::bundle::must_replace_daemon(
            old.as_deref(),
            "0.2.0+b0f0d8a.20261014",
            true
        ));
        let same = info(r#"{"port": 1, "version": "0.2.0", "build": "0.2.0+b0f0d8a.20261014"}"#);
        assert!(!crate::bundle::must_replace_daemon(
            same.as_deref(),
            "0.2.0+b0f0d8a.20261014",
            true
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_a_chauffeurd_is_signalled() {
        let dir = std::env::temp_dir().join(format!("chauffeur-app-pid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |pid: &str| {
            std::fs::write(
                dir.join("daemon.json"),
                format!(r#"{{"pid": {pid}, "port": 1}}"#),
            )
            .unwrap()
        };
        write("1");
        assert_eq!(daemon_pid(&dir), None, "launchd is never signalled");
        write(&std::process::id().to_string());
        assert_eq!(daemon_pid(&dir), None, "another program is not the daemon");
        std::fs::write(dir.join("daemon.json"), r#"{"port": 1}"#).unwrap();
        assert_eq!(daemon_pid(&dir), None, "no pid");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_or_dead_socket_is_not_healthy() {
        let dir = std::env::temp_dir().join(format!("chauffeur-app-health-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!healthy(&dir), "no socket");
        // Bind then drop: a socket file nobody listens on.
        drop(std::os::unix::net::UnixListener::bind(dir.join("chauffeurd.sock")).unwrap());
        assert!(!healthy(&dir), "dead socket");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_program_holding_the_port_is_not_the_daemon() {
        // A stale daemon.json names a port someone else answers on: the token must not go there.
        let dir = std::env::temp_dir().join(format!("chauffeur-app-squat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.0 200 OK\r\n\r\nok");
            }
        });
        std::fs::write(
            dir.join("daemon.json"),
            format!(r#"{{"pid": 1, "port": {port}}}"#),
        )
        .unwrap();
        std::fs::write(dir.join("token"), "secret").unwrap();
        assert!(read_connection(&dir).is_some());
        assert!(!healthy(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
