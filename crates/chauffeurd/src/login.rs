//! Starting the daemon at login: a LaunchAgent on macOS, a systemd user service on Linux. Either
//! way it restarts after a crash, not after a clean `chauffeur daemon stop` (exit 0), and stopping
//! it stops the daemon only: the dev servers and terminals it started keep running.
//!
//! The daemon writes or removes the file, which takes effect at the next login; `chauffeur daemon
//! install` also hands the running daemon over to launchd or systemd at once. A daemon whose state
//! is not `~/.chauffeur` (tests) keeps the file in its own folder and tells neither.

use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::paths::Paths;

#[cfg(target_os = "macos")]
pub const LABEL: &str = "com.askdialog.chauffeurd";
#[cfg(not(target_os = "macos"))]
pub const UNIT: &str = "chauffeurd.service";

pub struct Login {
    file: PathBuf,
    /// The user's own launchd or systemd, rather than a test folder.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    system: bool,
    daemon: PathBuf,
    /// Set when the state is not `~/.chauffeur`: the started daemon must find it.
    home: Option<PathBuf>,
    log: PathBuf,
}

fn user_home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

impl Login {
    pub fn for_paths(paths: &Paths) -> Login {
        let system = paths.home == user_home().join(".chauffeur");
        let file = if system {
            system_file()
        } else {
            paths.home.join("login").join(file_name())
        };
        Login {
            file,
            system,
            daemon: std::env::current_exe()
                .and_then(|exe| exe.canonicalize())
                .unwrap_or_else(|_| PathBuf::from("chauffeurd")),
            home: (!system).then(|| paths.home.clone()),
            log: paths.logs_dir.join("chauffeurd.log"),
        }
    }

    /// The LaunchAgent's plist or the systemd unit.
    pub fn file(&self) -> &Path {
        &self.file
    }

    pub fn enabled(&self) -> bool {
        self.file.exists()
    }

    /// From the next login on.
    pub fn enable(&self) -> anyhow::Result<()> {
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::write(&self.file, self.contents())
            .with_context(|| format!("writing {}", self.file.display()))?;
        #[cfg(not(target_os = "macos"))]
        if self.system {
            systemctl(&["daemon-reload"])?;
            systemctl(&["enable", UNIT])?;
        }
        Ok(())
    }

    /// The running daemon goes on; nothing starts it at the next login.
    pub fn disable(&self) -> anyhow::Result<()> {
        #[cfg(not(target_os = "macos"))]
        if self.system {
            let _ = systemctl(&["disable", UNIT]);
        }
        match std::fs::remove_file(&self.file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error).with_context(|| format!("removing {}", self.file.display()));
            }
            _ => {}
        }
        #[cfg(not(target_os = "macos"))]
        if self.system {
            let _ = systemctl(&["daemon-reload"]);
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn contents(&self) -> String {
        let escape = |s: &str| {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        };
        let log = escape(&self.log.to_string_lossy());
        let env = self
            .home
            .as_ref()
            .map(|home| {
                format!(
                    "<key>EnvironmentVariables</key><dict><key>CHAUFFEUR_HOME</key><string>{}</string></dict>",
                    escape(&home.to_string_lossy())
                )
            })
            .unwrap_or_default();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{}</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ProcessType</key><string>Interactive</string>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
  {env}
</dict>
</plist>
"#,
            escape(&self.daemon.to_string_lossy())
        )
    }

    #[cfg(not(target_os = "macos"))]
    fn contents(&self) -> String {
        let log = self.log.to_string_lossy().replace('%', "%%");
        let env = self
            .home
            .as_ref()
            .map(|home| {
                format!(
                    "Environment={}\n",
                    quoted(&format!("CHAUFFEUR_HOME={}", home.to_string_lossy()))
                )
            })
            .unwrap_or_default();
        format!(
            "[Unit]
Description=Chauffeur daemon: multi-repo workspaces for coding agents

[Service]
ExecStart={}
{env}Restart=on-failure
# Stop the daemon only: the dev servers and terminals it started keep running, as on macOS.
KillMode=process
StandardOutput=append:{log}
StandardError=append:{log}

[Install]
WantedBy=default.target
",
            quoted(&self.daemon.to_string_lossy())
        )
    }
}

#[cfg(target_os = "macos")]
fn file_name() -> String {
    format!("{LABEL}.plist")
}

#[cfg(not(target_os = "macos"))]
fn file_name() -> String {
    UNIT.to_string()
}

#[cfg(target_os = "macos")]
fn system_file() -> PathBuf {
    user_home().join("Library/LaunchAgents").join(file_name())
}

#[cfg(not(target_os = "macos"))]
fn system_file() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| user_home().join(".config"))
        .join("systemd/user")
        .join(file_name())
}

#[cfg(not(target_os = "macos"))]
fn systemctl(args: &[&str]) -> anyhow::Result<()> {
    let output = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "systemctl --user {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// A value for systemd: `%` starts a specifier, and quotes keep spaces in one argument.
#[cfg(not(target_os = "macos"))]
fn quoted(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(home: &Path, daemon: &str) -> Login {
        let paths = Paths::new(home.join("home"), home.join("ws"), home.join("repos"));
        Login {
            daemon: PathBuf::from(daemon),
            ..Login::for_paths(&paths)
        }
    }

    #[test]
    fn a_test_daemon_keeps_its_login_file_to_itself() {
        let dir = tempfile::tempdir().unwrap();
        let login = login(dir.path(), "/opt/chauffeurd");
        assert!(login.file().starts_with(dir.path()));
        assert!(!login.enabled());
        login.enable().unwrap();
        assert!(login.enabled());
        login.disable().unwrap();
        login.disable().unwrap();
        assert!(!login.enabled());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_launch_agent_restarts_only_after_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let plist = login(dir.path(), "/opt/c&c/chauffeurd").contents();
        assert!(plist.contains("<string>/opt/c&amp;c/chauffeurd</string>"));
        assert!(plist.contains("<key>SuccessfulExit</key><false/>"));
        assert!(plist.contains("home/logs/chauffeurd.log"));
        assert!(plist.contains("<key>CHAUFFEUR_HOME</key>"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_service_restarts_only_after_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let unit = login(dir.path(), "/opt/my apps/100%/chauffeurd").contents();
        assert!(
            unit.contains("ExecStart=\"/opt/my apps/100%%/chauffeurd\"\n"),
            "{unit}"
        );
        assert!(unit.contains("Restart=on-failure\n"));
        assert!(unit.contains("KillMode=process\n"));
        assert!(unit.contains("home/logs/chauffeurd.log\n"));
        assert!(unit.contains("WantedBy=default.target"));
    }
}
