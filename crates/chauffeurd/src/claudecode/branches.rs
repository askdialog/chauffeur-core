//! The live branches' workspaces (docs/adr/claude-code-import.md): each a workspace on its branch,
//! its sessions resumable chats (`copy`). Each repo's clone is chosen at planning, once for the
//! run: Chauffeur's, else the user's holding the branch, which becomes Chauffeur's (`adopt`).
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
/// The same branch in several repos is one workspace, across runs too: one made for the branch
/// in another repo.
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
    let Some(id) = d.store.kv_get(&branch_record(branch))? else {
        return Ok(None);
    };
    Ok(d.store
        .find_workspace(&id)?
        .filter(|row| row.state == WorkspaceState::Ready))
}

/// `claudecode_branch_name:<branch>` → the id of the workspace an import made on it, in any repo.
fn branch_record(branch: &str) -> String {
    format!("claudecode_branch_name:{branch}")
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
        let made = make(d, &git, workspace, &branch, found, report.warnings).await;
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

/// The repos of `workspace` an earlier import's workspace on the branch lacks (all of them when
/// there is none): a later run found the branch in another repo too.
pub(super) fn missing_repos(
    d: &Daemon,
    workspace: &ClaudeCodeWorkspacePlan,
    branch: &str,
    existing: Option<&WorkspaceRow>,
) -> Result<Vec<String>> {
    let mut missing = Vec::new();
    for repo in &workspace.repos {
        let has =
            recorded(d, repo, branch)?.is_some_and(|row| existing.is_some_and(|e| e.id == row.id));
        if !has {
            missing.push(repo.clone());
        }
    }
    Ok(missing)
}

/// The branch's workspace: an earlier import's, the repos it lacks added; else a new one. The
/// branch is made available in each new repo's clone and its changes copied in; what was
/// released is put back if the workspace cannot be made or extended.
async fn make(
    d: &Arc<Daemon>,
    git: &Git,
    workspace: &ClaudeCodeWorkspacePlan,
    branch: &str,
    found: &Found<'_>,
    warnings: &mut Vec<String>,
) -> Result<WorkspaceRow> {
    let existing = existing(d, workspace, branch)?;
    let missing = missing_repos(d, workspace, branch, existing.as_ref())?;
    if let Some(row) = &existing
        && missing.is_empty()
    {
        return Ok(row.clone());
    }
    let mut released = Vec::new();
    let prepared = prepare(d, git, &missing, branch, found, &mut released).await;
    let made = match (prepared, &existing) {
        (Err(error), _) => Err(error),
        (Ok(()), Some(row)) => extend(d, row, &missing).await.map(|()| row.id.clone()),
        (Ok(()), None) => d
            .workspaces
            .create(CreateWorkspaceParams {
                name: Some(workspace.name.clone()),
                repos: workspace.repos.clone(),
                branch: Some(branch.to_owned()),
                ..Default::default()
            })
            .await
            .map(|created| created.workspace.id),
    };
    let id = match made {
        Ok(id) => id,
        Err(error) => {
            for checkout in released.iter().filter(|r| !r.kept) {
                put_back(git, checkout).await;
            }
            return Err(error);
        }
    };
    let row = d.workspaces.row(&id)?;
    // Made: a record that cannot be written only makes a later run miss it.
    let records = missing
        .iter()
        .map(|repo| RepoBranch::new(repo, branch).record())
        .chain(std::iter::once(branch_record(branch)));
    for record in records {
        if let Err(error) = d.store.kv_set(&record, &row.id) {
            warnings.push(format!(
                "{} was made, but could not be recorded ({error}): a later import will not add to it.",
                row.slug
            ));
        }
    }
    let root = PathBuf::from(&row.path);
    for checkout in &released {
        let Some(changes) = &checkout.changes else {
            continue;
        };
        let into = root.join(repo_folder(&checkout.at.repo));
        if let Err(error) = conductor::reapply(git, changes, &checkout.folder, &into).await {
            warnings.push(format!(
                "Some uncommitted changes of {} could not be copied into {} ({error:#}): they are still there.",
                checkout.folder.display(),
                row.slug
            ));
        }
    }
    Ok(row)
}

/// The repos an earlier import's workspace lacks, added to it on its branch.
async fn extend(d: &Arc<Daemon>, row: &WorkspaceRow, repos: &[String]) -> Result<()> {
    for repo in repos {
        d.workspaces.add_repo(&row.id, repo).await?;
    }
    Ok(())
}

/// For each repo, the branch in the clone chosen at planning: released from the user's
/// checkouts when it is the user's clone, fetched from it into Chauffeur's own otherwise.
async fn prepare(
    d: &Daemon,
    git: &Git,
    repos: &[String],
    branch: &str,
    found: &Found<'_>,
    released: &mut Vec<Released>,
) -> Result<()> {
    for repo in repos {
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

/// Chauffeur's clone of `repo` when it has none: the user's. A repo of the main organization gets
/// it named in the settings (`[repos.<name>] path`), for it alone; a repo named with its owner,
/// which Chauffeur finds by its folder's name and origin, gets its folder among the clone
/// folders (Chauffeur's own clones of such repos are read first).
fn adopt(d: &Daemon, repo: &str, clone: &Path) -> Result<PathBuf> {
    let name = repo_folder(repo);
    let text = clone.to_string_lossy().into_owned();
    let written = if repo == name {
        d.settings.update(|doc| {
            let repos = conductor::implicit_table(doc.as_table_mut(), "repos");
            conductor::implicit_table(repos, name).insert("path", toml_edit::value(text.as_str()));
        })
    } else {
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
        let root = root.to_string_lossy().into_owned();
        d.settings.update(|doc| {
            if known {
                return;
            }
            if let Some(roots) = doc
                .entry("clone_roots")
                .or_insert_with(|| toml_edit::value(toml_edit::Array::new()))
                .as_array_mut()
            {
                roots.push(root.as_str());
            }
        })
    };
    written.map_err(|e| {
        Error::Internal(e.context(format!("naming {repo}'s clone in the settings")))
    })?;
    d.repos.existing_clone(repo).ok_or_else(|| {
        Error::not_found(format!(
            "{repo}'s clone at {text} (its origin must be {repo}'s)"
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
