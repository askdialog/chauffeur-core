//! Durable device trust, written only by the daemon. The host bridge reads private
//! configuration locally; mobile's RPC allowlist excludes every `remote.*` method.
use std::sync::Mutex;

use base64::Engine;
use chauffeur_protocol::client::{
    PushPreferences, PushRegistration, RemoteApproveParams, RemoteDevice, RemoteDeviceInfo,
    RemoteDevicesResult, RemotePushParams, RemoteStateResult,
};

use crate::{Error, Result, store::Store};

const KEY: &str = "remote.trust.v1";
const MAX_DEVICES: usize = 16;

pub struct RemoteTrust {
    store: Store,
    lock: Mutex<()>,
}

impl RemoteTrust {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            lock: Mutex::new(()),
        }
    }

    fn load(&self) -> Result<RemoteStateResult> {
        if let Some(value) = self.store.kv_get(KEY)? {
            let state: RemoteStateResult = serde_json::from_str(&value)?;
            if state.identity_seed.len() != 32 {
                return Err(Error::Unavailable(
                    "Invalid remote identity; refusing to replace it".into(),
                ));
            }
            return Ok(state);
        }
        let state = RemoteStateResult {
            identity_seed: rand::random::<[u8; 32]>().to_vec(),
            devices: vec![],
        };
        self.save(&state)?;
        Ok(state)
    }

    fn save(&self, state: &RemoteStateResult) -> Result<()> {
        self.store.kv_set(KEY, &serde_json::to_string(state)?)
    }

    pub fn state(&self) -> Result<RemoteStateResult> {
        let _guard = self.lock.lock().unwrap();
        self.load()
    }

    pub fn devices(&self) -> Result<RemoteDevicesResult> {
        let _guard = self.lock.lock().unwrap();
        Ok(RemoteDevicesResult {
            devices: self
                .load()?
                .devices
                .into_iter()
                .map(|device| device.info)
                .collect(),
        })
    }

    pub fn approve(&self, params: RemoteApproveParams) -> Result<RemoteDevice> {
        if !params.peer_id.starts_with("12D3KooW")
            || params.peer_id.len() != 52
            || !params.peer_id.bytes().all(|byte| {
                b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz".contains(&byte)
            })
        {
            return Err(Error::BadRequest(
                "Expected an Ed25519 phone identity".into(),
            ));
        }
        let name = params.name.trim();
        if name.is_empty() || name.len() > 80 || name.chars().any(char::is_control) {
            return Err(Error::BadRequest(
                "Device name must be 1–80 bytes without control characters".into(),
            ));
        }
        let url = reqwest::Url::parse(&params.relay)
            .map_err(|_| Error::BadRequest("Invalid relay origin".into()))?;
        let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if (url.scheme() != "wss" && !(url.scheme() == "ws" && loopback))
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(Error::BadRequest(
                "Use a WSS relay origin (WS loopback is for local testing)".into(),
            ));
        }
        let _guard = self.lock.lock().unwrap();
        let mut state = self.load()?;
        if let Some(device) = state.devices.iter().find(|device| {
            device.info.peer_id == params.peer_id && device.info.relay == params.relay
        }) {
            return Ok(device.clone());
        }
        if state.devices.len() >= MAX_DEVICES {
            return Err(Error::Conflict(
                "Revoke an existing device before adding another".into(),
            ));
        }
        let device = RemoteDevice {
            info: RemoteDeviceInfo {
                id: token::<16>(),
                peer_id: params.peer_id,
                name: name.into(),
                relay: params.relay,
                created_at: chrono::Utc::now().to_rfc3339(),
            },
            host_token: token::<32>(),
            phone_token: token::<32>(),
            secret: token::<32>(),
            push: None,
        };
        state.devices.push(device.clone());
        self.save(&state)?;
        Ok(device)
    }

    pub fn push_get(&self, id: &str) -> Result<PushPreferences> {
        let _guard = self.lock.lock().unwrap();
        let state = self.load()?;
        let device = state
            .devices
            .iter()
            .find(|d| d.info.id == id)
            .ok_or_else(|| Error::NotFound("Device is no longer authorized".into()))?;
        Ok(device
            .push
            .as_ref()
            .map(|p| p.preferences.clone())
            .unwrap_or_default())
    }
    pub fn push_set(&self, params: RemotePushParams) -> Result<PushPreferences> {
        let prefs = &params.config.preferences;
        if !matches!(prefs.language.as_str(), "fr" | "en") {
            return Err(Error::BadRequest(
                "Unsupported notification language".into(),
            ));
        }
        if let Some(token) = &params.config.token
            && (!(16..=4096).contains(&token.len())
                || !token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b)))
        {
            return Err(Error::BadRequest("Invalid push registration".into()));
        }
        let _guard = self.lock.lock().unwrap();
        let mut state = self.load()?;
        let device = state
            .devices
            .iter_mut()
            .find(|d| d.info.id == params.device_id)
            .ok_or_else(|| Error::NotFound("Device is no longer authorized".into()))?;
        let provider = if params.config.token.is_some() {
            params.config.provider
        } else {
            params
                .config
                .provider
                .or_else(|| device.push.as_ref().and_then(|p| p.provider.clone()))
        };
        let token = params
            .config
            .token
            .or_else(|| device.push.as_ref().map(|p| p.token.clone()));
        if provider == Some(chauffeur_protocol::client::PushProvider::Apns)
            && token.as_ref().is_some_and(|token| {
                !token.len().is_multiple_of(2) || !token.bytes().all(|b| b.is_ascii_hexdigit())
            })
        {
            return Err(Error::BadRequest("Invalid Apple push registration".into()));
        }
        if prefs.enabled && token.is_none() {
            return Err(Error::BadRequest(
                "Register this phone before enabling notifications".into(),
            ));
        }
        if let Some(token) = token {
            device.push = Some(PushRegistration {
                provider,
                token,
                preferences: prefs.clone(),
            });
        }
        self.save(&state)?;
        Ok(prefs.clone())
    }

    pub fn check(&self, id: &str) -> Result<RemoteDeviceInfo> {
        let _guard = self.lock.lock().unwrap();
        self.load()?
            .devices
            .into_iter()
            .find(|device| device.info.id == id)
            .map(|device| device.info)
            .ok_or_else(|| Error::NotFound("Device is no longer authorized".into()))
    }

    pub fn revoke(&self, id: &str) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        let mut state = self.load()?;
        state.devices.retain(|device| device.info.id != id);
        self.save(&state)
    }
}

