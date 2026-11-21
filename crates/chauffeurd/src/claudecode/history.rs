//! The sessions that left no live branch, imported: each a closed tab of its repo's history
//! exploration, its Claude Code session copied there under the same id (the sidecar's
//! `context.copy`, streamed whatever its size), its conversation read back from the copy.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use chauffeur_protocol::claudecode::{
    ClaudeCodeHistoryPlan, ClaudeCodeOutcome, ClaudeCodeSessionPlan,
};
use chauffeur_protocol::client::CreateWorkspaceParams;
use chauffeur_protocol::model::{AgentKind, WorkspaceState};
use chauffeur_protocol::sidecar::{
    AgentSessionMessagesParams, AgentSessionMessagesResult, ContextCopyParams,
    ContextRestoreResult, SidecarCall,
};

use super::sessions::Session;
use crate::store::WorkspaceRow;
use crate::{Daemon, Error, Result};

/// `<prefix><repo>` → the id of the history exploration an import made for the repo.
const EXPLORATION: &str = "claudecode_history:";

/// `<prefix><session id>` → the id of the chat it was imported as: never imported twice.
const IMPORTED: &str = "claudecode_import:";

/// The history exploration an earlier import made for `repo`, while it is ready: one archived
/// since gets a new one.
pub(super) fn exploration(d: &Daemon, repo: &str) -> Result<Option<WorkspaceRow>> {
    let Some(id) = d.store.kv_get(&format!("{EXPLORATION}{repo}"))? else {
        return Ok(None);
    };
    Ok(d.store
        .find_workspace(&id)?
        .filter(|row| row.state == WorkspaceState::Ready))
}

/// The chat an earlier import made of `session`, by id.
pub(super) fn imported(d: &Daemon, session: &str) -> Result<Option<String>> {
    d.store.kv_get(&format!("{IMPORTED}{session}"))
}

/// The workspace holding `chat`, by name, while both exist.
pub(super) fn workspace_of(d: &Daemon, chat: &str) -> Result<Option<String>> {
    let Some(chat) = d.store.chat(chat)? else {
        return Ok(None);
    };
    Ok(d.store
        .find_workspace(&chat.workspace_id)?
        .map(|row| row.slug))
}

/// Imports each history's sessions into its exploration, made when the repo has none ready, and
/// says in `plans` how each went: a session that fails is said, the others go on.
pub(super) async fn import(
    d: &Arc<Daemon>,
    claude: &Path,
    histories: &mut [ClaudeCodeHistoryPlan],
    plans: &mut [ClaudeCodeSessionPlan],
    sessions: &HashMap<String, Session>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let index: BTreeMap<String, usize> = plans
        .iter()
        .enumerate()
        .map(|(i, plan)| (plan.id.clone(), i))
        .collect();
    for history in histories.iter_mut() {
        let workspace = match exploration(d, &history.repo)? {
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
            let (Some(&i), Some(session)) = (index.get(id), sessions.get(id)) else {
                continue;
            };
            let plan = &mut plans[i];
            plan.lands_in = Some(workspace.slug.clone());
            match import_session(d, claude, &workspace, session).await {
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
) -> Result<bool> {
    let record = format!("{IMPORTED}{}", session.id);
    if d.store.kv_get(&record)?.is_some() {
        return Ok(false);
    }
    // Claude keeps a folder's sessions under its name as the file system gives it.
    let cwd = Path::new(&workspace.path)
        .canonicalize()
        .with_context(|| format!("{} is missing", workspace.path))?
        .to_string_lossy()
        .into_owned();
    let homes = crate::cloud_workspace_restore::native_homes(d, Vec::new())
        .context("no Claude folder for the exploration's agents")?;
    let added: ContextRestoreResult = serde_json::from_value(
        d.chats
            .sidecar
            .call(SidecarCall::ContextCopy(ContextCopyParams {
                session_id: session.id.clone(),
                source_home: claude.to_string_lossy().into_owned(),
                project_key: session.project.clone(),
                claude_home: homes.claude_home.clone(),
                cwd: cwd.clone(),
            }))
            .await?,
    )
    .context("unexpected answer from the sidecar to a copy")?;
    let opened = open_copy(d, workspace, session, cwd, &record).await;
    if opened.is_err() {
        // Not imported: the copy goes, the files it made only.
        let copy = crate::cloud_workspace_discard::NativeFiles::new(vec![homes.claude_home], added);
        if let Err(error) = copy.remove() {
            tracing::warn!(%error, session = session.id, "an import's copy stayed");
        }
    }
    opened
}

/// The copy of `session` in `cwd`'s project folder, opened as a closed tab of `workspace`.
async fn open_copy(
    d: &Arc<Daemon>,
    workspace: &WorkspaceRow,
    session: &Session,
    cwd: String,
    record: &str,
) -> Result<bool> {
    let sidecar = &d.chats.sidecar;
    let conversation: AgentSessionMessagesResult = serde_json::from_value(
        sidecar
            .call(SidecarCall::AgentSessionMessages(
                AgentSessionMessagesParams {
                    agent: AgentKind::Claude,
                    session_id: session.id.clone(),
                    cwd,
                },
            ))
            .await?,
    )
    .context("unexpected answer from the sidecar to a conversation's reading")?;
    if !conversation.found {
        return Err(Error::Internal(anyhow::anyhow!(
            "its copy could not be read back from {}",
            workspace.slug
        )));
    }
    let events: Vec<_> = conversation
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
            record,
        )
        .await?;
    match imported {
        Some(_) => Ok(true),
        None if d.store.kv_get(record)?.is_some() => Ok(false),
        None => Err(Error::not_found(format!("workspace {}", workspace.slug))),
    }
}
