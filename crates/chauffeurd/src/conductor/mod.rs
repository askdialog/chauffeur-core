//! `conductor.import` (docs/adr/conductor-import.md): Conductor's repos, workspaces and chat
//! history into Chauffeur. The plan is read from Conductor's database, opened read-only, and its
//! worktrees, read with git without optional locks; a dry run stops there. Otherwise the repos'
//! settings are written, then each workspace is imported on its own, one failing leaving the
//! others going. Conductor's files change in one way only: a released branch, its worktree's
//! HEAD detached on the same commit.

mod db;
mod history;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chauffeur_protocol::client::CreateWorkspaceParams;
use chauffeur_protocol::conductor::{
    ConductorClone, ConductorImportParams, ConductorImportResult, ConductorOutcome,
    ConductorRepoPlan, ConductorWorkspacePlan,
};
use chauffeur_protocol::model::{TicketRef, WorkspaceState};

use crate::git::Git;
use crate::paths::NOTES_DIR;
use crate::repos::repo_folder;
use crate::settings::expand_home;
use crate::store::{TitleSource, WorkspaceRow, now};
use crate::{Daemon, Error, Result};

/// One import at a time: two would plan the same workspaces.
static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The store's key remembering a Conductor workspace imported, by its Conductor id; its value is
/// the Chauffeur workspace's id.
const IMPORTED: &str = "conductor_import:";

/// Where an imported workspace's Conductor files go, in its notes folder.
pub const CONDUCTOR_NOTES: &str = "conductor";

/// Conductor's database on this Mac.
pub fn default_db() -> PathBuf {
    expand_home("~/Library/Application Support/com.conductor.app/conductor.db")
}

/// A Conductor repo as Chauffeur will use it.
struct Repo {
    plan: ConductorRepoPlan,
    conductor_id: String,
    /// A folder to add to the clone folders, for a Conductor clone Chauffeur did not know.
    clone_root: Option<PathBuf>,
    /// The scripts and variables to write into `[repos.<name>]`, values included.
    scripts: Vec<(String, String)>,
    env: Vec<(String, String)>,
}

impl Repo {
    fn usable(&self) -> bool {
        self.plan.skipped.is_none()
    }
}

/// A workspace as planned, with what applying it needs.
struct Planned {
    plan: ConductorWorkspacePlan,
    created_at: Option<String>,
    sessions: Vec<db::Session>,
    notes: Option<String>,
    /// Its repo's clone in Conductor and in Chauffeur.
    conductor_clone: Option<PathBuf>,
    chauffeur_clone: Option<PathBuf>,
    ticket: Option<TicketRef>,
}

pub async fn import(
    d: &Arc<Daemon>,
    params: ConductorImportParams,
) -> Result<ConductorImportResult> {
    let _running = RUNNING.lock().await;
    let db_path = params
        .db
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .map(expand_home)
        .unwrap_or_else(default_db);
    let data = {
        let path = db_path.clone();
        tokio::task::spawn_blocking(move || db::read(&path))
            .await
            .map_err(|e| Error::Internal(e.into()))??
    };
    let git = Git::new(d.shell_env.clone());
    let mut repos = Vec::new();
    for repo in &data.repos {
        repos.push(plan_repo(d, &git, repo, &data.env).await);
    }
    let mut planned = Vec::new();
    for workspace in select(&data.workspaces, &params)? {
        planned
            .push(plan_workspace(d, &git, &data, &repos, workspace, params.include_archived).await);
    }
    if !params.dry_run {
        write_repo_settings(d, &repos)?;
        for workspace in &mut planned {
            if workspace.plan.outcome != ConductorOutcome::Import {
                continue;
            }
            let outcome = if workspace.plan.archived {
                import_archived(d, &db_path, workspace).await
            } else {
                import_active(d, &git, &db_path, workspace).await
            };
            match outcome {
                Ok(slug) => {
                    workspace.plan.outcome = ConductorOutcome::Imported;
                    workspace.plan.workspace = Some(slug);
                }
                Err(error) => {
                    tracing::warn!(%error, workspace = workspace.plan.name, "could not import a conductor workspace");
                    workspace.plan.outcome = ConductorOutcome::Failed;
                    workspace.plan.reason = Some(error.to_rpc().message);
                }
            }
        }
    }
    let archived_left_out = if params.include_archived || !params.workspaces.is_empty() {
        0
    } else {
        data.workspaces.iter().filter(|w| w.archived).count()
    };
    Ok(ConductorImportResult {
        archived_left_out,
        dry_run: params.dry_run,
        db: db_path.to_string_lossy().into_owned(),
        repos: repos.into_iter().map(|r| r.plan).collect(),
        workspaces: planned.into_iter().map(|w| w.plan).collect(),
    })
}

