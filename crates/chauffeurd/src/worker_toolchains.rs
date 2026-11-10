//! The toolchains a repo declares, installed on a GCP worker before its setup script runs when a
//! transfer brings a workspace there (docs/cloud-workers.md): mise for the tools its version files
//! name, corepack for the package manager its package.json names. Only the Mac's driver asks for
//! it, sending to a worker it created: never on the Mac, nor on a machine declared by hand.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::bail;
use serde::Deserialize;

use crate::{Daemon, Result};

/// One install step longer than this is given up: the setup script runs without it.
const STEP_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// What a repo's own files ask for.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Declared {
    /// Each tool with its version, as shown: `node 24`.
    tools: Vec<String>,
    /// A mise configuration, which mise reads only once trusted.
    mise_config: bool,
    /// The package manager its package.json names, for corepack: `pnpm@10.24.0`.
    package_manager: Option<String>,
}

impl Declared {
    fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.package_manager.is_none()
    }

    /// The tools and the package manager, for the transfer's log: `node 24, pnpm 10.24.0`.
    fn shown(&self) -> String {
        let manager = self.package_manager.as_deref().map(|spec| {
            spec.split_once('@')
                .map_or(spec.to_owned(), |(n, v)| format!("{n} {v}"))
        });
        self.tools
            .iter()
            .cloned()
            .chain(manager)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The mise configuration files mise reads in a folder.
const MISE_CONFIGS: [&str; 4] = [
    "mise.toml",
    ".mise.toml",
    "mise/config.toml",
    ".config/mise.toml",
];

/// What the repo at `dir` declares: tools from `.tool-versions` and mise's configuration first,
/// then node from `.nvmrc` or `.node-version` and python from `.python-version` when those did not
/// name them, uv for a `uv.lock`, and a pnpm or yarn `packageManager`. Nothing when it names none.
pub(crate) fn declared(dir: &Path) -> Declared {
    let mut declared = Declared::default();
    let named = |declared: &mut Declared, tool: &str, version: &str| {
        let version = version.trim();
        if !version.is_empty()
            && !declared
                .tools
                .iter()
                .any(|t| t.split(' ').next() == Some(tool))
        {
            declared.tools.push(format!("{tool} {version}"));
        }
    };
    if let Ok(text) = std::fs::read_to_string(dir.join(".tool-versions")) {
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or_default();
            let mut words = line.split_whitespace();
            if let (Some(tool), Some(version)) = (words.next(), words.next()) {
                named(&mut declared, tool, version);
            }
        }
    }
    for config in MISE_CONFIGS {
        let Ok(text) = std::fs::read_to_string(dir.join(config)) else {
            continue;
        };
        declared.mise_config = true;
        let Ok(table) = toml::from_str::<toml::Table>(&text) else {
            continue;
        };
        if let Some(toml::Value::Table(tools)) = table.get("tools") {
            for (tool, version) in tools {
                let version = match version {
                    toml::Value::String(v) => v.clone(),
                    toml::Value::Array(versions) => versions
                        .first()
                        .and_then(toml::Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    toml::Value::Table(t) => t
                        .get("version")
                        .and_then(toml::Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    other => other.to_string(),
                };
                named(&mut declared, tool, &version);
            }
        }
    }
    for (file, tool) in [
        (".nvmrc", "node"),
        (".node-version", "node"),
        (".python-version", "python"),
    ] {
        if let Ok(text) = std::fs::read_to_string(dir.join(file)) {
            let version = text.lines().next().unwrap_or_default().trim();
            named(
                &mut declared,
                tool,
                version.strip_prefix('v').unwrap_or(version),
            );
        }
    }
    if dir.join("uv.lock").is_file() && !declared.tools.iter().any(|t| t.starts_with("uv ")) {
        declared.tools.push("uv".into());
    }
    #[derive(Deserialize)]
    struct Package {
        #[serde(rename = "packageManager")]
        package_manager: Option<String>,
    }
    declared.package_manager = std::fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Package>(&text).ok())
        .and_then(|package| package.package_manager)
        // `pnpm@10.24.0+sha512.…`: corepack checks the hash; the log shows the version.
        .map(|spec| spec.split('+').next().unwrap_or_default().to_owned())
        .filter(|spec| spec.starts_with("pnpm@") || spec.starts_with("yarn@"));
    declared
}

/// Installs what each repo of `workspace_id` declares, in its worktree here: a line per repo said
/// what, and one per repo that could not, whose setup script runs all the same.
pub(crate) async fn install(
    daemon: &Daemon,
    workspace_id: &str,
) -> Result<(Vec<String>, Vec<String>)> {
    let mut lines = Vec::new();
    let mut failures = Vec::new();
    for worktree in daemon.store.worktrees_of(workspace_id)? {
        let dir = Path::new(&worktree.path);
        let declared = declared(dir);
        if worktree.removed || declared.is_empty() {
            continue;
        }
        lines.push(format!(
            "installing {} for {}",
            declared.shown(),
            worktree.repo
        ));
        if let Err(error) = install_in(daemon, dir, &declared).await {
            failures.push(format!("{}: {error:#}", worktree.repo));
            lines.push(format!(
                "  could not install what {} declares ({error:#}); its setup runs without it",
                worktree.repo
            ));
        }
    }
    Ok((lines, failures))
}

async fn install_in(daemon: &Daemon, dir: &Path, declared: &Declared) -> anyhow::Result<()> {
    if declared.mise_config {
        run(daemon, dir, "mise", &["trust", "."]).await?;
    }
    run(daemon, dir, "mise", &["install"]).await?;
    if declared.package_manager.is_some() {
        run(daemon, dir, "corepack", &["enable"]).await?;
        run(daemon, dir, "corepack", &["install"]).await?;
        // The package manager's command, put beside node by corepack, gets its shim.
        run(daemon, dir, "mise", &["reshim"]).await?;
    }
    Ok(())
}

/// Runs `program` in `dir` with the login shell's environment, answering yes to mise and corepack;
/// its error is the last line it printed.
async fn run(daemon: &Daemon, dir: &Path, program: &str, args: &[&str]) -> anyhow::Result<()> {
    let child = tokio::process::Command::new(program)
        .args(args)
        .current_dir(dir)
        .env_clear()
        .envs(daemon.shell_env.vars())
        .env("MISE_YES", "1")
        .env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            bail!("{program} is missing on this worker; installing Chauffeur on it again adds it")
        }
        Err(error) => bail!("could not run {program}: {error}"),
    };
    let Ok(output) = tokio::time::timeout(STEP_TIMEOUT, child.wait_with_output()).await else {
        bail!("{program} {} took too long", args.join(" "));
    };
    let output = output?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        match stderr.trim().lines().last() {
            Some(line) => bail!("{line}"),
            None => bail!("{program} {} failed ({})", args.join(" "), output.status),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = dir.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        dir
    }

    #[test]
    fn a_node_repo_names_its_node_and_its_pinned_package_manager() {
        let dir = repo(&[
            (".nvmrc", "v24\n"),
            (
                "package.json",
                r#"{"packageManager": "pnpm@10.24.0+sha512.abc", "engines": {"node": "^24.15.0"}}"#,
            ),
        ]);
        let declared = declared(dir.path());
        assert_eq!(declared.tools, ["node 24"]);
        assert_eq!(declared.package_manager.as_deref(), Some("pnpm@10.24.0"));
        assert!(!declared.mise_config);
        assert_eq!(declared.shown(), "node 24, pnpm 10.24.0");
    }

    #[test]
    fn a_python_repo_with_a_uv_lock_names_python_and_uv() {
        let dir = repo(&[(".python-version", "3.12\n"), ("uv.lock", "version = 1\n")]);
        assert_eq!(declared(dir.path()).tools, ["python 3.12", "uv"]);
    }

    #[test]
    fn tool_versions_and_mise_configuration_come_before_the_idiomatic_files() {
        let dir = repo(&[
            (
                ".tool-versions",
                "# pinned\nnode 22.1.0\nterraform 1.9.0 # infra\n",
            ),
            (
                "mise.toml",
                "[tools]\npython = \"3.11\"\nuv = { version = \"0.5\" }\ngo = [\"1.23\", \"1.22\"]\n",
            ),
            (".nvmrc", "v24\n"),
            (".python-version", "3.12\n"),
            ("uv.lock", ""),
        ]);
        let declared = declared(dir.path());
        assert!(declared.mise_config);
        assert_eq!(
            declared.tools,
            [
                "node 22.1.0",
                "terraform 1.9.0",
                "go 1.23",
                "python 3.11",
                "uv 0.5"
            ]
        );
    }

    #[test]
    fn a_repo_that_names_nothing_installs_nothing() {
        let dir = repo(&[
            ("README.md", "# alpha\n"),
            ("package.json", r#"{"packageManager": "npm@10.0.0"}"#),
        ]);
        assert!(declared(dir.path()).is_empty());
        let unreadable = repo(&[("package.json", "{ not json"), (".nvmrc", "\n")]);
        assert!(declared(unreadable.path()).is_empty());
    }
}
