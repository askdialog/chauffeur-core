//! The sessions that left no live branch, imported: each a closed tab of its repo's history
//! exploration, its Claude Code session copied there under the same id (the sidecar's context
//! capture and restore, as cloud transfers do), its conversation read back from the copy.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chauffeur_protocol::claudecode::{
    ClaudeCodeHistoryPlan, ClaudeCodeOutcome, ClaudeCodeSessionPlan,
};
use chauffeur_protocol::client::CreateWorkspaceParams;
use chauffeur_protocol::model::{AgentKind, WorkspaceState};
use chauffeur_protocol::sidecar::{
    AgentSessionMessagesParams, AgentSessionMessagesResult, ContextCaptureParams,
    ContextCaptureResult, ContextRestoreEntry, SidecarCall,
};

use super::sessions::Session;
use crate::store::WorkspaceRow;
use crate::{Daemon, Error, Result};

/// `<prefix><repo>` → the id of the history exploration an import made for the repo.
const EXPLORATION: &str = "claudecode_history:";

/// `<prefix><session id>` → the id of the chat it was imported as: never imported twice.
pub(super) const IMPORTED: &str = "claudecode_import:";

/// The history exploration an earlier import made for `repo`, while it is ready: one archived
/// since gets a new one.
pub(super) fn exploration(d: &Daemon, repo: &str) -> Option<WorkspaceRow> {
    let id = d.store.kv_get(&format!("{EXPLORATION}{repo}")).ok()??;
    d.workspaces
        .row(&id)
        .ok()
        .filter(|row| row.state == WorkspaceState::Ready)
}

/// The workspace a recorded session was imported into, by name; none when it is gone.
pub(super) fn imported_into(d: &Daemon, session: &str) -> Option<Option<String>> {
    let chat = d.store.kv_get(&format!("{IMPORTED}{session}")).ok()??;
    Some(
        d.store
            .chat(&chat)
            .ok()
            .flatten()
            .and_then(|chat| d.workspaces.row(&chat.workspace_id).ok())
            .map(|row| row.slug),
    )
}

/// Imports each history's sessions into its exploration, made when the repo has none ready, and
/// says in `plans` how each went: a session that fails is said, the others go on.
pub(super) async fn import(
    d: &Arc<Daemon>,
    claude: &Path,
    histories: &mut [ClaudeCodeHistoryPlan],
    plans: &mut [ClaudeCodeSessionPlan],
    read: &HashMap<String, Session>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let scratch = Scratch::new(d)?;
    let index: BTreeMap<String, usize> = plans
        .iter()
        .enumerate()
        .map(|(i, plan)| (plan.id.clone(), i))
        .collect();
    for history in histories.iter_mut() {
        let workspace = match exploration(d, &history.repo) {
            Some(row) => row,
            None => match make_exploration(d, &history.repo).await {
                Ok(row) => row,
                Err(error) => {
                    let reason = format!("its history exploration could not be made: {error}");
                    for id in &history.sessions {
                        if let Some(&i) = index.get(id) {
                            fail(&mut plans[i], &reason);
                        }
                    }
                    warnings.push(format!(
                        "The history of {} could not be imported: {error}",
                        history.repo
                    ));
                    continue;
                }
            },
        };
        history.workspace = Some(workspace.slug.clone());
        for id in &history.sessions {
            let (Some(&i), Some(session)) = (index.get(id), read.get(id)) else {
                continue;
            };
            let plan = &mut plans[i];
            plan.lands_in = Some(workspace.slug.clone());
            match import_session(d, claude, &workspace, session, &scratch.0).await {
                Ok(true) => plan.outcome = ClaudeCodeOutcome::Imported,
                Ok(false) => plan.outcome = ClaudeCodeOutcome::AlreadyImported,
                Err(error) => {
                    warnings.push(format!("“{}” could not be imported: {error}", plan.title));
                    fail(plan, &error.to_string());
                }
            }
        }
    }
    Ok(())
}

