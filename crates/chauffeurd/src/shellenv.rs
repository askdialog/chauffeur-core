//! The environment every spawned process gets: the user's login shell environment, captured at
//! start so that a daemon started by launchd still finds node, pnpm, uv, gh and the SSH agent, and
//! read again on demand (`env.reload`) when the user exported a variable since.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

/// Variables never passed on: they belong to the process that started the daemon (a Conductor
/// workspace, a Claude Code session) and would mislead tools in Chauffeur's workspaces.
const SCRUBBED_PREFIXES: &[&str] = &["CONDUCTOR_", "CHAUFFEUR_", "CLAUDE_CODE_", "CODEX_"];
const SCRUBBED: &[&str] = &[
    "CLAUDECODE",
    "OLDPWD",
    "PWD",
    "SHLVL",
    "_",
    "TERM_SESSION_ID",
    "VSCODE_INJECTION",
];

/// Shared by every clone: a reload is seen by all.
#[derive(Clone, Default)]
pub struct ShellEnv {
    state: Arc<RwLock<State>>,
}

#[derive(Default)]
struct State {
    captured: Option<BTreeMap<String, String>>,
    /// Tests: the shell read instead of the user's.
    shell: Option<String>,
    /// Chauffeur's own tools (`gh`), last on `PATH` once the folder exists.
    tools: Option<PathBuf>,
}

/// What a reload changed, by variable name (never a value).
#[derive(Debug, Default, PartialEq)]
pub struct EnvChange {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

impl EnvChange {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

impl ShellEnv {
    pub fn new() -> ShellEnv {
        ShellEnv::default()
    }

    /// Captures the login shell's environment. Blocking (a few hundred ms); call once at start.
    pub fn capture(&self) {
        if self.state.read().unwrap().captured.is_some() {
            return;
        }
        let vars = capture_login_env(&self.shell()).unwrap_or_else(|error| {
            tracing::warn!(%error, "could not capture the login shell environment, using the daemon's");
            std::env::vars().collect()
        });
        self.state.write().unwrap().captured = Some(scrub(vars));
    }

    /// Reads the login shell's environment again and uses it from now on; on failure the one
    /// before stays. Blocking (a few hundred ms).
    pub fn reload(&self) -> anyhow::Result<EnvChange> {
        let fresh = scrub(capture_login_env(&self.shell())?);
        // The captured environment, without Chauffeur's tools: they are not the user's change.
        let before = self
            .state
            .read()
            .unwrap()
            .captured
            .clone()
            .unwrap_or_else(|| scrub(std::env::vars().collect()));
        let mut change = EnvChange::default();
        for (key, value) in &fresh {
            match before.get(key) {
                None => change.added.push(key.clone()),
                Some(old) if old != value => change.changed.push(key.clone()),
                Some(_) => {}
            }
        }
        change.removed = before
            .keys()
            .filter(|key| !fresh.contains_key(*key))
            .cloned()
            .collect();
        self.state.write().unwrap().captured = Some(fresh);
        Ok(change)
    }

    /// Tests: this environment instead of the login shell's (a fake tool first on `PATH`).
    pub fn set_vars(&self, vars: BTreeMap<String, String>) {
        self.state.write().unwrap().captured = Some(scrub(vars));
    }

    /// Tests: reads `shell` (run as `shell -l -i -c …`) instead of the user's login shell.
    pub fn use_login_shell(&self, shell: String) {
        self.state.write().unwrap().shell = Some(shell);
    }

    /// Puts `dir` last on every child's `PATH` once it exists: a tool the user installed comes
    /// first, Chauffeur's own copy fills in when there is none.
    pub fn use_tools(&self, dir: PathBuf) {
        self.state.write().unwrap().tools = Some(dir);
    }

    /// The login shell whose environment is used.
    pub fn shell(&self) -> String {
        self.state
            .read()
            .unwrap()
            .shell
            .clone()
            .unwrap_or_else(login_shell)
    }

    /// The base environment for a child process.
    pub fn vars(&self) -> BTreeMap<String, String> {
        let state = self.state.read().unwrap();
        let mut vars = match &state.captured {
            Some(vars) => vars.clone(),
            None => scrub(std::env::vars().collect()),
        };
        if let Some(tools) = state.tools.as_ref().filter(|dir| dir.is_dir()) {
            let path = vars.entry("PATH".into()).or_default();
            if !std::env::split_paths(path).any(|dir| dir == *tools) {
                let tools = tools.to_string_lossy();
                *path = if path.is_empty() {
                    tools.into_owned()
                } else {
                    format!("{path}:{tools}")
                };
            }
        }
        vars
    }
}

/// The file of a login shell where a variable is best exported for Chauffeur to get it: read by
/// every zsh (`~/.zshenv`), by bash as a login shell (`~/.bash_profile`).
pub fn shell_profile(shell: &str) -> &'static str {
    match std::path::Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
    {
        "zsh" => "~/.zshenv",
        "bash" => "~/.bash_profile",
        "fish" => "~/.config/fish/config.fish",
        _ => "~/.profile",
    }
}

fn scrub(mut vars: BTreeMap<String, String>) -> BTreeMap<String, String> {
    vars.retain(|key, _| {
        !SCRUBBED.contains(&key.as_str()) && !SCRUBBED_PREFIXES.iter().any(|p| key.starts_with(p))
    });
    vars
}

/// The user's login shell: `$SHELL`, else their account's (a daemon that launchd or systemd starts
/// has no `$SHELL`), else the shell scripts run in.
pub fn login_shell() -> String {
    if let Ok(shell) = std::env::var("SHELL")
        && !shell.is_empty()
    {
        return shell;
    }
    // SAFETY: getpwuid returns a pointer into static storage, or null; copied out at once.
    let from_passwd = unsafe {
        let entry = libc::getpwuid(libc::getuid());
        (!entry.is_null() && !(*entry).pw_shell.is_null()).then(|| {
            std::ffi::CStr::from_ptr((*entry).pw_shell)
                .to_string_lossy()
                .into_owned()
        })
    };
    from_passwd
        .filter(|shell| std::path::Path::new(shell).is_file())
        .unwrap_or_else(|| script_shell().to_string())
}

/// The shell setup, run and archive scripts run in: zsh, as in Conductor, where installed (always
/// on macOS); else bash, else sh.
pub fn script_shell() -> &'static str {
    static SHELL: OnceLock<&'static str> = OnceLock::new();
    SHELL.get_or_init(|| {
        ["/bin/zsh", "/usr/bin/zsh", "/bin/bash", "/usr/bin/bash"]
            .into_iter()
            .find(|path| std::path::Path::new(path).is_file())
            .unwrap_or("/bin/sh")
    })
}

