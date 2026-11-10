//! What a workspace left alone keeps running: after `idle_session_minutes` without activity, its
//! agents' sessions are closed (the next message resumes them with their conversation) and its Nx
//! daemons stopped when nothing else runs there (the next `nx` command starts one), freeing the
//! memory and CPU they hold.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chauffeur_protocol::model::WorkspaceState;

use crate::{Daemon, procs};

/// A workspace whose Nx daemons are kept because something else runs there is looked at again
/// after this; one already swept, after `after` again.
const BUSY_RETRY: Duration = Duration::from_secs(600);

/// Runs until the daemon stops.
pub fn spawn(daemon: Arc<Daemon>) {
    tokio::spawn(async move {
        // Workspace → when to look for its Nx daemons again, unless its chats do something.
        let mut next_sweep: HashMap<String, (Instant, Instant)> = HashMap::new();
        loop {
            let after = idle_after(&daemon);
            let tick = after.map_or(Duration::from_secs(60), |after| {
                (after / 4).clamp(Duration::from_secs(1), Duration::from_secs(60))
            });
            tokio::select! {
                _ = daemon.shutdown.cancelled() => return,
                _ = tokio::time::sleep(tick) => {}
            }
            if let Some(after) = after {
                daemon.chats.close_idle_sessions(after).await;
                sweep_nx_daemons(&daemon, after, &mut next_sweep).await;
                daemon.ollama.stop_if_unused(after).await;
            }
        }
    });
}

/// How long a chat stays idle before its session closes; none when the setting is 0.
/// `CHAUFFEUR_IDLE_SESSION_SECS` overrides it in tests.
fn idle_after(daemon: &Daemon) -> Option<Duration> {
    let secs = std::env::var("CHAUFFEUR_IDLE_SESSION_SECS")
        .ok()
        .and_then(|secs| secs.parse::<u64>().ok())
        .unwrap_or(daemon.settings.get().idle_session_minutes * 60);
    (secs > 0).then(|| Duration::from_secs(secs))
}

async fn sweep_nx_daemons(
    daemon: &Daemon,
    after: Duration,
    next_sweep: &mut HashMap<String, (Instant, Instant)>,
) {
    let now = Instant::now();
    let workspaces = daemon.store.list_workspaces().unwrap_or_default();
    let due: Vec<_> = workspaces
        .into_iter()
        .filter(|w| w.state == WorkspaceState::Ready)
        .filter(|w| daemon.chats.workspace_quiet(&w.id, after))
        .filter(|w| {
            // Swept since its chats last did something: wait for the next look.
            let quiet_since = daemon.chats.workspace_quiet_since(&w.id);
            !next_sweep
                .get(&w.id)
                .is_some_and(|(swept, next)| *swept >= quiet_since && now < *next)
        })
        .collect();
    if due.is_empty() {
        return;
    }
    let running = procs::listing().await;
    for workspace in due {
        let daemons = procs::idle_nx_daemons(&running, Path::new(&workspace.path));
        let busy = daemons.is_empty()
            && running.iter().any(|p| {
                p.cwd
                    .as_deref()
                    .is_some_and(|cwd| cwd.starts_with(&workspace.path))
            });
        for pid in &daemons {
            // SAFETY: sending a signal to a pid; a stale pid fails with ESRCH.
            unsafe {
                libc::kill(*pid, libc::SIGTERM);
            }
        }
        if !daemons.is_empty() {
            tracing::info!(workspace = %workspace.slug, count = daemons.len(), "idle Nx daemons stopped");
        }
        let next = now + if busy { BUSY_RETRY } else { after };
        next_sweep.insert(workspace.id.clone(), (now, next));
    }
}
