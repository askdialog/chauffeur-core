//! The live branches' workspaces (docs/adr/claude-code-import.md): each a workspace on its branch,
//! its sessions resumable chats (`copy`). The branch comes from the user's clone. When Chauffeur
//! uses that clone, the branch is released where it is checked out (HEAD detached on the same
//! commit, its uncommitted and untracked changes copied into the workspace); when Chauffeur has a
//! clone of its own, the branch is fetched there and the user's checkout keeps it, its changes
//! copied too. A checkout gone, or half deleted by a cleanup, carries the branch's commits only.
//! A workspace that fails to import puts back what it released; the others go on.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use chauffeur_protocol::claudecode::{
    ClaudeCodeOutcome, ClaudeCodeSessionPlan, ClaudeCodeWorkspaceKind, ClaudeCodeWorkspacePlan,
};
use chauffeur_protocol::client::CreateWorkspaceParams;
use chauffeur_protocol::model::WorkspaceState;

use super::sessions::Session;
use super::{Holder, copy};
use crate::conductor::{self, Changes, same_folder};
use crate::git::Git;
use crate::repos::repo_folder;
use crate::store::WorkspaceRow;
use crate::{Daemon, Error, Result};

/// `<prefix><repo>\n<branch>` → the id of the workspace an import made on that branch.
const BRANCH: &str = "claudecode_branch:";

fn key(repo: &str, branch: &str) -> String {
    format!("{BRANCH}{repo}\n{branch}")
}

/// The workspace an earlier import made on `branch` of `repo`, while it is ready: the sessions
/// found on that branch since go there.
pub(super) fn recorded(d: &Daemon, repo: &str, branch: &str) -> Result<Option<WorkspaceRow>> {
    let Some(id) = d.store.kv_get(&key(repo, branch))? else {
        return Ok(None);
    };
    Ok(d.store
        .find_workspace(&id)?
        .filter(|row| row.state == WorkspaceState::Ready))
}

/// Where a branch is: the user's clone holding it, and its checkouts there.
pub(super) struct Source {
    pub(super) clone: Option<PathBuf>,
    pub(super) holders: Vec<Holder>,
}

