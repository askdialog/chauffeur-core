//! `claudecode.import` (docs/adr/claude-code-import.md): Claude Code's sessions into Chauffeur.
//! For now it plans: which sessions it reads, the branches each wrote on or created, and where
//! each lands (a workspace on a live branch, a parent with its children, a workspace carrying a
//! clone's uncommitted default-branch changes, or its repo's history exploration). It reads
//! Claude Code's transcripts and the repos with git, writing nothing anywhere.

mod place;
mod sessions;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chauffeur_protocol::claudecode::{
    ClaudeCodeBranch, ClaudeCodeImportParams, ClaudeCodeImportResult, ClaudeCodeOutcome,
    ClaudeCodeRelease, ClaudeCodeSessionPlan,
};

use crate::conductor::{reference, remote_of};
use crate::git::Git;
use crate::settings::expand_home;
use crate::worker_claude_config::Locations;
use crate::{Daemon, Error, Result};

/// The window read by default, in days.
const DEFAULT_DAYS: u32 = 60;

/// The merged PRs read per repo to tell a squash-merged branch: the most recent.
const MERGED_PRS: u32 = 1000;

/// The switch the import waits behind until it is complete: without it, only a dry run runs.
const SWITCH: &str = "CHAUFFEUR_IMPORT_CLAUDE_CODE";

