//! The sessions that left no live branch, imported into their repo's history exploration
//! (`copy`): the most recent of a new exploration an open tab, the others closed tabs.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use chauffeur_protocol::claudecode::ClaudeCodeHistoryPlan;
use chauffeur_protocol::client::CreateWorkspaceParams;

use super::copy::{self, Report};
use super::sessions::Session;
use crate::store::WorkspaceRow;
use crate::{Daemon, Result};

/// `<prefix><repo>` → the id of the history exploration an import made for the repo.
const EXPLORATION: &str = "claudecode_history:";

/// The history exploration an earlier import made for `repo`, while it is ready: one archived
/// since gets a new one.
pub(super) fn exploration(d: &Daemon, repo: &str) -> Result<Option<WorkspaceRow>> {
    super::ready_workspace(d, &format!("{EXPLORATION}{repo}"))
}

/// Imports each history's sessions into its exploration, made when the repo has none ready, and
/// says in `plans` how each went: a session that fails is said, the others go on.
pub(super) async fn import(
    d: &Arc<Daemon>,
    claude: &Path,
    histories: &mut [ClaudeCodeHistoryPlan],
    sessions: &HashMap<String, Session>,
    report: &mut Report<'_>,
) -> Result<()> {
    for history in histories.iter_mut() {
        let workspace = match exploration(d, &history.repo)? {
            Some(row) => row,
            None => match make_exploration(d, &history.repo).await {
                Ok(row) => row,
                Err(error) => {
                    let reason = format!("its history exploration could not be made: {error}");
                    report.fail(&history.sessions, &reason);
                    report.warnings.push(format!(
                        "The history of {} could not be imported: {error}",
                        history.repo
                    ));
                    continue;
                }
            },
        };
        history.workspace = Some(workspace.slug.clone());
        let opened = if d.store.chats_of(&workspace.id)?.is_empty() {
            latest(d, &history.sessions, sessions)?
        } else {
            None
        };
        let (open, closed): (Vec<String>, Vec<String>) = history
            .sessions
            .iter()
            .cloned()
            .partition(|id| opened.as_ref() == Some(id));
        // The open tab first: a workspace holding only closed tabs gets a blank one beside them.
        report
            .import(d, claude, &workspace, &open, sessions, false)
            .await;
        report
            .import(d, claude, &workspace, &closed, sessions, true)
            .await;
    }
    Ok(())
}

/// The most recently active of `ids` that no earlier import has.
fn latest(
    d: &Daemon,
    ids: &[String],
    sessions: &HashMap<String, Session>,
) -> Result<Option<String>> {
    let mut best: Option<&Session> = None;
    for session in ids.iter().filter_map(|id| sessions.get(id)) {
        if copy::imported(d, &session.id)?.is_none()
            && best.is_none_or(|best| session.last_active > best.last_active)
        {
            best = Some(session);
        }
    }
    Ok(best.map(|session| session.id.clone()))
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
