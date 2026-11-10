//! Workspaces: creation (one worktree per repo, ports, files to copy, generated config, setup
//! scripts), agent worktree groups, child workspaces, archive (everything stopped, snapshotted and
//! removed) and restore.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use chauffeur_protocol::client::{
    AddWorktreeParams, AddWorktreeResult, CreateChatParams, CreateWorkspaceParams,
    CreateWorkspaceResult, ProcessInfo, ProcessesResult, PromoteLeftovers, RemoveWorktreeResult,
    RepoLeftovers, SetTicketsResult, WaitResult, WorkspaceDesign,
};
use chauffeur_protocol::events::{ProcessKind, RegistryEvent, WorkspaceEvent};
use chauffeur_protocol::model::{
    Activity, ChatInfo, ChatKind, ChatStatus, TicketRef, WorkspaceInfo, WorkspaceState,
    WorktreeInfo, WorktreeOrigin,
};
use globset::{Glob, GlobSetBuilder};

use crate::git::{Git, validate_branch_name};
use crate::paths::{AGENT_WORKTREES_DIR, NOTES_DIR};
use crate::procs::{self, ScriptSpec};
use crate::repos::{ResolvedRepo, parse_repo_ref, repo_folder};
use crate::settings::RepoSettings;
use crate::slug;
use crate::store::{TitleSource, WorkspaceRow, WorktreeRow, now, ticket_title};
use crate::{Daemon, Error, Result, genconfig};

pub struct WorkspaceManager {
    daemon: Weak<Daemon>,
    /// Clones fetched in the last moments: a creation fetches each repo once (see prefetch).
    fetched: std::sync::Mutex<HashMap<PathBuf, Instant>>,
    /// One change of a workspace's tickets at a time, by workspace.
    ticket_locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Tests: Docker's timeouts, instead of 10 s and the archive scripts' timeout.
    compose_timeouts: std::sync::Mutex<Option<procs::ComposeTimeouts>>,
}

/// How long a new workspace's worktrees wait for its name before keeping the generated one; the
/// repos are fetched meanwhile.
const NAMING_WAIT: Duration = Duration::from_secs(15);
/// A fetch this recent is not repeated.
const FETCH_FRESH: Duration = Duration::from_secs(20);
/// How long a change of tickets waits for Linear.
const TICKETS_FETCH: Duration = Duration::from_secs(60);
/// How long an agent session's start waits for Linear when its workspace's tickets were never
/// read.
const BRIEFING_FETCH: Duration = Duration::from_secs(10);

/// What `update_tickets` did.
struct TicketsUpdate {
    warnings: Vec<String>,
    /// Tickets attached by this update, as Linear described them.
    attached: Vec<crate::linear::IssueDetails>,
    /// The briefing or the tickets' line of the generated config changed.
    changed: bool,
}

/// Tickets of a workspace whose content its agents do not have, and why.
pub struct UnreadTickets {
    pub tickets: Vec<String>,
    pub reason: String,
}

/// What `create` decided, handed to `finish_create` (inline or in the background).
struct Creation {
    shared: Option<crate::workspace_share::SharedWorkspaceCopy>,
    row: WorkspaceRow,
    params: CreateWorkspaceParams,
    repo_names: Vec<String>,
    tickets: Vec<TicketRef>,
    issues: Vec<crate::linear::IssueDetails>,
    parent: Option<WorkspaceRow>,
    /// The design chat a picked design goes into once the workspace's folder exists.
    design: Option<(ChatInfo, DesignSource)>,
}

/// Where a design picked at creation is copied from.
enum DesignSource {
    Claude(Box<crate::claude_artifact::Imported>),
    Share(Box<crate::share::SharedCopy>),
    /// One of the user's design chats: its folder.
    Folder(PathBuf),
}

impl WorkspaceManager {
    pub fn new(daemon: Weak<Daemon>) -> WorkspaceManager {
        WorkspaceManager {
            daemon,
            fetched: std::sync::Mutex::new(HashMap::new()),
            ticket_locks: Default::default(),
            compose_timeouts: Default::default(),
        }
    }

    /// Tests: shorter timeouts for Docker.
    pub fn set_compose_timeouts(&self, timeouts: procs::ComposeTimeouts) {
        *self.compose_timeouts.lock().unwrap() = Some(timeouts);
    }

    fn d(&self) -> Arc<Daemon> {
        self.daemon.upgrade().expect("daemon alive")
    }

    fn git(&self) -> Git {
        Git::new(self.d().shell_env.clone())
    }

    // --- reading ---

    pub fn row(&self, key: &str) -> Result<WorkspaceRow> {
        self.d()
            .store
            .find_workspace(key)?
            .ok_or_else(|| Error::not_found(format!("workspace {key}")))
    }

    pub fn info(&self, row: &WorkspaceRow) -> Result<WorkspaceInfo> {
        let d = self.d();
        let worktrees = d
            .store
            .worktrees_of(&row.id)?
            .into_iter()
            .map(worktree_info)
            .collect();
        let block = d.store.port_block(&row.id)?;
        let chats = d.store.chats_of(&row.id)?;
        let moved_to = crate::cloud_transfer_driver::moved_to(&d, &row.id);
        // Moved: its chats run on that machine, and show what they do there.
        let remote = moved_to
            .as_ref()
            .and_then(|_| d.relay.remote_state(&row.id));
        let section = match &row.section_id {
            Some(id) => d
                .store
                .list_sections()?
                .into_iter()
                .find(|s| &s.id == id)
                .map(|s| s.name),
            None => None,
        };
        Ok(WorkspaceInfo {
            id: row.id.clone(),
            slug: row.slug.clone(),
            title: row.title.clone(),
            path: row.path.clone(),
            branch: row.branch.clone(),
            parent_id: row.parent_id.clone(),
            state: row.state,
            activity: remote.as_ref().map_or_else(
                || activity(chats.iter().filter(|c| !c.closed).map(|c| c.status)),
                |r| r.activity,
            ),
            explore: row.explore,
            daily_review_date: d.store.review_day(&row.id)?,
            tickets: row.tickets.clone(),
            worktrees,
            port_base: block.as_ref().map(|b| b.base).unwrap_or(0),
            port_count: block.as_ref().map(|b| b.size).unwrap_or(0),
            created_at: row.created_at.clone(),
            archived_at: row.archived_at.clone(),
            last_error: row.last_error.clone(),
            unread: remote.as_ref().map_or_else(|| row.unread(), |r| r.unread),
            pinned: row.pinned,
            position: row.position,
            house_position: row.house_position,
            running_tasks: remote.as_ref().map_or_else(
                || chats.iter().map(|c| d.chats.running_tasks(&c.id)).sum(),
                |r| r.running_tasks,
            ),
            section_id: section.as_ref().and(row.section_id.clone()),
            section,
            idle_since: moved_to
                .as_ref()
                .and_then(|_| d.idle_reminders.idle_since(&row.id)),
            backup: crate::cloud_workspace_backup::shown(&d, &row.id, moved_to.is_some())
                .map(Box::new),
            moved_to,
        })
    }

    pub fn get(&self, key: &str) -> Result<WorkspaceInfo> {
        self.info(&self.row(key)?)
    }

    /// In the sidebar's order: pinned first, then by section, then the others, each in the user's
    /// order; archived workspaces last, unsectioned.
    pub fn list(&self, include_archived: bool, parent: Option<&str>) -> Result<Vec<WorkspaceInfo>> {
        let d = self.d();
        let parent_id = match parent {
            Some(key) => Some(self.row(key)?.id),
            None => None,
        };
        let sections: HashMap<String, usize> = d
            .store
            .list_sections()?
            .into_iter()
            .enumerate()
            .map(|(i, s)| (s.id, i))
            .collect();
        let mut rows = d.store.list_workspaces()?;
        rows.sort_by_key(|row| {
            let archived = row.state == WorkspaceState::Archived;
            let section = match (&row.section_id, row.pinned || archived) {
                (_, true) => 0,
                (Some(id), false) => sections.get(id).map_or(usize::MAX, |i| i + 1),
                (None, false) => usize::MAX,
            };
            (archived, !row.pinned, section)
        });
        rows.into_iter()
            .filter(|row| include_archived || row.state != WorkspaceState::Archived)
            .filter(|row| parent_id.is_none() || row.parent_id == parent_id)
            .map(|row| self.info(&row))
            .collect()
    }

    /// Names a workspace still waiting for a title after `prompt`: its title, and its folder and
    /// branch while they exist nowhere yet. None when it waits no more or no title came.
    async fn name_from(&self, row: &WorkspaceRow, prompt: &str) -> Option<WorkspaceRow> {
        let d = self.d();
        if !d.store.take_auto_title(&row.id).ok()? {
            return None;
        }
        // The worktrees wait for the name a few seconds at most; a later title is only shown.
        let mut asking = {
            let (d, prompt) = (d.clone(), prompt.to_string());
            tokio::spawn(async move { d.chats.title_for(&prompt).await })
        };
        let title = match tokio::time::timeout(NAMING_WAIT, &mut asking).await {
            Ok(answer) => answer.ok().flatten()?,
            Err(_) => {
                let (d, id) = (d.clone(), row.id.clone());
                tokio::spawn(async move {
                    if let Ok(Some(title)) = asking.await
                        && d.store.set_message_title(&id, &title).unwrap_or(false)
                    {
                        d.workspaces.publish(&id);
                    }
                });
                return None;
            }
        };
        let base = slug::slugify(&title, 40);
        let mut renamed = row.clone();
        renamed.title = Some(title);
        renamed.auto_title = false;
        if !base.is_empty() {
            let workspaces_dir = d.paths.workspaces_dir.clone();
            let slug = slug::unique(&base, |candidate| {
                d.store.slug_exists(candidate).unwrap_or(true)
                    || workspaces_dir.join(candidate).exists()
            });
            // A generated branch follows the name; a chosen one stays.
            if let Some(prefix) = row.branch.strip_suffix(&format!("/{}", row.slug)) {
                renamed.branch = format!("{prefix}/{slug}");
            }
            renamed.path = d.paths.workspace_dir(&slug).to_string_lossy().into_owned();
            renamed.slug = slug;
        }
        // The first message's images are already in the folder: it moves with the name.
        if renamed.path != row.path
            && Path::new(&row.path).exists()
            && let Err(error) = std::fs::rename(&row.path, &renamed.path)
        {
            tracing::warn!(%error, workspace = row.slug, "could not move the workspace folder");
            return None;
        }
        if let Err(error) = d.store.rename_workspace(&renamed) {
            tracing::warn!(%error, workspace = row.slug, "could not name the workspace");
            return None;
        }
        self.publish(&row.id);
        // The title stays its tickets' when they gave it one.
        Some(self.row(&row.id).unwrap_or(renamed))
    }

    /// Turns an exploration into a workspace: a branch per repo from the commit it reads, the
    /// files to copy, a port block and the setup scripts; its chats can write from their next
    /// message, their conversation kept. Refused while one of its agents works, and while a
    /// checkout holds files its commit does not (what the exploration's commands wrote, which
    /// must not slip into the workspace's commits), unless `discard`: the user's say-so to put
    /// each checkout back at its commit first. Its temporary folder goes once its sessions stop.
    pub async fn promote(&self, key: &str, discard: bool) -> Result<WorkspaceInfo> {
        let d = self.d();
        let row = self.row(key)?;
        let _admission = d.execution.admit(&row.id)?;
        if !row.explore {
            return Err(Error::BadRequest(format!(
                "{} is already a workspace",
                row.slug
            )));
        }
        if row.state != WorkspaceState::Ready {
            return Err(Error::BadRequest(format!(
                "{} is not ready; wait until it is",
                row.slug
            )));
        }
        let chats = d.store.chats_of(&row.id)?;
        if chats.iter().any(|chat| {
            matches!(
                chat.status,
                ChatStatus::Working | ChatStatus::NeedsInput | ChatStatus::Starting
            )
        }) {
            return Err(Error::BadRequest(format!(
                "an agent of {} is working; wait for the end of its turn, or stop it",
                row.slug
            )));
        }
        let git = self.git();
        let worktrees: Vec<WorktreeRow> = d
            .store
            .worktrees_of(&row.id)?
            .into_iter()
            .filter(|w| !w.removed && w.origin == WorktreeOrigin::Workspace)
            .collect();
        let leftovers = self.leftovers(&worktrees).await?;
        if !leftovers.is_empty() {
            if !discard {
                return Err(leftovers_refusal(&row.slug, leftovers));
            }
            for left in &leftovers {
                let Some(w) = worktrees.iter().find(|w| w.repo == left.repo) else {
                    continue;
                };
                self.progress(
                    &row.id,
                    format!(
                        "discarding what the exploration left in {}: {}",
                        w.repo,
                        files_line(&left.files)
                    ),
                );
                git.discard_leftovers(Path::new(&w.path))
                    .await
                    .map_err(|e| Error::Internal(e.context(format!("discarding in {}", w.repo))))?;
            }
            // Written again meanwhile (a command still running): nothing is carried over unseen.
            let still = self.leftovers(&worktrees).await?;
            if !still.is_empty() {
                return Err(leftovers_refusal(&row.slug, still));
            }
        }
        // A branch no repo has yet.
        let base = format!("{}/{}", self.branch_prefix().await, row.slug);
        let mut branch = base.clone();
        for n in 2.. {
            let mut taken = false;
            for w in &worktrees {
                taken |= git.branch_exists(Path::new(&w.clone_path), &branch).await;
            }
            if !taken {
                break;
            }
            branch = format!("{base}-{n}");
        }
        let mut switched: Vec<&WorktreeRow> = Vec::new();
        for w in &worktrees {
            if let Err(error) = git
                .run(
                    Path::new(&w.path),
                    &["switch", "-c", &branch, "--end-of-options"],
                )
                .await
            {
                // Back to the exploration as it was.
                for done in switched {
                    let _ = git
                        .run(Path::new(&done.path), &["switch", "--detach"])
                        .await;
                    let _ = git
                        .delete_branch(Path::new(&done.clone_path), &branch)
                        .await;
                }
                return Err(Error::Internal(
                    error.context(format!("branch {branch} in {}", w.repo)),
                ));
            }
            switched.push(w);
        }
        for w in &worktrees {
            d.store.set_worktree_branch(&w.id, &branch)?;
            match self.repo_settings(&w.repo, Path::new(&w.clone_path), Path::new(&w.path)) {
                Ok(settings) => {
                    if let Err(error) = copy_files(
                        &git,
                        Path::new(&w.clone_path),
                        Path::new(&w.path),
                        &settings.files_to_copy,
                    )
                    .await
                    {
                        self.warn(&row.id, format!("files to copy of {}: {error:#}", w.repo));
                    }
                }
                Err(error) => self.warn(&row.id, format!("settings of {}: {error:#}", w.repo)),
            }
        }
        crate::ports::allocate(
            &d.store,
            &row.id,
            &row.id,
            d.settings.get().port_range_start,
            d.settings.get().port_block_size,
        )?;
        d.store.promote_workspace(&row.id, &branch)?;
        if let Err(error) = self.regenerate_config(&row.id) {
            self.warn(&row.id, format!("workspace config: {error:#}"));
        }
        self.progress(&row.id, format!("now a workspace on branch {branch}"));
        d.chats.workspace_promoted(&row.id, &branch).await;
        if let Err(error) = crate::explore_tmp::remove(&d.paths.explore_tmp_root, &row.id) {
            self.warn(
                &row.id,
                format!("the exploration's temporary folder: {error}"),
            );
        }
        self.start_setup_scripts(&row.id, None)?;
        self.publish(&row.id);
        self.info(&self.row(&row.id)?)
    }

