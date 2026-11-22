//! The live branches' workspaces (docs/adr/claude-code-import.md): each a workspace on its branch,
//! its sessions resumable chats (`copy`). Each repo's clone is chosen at planning, once for the
//! run: Chauffeur's, else the user's holding the branch, which becomes Chauffeur's (`adopt`).
//! When that is the user's clone, the branch is released where it is checked out there (HEAD
//! detached on the same commit, its uncommitted and untracked changes copied into the workspace);
//! when Chauffeur has a clone of its own, the branch is fetched there and the user's checkouts
//! keep it. A checkout gone, or half deleted by a cleanup, carries the branch's commits only. A
//! workspace that fails to import puts back what it released; the others go on.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chauffeur_protocol::claudecode::{
    ClaudeCodeMoved, ClaudeCodeOutcome, ClaudeCodeWorkspaceKind, ClaudeCodeWorkspacePlan,
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
    existing_in(d, &workspace.repos, branch)
}

/// The workspace an earlier import made on `branch`, in one of `repos` or another.
pub(super) fn existing_in(
    d: &Daemon,
    repos: &[String],
    branch: &str,
) -> Result<Option<WorkspaceRow>> {
    for repo in repos {
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
    /// Chauffeur's clone of the repo: its own, else the user's chosen for the whole run (the
    /// first one holding one of its branches); none when no clone holds the branch.
    pub(super) chauffeur: Option<PathBuf>,
    /// `chauffeur` is the user's clone, which becomes Chauffeur's (`adopt`).
    pub(super) adopt: bool,
    /// The user's clone holding the branch.
    pub(super) clone: Option<PathBuf>,
    /// Its checkouts in that clone: released when Chauffeur uses it, their changes copied.
    pub(super) holders: Vec<Holder>,
}

/// What the plan found, for the import.
pub(super) struct Found<'a> {
    pub(super) sources: &'a HashMap<RepoBranch, Source>,
    /// The clone of each repo a workspace is made in on a new branch (a parent's, a changes
    /// one's), by repo.
    pub(super) new_branch_clones: &'a HashMap<String, RepoClone>,
    /// The user's clone holding uncommitted changes on its default branch, by repo.
    pub(super) changed: &'a BTreeMap<String, PathBuf>,
    pub(super) sessions: &'a HashMap<String, Session>,
    /// The folders Claude Code still runs in: no branch is released from under it.
    pub(super) running: &'a [PathBuf],
    /// A released checkout's changes, once copied, are saved in a ref and it is cleaned.
    pub(super) move_changes: bool,
}

/// The clone a workspace on a new branch is made from: Chauffeur's, else the user's it adopts.
pub(super) struct RepoClone {
    pub(super) clone: PathBuf,
    pub(super) adopt: bool,
}

