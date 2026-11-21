//! The sessions that left no live branch, imported: each a closed tab of its repo's history
//! exploration (`copy`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use chauffeur_protocol::claudecode::ClaudeCodeHistoryPlan;
use chauffeur_protocol::client::CreateWorkspaceParams;
use chauffeur_protocol::model::WorkspaceState;

use super::copy::Report;
use super::sessions::Session;
use crate::store::WorkspaceRow;
use crate::{Daemon, Result};

/// `<prefix><repo>` → the id of the history exploration an import made for the repo.
const EXPLORATION: &str = "claudecode_history:";

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
        report
            .import(d, claude, &workspace, &history.sessions, sessions, true)
            .await;
    }
    Ok(())
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
