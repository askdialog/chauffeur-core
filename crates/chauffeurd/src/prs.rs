//! Pull requests: one per repo of a workspace, read with `gh`, opened by the agent on request
//! (with `open-pr` and `shepherd-pr` skills when installed), merged by the user one repo after
//! the other.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use std::time::{Duration, Instant};

use chauffeur_protocol::client::{MergeMethod, MergeOutcome, MergePrsParams, PrInfo, PrRef};
use chauffeur_protocol::model::{WorkspaceState, WorktreeOrigin};
use serde_json::Value;

use crate::events::Activity;
use crate::git::Git;
use crate::shellenv::ShellEnv;
use crate::store::{Store, WorktreeRow};
use crate::{Daemon, Error, Result};

/// A workspace's `status` read lately, or being read.
type StatusCell = Arc<tokio::sync::OnceCell<Vec<PrInfo>>>;

fn status_cache() -> &'static Mutex<HashMap<String, (Instant, StatusCell)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, StatusCell)>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// `status` for `pr.status`: the Office and the workspace view ask for the same workspaces within
/// seconds of each other, and each read runs git and `gh` in every worktree. The calls that arrive
/// while one reads wait for its answer, which then serves for `STATUS_FRESH`, until a turn of the
/// workspace ends or a merge (`forget_status`).
pub async fn cached_status(daemon: &Arc<Daemon>, workspace: &str) -> Result<Vec<PrInfo>> {
    let id = daemon.workspaces.row(workspace)?.id;
    let cell = {
        let mut cache = status_cache().lock().unwrap();
        cache.retain(|_, (at, _)| at.elapsed() < STATUS_FRESH);
        cache
            .entry(id.clone())
            .or_insert_with(|| (Instant::now(), Default::default()))
            .1
            .clone()
    };
    cell.get_or_try_init(|| status(daemon, &id)).await.cloned()
}

/// The next `cached_status` of the workspace reads it again.
pub fn forget_status(workspace: &str) {
    status_cache().lock().unwrap().remove(workspace);
}

pub async fn status(daemon: &Arc<Daemon>, workspace: &str) -> Result<Vec<PrInfo>> {
    let row = daemon.workspaces.row(workspace)?;
    // The branch each worktree is on, and the worktrees made with plain git, before they are read.
    if let Err(error) = crate::worktree_sync::sync(daemon, &row.id).await {
        tracing::warn!(workspace = %row.id, "could not sync the worktrees: {error:#}");
    }
    let git = Git::new(daemon.shell_env.clone());
    // The workspace's own worktrees, then those its agents made on their own branches (one per
    // sub-ticket, say), whose PRs are the workspace's too.
    let mut worktrees: Vec<WorktreeRow> = daemon
        .store
        .worktrees_of(&row.id)?
        .into_iter()
        .filter(|w| match w.origin {
            WorktreeOrigin::Workspace => !w.removed || row.state == WorkspaceState::Archived,
            _ => !w.removed && !w.branch.is_empty(),
        })
        .collect();
    worktrees.sort_by(|a, b| {
        (a.origin != WorktreeOrigin::Workspace, &a.group, &a.repo).cmp(&(
            b.origin != WorktreeOrigin::Workspace,
            &b.group,
            &b.repo,
        ))
    });
    let futures = worktrees
        .iter()
        .map(|w| repo_status(&daemon.store, &git, &daemon.shell_env, w));
    let read = futures_util::future::join_all(futures).await;
    // A gh call that failed reads as no PR: only a whole read is reported, or a hook would take
    // the workspace for one without PRs.
    let complete = read.iter().all(|(_, read)| *read);
    let prs: Vec<PrInfo> = read.into_iter().map(|(info, _)| info).collect();
    if complete {
        daemon.events.report(Activity::PrsRead {
            workspace: &row.id,
            prs: &prs,
        });
        daemon.search.record_pull_requests(&row.id, &prs);
    }
    Ok(prs)
}

