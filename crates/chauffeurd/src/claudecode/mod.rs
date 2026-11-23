//! `claudecode.import` (docs/adr/claude-code-import.md): Claude Code's sessions into Chauffeur.
//! It plans which sessions it reads, the branches each wrote on or created, and where each lands
//! (a workspace on a live branch, a parent with its children, a workspace carrying a clone's
//! uncommitted default-branch changes, or its repo's history exploration), reading Claude Code's
//! transcripts and the repos with git. Without a dry run, it makes those workspaces and opens the
//! sessions there as chats, never touching Claude Code's files.

mod branches;
mod copy;
mod history;
mod mcp;
pub(crate) mod moved;
mod place;
pub(crate) mod resume;
mod sessions;
mod setup;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chauffeur_protocol::claudecode::{
    ClaudeCodeBranch, ClaudeCodeHistoryPlan, ClaudeCodeImportParams, ClaudeCodeImportResult,
    ClaudeCodeOutcome, ClaudeCodeRelease, ClaudeCodeSessionPlan, ClaudeCodeWorkspaceKind,
    ClaudeCodeWorkspacePlan,
};
use chauffeur_protocol::model::WorkspaceState;

use crate::conductor::{remote_of, same_folder};
use crate::git::Git;
use crate::settings::expand_home;
use crate::worker_claude_config::Locations;
use crate::{Daemon, Error, Result};

/// The window read by default, in days.
const DEFAULT_DAYS: u32 = 60;

/// The merged PRs read per repo to tell a squash-merged branch: the most recent.
const MERGED_PRS: u32 = 1000;

