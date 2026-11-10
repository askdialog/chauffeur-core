//! Running the daemon at login. The daemon writes (or removes) its LaunchAgent or systemd user
//! service, as the app's Settings do; `install` then hands the running daemon over to launchd or
//! systemd at once, `uninstall` stops it there.

use std::path::PathBuf;

use chauffeur_protocol::client::{AppSettings, ClientCall, EmptyParams, SettingsUpdate};

use crate::client::{Client, Home};

/// Asks the daemon of this version (a running one is stopped first: it may be older) to write or
/// remove its login file.
async fn start_at_login(home: &Home, on: bool) -> anyhow::Result<()> {
    stop_daemon(home).await;
    let mut client = Client::connect(true).await?;
    let _: AppSettings = client
        .call(ClientCall::SettingsSet(Box::new(SettingsUpdate {
            start_at_login: Some(on),
            ..Default::default()
        })))
        .await?;
    Ok(())
}

/// Stops the running daemon and waits for its process to end (it stops the sidecar last, still
/// holding the lock), so that a start right after cannot race it. A daemon of another version
/// refuses this CLI's hello: it gets SIGTERM, from the pid its `daemon.json` gives. Whether one
/// was running.
pub async fn stop_daemon(home: &Home) -> bool {
    let pid = daemon_pid(home);
    match Client::connect(false).await {
        Ok(mut client) => {
            let _ = client
                .call_value(ClientCall::DaemonShutdown(EmptyParams {}))
                .await;
        }
        Err(_) => match pid.filter(|&pid| home.socket().exists() && is_chauffeurd(pid)) {
            // SAFETY: kill only sends a signal.
            Some(pid) => unsafe {
                libc::kill(pid, libc::SIGTERM);
            },
            None => return false,
        },
    }
    for _ in 0..150 {
        let alive = match pid {
            // SAFETY: signal 0 only checks that the process exists.
            Some(pid) => unsafe { libc::kill(pid, 0) == 0 },
            None => home.socket().exists(),
        };
        if !alive {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    true
}

fn daemon_pid(home: &Home) -> Option<i32> {
    let text = std::fs::read_to_string(home.home.join("daemon.json")).ok()?;
    let info: serde_json::Value = serde_json::from_str(&text).ok()?;
    i32::try_from(info["pid"].as_i64()?).ok()
}

/// Whether `pid` is a chauffeurd, not a process that took the pid of one that ended.
fn is_chauffeurd(pid: i32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .is_ok_and(|out| {
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .ends_with("chauffeurd")
        })
}

/// Where the daemon wrote the file: the user's own launchd or systemd folder, or the state folder
/// of a daemon with another `CHAUFFEUR_HOME` (see chauffeurd's `login`).
fn login_file(home: &Home, system: PathBuf, name: &str) -> PathBuf {
    let user_state = PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".chauffeur");
    if home.home == user_state {
        system.join(name)
    } else {
        home.home.join("login").join(name)
    }
}

#[cfg(target_os = "macos")]
const LABEL: &str = "com.askdialog.chauffeurd";

#[cfg(target_os = "macos")]
fn domain() -> String {
    // SAFETY: getuid cannot fail.
    format!("gui/{}", unsafe { libc::getuid() })
}

#[cfg(target_os = "macos")]
pub async fn install(home: &Home) -> anyhow::Result<()> {
    start_at_login(home, true).await?;
    stop_daemon(home).await;
    let system =
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/LaunchAgents");
    let plist = login_file(home, system, &format!("{LABEL}.plist"));
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &format!("{}/{LABEL}", domain())])
        .stderr(std::process::Stdio::null())
        .status();
    let status = std::process::Command::new("launchctl")
        .args(["bootstrap", &domain()])
        .arg(&plist)
        .status()?;
    anyhow::ensure!(
        status.success(),
        "launchctl bootstrap failed for {}",
        plist.display()
    );
    println!(
        "chauffeurd runs as the LaunchAgent {LABEL} ({})",
        plist.display()
    );
    Ok(())
}

#[cfg(target_os = "macos")]
pub async fn uninstall(home: &Home) -> anyhow::Result<()> {
    start_at_login(home, false).await?;
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &format!("{}/{LABEL}", domain())])
        .stderr(std::process::Stdio::null())
        .status();
    println!("LaunchAgent removed");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
const UNIT: &str = "chauffeurd.service";

#[cfg(not(target_os = "macos"))]
pub async fn install(home: &Home) -> anyhow::Result<()> {
    start_at_login(home, true).await?;
    stop_daemon(home).await;
    let output = std::process::Command::new("systemctl")
        .args(["--user", "start", UNIT])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "systemctl --user start {UNIT} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let system = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
        .join("systemd/user");
    println!(
        "chauffeurd runs as the systemd user service {UNIT} ({}); `systemctl --user status chauffeurd` shows it",
        login_file(home, system, UNIT).display()
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub async fn uninstall(home: &Home) -> anyhow::Result<()> {
    start_at_login(home, false).await?;
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "stop", UNIT])
        .output();
    println!("systemd user service removed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pid_taken_by_another_process_is_not_stopped() {
        let mut other = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = i32::try_from(other.id()).unwrap();
        assert!(!is_chauffeurd(pid), "sleep was taken for a chauffeurd");
        assert!(
            !is_chauffeurd(i32::MAX),
            "a pid nothing runs as was taken for a chauffeurd"
        );
        other.kill().unwrap();
        other.wait().unwrap();
    }

    #[test]
    fn the_pid_is_read_from_daemon_json() {
        let dir = std::env::temp_dir().join(format!("chauffeur-pid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let home = Home { home: dir.clone() };
        assert_eq!(daemon_pid(&home), None);
        std::fs::write(
            dir.join("daemon.json"),
            "{\n  \"build\": \"0.28.0\",\n  \"pid\": 38090,\n  \"version\": \"0.28.0\"\n}",
        )
        .unwrap();
        assert_eq!(daemon_pid(&home), Some(38090));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
