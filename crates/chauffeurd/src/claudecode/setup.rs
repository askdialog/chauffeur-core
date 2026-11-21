//! A setup script guessed for each repo the import makes workspaces of that has none (committed,
//! or in the user's settings), from the lockfiles at its root: written to the user's settings
//! (`[repos.<name>.scripts] setup`) before any worktree exists, never into the repo. Lockfiles
//! only in subfolders are listed, nothing proposed.

use std::path::Path;

use chauffeur_protocol::claudecode::ClaudeCodeSetup;

use crate::git::Git;
use crate::repos::repo_folder;
use crate::settings::RepoSettings;
use crate::{Daemon, Error, Result};

/// The lockfiles read, with the command each gives and whether it is JavaScript's: one JavaScript
/// package manager at most, the first found in this order.
const LOCKFILES: [(&str, &str, bool); 8] = [
    ("pnpm-lock.yaml", "pnpm install", true),
    ("bun.lock", "bun install", true),
    ("bun.lockb", "bun install", true),
    ("yarn.lock", "yarn install", true),
    ("package-lock.json", "npm install", true),
    ("uv.lock", "uv sync", false),
    ("poetry.lock", "poetry install", false),
    ("Gemfile.lock", "bundle install", false),
];

/// The setup of `repo`, whose clone is `clone`, when it has none: guessed from its root
/// lockfiles, else its lockfiles in subfolders listed; none when it has a setup or no lockfile.
pub(super) async fn guess(
    d: &Daemon,
    git: &Git,
    repo: &str,
    clone: &Path,
) -> Option<ClaudeCodeSetup> {
    let settings = d.settings.get();
    let user = settings
        .repos
        .get(repo_folder(repo))
        .and_then(|r| r.scripts.as_ref());
    // Committed (in the clone's checkout) or the user's: theirs stays.
    let has = RepoSettings::load(clone, clone, user).map_or(true, |s| s.setup.is_some());
    if has {
        return None;
    }
    let tracked = git.run_bytes(clone, &["ls-files", "-z"]).await.ok()?;
    let files: Vec<String> = tracked
        .split(|b| *b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .filter(|f| {
            let name = f.rsplit('/').next().unwrap_or(f);
            LOCKFILES.iter().any(|(lockfile, _, _)| *lockfile == name)
        })
        .collect();
    let root: Vec<&String> = files.iter().filter(|f| !f.contains('/')).collect();
    if root.is_empty() {
        return (!files.is_empty()).then(|| ClaudeCodeSetup {
            repo: repo.to_owned(),
            command: None,
            lockfiles: files,
        });
    }
    // In the order of `LOCKFILES`: the first JavaScript one alone, then each other ecosystem's.
    let mut commands: Vec<&str> = Vec::new();
    let mut used = Vec::new();
    let mut javascript = false;
    for (lockfile, command, js) in LOCKFILES {
        if !root.iter().any(|f| f.as_str() == lockfile) || (js && javascript) {
            continue;
        }
        javascript |= js;
        used.push(lockfile.to_owned());
        if !commands.contains(&command) {
            commands.push(command);
        }
    }
    Some(ClaudeCodeSetup {
        repo: repo.to_owned(),
        command: Some(commands.join(" && ")),
        lockfiles: used,
    })
}

/// The guessed setups written to the user's settings, as `[repos.<name>.scripts] setup`.
pub(super) fn write(d: &Daemon, setups: &[ClaudeCodeSetup]) -> Result<()> {
    let guessed: Vec<(&str, &str)> = setups
        .iter()
        .filter_map(|s| Some((repo_folder(&s.repo), s.command.as_deref()?)))
        .collect();
    if guessed.is_empty() {
        return Ok(());
    }
    d.settings
        .update(|doc| {
            for (name, command) in &guessed {
                let repos = crate::conductor::implicit_table(doc.as_table_mut(), "repos");
                let repo = crate::conductor::implicit_table(repos, name);
                crate::conductor::implicit_table(repo, "scripts")
                    .insert("setup", toml_edit::value(*command));
            }
        })
        .map_err(|e| Error::Internal(e.context("writing the guessed setup scripts")))
}