pub async fn import(
    d: &Arc<Daemon>,
    params: ClaudeCodeImportParams,
) -> Result<ClaudeCodeImportResult> {
    // One import at a time: two would each find a session not yet imported.
    static IMPORTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _importing = if params.dry_run {
        None
    } else {
        Some(IMPORTING.try_lock().map_err(|_| {
            Error::Conflict(
                "An import from Claude Code is already running: wait for it to end".into(),
            )
        })?)
    };
    let locations = Locations::of(&d.shell_env.vars());
    let claude = match params
        .claude_dir
        .as_deref()
        .filter(|p| !p.trim().is_empty())
    {
        Some(dir) => {
            let dir = expand_home(dir);
            if !dir.is_absolute() {
                return Err(Error::BadRequest(format!(
                    "Claude Code's folder {} is relative: name it from the root",
                    dir.display()
                )));
            }
            dir
        }
        None => locations
            .as_ref()
            .map(|at| at.config().to_path_buf())
            .ok_or_else(|| {
                Error::BadRequest(
                    "No home folder to find Claude Code's sessions in: pass `--claude-dir <folder>`"
                        .into(),
                )
            })?,
    };
    let since_date = params.since_date.filter(|_| !params.all);
    let since_days =
        (!params.all && since_date.is_none()).then(|| params.since_days.unwrap_or(DEFAULT_DAYS));
    let start = match (&since_date, since_days) {
        (Some(date), _) => Some(midnight(date)?),
        (None, Some(days)) => Some(
            chrono::Duration::try_days(i64::from(days))
                .and_then(|span| chrono::Utc::now().checked_sub_signed(span))
                .ok_or_else(|| {
                    Error::BadRequest(format!(
                        "--since {days} days goes back before any date: use --all"
                    ))
                })?,
        ),
        (None, None) => None,
    };
    let cutoff = start.map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    let (read, running) = {
        let claude = claude.clone();
        tokio::task::spawn_blocking(move || {
            (
                sessions::read_all(&claude.join("projects"), cutoff.as_deref()),
                sessions::running(&claude.join("sessions")),
            )
        })
        .await
        .map_err(|e| Error::Internal(e.into()))?
    };
    let mut repos = Repos::new(d);
    let mut warnings = Vec::new();
    if !claude.join("projects").is_dir() {
        warnings.push(format!(
            "{} holds no `projects` folder: is it Claude Code's?",
            claude.display()
        ));
    }
    let mut plans = Vec::new();
    let mut earlier_parents = Vec::new();
    // Each placed session's clone, the one it ran in: its parent's, when it is one.
    let mut homes: HashMap<String, PathBuf> = HashMap::new();
    let mut placed = Vec::new();
    let mut found = HashMap::new();
    let mut running_folders = Vec::new();
    // The transcripts Claude keeps for a Chauffeur workspace's own folder (archived ones too): its
    // chats', and the sessions an import copied there, which still name the folder they first ran
    // in. A session run in a folder inside a workspace is left out by its folder, below.
    let workspaces: BTreeSet<String> = d
        .store
        .list_workspaces()?
        .iter()
        .flat_map(|row| {
            let path = Path::new(&row.path);
            [project_key(path), project_key(&real(path))]
        })
        .collect();
    // The project folder each session is read from, and its title there.
    let mut seen: HashMap<String, (String, String)> = HashMap::new();
    for mut session in read {
        let Some(folder) = session.folder.clone() else {
            continue;
        };
        // Chauffeur's own chats are Claude Code sessions too, run through the Agent SDK (as other
        // apps' are): only sessions of Claude Code itself, in a terminal or an editor, come.
        if real(&folder).starts_with(real(&d.paths.workspaces_dir))
            || workspaces.contains(&session.project)
            || session
                .entrypoint
                .as_deref()
                .is_some_and(|e| e.starts_with("sdk"))
        {
            continue;
        }
        // A session kept in several folders (resumed from a renamed one): its copy read first, the
        // most recent.
        if let Some((used, title)) = seen.get(&session.id) {
            warnings.push(format!(
                "“{title}” ({}) is kept in several folders of Claude Code's projects: read from {used}, its copy in {} left out.",
                session.id, session.project
            ));
            continue;
        }
        seen.insert(
            session.id.clone(),
            (session.project.clone(), session.title.clone()),
        );
        let mut plan = ClaudeCodeSessionPlan {
            id: session.id.clone(),
            title: session.title.clone(),
            folder: folder.to_string_lossy().into_owned(),
            last_active: session.last_active.clone(),
            branches: Vec::new(),
            outcome: ClaudeCodeOutcome::Import,
            reason: None,
            lands_in: None,
        };
        // Where Claude Code still runs: no branch is released from under it.
        if running.contains(&session.id) {
            running_folders.push(real(&folder));
        }
        // An orchestration an earlier import made the parent of: its children still missing come.
        let parent = branches::recorded_parent(d, &session.id)?;
        if let Some(chat) = copy::imported(d, &session.id)? {
            plan.outcome = ClaudeCodeOutcome::AlreadyImported;
            plan.lands_in = copy::workspace_of(d, &chat)?;
            plans.push(plan);
            earlier_parents.extend(parent.map(|(row, record)| (None, row, record)));
            continue;
        }
        if running.contains(&session.id) {
            plan.outcome = ClaudeCodeOutcome::Running;
            plan.reason =
                Some("still running in Claude Code: close it, then run the import again".into());
            plans.push(plan);
            continue;
        }
        let Some(home) = repos.checkout(&folder).await else {
            plan.outcome = ClaudeCodeOutcome::Skipped;
            plan.reason = Some(format!("{} is not a git repo", folder.display()));
            warnings.push(format!(
                "“{}” ran in {}, which is not a git repo: skipped.",
                session.title,
                folder.display()
            ));
            plans.push(plan);
            continue;
        };
        let Some(repo) = home.repo.clone() else {
            plan.outcome = ClaudeCodeOutcome::Skipped;
            plan.reason = Some(format!(
                "{} has no {}",
                home.clone.display(),
                crate::repos::forge_remote(&d.settings.get())
            ));
            plans.push(plan);
            continue;
        };
        let (branches, checkouts) = repos.branches(&session).await;
        plan.branches = branches;
        let stood = resume::Checkout {
            repo: repo.clone(),
            folder: home.top.clone(),
            branch: home.branch.clone(),
        };
        for checkout in std::iter::once(stood).chain(checkouts) {
            if !session.checkouts.contains(&checkout) {
                session.checkouts.push(checkout);
            }
        }
        homes.insert(session.id.clone(), home.clone.clone());
        // Its parent made, its chat not: it goes there, placed no more.
        if let Some((row, record)) = parent {
            plan.lands_in = Some(row.slug.clone());
            earlier_parents.push((Some(session.id.clone()), row, record));
        } else {
            placed.push(place::Session {
                id: session.id.clone(),
                title: session.title.clone(),
                last_active: session.last_active.clone(),
                repo,
                live: plan
                    .branches
                    .iter()
                    .filter(|b| b.live)
                    .map(|b| (b.repo.clone(), b.branch.clone()))
                    .collect(),
            });
        }
        plans.push(plan);
        found.insert(session.id.clone(), session);
    }
    // Every repo the sessions ran in or wrote on, live branch or not: their clones' default-branch
    // changes come too.
    let touched: BTreeSet<String> = placed
        .iter()
        .map(|s| s.repo.clone())
        .chain(
            plans
                .iter()
                .flat_map(|p| p.branches.iter().map(|b| b.repo.clone())),
        )
        .collect();
    // The repos whose clone holds uncommitted changes on its default branch, each with the clone
    // they are read from: the first holding some.
    let mut changed: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut bases = HashMap::new();
    for repo in &touched {
        for clone in repos.clones_of(repo) {
            let base = repos.refs(&clone).await.base.clone();
            if repos.default_has_changes(&clone, &base).await {
                match changed.get(repo) {
                    None => {
                        changed.insert(repo.clone(), clone.clone());
                    }
                    Some(first) => warnings.push(format!(
                        "The uncommitted changes on the default branch of {} are not carried: those of {}, another clone of {repo}, are. Carry them by hand if they are needed.",
                        clone.display(),
                        first.display()
                    )),
                }
            }
            bases.entry(repo.clone()).or_insert(base);
        }
    }
    // A branch an earlier import made a workspace of stays there: no new parent takes it.
    let mut made = BTreeSet::new();
    for session in &placed {
        for (repo, branch) in &session.live {
            if branches::existing_in(d, std::slice::from_ref(repo), branch)?.is_some() {
                made.insert(branch.clone());
            }
        }
    }
    let changes: Vec<String> = changed.keys().cloned().collect();
    let mut plan = place::place(&placed, &changes, &bases, &made);
    for session in &mut plans {
        if let Some(lands_in) = plan.lands_in.get(&session.id) {
            session.lands_in = Some(lands_in.clone());
        }
    }
    for (session, row, record) in earlier_parents {
        with_earlier_parent(d, &mut plan, session, &row, &record)?;
    }
    // Changes an earlier import carried: no new parent carries them again.
    for workspace in plan
        .workspaces
        .iter_mut()
        .filter(|w| w.kind == ClaudeCodeWorkspaceKind::Parent)
    {
        let mut kept = Vec::new();
        for repo in std::mem::take(&mut workspace.changes_from) {
            if branches::changes_workspace(d, &repo)?.is_none() {
                kept.push(repo);
            }
        }
        workspace.changes_from = kept;
    }
    // The branches this version makes workspaces of (a plain one's, a parent's child), in the
    // repos an earlier import's workspace on it lacks (all, without one). Each repo's clone is
    // chosen now, once for the whole run: Chauffeur's, else the user's holding the branch, which
    // becomes Chauffeur's (`branches::adopt`).
    let mut releases = Vec::new();
    let mut sources = HashMap::new();
    let mut chosen: HashMap<String, PathBuf> = HashMap::new();
    for workspace in plan
        .workspaces
        .iter()
        .filter(|w| w.kind == ClaudeCodeWorkspaceKind::Branch)
    {
        let Some(branch) = &workspace.branch else {
            continue;
        };
        // An earlier import's workspace on the branch: only the repos it lacks.
        let existing = branches::existing_in(d, &workspace.repos, branch)?;
        for repo in &branches::missing_repos(d, workspace, branch, existing.as_ref())? {
            let clone = repos.source(repo, branch).await;
            let (chauffeur, adopt) = chosen_clone(d, &mut chosen, repo, clone.as_ref());
            // Its checkouts in the user's clone holding it: their changes are copied, and they are
            // released only when that clone is Chauffeur's; in another, the branch is fetched
            // there and they keep it.
            let holders: Vec<Holder> = repos
                .holders(repo, branch)
                .await
                .into_iter()
                .filter(|h| clone.as_ref().is_some_and(|c| same_folder(c, &h.clone)))
                .collect();
            let released = chauffeur
                .as_ref()
                .is_some_and(|c| clone.as_ref().is_some_and(|u| same_folder(c, u)));
            // Where Claude Code still runs, nothing is released: the import refuses that branch.
            if released {
                releases.extend(
                    holders
                        .iter()
                        .filter(|h| !runs_in(&running_folders, &h.release.folder))
                        .map(|h| h.release.clone()),
                );
            }
            sources.insert(
                branches::RepoBranch::new(repo, branch),
                branches::Source {
                    chauffeur,
                    adopt,
                    clone,
                    holders,
                },
            );
        }
    }
    // The repo of a workspace on a new branch (a parent's, a changes one's): the clone chosen for
    // it, else the user's its session ran in, or holding the changes, else the first met.
    let mut new_branch_clones: HashMap<String, branches::RepoClone> = HashMap::new();
    for workspace in plan.workspaces.iter().filter(|w| {
        matches!(
            w.kind,
            ClaudeCodeWorkspaceKind::Parent | ClaudeCodeWorkspaceKind::Changes
        )
    }) {
        let Some(repo) = workspace.repos.first() else {
            continue;
        };
        let ran_in = workspace.sessions.first().and_then(|id| homes.get(id));
        let first = repos.clones.get(repo).and_then(|c| c.first());
        let user = ran_in.or(changed.get(repo)).or(first);
        let (clone, adopt) = chosen_clone(d, &mut chosen, repo, user);
        if let Some(clone) = clone {
            new_branch_clones.insert(repo.clone(), branches::RepoClone { clone, adopt });
        }
    }
    let trees = workspace_trees(d, &sources, &new_branch_clones, &plan.histories, &repos).await?;
    let setups = setup::guess_all(d, &repos.git, &trees).await;
    // Every repo whose sessions the run imports, into a new workspace or an earlier one's.
    let user_clones: BTreeMap<String, Vec<PathBuf>> = touched
        .iter()
        .map(|repo| (repo.clone(), repos.clones_of(repo)))
        .collect();
    let state = locations
        .as_ref()
        .map_or_else(|| claude.join(".claude.json"), |at| at.state_for(&claude));
    warnings.extend(mcp::warnings(&state, &user_clones));
    warnings.extend(repos.warnings);
    if params.dry_run {
        for history in &mut plan.histories {
            history.workspace = history::exploration(d, &history.repo)?.map(|row| row.slug);
        }
        for workspace in &mut plan.workspaces {
            if let Some(branch) = workspace.branch.clone() {
                workspace.workspace =
                    branches::existing_in(d, &workspace.repos, &branch)?.map(|row| row.slug);
            } else if workspace.kind == ClaudeCodeWorkspaceKind::Changes
                && let Some(repo) = workspace.repos.first()
            {
                workspace.workspace = branches::changes_workspace(d, repo)?.map(|row| row.slug);
            }
        }
    } else {
        // Before any worktree: the workspaces made below run them.
        setup::write(d, &setups)?;
        let mut report = copy::Report::new(&mut plans, &mut warnings);
        let found = branches::Found {
            sources: &sources,
            new_branch_clones: &new_branch_clones,
            changed: &changed,
            sessions: &found,
            running: &running_folders,
            move_changes: params.move_changes,
        };
        branches::import(d, &claude, &mut plan.workspaces, &found, &mut report).await?;
        for (folder, done) in std::mem::take(&mut report.moved) {
            if let Some(release) = releases
                .iter_mut()
                .find(|r| same_folder(Path::new(&r.folder), &folder))
            {
                release.moved = Some(done);
            }
        }
        history::import(d, &claude, &mut plan.histories, found.sessions, &mut report).await?;
    }
    let mut said = BTreeSet::new();
    warnings.retain(|w| said.insert(w.clone()));
    Ok(ClaudeCodeImportResult {
        dry_run: params.dry_run,
        move_changes: params.move_changes,
        since_days,
        since_date,
        claude_dir: claude.to_string_lossy().into_owned(),
        sessions: plans,
        workspaces: plan.workspaces,
        histories: plan.histories,
        releases,
        setups,
        warnings,
    })
}