/// The worktree's PR, and whether GitHub answered (a failed `gh` call reads as no PR).
async fn repo_status(
    store: &Store,
    git: &Git,
    env: &ShellEnv,
    worktree: &WorktreeRow,
) -> (PrInfo, bool) {
    let dir = Path::new(&worktree.path);
    let mut info = PrInfo {
        repo: worktree.repo.clone(),
        group: (worktree.origin != WorktreeOrigin::Workspace)
            .then(|| worktree.group.clone())
            .flatten(),
        branch: worktree.branch.clone(),
        base_branch: worktree.base_branch.clone(),
        ahead: 0,
        dirty: false,
        number: None,
        pr_base_branch: None,
        url: None,
        state: None,
        title: None,
        is_draft: false,
        checks: None,
        review_decision: None,
        mergeable: None,
        merge_methods: Vec::new(),
        can_merge: None,
    };
    if !worktree.removed && dir.exists() {
        let base = format!("origin/{}", worktree.base_branch);
        info.ahead = git
            .run(dir, &["rev-list", "--count", &format!("{base}..HEAD")])
            .await
            .ok()
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0);
        info.dirty = git
            .run(dir, &["status", "--porcelain"])
            .await
            .is_ok_and(|s| !s.trim().is_empty());
    }
    // A detached checkout (an exploration's) has no branch, so no pull request.
    if worktree.branch.is_empty() {
        return (info, true);
    }
    let found = find_pr_checked(store, git, env, worktree, "all", STATUS_FIELDS).await;
    let answered = found.is_ok();
    if let Ok(Some((pr, _))) = found {
        info.number = pr["number"].as_u64();
        info.pr_base_branch = pr["baseRefName"].as_str().map(String::from);
        info.url = pr["url"].as_str().map(String::from);
        info.state = pr["state"].as_str().map(String::from);
        info.title = pr["title"].as_str().map(String::from);
        info.is_draft = pr["isDraft"].as_bool().unwrap_or(false);
        info.checks = combined_checks(&pr["statusCheckRollup"]);
        info.review_decision = pr["reviewDecision"]
            .as_str()
            .filter(|d| !d.is_empty())
            .map(String::from);
        info.mergeable = pr["mergeable"].as_str().map(String::from);
        if info.state.as_deref() == Some("OPEN") {
            let repo = merge_rights(env, Path::new(&worktree.clone_path)).await;
            info.merge_methods = repo.methods;
            info.can_merge = repo.can_merge;
        }
    }
    (info, answered)
}

/// What the header shows of a PR.
const STATUS_FIELDS: &str =
    "number,baseRefName,url,state,title,isDraft,statusCheckRollup,reviewDecision,mergeable";

/// What tells a worktree's PR from another's: its head branch and commit, and whether it comes
/// from a fork.
const IDENTITY_FIELDS: &str = "headRefName,headRefOid,isCrossRepository";

/// Branches shared by everyone, whatever the repo: a worktree tracking one of them (cut from
/// `origin/main`, say) has not been pushed there, and their PRs are nobody's in particular.
const SHARED_BRANCHES: &[&str] = &[
    "main",
    "master",
    "staging",
    "develop",
    "development",
    "dev",
    "trunk",
    "production",
    "prod",
    "release",
    "gh-pages",
];

/// How a worktree's PR was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// Under the worktree's branch name.
    LocalName,
    /// Under the name its branch was pushed under (`git push -u origin HEAD:<name>`).
    Upstream,
}

/// The PR of a worktree's branch in `state` (`all`, `merged`…), with `fields` (and
/// [`IDENTITY_FIELDS`]). First the PR GitHub has under the branch's name, as always (a workspace
/// resuming a PR checks its branch out under that name): the newest that is not from a fork.
/// Without one, the PR under the name the branch was pushed under, only when that name is
/// nobody else's (see [`pushed_name`]), the PR is from the repo itself and its head commit is in
/// the local branch (the PR head is the local tip, or behind it). A PR opened from another
/// workspace is never returned: when in doubt (a git or gh error, several PRs), none is.
pub async fn find_pr(
    store: &Store,
    git: &Git,
    env: &ShellEnv,
    worktree: &WorktreeRow,
    state: &str,
    fields: &str,
) -> Option<(Value, Found)> {
    find_pr_checked(store, git, env, worktree, state, fields)
        .await
        .ok()
        .flatten()
}