    /// Keeps a workspace at the top of the sidebar, or lets it go back in its place.
    pub fn pin(&self, key: &str, pinned: bool) -> Result<WorkspaceInfo> {
        let row = self.row(key)?;
        self.d().store.set_workspace_pinned(&row.id, pinned)?;
        self.publish(&row.id);
        self.info(&self.row(&row.id)?)
    }

    /// Moves a workspace in the sidebar: above `before`, or last of its group, among the pinned
    /// workspaces or the others of `section` (id or name; none: the "Workspaces" list). A pinned
    /// workspace keeps its section. Only top-level workspaces move; children follow their parent.
    pub fn move_to(
        &self,
        key: &str,
        before: Option<&str>,
        pinned: bool,
        section: Option<&str>,
    ) -> Result<WorkspaceInfo> {
        let d = self.d();
        let row = self.row(key)?;
        let before = before.map(|b| self.row(b)).transpose()?.map(|b| b.id);
        let section = if pinned {
            row.section_id.clone()
        } else {
            section
                .map(|s| crate::sections::resolve(&d, s))
                .transpose()?
                .map(|s| s.id)
        };
        if before.as_deref() == Some(row.id.as_str()) {
            return self.info(&row);
        }
        let rows = d.store.list_workspaces()?;
        let group = sidebar_group(&rows, pinned, section.as_deref());
        if before
            .as_deref()
            .is_some_and(|b| !group.iter().any(|(id, _)| id == b))
        {
            return Err(Error::BadRequest(
                "a workspace moves above another one of the same group, shown in the sidebar"
                    .into(),
            ));
        }
        let renumbered = d.store.move_workspace(
            &row.id,
            &group,
            before.as_deref(),
            pinned,
            section.as_deref(),
        )?;
        // Its children follow it into the section.
        if row.section_id != section {
            d.store.set_workspace_section(&row.id, section.as_deref())?;
        }
        let moved_children = row.section_id != section;
        for other in rows.iter().filter(|r| r.id != row.id) {
            if renumbered || (moved_children && other.parent_id.as_deref() == Some(&row.id)) {
                self.publish(&other.id);
            }
        }
        self.publish(&row.id);
        self.info(&self.row(&row.id)?)
    }

    /// Moves a workspace's house in the Office before `before`'s, or last. Every live workspace
    /// is numbered (sub-workspaces too, which keep their order beside their house), so that the
    /// order no longer depends on when they were made.
    pub fn move_house(
        &self,
        key: &str,
        before: Option<&str>,
        by_house: fn(&WorkspaceRow, &WorkspaceRow) -> std::cmp::Ordering,
    ) -> Result<WorkspaceInfo> {
        let d = self.d();
        let row = self.row(key)?;
        let before = before.map(|b| self.row(b)).transpose()?.map(|b| b.id);
        if before.as_deref() == Some(row.id.as_str()) {
            return self.info(&row);
        }
        let mut rows = d.store.list_workspaces()?;
        rows.retain(|r| matches!(r.state, WorkspaceState::Creating | WorkspaceState::Ready));
        rows.sort_by(by_house);
        let order = crate::chats::moved_before(
            rows.iter().map(|r| r.id.clone()).collect(),
            &row.id,
            before.as_deref(),
        );
        d.store.order_houses(&order)?;
        for (i, id) in order.iter().enumerate() {
            let was = rows
                .iter()
                .find(|r| &r.id == id)
                .and_then(|r| r.house_position);
            if was != Some(i as u32) {
                self.publish(id);
            }
        }
        self.info(&self.row(&row.id)?)
    }

    /// Files a workspace in a section (id or name), or in none: on top of it when the workspace
    /// is shown among the others; a pinned one stays pinned, its section kept for when it is
    /// unpinned. Its children follow it.
    pub fn set_section(&self, key: &str, section: Option<&str>) -> Result<WorkspaceInfo> {
        let d = self.d();
        let row = self.row(key)?;
        let section = section
            .map(|s| crate::sections::resolve(&d, s))
            .transpose()?
            .map(|s| s.id);
        let rows = d.store.list_workspaces()?;
        let top_level = sidebar_group(&rows, false, row.section_id.as_deref())
            .iter()
            .any(|(id, _)| *id == row.id);
        if top_level {
            let group = sidebar_group(&rows, false, section.as_deref());
            let first = group.iter().map(|(id, _)| id).find(|id| **id != row.id);
            return self.move_to(
                &row.id,
                first.map(String::as_str),
                false,
                section.as_deref(),
            );
        }
        d.store.set_workspace_section(&row.id, section.as_deref())?;
        for child in rows
            .iter()
            .filter(|r| r.parent_id.as_deref() == Some(&row.id))
        {
            self.publish(&child.id);
        }
        self.publish(&row.id);
        self.info(&self.row(&row.id)?)
    }

    /// Shows a workspace as unread again, as when a chat of it finishes.
    pub fn mark_unread(&self, key: &str) -> Result<WorkspaceInfo> {
        let row = self.row(key)?;
        self.d().store.set_workspace_attention(&row.id)?;
        self.publish(&row.id);
        self.info(&self.row(&row.id)?)
    }

    pub fn rename(&self, key: &str, title: &str) -> Result<WorkspaceInfo> {
        let d = self.d();
        let row = self.row(key)?;
        let _admission = d.execution.admit(&row.id)?;
        let title: String = title.split_whitespace().collect::<Vec<_>>().join(" ");
        let title = (!title.is_empty() && title != row.slug)
            .then(|| title.chars().take(120).collect::<String>());
        d.store.set_workspace_title(&row.id, title.as_deref())?;
        self.publish(&row.id);
        self.info(&self.row(&row.id)?)
    }

    /// Publishes the workspace's current state to registry subscribers.
    pub fn publish(&self, id: &str) {
        let d = self.d();
        match d
            .store
            .find_workspace(id)
            .map(|row| row.map(|r| self.info(&r)))
        {
            Ok(Some(Ok(workspace))) => {
                d.events
                    .registry(RegistryEvent::WorkspaceUpserted { workspace });
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "could not publish workspace {id}"),
        }
    }

    fn set_state(&self, id: &str, state: WorkspaceState, error: Option<String>) -> Result<()> {
        let d = self.d();
        d.store.set_workspace_state(id, state, error.as_deref())?;
        d.events
            .workspace(id, WorkspaceEvent::StateChanged { state, error });
        self.publish(id);
        Ok(())
    }

    pub fn progress(&self, id: &str, message: impl Into<String>) {
        self.d().events.workspace(
            id,
            WorkspaceEvent::Progress {
                message: message.into(),
            },
        );
    }

    pub fn warn(&self, id: &str, message: impl Into<String>) {
        let message = message.into();
        tracing::warn!(workspace = id, "{message}");
        self.d()
            .events
            .workspace(id, WorkspaceEvent::Warning { message });
    }

    // --- creation ---

    pub async fn create(&self, params: CreateWorkspaceParams) -> Result<CreateWorkspaceResult> {
        self.create_for_review(params, None, None).await
    }

    /// `day` claims that day's daily review for the new workspace, filed in `section`.
    pub(crate) async fn create_for_review(
        &self,
        mut params: CreateWorkspaceParams,
        day: Option<&str>,
        section: Option<String>,
    ) -> Result<CreateWorkspaceResult> {
        let d = self.d();
        let mut shared = match params.workspace_share.as_deref() {
            Some(url) => Some(d.workspace_shares.read_copy(url).await?),
            None => None,
        };
        if params
            .name
            .as_deref()
            .is_none_or(|name| name.trim().is_empty())
            && let Some(copy) = &shared
        {
            params.name = Some(format!("{} (fork)", copy.snapshot.title));
        }
        // They reach the workspace's CLAUDE.md and AGENTS.md, and name its folder and branch.
        params.tickets = params
            .tickets
            .iter()
            .filter(|t| !t.trim().is_empty())
            .map(|t| crate::linear::ticket_identifier(t))
            .collect::<Result<_>>()?;
        // Before a milestone's issues and parents' sub-issues join them.
        let picked_tickets = params.tickets.len();
        if let Some(milestone) = params.milestone.clone().filter(|m| !m.trim().is_empty()) {
            if !d.settings.get().linear_enabled {
                return Err(Error::Unavailable(
                    "Linear is turned off in the settings".into(),
                ));
            }
            params
                .tickets
                .extend(d.linear.milestone_issues(&milestone).await?);
        }
        if params.explore && params.parent.is_some() {
            return Err(Error::BadRequest(
                "an exploration cannot be a child workspace".into(),
            ));
        }
        if params.explore
            && params
                .branch
                .as_deref()
                .is_some_and(|b| !b.trim().is_empty())
        {
            return Err(Error::BadRequest(
                "an exploration has no branch: each repo is checked out at its base branch".into(),
            ));
        }
        let (tickets, issues, ticket_warnings) = self.fetch_tickets(&params.tickets).await;
        if params.no_repos && !params.repos.is_empty() {
            return Err(Error::BadRequest(
                "choose repos or no_repos, not both".into(),
            ));
        }
        if params.no_repos && params.chat_kind != Some(ChatKind::Design) {
            return Err(Error::BadRequest(
                "only a design can be created without repos: pick at least one repo".into(),
            ));
        }
        let mut repo_names = if params.no_repos {
            Vec::new()
        } else if params.repos.is_empty() {
            // The default set, plus the repos the tickets name in their `## Repo` section.
            let known = d.repos.known_names_loaded(&d.shell_env).await?;
            let mut names = d.settings.get().default_repos.clone();
            for issue in &issues {
                names.extend(
                    issue
                        .issue
                        .repos
                        .iter()
                        .filter(|repo| known.is_empty() || known.contains(*repo))
                        .cloned(),
                );
            }
            names
        } else {
            params.repos.clone()
        };
        dedup(&mut repo_names);
        if repo_names.is_empty() && !params.no_repos {
            return Err(Error::BadRequest(
                "pick at least one repo (no default repos are configured)".into(),
            ));
        }
        check_repo_refs(&repo_names)?;

        let parent = match &params.parent {
            Some(key) => Some(self.check_parent(key)?),
            None => None,
        };
        // Read before anything exists: a link that cannot be read refuses the creation.
        let design = match &params.design {
            Some(design) => Some(self.read_design(design).await?),
            None => None,
        };

        let single_ticket = naming_ticket(picked_tickets, &tickets);
        let named = params.name.as_deref().is_some_and(|n| !n.trim().is_empty());
        // Neither a name nor a ticket: the name is generated, then the first message names the
        // workspace (its folder and branch too when it comes with the creation, see name_from).
        let unnamed = !named && single_ticket.is_none();
        // The name shown: the one given, else the first ticket picked's title (filled when Linear
        // reads it, if it could not now).
        let (title, title_source) = if named {
            (None, Some(TitleSource::User))
        } else {
            match ticket_title(&tickets).filter(|_| picked_tickets > 0) {
                Some(title) => (Some(title.to_string()), Some(TitleSource::Ticket)),
                None => (None, None),
            }
        };
        let base_slug = slug::workspace_slug(
            params.name.as_deref(),
            single_ticket.map(|t| (t.identifier.as_str(), t.title.as_deref())),
        );
        let store = d.store.clone();
        let workspaces_dir = d.paths.workspaces_dir.clone();
        let slug = slug::unique(&base_slug, |candidate| {
            store.slug_exists(candidate).unwrap_or(true) || workspaces_dir.join(candidate).exists()
        });
        let linear_branch = single_ticket
            .and_then(|t| issues.iter().find(|i| i.issue.identifier == t.identifier))
            .and_then(|i| i.issue.branch_name.clone());
        let branch = match params.branch.clone().filter(|b| !b.trim().is_empty()) {
            _ if params.explore => String::new(),
            Some(branch) => branch,
            None => match (&parent, single_ticket) {
                // Children follow implement-ticket's convention: `<ID>/<title>`.
                (Some(_), Some(ticket)) => match &ticket.title {
                    Some(title) => format!("{}/{}", ticket.identifier, slug::slugify(title, 50)),
                    None => ticket.identifier.clone(),
                },
                (None, Some(_)) if linear_branch.is_some() => linear_branch.clone().unwrap(),
                _ => format!("{}/{}", self.branch_prefix().await, slug),
            },
        };
        // Chosen or derived from a ticket id given on the command line: checked before any git
        // command sees it.
        if !params.explore {
            validate_branch_name(&branch).map_err(|error| match error {
                Error::BadRequest(message) => {
                    Error::BadRequestWith(message, serde_json::json!({ "field": "branch" }))
                }
                other => other,
            })?;
            self.check_branch_free(&branch, &repo_names).await?;
        }
        let row = WorkspaceRow {
            id: uuid::Uuid::now_v7().to_string(),
            slug: slug.clone(),
            path: d.paths.workspace_dir(&slug).to_string_lossy().into_owned(),
            branch,
            parent_id: parent.as_ref().map(|p| p.id.clone()),
            state: WorkspaceState::Creating,
            tickets: tickets.clone(),
            created_at: now(),
            archived_at: None,
            last_error: None,
            attention_at: None,
            seen_at: None,
            explore: params.explore,
            auto_title: unnamed,
            pinned: false,
            position: 0.0,
            title,
            title_source,
            // A child goes into its parent's section, a daily review into the reviews' one; any
            // other workspace on top of the others.
            section_id: parent
                .as_ref()
                .and_then(|p| p.section_id.clone())
                .or(section),
            house_position: None,
        };
        d.store.insert_review_workspace(&row, day)?;
        self.publish(&row.id);
        for warning in ticket_warnings {
            self.warn(&row.id, warning);
        }
        // The design's chat comes first, so that the first prompt names its folder; its files
        // follow once the workspace's folder exists.
        // A new design from an imported one (Design in the creation dialog): that design's chat
        // is the first chat, with the first prompt, the model and the images; no second one.
        let design_first = design.is_some() && params.chat_kind == Some(ChatKind::Design);
        let mut first_made_chat = None;
        let design = match design {
            Some((title, source)) => {
                let mut chat_params = CreateChatParams {
                    workspace: row.id.clone(),
                    kind: Some(ChatKind::Design),
                    title: Some(title.clone()),
                    ..CreateChatParams::default()
                };
                if design_first {
                    chat_params.agent = params.agent;
                    chat_params.model = params.model.clone();
                    chat_params.effort = params.effort.clone();
                    chat_params.fast = params.fast;
                }
                let created = d.chats.create(chat_params).await;
                match created {
                    Ok(chat) => {
                        let prompt = with_design(
                            params.prompt.as_deref(),
                            &title,
                            &design_folder(&row.path, &chat.id),
                        );
                        if design_first {
                            let has_prompt = params
                                .prompt
                                .as_deref()
                                .is_some_and(|p| !p.trim().is_empty());
                            params.prompt = None;
                            params.chat_kind = None;
                            let images = params.images.take().unwrap_or_default();
                            let starts_agent = !matches!(&source, DesignSource::Claude(_))
                                || has_prompt
                                || !images.is_empty();
                            if starts_agent
                                && let Err(error) = d.chats.hold_first(&chat, prompt, &images)
                            {
                                return Err(self.abandon(&row, error).await);
                            }
                            first_made_chat = Some(chat.clone());
                        } else {
                            params.prompt = Some(prompt);
                        }
                        Some((chat, source))
                    }
                    Err(error) => return Err(self.abandon(&row, error).await),
                }
            }
            None => None,
        };
        // A share of one chat imported with a first message: the message continues that chat
        // instead of opening a second one beside it.
        let has_first_message = params
            .prompt
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty())
            || params.images.as_ref().is_some_and(|i| !i.is_empty());
        if has_first_message
            && params.chat_kind != Some(ChatKind::Design)
            && let Some((source, history)) = shared.as_mut().and_then(|copy| copy.take_single())
        {
            let chat_params = CreateChatParams {
                workspace: row.id.clone(),
                agent: params.agent.or(Some(source.agent)),
                model: params.model.clone(),
                effort: params.effort.clone(),
                fast: params.fast,
                permission_mode: params.permission_mode,
                title: source.title,
                ..Default::default()
            };
            let prompt = params.prompt.take().unwrap_or_default();
            let images = params.images.take().unwrap_or_default();
            let continued = match d.workspace_shares.import_chat(chat_params, &history).await {
                Ok(chat) => d.chats.hold_first(&chat, prompt, &images).map(|()| chat),
                Err(error) => Err(error),
            };
            match continued {
                Ok(chat) => first_made_chat = Some(chat),
                Err(error) => return Err(self.abandon(&row, error).await),
            }
        }

