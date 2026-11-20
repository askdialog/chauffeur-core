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
        Some(dir) => expand_home(dir),
        None => Locations::of(&d.shell_env.vars())
            .map(|at| at.config().to_path_buf())
            .ok_or_else(|| {
                Error::BadRequest("No home folder to find Claude Code's sessions in".into())
            })?,
    };
    let since_days = (!params.all).then(|| params.since_days.unwrap_or(DEFAULT_DAYS));
    let cutoff = since_days.map(|days| {
        (chrono::Utc::now() - chrono::Duration::days(i64::from(days)))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    });
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
    let mut plans = Vec::new();
    let mut placed = Vec::new();
    for session in read {
        let Some(folder) = session.folder.clone() else {
            continue;
        };
        // Chauffeur's own chats are Claude Code sessions too, run through the Agent SDK (as other
        // apps' are): only sessions of Claude Code itself, in a terminal or an editor, come.
        if folder.starts_with(&d.paths.workspaces_dir)
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
        plan.branches = repos.branches(&session, &mut warnings).await;
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
    let touched: BTreeSet<String> = placed
        .iter()
        .flat_map(|s| {
            std::iter::once(s.repo.clone()).chain(s.live.iter().map(|(repo, _)| repo.clone()))
        })
        .collect();
    let mut changes = Vec::new();
    let mut bases = HashMap::new();
    for repo in &touched {
        let Some(clone) = repos.clones.get(repo).cloned() else {
            continue;
        };
        let base = repos.default_branch(&clone).await;
        if repos.default_has_changes(&clone, &base).await {
            changes.push(repo.clone());
        }
        bases.insert(repo.clone(), base);
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
    Ok(ClaudeCodeImportResult {
        dry_run: params.dry_run,
        since_days,
        claude_dir: claude.to_string_lossy().into_owned(),
        sessions: plans,
        workspaces: plan.workspaces,
        histories: plan.histories,
        releases,
        warnings,
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
    git: Git,
    checkouts: HashMap<PathBuf, Option<Checkout>>,
    /// Each repo's clone, by Chauffeur's name.
    clones: HashMap<String, PathBuf>,
    live: HashMap<(PathBuf, String), bool>,
    /// Each clone's merged PRs' heads by branch, read once; none when GitHub could not say.
    merged: HashMap<PathBuf, Option<HashMap<String, Vec<String>>>>,
    refs: HashMap<PathBuf, Refs>,
    /// Clones without a GitHub remote, warned about once.
    unnamed: BTreeSet<PathBuf>,
    warnings: Vec<String>,
}

impl<'a> Repos<'a> {
    fn new(d: &'a Daemon) -> Repos<'a> {
        Repos {
            d,
            git: Git::new(d.shell_env.clone()),
            checkouts: HashMap::new(),
            clones: HashMap::new(),
            live: HashMap::new(),
            merged: HashMap::new(),
            refs: HashMap::new(),
            unnamed: BTreeSet::new(),
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
        if top.starts_with(&self.d.paths.workspaces_dir) {
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
                self.clones
                    .entry(name.clone())
                    .or_insert_with(|| clone.clone());
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
    async fn branches(
        &mut self,
        session: &sessions::Session,
        warnings: &mut Vec<String>,
    ) -> Vec<ClaudeCodeBranch> {
        let mut found: Vec<(String, PathBuf, String)> = Vec::new();
        let mut gone = BTreeSet::new();
        for write in &session.writes {
            // The checkout the session stood in (from its nearest folder still there: a worktree
            // removed since resolves to its clone, the same repo), on the branch recorded then.
            let stood = self.checkout(&write.cwd).await;
            let dir = write.path.parent().unwrap_or(&write.path);
            let (checkout, branch) = if dir.is_dir() {
                let Some(checkout) = self.checkout(&write.path).await else {
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
                // A folder missing now: one of another branch of the checkout it stood in, or
                // gone (a worktree removed, a scratch folder cleaned up).
                match stood.filter(|s| write.path.starts_with(&s.top)) {
                    Some(stood) => (stood, write.branch.clone()),
                    None => {
                        gone.insert(dir.to_path_buf());
                        continue;
                    }
                }
            };
            if let (Some(repo), Some(branch)) = (checkout.repo.clone(), branch) {
                found.push((repo, checkout.clone, branch));
            }
        }
        for created in &session.created {
            if let Some(checkout) = self.checkout(&created.dir).await
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
            warnings.push(format!(
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

    /// A branch that exists and is not merged into the default branch (its tip in it, or a merged
    /// PR's head at or after it), or whose checkout holds uncommitted changes. The default branch
    /// is none: its changes get a workspace of their own.
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
        // Uncommitted changes in a checkout make it live, merged or not.
        for holder in holders {
            if self.has_changes(&holder).await {
                return true;
            }
        }
        if merged {
            return false;
        }
        // Squash-merged: a merged PR from the branch whose head is its tip or after it.
        let Some(heads) = self.merged_heads(clone, branch).await else {
            self.warnings.push(format!(
                "Could not ask whether {branch} is merged: it is counted as live."
            ));
            return true;
        };
        for head in heads {
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

    /// The heads of the merged PRs from `branch`: on GitHub from the repo's recent merged PRs,
    /// read once (`MERGED_PRS`); on GitLab, asked for that branch.
    async fn merged_heads(&mut self, clone: &Path, branch: &str) -> Option<Vec<String>> {
        let forge = self.d.settings.get().forge;
        if forge == chauffeur_protocol::client::Forge::Gitlab {
            let prs = crate::prs::pr_list(
                forge,
                &self.d.shell_env,
                clone,
                branch,
                "merged",
                "headRefOid",
            )
            .await?;
            return Some(
                prs.iter()
                    .filter_map(|pr| pr["headRefOid"].as_str().map(String::from))
                    .collect(),
            );
        }
        if !self.merged.contains_key(clone) {
            let heads = crate::prs::merged_heads(&self.d.shell_env, clone, MERGED_PRS).await;
            self.merged.insert(clone.to_path_buf(), heads);
        }
        let heads = self.merged.get(clone)?.as_ref()?;
        Some(heads.get(branch).cloned().unwrap_or_default())
    }

    async fn default_branch(&mut self, clone: &Path) -> String {
        self.refs(clone).await.base.clone()
    }

    /// A clone's branches, read once: its default branch, each branch's tip, those merged into
    /// the default branch (their tip in `origin`'s, else the local one), its worktrees.
    async fn refs(&mut self, clone: &Path) -> &Refs {
        if !self.refs.contains_key(clone) {
            let refs = self.read_refs(clone).await;
            self.refs.insert(clone.to_path_buf(), refs);
        }
        &self.refs[clone]
    }

    async fn read_refs(&self, clone: &Path) -> Refs {
        let base = self
            .git
            .remote_default_branch(clone)
            .await
            .unwrap_or_else(|| "main".into());
        let remote = format!("refs/remotes/origin/{base}");
        let target = if self
            .git
            .ok(clone, &["rev-parse", "-q", "--verify", &remote])
            .await
        {
            remote
        } else {
            format!("refs/heads/{base}")
        };
        let list = |args: Vec<&str>| {
            let git = &self.git;
            let args: Vec<String> = args.into_iter().map(String::from).collect();
            async move {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                git.run(clone, &args).await.unwrap_or_default()
            }
        };
        let tips = list(vec![
            "for-each-ref",
            "--format=%(refname:short) %(objectname)",
            "refs/heads",
        ])
        .await
        .lines()
        .filter_map(|l| {
            l.rsplit_once(' ')
                .map(|(name, oid)| (name.to_string(), oid.to_string()))
        })
        .collect();
        let merged = list(vec![
            "for-each-ref",
            "--format=%(refname:short)",
            "--merged",
            &target,
            "refs/heads",
        ])
        .await
        .lines()
        .map(String::from)
        .collect();
        let worktrees = self.git.worktree_list(clone).await.unwrap_or_default();
        Refs {
            base,
            tips,
            merged,
            worktrees,
        }
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

    /// Where a live branch is checked out: the clone or one of the user's worktrees, not
    /// Chauffeur's.
    async fn holders(&mut self, repo: &str, branch: &str) -> Vec<ClaudeCodeRelease> {
        let Some(clone) = self.clones.get(repo).cloned() else {
            return Vec::new();
        };
        let workspaces = self.d.paths.workspaces_dir.clone();
        self.refs(&clone)
            .await
            .worktrees
            .iter()
            .filter(|e| {
                e.branch.as_deref() == Some(branch)
                    && !e.prunable
                    && !e.path.starts_with(&workspaces)
            })
            .map(|e| ClaudeCodeRelease {
                repo: repo.to_string(),
                branch: branch.to_string(),
                folder: e.path.to_string_lossy().into_owned(),
            })
            .collect()
    }
}
