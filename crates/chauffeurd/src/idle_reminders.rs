//! Reminders about workspaces left idle on another machine (docs/cloud-workspaces.md): a moved
//! workspace keeps its machine running, and paid for, until it comes back. After an hour without
//! an agent turn or the user acting on it from here, one reminder groups every workspace that
//! reached the hour at the same check (`RegistryEvent::IdleReminder`), and each shows idle
//! (`WorkspaceInfo.idle_since`) until something happens there or the user asks to be reminded
//! in an hour. An ignored reminder is not repeated. A dev server left running is not activity.
//!
//! It never stops, deletes or brings back anything. What it knows lives in memory: a daemon that
//! starts again counts the hour from its start.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chauffeur_protocol::events::RegistryEvent;
use chauffeur_protocol::model::{Activity, ChatInfo, ChatStatus};

use crate::Daemon;

/// What each moved workspace did last, as this daemon saw it.
#[derive(Default)]
pub struct IdleReminders {
    tracker: Mutex<Tracker>,
}

#[derive(Default)]
struct Tracker {
    workspaces: HashMap<String, Tracked>,
    /// The last status and stored event seen of each of their chats: a change is an agent's turn
    /// starting or ending, or something written in the chat (a message, a one-off command).
    chats: HashMap<String, (ChatStatus, Option<String>)>,
}

struct Tracked {
    active_at: Instant,
    /// `active_at` as the user reads it.
    since: String,
    /// Reminded since it was last active: it shows idle.
    reminded: bool,
    /// Asked to be reminded again at this time.
    snoozed_until: Option<Instant>,
}

impl Tracked {
    fn new(now: Instant) -> Tracked {
        Tracked {
            active_at: now,
            since: crate::store::now(),
            reminded: false,
            snoozed_until: None,
        }
    }
}

impl Tracker {
    /// True when the workspace showed idle and no longer does.
    fn active(&mut self, workspace: &str, now: Instant) -> bool {
        let tracked = self
            .workspaces
            .entry(workspace.to_owned())
            .or_insert_with(|| Tracked::new(now));
        let was_idle = tracked.reminded;
        *tracked = Tracked::new(now);
        was_idle
    }

    /// A chat as its machine says it is now: activity while it works, or when its status or last
    /// stored event changed.
    fn chat(&mut self, chat: &ChatInfo, now: Instant) -> bool {
        let seen = (chat.status, chat.last_activity_at.clone());
        let before = self.chats.insert(chat.id.clone(), seen.clone());
        let working = matches!(chat.status, ChatStatus::Starting | ChatStatus::Working);
        let changed = before.is_some_and(|before| before != seen);
        (working || changed) && self.active(&chat.workspace_id, now)
    }

    /// The workspaces to remind of now: `moved` are those that run elsewhere, with whether an
    /// agent works there. The others are forgotten.
    fn due(&mut self, moved: &HashMap<String, bool>, after: Duration, now: Instant) -> Vec<String> {
        self.workspaces.retain(|id, _| moved.contains_key(id));
        let mut due = Vec::new();
        for (id, working) in moved {
            let tracked = self
                .workspaces
                .entry(id.clone())
                .or_insert_with(|| Tracked::new(now));
            if *working {
                *tracked = Tracked::new(now);
                continue;
            }
            let snoozed = tracked.snoozed_until.is_some_and(|until| now < until);
            if tracked.reminded || snoozed || now.duration_since(tracked.active_at) < after {
                continue;
            }
            tracked.reminded = true;
            tracked.snoozed_until = None;
            due.push(id.clone());
        }
        due.sort();
        due
    }

    /// Reminds of `workspace` again `after` from now, unless something happens there; true when
    /// it showed idle.
    fn snooze(&mut self, workspace: &str, after: Duration, now: Instant) -> bool {
        let tracked = self
            .workspaces
            .entry(workspace.to_owned())
            .or_insert_with(|| Tracked::new(now));
        let was_idle = tracked.reminded;
        tracked.reminded = false;
        tracked.snoozed_until = Some(now + after);
        was_idle
    }

    fn idle_since(&self, workspace: &str) -> Option<String> {
        self.workspaces
            .get(workspace)
            .filter(|t| t.reminded)
            .map(|t| t.since.clone())
    }
}

impl IdleReminders {
    /// The user acted on `workspace` from here (a relayed call), or its agents did there.
    pub fn active(&self, daemon: &Daemon, workspace: &str) {
        if self
            .tracker
            .lock()
            .unwrap()
            .active(workspace, Instant::now())
        {
            daemon.workspaces.publish(workspace);
        }
    }