fn fail(plan: &mut ClaudeCodeSessionPlan, reason: &str) {
    plan.outcome = ClaudeCodeOutcome::Failed;
    plan.reason = Some(reason.to_owned());
}

/// A new exploration of `repo`, at its base branch, recorded as its history.
async fn make_exploration(d: &Arc<Daemon>, repo: &str) -> Result<WorkspaceRow> {
    let made = d
        .workspaces
        .create(CreateWorkspaceParams {
            repos: vec![repo.to_owned()],
            name: Some(format!("{repo} history")),
            explore: true,
            ..Default::default()
        })
        .await?;
    d.store
        .kv_set(&format!("{EXPLORATION}{repo}"), &made.workspace.id)?;
    d.workspaces.row(&made.workspace.id)
}

/// Copies `session` into `workspace`'s Claude project folder and opens it there as a closed tab;
/// false when an earlier import has it.
async fn import_session(
    d: &Arc<Daemon>,
    claude: &Path,
    workspace: &WorkspaceRow,
    session: &Session,
    scratch: &Path,
) -> Result<bool> {
    let record = format!("{IMPORTED}{}", session.id);
    if d.store.kv_get(&record)?.is_some() {
        return Ok(false);
    }
    let output = scratch.join(format!("{}.json", session.id));
    let sidecar = &d.chats.sidecar;
    let captured = sidecar
        .call(SidecarCall::ContextCapture(ContextCaptureParams {
            chat_id: format!("claude-code-import-{}", session.id),
            agent: AgentKind::Claude,
            session_id: session.id.clone(),
            cwd: session
                .folder
                .as_deref()
                .unwrap_or(claude)
                .to_string_lossy()
                .into_owned(),
            env: BTreeMap::from([(
                "CLAUDE_CONFIG_DIR".to_owned(),
                claude.to_string_lossy().into_owned(),
            )]),
            output: output.to_string_lossy().into_owned(),
            project_key: Some(session.project.clone()),
            live: false,
        }))
        .await?;
    let (sha256, bytes) = match serde_json::from_value(captured)? {
        ContextCaptureResult::Captured { sha256, bytes } => (sha256, bytes),
        ContextCaptureResult::Blocked { reason } => return Err(Error::BadRequest(reason)),
    };
    let homes = crate::cloud_workspace_restore::native_homes(
        d,
        vec![ContextRestoreEntry {
            agent: AgentKind::Claude,
            session_id: session.id.clone(),
            cwd: workspace.path.clone(),
            path: output.to_string_lossy().into_owned(),
            sha256,
            bytes,
        }],
    )
    .map_err(Error::Internal)?;
    sidecar.call(SidecarCall::ContextRestore(homes)).await?;
    let read: AgentSessionMessagesResult = serde_json::from_value(
        sidecar
            .call(SidecarCall::AgentSessionMessages(
                AgentSessionMessagesParams {
                    agent: AgentKind::Claude,
                    session_id: session.id.clone(),
                    cwd: workspace.path.clone(),
                },
            ))
            .await?,
    )?;
    let events: Vec<_> = read
        .events
        .into_iter()
        .map(|e| (e.at.unwrap_or_else(|| session.last_active.clone()), e.event))
        .collect();
    let imported = d
        .chats
        .import_closed(
            workspace,
            &session.title,
            &session.id,
            &session.last_active,
            &events,
            &record,
        )
        .await?;
    match imported {
        Some(_) => Ok(true),
        None if d.store.kv_get(&record)?.is_some() => Ok(false),
        None => Err(Error::Conflict(format!("{} is gone", workspace.slug))),
    }
}

/// A private folder of the daemon's for the captures, removed with what it holds.
struct Scratch(PathBuf);

impl Scratch {
    fn new(d: &Daemon) -> Result<Scratch> {
        // The sidecar writes only into a folder named as the file system names it.
        let dir = d
            .paths
            .home
            .canonicalize()?
            .join(format!("claude-code-import-{}", uuid::Uuid::now_v7()));
        std::os::unix::fs::DirBuilderExt::mode(&mut std::fs::DirBuilder::new(), 0o700)
            .create(&dir)?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
