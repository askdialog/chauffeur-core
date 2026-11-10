//! Compaction of Claude and Codex chats through Jev (TypeSafe), off until the user turns it on in
//! the app's settings: on/off and threshold in the store, the TypeSafe API key with the user's
//! secrets. A session started while it is on compacts itself past the threshold: Claude through
//! Chauffeur's copy of the fast-jev-compaction plugin (the sidecar's `agents/claude/jev.ts`), Codex
//! by moving to a thread holding what Jev keeps (`agents/codex/jev.ts`).

use std::ops::RangeInclusive;
use std::sync::Mutex;

use chauffeur_protocol::client::{JevSettings, JevUpdate};
use chauffeur_protocol::sidecar::JevParams;
use serde::{Deserialize, Serialize};

use crate::secrets::{SecretName, Secrets};
use crate::store::Store;
use crate::{Error, Result};

pub const API_KEY: SecretName<'static> = SecretName {
    service: "chauffeur-jev",
    file: "jev-api-key",
    label: "Chauffeur: TypeSafe API key (Jev)",
};
const KV_KEY: &str = "jev";
pub const DEFAULT_THRESHOLD: u8 = 60;
pub const THRESHOLDS: RangeInclusive<u8> = 1..=95;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    enabled: bool,
    threshold_percent: u8,
}

impl Default for Stored {
    fn default() -> Self {
        Stored {
            enabled: false,
            threshold_percent: DEFAULT_THRESHOLD,
        }
    }
}

pub struct Jev {
    secrets: Secrets,
    /// The saved key, once read (the Keychain is asked at most once).
    key: Mutex<Option<Option<String>>>,
}

impl Jev {
    pub fn new(secrets: Secrets) -> Jev {
        Jev {
            secrets,
            key: Mutex::new(None),
        }
    }

    fn key(&self) -> Option<String> {
        self.key
            .lock()
            .unwrap()
            .get_or_insert_with(|| self.secrets.read(&API_KEY))
            .clone()
    }

    fn stored(store: &Store) -> Result<Stored> {
        Ok(match store.kv_get(KV_KEY)? {
            Some(json) => serde_json::from_str(&json).unwrap_or_default(),
            None => Stored::default(),
        })
    }

    pub fn settings(&self, store: &Store) -> Result<JevSettings> {
        let stored = Self::stored(store)?;
        Ok(JevSettings {
            enabled: stored.enabled,
            threshold_percent: stored.threshold_percent,
            key_saved: self.key().is_some(),
        })
    }

    /// Applies what changes. Jev cannot be on without a key: forgetting the key turns it off.
    pub fn update(&self, store: &Store, update: JevUpdate) -> Result<JevSettings> {
        let mut stored = Self::stored(store)?;
        if let Some(percent) = update.threshold_percent {
            if !THRESHOLDS.contains(&percent) {
                return Err(Error::BadRequest(format!(
                    "the threshold is a percentage between {} and {}",
                    THRESHOLDS.start(),
                    THRESHOLDS.end()
                )));
            }
            stored.threshold_percent = percent;
        }
        if let Some(key) = update.api_key.as_deref().map(str::trim) {
            if key.is_empty() {
                self.secrets.delete(&API_KEY)?;
                *self.key.lock().unwrap() = Some(None);
                stored.enabled = false;
            } else {
                self.secrets
                    .store(&API_KEY, key)
                    .map_err(|error| Error::BadRequest(format!("{error:#}")))?;
                *self.key.lock().unwrap() = Some(Some(key.to_string()));
            }
        }
        if let Some(enabled) = update.enabled {
            if enabled && self.key().is_none() {
                return Err(Error::BadRequest(
                    "save a TypeSafe API key before turning Jev on".into(),
                ));
            }
            stored.enabled = enabled;
        }
        store.kv_set(KV_KEY, &serde_json::to_string(&stored)?)?;
        self.settings(store)
    }

    /// What a session gets: None while Jev is off or without a key.
    pub fn session_params(&self, store: &Store) -> Option<JevParams> {
        let stored = Self::stored(store).ok()?;
        if !stored.enabled {
            return None;
        }
        Some(JevParams {
            api_key: self.key()?,
            threshold_percent: stored.threshold_percent,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;

    #[test]
    fn jev_is_off_until_a_key_is_saved_and_it_is_turned_on() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(
            dir.path().join("home"),
            dir.path().join("workspaces"),
            dir.path().join("repos"),
        );
        paths.ensure().unwrap();
        let store = Store::open(&paths.db).unwrap();
        let jev = Jev::new(Secrets::for_paths(&paths));

        let initial = jev.settings(&store).unwrap();
        assert_eq!(
            initial,
            JevSettings {
                enabled: false,
                threshold_percent: 60,
                key_saved: false
            }
        );
        assert!(jev.session_params(&store).is_none());

        let on = JevUpdate {
            enabled: Some(true),
            ..Default::default()
        };
        assert!(matches!(
            jev.update(&store, on.clone()),
            Err(Error::BadRequest(_))
        ));
        let too_high = JevUpdate {
            threshold_percent: Some(99),
            ..Default::default()
        };
        assert!(matches!(
            jev.update(&store, too_high),
            Err(Error::BadRequest(_))
        ));

        let saved = jev
            .update(
                &store,
                JevUpdate {
                    api_key: Some(" ts_key-1 ".into()),
                    threshold_percent: Some(40),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(saved.key_saved && !saved.enabled && saved.threshold_percent == 40);
        assert!(jev.session_params(&store).is_none());

        jev.update(&store, on).unwrap();
        assert_eq!(
            jev.session_params(&store),
            Some(JevParams {
                api_key: "ts_key-1".into(),
                threshold_percent: 40
            })
        );
        // A daemon started afresh reads the same.
        let again = Jev::new(Secrets::for_paths(&paths));
        assert_eq!(again.session_params(&store), jev.session_params(&store));

        let forgotten = jev
            .update(
                &store,
                JevUpdate {
                    api_key: Some(String::new()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!forgotten.enabled && !forgotten.key_saved);
        assert!(jev.session_params(&store).is_none());
    }
}
