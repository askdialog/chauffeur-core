//! What is left of each agent's plan limits: the sidecar asks the agents' own CLIs (which keep
//! their login fresh), the daemon keeps a reading a few minutes and reads again after a turn.
//! The same reading tells whether each CLI is logged in; a chat whose agent refused its
//! credentials says it sooner. A signed-out agent is read again at every ask, so that a login
//! made anywhere (the app, a terminal) shows at once.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chauffeur_protocol::client::AgentUsage;
use chauffeur_protocol::events::RegistryEvent;
use chauffeur_protocol::model::{AccountStatus, AgentKind};
use chauffeur_protocol::sidecar::{
    AgentLoginCodeParams, AgentLoginEndedParams, AgentLoginResult, AgentLoginStartParams,
    AgentModelsParams, AgentUsageResult, SidecarCall,
};

use crate::{Daemon, Result};

/// How long a reading is shown before it is read again.
const FRESH: Duration = Duration::from_secs(300);
/// An agent that does not answer in this time is reported, not waited for.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const AGENTS: [AgentKind; 2] = [AgentKind::Claude, AgentKind::Codex];
/// A plan window used this far leaves no room for a delegated chat's task.
const SPENT_PERCENT: f64 = 95.0;

#[derive(Default)]
pub struct Usage {
    entries: Mutex<HashMap<AgentKind, Entry>>,
    /// One reading at a time: clients asking meanwhile get it too.
    reading: tokio::sync::Mutex<()>,
}

#[derive(Default, Clone)]
struct Entry {
    last: Option<(AgentUsageResult, String)>,
    error: Option<String>,
    /// When `error` was set, RFC 3339.
    failed_at: Option<String>,
    read: Option<Instant>,
    /// A turn ended since: the limits moved.
    stale: bool,
    account: AccountStatus,
    account_error: Option<String>,
}

impl Usage {
    /// An agent of this kind finished a turn: its limits are read again on the next ask.
    pub fn turn_ended(&self, agent: AgentKind) {
        self.entries.lock().unwrap().entry(agent).or_default().stale = true;
    }

    /// A chat's agent refused its credentials: signed out until a read or a login says otherwise.
    pub fn account_lost(&self, daemon: &Daemon, agent: AgentKind, message: String) {
        self.set_account(daemon, agent, AccountStatus::SignedOut, Some(message));
    }

    /// Why `agent` cannot take new work now, from the last reading (never read again here): it is
    /// signed out, or one of its plan's own windows (`5h`, `7d`, not a model's) is used up to
    /// `SPENT_PERCENT` and not reset yet. None when it can, or nothing is known.
    pub fn spent(&self, agent: AgentKind) -> Option<String> {
        let entries = self.entries.lock().unwrap();
        let entry = entries.get(&agent)?;
        if entry.account == AccountStatus::SignedOut {
            return Some("is signed out".into());
        }
        let (reading, _) = entry.last.as_ref()?;
        let now = chrono::Utc::now();
        reading
            .windows
            .iter()
            .filter(|window| !window.label.contains(' ') && window.used_percent >= SPENT_PERCENT)
            .find(|window| {
                window.resets_at.as_deref().is_none_or(|at| {
                    chrono::DateTime::parse_from_rfc3339(at).is_ok_and(|at| at > now)
                })
            })
            .map(|window| {
                format!(
                    "has used {:.0}% of its {} limit",
                    window.used_percent, window.label
                )
            })
    }