fn token<const N: usize>() -> String {
    let bytes: [u8; N] = std::array::from_fn(|_| rand::random());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_survives_restart_and_revocation_preserves_host_identity() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("trust.db");
        let trust = RemoteTrust::new(Store::open(&path).unwrap());
        let seed = trust.state().unwrap().identity_seed;
        let params = || RemoteApproveParams {
            peer_id: format!("12D3KooW{}", "A".repeat(44)),
            name: "Pixel".into(),
            relay: "ws://127.0.0.1:41240".into(),
        };
        let device = trust.approve(params()).unwrap();
        assert_eq!(trust.approve(params()).unwrap().info.id, device.info.id);
        drop(trust);
        let trust = RemoteTrust::new(Store::open(&path).unwrap());
        assert_eq!(trust.state().unwrap().identity_seed, seed);
        assert_eq!(trust.check(&device.info.id).unwrap().name, "Pixel");
        let public = serde_json::to_string(&trust.devices().unwrap()).unwrap();
        assert!(!public.contains(&device.secret));
        trust.revoke(&device.info.id).unwrap();
        assert!(trust.check(&device.info.id).is_err());
        assert_eq!(trust.state().unwrap().identity_seed, seed);
        assert_ne!(trust.approve(params()).unwrap().info.id, device.info.id);
    }
}

#[cfg(test)]
mod push_tests {
    use super::*;
    use chauffeur_protocol::client::{PushConfigureParams, PushPreferences};
    #[test]
    fn a_grant_saved_before_idle_reminders_existed_does_not_receive_them() {
        let legacy: PushPreferences = serde_json::from_value(serde_json::json!({
            "enabled": true, "approvals": true, "completed": true, "errors": true,
            "show_workspace": false, "language": "fr",
        }))
        .expect("legacy preferences decode");
        assert!(!legacy.idle_reminders);
        assert!(PushPreferences::default().idle_reminders);
    }