        let job = Creation {
            shared,
            row: row.clone(),
            params,
            repo_names,
            tickets,
            issues,
            parent,
            design,
        };
        if job.params.background {
            // The first message shows in its chat at once; the agent gets it once the workspace
            // is ready (see ChatManager::workspace_ready).
            let chat = match self.first_chat(&job) {
                Some(params) => match d.chats.create(params).await {
                    Ok(chat) => Some(chat),
                    Err(error) => return Err(self.abandon(&row, error).await),
                },
                None => first_made_chat,
            };
            // Read before the creation goes on: one that fails at once takes the workspace away,
            // and the caller learns it from the events, not from a missing workspace here.
            let workspace = self.get(&row.id)?;
            let daemon = d.clone();
            tokio::spawn(async move {
                if let Err(error) = daemon.workspaces.finish_create(job).await {
                    tracing::warn!(%error, "background creation failed");
                }
            });
            return Ok(CreateWorkspaceResult { workspace, chat });
        }
        let chat = self.finish_create(job).await?.or(first_made_chat);
        Ok(CreateWorkspaceResult {
            workspace: self.get(&row.id)?,
            chat,
        })
    }

    /// Worktrees, config, setup, Linear and the first chat of a workspace just inserted.
    async fn finish_create(&self, job: Creation) -> Result<Option<ChatInfo>> {
        let d = self.d();
        let Creation {
            shared,
            row,
            params,
            repo_names,
            tickets,
            issues,
            parent,
            design,
        } = job;
        // Named after its first message before anything of it exists on disk, the repos being
        // fetched meanwhile.
        let row = match params.prompt.as_deref().filter(|p| !p.trim().is_empty()) {
            Some(prompt) => {
                let (named, ()) = tokio::join!(
                    self.name_from(&row, prompt),
                    self.prefetch(&row, &repo_names)
                );
                named.unwrap_or(row)
            }
            None => row,
        };
        if let Err(error) = self.populate(&row, &repo_names).await {
            self.rollback(&row).await;
            // The full chain: "worktree of alpha: git worktree add failed …", not just the context.
            let message = error.to_rpc().message;
            // Whoever follows the creation learns why it failed; then nothing of it is kept, so
            // the same name can be used again.
            d.events.workspace(
                &row.id,
                WorkspaceEvent::StateChanged {
                    state: WorkspaceState::Archived,
                    error: Some(message.clone()),
                },
            );
            if let Err(error) = d.store.delete_workspace(&row.id) {
                tracing::error!(%error, workspace = row.id, "could not forget a failed creation");
            }
            // Once its chats are gone: a chat opened later cannot hold a first message any more.
            d.chats.forget_creation(&row.id);
            d.events.registry(RegistryEvent::WorkspaceRemoved {
                workspace_id: row.id.clone(),
                slug: row.slug.clone(),
                error: Some(message),
            });
            return Err(error);
        }
        if let Some(shared) = shared
            && let Err(error) = d.workspace_shares.copy_into(&row, shared).await
        {
            return Err(self.abandon(&row, error).await);
        }
        if let Some(parent) = &parent {
            self.attach_tickets(&parent.id, &tickets)?;
        }
        // Before any agent starts: the first one reads it at once. A copy that fails leaves the
        // design chat empty and says so, the workspace goes on.
        if let Some((chat, source)) = design
            && let Err(error) = self.copy_design(&row, &chat, &source).await
        {
            if matches!(source, DesignSource::Claude(_)) {
                return Err(self.abandon(&row, error).await);
            }
            self.warn(&row.id, format!("could not copy the design: {error}"));
        }
        self.write_ticket_briefing(&row, &issues);
        self.set_state(&row.id, WorkspaceState::Ready, None)?;
        // An exploration runs nothing and starts no ticket.
        if !row.explore {
            self.start_setup_scripts(&row.id, None)?;
        }
        // The chats opened meanwhile (the first one, in the background) start their agents.
        d.chats.workspace_ready(&row.id).await;
        if d.settings.get().linear_start_tickets && !row.explore {
            for details in issues.iter().filter(|i| {
                params
                    .tickets
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(&i.issue.identifier))
            }) {
                match d.linear.start(details).await {
                    Ok(true) => self.progress(
                        &row.id,
                        format!("{} moved to In Progress", details.issue.identifier),
                    ),
                    Ok(false) => {}
                    Err(error) => self.warn(
                        &row.id,
                        format!("could not start {}: {error}", details.issue.identifier),
                    ),
                }
            }
        }

        // A creation waited on starts its first chat now.
        if params.background {
            return Ok(None);
        }
        let chat = match first_chat_params(&row.id, &params) {
            Some(chat) => Some(d.chats.create(chat).await?),
            None => None,
        };
        Ok(chat)
    }

    fn first_chat(&self, job: &Creation) -> Option<CreateChatParams> {
        first_chat_params(&job.row.id, &job.params)
    }

    /// A creation given up before its folder was made: nothing of it is kept.
    pub(crate) async fn abandon(&self, row: &WorkspaceRow, error: Error) -> Error {
        let d = self.d();
        self.rollback(row).await;
        let _ = d.store.delete_workspace(&row.id);
        d.chats.forget_creation(&row.id);
        d.events.registry(RegistryEvent::WorkspaceRemoved {
            workspace_id: row.id.clone(),
            slug: row.slug.clone(),
            error: Some(error.to_string()),
        });
        error
    }

    /// At daemon start: a creation or a restore the previous daemon did not finish stays
    /// `creating` for good (its task went with it, and a `creating` workspace cannot be archived).
    /// A creation is given up like a failed one, so its name can be used again; a restore leaves
    /// the workspace archived for another try.
    pub fn recover_after_restart(&self) {
        let d = self.d();
        let stranded: Vec<WorkspaceRow> = match d.store.list_workspaces() {
            Ok(rows) => rows
                .into_iter()
                .filter(|row| row.state == WorkspaceState::Creating)
                .collect(),
            Err(error) => {
                tracing::error!(%error, "could not look for unfinished creations");
                return;
            }
        };
        if stranded.is_empty() {
            return;
        }
        tokio::spawn(async move {
            for row in stranded {
                tracing::warn!(
                    workspace = row.slug,
                    "giving up a creation the last daemon left"
                );
                if row.archived_at.is_some() {
                    let _ = d.workspaces.set_state(
                        &row.id,
                        WorkspaceState::CleanupFailed,
                        Some("restore interrupted when Chauffeur stopped: restore it again".into()),
                    );
                } else {
                    d.workspaces
                        .abandon(
                            &row,
                            Error::Unavailable(format!(
                                "Chauffeur stopped before {} was ready: create it again",
                                row.slug
                            )),
                        )
                        .await;
                }
            }
        });
    }

    /// The design picked for a new workspace, read: its title and where its files come from.
    async fn read_design(&self, design: &WorkspaceDesign) -> Result<(String, DesignSource)> {
        let d = self.d();
        match design {
            WorkspaceDesign::Claude { url } => {
                let imported = crate::claude_artifact::read(&d, url).await?;
                Ok((
                    imported.preview.title.clone(),
                    DesignSource::Claude(Box::new(imported)),
                ))
            }
            WorkspaceDesign::Share { url } => {
                let share = d.shares.read(url).await?;
                Ok((
                    share.snapshot.title.clone(),
                    DesignSource::Share(Box::new(share)),
                ))
            }
            WorkspaceDesign::Chat { chat } => {
                let chat = d.chats.resolve(chat)?;
                if chat.kind != ChatKind::Design {
                    return Err(Error::BadRequest(format!(
                        "{} is not a design chat",
                        chat.title.as_deref().unwrap_or(&chat.id)
                    )));
                }
                let workspace = self.row(&chat.workspace_id)?;
                let dir =
                    crate::design::design_dir(&workspace.path, &chat.id).map_err(|error| {
                        Error::Internal(anyhow::anyhow!("the design folder: {error}"))
                    })?;
                let title = chat.title.unwrap_or_else(|| "Design".into());
                Ok((title, DesignSource::Folder(PathBuf::from(dir))))
            }
        }
    }

    async fn copy_design(
        &self,
        row: &WorkspaceRow,
        chat: &ChatInfo,
        source: &DesignSource,
    ) -> Result<()> {
        let dir = crate::design::design_dir(&row.path, &chat.id)
            .map_err(|error| Error::Internal(anyhow::anyhow!("the design folder: {error}")))?;
        let dir = Path::new(&dir);
        match source {
            DesignSource::Claude(imported) => imported.write(dir),
            DesignSource::Share(share) => self.d().shares.copy(share, dir).await,
            DesignSource::Folder(from) => crate::design::copy(from, dir),
        }
    }

    /// Reads the tickets from Linear: titles and urls for the refs, sub-issues of parent tickets
    /// attached too. Without Linear (no key, offline) the identifiers are kept as given.
    async fn fetch_tickets(
        &self,
        identifiers: &[String],
    ) -> (
        Vec<TicketRef>,
        Vec<crate::linear::IssueDetails>,
        Vec<String>,
    ) {
        let d = self.d();
        let mut refs: Vec<TicketRef> = Vec::new();
        let mut details = Vec::new();
        let mut warnings = Vec::new();
        if identifiers.is_empty() {
            return (refs, details, warnings);
        }
        let configured = d.settings.get().linear_enabled && d.linear.is_configured();
        if d.settings.get().linear_enabled && !configured {
            warnings.push(
                "Linear is not configured (`chauffeur linear login`): tickets are attached by identifier only"
                    .to_string(),
            );
        }
        for identifier in identifiers {
            let identifier = identifier.trim().to_uppercase();
            if identifier.is_empty() || refs.iter().any(|t| t.identifier == identifier) {
                continue;
            }
            let fetched = if configured {
                match d.linear.issue(&identifier).await {
                    Ok(issue) => Some(issue),
                    Err(error) => {
                        warnings.push(format!("{identifier}: {error}"));
                        None
                    }
                }
            } else {
                None
            };
            match fetched {
                Some(issue) => {
                    refs.push(TicketRef {
                        identifier: issue.issue.identifier.clone(),
                        title: Some(issue.issue.title.clone()),
                        url: Some(issue.issue.url.clone()),
                    });
                    for child in &issue.issue.children {
                        if !refs.iter().any(|t| t.identifier == child.identifier) {
                            refs.push(child.clone());
                        }
                    }
                    details.push(issue);
                }
                None => refs.push(TicketRef {
                    identifier,
                    title: None,
                    url: None,
                }),
            }
        }
        (refs, details, warnings)
    }

    /// Writes the tickets' briefing to the notes folder, where agents read it.
    fn write_ticket_briefing(&self, row: &WorkspaceRow, issues: &[crate::linear::IssueDetails]) {
        if issues.is_empty() {
            return;
        }
        let path = PathBuf::from(&row.path).join(NOTES_DIR).join("tickets.md");
        if let Err(error) = std::fs::write(&path, crate::linear::briefing(issues)) {
            self.warn(
                &row.id,
                format!("could not write {}: {error}", path.display()),
            );
        }
    }

    fn check_parent(&self, key: &str) -> Result<WorkspaceRow> {
        let d = self.d();
        let parent = self.row(key)?;
        read_only(&parent, "it starts no child workspace")?;
        if parent.state != WorkspaceState::Ready {
            return Err(Error::Conflict(format!(
                "parent workspace {} is {}",
                parent.slug,
                parent.state.as_str()
            )));
        }
        if parent.parent_id.is_some() {
            return Err(Error::Conflict(format!(
                "{} is itself a child workspace: children cannot have children",
                parent.slug
            )));
        }
        let active_children = d
            .store
            .list_workspaces()?
            .iter()
            .filter(|w| {
                w.parent_id.as_deref() == Some(&parent.id) && w.state != WorkspaceState::Archived
            })
            .count();
        if active_children >= d.settings.get().max_children {
            return Err(Error::Conflict(format!(
                "{} already has {active_children} active child workspaces (max {}); archive one first",
                parent.slug,
                d.settings.get().max_children
            )));
        }
        Ok(parent)
    }

    pub(crate) fn attach_tickets(&self, workspace_id: &str, tickets: &[TicketRef]) -> Result<()> {
        if tickets.is_empty() {
            return Ok(());
        }
        let d = self.d();
        let row = self.row(workspace_id)?;
        let mut merged = row.tickets.clone();
        for ticket in tickets {
            if !merged.iter().any(|t| t.identifier == ticket.identifier) {
                merged.push(ticket.clone());
            }
        }
        d.store.set_workspace_tickets(workspace_id, &merged)?;
        d.store.take_ticket_title(workspace_id, &merged)?;
        self.publish(workspace_id);
        Ok(())
    }

    fn ticket_lock(&self, workspace_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.ticket_locks
            .lock()
            .unwrap()
            .entry(workspace_id.to_string())
            .or_default()
            .clone()
    }

    /// Replaces a workspace's tickets with `identifiers`, all read from Linear again (the same list
    /// refreshes them): their refs, the agents' briefing and the generated config. A ticket newly
    /// attached is moved to In Progress as at creation. The agents' idle sessions close, so that
    /// their next message resumes them with the new briefing.
    pub async fn set_tickets(&self, key: &str, identifiers: &[String]) -> Result<SetTicketsResult> {
        let d = self.d();
        let row = self.row(key)?;
        let _admission = d.execution.admit(&row.id)?;
        if row.state != WorkspaceState::Ready {
            return Err(Error::Conflict(format!(
                "{} is {}",
                row.slug,
                row.state.as_str()
            )));
        }
        let mut requested: Vec<String> = Vec::new();
        for identifier in identifiers {
            let identifier = crate::linear::ticket_identifier(identifier)?;
            if !requested.contains(&identifier) {
                requested.push(identifier);
            }
        }
        let lock = self.ticket_lock(&row.id);
        let guard = lock.lock().await;
        let row = self.row(&row.id)?;
        let mut update = self.update_tickets(&row, &requested, TICKETS_FETCH).await?;
        drop(guard);
        if update.changed {
            d.chats.briefing_changed(&row.id, true).await;
        }
        // As at creation: only what the user attaches, never in an exploration.
        if d.settings.get().linear_start_tickets && !row.explore {
            for details in &update.attached {
                match d.linear.start(details).await {
                    Ok(true) => self.progress(
                        &row.id,
                        format!("{} moved to In Progress", details.issue.identifier),
                    ),
                    Ok(false) => {}
                    Err(error) => {
                        let warning =
                            format!("could not start {}: {error}", details.issue.identifier);
                        self.warn(&row.id, warning.clone());
                        update.warnings.push(warning);
                    }
                }
            }
        }
        Ok(SetTicketsResult {
            workspace: self.get(&row.id)?,
            warnings: update.warnings,
        })
    }

    /// Before an agent session starts: reads from Linear, within a few seconds, the tickets of a
    /// workspace whose briefing is missing or that Linear never described (Linear failed or was
    /// off when they were attached). Never twice for a briefing already written. Returns the
    /// tickets still unread, and why.
    pub async fn ensure_briefing(&self, workspace_id: &str) -> Option<UnreadTickets> {
        let d = self.d();
        let lock = self.ticket_lock(workspace_id);
        let guard = lock.lock().await;
        let row = self.row(workspace_id).ok()?;
        let unread = unread_tickets(&row);
        if unread.is_empty() {
            return None;
        }
        let settings = d.settings.get();
        let reason = if !settings.linear_enabled {
            "Linear is turned off in the settings".to_string()
        } else if !d.linear.is_configured() {
            "no Linear API key: save one in Settings (⌘,), or run `chauffeur linear login`"
                .to_string()
        } else {
            let identifiers: Vec<String> =
                row.tickets.iter().map(|t| t.identifier.clone()).collect();
            let update = self
                .update_tickets(&row, &identifiers, BRIEFING_FETCH)
                .await;
            let row = self.row(workspace_id).ok()?;
            drop(guard);
            if update.as_ref().is_ok_and(|u| u.changed) {
                d.chats.briefing_changed(workspace_id, false).await;
            }
            let tickets = unread_tickets(&row);
            if tickets.is_empty() {
                return None;
            }
            let reason = match update {
                Ok(u) if !u.warnings.is_empty() => u.warnings.join("; "),
                Ok(_) => "Linear did not describe them".to_string(),
                Err(error) => error.to_string(),
            };
            return Some(UnreadTickets { tickets, reason });
        };
        Some(UnreadTickets {
            tickets: unread,
            reason,
        })
    }

    /// Reads `requested` from Linear and makes them the workspace's tickets: refs in the store,
    /// briefing in the notes folder (none when Linear described no ticket), generated config. A
    /// ticket Linear does not return keeps what was known of it. The caller holds the ticket lock.
    async fn update_tickets(
        &self,
        row: &WorkspaceRow,
        requested: &[String],
        limit: Duration,
    ) -> Result<TicketsUpdate> {
        let d = self.d();
        let old = &row.tickets;
        let was_attached = |id: &str| old.iter().any(|t| t.identifier == id);
        let mut warnings = Vec::new();
        if !requested.is_empty() && !d.settings.get().linear_enabled {
            warnings.push(
                "Linear is turned off in the settings: tickets are attached by identifier only"
                    .to_string(),
            );
        }
        let (mut refs, details, fetch_warnings) =
            match tokio::time::timeout(limit, self.fetch_tickets(requested)).await {
                Ok(fetched) => fetched,
                Err(_) => (
                    requested
                        .iter()
                        .map(|identifier| TicketRef {
                            identifier: identifier.clone(),
                            title: None,
                            url: None,
                        })
                        .collect(),
                    Vec::new(),
                    vec![format!(
                        "Linear did not answer within {} s",
                        limit.as_secs()
                    )],
                ),
            };
        warnings.extend(fetch_warnings);
        // The sub-issues of a ticket already attached stay only while asked for: removing one
        // sticks.
        let dropped: HashSet<String> = details
            .iter()
            .filter(|d| was_attached(&d.issue.identifier))
            .flat_map(|d| d.issue.children.iter().map(|c| c.identifier.clone()))
            .filter(|id| !requested.contains(id))
            .collect();
        refs.retain(|t| !dropped.contains(&t.identifier));

        // A ticket Linear did not describe keeps its title and its part of the briefing.
        let path = PathBuf::from(&row.path).join(NOTES_DIR).join("tickets.md");
        let before = std::fs::read_to_string(&path).ok();
        let old_ids: Vec<String> = old.iter().map(|t| t.identifier.clone()).collect();
        let previous = before
            .as_deref()
            .map(|text| crate::linear::briefing_sections(text, &old_ids))
            .unwrap_or_default();
        let mut sections = Vec::new();
        for ticket in &mut refs {
            match details
                .iter()
                .find(|d| d.issue.identifier == ticket.identifier)
            {
                Some(fetched) => sections.push(crate::linear::briefing_section(fetched)),
                None => {
                    if ticket.title.is_none()
                        && let Some(known) = old.iter().find(|t| t.identifier == ticket.identifier)
                    {
                        *ticket = known.clone();
                    }
                    if let Some(section) = previous.get(&ticket.identifier) {
                        sections.push(section.clone());
                    }
                }
            }
        }
        let after = (!sections.is_empty()).then(|| crate::linear::briefing_of(&sections));
        if after != before {
            let written = match &after {
                Some(text) => std::fs::create_dir_all(PathBuf::from(&row.path).join(NOTES_DIR))
                    .and_then(|()| std::fs::write(&path, text)),
                None => std::fs::remove_file(&path),
            };
            if let Err(error) = written {
                warnings.push(format!("could not write {}: {error}", path.display()));
            }
        }
        let refs_changed = refs != *old;
        d.store.set_workspace_tickets(&row.id, &refs)?;
        d.store.take_ticket_title(&row.id, &refs)?;
        if refs_changed && let Err(error) = self.regenerate_config(&row.id) {
            warnings.push(format!("workspace config: {error:#}"));
        }
        for warning in &warnings {
            self.warn(&row.id, warning.clone());
        }
        self.publish(&row.id);
        let attached = details
            .into_iter()
            .filter(|d| {
                requested.contains(&d.issue.identifier) && !was_attached(&d.issue.identifier)
            })
            .collect();
        Ok(TicketsUpdate {
            warnings,
            attached,
            changed: refs_changed || after != before,
        })
    }

    async fn branch_prefix(&self) -> String {
        let prefix = self.d().settings.get().branch_prefix.clone();
        if !prefix.is_empty() {
            return prefix;
        }
        self.github_login()
            .await
            .unwrap_or_else(|| "chauffeur".into())
    }

    /// The user's GitHub login, asked of `gh` once.
    pub async fn github_login(&self) -> Option<String> {
        let d = self.d();
        if let Ok(Some(login)) = d.store.kv_get("gh_login") {
            return Some(login);
        }
        let login = tokio::process::Command::new("gh")
            .args(["api", "user", "--jq", ".login"])
            .env_clear()
            .envs(d.shell_env.vars())
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|l| !l.is_empty())?;
        let _ = d.store.kv_set("gh_login", &login);
        Some(login)
    }

    /// Creates the folder, the port block and one worktree per repo, then the generated config.
    async fn populate(&self, row: &WorkspaceRow, repo_names: &[String]) -> Result<()> {
        let d = self.d();
        let root = PathBuf::from(&row.path);
        std::fs::create_dir_all(root.join(NOTES_DIR))?;
        // An exploration runs no server: no ports.
        if !row.explore {
            crate::ports::allocate(
                &d.store,
                &row.id,
                &row.id,
                d.settings.get().port_range_start,
                d.settings.get().port_block_size,
            )?;
        }
        let targets: Vec<(&String, PathBuf)> = repo_names
            .iter()
            .map(|name| (name, root.join(repo_folder(name))))
            .collect();
        let results = futures_util::future::join_all(targets.iter().map(|(name, path)| {
            self.add_worktree(
                row,
                name,
                path,
                &row.branch,
                None,
                WorktreeOrigin::Workspace,
                None,
            )
        }))
        .await;
        for result in results {
            result?;
        }
        self.regenerate_config(&row.id)?;
        Ok(())
    }

    /// Fetches each repo's base branch, ahead of its worktree (while the workspace is named).
    async fn prefetch(&self, row: &WorkspaceRow, repo_names: &[String]) {
        let d = self.d();
        let git = self.git();
        futures_util::future::join_all(repo_names.iter().map(|name| {
            let (d, git) = (&d, &git);
            async move {
                let Ok(repo) = d.repos.resolve(name, git, &d.shell_env).await else {
                    return;
                };
                let lock = d.repos.lock_for(&repo.clone_path);
                let _guard = lock.lock().await;
                self.progress(&row.id, format!("fetching {}", repo.name));
                if git
                    .fetch(&repo.clone_path, &repo.default_branch)
                    .await
                    .is_ok()
                {
                    self.mark_fetched(&repo.clone_path);
                }
            }
        }))
        .await;
    }

    fn fetched_lately(&self, clone: &Path) -> bool {
        let fetched = self.fetched.lock().unwrap();
        fetched
            .get(clone)
            .is_some_and(|at| at.elapsed() < FETCH_FRESH)
    }

    fn mark_fetched(&self, clone: &Path) {
        self.fetched
            .lock()
            .unwrap()
            .insert(clone.to_path_buf(), Instant::now());
    }

    /// Creates one worktree, records it and copies the repo's files to copy into it.
    /// Refuses, before anything exists, a branch another worktree of a picked repo's clone has
    /// checked out (git would refuse it mid-creation), suggesting the first free `<branch>-N`.
    /// A branch that only exists, here or on origin, is taken as is: resuming a PR's branch.
    async fn check_branch_free(&self, branch: &str, repos: &[String]) -> Result<()> {
        let d = self.d();
        let git = self.git();
        let mut taken: HashSet<String> = HashSet::new();
        let mut holder = None;
        for repo in repos {
            let Some(clone) = d.repos.existing_clone(repo) else {
                continue;
            };
            let Ok(entries) = git.worktree_list(&clone).await else {
                continue;
            };
            for entry in entries.into_iter().filter(|e| !e.prunable) {
                if let Some(checked_out) = entry.branch {
                    if checked_out == branch && holder.is_none() {
                        holder = Some((repo.clone(), entry.path.clone()));
                    }
                    taken.insert(checked_out);
                }
            }
        }
        let Some((repo, path)) = holder else {
            return Ok(());
        };
        let suggestion = free_branch(branch, &taken);
        Err(Error::ConflictWith(
            format!(
                "Branch {branch} is already checked out in {repo}, at {}. Pick another name, e.g. {suggestion}.",
                path.display()
            ),
            serde_json::json!({ "field": "branch", "suggestion": suggestion }),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn add_worktree(
        &self,
        row: &WorkspaceRow,
        repo_name: &str,
        path: &Path,
        branch: &str,
        start: Option<String>,
        origin: WorktreeOrigin,
        group: Option<String>,
    ) -> Result<WorktreeRow> {
        let d = self.d();
        let git = self.git();
        // A repo the workspace has: the clone its worktree came from, whoever owns the repo.
        let had =
            d.store.worktrees_of(&row.id)?.into_iter().find(|w| {
                !w.removed && w.origin == WorktreeOrigin::Workspace && w.repo == repo_name
            });
        let repo: ResolvedRepo = match had {
            Some(w) => ResolvedRepo {
                full_name: w.repo.clone(),
                name: w.repo,
                clone_path: PathBuf::from(w.clone_path),
                default_branch: w.base_branch,
            },
            None => d.repos.resolve(repo_name, &git, &d.shell_env).await?,
        };
        let lock = d.repos.lock_for(&repo.clone_path);
        let _guard = lock.lock().await;
        if !self.fetched_lately(&repo.clone_path) {
            self.progress(&row.id, format!("fetching {}", repo.name));
            match git.fetch(&repo.clone_path, &repo.default_branch).await {
                Ok(_) => self.mark_fetched(&repo.clone_path),
                Err(error) => self.warn(
                    &row.id,
                    format!(
                        "could not fetch {}, using the local state: {error:#}",
                        repo.name
                    ),
                ),
            }
        }
        if row.explore {
            return self.add_explore_worktree(row, &repo, path).await;
        }
        let start = match start {
            Some(start) => start,
            // An existing remote branch (resuming a PR): start the local branch from it.
            None if !git.branch_exists(&repo.clone_path, branch).await
                && git.fetch(&repo.clone_path, branch).await.is_ok()
                && git
                    .rev_parse(&repo.clone_path, &format!("origin/{branch}"))
                    .await
                    .is_ok() =>
            {
                format!("origin/{branch}")
            }
            None => {
                let remote = format!("origin/{}", repo.default_branch);
                if git.rev_parse(&repo.clone_path, &remote).await.is_ok() {
                    remote
                } else {
                    repo.default_branch.clone()
                }
            }
        };
        self.progress(&row.id, format!("creating the {} worktree", repo.name));
        git.worktree_add(&repo.clone_path, path, branch, &start)
            .await
            .map_err(|e| Error::Internal(e.context(format!("worktree of {}", repo.name))))?;
        let base_commit = git.rev_parse(path, "HEAD").await.ok();
        let worktree = WorktreeRow {
            id: uuid::Uuid::now_v7().to_string(),
            workspace_id: row.id.clone(),
            repo: repo.name.clone(),
            clone_path: repo.clone_path.to_string_lossy().into_owned(),
            path: path.to_string_lossy().into_owned(),
            branch: branch.to_string(),
            base_branch: repo.default_branch.clone(),
            base_commit,
            origin,
            group,
            removed: false,
            snapshot_ref: None,
            created_at: now(),
        };
        d.store.insert_worktree(&worktree)?;
        d.events.workspace(
            &row.id,
            WorkspaceEvent::WorktreeAdded {
                worktree: worktree_info(worktree.clone()),
            },
        );
        match self.repo_settings(&repo.name, &repo.clone_path, path) {
            Ok(settings) => {
                if let Err(error) =
                    copy_files(&git, &repo.clone_path, path, &settings.files_to_copy).await
                {
                    self.warn(
                        &row.id,
                        format!("files to copy of {}: {error:#}", repo.name),
                    );
                }
            }
            Err(error) => self.warn(&row.id, format!("settings of {}: {error:#}", repo.name)),
        }
        Ok(worktree)
    }

    /// What each checkout holds beyond its commit, for the repos that hold anything.
    async fn leftovers(&self, worktrees: &[WorktreeRow]) -> Result<Vec<RepoLeftovers>> {
        let git = self.git();
        let mut repos = Vec::new();
        for w in worktrees {
            let files = git
                .leftovers(Path::new(&w.path))
                .await
                .map_err(|e| Error::Internal(e.context(format!("status of {}", w.repo))))?;
            if !files.is_empty() {
                repos.push(RepoLeftovers {
                    repo: w.repo.clone(),
                    files,
                });
            }
        }
        Ok(repos)
    }

    /// An exploration's checkout: the base branch as just fetched, detached, without the files to
    /// copy (`.env`…): nothing is meant to run there.
    async fn add_explore_worktree(
        &self,
        row: &WorkspaceRow,
        repo: &ResolvedRepo,
        path: &Path,
    ) -> Result<WorktreeRow> {
        let d = self.d();
        let git = self.git();
        let remote = format!("origin/{}", repo.default_branch);
        let start = if git.rev_parse(&repo.clone_path, &remote).await.is_ok() {
            remote
        } else {
            repo.default_branch.clone()
        };
        self.progress(&row.id, format!("checking out {} at {start}", repo.name));
        git.worktree_add_detached(&repo.clone_path, path, &start)
            .await
            .map_err(|e| Error::Internal(e.context(format!("checkout of {}", repo.name))))?;
        let worktree = WorktreeRow {
            id: uuid::Uuid::now_v7().to_string(),
            workspace_id: row.id.clone(),
            repo: repo.name.clone(),
            clone_path: repo.clone_path.to_string_lossy().into_owned(),
            path: path.to_string_lossy().into_owned(),
            branch: String::new(),
            base_branch: repo.default_branch.clone(),
            base_commit: git.rev_parse(path, "HEAD").await.ok(),
            origin: WorktreeOrigin::Workspace,
            group: None,
            removed: false,
            snapshot_ref: None,
            created_at: now(),
        };
        d.store.insert_worktree(&worktree)?;
        d.events.workspace(
            &row.id,
            WorkspaceEvent::WorktreeAdded {
                worktree: worktree_info(worktree.clone()),
            },
        );
        Ok(worktree)
    }

    async fn rollback(&self, row: &WorkspaceRow) {
        let d = self.d();
        let git = self.git();
        for worktree in d.store.worktrees_of(&row.id).unwrap_or_default() {
            let _ = git
                .worktree_remove(Path::new(&worktree.clone_path), Path::new(&worktree.path))
                .await;
            let _ = git
                .delete_branch(Path::new(&worktree.clone_path), &worktree.branch)
                .await;
            let _ = d.store.set_worktree_removed(&worktree.id, true, None);
        }
        let _ = std::fs::remove_dir_all(&row.path);
        let _ = d.store.release_port_blocks(&row.id);
    }

    /// Port blocks of this machine for a workspace that arrived from another: its own and each of
    /// its agent worktree groups' (their setup scripts get `CHAUFFEUR_PORT` from them), kept when
    /// it had them (a workspace coming back). An exploration runs no server: no ports.
    pub fn allocate_arrived_ports(&self, workspace_id: &str) -> Result<()> {
        let d = self.d();
        let row = self.row(workspace_id)?;
        if row.explore {
            return Ok(());
        }
        let settings = d.settings.get();
        let mut owners = vec![port_owner(&row.id, None)];
        for worktree in d.store.worktrees_of(&row.id)? {
            if worktree.removed {
                continue;
            }
            let owner = port_owner(&row.id, worktree.group.as_deref());
            if !owners.contains(&owner) {
                owners.push(owner);
            }
        }
        for owner in owners {
            crate::ports::allocate(
                &d.store,
                &owner,
                &row.id,
                settings.port_range_start,
                settings.port_block_size,
            )?;
        }
        Ok(())
    }

    /// Rewrites CLAUDE.md, AGENTS.md, `.claude/settings.json` and `.mcp.json` at the root.
    pub fn regenerate_config(&self, workspace_id: &str) -> Result<()> {
        let d = self.d();
        let row = self.row(workspace_id)?;
        let repos: Vec<genconfig::RepoEntry> = d
            .store
            .worktrees_of(&row.id)?
            .into_iter()
            .filter(|w| !w.removed && w.origin == WorktreeOrigin::Workspace)
            .map(|w| genconfig::RepoEntry {
                repo: w.repo,
                path: PathBuf::from(w.path),
                base_branch: w.base_branch,
            })
            .collect();
        let block = d.store.port_block(&row.id)?;
        let parent_slug = match &row.parent_id {
            Some(parent) => d.store.find_workspace(parent)?.map(|p| p.slug),
            None => None,
        };
        let cli = cli_path();
        let warnings = genconfig::write(&genconfig::WorkspaceConfig {
            slug: &row.slug,
            branch: &row.branch,
            root: Path::new(&row.path),
            repos: &repos,
            tickets: &row.tickets,
            port_base: block.as_ref().map(|b| b.base).unwrap_or(0),
            port_count: block.as_ref().map(|b| b.size).unwrap_or(0),
            parent_slug: parent_slug.as_deref(),
            cli: &cli,
            explore: row.explore,
        })?;
        for warning in warnings {
            self.warn(&row.id, warning);
        }
        Ok(())
    }

    /// A repo's settings, the user's `[repos.<name>.scripts]` included.
    fn repo_settings(
        &self,
        repo: &str,
        clone: &Path,
        worktree: &Path,
    ) -> anyhow::Result<RepoSettings> {
        let settings = self.d().settings.get();
        let user = settings.repos.get(repo).and_then(|o| o.scripts.as_ref());
        RepoSettings::load(clone, worktree, user)
    }

    // --- environment ---

    /// The user's `[repos.<name>.env]`, but for the variables Chauffeur sets itself.
    fn user_env(&self, repo: &str) -> Vec<(String, String)> {
        let settings = self.d().settings.get();
        settings
            .repos
            .get(repo)
            .map(|o| {
                o.env
                    .0
                    .iter()
                    .filter(|(name, _)| {
                        *name != "PATH"
                            && !name.starts_with("CHAUFFEUR_")
                            && !name.starts_with("CONDUCTOR_")
                    })
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Variables of a script running in `worktree`. Conductor's names are set too, so that
    /// repos still configured for Conductor keep working.
    pub fn script_env(
        &self,
        row: &WorkspaceRow,
        worktree: &WorktreeRow,
    ) -> Result<BTreeMap<String, String>> {
        let d = self.d();
        let owner = port_owner(&row.id, worktree.group.as_deref());
        let block = d.store.port_block(&owner)?;
        let mut env = self.agent_env(row, false)?;
        // Its own repo's variables win over another repo's of the same name.
        env.extend(self.user_env(&worktree.repo));
        let port = block.map(|b| b.base).unwrap_or(0).to_string();
        let siblings = d
            .store
            .worktrees_of(&row.id)?
            .into_iter()
            .filter(|w| !w.removed && w.group == worktree.group);
        for sibling in siblings {
            env.insert(repo_path_var(&sibling.repo), sibling.path.clone());
        }
        env.insert("CHAUFFEUR_PORT".into(), port.clone());
        env.insert("CHAUFFEUR_REPO".into(), worktree.repo.clone());
        env.insert("CHAUFFEUR_REPO_PATH".into(), worktree.path.clone());
        env.insert("CHAUFFEUR_ROOT_PATH".into(), worktree.clone_path.clone());
        env.insert(
            "CHAUFFEUR_DEFAULT_BRANCH".into(),
            worktree.base_branch.clone(),
        );
        env.insert("CONDUCTOR_WORKSPACE_ID".into(), row.id.clone());
        env.insert("CONDUCTOR_WORKSPACE_NAME".into(), row.slug.clone());
        env.insert("CONDUCTOR_WORKSPACE_PATH".into(), worktree.path.clone());
        env.insert("CONDUCTOR_ROOT_PATH".into(), worktree.clone_path.clone());
        env.insert(
            "CONDUCTOR_DEFAULT_BRANCH".into(),
            worktree.base_branch.clone(),
        );
        env.insert("CONDUCTOR_PORT".into(), port);
        env.insert("CONDUCTOR_IS_LOCAL".into(), "1".into());
        Ok(env)
    }

    /// Variables of an agent session (and its tools) in the workspace.
    /// `confined`: a design chat's agent, which writes its own folder only, gets the CLI no
    /// more than an exploration's.
    pub fn agent_env(
        &self,
        row: &WorkspaceRow,
        confined: bool,
    ) -> Result<BTreeMap<String, String>> {
        let d = self.d();
        let block = d.store.port_block(&row.id)?;
        let mut env = BTreeMap::new();
        for worktree in d.store.worktrees_of(&row.id)? {
            if !worktree.removed && worktree.origin == WorktreeOrigin::Workspace {
                env.extend(self.user_env(&worktree.repo));
            }
        }
        let without_cli = row.explore || confined;
        // The `chauffeur` CLI an agent runs must reach this daemon, whatever its home. Not in an
        // exploration or a design chat: its agent has nothing to ask the daemon, and with the CLI
        // it could create a workspace that edits, or promote its own (the daemon cannot tell who
        // calls it).
        if !without_cli {
            env.insert(
                "CHAUFFEUR_HOME".into(),
                d.paths.home.to_string_lossy().into_owned(),
            );
        }
        env.insert("CHAUFFEUR_WORKSPACE_ID".into(), row.id.clone());
        env.insert("CHAUFFEUR_WORKSPACE_NAME".into(), row.slug.clone());
        env.insert("CHAUFFEUR_WORKSPACE_PATH".into(), row.path.clone());
        if row.explore {
            env.insert("CHAUFFEUR_EXPLORE".into(), "1".into());
        }
        if let Some(block) = block {
            env.insert("CHAUFFEUR_PORT".into(), block.base.to_string());
            env.insert("CHAUFFEUR_PORT_COUNT".into(), block.size.to_string());
        }
        for worktree in d.store.worktrees_of(&row.id)? {
            if !worktree.removed && worktree.origin == WorktreeOrigin::Workspace {
                env.insert(repo_path_var(&worktree.repo), worktree.path);
            }
        }
        let base_path = d.shell_env.vars().get("PATH").cloned().unwrap_or_default();
        if without_cli {
            env.insert("PATH".into(), base_path);
        } else {
            let cli_dir = cli_path()
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            env.insert("PATH".into(), format!("{cli_dir}:{base_path}"));
        }
        Ok(env)
    }

    // --- scripts ---

    /// Starts the setup script of every live worktree (of one group when given), in background.
    /// Starts the setup script of every worktree of a workspace that arrived by transfer, as
    /// for a new workspace; returns the names of those started.
    pub(crate) fn start_arrived_setup(&self, workspace_id: &str) -> Result<Vec<String>> {
        let mut groups: Vec<Option<String>> = self
            .d()
            .store
            .worktrees_of(workspace_id)?
            .into_iter()
            .filter(|w| !w.removed)
            .map(|w| w.group)
            .collect();
        groups.sort();
        groups.dedup();
        let mut started = Vec::new();
        for group in groups {
            started.extend(self.start_setup_scripts(workspace_id, group.as_deref())?);
        }
        Ok(started)
    }

    fn start_setup_scripts(&self, workspace_id: &str, group: Option<&str>) -> Result<Vec<String>> {
        let d = self.d();
        let row = self.row(workspace_id)?;
        let mut started = Vec::new();
        for worktree in d.store.worktrees_of(workspace_id)? {
            if worktree.removed || worktree.group.as_deref() != group {
                continue;
            }
            let settings = self
                .repo_settings(
                    &worktree.repo,
                    Path::new(&worktree.clone_path),
                    Path::new(&worktree.path),
                )
                .unwrap_or_default();
            let Some(command) = settings.setup else {
                continue;
            };
            let spec = ScriptSpec {
                workspace_id: workspace_id.to_string(),
                kind: ProcessKind::Setup,
                repo: Some(worktree.repo.clone()),
                name: group.map_or_else(
                    || format!("setup {}", worktree.repo),
                    |group| format!("setup {} ({group})", worktree.repo),
                ),
                command,
                cwd: PathBuf::from(&worktree.path),
                env: self.script_env(&row, &worktree)?,
            };
            let name = spec.name.clone();
            started.push(name);
            match d.procs.spawn_setup(spec) {
                Ok(_) => {}
                Err(error) => self.warn(
                    workspace_id,
                    format!("setup of {}: {error:#}", worktree.repo),
                ),
            }
        }
        Ok(started)
    }

    pub fn processes(&self, key: &str) -> Result<ProcessesResult> {
        let d = self.d();
        let row = self.row(key)?;
        let mut scripts = Vec::new();
        for worktree in d.store.worktrees_of(&row.id)? {
            if worktree.removed || worktree.origin != WorktreeOrigin::Workspace {
                continue;
            }
            let settings = self
                .repo_settings(
                    &worktree.repo,
                    Path::new(&worktree.clone_path),
                    Path::new(&worktree.path),
                )
                .unwrap_or_default();
            scripts.extend(
                settings
                    .run
                    .iter()
                    .map(|s| format!("{}:{}", worktree.repo, s.name)),
            );
        }
        Ok(ProcessesResult {
            processes: d.procs.list(&row.id),
            scripts,
        })
    }

    /// Retries the configured setup, retaining the previous process and its log.
    pub fn retry_setup(&self, key: &str, process_id: &str) -> Result<ProcessInfo> {
        let d = self.d();
        let row = self.row(key)?;
        let _admission = d.execution.admit(&row.id)?;
        read_only(&row, "it runs no setup")?;
        if row.state != WorkspaceState::Ready {
            return Err(Error::Conflict(format!(
                "{} is not ready; wait for its transfer to finish",
                row.slug
            )));
        }
        let previous = d.procs.get(process_id).filter(|p| p.workspace_id == row.id && p.kind == ProcessKind::Setup)
            .ok_or_else(|| Error::not_found("The setup process is no longer available; run the repo's setup in its worktree terminal"))?;
        let cwd = d
            .procs
            .directory(process_id)
            .ok_or_else(|| Error::not_found("setup directory"))?;
        let worktree = d
            .store
            .worktrees_of(&row.id)?
            .into_iter()
            .find(|w| !w.removed && Path::new(&w.path) == cwd)
            .ok_or_else(|| Error::not_found("The setup's worktree was removed"))?;
        let settings = self.repo_settings(
            &worktree.repo,
            Path::new(&worktree.clone_path),
            Path::new(&worktree.path),
        )?;
        let command = settings
            .setup
            .ok_or_else(|| Error::not_found(format!("setup of {}", worktree.repo)))?;
        Ok(d.procs.spawn_setup(ScriptSpec {
            workspace_id: row.id.clone(),
            kind: ProcessKind::Setup,
            repo: Some(worktree.repo.clone()),
            name: previous.name,
            command,
            cwd,
            env: self.script_env(&row, &worktree)?,
        })?)
    }

    pub async fn run_script(
        &self,
        key: &str,
        repo: &str,
        name: Option<&str>,
    ) -> Result<ProcessInfo> {
        let d = self.d();
        let row = self.row(key)?;
        read_only(&row, "it runs no script")?;
        if row.state != WorkspaceState::Ready {
            return Err(Error::Conflict(format!(
                "{} is {}",
                row.slug,
                row.state.as_str()
            )));
        }
        let worktree = d
            .store
            .worktrees_of(&row.id)?
            .into_iter()
            .find(|w| !w.removed && w.origin == WorktreeOrigin::Workspace && w.repo == repo)
            .ok_or_else(|| Error::not_found(format!("repo {repo} in {}", row.slug)))?;
        let settings = self.repo_settings(
            &worktree.repo,
            Path::new(&worktree.clone_path),
            Path::new(&worktree.path),
        )?;
        let script = match name {
            Some(name) => settings.run.iter().find(|s| s.name == name),
            None => settings.default_run(),
        }
        .ok_or_else(|| {
            Error::not_found(format!(
                "run script {} of {repo}",
                name.unwrap_or("(default)")
            ))
        })?
        .clone();
        if !settings.run_concurrent {
            for other in d.procs.running_runs_of_repo(repo) {
                d.procs.stop(&other.id).await;
            }
        }
        let spec = ScriptSpec {
            workspace_id: row.id.clone(),
            kind: ProcessKind::Run,
            repo: Some(repo.to_string()),
            name: format!("{repo}:{}", script.name),
            command: script.command,
            cwd: PathBuf::from(&worktree.path),
            env: self.script_env(&row, &worktree)?,
        };
        Ok(d.procs.spawn(spec)?)
    }

    // --- repos and agent worktrees ---

    /// Adds a repo to a workspace, on the workspace's branch.
    pub async fn add_repo(&self, key: &str, repo: &str) -> Result<WorkspaceInfo> {
        Ok(self.add_repo_to(key, repo).await?.0)
    }

    /// Adds a repo to a workspace, on the workspace's branch (a detached checkout in an
    /// exploration), and says whether its setup script was started. The workspace's agents take
    /// it in from their next message.
    pub async fn add_repo_to(&self, key: &str, reference: &str) -> Result<(WorkspaceInfo, bool)> {
        parse_repo_ref(reference)?;
        // `owner/name` takes the folder `name`, as one of the main organization's would.
        let repo = repo_folder(reference);
        let d = self.d();
        let row = self.row(key)?;
        let _admission = d.execution.admit(&row.id)?;
        if row.state != WorkspaceState::Ready {
            return Err(Error::Conflict(format!(
                "{} is {}",
                row.slug,
                row.state.as_str()
            )));
        }
        let exists = d
            .store
            .worktrees_of(&row.id)?
            .iter()
            .any(|w| !w.removed && w.origin == WorktreeOrigin::Workspace && w.repo == repo);
        if exists {
            return Err(Error::Conflict(format!(
                "{repo} is already in {}",
                row.slug
            )));
        }
        let path = PathBuf::from(&row.path).join(repo);
        self.add_worktree(
            &row,
            reference,
            &path,
            &row.branch,
            None,
            WorktreeOrigin::Workspace,
            None,
        )
        .await?;
        self.regenerate_config(&row.id)?;
        // Setup of the new repo only.
        let worktree = d
            .store
            .worktrees_of(&row.id)?
            .into_iter()
            .find(|w| !w.removed && w.origin == WorktreeOrigin::Workspace && w.repo == repo)
            .expect("just added");
        let setup = self
            .repo_settings(&worktree.repo, Path::new(&worktree.clone_path), &path)?
            .setup
            .filter(|_| !row.explore);
        let started = setup.is_some();
        if let Some(command) = setup {
            d.procs.spawn(ScriptSpec {
                workspace_id: row.id.clone(),
                kind: ProcessKind::Setup,
                repo: Some(repo.to_string()),
                name: format!("setup {repo}"),
                command,
                cwd: path.clone(),
                env: self.script_env(&row, &worktree)?,
            })?;
        }
        self.publish(&row.id);
        // In the background: closing the other chats' idle sessions need not delay the caller
        // (the agent's `add_repo` tool waits for this answer).
        let (daemon, workspace_id, added) = (d.clone(), row.id.clone(), repo.to_string());
        tokio::spawn(async move { daemon.chats.repo_added(&workspace_id, &added).await });
        Ok((self.get(&row.id)?, started))
    }

    /// Creates a group of worktrees for an agent, in `<workspace>/.worktrees/<name>/<repo>`, each
    /// cut from the workspace worktree's HEAD (or the base branch for repos outside the workspace).
    /// Idempotent: an existing group with these repos is returned as is.
    pub async fn add_agent_worktrees(
        &self,
        params: AddWorktreeParams,
    ) -> Result<AddWorktreeResult> {
        let d = self.d();
        let row = self.row(&params.workspace)?;
        let _admission = d.execution.admit(&row.id)?;
        read_only(&row, "agents create no worktree there")?;
        if row.state != WorkspaceState::Ready {
            return Err(Error::Conflict(format!(
                "{} is {}",
                row.slug,
                row.state.as_str()
            )));
        }
        let own: Vec<WorktreeRow> = d
            .store
            .worktrees_of(&row.id)?
            .into_iter()
            .filter(|w| !w.removed)
            .collect();
        let mut repos = params.repos.clone();
        if repos.is_empty() {
            repos = own
                .iter()
                .filter(|w| w.origin == WorktreeOrigin::Workspace)
                .map(|w| w.repo.clone())
                .collect();
        }
        dedup(&mut repos);
        check_repo_refs(&repos)?;
        let name = params
            .name
            .as_deref()
            .map(|n| slug::slugify(n, 40))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("wt-{}", &uuid::Uuid::now_v7().simple().to_string()[24..]));
        let group_dir = PathBuf::from(&row.path)
            .join(AGENT_WORKTREES_DIR)
            .join(&name);
        let branch = params
            .branch
            .clone()
            .filter(|b| !b.trim().is_empty())
            .unwrap_or_else(|| format!("{}--{name}", row.branch));
        validate_branch_name(&branch)?;
        std::fs::create_dir_all(&group_dir)?;
        crate::ports::allocate(
            &d.store,
            &port_owner(&row.id, Some(&name)),
            &row.id,
            d.settings.get().port_range_start,
            d.settings.get().port_block_size,
        )?;
        let mut created = Vec::new();
        for reference in &repos {
            let repo = repo_folder(reference);
            if let Some(existing) = own
                .iter()
                .find(|w| w.group.as_deref() == Some(name.as_str()) && w.repo == repo)
            {
                created.push(worktree_info(existing.clone()));
                continue;
            }
            let start = own
                .iter()
                .find(|w| w.origin == WorktreeOrigin::Workspace && w.repo == repo)
                .and_then(|w| w.base_commit.clone().map(|_| w.path.clone()));
            let start = match start {
                Some(path) => Some(self.git().rev_parse(Path::new(&path), "HEAD").await?),
                None => None,
            };
            let worktree = self
                .add_worktree(
                    &row,
                    reference,
                    &group_dir.join(repo),
                    &branch,
                    start,
                    WorktreeOrigin::Agent,
                    Some(name.clone()),
                )
                .await?;
            created.push(worktree_info(worktree));
        }
        self.start_setup_scripts(&row.id, Some(&name))?;
        self.publish(&row.id);
        Ok(AddWorktreeResult {
            path: group_dir.to_string_lossy().into_owned(),
            worktrees: created,
        })
    }

    /// Removes agent worktrees by id, path, group folder or group name.
    pub async fn remove_agent_worktrees(
        &self,
        key: &str,
        target: &str,
    ) -> Result<RemoveWorktreeResult> {
        let d = self.d();
        let row = self.row(key)?;
        let _admission = d.execution.admit(&row.id)?;
        let target_path = PathBuf::from(target);
        let matching: Vec<WorktreeRow> = d
            .store
            .worktrees_of(&row.id)?
            .into_iter()
            .filter(|w| !w.removed && w.origin == WorktreeOrigin::Agent)
            .filter(|w| {
                w.id == target
                    || w.group.as_deref() == Some(target)
                    || Path::new(&w.path) == target_path
                    || Path::new(&w.path).starts_with(&target_path)
            })
            .collect();
        if matching.is_empty() {
            return Err(Error::not_found(format!(
                "agent worktree {target} in {}",
                row.slug
            )));
        }
        let git = self.git();
        let mut removed = Vec::new();
        let mut groups = Vec::new();
        for worktree in matching {
            let path = PathBuf::from(&worktree.path);
            procs::kill_inside(&path, Duration::from_secs(2)).await;
            self.stop_compose_projects(&row.id, &path, &[path.as_path()])
                .await;
            let lock = d.repos.lock_for(Path::new(&worktree.clone_path));
            let _guard = lock.lock().await;
            git.worktree_remove(Path::new(&worktree.clone_path), &path)
                .await?;
            d.store.set_worktree_removed(&worktree.id, true, None)?;
            d.events.workspace(
                &row.id,
                WorkspaceEvent::WorktreeRemoved {
                    worktree_id: worktree.id.clone(),
                },
            );
            if let Some(group) = worktree.group.clone() {
                groups.push(group);
            }
            removed.push(worktree.id);
        }
        groups.sort();
        groups.dedup();
        let remaining = d.store.worktrees_of(&row.id)?;
        for group in groups {
            if !remaining
                .iter()
                .any(|w| !w.removed && w.group.as_deref() == Some(group.as_str()))
            {
                d.store
                    .release_port_block(&port_owner(&row.id, Some(&group)))?;
                let dir = PathBuf::from(&row.path)
                    .join(AGENT_WORKTREES_DIR)
                    .join(&group);
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        self.publish(&row.id);
        Ok(RemoveWorktreeResult { removed })
    }

    /// Resolves a workspace from a path inside it (used by Claude's WorktreeRemove hook).
    pub fn find_by_path(&self, path: &Path) -> Result<Option<WorkspaceRow>> {
        Ok(self
            .d()
            .store
            .list_workspaces()?
            .into_iter()
            .find(|row| path.starts_with(&row.path)))
    }

    // --- archive and restore ---

    pub async fn archive(&self, key: &str, force: bool) -> Result<WorkspaceInfo> {
        let row = self.row(key)?;
        if row.state == WorkspaceState::Archived {
            return self.info(&row);
        }
        // A workspace fenced for a transfer keeps its source until the transfer settles.
        let _admission = self.d().execution.admit(&row.id)?;
        self.check_archivable(&row, force)?;
        self.archive_checked(row, force).await
    }

    /// Starts archiving and returns at once, the workspace `archiving`: its state then says how it
    /// went (`archived`, or `cleanup_failed` with the reason), its events what it did.
    pub fn archive_in_background(&self, key: &str, force: bool) -> Result<WorkspaceInfo> {
        let d = self.d();
        let row = self.row(key)?;
        if row.state == WorkspaceState::Archived {
            return self.info(&row);
        }
        let admission = d.execution.admit(&row.id)?;
        self.check_archivable(&row, force)?;
        self.set_state(&row.id, WorkspaceState::Archiving, None)?;
        d.shares.stop_for_workspace(&row.id);
        d.workspace_shares.stop_for_workspace(&row.id);
        let info = self.info(&self.row(&row.id)?)?;
        tokio::spawn(async move {
            let _admission = admission;
            let id = row.id.clone();
            if let Err(error) = d.workspaces.archive_checked(row, force).await {
                tracing::warn!(%error, workspace = id, "archive failed");
                let _ = d.workspaces.set_state(
                    &id,
                    WorkspaceState::CleanupFailed,
                    Some(error.to_string()),
                );
            }
        });
        Ok(info)
    }

    fn check_archivable(&self, row: &WorkspaceRow, force: bool) -> Result<()> {
        let d = self.d();
        if matches!(
            row.state,
            WorkspaceState::Creating | WorkspaceState::Archiving
        ) {
            return Err(Error::Conflict(format!(
                "{} is {}",
                row.slug,
                row.state.as_str()
            )));
        }
        let chats = d.store.chats_of(&row.id)?;
        if !force
            && chats
                .iter()
                .any(|c| matches!(c.status, ChatStatus::Working | ChatStatus::Starting))
        {
            return Err(Error::Conflict(format!(
                "an agent is working in {}: interrupt it or archive with force",
                row.slug
            )));
        }
        Ok(())
    }

    /// Archives a workspace that may be (see check_archivable); `force` archives it even when a
    /// cleanup step failed.
    async fn archive_checked(&self, row: WorkspaceRow, force: bool) -> Result<WorkspaceInfo> {
        let d = self.d();
        // Children first: archiving the parent archives its children.
        let children: Vec<WorkspaceRow> = d
            .store
            .list_workspaces()?
            .into_iter()
            .filter(|w| {
                w.parent_id.as_deref() == Some(&row.id) && w.state != WorkspaceState::Archived
            })
            .collect();
        for child in children {
            // Moved to another machine, it comes back first and is archived here once back.
            if Box::pin(crate::cloud_transfer_driver::archive_moved(&d, &child.id))
                .await?
                .is_none()
            {
                Box::pin(self.archive(&child.id, true)).await?;
            }
        }

        self.set_state(&row.id, WorkspaceState::Archiving, None)?;
        let mut errors: Vec<String> = Vec::new();
        let root = PathBuf::from(&row.path);
        let git = self.git();

        self.progress(&row.id, "stopping agents and processes");
        d.chats.close_workspace(&row.id).await;
        // An exploration's temporary folder, now that its sessions are stopped.
        if let Err(error) = crate::explore_tmp::remove(&d.paths.explore_tmp_root, &row.id) {
            errors.push(format!("temporary folder: {error}"));
        }
        d.terminals.close_workspace(&row.id);
        d.procs.stop_workspace(&row.id).await;
        let killed = procs::kill_inside(&root, Duration::from_secs(3)).await;
        if killed > 0 {
            self.progress(&row.id, format!("stopped {killed} leftover processes"));
        }

        let worktrees: Vec<WorktreeRow> = d
            .store
            .worktrees_of(&row.id)?
            .into_iter()
            .filter(|w| !w.removed)
            .collect();

        for worktree in &worktrees {
            let settings = self
                .repo_settings(
                    &worktree.repo,
                    Path::new(&worktree.clone_path),
                    Path::new(&worktree.path),
                )
                .unwrap_or_default();
            // An exploration ran no setup: nothing for an archive script to undo.
            let Some(command) = settings.archive.filter(|_| !row.explore) else {
                continue;
            };
            self.progress(&row.id, format!("archive script of {}", worktree.repo));
            let spec = ScriptSpec {
                workspace_id: row.id.clone(),
                kind: ProcessKind::Archive,
                repo: Some(worktree.repo.clone()),
                name: format!("archive {}", worktree.repo),
                command,
                cwd: PathBuf::from(&worktree.path),
                env: self.script_env(&row, worktree)?,
            };
            let timeout = Duration::from_secs(d.settings.get().archive_script_timeout_secs);
            match d.procs.run_to_end(spec, timeout).await {
                Ok(0) => {}
                Ok(code) => errors.push(format!(
                    "archive script of {} exited with {code}",
                    worktree.repo
                )),
                Err(error) => {
                    errors.push(format!("archive script of {}: {error:#}", worktree.repo))
                }
            }
        }

        let checkouts: Vec<&Path> = worktrees.iter().map(|w| Path::new(&w.path)).collect();
        self.stop_compose_projects(&row.id, &root, &checkouts).await;

        for worktree in &worktrees {
            let path = Path::new(&worktree.path);
            if !path.exists() {
                d.store.set_worktree_removed(&worktree.id, true, None)?;
                continue;
            }
            if !row.explore
                && let Ok(count) = git.unpushed_commits(path).await
                && count > 0
            {
                self.warn(
                    &row.id,
                    format!(
                        "{} has {count} commit(s) on {} pushed nowhere; the snapshot keeps them for {} days",
                        worktree.repo, worktree.branch, d.settings.get().snapshot_retention_days
                    ),
                );
            }
            // An exploration's checkouts hold no work to keep.
            let ref_name = (!row.explore)
                .then(|| format!("refs/chauffeur/archive/{}/{}", row.id, worktree.id));
            let clone = Path::new(&worktree.clone_path);
            let lock = d.repos.lock_for(clone);
            let _guard = lock.lock().await;
            if let Some(ref_name) = &ref_name
                && let Err(error) = git.snapshot(path, ref_name).await
            {
                // Never remove a worktree whose work is not saved.
                errors.push(format!("snapshot of {}: {error:#}", worktree.repo));
                continue;
            }
            match git.worktree_remove(clone, path).await {
                Ok(()) => {
                    d.store
                        .set_worktree_removed(&worktree.id, true, ref_name.as_deref())?;
                    d.events.workspace(
                        &row.id,
                        WorkspaceEvent::WorktreeRemoved {
                            worktree_id: worktree.id.clone(),
                        },
                    );
                }
                Err(error) => errors.push(format!("removing {}: {error:#}", worktree.repo)),
            }
        }

        for worktree in worktrees.iter().filter(|_| !row.explore) {
            if worktree.origin == WorktreeOrigin::Workspace
                && pr_is_merged(&d, &git, worktree).await
            {
                let _ = git
                    .delete_branch(Path::new(&worktree.clone_path), &worktree.branch)
                    .await;
            }
        }

        if let Err(error) = self.archive_notes(&row) {
            errors.push(format!("notes: {error:#}"));
        }
        let leftovers = d.store.worktrees_of(&row.id)?.iter().any(|w| !w.removed);
        if !leftovers {
            let _ = std::fs::remove_dir_all(&root);
        }
        d.store.release_port_blocks(&row.id)?;
        d.procs.forget_workspace(&row.id);

        if errors.is_empty() || (force && !leftovers) {
            for error in &errors {
                self.warn(&row.id, error.clone());
            }
            self.set_state(&row.id, WorkspaceState::Archived, None)?;
        } else {
            self.set_state(
                &row.id,
                WorkspaceState::CleanupFailed,
                Some(errors.join("; ")),
            )?;
        }
        self.get(&row.id)
    }

    /// `docker compose down` the compose projects whose files live in `dir`, when one of
    /// `checkouts` tracks a compose file: without one, Docker is not called at all. A compose file
    /// that no repo tracks is missed, an accepted risk: asking Docker costs a call that can hang.
    /// Docker taking too long is a warning, never a failure: archiving goes on without it
    /// (decided with Justin 2026-10-18).
    async fn stop_compose_projects(&self, workspace_id: &str, dir: &Path, checkouts: &[&Path]) {
        let d = self.d();
        if !self.tracks_compose_file(checkouts).await {
            return;
        }
        let timeouts = self
            .compose_timeouts
            .lock()
            .unwrap()
            .unwrap_or(procs::ComposeTimeouts {
                ls: procs::COMPOSE_LS_TIMEOUT,
                down: Duration::from_secs(d.settings.get().archive_script_timeout_secs),
            });
        self.progress(workspace_id, "stopping docker compose projects");
        let outcome = procs::compose_down_inside(dir, &d.shell_env, timeouts).await;
        if !outcome.stopped.is_empty() {
            self.progress(
                workspace_id,
                format!(
                    "stopped docker compose projects: {}",
                    outcome.stopped.join(", ")
                ),
            );
        }
        for timeout in outcome.timed_out {
            self.warn(workspace_id, timeout.to_string());
        }
    }

    /// Whether one of `checkouts` tracks a compose file; when git cannot tell, Docker is asked.
    async fn tracks_compose_file(&self, checkouts: &[&Path]) -> bool {
        let git = self.git();
        for checkout in checkouts.iter().filter(|path| path.exists()) {
            if git.tracks_compose_file(checkout).await.unwrap_or(true) {
                return true;
            }
        }
        false
    }

    /// Moves the notes folder out of the workspace, without worktrees or dependency folders.
    fn archive_notes(&self, row: &WorkspaceRow) -> anyhow::Result<()> {
        let d = self.d();
        let notes = PathBuf::from(&row.path).join(NOTES_DIR);
        if !notes.exists() {
            return Ok(());
        }
        strip_heavy(&notes)?;
        let target = d.paths.archive_dir.join(&row.slug);
        if target.exists() {
            std::fs::remove_dir_all(&target)?;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if std::fs::rename(&notes, &target).is_err() {
            copy_dir(&notes, &target)?;
            std::fs::remove_dir_all(&notes)?;
        }
        Ok(())
    }

    pub async fn restore(&self, key: &str) -> Result<WorkspaceInfo> {
        let d = self.d();
        let row = self.row(key)?;
        read_only(
            &row,
            "start a new exploration rather than restoring this one",
        )?;
        if !matches!(
            row.state,
            WorkspaceState::Archived | WorkspaceState::CleanupFailed
        ) {
            return Err(Error::Conflict(format!(
                "{} is {}",
                row.slug,
                row.state.as_str()
            )));
        }
        let restorable: Vec<WorktreeRow> = d
            .store
            .worktrees_of(&row.id)?
            .into_iter()
            .filter(|w| w.removed && w.snapshot_ref.is_some())
            .collect();
        if restorable.is_empty() && row.state == WorkspaceState::Archived {
            return Err(Error::Conflict(format!(
                "{} has no snapshot left to restore (kept {} days)",
                row.slug,
                d.settings.get().snapshot_retention_days
            )));
        }
        self.set_state(&row.id, WorkspaceState::Creating, None)?;
        let root = PathBuf::from(&row.path);
        std::fs::create_dir_all(&root)?;
        let archived_notes = d.paths.archive_dir.join(&row.slug);
        let notes = root.join(NOTES_DIR);
        if archived_notes.exists()
            && !notes.exists()
            && std::fs::rename(&archived_notes, &notes).is_err()
        {
            copy_dir(&archived_notes, &notes)?;
            std::fs::remove_dir_all(&archived_notes)?;
        }
        std::fs::create_dir_all(&notes)?;
        crate::ports::allocate(
            &d.store,
            &row.id,
            &row.id,
            d.settings.get().port_range_start,
            d.settings.get().port_block_size,
        )?;
        let git = self.git();
        let mut errors = Vec::new();
        let mut groups = Vec::new();
        for worktree in &restorable {
            let snapshot = worktree.snapshot_ref.clone().unwrap();
            let clone = Path::new(&worktree.clone_path);
            let path = Path::new(&worktree.path);
            let lock = d.repos.lock_for(clone);
            let _guard = lock.lock().await;
            let result = async {
                let head = git
                    .run(clone, &["log", "-1", "--format=%B", &snapshot])
                    .await?
                    .lines()
                    .find_map(|l| l.strip_prefix("head ").map(str::to_string))
                    .ok_or_else(|| anyhow::anyhow!("snapshot {snapshot} has no head"))?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                git.worktree_add(clone, path, &worktree.branch, &head)
                    .await?;
                git.restore_snapshot(path, &snapshot).await?;
                anyhow::Ok(())
            }
            .await;
            match result {
                Ok(()) => {
                    d.store.set_worktree_removed(&worktree.id, false, None)?;
                    let mut restored = worktree.clone();
                    restored.removed = false;
                    d.events.workspace(
                        &row.id,
                        WorkspaceEvent::WorktreeAdded {
                            worktree: worktree_info(restored),
                        },
                    );
                    if let Some(group) = &worktree.group {
                        groups.push(group.clone());
                    }
                }
                Err(error) => errors.push(format!("{}: {error:#}", worktree.repo)),
            }
        }
        groups.sort();
        groups.dedup();
        for group in &groups {
            crate::ports::allocate(
                &d.store,
                &port_owner(&row.id, Some(group)),
                &row.id,
                d.settings.get().port_range_start,
                d.settings.get().port_block_size,
            )?;
        }
        self.regenerate_config(&row.id)?;
        if !errors.is_empty() {
            self.set_state(
                &row.id,
                WorkspaceState::CleanupFailed,
                Some(format!("restore failed: {}", errors.join("; "))),
            )?;
            return self.get(&row.id);
        }
        // Snapshots stay until expiry: restoring again after a new archive replaces them.
        for worktree in &restorable {
            d.store.clear_snapshot_ref(&worktree.id)?;
        }
        for chat in d.store.chats_of(&row.id)? {
            if chat.status == ChatStatus::Closed {
                d.chats.unarchive(&chat.id)?;
            }
        }
        self.set_state(&row.id, WorkspaceState::Ready, None)?;
        self.start_setup_scripts(&row.id, None)?;
        for group in &groups {
            self.start_setup_scripts(&row.id, Some(group))?;
        }
        self.get(&row.id)
    }

    /// Deletes archive snapshots past their retention.
    pub async fn purge_expired_snapshots(&self) -> Result<usize> {
        let d = self.d();
        let cutoff =
            chrono::Utc::now() - chrono::Duration::days(d.settings.get().snapshot_retention_days);
        let expired = d.store.expired_snapshots(&cutoff.to_rfc3339())?;
        let git = self.git();
        for worktree in &expired {
            if let Some(ref_name) = &worktree.snapshot_ref {
                let _ = git
                    .delete_ref(Path::new(&worktree.clone_path), ref_name)
                    .await;
            }
            d.store.clear_snapshot_ref(&worktree.id)?;
        }
        Ok(expired.len())
    }

    // --- waiting ---

    /// Waits until no agent of the workspace is working (or the timeout); given a chat's id, until
    /// that chat's agent is not.
    pub async fn wait(&self, key: &str, timeout: Option<Duration>) -> Result<WaitResult> {
        let d = self.d();
        if let Some(chat) = d.store.chat(key)? {
            let (chat, timed_out) = d.chats.wait_settled(&chat.id, false, timeout).await?;
            return Ok(WaitResult {
                workspace: self.get(&chat.workspace_id)?,
                last_text: d.chats.last_text(&chat.id)?,
                chats: vec![chat],
                timed_out,
            });
        }
        let row = self.row(key)?;
        let mut rx = d.events.subscribe();
        let deadline = timeout.map(|t| tokio::time::Instant::now() + t);
        let timed_out = loop {
            let current = self.row(&row.id)?;
            let chats = d.store.chats_of(&row.id)?;
            // A chat just given a message counts too: its agent reports the turn a moment later.
            let busy = matches!(
                current.state,
                WorkspaceState::Creating | WorkspaceState::Archiving
            ) || chats
                .iter()
                .any(|c| !d.chats.chat_settled(c, false).unwrap_or(true));
            if !busy {
                break false;
            }
            let next = rx.recv();
            match deadline {
                Some(deadline) => match tokio::time::timeout_at(deadline, next).await {
                    Ok(_) => {}
                    Err(_) => break true,
                },
                None => {
                    let _ = next.await;
                }
            }
        };
        let chats = d.store.chats_of(&row.id)?;
        let last_text = match chats.iter().find(|chat| !chat.closed).or(chats.first()) {
            Some(chat) => d.chats.last_text(&chat.id)?,
            None => None,
        };
        Ok(WaitResult {
            workspace: self.get(&row.id)?,
            chats,
            last_text,
            timed_out,
        })
    }
}

/// A group of the sidebar as it shows it, in its order: the top-level workspaces shown (not
/// archived, their parent not shown), pinned or not, and when not pinned, of `section`.
fn sidebar_group(rows: &[WorkspaceRow], pinned: bool, section: Option<&str>) -> Vec<(String, f64)> {
    let shown = |r: &WorkspaceRow| {
        !matches!(
            r.state,
            WorkspaceState::Archived | WorkspaceState::Archiving
        )
    };
    let shown_ids: HashSet<&str> = rows
        .iter()
        .filter(|r| shown(r))
        .map(|r| r.id.as_str())
        .collect();
    let mut group: Vec<(String, f64)> = rows
        .iter()
        .filter(|r| shown(r) && r.pinned == pinned)
        .filter(|r| pinned || r.section_id.as_deref() == section)
        .filter(|r| {
            r.parent_id
                .as_deref()
                .is_none_or(|p| !shown_ids.contains(p))
        })
        .map(|r| (r.id.clone(), r.position))
        .collect();
    group.sort_by(|a, b| b.1.total_cmp(&a.1));
    group
}

pub fn activity(statuses: impl Iterator<Item = ChatStatus>) -> Activity {
    let statuses: Vec<ChatStatus> = statuses.collect();
    if statuses.contains(&ChatStatus::NeedsInput) {
        Activity::NeedsInput
    } else if statuses
        .iter()
        .any(|s| matches!(s, ChatStatus::Working | ChatStatus::Starting))
    {
        Activity::Working
    } else if statuses.contains(&ChatStatus::Error) {
        Activity::Error
    } else {
        Activity::Idle
    }
}

fn worktree_info(row: WorktreeRow) -> WorktreeInfo {
    WorktreeInfo {
        id: row.id,
        repo: row.repo,
        path: row.path,
        branch: row.branch,
        base_commit: row.base_commit,
        base_branch: row.base_branch,
        origin: row.origin,
        group: row.group,
        removed: row.removed,
    }
}

fn port_owner(workspace_id: &str, group: Option<&str>) -> String {
    match group {
        Some(group) => format!("{workspace_id}:{group}"),
        None => workspace_id.to_string(),
    }
}

pub fn repo_path_var(repo: &str) -> String {
    let name: String = repo
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("CHAUFFEUR_REPO_PATH_{name}")
}

/// The ticket that names a workspace and its branch: the one the user picked, when they picked one,
/// even with its sub-issues attached alongside (they come after it).
/// `<branch>-2`, `-3`… the first that none of `taken` is.
fn free_branch(branch: &str, taken: &HashSet<String>) -> String {
    (2..)
        .map(|n| format!("{branch}-{n}"))
        .find(|candidate| !taken.contains(candidate))
        .expect("a free suffix")
}

fn naming_ticket(picked: usize, tickets: &[TicketRef]) -> Option<&TicketRef> {
    (picked == 1).then(|| tickets.first()).flatten()
}

/// The tickets of a workspace its agents have no content of: every one when the briefing is
/// missing, else those Linear never described (no title).
fn unread_tickets(row: &WorkspaceRow) -> Vec<String> {
    let briefing = Path::new(&row.path)
        .join(NOTES_DIR)
        .join("tickets.md")
        .exists();
    row.tickets
        .iter()
        .filter(|t| !briefing || t.title.is_none())
        .map(|t| t.identifier.clone())
        .collect()
}

/// A promote refused for what the checkouts hold: the files in the message (for the CLI) and in
/// the data (for the app, which asks the user before discarding them).
fn leftovers_refusal(slug: &str, repos: Vec<RepoLeftovers>) -> Error {
    let list: Vec<String> = repos
        .iter()
        .map(|r| format!("{}: {}", r.repo, files_line(&r.files)))
        .collect();
    let message = format!(
        "{slug}'s checkouts hold files their commit does not, left by the exploration's commands or by hand ({}); they would be carried into the workspace. To continue, discard them (`chauffeur promote {slug} --discard`, which deletes them for good); to keep them, leave it an exploration",
        list.join("; ")
    );
    let data = serde_json::to_value(PromoteLeftovers { repos }).unwrap_or_default();
    Error::ConflictWith(message, data)
}

/// The first files of a list, and how many more.
fn files_line(files: &[String]) -> String {
    const SHOWN: usize = 10;
    let mut line = files
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if files.len() > SHOWN {
        line.push_str(&format!(" and {} more", files.len() - SHOWN));
    }
    line
}

/// Refuses, in an exploration, what only a workspace does.
pub fn read_only(row: &WorkspaceRow, what: &str) -> Result<()> {
    if row.explore {
        return Err(Error::Conflict(format!(
            "{} is an exploration, read-only: {what}; create a workspace to change code",
            row.slug
        )));
    }
    Ok(())
}

/// Repos named in a request (`name` or `owner/name`): valid, and one per folder, since a
/// workspace holds each repo in a folder of its short name.
fn check_repo_refs(references: &[String]) -> Result<()> {
    let mut folders: HashMap<&str, &str> = HashMap::new();
    for reference in references {
        parse_repo_ref(reference)?;
        if let Some(other) = folders.insert(repo_folder(reference), reference) {
            return Err(Error::BadRequest(format!(
                "{other} and {reference} would share the folder {}: a workspace holds one repo of each name",
                repo_folder(reference)
            )));
        }
    }
    Ok(())
}

fn dedup(names: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| !n.trim().is_empty() && seen.insert(n.clone()));
}

/// The `chauffeur` CLI next to the daemon's binary, or the one on PATH.
pub fn cli_path() -> PathBuf {
    if let Some(path) = std::env::var_os("CHAUFFEUR_CLI") {
        return PathBuf::from(path);
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("chauffeur")))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("chauffeur"))
}

/// Copies the clone's gitignored files matching `patterns` into the worktree (copy-on-write on
/// APFS). Fully ignored folders such as `node_modules` are never entered.
async fn copy_files(
    git: &Git,
    clone: &Path,
    worktree: &Path,
    patterns: &[String],
) -> anyhow::Result<usize> {
    if patterns.is_empty() {
        return Ok(0);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern.trim_start_matches('/'))?);
    }
    let set = builder.build()?;
    let mut copied = 0;
    for entry in git.ignored_entries(clone).await? {
        if entry.ends_with('/') || !set.is_match(&entry) {
            continue;
        }
        let source = clone.join(&entry);
        let target = worktree.join(&entry);
        if target.exists() || !source.is_file() {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&source, &target)?;
        copied += 1;
    }
    Ok(copied)
}

/// Removes git worktrees and dependency folders from a notes folder before it is kept.
fn strip_heavy(dir: &Path) -> anyhow::Result<()> {
    let mut doomed = Vec::new();
    let mut walker = walkdir::WalkDir::new(dir).min_depth(1).into_iter();
    while let Some(entry) = walker.next() {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy();
        let heavy = entry.file_type().is_dir()
            && (matches!(
                name.as_ref(),
                "node_modules" | ".venv" | "target" | ".next" | "dist"
            ) || entry.path().join(".git").exists());
        if heavy {
            doomed.push(entry.path().to_path_buf());
            walker.skip_current_dir();
        }
    }
    for path in doomed {
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

pub(crate) fn copy_dir(from: &Path, to: &Path) -> anyhow::Result<()> {
    for entry in walkdir::WalkDir::new(from) {
        let entry = entry?;
        let relative = entry.path().strip_prefix(from)?;
        let target = to.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Whether the branch of a worktree being archived has a merged PR, found as the header finds it
/// ([`crate::prs::find_pr`]). Through the name the branch was pushed under, only when the
/// branch's tip is the merged PR's head: the branch then holds nothing that was not merged.
async fn pr_is_merged(d: &Daemon, git: &Git, worktree: &WorktreeRow) -> bool {
    let Some((pr, found)) =
        crate::prs::find_pr(&d.store, git, &d.shell_env, worktree, "merged", "number").await
    else {
        return false;
    };
    match found {
        crate::prs::Found::LocalName => true,
        crate::prs::Found::Upstream => {
            let tip = git
                .rev_parse(
                    Path::new(&worktree.clone_path),
                    &format!("refs/heads/{}", worktree.branch),
                )
                .await;
            matches!((tip, pr["headRefOid"].as_str()), (Ok(tip), Some(head)) if tip.trim() == head)
        }
    }
}

/// Where a design chat of the workspace keeps its files.
fn design_folder(workspace_path: &str, chat_id: &str) -> String {
    Path::new(workspace_path)
        .join(NOTES_DIR)
        .join(crate::paths::DESIGNS_DIR)
        .join(chat_id)
        .to_string_lossy()
        .into_owned()
}

/// The first prompt, with the design picked at creation named after it; alone, it asks the
/// agent to read the design and wait.
fn with_design(prompt: Option<&str>, title: &str, folder: &str) -> String {
    let design = format!(
        "The design “{title}” comes with this work, in {folder}/ (`canvas.json` lists its pages, \
         sections and screens, one HTML file each; `comments.json` holds its review comments)."
    );
    match prompt.map(str::trim).filter(|p| !p.is_empty()) {
        Some(prompt) => format!("{prompt}\n\n{design} Read it before you start."),
        None => format!("{design} Read it, then wait for my instructions."),
    }
}

/// The first chat a creation opens: on a first message, images, or an explicit design.
fn first_chat_params(
    workspace_id: &str,
    params: &CreateWorkspaceParams,
) -> Option<CreateChatParams> {
    let prompt = params.prompt.clone().filter(|p| !p.trim().is_empty());
    let has_images = params.images.as_ref().is_some_and(|i| !i.is_empty());
    (prompt.is_some() || has_images || params.chat_kind == Some(ChatKind::Design)).then(|| {
        CreateChatParams {
            workspace: workspace_id.to_string(),
            kind: params.chat_kind,
            agent: params.agent,
            model: params.model.clone(),
            effort: params.effort.clone(),
            fast: params.fast,
            images: params.images.clone(),
            permission_mode: params.permission_mode,
            prompt,
            ..Default::default()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(identifier: &str) -> TicketRef {
        TicketRef {
            identifier: identifier.into(),
            title: Some(format!("{identifier} title")),
            url: None,
        }
    }

    #[test]
    fn a_new_design_opens_its_canvas_without_a_first_prompt() {
        let params = CreateWorkspaceParams {
            chat_kind: Some(ChatKind::Design),
            ..Default::default()
        };
        let chat = first_chat_params("workspace", &params).expect("a blank design opens a chat");
        assert_eq!(chat.kind, Some(ChatKind::Design));
        assert_eq!(chat.workspace, "workspace");
        assert!(chat.prompt.is_none());
        assert!(first_chat_params("workspace", &CreateWorkspaceParams::default()).is_none());
    }

    #[test]
    fn a_design_first_prompt_and_model_stay_in_the_design_chat() {
        let params = CreateWorkspaceParams {
            chat_kind: Some(ChatKind::Design),
            prompt: Some("Design a checkout".into()),
            agent: Some(chauffeur_protocol::model::AgentKind::Claude),
            model: Some("design-model".into()),
            ..Default::default()
        };
        let chat = first_chat_params("workspace", &params).unwrap();
        assert_eq!(chat.kind, Some(ChatKind::Design));
        assert_eq!(chat.prompt, params.prompt);
        assert_eq!(chat.model, params.model);
        assert_eq!(chat.agent, params.agent);
    }

    #[test]
    fn a_picked_design_follows_the_first_prompt_or_stands_alone() {
        let with = with_design(Some("Build it  "), "Checkout", "/w/.chauffeur/designs/c1");
        assert!(with.starts_with(
            "Build it\n\nThe design “Checkout” comes with this work, in /w/.chauffeur/designs/c1/"
        ));
        assert!(with.ends_with("Read it before you start."));
        let alone = with_design(Some("  "), "Checkout", "/w/.chauffeur/designs/c1");
        assert!(alone.starts_with("The design “Checkout”"));
        assert!(alone.ends_with("Read it, then wait for my instructions."));
    }

    #[test]
    fn the_picked_ticket_names_the_workspace_even_with_its_sub_issues() {
        // XYZ-1578 picked; its sub-issues follow it.
        let tickets = [ticket("XYZ-1578"), ticket("XYZ-1579"), ticket("XYZ-1580")];
        assert_eq!(naming_ticket(1, &tickets).unwrap().identifier, "XYZ-1578");
        // Several tickets picked, or only a milestone's: no single name.
        assert!(naming_ticket(2, &tickets).is_none());
        assert!(naming_ticket(0, &tickets).is_none());
        assert!(naming_ticket(1, &[]).is_none());
    }
}
