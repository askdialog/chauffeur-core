//! `--move-changes`: a released checkout's changes, once copied into the workspace, are also saved
//! in a git ref kept as long as an archive's snapshot (`snapshot_retention_days`, 30 by default),
//! and the checkout goes back to its default branch, so that one copy of the work is left:
//! Chauffeur's.

use std::path::{Path, PathBuf};

use chauffeur_protocol::claudecode::ClaudeCodeMoved;
use serde::{Deserialize, Serialize};

use super::branches::Released;
use crate::git::Git;
use crate::store::WorkspaceRow;
use crate::{Daemon, Result};

/// `kv` key of the refs saved, each with when, for the purge.
const SAVED: &str = "claudecode_moved";

#[derive(Serialize, Deserialize, Debug)]
struct Saved {
    /// A folder of the repo holding the ref.
    clone: PathBuf,
    reference: String,
    at: String,
}

/// What `move_out` did: where the changes went, and what to say.
pub(super) enum Moved {
    /// Left as it is, for this reason: nothing saved, nothing cleaned.
    Left(String),
    /// Saved; and, when something stopped the cleaning, what.
    Saved(ClaudeCodeMoved, Option<String>),
}

/// Saves the changes of a checkout `released` for `row` in a ref of its own, then cleans it and
/// puts it back on `base`, the repo's default branch: the clone's own checkout on it, a worktree
/// of the user's detached at it (the clone's is where the default branch goes), as is one whose
/// branch another checkout holds. One whose changes git does not fully see (files told to be
/// left alone, a merge or rebase under way) is left as it is. Submodules are left as they are.
pub(super) async fn move_out(
    d: &Daemon,
    git: &Git,
    released: &Released,
    base: &str,
    row: &WorkspaceRow,
) -> anyhow::Result<Moved> {
    let folder = released.folder.as_path();
    if let Some(reason) = unmovable(git, folder).await? {
        return Ok(Moved::Left(reason));
    }
    let Some(target) = target(git, folder, base).await else {
        return Ok(Moved::Left(format!(
            "its default branch {base} is found neither here nor on its remote"
        )));
    };
    let reference = free_reference(git, folder, row, &released.at.repo).await;
    git.snapshot(folder, &reference).await?;
    // Unrecorded, it would never be purged: it goes, and nothing is cleaned.
    if let Err(error) = remember(d, &released.clone, &reference) {
        let _ = git.delete_ref(folder, &reference).await;
        return Err(error.into());
    }
    // Saved: whatever stops the cleaning now is said, the ref named.
    let no_recursion = ["-c", "submodule.recurse=false"];
    let linked = git
        .run(folder, &["rev-parse", "--absolute-git-dir"])
        .await?
        != git
            .run(
                folder,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await?;
    let mut stopped = None;
    for step in [
        &[&no_recursion[..], &["reset", "--hard", "-q"]].concat()[..],
        &["clean", "-fdq"],
    ] {
        if let Err(error) = git.run(folder, step).await {
            stopped = Some(format!("{error:#}"));
            break;
        }
    }
    if stopped.is_none() {
        let on_branch = !linked
            && target == format!("refs/heads/{base}")
            && git
                .ok(
                    folder,
                    &[&no_recursion[..], &["switch", "-q", base]].concat(),
                )
                .await;
        if !on_branch
            && let Err(error) = git
                .run(
                    folder,
                    &[&no_recursion[..], &["switch", "-q", "--detach", &target]].concat(),
                )
                .await
        {
            stopped = Some(format!("{error:#}"));
        }
    }
    // Where it is, as git says.
    let back_on = match git.current_branch(folder).await {
        Ok(Some(branch)) => branch,
        _ => {
            let commit = git.run(folder, &["rev-parse", "--short", "HEAD"]).await?;
            format!("{commit}, detached")
        }
    };
    Ok(Moved::Saved(
        ClaudeCodeMoved {
            saved_in: reference,
            back_on,
        },
        stopped,
    ))
}

/// Why `folder`'s changes cannot be saved whole, if so: files git is told to leave alone
/// (`--assume-unchanged`, `--skip-worktree`), whose changes neither the copy nor the ref holds;
/// a merge, rebase, cherry-pick or revert under way, which a reset would end.
async fn unmovable(git: &Git, folder: &Path) -> anyhow::Result<Option<String>> {
    let listed = git.run(folder, &["ls-files", "-v"]).await?;
    let hidden = listed
        .lines()
        .any(|line| line.starts_with(|c: char| c.is_ascii_lowercase()) || line.starts_with("S "));
    if hidden {
        return Ok(Some(
            "some of its files are marked --assume-unchanged or --skip-worktree, whose changes git does not show".into(),
        ));
    }
    let git_dir = PathBuf::from(
        git.run(folder, &["rev-parse", "--absolute-git-dir"])
            .await?,
    );
    let under_way = [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
    ];
    if under_way.iter().any(|name| git_dir.join(name).exists()) {
        return Ok(Some(
            "a merge, rebase, cherry-pick or revert is under way there".into(),
        ));
    }
    Ok(None)
}

/// The ref the checkout goes back to: the local default branch, else its remote's copy.
async fn target(git: &Git, folder: &Path, base: &str) -> Option<String> {
    let local = format!("refs/heads/{base}");
    if git
        .ok(folder, &["rev-parse", "-q", "--verify", &local])
        .await
    {
        return Some(local);
    }
    let remote = format!("refs/remotes/{}/{base}", git.base_remote(folder).await);
    git.ok(folder, &["rev-parse", "-q", "--verify", &remote])
        .await
        .then_some(remote)
}

/// A ref no other checkout's changes are saved in: two checkouts of one branch get one each.
async fn free_reference(git: &Git, folder: &Path, row: &WorkspaceRow, repo: &str) -> String {
    let base = format!(
        "refs/chauffeur/claude-code/{}/{}",
        row.id,
        crate::repos::repo_folder(repo)
    );
    let mut reference = base.clone();
    for n in 2.. {
        if !git
            .ok(folder, &["show-ref", "--verify", "--quiet", &reference])
            .await
        {
            break;
        }
        reference = format!("{base}-{n}");
    }
    reference
}

/// Held while the list of saved refs is read and written back: a ref recorded while the purge
/// deletes others is kept.
static LEDGER: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn remember(d: &Daemon, clone: &Path, reference: &str) -> Result<()> {
    let _ledger = LEDGER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut saved = read(d)?;
    saved.push(Saved {
        clone: clone.to_path_buf(),
        reference: reference.to_owned(),
        at: chrono::Utc::now().to_rfc3339(),
    });
    write(d, &saved)
}

fn read(d: &Daemon) -> Result<Vec<Saved>> {
    match d.store.kv_get(SAVED)? {
        Some(text) => Ok(serde_json::from_str(&text)?),
        None => Ok(Vec::new()),
    }
}

fn write(d: &Daemon, saved: &[Saved]) -> Result<()> {
    d.store.kv_set(SAVED, &serde_json::to_string(saved)?)
}

/// Deletes the refs saved before `cutoff`, and forgets them; one that cannot be deleted is tried
/// again at the next purge, unless its clone is gone. How many went.
pub async fn purge(d: &Daemon, git: &Git, cutoff: &str) -> Result<usize> {
    let expired: Vec<Saved> = {
        let _ledger = LEDGER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        read(d)?
            .into_iter()
            .filter(|s| s.at.as_str() < cutoff)
            .collect()
    };
    let mut gone = Vec::new();
    for saved in expired {
        match git.delete_ref(&saved.clone, &saved.reference).await {
            Ok(()) => gone.push(saved.reference),
            Err(_) if !saved.clone.exists() => gone.push(saved.reference),
            Err(error) => {
                tracing::warn!(%error, reference = saved.reference, "could not delete a saved import's ref: tried again at the next purge");
            }
        }
    }
    let _ledger = LEDGER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut saved = read(d)?;
    saved.retain(|s| !gone.contains(&s.reference));
    write(d, &saved)?;
    Ok(gone.len())
}