    /// Starts logging an agent's CLI in; the sidecar reports the end with `agent.login_ended`.
    /// Headless, for a browser on another machine: a flow whose page needs no callback here.
    pub async fn login(
        &self,
        daemon: &Daemon,
        agent: AgentKind,
        headless: bool,
    ) -> Result<AgentLoginResult> {
        let value = daemon
            .chats
            .sidecar
            .call(SidecarCall::AgentLogin(AgentLoginStartParams {
                agent,
                headless,
            }))
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    /// Passes the code a headless login's page showed on to the agent's CLI. Never logged.
    pub async fn login_code(&self, daemon: &Daemon, params: AgentLoginCodeParams) -> Result<()> {
        daemon
            .chats
            .sidecar
            .call(SidecarCall::AgentLoginCode(params))
            .await?;
        Ok(())
    }

    pub async fn login_cancel(&self, daemon: &Daemon, agent: AgentKind) -> Result<()> {
        daemon
            .chats
            .sidecar
            .call(SidecarCall::AgentLoginCancel(AgentModelsParams { agent }))
            .await?;
        Ok(())
    }

    /// A login ended. A completed one signs the agent in; either way the next ask reads it again,
    /// which confirms it (clients ask on `account_changed`).
    pub fn login_ended(&self, daemon: &Daemon, ended: AgentLoginEndedParams) {
        self.entries
            .lock()
            .unwrap()
            .entry(ended.agent)
            .or_default()
            .stale = true;
        if ended.success {
            self.set_account(daemon, ended.agent, AccountStatus::SignedIn, None);
        } else {
            tracing::info!(agent = ?ended.agent, error = ?ended.error, "agent login did not complete");
        }
        daemon.events.registry(RegistryEvent::LoginEnded {
            agent: ended.agent,
            worker: None,
            success: ended.success,
            error: ended.error,
        });
    }

    /// Records whether an agent is logged in, and tells clients when it changed.
    fn set_account(
        &self,
        daemon: &Daemon,
        agent: AgentKind,
        account: AccountStatus,
        error: Option<String>,
    ) {
        let changed = {
            let mut entries = self.entries.lock().unwrap();
            let entry = entries.entry(agent).or_default();
            let changed = entry.account != account;
            entry.account = account;
            entry.account_error = error.clone();
            changed
        };
        // Unknown is no news: the last word stands until a read says otherwise.
        if changed && account != AccountStatus::Unknown {
            daemon.events.registry(RegistryEvent::AccountChanged {
                agent,
                account,
                error,
            });
        }
    }

    pub async fn get(&self, daemon: &Daemon, refresh: bool) -> Vec<AgentUsage> {
        let _reading = self.reading.lock().await;
        for agent in AGENTS {
            let due = {
                let entries = self.entries.lock().unwrap();
                let entry = entries.get(&agent);
                refresh
                    || entry.is_none_or(|e| {
                        e.stale
                            || e.account == AccountStatus::SignedOut
                            || e.read.is_none_or(|at| at.elapsed() >= FRESH)
                    })
            };
            if !due {
                continue;
            }
            let answer = tokio::time::timeout(
                READ_TIMEOUT,
                daemon
                    .chats
                    .sidecar
                    .call(SidecarCall::AgentUsage(AgentModelsParams { agent })),
            )
            .await;
            let reading = match answer {
                Ok(Ok(value)) => serde_json::from_value::<AgentUsageResult>(value)
                    .map_err(|error| error.to_string()),
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => Err(format!("no answer after {} s", READ_TIMEOUT.as_secs())),
            };
            let account = {
                let mut entries = self.entries.lock().unwrap();
                let entry = entries.entry(agent).or_default();
                entry.read = Some(Instant::now());
                entry.stale = false;
                match reading {
                    // Signed out: the last limits read stay, behind the login to redo.
                    Ok(AgentUsageResult {
                        signed_out: Some(why),
                        ..
                    }) => {
                        entry.error = None;
                        entry.failed_at = None;
                        Some((AccountStatus::SignedOut, Some(why)))
                    }
                    Ok(result) => {
                        entry.last = Some((result, crate::store::now()));
                        entry.error = None;
                        entry.failed_at = None;
                        Some((AccountStatus::SignedIn, None))
                    }
                    // A failed read says nothing of the login: what was known stands.
                    Err(error) => {
                        tracing::debug!(%error, ?agent, "could not read the plan's usage");
                        entry.error = Some(error);
                        entry.failed_at = Some(crate::store::now());
                        None
                    }
                }
            };
            if let Some((account, why)) = account {
                self.set_account(daemon, agent, account, why);
            }
        }
        let entries = self.entries.lock().unwrap();
        AGENTS
            .iter()
            .map(|agent| {
                let entry = entries.get(agent).cloned().unwrap_or_default();
                let (plan, windows, extra_usage, reset_credits, read_at) = match entry.last {
                    Some((result, at)) => (
                        result.plan,
                        result.windows,
                        result.extra_usage,
                        result.reset_credits,
                        Some(at),
                    ),
                    None => (None, Vec::new(), None, None, None),
                };
                AgentUsage {
                    agent: *agent,
                    plan,
                    windows,
                    error: entry.error,
                    read_at,
                    failed_at: entry.failed_at,
                    extra_usage,
                    reset_credits,
                    account: entry.account,
                    account_error: entry.account_error,
                }
            })
            .collect()
    }
}
