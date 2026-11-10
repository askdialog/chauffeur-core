//! The agents' model catalogs, asked of the sidecar and cached: listing starts an agent process.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chauffeur_protocol::model::{AgentKind, AgentModel, AgentModels, LoadoutEntry};
use chauffeur_protocol::sidecar::{
    AgentModelsParams, AgentModelsResult, DelegateTier, SidecarCall,
};

use crate::{Daemon, Result};

const TTL: Duration = Duration::from_secs(60 * 60);
const AGENTS: [AgentKind; 2] = [AgentKind::Claude, AgentKind::Codex];
const LOADOUT_KEY: &str = "loadout";

#[derive(Default)]
pub struct ModelCatalog {
    /// Successful listings only: a failure is retried on the next call.
    cache: tokio::sync::Mutex<HashMap<AgentKind, (Instant, Vec<AgentModel>)>>,
}

impl ModelCatalog {
    pub async fn list(&self, daemon: &Arc<Daemon>, refresh: bool) -> Vec<AgentModels> {
        let [claude, codex] = AGENTS.map(|agent| self.agent(daemon, agent, refresh));
        let (claude, mut codex) = tokio::join!(claude, codex);
        // Read from disk at each call, never cached: a pull or a delete shows at once.
        codex.models.extend(daemon.ollama.agent_models());
        vec![claude, codex]
    }

    async fn agent(&self, daemon: &Arc<Daemon>, agent: AgentKind, refresh: bool) -> AgentModels {
        if !refresh
            && let Some((at, models)) = self.cache.lock().await.get(&agent)
            && at.elapsed() < TTL
        {
            return AgentModels {
                agent,
                models: models.clone(),
                error: None,
            };
        }
        let listed = daemon
            .chats
            .sidecar
            .call(SidecarCall::AgentModels(AgentModelsParams { agent }))
            .await
            .and_then(|value| Ok(serde_json::from_value::<AgentModelsResult>(value)?));
        match listed {
            Ok(result) => {
                self.cache
                    .lock()
                    .await
                    .insert(agent, (Instant::now(), result.models.clone()));
                AgentModels {
                    agent,
                    models: result.models,
                    error: None,
                }
            }
            Err(error) => {
                tracing::warn!(%error, agent = agent.as_str(), "could not list the agent's models");
                AgentModels {
                    agent,
                    models: Vec::new(),
                    error: Some(format!("{error:#}")),
                }
            }
        }
    }
}

/// The model a delegated chat of `tier` runs on, by its id in the agent's catalog; none for
/// `default`, which follows the parent or the user's default. Kept by hand: the `release` skill
/// checks it against `chauffeur models`. No tier picks Fable: it runs only when the user names it.
pub fn tier_model(agent: AgentKind, tier: DelegateTier) -> Option<&'static str> {
    match (agent, tier) {
        (_, DelegateTier::Default) => None,
        (AgentKind::Claude, DelegateTier::Fast) => Some("haiku"),
        (AgentKind::Claude, DelegateTier::Strongest) => Some("opus"),
        (AgentKind::Codex, DelegateTier::Fast) => Some("gpt-6-luna"),
        (AgentKind::Codex, DelegateTier::Strongest) => Some("gpt-6-astra"),
    }
}

/// The user's favorite model settings; empty until the user picks some.
pub fn loadout(daemon: &Daemon) -> Result<Vec<LoadoutEntry>> {
    Ok(match daemon.store.kv_get(LOADOUT_KEY)? {
        Some(json) => serde_json::from_str(&json).unwrap_or_default(),
        None => Vec::new(),
    })
}

pub fn set_loadout(daemon: &Daemon, entries: &[LoadoutEntry]) -> Result<()> {
    daemon
        .store
        .kv_set(LOADOUT_KEY, &serde_json::to_string(entries)?)
}