/// The trees the run checks out in each repo it makes a workspace in: first its base, in the clone
/// the workspaces come from (a branch's, or a history's whose exploration is not ready); then each
/// branch's, in the user's clone holding it.
async fn workspace_trees(
    d: &Daemon,
    sources: &HashMap<branches::RepoBranch, branches::Source>,
    parents: &HashMap<String, branches::RepoClone>,
    histories: &[ClaudeCodeHistoryPlan],
    repos: &Repos<'_>,
) -> Result<BTreeMap<String, Vec<setup::Tree>>> {
    let mut clones = BTreeMap::new();
    for (repo, parent) in parents {
        clones.insert(repo.clone(), parent.clone.clone());
    }
    let mut branches: BTreeMap<String, Vec<setup::Tree>> = BTreeMap::new();
    for (key, source) in sources {
        if let Some(clone) = source.chauffeur.as_ref().or(source.clone.as_ref()) {
            clones
                .entry(key.repo.clone())
                .or_insert_with(|| clone.clone());
        }
        if let Some(clone) = &source.clone {
            branches
                .entry(key.repo.clone())
                .or_default()
                .push(setup::Tree {
                    clone: clone.clone(),
                    reference: format!("refs/heads/{}", key.branch),
                });
        }
    }
    for history in histories {
        if history::exploration(d, &history.repo)?.is_some() {
            continue;
        }
        let clone = d
            .repos
            .existing_clone(&history.repo)
            .or_else(|| repos.clones.get(&history.repo)?.first().cloned());
        if let Some(clone) = clone {
            clones.entry(history.repo.clone()).or_insert(clone);
        }
    }
    let mut trees = BTreeMap::new();
    for (repo, clone) in clones {
        let (_, reference) = repos.base(&clone, Some(&repo)).await;
        let mut checked = vec![setup::Tree { clone, reference }];
        checked.extend(branches.remove(&repo).unwrap_or_default());
        trees.insert(repo, checked);
    }
    Ok(trees)
}

