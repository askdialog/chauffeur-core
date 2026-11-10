//! A moved workspace's events, from the machine it runs on (docs/workspace-relay.md).
//!
//! - A subscription to a moved workspace or its chats is opened there with the same filter; its
//!   events come back as that daemon numbered them, with this daemon's own transfer progress of
//!   the workspace. When the connection drops, the subscription says so (`WorkspaceEvent::Relay`),
//!   opens again, and goes on after the last event it passed.
//! - [`spawn_watch`] follows the registry of each machine a workspace moved to: its chats' upserts
//!   and removals are published here, and the workspace shows the activity, unread mark and
//!   running tasks it has there.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chauffeur_protocol::client::{
    ChatListParams, ChatsResult, ClientCall, DaemonNotification, EventNotification, Replay,
    SubscribeParams, SubscribeResult, WorkspaceParams,
};
use chauffeur_protocol::events::{Event, EventEnvelope, RegistryEvent, WorkspaceEvent};
use chauffeur_protocol::model::WorkspaceInfo;
use chauffeur_protocol::rpc::{Notification, Response};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::Daemon;
use crate::relay::{Machine, RemoteState, machine_of};

/// The longest wait between two attempts to reach a machine again.
const MOST_BACKOFF: Duration = Duration::from_secs(30);
/// How often the machines to follow are looked for again.
const WATCH_EVERY: Duration = Duration::from_secs(5);

/// Relays the subscription `params` to `machine`, answering request `request` under the local
/// id `subscription`: an error when the machine does not answer at first, else that daemon's
/// `last_seq`.
pub fn spawn_subscription(
    daemon: Arc<Daemon>,
    (machine, workspaces): (Machine, Vec<String>),
    subscription: String,
    params: SubscribeParams,
    request: Value,
    tx: mpsc::UnboundedSender<String>,
) -> JoinHandle<()> {
    let mut local = daemon.events.subscribe();
    tokio::spawn(async move {
        let notify = |envelope: EventEnvelope| {
            send(
                &tx,
                &Notification::new(DaemonNotification::Event(EventNotification {
                    subscription: subscription.clone(),
                    envelope,
                })),
            )
        };
        // This daemon drives the workspace's transfers: their progress is its own.
        let own = |envelope: &EventEnvelope| {
            params.ephemeral
                && matches!(
                    &envelope.event,
                    Event::Workspace { workspace_id, event: WorkspaceEvent::Transfer { .. } }
                        if workspaces.contains(workspace_id)
                )
        };
        let relay_status = |connected: bool, error: Option<String>| {
            if !params.ephemeral {
                return;
            }
            for workspace in &workspaces {
                notify(EventEnvelope {
                    seq: None,
                    ts: crate::store::now(),
                    event: Event::Workspace {
                        workspace_id: workspace.clone(),
                        event: WorkspaceEvent::Relay {
                            machine: machine.name.clone(),
                            connected,
                            error: error.clone(),
                        },
                    },
                });
            }
        };
        let still_there = || {
            workspaces
                .iter()
                .all(|w| machine_of(&daemon, w).as_ref() == Some(&machine))
        };
        let mut after = params.after_seq;
        let mut replay = params.replay;
        let mut replayed_workspaces = HashSet::new();
        let mut answered = false;
        // Said once when it goes down, once when it is back.
        let mut down = false;
        let mut backoff = Duration::from_secs(1);
        loop {
            let opened = daemon
                .relay
                .subscribe(
                    &machine,
                    SubscribeParams {
                        after_seq: after,
                        replay,
                        ..params.clone()
                    },
                )
                .await;
            let cause = match opened {
                Ok(mut remote) => {
                    if answered {
                        if down {
                            relay_status(true, None);
                            down = false;
                        }
                    } else {
                        send(
                            &tx,
                            &Response::ok(
                                request.clone(),
                                serde_json::to_value(SubscribeResult {
                                    subscription: subscription.clone(),
                                    last_seq: remote.last_seq,
                                })
                                .unwrap_or_default(),
                            ),
                        );
                        answered = true;
                    }
                    // Opened again, it goes on after what it passed, live only as asked.
                    after = Some(after.unwrap_or(remote.last_seq));
                    backoff = Duration::from_secs(1);
                    let mut check = tokio::time::interval(WATCH_EVERY);
                    loop {
                        tokio::select! {
                            next = remote.events.recv() => match next {
                                Some(envelope) => {
                                    if let Event::Workspace { workspace_id, event: WorkspaceEvent::Replayed } = &envelope.event {
                                        replayed_workspaces.insert(workspace_id.clone());
                                    }
                                    let replayed = workspaces.iter().all(|id| replayed_workspaces.contains(id));
                                    if let Some(seq) = envelope.seq {
                                        after = Some(after.unwrap_or(0).max(seq));
                                    }
                                    notify(envelope);
                                    if replayed {
                                        if matches!(replay, Some(Replay::BeforeTail { .. })) { return; }
                                        // Reconnection after a tail must include every missed event.
                                        replay = None;
                                    }
                                }
                                None => break,
                            },
                            next = local.recv() => match next {
                                Ok(envelope) if own(&envelope) => notify(Arc::unwrap_or_clone(envelope)),
                                Err(broadcast::error::RecvError::Closed) => return,
                                _ => {}
                            },
                            // Brought back here (or moved on): that machine's events are not its.
                            _ = check.tick() => if !still_there() { return },
                            _ = tx.closed() => return,
                        }
                    }
                    format!("{} does not answer (the connection closed)", machine.name)
                }
                // Refused there (an unknown workspace): said as it is.
                Err(error) if !answered && !matches!(error, crate::Error::Unavailable(_)) => {
                    send(&tx, &Response::err(request.clone(), error.to_rpc()));
                    return;
                }
                // Not reachable: subscribed all the same, it says so and follows once it is back.
                Err(error) => {
                    if !answered {
                        send(
                            &tx,
                            &Response::ok(
                                request.clone(),
                                serde_json::to_value(SubscribeResult {
                                    subscription: subscription.clone(),
                                    last_seq: after.unwrap_or(0),
                                })
                                .unwrap_or_default(),
                            ),
                        );
                        answered = true;
                    }
                    error.to_string()
                }
            };
            if !down {
                relay_status(false, Some(cause));
                down = true;
            }
            // Brought back here meanwhile: nothing is relayed any more.
            loop {
                let wait = tokio::time::sleep(backoff);
                tokio::pin!(wait);
                tokio::select! {
                    _ = &mut wait => break,
                    next = local.recv() => match next {
                        Ok(envelope) if own(&envelope) => notify(Arc::unwrap_or_clone(envelope)),
                        Err(broadcast::error::RecvError::Closed) => return,
                        _ => continue,
                    },
                    _ = tx.closed() => return,
                }
            }
            backoff = (backoff * 2).min(MOST_BACKOFF);
            if !still_there() {
                return;
            }
        }
    })
}

