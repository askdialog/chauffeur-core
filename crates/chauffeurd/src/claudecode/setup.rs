//! A setup script guessed for each repo the import makes workspaces in that has none (committed,
//! or in the user's settings), from the lockfiles at its root: written to the user's settings
//! (`[repos.<name>.scripts] setup`) before any worktree exists, never into the repo. Lockfiles
//! only in subfolders are listed, nothing proposed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chauffeur_protocol::claudecode::ClaudeCodeSetup;

use crate::git::Git;
use crate::repos::repo_folder;
use crate::settings::{RepoSettings, sets_setup};
use crate::{Daemon, Error, Result};

struct Lockfile {
    file: &'static str,
    command: &'static str,
    /// One JavaScript package manager at most: the first found, in this list's order.
    javascript: bool,
}

const LOCKFILES: [Lockfile; 8] = [
    Lockfile {
        file: "pnpm-lock.yaml",
        command: "pnpm install",
        javascript: true,
    },
    Lockfile {
        file: "bun.lock",
        command: "bun install",
        javascript: true,
    },
    Lockfile {
        file: "bun.lockb",
        command: "bun install",
        javascript: true,
    },
    Lockfile {
        file: "yarn.lock",
        command: "yarn install",
        javascript: true,
    },
    Lockfile {
        file: "package-lock.json",
        command: "npm install",
        javascript: true,
    },
    Lockfile {
        file: "uv.lock",
        command: "uv sync",
        javascript: false,
    },
    Lockfile {
        file: "poetry.lock",
        command: "poetry install",
        javascript: false,
    },
    Lockfile {
        file: "Gemfile.lock",
        command: "bundle install",
        javascript: false,
    },
];

/// A tree a workspace of the run checks out: a ref of a clone.
pub(super) struct Tree {
    pub(super) clone: PathBuf,
    pub(super) reference: String,
}

/// The setups guessed for the repos of `trees` (each one's base first, then its branches') that
/// have none: from the lockfiles of the base, none when one of the trees commits a setup (the
/// user's settings, where the guess goes, come before a committed one).
pub(super) async fn guess_all(
    d: &Daemon,
    git: &Git,
    trees: &BTreeMap<String, Vec<Tree>>,
) -> Vec<ClaudeCodeSetup> {
    let mut setups = Vec::new();
    for (repo, trees) in trees {
        let Some(base) = trees.first() else {
            continue;
        };
        if has_setup(d, repo, &base.clone) || committed(git, trees).await {
            continue;
        }
        let listed = git
            .run_bytes(
                &base.clone,
                &["ls-tree", "-r", "-z", "--name-only", &base.reference],
            )
            .await;
        match listed {
            Ok(tracked) => {
                let files = tracked
                    .split(|b| *b == 0)
                    .filter(|f| !f.is_empty())
                    .map(|f| String::from_utf8_lossy(f).into_owned());
                setups.extend(from_lockfiles(repo, files));
            }
            Err(error) => {
                tracing::warn!(%error, repo, "could not list the repo's files to guess its setup")
            }
        }
    }
    setups
}

/// A setup in the clone's own files (its local settings, its checkout's) or the user's settings;
/// one unreadable counts as one, left as it is.
fn has_setup(d: &Daemon, repo: &str, clone: &Path) -> bool {
    let settings = d.settings.get();
    let user = settings
        .repos
        .get(repo_folder(repo))
        .and_then(|r| r.scripts.as_ref());
    RepoSettings::load(clone, clone, user).map_or(true, |s| s.setup.is_some())
}

/// One of `trees` commits a setup, in Chauffeur's settings or Conductor's.
async fn committed(git: &Git, trees: &[Tree]) -> bool {
    for tree in trees {
        for file in [".chauffeur/settings.toml", ".conductor/settings.toml"] {
            let object = format!("{}:{file}", tree.reference);
            if !git.ok(&tree.clone, &["cat-file", "-e", &object]).await {
                continue;
            }
            let text = git.run(&tree.clone, &["show", &object]).await;
            if text.map_or(true, |text| sets_setup(&text)) {
                return true;
            }
        }
    }
    false
}

/// `repo`'s setup from its tracked `files`: its root lockfiles' commands, else its lockfiles in
/// subfolders listed with none; none without a lockfile.
fn from_lockfiles(repo: &str, files: impl Iterator<Item = String>) -> Option<ClaudeCodeSetup> {
    let named = |f: &str| {
        let name = f.rsplit('/').next().unwrap_or(f);
        LOCKFILES.iter().any(|l| l.file == name)
    };
    let found: Vec<String> = files.filter(|f| named(f)).collect();
    let mut commands: Vec<&str> = Vec::new();
    let mut used = Vec::new();
    let mut javascript = false;
    for lockfile in &LOCKFILES {
        if (lockfile.javascript && javascript) || !found.iter().any(|f| f == lockfile.file) {
            continue;
        }
        javascript |= lockfile.javascript;
        commands.push(lockfile.command);
        used.push(lockfile.file.to_owned());
    }
    if commands.is_empty() {
        return (!found.is_empty()).then(|| ClaudeCodeSetup {
            repo: repo.to_owned(),
            command: None,
            lockfiles: found,
        });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn guess(files: &[&str]) -> Option<(Option<String>, Vec<String>)> {
        from_lockfiles("r", files.iter().map(|f| f.to_string())).map(|s| (s.command, s.lockfiles))
    }

    #[test]
    fn the_root_lockfiles_give_the_setup() {
        let some = |command: &str, lockfiles: &[&str]| {
            Some((
                Some(command.to_string()),
                lockfiles.iter().map(|f| f.to_string()).collect(),
            ))
        };
        assert_eq!(
            guess(&["README.md", "pnpm-lock.yaml"]),
            some("pnpm install", &["pnpm-lock.yaml"])
        );
        assert_eq!(
            guess(&["package-lock.json"]),
            some("npm install", &["package-lock.json"])
        );
        assert_eq!(guess(&["bun.lockb"]), some("bun install", &["bun.lockb"]));
        assert_eq!(
            guess(&["Gemfile.lock"]),
            some("bundle install", &["Gemfile.lock"])
        );
        // One JavaScript package manager, the first in pnpm, bun, yarn, npm; then uv, poetry and
        // bundle, each.
        assert_eq!(
            guess(&["package-lock.json", "yarn.lock", "bun.lock", "bun.lockb"]),
            some("bun install", &["bun.lock"])
        );
        assert_eq!(
            guess(&["uv.lock", "yarn.lock", "poetry.lock", "Gemfile.lock"]),
            some(
                "yarn install && uv sync && poetry install && bundle install",
                &["yarn.lock", "uv.lock", "poetry.lock", "Gemfile.lock"]
            )
        );
        // A root lockfile wins over those in subfolders.
        assert_eq!(
            guess(&["web/yarn.lock", "uv.lock"]),
            some("uv sync", &["uv.lock"])
        );
    }

    #[test]
    fn lockfiles_only_in_subfolders_are_listed_without_a_setup() {
        assert_eq!(
            guess(&["app/package-lock.json", "api/uv.lock", "api/main.py"]),
            Some((
                None,
                vec!["app/package-lock.json".into(), "api/uv.lock".into()]
            ))
        );
        // A name that only ends like a lockfile is none.
        assert_eq!(
            guess(&["README.md", "my-yarn.lock", "src/uv.lock.bak"]),
            None
        );
    }
}