/// The parent an earlier import made of an orchestration (`row`), back in `plan` first: its
/// session, when given (its chat could not be made then), and its children still missing (a
/// branch planned already goes under it).
fn with_earlier_parent(
    d: &Daemon,
    plan: &mut place::Plan,
    session: Option<String>,
    row: &crate::store::WorkspaceRow,
    record: &branches::ParentRecord,
) -> Result<()> {
    let mut children = Vec::new();
    for child in &record.children {
        if let Some(planned) = plan
            .workspaces
            .iter_mut()
            .find(|w| w.branch.as_deref() == Some(child.branch.as_str()))
        {
            planned.parent.get_or_insert_with(|| row.slug.clone());
            continue;
        }
        if branches::existing_in(d, &child.repos, &child.branch)?.is_none() {
            children.push(ClaudeCodeWorkspacePlan {
                branch: Some(child.branch.clone()),
                parent: Some(row.slug.clone()),
                ..place::new_workspace(
                    child.branch.clone(),
                    ClaudeCodeWorkspaceKind::Branch,
                    child.repos.clone(),
                )
            });
        }
    }
    let parent = ClaudeCodeWorkspacePlan {
        sessions: session.into_iter().collect(),
        workspace: Some(row.slug.clone()),
        ..place::new_workspace(
            row.slug.clone(),
            ClaudeCodeWorkspaceKind::Parent,
            vec![record.repo.clone()],
        )
    };
    plan.workspaces.insert(0, parent);
    plan.workspaces.extend(children);
    Ok(())
}

/// The workspace whose id the `kv` record `key` holds, while it is ready.
fn ready_workspace(d: &Daemon, key: &str) -> Result<Option<crate::store::WorkspaceRow>> {
    let Some(id) = d.store.kv_get(key)? else {
        return Ok(None);
    };
    Ok(d.store
        .find_workspace(&id)?
        .filter(|row| row.state == WorkspaceState::Ready))
}

/// Chauffeur's clone of `repo` for the whole run: its own, else the user's chosen for it earlier
/// in the run, else `user` (the user's holding what is imported); with whether it is the user's,
/// to adopt.
fn chosen_clone(
    d: &Daemon,
    chosen: &mut HashMap<String, PathBuf>,
    repo: &str,
    user: Option<&PathBuf>,
) -> (Option<PathBuf>, bool) {
    if let Some(own) = d.repos.existing_clone(repo) {
        return (Some(own), false);
    }
    let clone = user.map(|user| {
        chosen
            .entry(repo.to_owned())
            .or_insert_with(|| user.clone())
            .clone()
    });
    (clone, true)
}

/// A checkout of a branch to release, with the clone it belongs to.
pub(super) struct Holder {
    pub(super) release: ClaudeCodeRelease,
    pub(super) clone: PathBuf,
    /// Its folder is gone: git keeps it only in the clone's `.git`.
    pub(super) gone: bool,
}

/// Claude Code still runs in `folder` or below it.
fn runs_in(running: &[PathBuf], folder: &str) -> bool {
    let folder = real(Path::new(folder));
    running.iter().any(|r| r.starts_with(&folder))
}

/// Under the system's temporary folders (`/tmp`, `$TMPDIR`), where cleanups delete files.
fn under_temporary_folder(folder: &Path) -> bool {
    let folder = real(folder);
    [
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
        PathBuf::from("/var/folders"),
        PathBuf::from("/private/var/folders"),
        real(&std::env::temp_dir()),
    ]
    .iter()
    .any(|temporary| folder.starts_with(temporary))
}

