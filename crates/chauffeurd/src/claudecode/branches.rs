//! The live branches' workspaces (docs/adr/claude-code-import.md): each a workspace on its branch,
//! its sessions resumable chats (`copy`). Each repo's clone is chosen at planning, once for the
//! run: Chauffeur's, else the user's holding the branch (its folder then joins the clone folders).
//! When that is the user's clone, the branch is released where it is checked out there (HEAD
//! detached on the same commit, its uncommitted and untracked changes copied into the workspace);
//! when Chauffeur has a clone of its own, the branch is fetched there and the user's checkouts
//! keep it. A checkout gone, or half deleted by a cleanup, carries the branch's commits only. A
//! workspace that fails to import puts back what it released; the others go on.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chauffeur_protocol::claudecode::{
    ClaudeCodeOutcome, ClaudeCodeWorkspaceKind, ClaudeCodeWorkspacePlan,
};
use chauffeur_protocol::client::CreateWorkspaceParams;
use chauffeur_protocol::model::WorkspaceState;

use super::copy::Report;
use super::sessions::Session;
use super::{Holder, real};
use crate::conductor::{self, Changes, same_folder};
use crate::git::Git;
use crate::repos::repo_folder;
use crate::settings::expand_home;
use crate::store::WorkspaceRow;
use crate::{Daemon, Error, Result};

/// A branch of a repo.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct RepoBranch {
    pub(super) repo: String,
    pub(super) branch: String,
}

impl RepoBranch {
    pub(super) fn new(repo: &str, branch: &str) -> RepoBranch {
        RepoBranch {
            repo: repo.to_owned(),
            branch: branch.to_owned(),
        }
    }

    /// `claudecode_branch:<repo>\n<branch>` → the id of the workspace an import made on it.
    fn record(&self) -> String {
        format!("claudecode_branch:{}\n{}", self.repo, self.branch)
    }
}

/// The workspace an earlier import made on `branch` of `repo`, while it is ready: the sessions
/// found on that branch since go there.
pub(super) fn recorded(d: &Daemon, repo: &str, branch: &str) -> Result<Option<WorkspaceRow>> {
    let Some(id) = d.store.kv_get(&RepoBranch::new(repo, branch).record())? else {
        return Ok(None);
    };
    Ok(d.store
        .find_workspace(&id)?
        .filter(|row| row.state == WorkspaceState::Ready))
}

/// The workspace an earlier import made on the branch, in any of its repos.
pub(super) fn existing(
    d: &Daemon,
    workspace: &ClaudeCodeWorkspacePlan,
    branch: &str,
) -> Result<Option<WorkspaceRow>> {
    for repo in &workspace.repos {
        if let Some(row) = recorded(d, repo, branch)? {
            return Ok(Some(row));
        }
    }
    Ok(None)
}

/// Where a branch of a repo is, as planned.
pub(super) struct Source {
    /// Chauffeur's clone of the repo; none when it has none yet.
    pub(super) chauffeur: Option<PathBuf>,
    /// The user's clone holding the branch.
    pub(super) clone: Option<PathBuf>,
    /// Its checkouts in that clone: released when Chauffeur uses it, their changes copied.
    pub(super) holders: Vec<Holder>,
}

/// What the plan found, for the import.
pub(super) struct Found<'a> {
    pub(super) sources: &'a HashMap<RepoBranch, Source>,
    pub(super) sessions: &'a HashMap<String, Session>,
    /// The folders Claude Code still runs in: no branch is released from under it.
    pub(super) running: &'a [PathBuf],
}

/// The workspaces of the live branches this version imports (a branch's own, not an
/// orchestration's), and their sessions as chats; `plans` say how each went.
pub(super) async fn import(
    d: &Arc<Daemon>,
    claude: &Path,
    workspaces: &mut [ClaudeCodeWorkspacePlan],
    found: &Found<'_>,
    report: &mut Report<'_>,
) -> Result<()> {
    let git = Git::new(d.shell_env.clone());
    for workspace in workspaces
        .iter_mut()
        .filter(|w| w.kind == ClaudeCodeWorkspaceKind::Branch && w.parent.is_none())
    {
        let Some(branch) = workspace.branch.clone() else {
            continue;
        };
        let made = match existing(d, workspace, &branch)? {
            Some(row) => Ok(row),
            None => make(d, &git, workspace, &branch, found, report.warnings).await,
        };
        let row = match made {
            Ok(row) => row,
            Err(error) => {
                let reason = format!(
                    "its workspace could not be made: {}",
                    error.to_rpc().message
                );
                report.warnings.push(format!(
                    "{} could not be imported: {reason}",
                    workspace.name
                ));
                report.fail(&workspace.sessions, &reason);
                workspace.outcome = ClaudeCodeOutcome::Failed;
                workspace.reason = Some(reason);
                continue;
            }
        };
        workspace.outcome = ClaudeCodeOutcome::Imported;
        workspace.workspace = Some(row.slug.clone());
        report
            .import(d, claude, &row, &workspace.sessions, found.sessions, false)
            .await;
    }
    Ok(())
}