/// The workspaces asked for: those named (by name, folder, branch or id), else every active one
/// and, with `include_archived`, every archived one. A name nothing matches refuses the import.
fn select<'a>(
    workspaces: &'a [db::Workspace],
    params: &ConductorImportParams,
) -> Result<Vec<&'a db::Workspace>> {
    if params.workspaces.is_empty() {
        return Ok(workspaces
            .iter()
            .filter(|w| params.include_archived || !w.archived)
            .collect());
    }
    let mut picked: Vec<&db::Workspace> = Vec::new();
    for asked in &params.workspaces {
        let asked = asked.trim();
        let matches: Vec<&db::Workspace> = workspaces
            .iter()
            .filter(|w| {
                w.id == asked
                    || w.name().eq_ignore_ascii_case(asked)
                    || w.directory_name == asked
                    || w.branch.as_deref() == Some(asked)
            })
            .collect();
        if matches.is_empty() {
            return Err(Error::NotFound(format!(
                "No Conductor workspace is named {asked}: `chauffeur import conductor --dry-run --include-archived` lists them"
            )));
        }
        for workspace in matches {
            if !picked.iter().any(|p| p.id == workspace.id) {
                picked.push(workspace);
            }
        }
    }
    Ok(picked)
}

/// `owner/name` of a GitHub remote URL (`git@github.com:o/n.git`, `https://github.com/o/n`,
/// `ssh://git@github.com/o/n`).
fn github_repo(url: &str) -> Option<(String, String)> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let (_, path) = url.split_once("github.com")?;
    let mut parts = path.trim_start_matches([':', '/']).split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((owner.to_string(), name.to_string()))
}

/// Chauffeur's name of a GitHub repo: its short name in the main organization, else `owner/name`.
fn reference(d: &Daemon, owner: &str, name: &str) -> String {
    let org = d.settings.get().github_org.clone();
    if !org.is_empty() && org.eq_ignore_ascii_case(owner) {
        name.to_string()
    } else {
        format!("{owner}/{name}")
    }
}

