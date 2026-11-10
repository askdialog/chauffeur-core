//! Signing a GCP worker's agents in from this machine (docs/cloud-workers.md): through its
//! forwarded socket, the worker's own CLI starts a login whose page needs no callback to the
//! worker (Claude Code's manual page, Codex's device code); the user opens it here and, for Claude
//! Code, pastes back the code it shows. The credentials stay on the worker: only that single-use
//! code, which the worker's CLI alone can exchange (it keeps the PKCE secret), passes through this
//! daemon, never kept or logged. A transfer refused for want of these logins resumes once the
//! worker has them all.
use std::sync::Arc;
use std::time::Duration;

use chauffeur_protocol::client::{
    AgentLoginParams, ClientCall, ModelsListParams, SubscribeParams, UsageResult,
};
use chauffeur_protocol::events::{Event, RegistryEvent};
use chauffeur_protocol::model::{AccountStatus, AgentKind};
use chauffeur_protocol::sidecar::{AgentLoginCodeParams, AgentLoginResult};
use chauffeur_protocol::worker::{WorkerLoginCodeParams, WorkerLoginParams};

use crate::relay::Machine;
use crate::{Daemon, Error, Result};

/// The worker's sidecar gives a login up after ten minutes: past this, its end is not coming.
const WAIT: Duration = Duration::from_secs(11 * 60);

fn machine(daemon: &Daemon, name: &str) -> Result<Machine> {
    if !crate::workers::is_worker(daemon, name) {
        return Err(Error::NotFound(format!(
            "No worker named {name}; see Settings → Transfers"
        )));
    }
    let socket = crate::workers::machine_socket(daemon, name)
        .map_err(|error| Error::Conflict(format!("{error:#}")))?;
    Ok(Machine {
        name: name.to_owned(),
        socket,
    })
}

/// Starts the login on the worker and returns its page; its end comes as a `login_ended` event
/// naming the worker.
pub async fn start(daemon: &Arc<Daemon>, params: WorkerLoginParams) -> Result<AgentLoginResult> {
    let machine = machine(daemon, &params.name)?;
    // Open before the login starts: its end may come at any time after.
    let mut subscription = daemon
        .relay
        .subscribe(
            &machine,
            SubscribeParams {
                registry: true,
                ..Default::default()
            },
        )
        .await?;
    let started: AgentLoginResult = serde_json::from_value(
        daemon
            .relay
            .call(
                &machine,
                ClientCall::AgentLogin(AgentLoginParams {
                    agent: params.agent,
                    headless: true,
                }),
            )
            .await?,
    )?;
    let WorkerLoginParams { name, agent } = params;
    let key = (name.clone(), agent);
    let follower = daemon.clone();
    let task = tokio::spawn(async move {
        let heard = async {
            while let Some(envelope) = subscription.events.recv().await {
                if let Event::Registry {
                    event:
                        RegistryEvent::LoginEnded {
                            agent: ended,
                            worker: None,
                            success,
                            error,
                        },
                } = envelope.event
                    && ended == agent
                {
                    return (success, error);
                }
            }
            (false, Some(format!("{name} stopped answering")))
        };
        let (success, error) = tokio::time::timeout(WAIT, heard)
            .await
            .unwrap_or_else(|_| (false, Some("the login was not completed in time".into())));
        {
            let mut logins = follower.workers.logins.lock().unwrap();
            let key = (name.clone(), agent);
            if logins
                .get(&key)
                .is_some_and(|h| h.id() == tokio::task::id())
            {
                logins.remove(&key);
            }
        }
        ended(&follower, &name, agent, success, error).await;
    });
    // The worker's sidecar replaced the earlier login without a word: this one tells the end.
    if let Some(earlier) = daemon
        .workers
        .logins
        .lock()
        .unwrap()
        .insert(key, task.abort_handle())
    {
        earlier.abort();
    }
    Ok(started)
}

pub async fn code(daemon: &Daemon, params: WorkerLoginCodeParams) -> Result<()> {
    let code = params.code.trim();
    if code.is_empty() {
        return Err(Error::BadRequest(
            "No code: paste the one the login page shows".into(),
        ));
    }
    daemon
        .relay
        .call(
            &machine(daemon, &params.name)?,
            ClientCall::AgentLoginCode(AgentLoginCodeParams {
                agent: params.agent,
                code: code.to_owned(),
            }),
        )
        .await?;
    Ok(())
}

pub async fn cancel(daemon: &Daemon, params: WorkerLoginParams) -> Result<()> {
    daemon
        .relay
        .call(
            &machine(daemon, &params.name)?,
            ClientCall::AgentLoginCancel(AgentLoginParams {
                agent: params.agent,
                headless: false,
            }),
        )
        .await?;
    Ok(())
}

/// The worker's agents as its daemon reads them: whether each is signed in there.
pub async fn usage(daemon: &Daemon, name: &str) -> Result<UsageResult> {
    let value = daemon
        .relay
        .call(
            &machine(daemon, name)?,
            ClientCall::UsageGet(ModelsListParams {
                refresh: false,
                workspace: None,
            }),
        )
        .await?;
    Ok(serde_json::from_value(value)?)
}

async fn ended(
    daemon: &Arc<Daemon>,
    worker: &str,
    agent: AgentKind,
    success: bool,
    error: Option<String>,
) {
    daemon.events.registry(RegistryEvent::LoginEnded {
        agent,
        worker: Some(worker.to_owned()),
        success,
        error,
    });
    if !success {
        return;
    }
    // The agent just signed in counts even if the worker's reading lags behind.
    let usage = usage(daemon, worker).await.unwrap_or_else(|error| {
        tracing::warn!(%error, worker, "could not read which agents are signed in on a worker");
        UsageResult { agents: Vec::new() }
    });
    let signed_in = |other: AgentKind| {
        other == agent
            || usage
                .agents
                .iter()
                .any(|u| u.agent == other && u.account == AccountStatus::SignedIn)
    };
    if let Err(error) =
        crate::cloud_transfer_driver::resume_after_login(daemon, worker, signed_in).await
    {
        tracing::warn!(%error, worker, "could not resume the transfers waiting for a login");
    }
}