/// Follows the registry of every machine a workspace moved to, for as long as one runs there.
pub fn spawn_watch(daemon: Arc<Daemon>) {
    tokio::spawn(async move {
        let mut watching: HashMap<std::path::PathBuf, (Machine, JoinHandle<()>)> = HashMap::new();
        loop {
            let moved = moved_workspaces(&daemon);
            let machines: HashSet<&Machine> = moved.values().collect();
            watching.retain(|_, (machine, task)| {
                let keep = machines.contains(machine) && !task.is_finished();
                if !keep {
                    task.abort();
                }
                keep
            });
            for machine in machines {
                watching
                    .entry(machine.socket.clone())
                    .or_insert_with(|| (machine.clone(), spawn_machine(&daemon, machine.clone())));
            }
            // Back here: its own chats show again.
            for workspace in daemon.relay.remote_workspaces() {
                if !moved.contains_key(&workspace)
                    && daemon.relay.set_remote_state(&workspace, None)
                {
                    daemon.workspaces.publish(&workspace);
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(WATCH_EVERY) => {}
                _ = daemon.shutdown.cancelled() => return,
            }
        }
    });
}

/// The workspaces that run on a machine of `[transfer]`, with it.
pub(crate) fn moved_workspaces(daemon: &Daemon) -> HashMap<String, Machine> {
    daemon
        .store
        .list_workspaces()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| Some((row.id.clone(), machine_of(daemon, &row.id)?)))
        .collect()
}