fn same_folder(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The GitHub remote of a clone: Conductor's record of it, else its `origin` as configured (not
/// as rewritten by `insteadOf`).
async fn remote_of(git: &Git, clone: &Path, recorded: Option<&str>) -> Option<(String, String)> {
    if let Some(found) = recorded.and_then(github_repo) {
        return Some(found);
    }
    let url = git
        .run(clone, &["config", "--get", "remote.origin.url"])
        .await
        .ok()?;
    github_repo(&url)
}

async fn plan_repo(d: &Daemon, git: &Git, repo: &db::Repo, env: &[db::EnvVar]) -> Repo {
    let mut planned = Repo {
        plan: ConductorRepoPlan {
            conductor_name: repo.name.clone(),
            name: None,
            conductor_path: repo.root_path.clone(),
            clone: ConductorClone::None,
            clone_path: None,
            scripts: Vec::new(),
            env: Vec::new(),
            skipped: None,
        },
        conductor_id: repo.id.clone(),
        clone_root: None,
        scripts: Vec::new(),
        env: Vec::new(),
    };
    let root = PathBuf::from(&repo.root_path);
    if !root.join(".git").exists() {
        planned.plan.skipped = Some(format!("its clone {} is gone", repo.root_path));
        return planned;
    }
    let Some((owner, name)) = remote_of(git, &root, repo.remote_url.as_deref()).await else {
        planned.plan.skipped = Some("it has no GitHub remote".into());
        return planned;
    };
    let reference = reference(d, &owner, &name);
    match d.repos.existing_clone(&reference) {
        Some(clone) if same_folder(&clone, &root) => {
            planned.plan.clone = ConductorClone::Shared;
            planned.plan.clone_path = Some(clone.to_string_lossy().into_owned());
        }
        Some(clone) => {
            planned.plan.clone = ConductorClone::Chauffeur;
            planned.plan.clone_path = Some(clone.to_string_lossy().into_owned());
        }
        // Found by its folder's name under a clone folder, as Chauffeur finds clones.
        None if root
            .file_name()
            .is_some_and(|f| f.to_string_lossy() == name) =>
        {
            planned.plan.clone = ConductorClone::Conductor;
            planned.plan.clone_path = Some(repo.root_path.clone());
            planned.clone_root = root.parent().map(Path::to_path_buf);
        }
        None => {
            planned.plan.skipped = Some(format!(
                "its clone's folder is not named {name}, as Chauffeur's clone folders need"
            ));
            return planned;
        }
    }
    let key = repo_folder(&reference).to_string();
    let settings = d.settings.get();
    let user = settings.repos.get(&key);
    let has_script = |script: &str| {
        user.and_then(|o| o.scripts.as_ref())
            .is_some_and(|s| s.has(script))
    };
    let typed = |text: &Option<String>| {
        text.as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from)
    };
    for (script, text) in [
        ("setup", typed(&repo.setup_script)),
        ("run", typed(&repo.run_script)),
        ("archive", typed(&repo.archive_script)),
    ] {
        if let Some(text) = text
            && !has_script(script)
        {
            planned.scripts.push((script.to_string(), text));
        }
    }
    if typed(&repo.run_script).is_some()
        && repo.run_script_mode.as_deref() == Some("nonconcurrent")
        && !has_script("run_mode")
    {
        planned
            .scripts
            .push(("run_mode".into(), "nonconcurrent".into()));
    }
    // Conductor's variables of this repo, and its global ones.
    for var in env.iter().filter(|v| {
        !v.key.is_empty() && (v.context == repo.id || v.scope == "global" || v.context.is_empty())
    }) {
        let set = user.is_some_and(|o| o.env.0.contains_key(&var.key));
        if !set && !planned.env.iter().any(|(k, _)| *k == var.key) {
            planned.env.push((var.key.clone(), var.value.clone()));
        }
    }
    planned.plan.scripts = planned.scripts.iter().map(|(k, _)| k.clone()).collect();
    planned.plan.env = planned.env.iter().map(|(k, _)| k.clone()).collect();
    planned.plan.name = Some(reference);
    planned
}

fn skipped(plan: &mut ConductorWorkspacePlan, reason: impl Into<String>) {
    plan.outcome = ConductorOutcome::Skipped;
    plan.reason = Some(reason.into());
}