/// [`find_pr`], telling a `gh` call that failed (`Err`) from no PR (`Ok(None)`).
pub(crate) async fn find_pr_checked(
    store: &Store,
    git: &Git,
    env: &ShellEnv,
    worktree: &WorktreeRow,
    state: &str,
    fields: &str,
) -> std::result::Result<Option<(Value, Found)>, ()> {
    let branch = worktree.branch.as_str();
    if branch.is_empty() {
        return Ok(None);
    }
    let clone = Path::new(&worktree.clone_path);
    let fields = format!("{fields},{IDENTITY_FIELDS}");
    let prs = gh_pr_list(env, clone, branch, state, &fields)
        .await
        .ok_or(())?;
    if let Some(pr) = prs.into_iter().find(|pr| {
        pr["isCrossRepository"].as_bool() != Some(true)
            && pr["headRefName"].as_str().is_none_or(|head| head == branch)
    }) {
        return Ok(Some((pr, Found::LocalName)));
    }
    let Some(name) = pushed_name(store, git, clone, worktree).await else {
        return Ok(None);
    };
    let prs = gh_pr_list(env, clone, &name, state, &fields)
        .await
        .ok_or(())?;
    let local = format!("refs/heads/{branch}");
    let mut ours = Vec::new();
    for pr in prs {
        if pr["headRefName"].as_str() != Some(name.as_str())
            || pr["isCrossRepository"].as_bool() != Some(false)
        {
            continue;
        }
        let Some(head) = pr["headRefOid"].as_str().filter(|oid| is_object_id(oid)) else {
            continue;
        };
        // Exits 0 only when the head is the local tip or behind it; 1 when not, 128 when git
        // does not know that commit: both mean not ours.
        if git
            .ok(clone, &["merge-base", "--is-ancestor", head, &local])
            .await
        {
            ours.push(pr);
        }
    }
    Ok(only_one(ours).map(|pr| (pr, Found::Upstream)))
}

/// One PR of several found through the pushed-under name: the only one, else the only open one
/// (a PR closed, then opened again from the same branch); anything else is ambiguous.
fn only_one(mut prs: Vec<Value>) -> Option<Value> {
    if prs.len() == 1 {
        return prs.pop();
    }
    let mut open = prs
        .into_iter()
        .filter(|pr| pr["state"].as_str() == Some("OPEN"));
    let pr = open.next()?;
    open.next().is_none().then_some(pr)
}

/// A full commit id, so never an option nor a ref name.
fn is_object_id(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `gh pr list --head <head>`; `None` when gh fails or answers something else than a list.
async fn gh_pr_list(
    env: &ShellEnv,
    clone: &Path,
    head: &str,
    state: &str,
    fields: &str,
) -> Option<Vec<Value>> {
    let output = tokio::process::Command::new("gh")
        .args([
            "pr", "list", "--head", head, "--state", state, "--limit", "10", "--json", fields,
        ])
        .current_dir(clone)
        .env_clear()
        .envs(env.vars())
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .ok()
        .filter(|output| output.status.success())?;
    match serde_json::from_slice(&output.stdout) {
        Ok(Value::Array(prs)) => Some(prs),
        _ => None,
    }
}

/// A local branch of a clone and what it tracks: the remote and the branch there.
#[derive(Debug, Clone, PartialEq)]
struct LocalBranch {
    name: String,
    upstream: Option<(String, String)>,
}

/// The name a worktree's branch was pushed under, when its PR may be looked up by it: the
/// branch tracks a branch of `origin` of another name, which is no shared branch
/// ([`SHARED_BRANCHES`], the worktree's base, origin's default) and neither the name nor the
/// upstream of any other branch of the clone, nor the branch of any other worktree Chauffeur has
/// known in that repo (any workspace, archived ones included).
async fn pushed_name(
    store: &Store,
    git: &Git,
    clone: &Path,
    worktree: &WorktreeRow,
) -> Option<String> {
    let listing = git
        .run(
            clone,
            &[
                "for-each-ref",
                "--format=%(refname)%00%(upstream:remotename)%00%(upstream:remoteref)",
                "refs/heads/",
            ],
        )
        .await
        .ok()?;
    let branches = parse_branches(&listing);
    let others: Vec<String> = store
        .worktrees_of_repo(&worktree.clone_path, &worktree.repo)
        .ok()?
        .into_iter()
        .filter(|w| w.id != worktree.id && !w.branch.is_empty())
        .map(|w| w.branch)
        .collect();
    let default = git.remote_default_branch(clone).await;
    pushed_name_among(
        &branches,
        &worktree.branch,
        &others,
        &[worktree.base_branch.as_str()],
        default.as_deref(),
    )
}

/// `for-each-ref`'s lines: the ref, its upstream's remote and the branch there, NUL-separated.
fn parse_branches(listing: &str) -> Vec<LocalBranch> {
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\0');
            let name = fields.next()?.strip_prefix("refs/heads/")?.to_string();
            let remote = fields.next().unwrap_or("");
            let remote_ref = fields.next().unwrap_or("");
            let upstream = remote_ref
                .strip_prefix("refs/heads/")
                .filter(|_| !remote.is_empty())
                .map(|branch| (remote.to_string(), branch.to_string()));
            Some(LocalBranch { name, upstream })
        })
        .collect()
}