fn capture_login_env(shell: &str) -> anyhow::Result<BTreeMap<String, String>> {
    const MARKER: &str = "__CHAUFFEUR_ENV__";
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &format!("printf '{MARKER}'; env -0")])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            anyhow::bail!("{shell} -l -i took more than 10 s");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (_, env) = text
        .split_once(MARKER)
        .ok_or_else(|| anyhow::anyhow!("no environment in the shell's output"))?;
    let vars: BTreeMap<String, String> = env
        .split('\0')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    anyhow::ensure!(
        vars.contains_key("PATH"),
        "the captured environment has no PATH"
    );
    Ok(vars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_and_terminals_get_an_installed_shell() {
        assert!(std::path::Path::new(script_shell()).is_file());
        assert!(!login_shell().is_empty());
    }

    #[test]
    fn chauffeurs_tools_come_last_on_the_path_once_their_folder_exists() {
        let dir = tempfile::tempdir().unwrap();
        let tools = dir.path().join("bin");
        let env = ShellEnv::new();
        env.set_vars(BTreeMap::from([(
            "PATH".to_string(),
            "/usr/bin:/bin".to_string(),
        )]));
        env.use_tools(tools.clone());
        assert_eq!(env.vars()["PATH"], "/usr/bin:/bin");
        std::fs::create_dir(&tools).unwrap();
        assert_eq!(
            env.vars()["PATH"],
            format!("/usr/bin:/bin:{}", tools.display())
        );
        env.set_vars(BTreeMap::from([(
            "PATH".to_string(),
            format!("{}:/bin", tools.display()),
        )]));
        assert_eq!(env.vars()["PATH"], format!("{}:/bin", tools.display()));
    }

    #[test]
    fn foreign_session_variables_are_scrubbed() {
        let vars = BTreeMap::from([
            ("PATH".to_string(), "/bin".to_string()),
            ("CONDUCTOR_PORT".to_string(), "55610".to_string()),
            ("CLAUDECODE".to_string(), "1".to_string()),
            ("HOME".to_string(), "/Users/x".to_string()),
        ]);
        let kept = scrub(vars);
        assert_eq!(kept.keys().collect::<Vec<_>>(), vec!["HOME", "PATH"]);
    }

    #[test]
    fn a_variable_is_exported_from_the_file_its_shell_always_reads() {
        assert_eq!(shell_profile("/bin/zsh"), "~/.zshenv");
        assert_eq!(shell_profile("/opt/homebrew/bin/bash"), "~/.bash_profile");
        assert_eq!(shell_profile("/usr/bin/fish"), "~/.config/fish/config.fish");
        assert_eq!(shell_profile("/bin/sh"), "~/.profile");
    }

    #[test]
    fn a_reload_reads_the_login_shell_again_and_names_what_changed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile");
        let shell = dir.path().join("login-shell");
        std::fs::write(
            &shell,
            format!(
                "#!/bin/sh\nexport PATH=/usr/bin:/bin\n[ -f '{}' ] && . '{}'\nexec /bin/sh -c \"$4\"\n",
                profile.display(),
                profile.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(&profile, "export KEPT=1 GONE=1 EDITED=a CONDUCTOR_X=1\n").unwrap();
        let env = ShellEnv::new();
        env.use_login_shell(shell.display().to_string());
        env.capture();
        assert_eq!(env.vars().get("EDITED").map(String::as_str), Some("a"));

        std::fs::write(
            &profile,
            "export KEPT=1 EDITED=b NEW_TOKEN=s CONDUCTOR_Y=1\n",
        )
        .unwrap();
        let clone = env.clone();
        let change = env.reload().unwrap();
        assert_eq!(change.added, vec!["NEW_TOKEN"]);
        assert_eq!(change.changed, vec!["EDITED"]);
        assert_eq!(change.removed, vec!["GONE"]);
        // Every clone sees it; foreign session variables stay out.
        assert_eq!(clone.vars().get("NEW_TOKEN").map(String::as_str), Some("s"));
        assert!(!clone.vars().contains_key("CONDUCTOR_Y"));

        // A shell that fails keeps the environment before.
        env.use_login_shell(dir.path().join("missing").display().to_string());
        assert!(env.reload().is_err());
        assert!(env.vars().contains_key("NEW_TOKEN"));
    }
}