/// The start of a day here, `YYYY-MM-DD`.
fn midnight(date: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    let refused = || {
        Error::BadRequest(format!(
            "{date} is no date: `--since` takes a number of days or a date, YYYY-MM-DD"
        ))
    };
    let day = chrono::NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").map_err(|_| refused())?;
    day.and_hms_opt(0, 0, 0)
        .and_then(|at| at.and_local_timezone(chrono::Local).earliest())
        .map(|at| at.with_timezone(&chrono::Utc))
        .ok_or_else(refused)
}

/// A path as the file system names it: its nearest existing folder resolved (`/tmp` →
/// `/private/tmp`), the rest as written. Git names checkouts that way.
fn real(path: &Path) -> PathBuf {
    for existing in path.ancestors() {
        if let Ok(resolved) = existing.canonicalize() {
            let rest = path.strip_prefix(existing).unwrap_or(Path::new(""));
            return if rest.as_os_str().is_empty() {
                resolved
            } else {
                resolved.join(rest)
            };
        }
    }
    path.to_path_buf()
}

/// The name Claude gives the folder it keeps `dir`'s sessions in: every character but an ASCII
/// letter or digit as `-`.
fn project_key(dir: &Path) -> String {
    dir.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// A folder name worktrees are kept under (`.wt`, `.worktrees`, `.claude/worktrees`): a path
/// missing below one was in a worktree removed since.
fn in_worktree_folder(rel: &Path) -> bool {
    rel.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some(".wt" | ".worktrees" | "worktrees")
        )
    })
}

/// A checkout a session wrote in or ran from, and the repo it belongs to.
#[derive(Clone)]
struct Checkout {
    /// Its top folder.
    top: PathBuf,
    /// The repo's own checkout (the main worktree).
    clone: PathBuf,
    /// Chauffeur's name of the repo; none without a remote on the installation's forge.
    repo: Option<String>,
    /// The branch checked out there now.
    branch: Option<String>,
}

/// A repo's merged PRs, (head commit, base branch) by branch.
type MergedPrs = HashMap<String, Vec<(String, String)>>;

/// A clone's branches, as [`Repos::refs`] reads them.
struct Refs {
    base: String,
    tips: HashMap<String, String>,
    merged: BTreeSet<String>,
    worktrees: Vec<crate::git::WorktreeEntry>,
}

/// The repos the sessions touch, read with git once each.
struct Repos<'a> {
    d: &'a Daemon,
    /// Git with `GIT_NO_LAZY_FETCH`: a partial clone never fetches a missing object (a PR's
    /// head) on its own, so nothing is written there.
    git: Git,
    checkouts: HashMap<PathBuf, Option<Checkout>>,
    /// Each repo's clones, by Chauffeur's name, in the order met.
    clones: HashMap<String, Vec<PathBuf>>,
    /// Each clone's repo name.
    names: HashMap<PathBuf, String>,
    live: HashMap<(PathBuf, String), bool>,
    /// Each clone's merged PRs (head, base) by branch, read once; none when the forge could not say.
    merged: HashMap<PathBuf, Option<MergedPrs>>,
    refs: HashMap<PathBuf, Refs>,
    /// Clones without a remote on the installation's forge, warned about once.
    unnamed: BTreeSet<PathBuf>,
    /// Chauffeur's workspaces folder, resolved as git names folders.
    workspaces: PathBuf,
    warnings: Vec<String>,
}