async fn plan_workspace(
    d: &Daemon,
    git: &Git,
    data: &db::Data,
    repos: &[Repo],
    workspace: &db::Workspace,
    include_archived: bool,
) -> Planned {
    let repo = repos
        .iter()
        .find(|r| workspace.repo_id.as_deref() == Some(r.conductor_id.as_str()));
    let sessions: Vec<db::Session> = data
        .sessions
        .iter()
        .filter(|s| s.workspace_id == workspace.id && s.messages > 0)
        .map(|s| db::Session {
            id: s.id.clone(),
            workspace_id: s.workspace_id.clone(),
            agent: s.agent.clone(),
            title: s.title.clone(),
            model: s.model.clone(),
            created_at: s.created_at.clone(),
            messages: s.messages,
        })
        .collect();
    let mut planned = Planned {
        plan: ConductorWorkspacePlan {
            conductor_id: workspace.id.clone(),
            name: workspace.name().to_string(),
            repo: repo.and_then(|r| r.plan.name.clone()),
            branch: workspace.branch.clone().unwrap_or_default(),
            path: None,
            archived: workspace.archived,
            repos: Vec::new(),
            release: false,
            changes: 0,
            chats: sessions.len(),
            ticket: None,
            outcome: ConductorOutcome::Import,
            reason: None,
            workspace: None,
            warnings: Vec::new(),
        },
        created_at: sessions.first().map(|s| s.created_at.clone()),
        sessions,
        notes: workspace.notes.clone().filter(|n| !n.trim().is_empty()),
        conductor_clone: repo.map(|r| PathBuf::from(&r.plan.conductor_path)),
        chauffeur_clone: repo.and_then(|r| r.plan.clone_path.as_ref().map(PathBuf::from)),
        ticket: None,
    };
    let plan = &mut planned.plan;
    if let Ok(Some(id)) = d.store.kv_get(&format!("{IMPORTED}{}", workspace.id)) {
        plan.outcome = ConductorOutcome::AlreadyImported;
        plan.workspace = d.store.find_workspace(&id).ok().flatten().map(|w| w.slug);
        return planned;
    }
    if workspace.remote {
        skipped(
            plan,
            "a Conductor Cloud workspace: transfer it to local in Conductor before November 13, then run the import again",
        );
        return planned;
    }
    if workspace.archived {
        if !include_archived {
            skipped(plan, "archived in Conductor: pass --include-archived");
        } else if let Some(name) = repo
            .filter(|r| r.usable())
            .and_then(|r| r.plan.name.clone())
        {
            plan.repos.push(name);
        }
        return planned;
    }
    let Some(repo) = repo else {
        skipped(plan, "its repo is not in Conductor's database");
        return planned;
    };
    let (Some(name), None) = (repo.plan.name.clone(), &repo.plan.skipped) else {
        let why = repo.plan.skipped.clone().unwrap_or_default();
        skipped(
            plan,
            format!(
                "its repo {} is not imported: {why}",
                repo.plan.conductor_name
            ),
        );
        return planned;
    };
    let folder = workspace
        .path
        .clone()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            expand_home("~/conductor/workspaces")
                .join(&repo.plan.conductor_name)
                .join(&workspace.directory_name)
        });
    plan.path = Some(folder.to_string_lossy().into_owned());
    if !folder.join(".git").exists() {
        skipped(plan, format!("its folder {} is gone", folder.display()));
        return planned;
    }
    let checked_out = git
        .run(&folder, &["symbolic-ref", "-q", "--short", "HEAD"])
        .await
        .ok()
        .filter(|b| !b.is_empty());
    match checked_out {
        Some(branch) => {
            plan.branch = branch;
            plan.release = repo.plan.clone != ConductorClone::Chauffeur;
        }
        None if !plan.branch.is_empty()
            && git
                .ok(
                    &folder,
                    &[
                        "rev-parse",
                        "-q",
                        "--verify",
                        &format!("refs/heads/{}", plan.branch),
                    ],
                )
                .await => {}
        None => {
            skipped(
                plan,
                "its folder is on a detached HEAD, with no branch to import",
            );
            return planned;
        }
    }
    let conductor_clone = PathBuf::from(&repo.plan.conductor_path);
    let chauffeur_clone = planned
        .chauffeur_clone
        .clone()
        .unwrap_or_else(|| conductor_clone.clone());
    if let Some(reason) = branch_conflict(
        git,
        &chauffeur_clone,
        &conductor_clone,
        &folder,
        &plan.branch,
    )
    .await
    {
        skipped(plan, reason);
        return planned;
    }
    plan.changes = changes(git, &folder)
        .await
        .map(|c| c.files())
        .unwrap_or_default();
    plan.repos.push(name.clone());
    for linked in &workspace.linked {
        match linked_repo(d, git, repos, Path::new(linked)).await {
            Ok(other) if other != name && !plan.repos.contains(&other) => plan.repos.push(other),
            Ok(_) => {}
            Err(why) => plan
                .warnings
                .push(format!("its /add-dir folder {linked} is not added: {why}")),
        }
    }
    planned.ticket = ticket(d, &plan.branch).await;
    plan.ticket = planned.ticket.as_ref().map(|t| t.identifier.clone());
    planned
}

