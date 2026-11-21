//! A Claude Code session imported into a workspace as a chat: copied there under the same id (the
//! sidecar's `context.copy`, streamed whatever its size), its conversation read back from the
//! copy, recorded so that no session is imported twice.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use chauffeur_protocol::model::AgentKind;
use chauffeur_protocol::sidecar::{
    AgentSessionMessagesParams, AgentSessionMessagesResult, ContextCopyParams,
    ContextRestoreResult, SidecarCall,
};

use super::sessions::Session;
use crate::store::WorkspaceRow;
use crate::{Daemon, Error, Result};

/// `<prefix><session id>` → the id of the chat it was imported as: never imported twice.
const IMPORTED: &str = "claudecode_import:";

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

/// Copies `session` into `workspace`'s Claude project folder and opens it there as a chat, a
/// closed tab with `closed`; false when an earlier import has it.
pub(super) async fn import_session(
    d: &Arc<Daemon>,
    claude: &Path,
    workspace: &WorkspaceRow,
    session: &Session,
    closed: bool,
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
        .context("no Claude folder for the workspace's agents")?;
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
    let opened = open_copy(d, workspace, session, cwd, &record, closed).await;
    if opened.is_err() {
        // Not imported: the copy goes, the files it made only.
        let copy = crate::cloud_workspace_discard::NativeFiles::new(vec![homes.claude_home], added);
        if let Err(error) = copy.remove() {
            tracing::warn!(%error, session = session.id, "an import's copy stayed");
        }
    }
    opened
}

/// The copy of `session` in `cwd`'s project folder, opened as a chat of `workspace`.
async fn open_copy(
    d: &Arc<Daemon>,
    workspace: &WorkspaceRow,
    session: &Session,
    cwd: String,
    record: &str,
    closed: bool,
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
        .import_chat(
            workspace,
            &session.title,
            &session.id,
            &session.last_active,
            &events,
            record,
            closed,
        )
        .await?;
    match imported {
        Some(_) => Ok(true),
        None if d.store.kv_get(record)?.is_some() => Ok(false),
        None => Err(Error::not_found(format!("workspace {}", workspace.slug))),
    }
}