    #[test]
    fn apple_delivery_metadata_survives_restart_and_legacy_grants_stay_compatible() {
        use chauffeur_protocol::client::{PushProvider, PushRegistration};
        let legacy: PushRegistration = serde_json::from_value(serde_json::json!({
            "token": "private_fcm_token_123456",
            "preferences": PushPreferences::default(),
        }))
        .expect("legacy grant decodes");
        assert_eq!(legacy.provider, None);
        let folder = tempfile::tempdir().expect("temporary folder");
        let path = folder.path().join("trust.db");
        let trust = RemoteTrust::new(Store::open(&path).expect("temporary store"));
        let device = trust
            .approve(RemoteApproveParams {
                peer_id: format!("12D3KooW{}", "A".repeat(44)),
                name: "iPhone".into(),
                relay: "wss://relay.example".into(),
            })
            .expect("approved phone");
        let preferences = PushPreferences {
            enabled: true,
            ..Default::default()
        };
        let set = |token: Option<String>, provider: Option<PushProvider>| RemotePushParams {
            device_id: device.info.id.clone(),
            config: PushConfigureParams {
                token,
                provider,
                preferences: preferences.clone(),
            },
        };
        assert!(
            trust
                .push_set(set(
                    Some("not_an_apple_token".into()),
                    Some(PushProvider::Apns)
                ))
                .is_err()
        );
        trust
            .push_set(set(Some("a".repeat(64)), Some(PushProvider::Apns)))
            .expect("Apple registration");
        trust
            .push_set(set(None, None))
            .expect("preferences preserve provider");
        drop(trust);
        let trust = RemoteTrust::new(Store::open(&path).expect("reopened store"));
        assert_eq!(
            trust.load().expect("saved trust").devices[0]
                .push
                .as_ref()
                .expect("push registration")
                .provider,
            Some(PushProvider::Apns)
        );
        trust
            .push_set(set(Some(legacy.token), None))
            .expect("legacy Android registration");
        assert_eq!(
            trust.load().expect("saved trust").devices[0]
                .push
                .as_ref()
                .expect("push registration")
                .provider,
            None
        );
    }
    #[test]
    fn push_is_private_persisted_and_removed_with_device_trust() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("trust.db");
        let trust = RemoteTrust::new(Store::open(&path).unwrap());
        let device = trust
            .approve(RemoteApproveParams {
                peer_id: format!("12D3KooW{}", "A".repeat(44)),
                name: "Pixel".into(),
                relay: "wss://relay.example".into(),
            })
            .unwrap();
        let mut prefs = PushPreferences {
            enabled: true,
            language: "fr".into(),
            idle_reminders: true,
            show_workspace: false,
            ..Default::default()
        };
        let token = "private_fcm_token_123456";
        trust
            .push_set(RemotePushParams {
                device_id: device.info.id.clone(),
                config: PushConfigureParams {
                    provider: Default::default(),
                    token: Some(token.into()),
                    preferences: prefs.clone(),
                },
            })
            .unwrap();
        assert!(
            !serde_json::to_string(&trust.devices().unwrap())
                .unwrap()
                .contains(token)
        );
        assert!(
            !serde_json::to_string(&trust.push_get(&device.info.id).unwrap())
                .unwrap()
                .contains(token)
        );
        drop(trust);
        let trust = RemoteTrust::new(Store::open(&path).unwrap());
        assert!(trust.push_get(&device.info.id).unwrap().enabled);
        prefs.enabled = false;
        trust
            .push_set(RemotePushParams {
                device_id: device.info.id.clone(),
                config: PushConfigureParams {
                    provider: Default::default(),
                    token: None,
                    preferences: prefs,
                },
            })
            .unwrap();
        assert!(!trust.push_get(&device.info.id).unwrap().enabled);
        trust.revoke(&device.info.id).unwrap();
        assert!(trust.push_get(&device.info.id).is_err());
        assert!(
            !serde_json::to_string(&trust.state().unwrap())
                .unwrap()
                .contains(token)
        );
    }
}