/// Why the branch cannot come to Chauffeur: another worktree than Conductor's holds it, or
/// Chauffeur's own clone has another branch of that name.
async fn branch_conflict(
    git: &Git,
    chauffeur_clone: &Path,
    conductor_clone: &Path,
    folder: &Path,
    branch: &str,
) -> Option<String> {
    let entries = git.worktree_list(chauffeur_clone).await.ok()?;
    if let Some(holder) = entries.iter().find(|e| {
        !e.prunable && e.branch.as_deref() == Some(branch) && !same_folder(&e.path, folder)
    }) {
        return Some(format!(
            "branch {branch} is checked out in {} too",
            holder.path.display()
        ));
    }
    if same_folder(chauffeur_clone, conductor_clone) {
        return None;
    }
    let reference = format!("refs/heads/{branch}");
    let ours = git.rev_parse(chauffeur_clone, &reference).await.ok()?;
    let theirs = git.rev_parse(conductor_clone, &reference).await.ok()?;
    (ours != theirs).then(|| {
        format!(
            "Chauffeur's clone {} has another branch {branch}",
            chauffeur_clone.display()
        )
    })
}

/// The Chauffeur repo of an `/add-dir` folder: a Conductor repo's clone, or a clone Chauffeur
/// knows by its GitHub remote.
async fn linked_repo(
    d: &Daemon,
    git: &Git,
    repos: &[Repo],
    folder: &Path,
) -> std::result::Result<String, String> {
    if let Some(repo) = repos
        .iter()
        .find(|r| same_folder(Path::new(&r.plan.conductor_path), folder))
    {
        return match (&repo.plan.name, &repo.plan.skipped) {
            (Some(name), None) => Ok(name.clone()),
            (_, skipped) => Err(skipped.clone().unwrap_or_default()),
        };
    }
    if !folder.join(".git").exists() {
        return Err("it is not a git clone".into());
    }
    let (owner, name) = remote_of(git, folder, None)
        .await
        .ok_or("it has no GitHub remote")?;
    let reference = reference(d, &owner, &name);
    match d.repos.existing_clone(&reference) {
        Some(_) => Ok(reference),
        None => Err(format!("Chauffeur has no clone of {reference}")),
    }
}

/// The Linear issue a branch's name starts with (`dat-220-…`, `me/dat-220-…`), when Linear is on
/// and has it.
async fn ticket(d: &Daemon, branch: &str) -> Option<TicketRef> {
    let identifier = branch_ticket(branch)?;
    if !d.settings.get().linear_enabled || !d.linear.is_configured() {
        return None;
    }
    let details = d.linear.issue(&identifier).await.ok()?;
    Some(TicketRef {
        identifier: details.issue.identifier,
        title: Some(details.issue.title),
        url: Some(details.issue.url),
    })
}

fn branch_ticket(branch: &str) -> Option<String> {
    let last = branch.rsplit('/').next()?;
    let (team, rest) = last.split_once('-')?;
    let number: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let after = &rest[number.len()..];
    let team_ok = (2..=10).contains(&team.len()) && team.chars().all(|c| c.is_ascii_alphabetic());
    (team_ok && !number.is_empty() && (after.is_empty() || after.starts_with('-')))
        .then(|| format!("{}-{number}", team.to_ascii_uppercase()))
}

/// A worktree's uncommitted work: tracked changes against HEAD and untracked files.
struct Changes {
    patch: Vec<u8>,
    untracked: Vec<String>,
    tracked: usize,
}

impl Changes {
    fn files(&self) -> usize {
        self.tracked + self.untracked.len()
    }
}

/// Reads the uncommitted work of Conductor's worktree, writing nothing there: no optional locks
/// (git would otherwise refresh its index).
async fn changes(git: &Git, folder: &Path) -> anyhow::Result<Changes> {
    let patch = git
        .run_bytes(folder, &["--no-optional-locks", "diff", "--binary", "HEAD"])
        .await?;
    let tracked = git
        .run_bytes(
            folder,
            &["--no-optional-locks", "diff", "--name-only", "-z", "HEAD"],
        )
        .await?
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .count();
    let untracked = git
        .run_bytes(
            folder,
            &[
                "--no-optional-locks",
                "ls-files",
                "--others",
                "--exclude-standard",
                "-z",
            ],
        )
        .await?
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect();
    Ok(Changes {
        patch,
        untracked,
        tracked,
    })
}

