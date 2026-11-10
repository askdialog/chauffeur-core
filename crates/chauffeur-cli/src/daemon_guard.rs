//! Keeps an agent from stopping the user's real daemon by mistake. From inside a Chauffeur
//! workspace or an agent's session, the daemon commands that stop or replace the daemon refuse to
//! act on the default state folder (`~/.chauffeur`): it is the daemon the agent's own chats run on.
//! A throwaway daemon, any other `CHAUFFEUR_HOME`, is stopped as before; the user overrides the
//! refusal with a flag the refusal never names (see `daemon`'s `--real-daemon` in main.rs).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Variables that say where a command runs: in a Chauffeur workspace (its terminals, scripts and
/// agents get `CHAUFFEUR_WORKSPACE_ID` and `CHAUFFEUR_WORKSPACE_PATH`), or in an agent's session
/// (Claude Code sets `CLAUDECODE`, Codex `CODEX_THREAD_ID`, both and others `AI_AGENT`), even one
/// that cleared the workspace's variables to start a throwaway daemon.
const INSIDE: &[&str] = &[
    "CHAUFFEUR_WORKSPACE_ID",
    "CHAUFFEUR_WORKSPACE_PATH",
    "CLAUDECODE",
    "CODEX_THREAD_ID",
    "AI_AGENT",
];

/// A daemon command that stops the running daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopping {
    Stop,
    /// `daemon install` stops it to hand it over to launchd or systemd.
    Install,
    /// `daemon uninstall` stops it to remove its login file.
    Uninstall,
}

impl Stopping {
    /// What the command would do to the daemon, and how.
    fn what(self) -> (&'static str, &'static str) {
        match self {
            Stopping::Stop => ("stop", ""),
            Stopping::Install => ("restart", " (`chauffeur daemon install` stops it first)"),
            Stopping::Uninstall => ("stop", " (`chauffeur daemon uninstall` stops it first)"),
        }
    }
}

/// The first variable that says the command runs inside a workspace or an agent's session.
pub fn inside_marker(var: impl Fn(&str) -> Option<OsString>) -> Option<&'static str> {
    INSIDE
        .iter()
        .copied()
        .find(|name| var(name).is_some_and(|value| !value.is_empty()))
}

/// The state folders a daemon of this user runs on by default: `$HOME/.chauffeur`, and the one
/// under the home folder of the user's account, should `HOME` point elsewhere.
pub fn default_homes() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
        homes.push(PathBuf::from(home).join(".chauffeur"));
    }
    if let Some(home) = passwd_home() {
        homes.push(home.join(".chauffeur"));
    }
    homes
}

/// Refuses, with the reason, a command that would stop the default daemon from inside a workspace
/// or an agent's session. `home` is the state folder the command acts on.
pub fn check(
    stopping: Stopping,
    home: &Path,
    defaults: &[PathBuf],
    var: impl Fn(&str) -> Option<OsString>,
) -> anyhow::Result<()> {
    let Some(marker) = inside_marker(var) else {
        return Ok(());
    };
    if !defaults.iter().any(|default| same_folder(home, default)) {
        return Ok(());
    }
    let (verb, how) = stopping.what();
    anyhow::bail!(
        "refusing to {verb} the Chauffeur daemon of {home}{how}: it is the default daemon, the one \
         your own chats run on, and this command runs inside a Chauffeur workspace or an agent's \
         session ({marker} is set). Stopping it would stop your chats and every other running \
         session.\nTo stop a throwaway daemon you started, set CHAUFFEUR_HOME to its temp dir: \
         CHAUFFEUR_HOME=<temp dir> chauffeur daemon stop",
        home = home.display(),
    )
}

/// The message of `daemon start` refused inside a workspace: the daemon did not answer, and this
/// workspace's chats run on it, so none is started in its place.
pub fn start_refusal(home: &Path) -> String {
    format!(
        "refusing to start a daemon from inside a Chauffeur workspace (CHAUFFEUR_WORKSPACE_ID is \
         set): the daemon of {home}, the one this workspace's chats run on, did not answer here, \
         and none is started in its place from a workspace.\nTo start a throwaway daemon, unset \
         CHAUFFEUR_WORKSPACE_ID and set CHAUFFEUR_HOME to its temp dir.",
        home = home.display(),
    )
}