fn pushed_name_among(
    branches: &[LocalBranch],
    branch: &str,
    other_worktrees: &[String],
    bases: &[&str],
    default: Option<&str>,
) -> Option<String> {
    let ours = branches.iter().find(|b| b.name == branch)?;
    let (remote, name) = ours.upstream.as_ref()?;
    if remote != "origin" || name == branch || name.is_empty() || name.starts_with('-') {
        return None;
    }
    let shared = SHARED_BRANCHES.contains(&name.as_str())
        || name.starts_with("release/")
        || name.starts_with("releases/")
        || bases.contains(&name.as_str())
        || default == Some(name.as_str());
    let taken = other_worktrees.iter().any(|other| other == name)
        || branches.iter().filter(|b| b.name != branch).any(|b| {
            b.name == *name
                || b.upstream
                    .as_ref()
                    .is_some_and(|(_, upstream)| upstream == name)
        });
    (!shared && !taken).then(|| name.clone())
}

/// How long what a repo allows is trusted.
const RIGHTS_FRESH: Duration = Duration::from_secs(3600);
const STATUS_FRESH: Duration = Duration::from_secs(10);

/// What a repo allows: its merge methods, and whether the user may merge into it.
#[derive(Clone, Default)]
struct MergeRights {
    methods: Vec<MergeMethod>,
    can_merge: Option<bool>,
}

/// What each clone's repo allows, and when GitHub said so.
type RightsCache = Mutex<HashMap<PathBuf, (Instant, MergeRights)>>;

/// What a repo allows, asked of GitHub at most once an hour.
async fn merge_rights(env: &ShellEnv, clone: &Path) -> MergeRights {
    static CACHE: OnceLock<RightsCache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((at, rights)) = cache.lock().unwrap().get(clone)
        && at.elapsed() < RIGHTS_FRESH
    {
        return rights.clone();
    }
    let output = tokio::process::Command::new("gh")
        .args([
            "repo",
            "view",
            "--json",
            "squashMergeAllowed,mergeCommitAllowed,rebaseMergeAllowed,viewerPermission",
        ])
        .current_dir(clone)
        .env_clear()
        .envs(env.vars())
        .output()
        .await;
    let Some(output) = output.ok().filter(|o| o.status.success()) else {
        return MergeRights::default();
    };
    let Ok(repo) = serde_json::from_slice::<Value>(&output.stdout) else {
        return MergeRights::default();
    };
    let rights = parse_merge_rights(&repo);
    cache
        .lock()
        .unwrap()
        .insert(clone.to_path_buf(), (Instant::now(), rights.clone()));
    rights
}

/// `gh repo view`'s answer: merging takes write access, so `WRITE`, `MAINTAIN` or `ADMIN`.
fn parse_merge_rights(repo: &Value) -> MergeRights {
    let methods = [
        ("squashMergeAllowed", MergeMethod::Squash),
        ("mergeCommitAllowed", MergeMethod::Merge),
        ("rebaseMergeAllowed", MergeMethod::Rebase),
    ]
    .into_iter()
    .filter(|(key, _)| repo[*key].as_bool() == Some(true))
    .map(|(_, method)| method)
    .collect();
    let can_merge = repo["viewerPermission"]
        .as_str()
        .filter(|p| !p.is_empty())
        .map(|p| matches!(p, "WRITE" | "MAINTAIN" | "ADMIN"));
    MergeRights { methods, can_merge }
}