/// Adds Conductor's clone folders to the clone folders and the repos' typed scripts and variables
/// to `[repos.<name>]`, keeping what the user already set.
fn write_repo_settings(d: &Daemon, repos: &[Repo]) -> Result<()> {
    let current = d.settings.get();
    let known: Vec<PathBuf> = current.clone_roots.iter().map(|r| expand_home(r)).collect();
    let mut roots: Vec<String> = Vec::new();
    for root in repos
        .iter()
        .filter(|r| r.usable())
        .filter_map(|r| r.clone_root.as_ref())
    {
        let text = root.to_string_lossy().into_owned();
        if !known.iter().any(|k| same_folder(k, root)) && !roots.contains(&text) {
            roots.push(text);
        }
    }
    let changes: Vec<&Repo> = repos
        .iter()
        .filter(|r| r.usable() && (!r.scripts.is_empty() || !r.env.is_empty()))
        .collect();
    if roots.is_empty() && changes.is_empty() {
        return Ok(());
    }
    d.settings
        .update(|doc| {
            if !roots.is_empty() {
                let array = doc
                    .entry("clone_roots")
                    .or_insert_with(|| toml_edit::value(toml_edit::Array::new()))
                    .as_array_mut();
                if let Some(array) = array {
                    for root in &roots {
                        array.push(root.as_str());
                    }
                }
            }
            for repo in &changes {
                let Some(name) = repo.plan.name.as_deref() else {
                    continue;
                };
                let key = repo_folder(name);
                let table = implicit_table(doc.as_table_mut(), "repos");
                let table = implicit_table(table, key);
                if !repo.scripts.is_empty() {
                    let scripts = implicit_table(table, "scripts");
                    for (script, text) in &repo.scripts {
                        scripts.insert(script, toml_edit::value(text.as_str()));
                    }
                }
                if !repo.env.is_empty() {
                    let env = implicit_table(table, "env");
                    for (var, value) in &repo.env {
                        env.insert(var, toml_edit::value(value.as_str()));
                    }
                }
            }
        })
        .map_err(|e| Error::Internal(e.context("writing the imported repos' settings")))
}

fn implicit_table<'a>(table: &'a mut toml_edit::Table, key: &str) -> &'a mut toml_edit::Table {
    let item = table.entry(key).or_insert_with(|| {
        let mut new = toml_edit::Table::new();
        new.set_implicit(true);
        toml_edit::Item::Table(new)
    });
    if !item.is_table() {
        *item = toml_edit::Item::Table(toml_edit::Table::new());
    }
    item.as_table_mut().expect("just made a table")
}

/// Copies an active workspace: the branch released in Conductor (or fetched into Chauffeur's own
/// clone), a Chauffeur workspace on it, the uncommitted work re-applied, then its notes, chats
/// and ticket. Returns the workspace's slug.
async fn import_active(
    d: &Arc<Daemon>,
    git: &Git,
    db_path: &Path,
    workspace: &mut Planned,
) -> Result<String> {
    let plan = workspace.plan.clone();
    let folder = PathBuf::from(plan.path.as_deref().unwrap_or_default());
    let conductor_clone = workspace.conductor_clone.clone().unwrap_or_default();
    let changes = changes(git, &folder)
        .await
        .map_err(|e| Error::Internal(e.context("reading the uncommitted changes")))?;
    let head = git
        .rev_parse(&folder, "HEAD")
        .await
        .map_err(Error::Internal)?;
    if plan.release {
        git.run(
            &folder,
            &[
                "update-ref",
                "--no-deref",
                "-m",
                "chauffeur: branch released by the import",
                "HEAD",
                &head,
            ],
        )
        .await
        .map_err(|e| Error::Internal(e.context("releasing the branch in Conductor")))?;
    } else if let Some(clone) = workspace
        .chauffeur_clone
        .as_ref()
        .filter(|c| !same_folder(c, &conductor_clone))
    {
        let refspec = format!("refs/heads/{0}:refs/heads/{0}", plan.branch);
        git.run(
            clone,
            &[
                "fetch",
                "--no-tags",
                &conductor_clone.to_string_lossy(),
                &refspec,
            ],
        )
        .await
        .map_err(|e| Error::Internal(e.context("fetching the branch from Conductor's clone")))?;
    }
    let created = d
        .workspaces
        .create(CreateWorkspaceParams {
            name: Some(plan.name.clone()),
            repos: plan.repos.clone(),
            branch: Some(plan.branch.clone()),
            ..Default::default()
        })
        .await;
    let created = match created {
        Ok(created) => created.workspace,
        Err(error) => {
            if plan.release {
                put_back(git, &conductor_clone, &folder, &plan.branch, &head).await;
            }
            return Err(error);
        }
    };
    let warnings = &mut workspace.plan.warnings;
    let root = PathBuf::from(&created.path);
    let main = root.join(repo_folder(
        plan.repos.first().map(String::as_str).unwrap_or_default(),
    ));
    if let Err(error) = reapply(git, &changes, &folder, &main).await {
        warnings.push(format!(
            "the uncommitted changes could not all be re-applied ({error:#}): they are still in {}",
            folder.display()
        ));
    }
    let notes = root.join(NOTES_DIR).join(CONDUCTOR_NOTES);
    let context = folder.join(".context");
    if context.is_dir()
        && let Err(error) = crate::workspaces::copy_dir(&context, &notes.join("context"))
    {
        warnings.push(format!(
            "Conductor's notes (.context) could not be copied: {error:#}"
        ));
    }
    write_notes_and_history(
        db_path,
        &notes,
        workspace.notes.clone(),
        &workspace.sessions,
        warnings,
    )
    .await;
    if let Some(ticket) = workspace.ticket.clone() {
        d.workspaces.attach_tickets(&created.id, &[ticket])?;
    }
    d.workspaces.regenerate_config(&created.id)?;
    d.store.kv_set(
        &format!("{IMPORTED}{}", workspace.plan.conductor_id),
        &created.id,
    )?;
    Ok(created.slug)
}

