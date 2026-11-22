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

/// Saves the changes of a checkout `released` for `row` in a ref, then cleans
/// it and puts it back on `base`, the repo's default branch: the clone's own checkout on it, a
/// worktree of the user's detached at it (the clone's is where the default branch goes), as is
/// one whose branch another checkout holds.
pub(super) async fn move_out(
    d: &Daemon,
    git: &Git,
    released: &Released,
    base: &str,
    row: &WorkspaceRow,
) -> anyhow::Result<ClaudeCodeMoved> {
    let folder = released.folder.as_path();
    let reference = format!(
        "refs/chauffeur/claude-code/{}/{}",
        row.id,
        crate::repos::repo_folder(&released.at.repo)
    );
    git.snapshot(folder, &reference).await?;
    remember(d, &released.clone, &reference)?;
    git.run(folder, &["reset", "--hard", "-q"]).await?;
    git.run(folder, &["clean", "-fdq"]).await?;
    let linked = git
        .run(folder, &["rev-parse", "--absolute-git-dir"])
        .await?
        != git
            .run(
                folder,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await?;
    let back_on = if !linked && git.ok(folder, &["switch", "-q", base]).await {
        base.to_owned()
    } else {
        git.run(folder, &["switch", "-q", "--detach", base]).await?;
        format!("{base}, detached")
    };
    Ok(ClaudeCodeMoved {
        saved_in: reference,
        back_on,
    })
}

fn remember(d: &Daemon, clone: &Path, reference: &str) -> Result<()> {
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

/// Deletes the refs saved before `cutoff`; how many.
pub async fn purge(d: &Daemon, git: &Git, cutoff: &str) -> Result<usize> {
    let (old, kept): (Vec<Saved>, Vec<Saved>) =
        read(d)?.into_iter().partition(|s| s.at.as_str() < cutoff);
    for saved in &old {
        if let Err(error) = git.delete_ref(&saved.clone, &saved.reference).await {
            tracing::warn!(%error, reference = saved.reference, "could not delete a saved import's ref");
        }
    }
    write(d, &kept)?;
    Ok(old.len())
}