/// A checkout the branch was released from (or whose changes were read, the branch kept there),
/// to put back if the workspace cannot be made.
struct Released {
    at: RepoBranch,
    folder: PathBuf,
    clone: PathBuf,
    /// The branch's commit when it was released.
    head: String,
    /// Its folder is gone: its HEAD is in the clone's `.git` only.
    admin: Option<PathBuf>,
    /// What it held beyond its commit, copied into the workspace; none for commits only.
    changes: Option<Changes>,
    /// The branch is still checked out there (fetched into Chauffeur's own clone instead).
    kept: bool,
}

/// Makes the branch available in each repo's clone, the workspace on it, and its changes copied
/// in; puts back what it released if the workspace cannot be made.
async fn make(
    d: &Arc<Daemon>,
    git: &Git,
    workspace: &ClaudeCodeWorkspacePlan,
    branch: &str,
    found: &Found<'_>,
    warnings: &mut Vec<String>,
) -> Result<WorkspaceRow> {
    let mut released = Vec::new();
    let prepared = prepare(d, git, workspace, branch, found, &mut released).await;
    let created = match prepared {
        Ok(()) => {
            d.workspaces
                .create(CreateWorkspaceParams {
                    name: Some(workspace.name.clone()),
                    repos: workspace.repos.clone(),
                    branch: Some(branch.to_owned()),
                    ..Default::default()
                })
                .await
        }
        Err(error) => Err(error),
    };
    let created = match created {
        Ok(created) => created.workspace,
        Err(error) => {
            for checkout in released.iter().filter(|r| !r.kept) {
                put_back(git, checkout).await;
            }
            return Err(error);
        }
    };
    for repo in &workspace.repos {
        d.store
            .kv_set(&RepoBranch::new(repo, branch).record(), &created.id)?;
    }
    let root = PathBuf::from(&created.path);
    for checkout in &released {
        let Some(changes) = &checkout.changes else {
            continue;
        };
        let into = root.join(repo_folder(&checkout.at.repo));
        if let Err(error) = conductor::reapply(git, changes, &checkout.folder, &into).await {
            warnings.push(format!(
                "Some uncommitted changes of {} could not be copied into {} ({error:#}): they are still there.",
                checkout.folder.display(),
                created.slug
            ));
        }
    }
    d.workspaces.row(&created.id)
}

/// For each repo, the branch in the clone chosen at planning: released from the user's
/// checkouts when it is the user's clone, fetched from it into Chauffeur's own otherwise.
async fn prepare(
    d: &Daemon,
    git: &Git,
    workspace: &ClaudeCodeWorkspacePlan,
    branch: &str,
    found: &Found<'_>,
    released: &mut Vec<Released>,
) -> Result<()> {
    for repo in &workspace.repos {
        let source = found.sources.get(&RepoBranch::new(repo, branch));
        let Some(clone) = source.and_then(|s| s.clone.clone()) else {
            return Err(Error::BadRequest(format!(
                "{branch} is in none of the clones of {repo} the sessions ran in: open a session in the clone that has it, or fetch it into one, then run the import again"
            )));
        };
        let holders = source.map_or(&[][..], |s| &s.holders[..]);
        // A Claude Code session still running there would go on on a detached HEAD.
        if let Some(holder) = holders.iter().find(|h| {
            let folder = real(Path::new(&h.release.folder));
            found
                .running
                .iter()
                .any(|running| running.starts_with(&folder))
        }) {
            return Err(Error::Conflict(format!(
                "a Claude Code session still runs in {}: close it, then run the import again",
                holder.release.folder
            )));
        }
        let chauffeur = match source.and_then(|s| s.chauffeur.clone()) {
            Some(chauffeur) => chauffeur,
            None => adopt(d, repo, &clone)?,
        };
        let shared = same_folder(&chauffeur, &clone);
        if !shared {
            let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
            git.run(
                &chauffeur,
                &["fetch", "--no-tags", &clone.to_string_lossy(), &refspec],
            )
            .await
            .map_err(|e| {
                Error::Internal(e.context(format!(
                    "fetching {branch} from {} into Chauffeur's clone {}",
                    clone.display(),
                    chauffeur.display()
                )))
            })?;
        }
        for holder in holders {
            released.push(release(git, RepoBranch::new(repo, branch), holder, !shared).await?);
        }
    }
    Ok(())
}