    /// A chat of a moved workspace, as its machine published it.
    pub fn chat(&self, daemon: &Daemon, chat: &ChatInfo) {
        if self.tracker.lock().unwrap().chat(chat, Instant::now()) {
            daemon.workspaces.publish(&chat.workspace_id);
        }
    }

    /// Reminds of `workspace` again in an hour, unless something happens there meanwhile.
    pub fn snooze(&self, daemon: &Daemon, workspace: &str) {
        let after = remind_after();
        if self
            .tracker
            .lock()
            .unwrap()
            .snooze(workspace, after, Instant::now())
        {
            daemon.workspaces.publish(workspace);
        }
    }

    /// Since when `workspace` is idle, once it was reminded of.
    pub fn idle_since(&self, workspace: &str) -> Option<String> {
        self.tracker.lock().unwrap().idle_since(workspace)
    }
}

/// How long a moved workspace stays idle before a reminder, and between a "remind me later" and
/// the next one: an hour. `CHAUFFEUR_IDLE_REMINDER_SECS` shortens it in tests.
fn remind_after() -> Duration {
    let secs = std::env::var("CHAUFFEUR_IDLE_REMINDER_SECS")
        .ok()
        .and_then(|secs| secs.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(3600);
    Duration::from_secs(secs)
}

/// Runs until the daemon stops.
pub fn spawn(daemon: Arc<Daemon>) {
    tokio::spawn(async move {
        let after = remind_after();
        let tick = (after / 4).clamp(Duration::from_secs(1), Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = daemon.shutdown.cancelled() => return,
                _ = tokio::time::sleep(tick) => {}
            }
            let moved: HashMap<String, bool> = crate::relay_events::moved_workspaces(&daemon)
                .into_keys()
                .map(|id| {
                    let working = daemon
                        .relay
                        .remote_state(&id)
                        .is_some_and(|state| state.activity == Activity::Working);
                    (id, working)
                })
                .collect();
            let due =
                daemon
                    .idle_reminders
                    .tracker
                    .lock()
                    .unwrap()
                    .due(&moved, after, Instant::now());
            if due.is_empty() {
                continue;
            }
            for workspace in &due {
                daemon.workspaces.publish(workspace);
            }
            tracing::info!(count = due.len(), "moved workspaces idle: reminder sent");
            daemon
                .events
                .registry(RegistryEvent::IdleReminder { workspace_ids: due });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);

    fn moved(entries: &[(&str, bool)]) -> HashMap<String, bool> {
        entries.iter().map(|(id, w)| (id.to_string(), *w)).collect()
    }

    #[test]
    fn a_moved_workspace_is_reminded_of_once_after_an_hour_without_activity() {
        let mut tracker = Tracker::default();
        let start = Instant::now();
        let away = moved(&[("w1", false)]);
        assert!(tracker.due(&away, HOUR, start).is_empty());
        assert!(tracker.due(&away, HOUR, start + HOUR / 2).is_empty());
        assert_eq!(tracker.due(&away, HOUR, start + HOUR), vec!["w1"]);
        assert!(tracker.idle_since("w1").is_some());
        // Ignored: not repeated, it shows idle.
        assert!(tracker.due(&away, HOUR, start + HOUR * 5).is_empty());
        assert!(tracker.idle_since("w1").is_some());
    }

    #[test]
    fn workspaces_reaching_the_hour_at_the_same_check_are_reminded_of_together() {
        let mut tracker = Tracker::default();
        let start = Instant::now();
        let away = moved(&[("w2", false), ("w1", false), ("w3", false)]);
        tracker.due(&away, HOUR, start);
        tracker.active("w3", start + HOUR / 6);
        assert_eq!(tracker.due(&away, HOUR, start + HOUR), vec!["w1", "w2"]);
        assert_eq!(
            tracker.due(&away, HOUR, start + HOUR + HOUR / 6),
            vec!["w3"]
        );
    }

    #[test]
    fn activity_clears_the_idle_mark_and_starts_the_hour_again() {
        let mut tracker = Tracker::default();
        let start = Instant::now();
        let away = moved(&[("w1", false)]);
        tracker.due(&away, HOUR, start);
        tracker.due(&away, HOUR, start + HOUR);
        assert!(tracker.active("w1", start + HOUR * 2));
        assert_eq!(tracker.idle_since("w1"), None);
        assert!(
            tracker
                .due(&away, HOUR, start + HOUR * 2 + HOUR / 2)
                .is_empty()
        );
        assert_eq!(tracker.due(&away, HOUR, start + HOUR * 3), vec!["w1"]);
    }

    #[test]
    fn a_snoozed_reminder_comes_back_an_hour_later_unless_something_happens() {
        let mut tracker = Tracker::default();
        let start = Instant::now();
        let away = moved(&[("w1", false), ("w2", false)]);
        tracker.due(&away, HOUR, start);
        assert_eq!(tracker.due(&away, HOUR, start + HOUR), vec!["w1", "w2"]);
        let snoozed_at = start + HOUR + HOUR / 4;
        assert!(tracker.snooze("w1", HOUR, snoozed_at));
        assert!(tracker.snooze("w2", HOUR, snoozed_at));
        assert_eq!(tracker.idle_since("w1"), None);
        assert!(tracker.due(&away, HOUR, snoozed_at + HOUR / 2).is_empty());
        // Something happened on w2 meanwhile: its hour starts again from then.
        tracker.active("w2", snoozed_at + HOUR / 2);
        assert_eq!(tracker.due(&away, HOUR, snoozed_at + HOUR), vec!["w1"]);
        assert_eq!(
            tracker.due(&away, HOUR, snoozed_at + HOUR + HOUR / 2),
            vec!["w2"]
        );
    }

    #[test]
    fn an_agent_at_work_there_keeps_the_workspace_active() {
        let mut tracker = Tracker::default();
        let start = Instant::now();
        tracker.due(&moved(&[("w1", false)]), HOUR, start);
        assert!(
            tracker
                .due(&moved(&[("w1", true)]), HOUR, start + HOUR * 3)
                .is_empty()
        );
        let idle = moved(&[("w1", false)]);
        assert!(
            tracker
                .due(&idle, HOUR, start + HOUR * 3 + HOUR / 2)
                .is_empty()
        );
        assert_eq!(tracker.due(&idle, HOUR, start + HOUR * 4), vec!["w1"]);
    }

    #[test]
    fn a_chat_turn_starting_or_ending_there_is_activity_but_an_idle_chat_is_not() {
        let mut tracker = Tracker::default();
        let start = Instant::now();
        let away = moved(&[("w1", false)]);
        tracker.due(&away, HOUR, start);
        let chat = |status| ChatInfo {
            status,
            ..test_chat()
        };
        // First seen idle: nothing happened.
        tracker.chat(&chat(ChatStatus::Idle), start + HOUR / 2);
        assert_eq!(tracker.due(&away, HOUR, start + HOUR), vec!["w1"]);
        // Its turn starts, then ends waiting for the user: each is activity.
        assert!(tracker.chat(&chat(ChatStatus::Working), start + HOUR * 2));
        tracker.chat(&chat(ChatStatus::NeedsInput), start + HOUR * 2 + HOUR / 2);
        assert!(tracker.due(&away, HOUR, start + HOUR * 3).is_empty());
        // The same status published again (a rename) is not.
        tracker.chat(&chat(ChatStatus::NeedsInput), start + HOUR * 3);
        assert_eq!(
            tracker.due(&away, HOUR, start + HOUR * 3 + HOUR / 2),
            vec!["w1"]
        );
        // Something written in it (a one-off command's output) is.
        let ran = ChatInfo {
            last_activity_at: Some("2026-10-26T12:00:00.000Z".into()),
            ..chat(ChatStatus::NeedsInput)
        };
        assert!(tracker.chat(&ran, start + HOUR * 4));
    }

    #[test]
    fn a_workspace_brought_back_is_forgotten() {
        let mut tracker = Tracker::default();
        let start = Instant::now();
        tracker.due(&moved(&[("w1", false)]), HOUR, start);
        tracker.due(&moved(&[("w1", false)]), HOUR, start + HOUR);
        assert!(tracker.due(&moved(&[]), HOUR, start + HOUR * 2).is_empty());
        assert_eq!(tracker.idle_since("w1"), None);
        // Moved again: its hour starts from then.
        assert!(
            tracker
                .due(&moved(&[("w1", false)]), HOUR, start + HOUR * 3)
                .is_empty()
        );
    }

    fn test_chat() -> ChatInfo {
        serde_json::from_value(serde_json::json!({
            "id": "c1",
            "workspace_id": "w1",
            "agent": "claude",
            "model": null,
            "effort": null,
            "permission_mode": "auto",
            "title": null,
            "status": "idle",
            "agent_session_id": null,
            "created_at": "2026-10-26T10:00:00.000Z",
        }))
        .expect("a minimal chat")
    }
}