/// The same folder, through symlinks (`/tmp` and `/private/tmp` on macOS) when both exist.
fn same_folder(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn passwd_home() -> Option<PathBuf> {
    // SAFETY: getpwuid returns a pointer into static storage, or null; copied out at once.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        (!entry.is_null() && !(*entry).pw_dir.is_null()).then(|| {
            PathBuf::from(
                std::ffi::CStr::from_ptr((*entry).pw_dir)
                    .to_string_lossy()
                    .into_owned(),
            )
        })
    }
    .filter(|home| !home.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "chauffeur-guard-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stopping_the_default_daemon_from_a_workspace_is_refused_with_why() {
        let user = temp_dir("refused");
        let default = user.join(".chauffeur");
        std::fs::create_dir_all(&default).unwrap();
        let error = check(
            Stopping::Stop,
            &default,
            std::slice::from_ref(&default),
            env(&[("CHAUFFEUR_WORKSPACE_ID", "01a")]),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("refusing to stop"), "{error}");
        assert!(error.contains("CHAUFFEUR_WORKSPACE_ID is set"), "{error}");
        assert!(error.contains("your own chats run on"), "{error}");
        assert!(
            error.contains("CHAUFFEUR_HOME=<temp dir> chauffeur daemon stop"),
            "{error}"
        );
        // Never the way around it: an agent would take it.
        assert!(!error.contains("--"), "{error}");
        std::fs::remove_dir_all(&user).unwrap();
    }

    #[test]
    fn an_agent_session_counts_as_inside_without_the_workspace_variables() {
        let default = PathBuf::from("/nonexistent/u/.chauffeur");
        for marker in [
            "CLAUDECODE",
            "CODEX_THREAD_ID",
            "AI_AGENT",
            "CHAUFFEUR_WORKSPACE_PATH",
        ] {
            let error = check(
                Stopping::Stop,
                &default,
                std::slice::from_ref(&default),
                env(&[(marker, "1")]),
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains(&format!("{marker} is set")), "{error}");
        }
        // An empty variable says nothing.
        check(
            Stopping::Stop,
            &default,
            std::slice::from_ref(&default),
            env(&[("CLAUDECODE", "")]),
        )
        .unwrap();
    }

    #[test]
    fn install_and_uninstall_are_refused_too() {
        let default = PathBuf::from("/nonexistent/u/.chauffeur");
        for (stopping, what) in [
            (Stopping::Install, "refusing to restart"),
            (Stopping::Uninstall, "refusing to stop"),
        ] {
            let error = check(
                stopping,
                &default,
                std::slice::from_ref(&default),
                env(&[("CHAUFFEUR_WORKSPACE_ID", "01a")]),
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains(what), "{error}");
        }
    }

    #[test]
    fn a_throwaway_home_is_stopped_from_anywhere() {
        let default = PathBuf::from("/nonexistent/u/.chauffeur");
        check(
            Stopping::Stop,
            Path::new("/tmp/e2e-xyz/home"),
            std::slice::from_ref(&default),
            env(&[("CHAUFFEUR_WORKSPACE_ID", "01a"), ("CLAUDECODE", "1")]),
        )
        .unwrap();
    }

    #[test]
    fn the_default_home_is_stopped_outside_workspaces_and_agents() {
        let default = PathBuf::from("/nonexistent/u/.chauffeur");
        check(
            Stopping::Stop,
            &default,
            std::slice::from_ref(&default),
            env(&[("CHAUFFEUR_HOME", "/nonexistent/u/.chauffeur")]),
        )
        .unwrap();
    }

    #[test]
    fn the_default_home_is_recognised_through_a_symlink() {
        let user = temp_dir("symlink");
        let real = user.join(".chauffeur");
        std::fs::create_dir_all(&real).unwrap();
        let link = user.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(
            check(
                Stopping::Stop,
                &link,
                std::slice::from_ref(&real),
                env(&[("CHAUFFEUR_WORKSPACE_ID", "01a")]),
            )
            .is_err()
        );
        // With a trailing slash too.
        assert!(
            check(
                Stopping::Stop,
                &PathBuf::from(format!("{}/", real.display())),
                std::slice::from_ref(&real),
                env(&[("CHAUFFEUR_WORKSPACE_ID", "01a")]),
            )
            .is_err()
        );
        std::fs::remove_dir_all(&user).unwrap();
    }

    #[test]
    fn the_start_refusal_says_it_is_the_workspace_not_a_stopped_daemon() {
        let message = start_refusal(Path::new("/u/.chauffeur"));
        assert!(
            message.contains("inside a Chauffeur workspace"),
            "{message}"
        );
        assert!(message.contains("CHAUFFEUR_HOME"), "{message}");
        assert!(!message.contains("is not running"), "{message}");
    }
}