fn spawn_machine(daemon: &Arc<Daemon>, machine: Machine) -> JoinHandle<()> {
    let daemon = daemon.clone();
    tokio::spawn(async move {
        let mut after = None;
        let mut backoff = Duration::from_secs(1);
        loop {
            match follow(&daemon, &machine, &mut after).await {
                Ok(()) => backoff = Duration::from_secs(1),
                Err(error) => {
                    tracing::debug!(machine = %machine.name, %error, "registry not followed")
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MOST_BACKOFF);
        }
    })
}

/// Reads what `machine` has of its moved workspaces, then follows its registry until the
/// connection closes.
async fn follow(daemon: &Daemon, machine: &Machine, after: &mut Option<i64>) -> crate::Result<()> {
    let mut remote = daemon
        .relay
        .subscribe(
            machine,
            SubscribeParams {
                registry: true,
                after_seq: *after,
                ..Default::default()
            },
        )
        .await?;
    *after = Some(after.unwrap_or(remote.last_seq));
    for (workspace, on) in moved_workspaces(daemon) {
        if &on != machine {
            continue;
        }
        let call = ClientCall::WorkspaceGet(WorkspaceParams {
            workspace: workspace.clone(),
        });
        if let Ok(info) = daemon.relay.call(machine, call).await
            && let Ok(info) = serde_json::from_value::<WorkspaceInfo>(info)
        {
            mirror_workspace(daemon, machine, &info);
        }
        mirror_chats(daemon, machine, &workspace).await;
    }
    while let Some(envelope) = remote.events.recv().await {
        if let Some(seq) = envelope.seq {
            *after = Some(after.unwrap_or(0).max(seq));
        }
        let Event::Registry { event } = envelope.event else {
            continue;
        };
        let runs_there = |workspace: &str| machine_of(daemon, workspace).as_ref() == Some(machine);
        match event {
            RegistryEvent::WorkspaceUpserted { workspace } if runs_there(&workspace.id) => {
                mirror_workspace(daemon, machine, &workspace);
            }
            RegistryEvent::ChatUpserted { chat } if runs_there(&chat.workspace_id) => {
                daemon.relay.learn_chat(&chat.id, &chat.workspace_id);
                daemon.idle_reminders.chat(daemon, &chat);
                daemon.events.registry(RegistryEvent::ChatUpserted { chat });
            }
            RegistryEvent::ChatRemoved {
                workspace_id,
                chat_id,
            } if runs_there(&workspace_id) => {
                daemon.events.registry(RegistryEvent::ChatRemoved {
                    workspace_id,
                    chat_id,
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// The chats a workspace has on `machine` now, published here: those created there while this
/// daemon did not follow it come, and this machine's copies of those deleted there go.
async fn mirror_chats(daemon: &Daemon, machine: &Machine, workspace: &str) {
    let call = ClientCall::ChatList(ChatListParams {
        workspace: workspace.to_owned(),
        all: true,
    });
    let Ok(listed) = daemon.relay.call(machine, call).await else {
        return;
    };
    let Ok(listed) = serde_json::from_value::<ChatsResult>(listed) else {
        return;
    };
    let there: HashSet<String> = listed.chats.iter().map(|c| c.id.clone()).collect();
    for chat in daemon.store.chats_of(workspace).unwrap_or_default() {
        if !there.contains(&chat.id) {
            daemon.events.registry(RegistryEvent::ChatRemoved {
                workspace_id: workspace.to_owned(),
                chat_id: chat.id,
            });
        }
    }
    for chat in listed.chats {
        daemon.relay.learn_chat(&chat.id, workspace);
        daemon.idle_reminders.chat(daemon, &chat);
        daemon.events.registry(RegistryEvent::ChatUpserted { chat });
    }
}

/// Shows here what `machine` says of a workspace that runs there.
fn mirror_workspace(daemon: &Daemon, machine: &Machine, info: &WorkspaceInfo) {
    if machine_of(daemon, &info.id).as_ref() != Some(machine) {
        return;
    }
    let state = RemoteState {
        activity: info.activity,
        unread: info.unread,
        running_tasks: info.running_tasks,
    };
    let backups = info
        .backup
        .as_ref()
        .is_some_and(|backup| crate::cloud_workspace_backup::mirror(daemon, &info.id, backup));
    if daemon.relay.set_remote_state(&info.id, Some(state)) || backups {
        daemon.workspaces.publish(&info.id);
    }
}

fn send<T: serde::Serialize>(tx: &mpsc::UnboundedSender<String>, message: &T) {
    if let Ok(text) = serde_json::to_string(message) {
        let _ = tx.send(text);
    }
}