/// Merges the open PRs of `params.prs`, in order, with `gh pr merge`; the first failure stops
/// the rest. GitHub decides what may be merged (checks, reviews, branch protection). A PR stacked
/// on one merged before is first pointed at where that one landed: GitHub retargets it only once
/// the merged branch is deleted, and it would otherwise merge into that branch.
pub async fn merge(daemon: &Arc<Daemon>, params: &MergePrsParams) -> Result<Vec<MergeOutcome>> {
    let row = daemon.workspaces.row(&params.workspace)?;
    crate::workspaces::read_only(&row, "it has no pull request")?;
    let statuses = status(daemon, &row.id).await?;
    let targets = if params.prs.is_empty() {
        merge_order(&statuses)
    } else {
        params.prs.clone()
    };
    if targets.is_empty() {
        return Err(Error::BadRequest(format!(
            "{} has no open pull request",
            row.slug
        )));
    }
    let worktrees = daemon.store.worktrees_of(&row.id)?;
    // Where each merged PR landed: its repo and head branch, the branch it merged into.
    let mut landed: HashMap<PrRef, String> = statuses
        .iter()
        .filter(|pr| pr.state.as_deref() == Some("MERGED"))
        .filter_map(|pr| Some((pr_ref(pr), pr.pr_base_branch.clone()?)))
        .collect();
    let mut outcomes = Vec::new();
    let mut failed = false;
    for target in &targets {
        let PrRef { repo, branch } = target.clone();
        if failed {
            outcomes.push(MergeOutcome {
                repo,
                branch: branch.clone(),
                merged: false,
                error: Some("not merged: an earlier pull request failed".into()),
            });
            continue;
        }
        let pr = statuses
            .iter()
            .find(|pr| pr_ref(pr) == *target && pr.state.as_deref() == Some("OPEN"));
        let clone = worktrees
            .iter()
            .find(|w| w.repo == repo && w.branch == branch && !w.removed)
            .map(|w| PathBuf::from(&w.clone_path));
        let (Some(pr), Some(clone)) = (pr, clone) else {
            failed = true;
            outcomes.push(MergeOutcome {
                error: Some(format!("{repo} has no open pull request on {branch}")),
                repo,
                branch,
                merged: false,
            });
            continue;
        };
        let number = pr.number.unwrap_or_default().to_string();
        let mut base = pr
            .pr_base_branch
            .clone()
            .unwrap_or_else(|| pr.base_branch.clone());
        let mut error = None;
        let mut retargeted = false;
        let below = PrRef {
            repo: repo.clone(),
            branch: base.clone(),
        };
        if let Some(onto) = landed.get(&below).cloned() {
            match gh(daemon, &clone, &["pr", "edit", &number, "--base", &onto]).await {
                Ok(()) => {
                    base = onto;
                    retargeted = true;
                }
                Err(e) => error = Some(format!("could not point it at {onto}: {e}")),
            }
        } else if let Some(below) = statuses
            .iter()
            .find(|other| pr_ref(other) == below && other.state.as_deref() == Some("OPEN"))
        {
            error = Some(format!(
                "stacked on #{}, which is not merged",
                below.number.unwrap_or_default()
            ));
        }
        if error.is_none() {
            // GitHub takes a moment to recompute a retargeted PR; until then it refuses it.
            let attempts = if retargeted { 4 } else { 1 };
            for attempt in 1..=attempts {
                error = gh(
                    daemon,
                    &clone,
                    &["pr", "merge", &number, params.method.flag()],
                )
                .await
                .err();
                if error.is_none() || attempt == attempts {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
        match &error {
            None => {
                landed.insert(target.clone(), base.clone());
                daemon.workspaces.progress(
                    &row.id,
                    format!(
                        "{repo} #{number} merged into {base} ({})",
                        method_name(params.method)
                    ),
                )
            }
            Some(error) => daemon
                .workspaces
                .warn(&row.id, format!("{repo} #{number} not merged: {error}")),
        }
        failed = error.is_some();
        outcomes.push(MergeOutcome {
            repo,
            branch: branch.clone(),
            merged: error.is_none(),
            error,
        });
    }
    forget_status(&row.id);
    Ok(outcomes)
}

/// Runs `gh` in a clone; its error is the last line it printed, without its "X " or "! " mark.
async fn gh(daemon: &Daemon, clone: &Path, args: &[&str]) -> std::result::Result<(), String> {
    let output = tokio::process::Command::new("gh")
        .args(args)
        .current_dir(clone)
        .env_clear()
        .envs(daemon.shell_env.vars())
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => crate::github::NOT_CONNECTED.to_string(),
            _ => format!("could not run gh: {error}"),
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stderr.trim().lines().last().unwrap_or("gh failed");
    let line = line
        .strip_prefix("X ")
        .or_else(|| line.strip_prefix("! "))
        .unwrap_or(line);
    Err(line.to_string())
}

fn pr_ref(pr: &PrInfo) -> PrRef {
    PrRef {
        repo: pr.repo.clone(),
        branch: pr.branch.clone(),
    }
}

/// The open PRs, in the order they are merged by default: APIs first (the front end may need what
/// they add), then the others as the status lists them (the workspace's own, then its agents' by
/// group); a PR stacked on another of its repo always after it.
pub fn merge_order(prs: &[PrInfo]) -> Vec<PrRef> {
    let open: Vec<&PrInfo> = prs
        .iter()
        .filter(|pr| pr.state.as_deref() == Some("OPEN"))
        .collect();
    let (apis, others): (Vec<&PrInfo>, Vec<&PrInfo>) = open
        .into_iter()
        .partition(|pr| pr.repo.to_lowercase().contains("api"));
    let mut order: Vec<&PrInfo> = apis.into_iter().chain(others).collect();
    let (mut i, mut moves) = (0, 0);
    while i < order.len() && moves < order.len() * order.len() {
        let below = order.iter().position(|pr| {
            pr.repo == order[i].repo && order[i].pr_base_branch.as_ref() == Some(&pr.branch)
        });
        match below {
            Some(below) if below > i => {
                let pr = order.remove(i);
                order.insert(below, pr);
                moves += 1;
            }
            _ => i += 1,
        }
    }
    order.into_iter().map(pr_ref).collect()
}

fn method_name(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::Squash => "squash",
        MergeMethod::Merge => "merge commit",
        MergeMethod::Rebase => "rebase",
    }
}

/// Folds GitHub's check runs and statuses into one state.
fn combined_checks(rollup: &Value) -> Option<String> {
    let checks = rollup.as_array()?;
    if checks.is_empty() {
        return None;
    }
    let mut pending = false;
    for check in checks {
        let state = check["conclusion"]
            .as_str()
            .filter(|c| !c.is_empty())
            .or(check["state"].as_str())
            .or(check["status"].as_str())
            .unwrap_or("")
            .to_uppercase();
        match state.as_str() {
            "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED"
            | "STARTUP_FAILURE" => {
                return Some("FAILURE".into());
            }
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => {}
            _ => pending = true,
        }
    }
    Some(if pending { "PENDING" } else { "SUCCESS" }.into())
}

/// The request sent to the workspace's agent to open its PRs: with `open-pr` and `shepherd-pr`
/// skills when installed (the first opens each PR ready for review, the second drives it through
/// CI and the AI review, never merging), the repo's own conventions otherwise.
pub fn create_prompt(
    prs: &[PrInfo],
    tickets: &[String],
    draft: bool,
    instructions: Option<&str>,
) -> String {
    let mut out = String::from(
        "Open the pull requests of this workspace, one per repo below that has commits ahead of its base branch or uncommitted work that belongs to the task (skip the others).\n\n\
1. In each such repo's folder: commit that work and push the branch.\n\
2. Open its pull request with an `open-pr` skill when one is available",
    );
    out.push_str(if draft {
        ", as a draft"
    } else {
        ", ready for review"
    });
    out.push_str(
        "; without it, open it yourself against the base branch, following the repo's own conventions.\n\
3. Once every pull request exists, edit each one's description to link the others of this workspace, and list their URLs.\n\
4. Then drive each pull request with a `shepherd-pr` skill when one is available: CI and the AI review, until it is ready for a human. Never merge: the user does.\n\n",
    );
    for pr in prs {
        out.push_str(&format!(
            "- {}: branch `{}` → `{}`, {} commit(s) ahead{}{}\n",
            match &pr.group {
                Some(group) => format!(
                    "{} in `{}/{group}/{}`",
                    pr.repo,
                    crate::paths::AGENT_WORKTREES_DIR,
                    pr.repo
                ),
                None => pr.repo.clone(),
            },
            pr.branch,
            pr.base_branch,
            pr.ahead,
            if pr.dirty {
                ", uncommitted changes"
            } else {
                ""
            },
            pr.url
                .as_ref()
                .map(|u| format!(
                    ", existing PR {u} ({})",
                    pr.state.clone().unwrap_or_default()
                ))
                .unwrap_or_default()
        ));
    }
    if !tickets.is_empty() {
        out.push_str(&format!("\nLinear tickets: {}\n", tickets.join(", ")));
    }
    if let Some(extra) = instructions.filter(|i| !i.trim().is_empty()) {
        out.push_str(&format!("\n{extra}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pr(repo: &str, state: &str) -> PrInfo {
        PrInfo {
            repo: repo.into(),
            group: None,
            branch: format!("{repo}-branch"),
            base_branch: "main".into(),
            ahead: 1,
            dirty: false,
            number: Some(1),
            pr_base_branch: Some("main".into()),
            url: None,
            state: Some(state.into()),
            title: None,
            is_draft: false,
            checks: None,
            review_decision: None,
            mergeable: None,
            merge_methods: Vec::new(),
            can_merge: None,
        }
    }

    #[test]
    fn merging_takes_write_access() {
        let rights = |permission: &str| {
            parse_merge_rights(&json!({
                "squashMergeAllowed": true,
                "mergeCommitAllowed": false,
                "rebaseMergeAllowed": true,
                "viewerPermission": permission,
            }))
        };
        assert_eq!(
            rights("WRITE").methods,
            [MergeMethod::Squash, MergeMethod::Rebase]
        );
        for permission in ["WRITE", "MAINTAIN", "ADMIN"] {
            assert_eq!(rights(permission).can_merge, Some(true), "{permission}");
        }
        for permission in ["TRIAGE", "READ"] {
            assert_eq!(rights(permission).can_merge, Some(false), "{permission}");
        }
        assert_eq!(rights("").can_merge, None);
    }

    #[test]
    fn apis_are_merged_first() {
        let prs = [pr("web", "OPEN"), pr("infra", "MERGED"), pr("api", "OPEN")];
        assert_eq!(branches(&merge_order(&prs)), ["api-branch", "web-branch"]);
    }

    fn branches(order: &[PrRef]) -> Vec<&str> {
        order.iter().map(|pr| pr.branch.as_str()).collect()
    }

    #[test]
    fn a_stacked_pr_is_merged_after_the_one_below_it() {
        let stacked = |group: &str, base: &str| PrInfo {
            group: Some(group.into()),
            branch: group.into(),
            pr_base_branch: Some(base.into()),
            ..pr("web", "OPEN")
        };
        // Listed top of the stack first, the API in the middle.
        let prs = [
            stacked("abc-3", "abc-2"),
            stacked("abc-2", "abc-1"),
            pr("api", "OPEN"),
            stacked("abc-1", "main"),
        ];
        assert_eq!(
            branches(&merge_order(&prs)),
            ["api-branch", "abc-1", "abc-2", "abc-3"]
        );
    }

    #[test]
    fn a_pr_is_stacked_only_on_one_of_its_repo() {
        // Both repos on the workspace's branch; an agent's PR in alpha targets it.
        let own = |repo: &str| PrInfo {
            branch: "jc/dec".into(),
            ..pr(repo, "OPEN")
        };
        let sub = PrInfo {
            group: Some("abc-1".into()),
            branch: "jc/dec--abc-1".into(),
            pr_base_branch: Some("jc/dec".into()),
            ..pr("alpha", "OPEN")
        };
        let order = merge_order(&[sub.clone(), own("beta"), own("alpha")]);
        let refs: Vec<(&str, &str)> = order
            .iter()
            .map(|pr| (pr.repo.as_str(), pr.branch.as_str()))
            .collect();
        assert_eq!(
            refs,
            [
                ("beta", "jc/dec"),
                ("alpha", "jc/dec"),
                ("alpha", "jc/dec--abc-1")
            ]
        );
    }

    #[test]
    fn the_prompt_names_an_agent_worktree_s_folder() {
        let sub = PrInfo {
            group: Some("abc-3214".into()),
            branch: "abc-3214-runs".into(),
            ..pr("web", "OPEN")
        };
        let prompt = create_prompt(&[pr("web", "OPEN"), sub], &[], false, None);
        assert!(prompt.contains("- web: branch `web-branch`"));
        assert!(prompt.contains("- web in `.worktrees/abc-3214/web`: branch `abc-3214-runs`"));
    }

    fn local(name: &str, upstream: Option<(&str, &str)>) -> LocalBranch {
        LocalBranch {
            name: name.into(),
            upstream: upstream.map(|(remote, branch)| (remote.into(), branch.into())),
        }
    }

    #[test]
    fn branches_are_read_with_what_they_track() {
        let listing = "refs/heads/jc/ws--menu\0origin\0refs/heads/jc/menu\n\
refs/heads/jc/ws\0\0\n\
refs/heads/local-only\0.\0refs/heads/main";
        assert_eq!(
            parse_branches(listing),
            [
                local("jc/ws--menu", Some(("origin", "jc/menu"))),
                local("jc/ws", None),
                local("local-only", Some((".", "main"))),
            ]
        );
    }

    #[test]
    fn a_pushed_under_name_is_used_only_when_it_is_nobody_else_s() {
        let ours = "jc/ws--menu";
        let with = |upstream: (&str, &str), more: &[LocalBranch]| {
            let mut branches = vec![local(ours, Some(upstream))];
            branches.extend_from_slice(more);
            branches
        };
        let name = |branches: &[LocalBranch], others: &[&str]| {
            let others: Vec<String> = others.iter().map(|o| o.to_string()).collect();
            pushed_name_among(branches, ours, &others, &["staging"], Some("trunk-ish"))
        };
        // Pushed as jc/menu: looked up under it.
        assert_eq!(
            name(&with(("origin", "jc/menu"), &[]), &[]).as_deref(),
            Some("jc/menu")
        );
        // Not pushed, pushed under its own name, or to another remote (a fork): no second name.
        assert_eq!(name(&[local(ours, None)], &[]), None);
        assert_eq!(name(&with(("origin", ours), &[]), &[]), None);
        assert_eq!(name(&with(("fork", "jc/menu"), &[]), &[]), None);
        // Tracking a shared branch: the base, origin's default, main and the like.
        for shared in [
            "main",
            "master",
            "develop",
            "staging",
            "trunk-ish",
            "release/1.2",
        ] {
            assert_eq!(name(&with(("origin", shared), &[]), &[]), None, "{shared}");
        }
        // The name is another worktree's branch (in the store, even with its branch deleted),
        // another branch of the clone, or what another branch tracks.
        assert_eq!(name(&with(("origin", "jc/menu"), &[]), &["jc/menu"]), None);
        assert_eq!(
            name(&with(("origin", "jc/menu"), &[local("jc/menu", None)]), &[]),
            None
        );
        assert_eq!(
            name(
                &with(
                    ("origin", "jc/menu"),
                    &[local("jc/other--menu", Some(("origin", "jc/menu")))]
                ),
                &[]
            ),
            None
        );
        // Unrelated branches and worktrees do not matter.
        assert_eq!(
            name(
                &with(
                    ("origin", "jc/menu"),
                    &[local("jc/other", Some(("origin", "jc/other")))]
                ),
                &["jc/ws", "jc/third"]
            )
            .as_deref(),
            Some("jc/menu")
        );
    }

    #[test]
    fn several_prs_under_a_pushed_under_name_are_ambiguous_unless_one_is_open() {
        let pr = |number: u64, state: &str| json!({"number": number, "state": state});
        let numbers = |prs: Vec<Value>| only_one(prs).and_then(|pr| pr["number"].as_u64());
        assert_eq!(numbers(vec![]), None);
        assert_eq!(numbers(vec![pr(1, "MERGED")]), Some(1));
        assert_eq!(numbers(vec![pr(2, "OPEN"), pr(1, "CLOSED")]), Some(2));
        assert_eq!(numbers(vec![pr(2, "MERGED"), pr(1, "MERGED")]), None);
        assert_eq!(numbers(vec![pr(2, "OPEN"), pr(1, "OPEN")]), None);
        assert!(is_object_id(&"a".repeat(40)));
        assert!(!is_object_id("--upload-pack=x"));
        assert!(!is_object_id("abc123"));
    }

    #[test]
    fn checks_fold_to_the_worst_state() {
        assert_eq!(combined_checks(&json!([])), None);
        assert_eq!(
            combined_checks(&json!([{"conclusion": "SUCCESS"}, {"state": "SUCCESS"}])).as_deref(),
            Some("SUCCESS")
        );
        assert_eq!(
            combined_checks(
                &json!([{"conclusion": "SUCCESS"}, {"status": "IN_PROGRESS", "conclusion": ""}])
            )
            .as_deref(),
            Some("PENDING")
        );
        assert_eq!(
            combined_checks(&json!([{"conclusion": "FAILURE"}, {"status": "IN_PROGRESS"}]))
                .as_deref(),
            Some("FAILURE")
        );
    }
}