impl<'a> Repos<'a> {
    fn new(d: &'a Daemon) -> Repos<'a> {
        let env = crate::shellenv::ShellEnv::new();
        let mut vars = d.shell_env.vars();
        vars.insert("GIT_NO_LAZY_FETCH".into(), "1".into());
        env.set_vars(vars);
        Repos {
            d,
            git: Git::new(env),
            checkouts: HashMap::new(),
            clones: HashMap::new(),
            names: HashMap::new(),
            live: HashMap::new(),
            merged: HashMap::new(),
            refs: HashMap::new(),
            unnamed: BTreeSet::new(),
            workspaces: real(&d.paths.workspaces_dir),
            warnings: Vec::new(),
        }
    }

    /// The checkout holding `path`, a file or a folder, from its nearest folder still there;
    /// none outside git or in one of Chauffeur's workspaces.
    async fn checkout(&mut self, path: &Path) -> Option<Checkout> {
        let dir = path.ancestors().find(|a| a.is_dir())?;
        // Its top folder, the first holding a `.git`: git is asked once per checkout, not once
        // per folder written.
        let top = dir
            .ancestors()
            .find(|a| a.join(".git").exists())?
            .to_path_buf();
        if let Some(known) = self.checkouts.get(&top) {
            return known.clone();
        }
        let found = self.read_checkout(&top).await;
        self.checkouts.insert(top, found.clone());
        found
    }

    async fn read_checkout(&mut self, dir: &Path) -> Option<Checkout> {
        let top = PathBuf::from(
            self.git
                .run(dir, &["rev-parse", "--show-toplevel"])
                .await
                .ok()?
                .trim(),
        );
        if top.starts_with(&self.workspaces) {
            return None;
        }
        let common = self.git.common_dir(&top).await.ok()?;
        let clone = if common.file_name().is_some_and(|n| n == ".git") {
            common.parent()?.to_path_buf()
        } else {
            common
        };
        let repo = match remote_of(self.d, &self.git, &clone, None).await {
            Some((owner, name)) => {
                let name = crate::repos::repo_reference(&self.d.settings.get(), &owner, &name);
                let clones = self.clones.entry(name.clone()).or_default();
                if !clones.contains(&clone) {
                    clones.push(clone.clone());
                }
                self.names.insert(clone.clone(), name.clone());
                Some(name)
            }
            None => {
                if self.unnamed.insert(clone.clone()) {
                    self.warnings.push(format!(
                        "{} has no {}: its sessions and branches are left out.",
                        clone.display(),
                        crate::repos::forge_remote(&self.d.settings.get())
                    ));
                }
                None
            }
        };
        let branch = self.git.current_branch(&top).await.ok().flatten();
        Some(Checkout {
            top,
            clone,
            repo,
            branch,
        })
    }

    /// The branches a session wrote on (its file tools' paths, each in the checkout holding it, on
    /// the branch checked out there then) or created, by repo, each with whether it is live; and
    /// the checkouts it wrote in.
    async fn branches(
        &mut self,
        session: &sessions::Session,
    ) -> (Vec<ClaudeCodeBranch>, Vec<resume::Checkout>) {
        let mut found: Vec<(String, PathBuf, String)> = Vec::new();
        let mut checkouts = Vec::new();
        let mut gone = BTreeSet::new();
        for write in &session.writes {
            let (path, cwd) = (real(&write.path), real(&write.cwd));
            // The checkout the session stood in (from its nearest folder still there: a worktree
            // removed since resolves to its clone, the same repo), on the branch recorded then.
            let stood = self.checkout(&cwd).await;
            if let Some(stood) = &stood
                && let Some(repo) = &stood.repo
            {
                checkouts.push(resume::Checkout {
                    repo: repo.clone(),
                    folder: stood.top.clone(),
                    branch: write.branch.clone(),
                });
            }
            let dir = path.parent().unwrap_or(&path).to_path_buf();
            let (checkout, branch) = if dir.is_dir() {
                let Some(checkout) = self.checkout(&path).await else {
                    continue;
                };
                // Elsewhere than where it stood: the branch checked out there now.
                let branch = if stood.as_ref().is_some_and(|s| s.top == checkout.top) {
                    write.branch.clone()
                } else {
                    checkout.branch.clone()
                };
                (checkout, branch)
            } else {
                match self.missing(stood, &path, write.branch.clone()).await {
                    Some(found) => found,
                    None => {
                        gone.insert(dir);
                        continue;
                    }
                }
            };
            if let Some(repo) = checkout.repo.clone() {
                checkouts.push(resume::Checkout {
                    repo: repo.clone(),
                    folder: checkout.top.clone(),
                    branch: branch.clone(),
                });
                if let Some(branch) = branch {
                    found.push((repo, checkout.clone, branch));
                }
            }
        }
        for created in &session.created {
            // A folder that is not there names no repo: its parent's would be a guess.
            let dir = real(&created.dir);
            if !dir.is_dir() {
                continue;
            }
            if let Some(checkout) = self.checkout(&dir).await
                && let Some(repo) = checkout.repo.clone()
            {
                checkouts.push(resume::Checkout {
                    repo: repo.clone(),
                    folder: checkout.top.clone(),
                    branch: Some(created.branch.clone()),
                });
                found.push((repo, checkout.clone, created.branch.clone()));
            }
        }
        if let Some(first) = gone.iter().next() {
            let more = match gone.len() {
                1 => String::new(),
                n => format!(" and {} other folders", n - 1),
            };
            self.warnings.push(format!(
                "“{}” wrote in {}{more}, gone since: what it wrote there is left out.",
                session.title,
                first.display()
            ));
        }
        let mut branches: Vec<ClaudeCodeBranch> = Vec::new();
        for (repo, clone, branch) in found {
            if branches
                .iter()
                .any(|b| b.repo == repo && b.branch == branch)
            {
                continue;
            }
            let live = self.is_live(&clone, &branch).await;
            branches.push(ClaudeCodeBranch { repo, branch, live });
        }
        (branches, checkouts)
    }

    /// A file whose folder is missing now: in a worktree of a clone met so far, that worktree's
    /// branch (git still lists it until pruned); in a folder worktrees are kept under, gone;
    /// elsewhere in the checkout the session stood in, a folder of another branch, the one recorded.
    async fn missing(
        &mut self,
        stood: Option<Checkout>,
        path: &Path,
        recorded: Option<String>,
    ) -> Option<(Checkout, Option<String>)> {
        // A worktree of a clone met so far holding it, removed since but still listed (a scratch
        // worktree under /tmp, one inside the clone): that worktree's branch.
        let mut clones: Vec<PathBuf> = self.clones.values().flatten().cloned().collect();
        if let Some(stood) = &stood
            && !clones.contains(&stood.clone)
        {
            clones.push(stood.clone.clone());
        }
        let mut holder: Option<(PathBuf, crate::git::WorktreeEntry)> = None;
        for clone in clones {
            for entry in &self.refs(&clone).await.worktrees {
                let deeper = holder.as_ref().is_none_or(|(_, held)| {
                    entry.path.components().count() > held.path.components().count()
                });
                if entry.path != clone && path.starts_with(&entry.path) && deeper {
                    holder = Some((clone.clone(), entry.clone()));
                }
            }
        }
        if let Some((clone, entry)) = holder {
            let repo = self.names.get(&clone).cloned();
            let checkout = Checkout {
                top: entry.path,
                clone,
                repo,
                branch: entry.branch.clone(),
            };
            return Some((checkout, entry.branch));
        }
        let stood = stood.filter(|s| path.starts_with(&s.top))?;
        if in_worktree_folder(path.strip_prefix(&stood.top).unwrap_or(path)) {
            return None;
        }
        Some((stood, recorded))
    }

    /// A branch that exists and is not merged into the default branch (its tip in it, or a merged
    /// PR into it whose head is the tip or after it), or whose checkout holds uncommitted changes.
    /// The default branch is none: its changes get a workspace of their own. Nor is a branch
    /// already in one of Chauffeur's workspaces.
    async fn is_live(&mut self, clone: &Path, branch: &str) -> bool {
        let key = (clone.to_path_buf(), branch.to_string());
        if let Some(live) = self.live.get(&key) {
            return *live;
        }
        let live = self.read_live(clone, branch).await;
        self.live.insert(key, live);
        live
    }

    async fn read_live(&mut self, clone: &Path, branch: &str) -> bool {
        // A branch an earlier import made a workspace on: its sessions since go there.
        if let Some(repo) = self.names.get(clone)
            && branches::recorded(self.d, repo, branch)
                .ok()
                .flatten()
                .is_some()
        {
            return true;
        }
        let workspaces = self.workspaces.clone();
        // In a Chauffeur workspace already, made from this clone or another of the repo.
        let siblings = self
            .names
            .get(clone)
            .and_then(|repo| self.clones.get(repo))
            .cloned()
            .unwrap_or_else(|| vec![clone.to_path_buf()]);
        for sibling in siblings {
            let held =
                self.refs(&sibling).await.worktrees.iter().any(|e| {
                    e.branch.as_deref() == Some(branch) && e.path.starts_with(&workspaces)
                });
            if held {
                self.warnings.push(format!(
                    "{branch} is in a Chauffeur workspace already: left out."
                ));
                return false;
            }
        }
        let refs = self.refs(clone).await;
        if branch == refs.base {
            return false;
        }
        let Some(tip) = refs.tips.get(branch).cloned() else {
            return false;
        };
        let holders: Vec<PathBuf> = refs
            .worktrees
            .iter()
            .filter(|e| e.branch.as_deref() == Some(branch) && !e.prunable)
            .map(|e| e.path.clone())
            .collect();
        let merged = refs.merged.contains(branch);
        let base = refs.base.clone();
        // Uncommitted changes in a checkout make it live, merged or not.
        for holder in holders {
            if self.has_changes(&holder).await {
                return true;
            }
        }
        if merged {
            return false;
        }
        // Squash-merged: a merged PR from the branch into the default branch whose head is its
        // tip or after it.
        let Some(heads) = self.merged_heads(clone, branch, &base).await else {
            self.warnings.push(format!(
                "Could not ask whether {branch} is merged: it is counted as live."
            ));
            return true;
        };
        for (head, into) in heads {
            if into != base {
                continue;
            }
            if head == tip
                || self
                    .git
                    .ok(clone, &["merge-base", "--is-ancestor", &tip, &head])
                    .await
            {
                return false;
            }
        }
        true
    }

    /// The (head, base) of the merged PRs from `branch`, from the repo's recent merged PRs, read
    /// once (`MERGED_PRS`; on GitLab, its merge requests into `base`, the clone's default branch).
    async fn merged_heads(
        &mut self,
        clone: &Path,
        branch: &str,
        base: &str,
    ) -> Option<Vec<(String, String)>> {
        let forge = self.d.settings.get().forge;
        if !self.merged.contains_key(clone) {
            let heads =
                crate::prs::merged_heads(forge, &self.d.shell_env, clone, base, MERGED_PRS).await;
            self.merged.insert(clone.to_path_buf(), heads);
        }
        let heads = self.merged.get(clone)?.as_ref()?;
        if let Some(found) = heads.get(branch) {
            return Some(found.clone());
        }
        // The list stopped at its limit: an older PR is asked for by its branch (on GitLab, `base`
        // standing for a merge request that names no target).
        if heads.values().map(Vec::len).sum::<usize>() < MERGED_PRS as usize {
            return Some(Vec::new());
        }
        let named = (forge == chauffeur_protocol::client::Forge::Gitlab).then_some(base);
        self.merged_from(forge, clone, branch, named).await
    }

    /// The merged PRs from `branch`, asked for by name: (head, base), `base` standing for one
    /// that names none (dropped without it).
    async fn merged_from(
        &self,
        forge: chauffeur_protocol::client::Forge,
        clone: &Path,
        branch: &str,
        base: Option<&str>,
    ) -> Option<Vec<(String, String)>> {
        let prs = crate::prs::pr_list(
            forge,
            &self.d.shell_env,
            clone,
            branch,
            "merged",
            "headRefOid,baseRefName",
        )
        .await?;
        Some(
            prs.iter()
                .filter_map(|pr| {
                    Some((
                        pr["headRefOid"].as_str()?.to_string(),
                        pr["baseRefName"].as_str().or(base)?.to_string(),
                    ))
                })
                .collect(),
        )
    }

    /// A clone's branches, read once: its default branch, each branch's tip, those merged into
    /// the default branch, its worktrees.
    async fn refs(&mut self, clone: &Path) -> &Refs {
        if !self.refs.contains_key(clone) {
            let refs = self.read_refs(clone).await;
            self.refs.insert(clone.to_path_buf(), refs);
        }
        &self.refs[clone]
    }

    async fn read_refs(&self, clone: &Path) -> Refs {
        let (base, target) = self.base(clone, None).await;
        let heads = [
            "for-each-ref",
            "--format=%(refname:lstrip=2) %(objectname)",
            "refs/heads",
        ];
        let tips = self
            .git
            .run(clone, &heads)
            .await
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                l.rsplit_once(' ')
                    .map(|(name, oid)| (name.to_string(), oid.to_string()))
            })
            .collect();
        let in_base = [
            "for-each-ref",
            "--format=%(refname:lstrip=2)",
            "--merged",
            &target,
            "refs/heads",
        ];
        let merged = self
            .git
            .run(clone, &in_base)
            .await
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect();
        let worktrees = self
            .git
            .worktree_list(clone)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|e| crate::git::WorktreeEntry {
                path: real(&e.path),
                ..e
            })
            .collect();
        Refs {
            base,
            tips,
            merged,
            worktrees,
        }
    }

    /// The clone's default branch and the ref holding it: as the user set it for the repo, else
    /// as the base remote (a fork's parent, else `origin`) names it when it still has that branch,
    /// else `main` or `master`, whichever it has; the remote's copy when there is one. `repo`
    /// names the clone's repo when the sessions did not meet it (Chauffeur's own).
    async fn base(&self, clone: &Path, repo: Option<&str>) -> (String, String) {
        let remote = self.git.base_remote(clone).await;
        let has = |reference: String| async move {
            self.git
                .ok(clone, &["rev-parse", "-q", "--verify", &reference])
                .await
                .then_some(reference)
        };
        let set = repo
            .or(self.names.get(clone).map(String::as_str))
            .and_then(|name| {
                let settings = self.d.settings.get();
                settings
                    .repos
                    .get(crate::repos::repo_folder(name))
                    .and_then(|r| r.default_branch.clone())
            });
        let named = self
            .git
            .run(
                clone,
                &[
                    "symbolic-ref",
                    "-q",
                    "--short",
                    &format!("refs/remotes/{remote}/HEAD"),
                ],
            )
            .await
            .ok()
            .and_then(|r| {
                r.trim()
                    .strip_prefix(&format!("{remote}/"))
                    .map(String::from)
            });
        let mut candidates: Vec<String> = set.into_iter().collect();
        candidates.extend(named);
        candidates.extend(["main".to_string(), "master".to_string()]);
        for branch in &candidates {
            if let Some(found) = has(format!("refs/remotes/{remote}/{branch}")).await {
                return (branch.clone(), found);
            }
            if let Some(found) = has(format!("refs/heads/{branch}")).await {
                return (branch.clone(), found);
            }
        }
        let first = candidates
            .into_iter()
            .next()
            .unwrap_or_else(|| "main".into());
        let target = format!("refs/heads/{first}");
        (first, target)
    }

    /// The clone is on its default branch with uncommitted changes.
    async fn default_has_changes(&self, clone: &Path, base: &str) -> bool {
        self.git
            .current_branch(clone)
            .await
            .is_ok_and(|b| b.as_deref() == Some(base))
            && self.has_changes(clone).await
    }

    /// Uncommitted changes, untracked files included, read without refreshing the index.
    async fn has_changes(&self, folder: &Path) -> bool {
        self.git
            .run(folder, &["--no-optional-locks", "status", "--porcelain"])
            .await
            .is_ok_and(|out| !out.trim().is_empty())
    }

    /// The user's clones of `repo`, in the order met.
    fn clones_of(&self, repo: &str) -> Vec<PathBuf> {
        self.clones.get(repo).cloned().unwrap_or_default()
    }

    /// The checkouts of `branch` in the user's clones of `repo` (the clone itself or one of its
    /// worktrees, gone or not), outside Chauffeur's workspaces: released to make its workspace.
    async fn holders(&mut self, repo: &str, branch: &str) -> Vec<Holder> {
        let workspaces = self.workspaces.clone();
        let mut holders = Vec::new();
        for clone in self.clones_of(repo) {
            let entries = self.refs(&clone).await.worktrees.clone();
            for e in entries {
                if e.branch.as_deref() != Some(branch) || e.path.starts_with(&workspaces) {
                    continue;
                }
                let commits_only = e.prunable || self.half_deleted(&e.path).await;
                holders.push(Holder {
                    release: ClaudeCodeRelease {
                        repo: repo.to_string(),
                        branch: branch.to_string(),
                        folder: e.path.to_string_lossy().into_owned(),
                        commits_only,
                        moved: None,
                    },
                    clone: clone.clone(),
                    gone: e.prunable,
                });
            }
        }
        holders
    }

    /// The user's clone of `repo` holding `branch`: the one it is checked out in, else the first
    /// one that has it.
    async fn source(&mut self, repo: &str, branch: &str) -> Option<PathBuf> {
        let mut has = None;
        for clone in self.clones_of(repo) {
            let refs = self.refs(&clone).await;
            if !refs.tips.contains_key(branch) {
                continue;
            }
            if refs
                .worktrees
                .iter()
                .any(|e| e.branch.as_deref() == Some(branch))
            {
                return Some(clone);
            }
            has.get_or_insert(clone);
        }
        has
    }

    /// A checkout under a temporary folder whose cleanup was cut short: a tracked file of it is
    /// gone. Its diff would delete them: it carries its commits only, its changes staying there.
    async fn half_deleted(&self, folder: &Path) -> bool {
        if !under_temporary_folder(folder) {
            return false;
        }
        self.git
            .run_bytes(
                folder,
                &["--no-optional-locks", "ls-files", "--deleted", "-z"],
            )
            .await
            .is_ok_and(|missing| missing.iter().any(|b| *b != 0))
    }
}