/// The workspaces of the live branches this version imports (a branch's own, not an
/// orchestration's), and their sessions as chats; `plans` say how each went.
pub(super) async fn import(
    d: &Arc<Daemon>,
    claude: &Path,
    workspaces: &mut [ClaudeCodeWorkspacePlan],
    sources: &HashMap<(String, String), Source>,
    plans: &mut [ClaudeCodeSessionPlan],
    sessions: &HashMap<String, Session>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let git = Git::new(d.shell_env.clone());
    let index: BTreeMap<String, usize> = plans
        .iter()
        .enumerate()
        .map(|(i, plan)| (plan.id.clone(), i))
        .collect();
    for workspace in workspaces
        .iter_mut()
        .filter(|w| w.kind == ClaudeCodeWorkspaceKind::Branch && w.parent.is_none())
    {
        let Some(branch) = workspace.branch.clone() else {
            continue;
        };
        let made = match existing(d, workspace, &branch)? {
            Some(row) => Ok(row),
            None => make(d, &git, workspace, &branch, sources, warnings).await,
        };
        let row = match made {
            Ok(row) => row,
            Err(error) => {
                let reason = format!(
                    "its workspace could not be made: {}",
                    error.to_rpc().message
                );
                warnings.push(format!(
                    "{} could not be imported: {reason}",
                    workspace.name
                ));
                workspace.outcome = ClaudeCodeOutcome::Failed;
                workspace.reason = Some(reason.clone());
                for id in &workspace.sessions {
                    if let Some(&i) = index.get(id) {
                        plans[i].outcome = ClaudeCodeOutcome::Failed;
                        plans[i].reason = Some(reason.clone());
                    }
                }
                continue;
            }
        };
        workspace.outcome = ClaudeCodeOutcome::Imported;
        workspace.workspace = Some(row.slug.clone());
        for id in &workspace.sessions {
            let (Some(&i), Some(session)) = (index.get(id), sessions.get(id)) else {
                continue;
            };
            let plan = &mut plans[i];
            plan.lands_in = Some(row.slug.clone());
            match copy::import_session(d, claude, &row, session, false).await {
                Ok(true) => plan.outcome = ClaudeCodeOutcome::Imported,
                Ok(false) => plan.outcome = ClaudeCodeOutcome::AlreadyImported,
                Err(error) => {
                    warnings.push(format!("“{}” could not be imported: {error}", plan.title));
                    plan.outcome = ClaudeCodeOutcome::Failed;
                    plan.reason = Some(error.to_string());
                }
            }
        }
    }
    Ok(())
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

/// A checkout the branch was released from, to put back if the workspace cannot be made.
struct Released {
    repo: String,
    branch: String,
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

/// Makes the branch available in Chauffeur's clone of each repo, the workspace on it, and its
/// changes copied in; puts back what it released if the workspace cannot be made.
async fn make(
    d: &Arc<Daemon>,
    git: &Git,
    workspace: &ClaudeCodeWorkspacePlan,
    branch: &str,
    sources: &HashMap<(String, String), Source>,
    warnings: &mut Vec<String>,
) -> Result<WorkspaceRow> {
    let mut released = Vec::new();
    let prepared = prepare(d, git, workspace, branch, sources, &mut released).await;
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
        d.store.kv_set(&key(repo, branch), &created.id)?;
    }
    let root = PathBuf::from(&created.path);
    for checkout in &released {
        let Some(changes) = &checkout.changes else {
            continue;
        };
        let into = root.join(repo_folder(&checkout.repo));
        if let Err(error) = conductor::reapply(git, changes, &checkout.folder, &into).await {
            warnings.push(format!(
                "The uncommitted changes of {} could not all be copied into {} ({error:#}): they are still there.",
                checkout.folder.display(),
                created.slug
            ));
        }
    }
    d.workspaces.row(&created.id)
}

/// For each repo, the branch in Chauffeur's clone: released from the user's checkouts when it is
/// the user's clone, fetched from it otherwise.
async fn prepare(
    d: &Daemon,
    git: &Git,
    workspace: &ClaudeCodeWorkspacePlan,
    branch: &str,
    sources: &HashMap<(String, String), Source>,
    released: &mut Vec<Released>,
) -> Result<()> {
    for repo in &workspace.repos {
        let source = sources.get(&(repo.clone(), branch.to_owned()));
        let Some(clone) = source.and_then(|s| s.clone.clone()) else {
            return Err(Error::BadRequest(format!(
                "{branch} is in none of the clones of {repo} found"
            )));
        };
        let holders = source.map_or(&[][..], |s| &s.holders[..]);
        let chauffeur = match d.repos.existing_clone(repo) {
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
        for holder in holders.iter().filter(|h| same_folder(&h.clone, &clone)) {
            released.push(release(git, repo, branch, holder, !shared).await?);
        }
    }
    Ok(())
}

/// Chauffeur's clone of `repo` when it has none: the user's, its folder joining the clone folders
/// when it is named after the repo, as Chauffeur's clone folders need.
fn adopt(d: &Daemon, repo: &str, clone: &Path) -> Result<PathBuf> {
    let named = clone
        .file_name()
        .is_some_and(|name| name.to_string_lossy() == repo);
    let Some(root) = clone.parent().filter(|_| named) else {
        return Err(Error::BadRequest(format!(
            "Chauffeur has no clone of {repo}, and {} is not named {repo}, as its clone folders need: name its clone in the settings ([repos.\"{repo}\"] path)",
            clone.display()
        )));
    };
    let root = root.to_string_lossy().into_owned();
    d.settings
        .update(|doc| {
            let roots = doc
                .entry("clone_roots")
                .or_insert_with(|| toml_edit::value(toml_edit::Array::new()))
                .as_array_mut();
            if let Some(roots) = roots
                && !roots.iter().any(|r| r.as_str() == Some(root.as_str()))
            {
                roots.push(root.as_str());
            }
        })
        .map_err(|e| {
            Error::Internal(e.context("adding the clone's folder to the clone folders"))
        })?;
    d.repos
        .existing_clone(repo)
        .ok_or_else(|| Error::not_found(format!("the clone of {repo} in {root}")))
}

/// Frees the branch where `holder` has it checked out, its HEAD detached on the same commit, after
/// reading what it holds; with `kept`, the branch stays there (fetched into Chauffeur's own clone)
/// and only its changes are read.
async fn release(
    git: &Git,
    repo: &str,
    branch: &str,
    holder: &Holder,
    kept: bool,
) -> Result<Released> {
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
        .rev_parse(&holder.clone, &format!("refs/heads/{branch}"))
        .await
        .map_err(Error::Internal)?;
    let admin = if holder.gone {
        Some(admin_dir(git, &holder.clone, &folder).await?)
    } else {
        None
    };
    if !kept {
        let message = "chauffeur: branch released by the import from Claude Code";
        let detach = ["update-ref", "--no-deref", "-m", message, "HEAD", &head];
        match &admin {
            Some(admin) => {
                let git_dir = format!("--git-dir={}", admin.display());
                let args: Vec<&str> = std::iter::once(git_dir.as_str()).chain(detach).collect();
                git.run(&holder.clone, &args).await
            }
            None => git.run(&folder, &detach).await,
        }
        .map_err(|e| {
            Error::Internal(e.context(format!("releasing {branch} in {}", folder.display())))
        })?;
    }
    Ok(Released {
        repo: repo.to_owned(),
        branch: branch.to_owned(),
        folder,
        clone: holder.clone.clone(),
        head,
        admin,
        changes,
        kept,
    })
}

/// The folder in the clone's `.git` that keeps a worktree whose own folder is gone.
async fn admin_dir(git: &Git, clone: &Path, folder: &Path) -> Result<PathBuf> {
    let common = git
        .run(
            clone,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await
        .map_err(Error::Internal)?;
    let worktrees = PathBuf::from(common.trim()).join("worktrees");
    let wanted = folder.join(".git");
    for entry in std::fs::read_dir(&worktrees)
        .with_context(|| format!("reading {}", worktrees.display()))?
        .flatten()
    {
        let gitdir = std::fs::read_to_string(entry.path().join("gitdir")).unwrap_or_default();
        if Path::new(gitdir.trim()) == wanted {
            return Ok(entry.path());
        }
    }
    Err(Error::not_found(format!(
        "the worktree of {} in {}",
        folder.display(),
        worktrees.display()
    )))
}

/// After a failed workspace: the branch checked out again where it was released.
async fn put_back(git: &Git, checkout: &Released) {
    let Some(admin) = &checkout.admin else {
        conductor::put_back(
            git,
            &checkout.clone,
            &checkout.folder,
            &checkout.branch,
            &checkout.head,
        )
        .await;
        return;
    };
    let reference = format!("refs/heads/{}", checkout.branch);
    if !git
        .ok(
            &checkout.clone,
            &["rev-parse", "-q", "--verify", &reference],
        )
        .await
        && let Err(error) = git
            .run(
                &checkout.clone,
                &[
                    "branch",
                    "--end-of-options",
                    &checkout.branch,
                    &checkout.head,
                ],
            )
            .await
    {
        tracing::error!(%error, branch = checkout.branch, "could not make a released branch again");
    }
    let git_dir = format!("--git-dir={}", admin.display());
    if let Err(error) = git
        .run(
            &checkout.clone,
            &[&git_dir, "symbolic-ref", "HEAD", &reference],
        )
        .await
    {
        tracing::error!(%error, branch = checkout.branch, "could not attach a gone worktree to its branch again");
    }
}