/// After a failed creation: the branch back in Conductor's worktree, made again at its commit if
/// the creation's rollback deleted it.
async fn put_back(git: &Git, clone: &Path, folder: &Path, branch: &str, head: &str) {
    let reference = format!("refs/heads/{branch}");
    if !git
        .ok(clone, &["rev-parse", "-q", "--verify", &reference])
        .await
        && let Err(error) = git
            .run(clone, &["branch", "--end-of-options", branch, head])
            .await
    {
        tracing::error!(%error, branch, "could not make a released branch again");
    }
    if let Err(error) = git.run(folder, &["symbolic-ref", "HEAD", &reference]).await {
        tracing::error!(%error, branch, "could not attach conductor's worktree to its branch again");
    }
}

/// Applies the tracked changes and copies the untracked files into the new worktree.
async fn reapply(git: &Git, changes: &Changes, from: &Path, to: &Path) -> anyhow::Result<()> {
    if !changes.patch.is_empty() {
        let patch = tempfile::NamedTempFile::new()?;
        std::fs::write(patch.path(), &changes.patch)?;
        git.run(
            to,
            &[
                "apply",
                "--binary",
                "--whitespace=nowarn",
                &patch.path().to_string_lossy(),
            ],
        )
        .await?;
    }
    for file in &changes.untracked {
        let source = from.join(file);
        let target = to.join(file);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let meta = std::fs::symlink_metadata(&source)?;
        if meta.file_type().is_symlink() {
            let _ = std::fs::remove_file(&target);
            std::os::unix::fs::symlink(std::fs::read_link(&source)?, &target)?;
        } else {
            std::fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

/// Conductor's notes and one Markdown file per chat, into `folder`.
async fn write_notes_and_history(
    db_path: &Path,
    folder: &Path,
    notes: Option<String>,
    sessions: &[db::Session],
    warnings: &mut Vec<String>,
) {
    if let Err(error) = std::fs::create_dir_all(folder) {
        warnings.push(format!("the chat history could not be written: {error}"));
        return;
    }
    if let Some(notes) = notes
        && let Err(error) = std::fs::write(folder.join("notes.md"), notes)
    {
        warnings.push(format!("Conductor's notes could not be written: {error}"));
    }
    let mut taken = HashSet::new();
    for session in sessions {
        let path = db_path.to_path_buf();
        let id = session.id.clone();
        let messages = tokio::task::spawn_blocking(move || db::messages(&path, &id)).await;
        let written = match messages {
            Ok(Ok(messages)) => {
                let file = folder.join(history::file_name(&session.title, &mut taken));
                std::fs::write(file, history::render(session, &messages)).map_err(Error::from)
            }
            Ok(Err(error)) => Err(error),
            Err(error) => Err(Error::Internal(error.into())),
        };
        if let Err(error) = written {
            warnings.push(format!(
                "the chat {} could not be written: {error}",
                session.title
            ));
        }
    }
}

/// An archived Conductor workspace: an archived Chauffeur workspace holding its chat history.
async fn import_archived(
    d: &Arc<Daemon>,
    db_path: &Path,
    workspace: &mut Planned,
) -> Result<String> {
    let base = crate::slug::workspace_slug(Some(&workspace.plan.name), None);
    let store = d.store.clone();
    let workspaces_dir = d.paths.workspaces_dir.clone();
    let archive_dir = d.paths.archive_dir.clone();
    let slug = crate::slug::unique(&base, |candidate| {
        store.slug_exists(candidate).unwrap_or(true)
            || workspaces_dir.join(candidate).exists()
            || archive_dir.join(candidate).exists()
    });
    let notes = d.paths.archive_dir.join(&slug).join(CONDUCTOR_NOTES);
    write_notes_and_history(
        db_path,
        &notes,
        workspace.notes.clone(),
        &workspace.sessions,
        &mut workspace.plan.warnings,
    )
    .await;
    let stamp = workspace
        .created_at
        .as_deref()
        .map(rfc3339)
        .unwrap_or_else(now);
    let row = WorkspaceRow {
        id: uuid::Uuid::now_v7().to_string(),
        slug: slug.clone(),
        path: d.paths.workspace_dir(&slug).to_string_lossy().into_owned(),
        branch: workspace.plan.branch.clone(),
        parent_id: None,
        state: WorkspaceState::Archived,
        tickets: Vec::new(),
        created_at: stamp.clone(),
        archived_at: Some(now()),
        last_error: None,
        attention_at: None,
        seen_at: None,
        explore: false,
        auto_title: false,
        pinned: false,
        position: 0.0,
        title: Some(workspace.plan.name.clone()),
        title_source: Some(TitleSource::User),
        section_id: None,
        house_position: None,
    };
    d.store.insert_workspace(&row)?;
    d.store.kv_set(
        &format!("{IMPORTED}{}", workspace.plan.conductor_id),
        &row.id,
    )?;
    d.workspaces.publish(&row.id);
    Ok(slug)
}

/// Conductor's timestamps as the store's: `2026-07-20 15:31:57` becomes `2026-07-20T15:31:57Z`.
fn rfc3339(stamp: &str) -> String {
    let stamp = stamp.replacen(' ', "T", 1);
    if stamp.ends_with('Z') {
        stamp
    } else {
        format!("{stamp}Z")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_remotes_are_read_in_every_form_git_writes_them() {
        for url in [
            "git@github.com:acme/web.git",
            "https://github.com/acme/web",
            "https://github.com/acme/web.git/",
            "ssh://git@github.com/acme/web.git",
        ] {
            assert_eq!(
                github_repo(url),
                Some(("acme".into(), "web".into())),
                "{url}"
            );
        }
        assert_eq!(github_repo("https://gitlab.com/acme/web.git"), None);
        assert_eq!(github_repo("https://github.com/acme"), None);
    }

    #[test]
    fn a_linear_issue_is_read_from_the_start_of_the_branch_name_only() {
        assert_eq!(
            branch_ticket("dat-220-langfuse-batch").as_deref(),
            Some("DAT-220")
        );
        assert_eq!(
            branch_ticket("justin/eng-1374").as_deref(),
            Some("ENG-1374")
        );
        assert_eq!(branch_ticket("justincuzin/poc-agentique-factory"), None);
        assert_eq!(branch_ticket("v1-2"), None);
        assert_eq!(branch_ticket("dat-22x-thing"), None);
        assert_eq!(branch_ticket("main"), None);
    }

    #[test]
    fn conductors_two_timestamp_forms_become_the_stores() {
        assert_eq!(rfc3339("2026-07-20 15:31:57"), "2026-07-20T15:31:57Z");
        assert_eq!(
            rfc3339("2026-10-02T05:37:21.545Z"),
            "2026-10-02T05:37:21.545Z"
        );
    }
}
