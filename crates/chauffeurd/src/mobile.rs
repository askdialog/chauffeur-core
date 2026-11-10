//! Supervises the bundled local bridge. Trust and configuration belong to the daemon;
//! the opaque cloud relay and the bridge never receive the daemon's TCP token.
use crate::{
    Daemon, Error, Result,
    secrets::{SecretName, Secrets},
    settings::LiveSettings,
    store::Store,
};
use chauffeur_protocol::client::{MobileConfigureParams, MobileHostStatus};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
};

pub const OPERATOR_KEY: SecretName<'static> = SecretName {
    service: "chauffeur-mobile-relay",
    file: "mobile-relay-token",
    label: "Chauffeur: mobile relay operator credential",
};
const CONFIG: &str = "mobile-host";
/// Off, with no relay: an operator's relay is named when mobile access is turned on.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    enabled: bool,
    relay: String,
}
#[derive(Default)]
struct Runtime {
    phase: String,
    error: Option<String>,
    connected: usize,
}
pub struct MobileHost {
    secrets: Secrets,
    /// `[mobile] relay`: the relay offered while the user has named none.
    settings: Arc<LiveSettings>,
    key: Mutex<Option<Option<String>>>,
    revision: AtomicU64,
    runtime: Mutex<Runtime>,
}
impl MobileHost {
    pub fn new(secrets: Secrets, settings: Arc<LiveSettings>) -> Self {
        Self {
            secrets,
            settings,
            key: Mutex::new(None),
            revision: AtomicU64::new(0),
            runtime: Mutex::new(Runtime::default()),
        }
    }
    /// What the user configured; while they named no relay, `[mobile] relay` (a distribution's).
    fn config(&self, store: &Store) -> Result<Config> {
        let mut config: Config = match store.kv_get(CONFIG)? {
            Some(value) => serde_json::from_str(&value).map_err(anyhow::Error::from)?,
            None => Config::default(),
        };
        if config.relay.trim().is_empty() {
            config.relay = self.settings.get().mobile.relay.trim().to_string();
        }
        Ok(config)
    }
    fn key(&self) -> Option<String> {
        self.key
            .lock()
            .unwrap()
            .get_or_insert_with(|| self.secrets.read(&OPERATOR_KEY))
            .clone()
    }
    pub fn status(&self, store: &Store) -> Result<MobileHostStatus> {
        let config = self.config(store)?;
        let key_saved = self.key().is_some();
        let state = self.runtime.lock().unwrap();
        Ok(MobileHostStatus {
            enabled: config.enabled,
            relay: config.relay,
            key_saved,
            phase: if !config.enabled {
                "disabled".into()
            } else if state.phase.is_empty() {
                "starting".into()
            } else {
                state.phase.clone()
            },
            error: if config.enabled {
                state.error.clone()
            } else {
                None
            },
            connected_devices: if config.enabled { state.connected } else { 0 },
        })
    }
    pub fn configure(
        &self,
        store: &Store,
        params: MobileConfigureParams,
    ) -> Result<MobileHostStatus> {
        validate_relay(&params.relay)?;
        if let Some(key) = &params.operator_token
            && (!(43..=128).contains(&key.len())
                || !key
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'))
        {
            return Err(Error::BadRequest("Invalid relay credential".into()));
        }
        let old = self.config(store)?;
        // A credential is scoped to its relay; do not send an old key to a changed URL.
        if params.enabled
            && params.operator_token.is_none()
            && (old.relay != params.relay || self.key().is_none())
        {
            return Err(Error::BadRequest(
                "Enter the credential for this relay before enabling mobile access".into(),
            ));
        }
        if let Some(key) = params.operator_token {
            self.secrets.store(&OPERATOR_KEY, &key).map_err(|_| {
                Error::Unavailable("Could not save the relay credential in the secret store".into())
            })?;
            *self.key.lock().unwrap() = Some(Some(key));
        } else if old.relay != params.relay {
            self.secrets.delete(&OPERATOR_KEY)?;
            *self.key.lock().unwrap() = Some(None);
        }
        store.kv_set(
            CONFIG,
            &serde_json::to_string(&Config {
                enabled: params.enabled,
                relay: params.relay,
            })?,
        )?;
        tracing::info!(
            enabled = params.enabled,
            "mobile access configuration changed"
        );
        self.revision.fetch_add(1, Ordering::Relaxed);
        self.report("starting", None, 0);
        self.status(store)
    }
    fn report(&self, phase: &str, error: Option<&str>, connected: usize) {
        *self.runtime.lock().unwrap() = Runtime {
            phase: phase.into(),
            error: error.map(str::to_string),
            connected,
        };
    }
    pub async fn serve(&self, daemon: Arc<Daemon>) {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(800))
            .build()
            .expect("local HTTP client");
        let mut child: Option<Child> = None;
        let mut revision = self.revision.load(Ordering::Relaxed);
        let mut retry_at = tokio::time::Instant::now();
        let control = format!(
            "{:032x}{:032x}",
            rand::random::<u128>(),
            rand::random::<u128>()
        );
        let port = std::env::var("CHAUFFEUR_PAIRING_PAGE_PORT")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|p| *p != 0)
            .unwrap_or(41241);
        loop {
            let Ok(config) = self.config(&daemon.store) else {
                self.report("error", Some("Could not read mobile settings"), 0);
                tokio::select! { _ = daemon.shutdown.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(5)) => {} }
                continue;
            };
            let next = self.revision.load(Ordering::Relaxed);
            if !config.enabled || next != revision {
                if let Some(mut process) = child.take() {
                    let _ = process.kill().await;
                }
                revision = next;
                retry_at = tokio::time::Instant::now();
            }
            if config.enabled {
                if let Some(process) = child.as_mut()
                    && !matches!(process.try_wait(), Ok(None))
                {
                    tracing::warn!("mobile host exited; retrying automatically");
                    child = None;
                    retry_at = tokio::time::Instant::now() + Duration::from_secs(5);
                    self.report(
                        "error",
                        Some("Mobile host stopped. Check that another mobile host is not using the pairing port; retrying automatically."),
                        0,
                    );
                }
                if child.is_none() && tokio::time::Instant::now() >= retry_at {
                    self.report("starting", None, 0);
                    match self.spawn(&daemon, &config, &control, port).await {
                        Ok(process) => {
                            tracing::info!("mobile host started");
                            child = Some(process);
                        }
                        Err(_) => {
                            self.report("error", Some("Cannot start mobile host. Install a build containing chauffeur-mobile-host and check that its port is free."), 0);
                            retry_at = tokio::time::Instant::now() + Duration::from_secs(5);
                        }
                    }
                }
                if child.is_some() {
                    let health = client
                        .get(format!("http://127.0.0.1:{port}/health"))
                        .header("x-chauffeur-host", &control)
                        .send()
                        .await;
                    if let Ok(response) = health
                        && response.status().is_success()
                        && let Ok(value) = response.json::<serde_json::Value>().await
                    {
                        if value["daemonAvailable"] == true {
                            self.report(
                                "ready",
                                None,
                                value["connectedDevices"].as_u64().unwrap_or(0) as usize,
                            );
                        } else {
                            self.report(
                                "connecting",
                                Some("Waiting for the mobile host to reach the daemon"),
                                0,
                            );
                        }
                    } else {
                        self.report(
                            "connecting",
                            Some("Waiting for the mobile host to respond"),
                            0,
                        );
                    }
                }
            }
            tokio::select! { _ = daemon.shutdown.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(1)) => {} }
        }
        if let Some(mut process) = child {
            let _ = process.kill().await;
        }
    }
    async fn spawn(
        &self,
        daemon: &Daemon,
        config: &Config,
        control: &str,
        port: u16,
    ) -> anyhow::Result<Child> {
        use std::process::Stdio;
        let key = self
            .key()
            .ok_or_else(|| anyhow::anyhow!("relay credential missing"))?;
        let binary = host_binary();
        let mut child = Command::new(binary)
            .arg("--daemon")
            .env("CHAUFFEUR_HOME", &daemon.paths.home)
            .env("CHAUFFEUR_PAIRING_PAGE_PORT", port.to_string())
            .env_remove("RELAY_ADMIN_TOKEN")
            .env_remove("RELAY_ORIGIN")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let payload = serde_json::to_vec(
            &serde_json::json!({"relay": config.relay, "adminToken": key, "controlToken": control}),
        )?;
        let sent = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("missing stdin"))?
            .write_all(&payload)
            .await;
        if let Err(error) = sent {
            let _ = child.kill().await;
            return Err(error.into());
        }
        Ok(child)
    }
}
fn validate_relay(value: &str) -> Result<()> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| Error::BadRequest("Invalid WSS relay origin".into()))?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if (url.scheme() != "wss" && !(url.scheme() == "ws" && loopback))
        || url.host_str().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Error::BadRequest(
            "Use a WSS relay origin; WS is only allowed on loopback for testing".into(),
        ));
    }
    Ok(())
}
fn host_binary() -> PathBuf {
    if let Some(value) = std::env::var_os("CHAUFFEUR_MOBILE_HOST") {
        return value.into();
    }
    if let Some(path) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("chauffeur-mobile-host")))
        .filter(|p| p.is_file())
    {
        return path;
    }
    #[cfg(debug_assertions)]
    {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../relay/dist/chauffeur-mobile-host");
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from("chauffeur-mobile-host")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_survives_restart_without_exposing_credentials_and_binds_keys_to_relay() {
        let root = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(
            root.path().join("home"),
            root.path().join("workspaces"),
            root.path().join("repos"),
        );
        paths.ensure().unwrap();
        let store = Store::open(&paths.db).unwrap();
        let settings = Arc::new(LiveSettings::load(&paths).unwrap());
        let host = MobileHost::new(Secrets::for_paths(&paths), settings.clone());
        let key = "a".repeat(64);
        let mut params = MobileConfigureParams {
            enabled: true,
            relay: "wss://relay.example".into(),
            operator_token: None,
        };
        assert!(host.configure(&store, params.clone()).is_err());
        params.operator_token = Some(key.clone());
        let status = host.configure(&store, params.clone()).unwrap();
        assert!(status.enabled && status.key_saved);
        assert!(!serde_json::to_string(&status).unwrap().contains(&key));
        assert!(!store.kv_get(CONFIG).unwrap().unwrap().contains(&key));
        let restarted = MobileHost::new(Secrets::for_paths(&paths), settings);
        assert!(restarted.status(&store).unwrap().enabled);
        params.operator_token = None;
        params.relay = "wss://other.example".into();
        assert!(restarted.configure(&store, params.clone()).is_err());
        assert_eq!(
            restarted.status(&store).unwrap().relay,
            "wss://relay.example"
        );
        params.enabled = false;
        restarted.configure(&store, params).unwrap();
        assert!(!restarted.status(&store).unwrap().key_saved);
    }
    #[test]
    fn the_settings_relay_is_offered_until_the_user_names_one() {
        let root = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(
            root.path().join("home"),
            root.path().join("workspaces"),
            root.path().join("repos"),
        );
        paths.ensure().unwrap();
        let store = Store::open(&paths.db).unwrap();
        let settings = Arc::new(LiveSettings::load(&paths).unwrap());
        let host = MobileHost::new(Secrets::for_paths(&paths), settings.clone());
        // Core names no relay.
        assert_eq!(host.status(&store).unwrap().relay, "");
        settings
            .update(|doc| doc["mobile"]["relay"] = toml_edit::value("wss://relay.example"))
            .unwrap();
        let status = host.status(&store).unwrap();
        assert_eq!(
            (status.enabled, status.relay.as_str()),
            (false, "wss://relay.example")
        );
        // Turned off with the relay offered: still offered, no credential dropped.
        host.configure(
            &store,
            MobileConfigureParams {
                enabled: false,
                relay: "wss://relay.example".into(),
                operator_token: None,
            },
        )
        .unwrap();
        assert_eq!(host.status(&store).unwrap().relay, "wss://relay.example");
        // A relay the user names wins over the settings'.
        host.configure(
            &store,
            MobileConfigureParams {
                enabled: true,
                relay: "wss://own.example".into(),
                operator_token: Some("b".repeat(64)),
            },
        )
        .unwrap();
        assert_eq!(host.status(&store).unwrap().relay, "wss://own.example");
    }
    #[test]
    fn only_tls_origins_and_explicit_loopback_tests_are_allowed() {
        for value in [
            "wss://relay.example",
            "ws://127.0.0.1:41240",
            "ws://[::1]:41240",
        ] {
            assert!(validate_relay(value).is_ok(), "{value}");
        }
        for value in [
            "ws://192.168.1.3",
            "https://relay.example",
            "wss://user@relay.example",
            "wss://relay.example/private",
            "wss://relay.example?key=x",
            "wss://relay.example#x",
        ] {
            assert!(validate_relay(value).is_err(), "{value}");
        }
    }
}