pub async fn import(
    d: &Arc<Daemon>,
    params: ClaudeCodeImportParams,
) -> Result<ClaudeCodeImportResult> {
    if !params.dry_run && std::env::var(SWITCH).as_deref() != Ok("1") {
        return Err(Error::BadRequest(
            "The import from Claude Code is not ready yet: `chauffeur import claude-code --dry-run` shows its plan".into(),
        ));
    }
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
        None => Locations::of(&d.shell_env.vars())
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
    let mut placed = Vec::new();
    for session in read {
        let Some(folder) = session.folder.clone() else {
            continue;
        };
        // Chauffeur's own chats are Claude Code sessions too, run through the Agent SDK (as other
        // apps' are): only sessions of Claude Code itself, in a terminal or an editor, come.
        if real(&folder).starts_with(real(&d.paths.workspaces_dir))
            || session
                .entrypoint
                .as_deref()
                .is_some_and(|e| e.starts_with("sdk"))
        {
            continue;
        }
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
            plan.reason = Some(format!("{} has no GitHub remote", home.clone.display()));
            plans.push(plan);
            continue;
        };
        plan.branches = repos.branches(&session).await;
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
        plans.push(plan);
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
    let mut changes = Vec::new();
    let mut bases = HashMap::new();
    for repo in &touched {
        for clone in repos.clones.get(repo).cloned().unwrap_or_default() {
            let base = repos.refs(&clone).await.base.clone();
            if repos.default_has_changes(&clone, &base).await && !changes.contains(repo) {
                changes.push(repo.clone());
            }
            bases.entry(repo.clone()).or_insert(base);
        }
    }
    let plan = place::place(&placed, &changes, &bases);
    for session in &mut plans {
        session.lands_in = plan.lands_in.get(&session.id).cloned();
    }
    let mut releases = Vec::new();
    for workspace in &plan.workspaces {
        let Some(branch) = &workspace.branch else {
            continue;
        };
        for repo in &workspace.repos {
            releases.extend(repos.holders(repo, branch).await);
        }
    }
    warnings.extend(repos.warnings);
    let mut said = BTreeSet::new();
    warnings.retain(|w| said.insert(w.clone()));
    Ok(ClaudeCodeImportResult {
        dry_run: params.dry_run,
        since_days,
        since_date,
        claude_dir: claude.to_string_lossy().into_owned(),
        sessions: plans,
        workspaces: plan.workspaces,
        histories: plan.histories,
        releases,
        warnings,
    })
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
    /// Chauffeur's name of the repo; none without a GitHub remote.
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
    /// Each clone's merged PRs (head, base) by branch, read once; none when GitHub could not say.
    merged: HashMap<PathBuf, Option<MergedPrs>>,
    refs: HashMap<PathBuf, Refs>,
    /// Clones without a GitHub remote, warned about once.
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
        let common = self
            .git
            .run(
                &top,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await
            .ok()?;
        let common = PathBuf::from(common.trim());
        let clone = if common.file_name().is_some_and(|n| n == ".git") {
            common.parent()?.to_path_buf()
        } else {
            common
        };
        let repo = match remote_of(&self.git, &clone, None).await {
            Some((owner, name)) => {
                let name = reference(self.d, &owner, &name);
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
                        "{} has no GitHub remote: its sessions and branches are left out.",
                        clone.display()
                    ));
                }
                None
            }
        };
        let branch = self
            .git
            .run(&top, &["symbolic-ref", "-q", "--short", "HEAD"])
            .await
            .ok()
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty());
        Some(Checkout {
            top,
            clone,
            repo,
            branch,
        })
    }

    /// The branches a session wrote on (its file tools' paths, each in the checkout holding it, on
    /// the branch checked out there then) or created, by repo, each with whether it is live.
    async fn branches(&mut self, session: &sessions::Session) -> Vec<ClaudeCodeBranch> {
        let mut found: Vec<(String, PathBuf, String)> = Vec::new();
        let mut gone = BTreeSet::new();
        for write in &session.writes {
            let (path, cwd) = (real(&write.path), real(&write.cwd));
            // The checkout the session stood in (from its nearest folder still there: a worktree
            // removed since resolves to its clone, the same repo), on the branch recorded then.
            let stood = self.checkout(&cwd).await;
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
            if let (Some(repo), Some(branch)) = (checkout.repo.clone(), branch) {
                found.push((repo, checkout.clone, branch));
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
        branches
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
        let Some(heads) = self.merged_heads(clone, branch).await else {
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

    /// The (head, base) of the merged PRs from `branch`: on GitHub from the repo's recent merged
    /// PRs, read once (`MERGED_PRS`); on GitLab, asked for that branch.
    async fn merged_heads(&mut self, clone: &Path, branch: &str) -> Option<Vec<(String, String)>> {
        let forge = self.d.settings.get().forge;
        if forge == chauffeur_protocol::client::Forge::Gitlab {
            let prs = crate::prs::pr_list(
                forge,
                &self.d.shell_env,
                clone,
                branch,
                "merged",
                "headRefOid,baseRefName",
            )
            .await?;
            let base = self.refs(clone).await.base.clone();
            return Some(
                prs.iter()
                    .filter_map(|pr| {
                        let head = pr["headRefOid"].as_str()?.to_string();
                        Some((
                            head,
                            pr["baseRefName"]
                                .as_str()
                                .map_or_else(|| base.clone(), String::from),
                        ))
                    })
                    .collect(),
            );
        }
        if !self.merged.contains_key(clone) {
            let heads = crate::prs::merged_heads(&self.d.shell_env, clone, MERGED_PRS).await;
            self.merged.insert(clone.to_path_buf(), heads);
        }
        let heads = self.merged.get(clone)?.as_ref()?;
        if let Some(found) = heads.get(branch) {
            return Some(found.clone());
        }
        // The list stopped at its limit: an older PR is asked for by its branch.
        if heads.values().map(Vec::len).sum::<usize>() < MERGED_PRS as usize {
            return Some(Vec::new());
        }
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
                        pr["baseRefName"].as_str()?.to_string(),
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
        let (base, target) = self.base(clone).await;
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
    /// else `main` or `master`, whichever it has; the remote's copy when there is one.
    async fn base(&self, clone: &Path) -> (String, String) {
        let remote = self.git.base_remote(clone).await;
        let has = |reference: String| async move {
            self.git
                .ok(clone, &["rev-parse", "-q", "--verify", &reference])
                .await
                .then_some(reference)
        };
        let set = self.names.get(clone).and_then(|name| {
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
        let on = self
            .git
            .run(clone, &["symbolic-ref", "-q", "--short", "HEAD"])
            .await;
        on.is_ok_and(|b| b.trim() == base) && self.has_changes(clone).await
    }

    /// Uncommitted changes, untracked files included, read without refreshing the index.
    async fn has_changes(&self, folder: &Path) -> bool {
        self.git
            .run(folder, &["--no-optional-locks", "status", "--porcelain"])
            .await
            .is_ok_and(|out| !out.trim().is_empty())
    }

    /// Where a live branch is checked out: one of the repo's clones or the user's worktrees,
    /// not Chauffeur's.
    async fn holders(&mut self, repo: &str, branch: &str) -> Vec<ClaudeCodeRelease> {
        let workspaces = self.workspaces.clone();
        let mut releases = Vec::new();
        for clone in self.clones.get(repo).cloned().unwrap_or_default() {
            for e in &self.refs(&clone).await.worktrees {
                if e.branch.as_deref() == Some(branch)
                    && !e.prunable
                    && !e.path.starts_with(&workspaces)
                {
                    releases.push(ClaudeCodeRelease {
                        repo: repo.to_string(),
                        branch: branch.to_string(),
                        folder: e.path.to_string_lossy().into_owned(),
                    });
                }
            }
        }
        releases
    }
}