/// Chauffeur's clone of `repo` when it has none: the user's, its folder joining the clone folders
/// when it is named after the repo, as Chauffeur's clone folders need.
fn adopt(d: &Daemon, repo: &str, clone: &Path) -> Result<PathBuf> {
    let name = repo_folder(repo);
    let named = clone
        .file_name()
        .is_some_and(|folder| folder.to_string_lossy() == name);
    let Some(root) = clone.parent().filter(|_| named) else {
        return Err(Error::BadRequest(format!(
            "Chauffeur has no clone of {repo}, and {} is not named {name}, as its clone folders need: rename the folder, or clone {repo} into a folder named {name}, then run the import again",
            clone.display()
        )));
    };
    let known = d
        .settings
        .get()
        .clone_roots
        .iter()
        .any(|r| same_folder(&expand_home(r), root));
    if !known {
        let root = root.to_string_lossy().into_owned();
        d.settings
            .update(|doc| {
                if let Some(roots) = doc
                    .entry("clone_roots")
                    .or_insert_with(|| toml_edit::value(toml_edit::Array::new()))
                    .as_array_mut()
                {
                    roots.push(root.as_str());
                }
            })
            .map_err(|e| {
                Error::Internal(e.context("adding the clone's folder to the clone folders"))
            })?;
    }
    d.repos.existing_clone(repo).ok_or_else(|| {
        Error::not_found(format!(
            "the clone of {repo} in {} (its origin must be {repo}'s)",
            root.display()
        ))
    })
}

/// Frees the branch where `holder` has it checked out, its HEAD detached on the same commit, after
/// reading what it holds; with `kept`, the branch stays there (fetched into Chauffeur's own clone)
/// and only its changes are read.
async fn release(git: &Git, at: RepoBranch, holder: &Holder, kept: bool) -> Result<Released> {
    let folder = PathBuf::from(&holder.release.folder);
    let changes = if holder.release.commits_only {
        None
    } else {
        Some(
            conductor::changes(git, &folder)
                .await
                .map_err(|e| Error::Internal(e.context("reading the uncommitted changes")))?,
        )
    };
    let head = git
        .rev_parse(&holder.clone, &format!("refs/heads/{}", at.branch))
        .await
        .map_err(Error::Internal)?;
    let admin = if holder.gone {
        // Pruned since it was read (a rollback prunes its clone's worktrees): nothing holds the
        // branch any more, nothing to release or put back.
        let Some(admin) = admin_dir(git, &holder.clone, &folder).await? else {
            return Ok(Released {
                at,
                folder,
                clone: holder.clone.clone(),
                head,
                admin: None,
                changes,
                kept: true,
            });
        };
        Some(admin)
    } else {
        None
    };
    if !kept {
        let git_dir = admin.as_ref().map(|a| format!("--git-dir={}", a.display()));
        let message = "chauffeur: branch released by the import from Claude Code";
        let args: Vec<&str> = git_dir
            .iter()
            .map(String::as_str)
            .chain(["update-ref", "--no-deref", "-m", message, "HEAD", &head])
            .collect();
        let at_dir = if admin.is_some() {
            &holder.clone
        } else {
            &folder
        };
        git.run(at_dir, &args).await.map_err(|e| {
            Error::Internal(e.context(format!("releasing {} in {}", at.branch, folder.display())))
        })?;
    }
    Ok(Released {
        at,
        folder,
        clone: holder.clone.clone(),
        head,
        admin,
        changes,
        kept,
    })
}

/// The folder in the clone's `.git` that keeps a worktree whose own folder is gone; none once
/// pruned.
async fn admin_dir(git: &Git, clone: &Path, folder: &Path) -> Result<Option<PathBuf>> {
    let common = git
        .run(
            clone,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await
        .map_err(Error::Internal)?;
    let worktrees = PathBuf::from(common.trim()).join("worktrees");
    let wanted = folder.join(".git");
    let Ok(entries) = std::fs::read_dir(&worktrees) else {
        return Ok(None);
    };
    for entry in entries.flatten() {
        let gitdir = std::fs::read_to_string(entry.path().join("gitdir")).unwrap_or_default();
        // A relative `gitdir` (`worktree.useRelativePaths`) is from its own folder.
        if entry.path().join(gitdir.trim()) == wanted {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

/// After a failed workspace: the branch checked out again where it was released.
async fn put_back(git: &Git, checkout: &Released) {
    let git_dir = checkout
        .admin
        .as_ref()
        .map(|a| format!("--git-dir={}", a.display()));
    let git_dir: Vec<&str> = git_dir.iter().map(String::as_str).collect();
    conductor::put_back_head(
        git,
        &checkout.clone,
        &checkout.folder,
        &git_dir,
        &checkout.at.branch,
        &checkout.head,
    )
    .await;
}