/// The workspaces of the live branches this version imports and of the orchestrations' parents,
/// and their sessions as chats; `plans` say how each went.
pub(super) async fn import(
    d: &Arc<Daemon>,
    claude: &Path,
    workspaces: &mut [ClaudeCodeWorkspacePlan],
    found: &Found<'_>,
    report: &mut Report<'_>,
) -> Result<Vec<(PathBuf, ClaudeCodeMoved)>> {
    let git = Git::new(d.shell_env.clone());
    // The released checkouts moved out (`move_changes`), each with where its changes went.
    let mut moved = Vec::new();
    // Each parent made, by name (its children go under it); none for one that failed.
    let mut parents: HashMap<String, Option<String>> = HashMap::new();
    // Each parent's children as planned, kept with it for a later run to make those that fail.
    let mut planned: HashMap<String, Vec<PlannedChild>> = HashMap::new();
    for child in workspaces.iter() {
        if let (Some(parent), Some(branch)) = (&child.parent, &child.branch) {
            planned
                .entry(parent.clone())
                .or_default()
                .push(PlannedChild {
                    branch: branch.clone(),
                    repos: child.repos.clone(),
                });
        }
    }
    for workspace in workspaces.iter_mut() {
        // The default-branch changes it could not carry: no longer said to.
        let mut uncarried = Vec::new();
        let made = match (workspace.kind, workspace.branch.clone()) {
            (ClaudeCodeWorkspaceKind::Parent, _) => {
                let children = planned.remove(&workspace.name).unwrap_or_default();
                let made = make_parent(
                    d,
                    &git,
                    workspace,
                    children,
                    found,
                    report.warnings,
                    &mut uncarried,
                )
                .await;
                let id = made.as_ref().ok().map(|row| row.id.clone());
                parents.insert(workspace.name.clone(), id);
                made
            }
            (ClaudeCodeWorkspaceKind::Changes, _) => {
                // Carried by an earlier import: nothing more.
                let repo = workspace.repos.first().cloned().unwrap_or_default();
                if let Some(earlier) = changes_workspace(d, &repo)? {
                    workspace.outcome = ClaudeCodeOutcome::AlreadyImported;
                    workspace.workspace = Some(earlier.slug);
                    continue;
                }
                make_changes(d, &git, workspace, found, report.warnings, &mut uncarried).await
            }
            (ClaudeCodeWorkspaceKind::Branch, Some(branch)) => {
                match parent_id(&parents, workspace.parent.as_deref()) {
                    Ok(parent) => {
                        make(
                            d,
                            &git,
                            workspace,
                            &branch,
                            parent,
                            found,
                            report.warnings,
                            &mut moved,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                }
            }
            _ => continue,
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
        workspace
            .changes_from
            .retain(|repo| !uncarried.contains(repo));
        workspace.outcome = ClaudeCodeOutcome::Imported;
        workspace.workspace = Some(row.slug.clone());
        report
            .import(d, claude, &row, &workspace.sessions, found.sessions, false)
            .await;
    }
    Ok(moved)
}

/// The workspace a child goes under: its parent's, made earlier in the run; none for a plain one.
fn parent_id(
    parents: &HashMap<String, Option<String>>,
    parent: Option<&str>,
) -> Result<Option<String>> {
    let Some(name) = parent else {
        return Ok(None);
    };
    match parents.get(name) {
        Some(Some(id)) => Ok(Some(id.clone())),
        _ => Err(Error::Conflict(format!(
            "its parent workspace {name} could not be made: see why above, then run the import again"
        ))),
    }
}

/// `<prefix><session id>` → the parent an earlier import made of an orchestration session
/// ([`ParentRecord`]).
const PARENT: &str = "claudecode_parent:";

/// An orchestration's parent as made: a later run lands its session there (its chat failed) and
/// makes the children still missing.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub(super) struct ParentRecord {
    pub(super) workspace: String,
    pub(super) repo: String,
    pub(super) children: Vec<PlannedChild>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub(super) struct PlannedChild {
    pub(super) branch: String,
    pub(super) repos: Vec<String>,
}

/// The parent an earlier import made of `session`, while it is ready.
pub(super) fn recorded_parent(
    d: &Daemon,
    session: &str,
) -> Result<Option<(WorkspaceRow, ParentRecord)>> {
    let Some(text) = d.store.kv_get(&format!("{PARENT}{session}"))? else {
        return Ok(None);
    };
    let record: ParentRecord = serde_json::from_str(&text).map_err(|e| {
        Error::Internal(anyhow::Error::new(e).context("reading an imported parent"))
    })?;
    Ok(d.store
        .find_workspace(&record.workspace)?
        .filter(|row| row.state == WorkspaceState::Ready)
        .map(|row| (row, record)))
}

/// An orchestration's parent: the one an earlier import made (`workspace.workspace`), else a
/// workspace on its folder's repo, on a new branch named after it, from the clone chosen for that
/// repo (the user's becomes Chauffeur's), recorded with its `children`.
async fn make_parent(
    d: &Arc<Daemon>,
    git: &Git,
    workspace: &ClaudeCodeWorkspacePlan,
    children: Vec<PlannedChild>,
    found: &Found<'_>,
    warnings: &mut Vec<String>,
    uncarried: &mut Vec<String>,
) -> Result<WorkspaceRow> {
    if let Some(earlier) = &workspace.workspace {
        return d.workspaces.row(earlier);
    }
    let (repo, row) = make_on_new_branch(d, git, workspace, found).await?;
    for changed in &workspace.changes_from {
        if !carry_changes(d, git, changed, &row, found, warnings).await {
            uncarried.push(changed.clone());
        }
    }
    let record = ParentRecord {
        workspace: row.id.clone(),
        repo,
        children,
    };
    // Made: a record that cannot be written only makes a later run miss it.
    let written = serde_json::to_string(&record)
        .map_err(|e| Error::Internal(e.into()))
        .and_then(|text| {
            let session = workspace.sessions.first().cloned().unwrap_or_default();
            d.store.kv_set(&format!("{PARENT}{session}"), &text)
        });
    if let Err(error) = written {
        warnings.push(format!(
            "{} was made, but could not be recorded ({error}): a later import will not make its missing children.",
            row.slug
        ));
    }
    Ok(row)
}

/// `<prefix><repo>` → the workspace an earlier import made carrying the uncommitted changes on
/// the repo's default branch (a parent's, a changes one's): a later run makes none again.
const CHANGES: &str = "claudecode_changes:";

/// The workspace an earlier import made carrying `repo`'s default-branch changes, while ready.
pub(super) fn changes_workspace(d: &Daemon, repo: &str) -> Result<Option<WorkspaceRow>> {
    let Some(id) = d.store.kv_get(&format!("{CHANGES}{repo}"))? else {
        return Ok(None);
    };
    Ok(d.store
        .find_workspace(&id)?
        .filter(|row| row.state == WorkspaceState::Ready))
}

/// A workspace on a new branch from the default branch of its repo, carrying the uncommitted
/// changes of the user's clone there (which keeps its own).
async fn make_changes(
    d: &Arc<Daemon>,
    git: &Git,
    workspace: &ClaudeCodeWorkspacePlan,
    found: &Found<'_>,
    warnings: &mut Vec<String>,
    uncarried: &mut Vec<String>,
) -> Result<WorkspaceRow> {
    let (repo, row) = make_on_new_branch(d, git, workspace, found).await?;
    if !carry_changes(d, git, &repo, &row, found, warnings).await {
        uncarried.push(repo);
    }
    Ok(row)
}

/// A workspace on a new branch of its repo (a parent's, a changes one's), from the clone chosen
/// for it (the user's becomes Chauffeur's): from the default branch; when it carries the user's
/// default-branch changes, from the commit their clone stands on, where those changes apply.
async fn make_on_new_branch(
    d: &Arc<Daemon>,
    git: &Git,
    workspace: &ClaudeCodeWorkspacePlan,
    found: &Found<'_>,
) -> Result<(String, WorkspaceRow)> {
    let Some(repo) = workspace.repos.first().cloned() else {
        return Err(Error::Internal(anyhow::anyhow!(
            "a workspace planned with no repo"
        )));
    };
    let Some(chosen) = found.new_branch_clones.get(&repo) else {
        return Err(Error::BadRequest(format!(
            "none of the clones of {repo} the sessions ran in is left: open a session in one, then run the import again"
        )));
    };
    if chosen.adopt {
        adopt(d, &repo, &chosen.clone)?;
    }
    let changed = found
        .changed
        .get(&repo)
        .filter(|_| workspace.changes_from.contains(&repo));
    let branch = match changed {
        Some(from) => Some(branch_at(d, git, &chosen.clone, from, &workspace.name).await?),
        None => None,
    };
    let made = d
        .workspaces
        .create(CreateWorkspaceParams {
            name: Some(workspace.name.clone()),
            repos: vec![repo.clone()],
            branch: branch.clone(),
            ..Default::default()
        })
        .await;
    match made {
        Ok(made) => Ok((repo, d.workspaces.row(&made.workspace.id)?)),
        Err(error) => {
            // Made for it: it goes with it.
            if let Some(branch) = &branch {
                let _ = git.delete_branch(&chosen.clone, branch).await;
            }
            Err(error)
        }
    }
}

/// A new branch in `clone`, named as a workspace `name`'s would be, at the commit the user's
/// clone `from` stands on (fetched from it, when another clone).
async fn branch_at(d: &Daemon, git: &Git, clone: &Path, from: &Path, name: &str) -> Result<String> {
    let base = format!("{}/{name}", d.workspaces.branch_prefix().await);
    let mut branch = base.clone();
    for n in 2.. {
        if !git.branch_exists(clone, &branch).await {
            break;
        }
        branch = format!("{base}-{n}");
    }
    let from_text = from.to_string_lossy();
    git.run(
        clone,
        &[
            "fetch",
            "--no-tags",
            &from_text,
            &format!("HEAD:refs/heads/{branch}"),
        ],
    )
    .await
    .map_err(|e| {
        Error::Internal(e.context(format!("starting {branch} where {} stands", from.display())))
    })?;
    Ok(branch)
}

/// The uncommitted changes on `repo`'s default branch in the user's clone, copied into `row`'s
/// worktree of it, the clone keeping them; recorded, so that a later run copies them no more.
/// What cannot be copied is said; true when all were.
async fn carry_changes(
    d: &Daemon,
    git: &Git,
    repo: &str,
    row: &WorkspaceRow,
    found: &Found<'_>,
    warnings: &mut Vec<String>,
) -> bool {
    let Some(from) = found.changed.get(repo) else {
        return false;
    };
    // Carried by an earlier import: not again.
    if changes_workspace(d, repo).is_ok_and(|earlier| earlier.is_some_and(|e| e.id != row.id)) {
        return false;
    }
    let into = PathBuf::from(&row.path).join(repo_folder(repo));
    let copied = async {
        let changes = conductor::changes(git, from).await?;
        conductor::reapply(git, &changes, from, &into).await
    }
    .await;
    if let Err(error) = &copied {
        warnings.push(format!(
            "Some uncommitted changes on the default branch of {} could not be copied into {} ({error:#}): copy them there by hand, or archive {} and run the import again.",
            from.display(),
            row.slug,
            row.slug
        ));
    }
    if let Err(error) = d.store.kv_set(&format!("{CHANGES}{repo}"), &row.id) {
        warnings.push(format!(
            "{} was made, but could not be recorded ({error}): a later import will make another.",
            row.slug
        ));
    }
    copied.is_ok()
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
/// released for a repo that could not be added is put back.
async fn make(
    d: &Arc<Daemon>,
    git: &Git,
    workspace: &ClaudeCodeWorkspacePlan,
    branch: &str,
    parent: Option<String>,
    found: &Found<'_>,
    warnings: &mut Vec<String>,
    moved: &mut Vec<(PathBuf, ClaudeCodeMoved)>,
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
    // The repos now in a workspace on the branch: all of them, or none, or (an extension failing
    // on one) those added before; what was released for the others is put back.
    let (made, added) = match (prepared, &existing) {
        (Err(error), _) => (Err(error), Vec::new()),
        (Ok(()), Some(row)) => {
            let (added, extended) = extend(d, row, &missing).await;
            (extended.map(|()| row.id.clone()), added)
        }
        (Ok(()), None) => d
            .workspaces
            .create(CreateWorkspaceParams {
                name: Some(workspace.name.clone()),
                repos: workspace.repos.clone(),
                branch: Some(branch.to_owned()),
                parent,
                ..Default::default()
            })
            .await
            .map_or_else(
                |error| (Err(error), Vec::new()),
                |created| (Ok(created.workspace.id), missing.clone()),
            ),
    };
    for checkout in released
        .iter()
        .filter(|r| !r.kept && !added.contains(&r.at.repo))
    {
        put_back(git, checkout).await;
    }
    let (id, failed) = match (made, &existing) {
        (Ok(id), _) => (id, None),
        (Err(error), Some(row)) if !added.is_empty() => (row.id.clone(), Some(error)),
        (Err(error), _) => return Err(error),
    };
    let row = d.workspaces.row(&id)?;
    // Made: a record that cannot be written only makes a later run miss it.
    let records = added
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
    for checkout in released.iter().filter(|r| added.contains(&r.at.repo)) {
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
            continue;
        }
        // Copied: with `move_changes`, the checkout keeps them no more (a released one only).
        if !found.move_changes || checkout.kept {
            continue;
        }
        let base = d
            .store
            .worktrees_of(&row.id)?
            .into_iter()
            .find(|w| w.repo == repo_folder(&checkout.at.repo))
            .map(|w| w.base_branch)
            .unwrap_or_default();
        let out = super::moved::move_out(
            d,
            git,
            &checkout.folder,
            &checkout.clone,
            &checkout.at.repo,
            &base,
            &row,
        )
        .await;
        match out {
            Ok(done) => moved.push((checkout.folder.clone(), done)),
            Err(error) => warnings.push(format!(
                "The changes of {} are in {}, but it could not be cleaned ({error:#}): clean it by hand once you no longer need them there.",
                checkout.folder.display(),
                row.slug
            )),
        }
    }
    match failed {
        Some(error) => Err(error),
        None => Ok(row),
    }
}

/// The repos an earlier import's workspace lacks, added to it on its branch in order until one
/// fails: those added, and how it went.
async fn extend(
    d: &Arc<Daemon>,
    row: &WorkspaceRow,
    repos: &[String],
) -> (Vec<String>, Result<()>) {
    let mut added = Vec::new();
    for repo in repos {
        if let Err(error) = d.workspaces.add_repo(&row.id, repo).await {
            return (added, Err(error));
        }
        added.push(repo.clone());
    }
    (added, Ok(()))
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
        let chauffeur = source
            .and_then(|s| s.chauffeur.clone())
            .unwrap_or_else(|| clone.clone());
        if source.is_some_and(|s| s.adopt) {
            adopt(d, repo, &chauffeur)?;
        }
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
