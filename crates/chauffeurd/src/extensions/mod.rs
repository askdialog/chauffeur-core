//! Official runtime registrations. Core owns activation and admission, while the distribution
//! supplies implementations built against these services; catalogue metadata never loads code.
use crate::{Daemon, Error, Result, paths::Paths, store::Store};
use chauffeur_protocol::client::{ClientCall, ClientMethod};
use chauffeur_protocol::events::RegistryEvent;
use chauffeur_protocol::extensions::{ExtensionId, ExtensionInfo, ExtensionSetParams};
use futures_util::future::BoxFuture;
use serde_json::Value;
use std::{
    any::Any,
    sync::{Arc, Weak},
};
use tokio::sync::RwLock;

pub mod assets;
mod download;
pub mod managed;
pub mod packages;
pub mod process;
pub mod services;
pub mod storage;
pub mod web;

pub trait Runtime: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn dispatch<'a>(
        &'a self,
        daemon: &'a Arc<Daemon>,
        call: ClientCall,
    ) -> BoxFuture<'a, Result<Value>>;
    fn resume(&self) {}
    fn identity_changed(&self) {}
    fn activation_changed(&self) {}
    fn package_info(&self) -> Option<chauffeur_protocol::extensions::ExtensionPackageInfo> {
        None
    }
    fn busy(&self) -> bool {
        false
    }
    fn activation_lease(&self) -> Result<Option<Box<dyn Send>>> {
        Ok(None)
    }
    fn version(&self) -> Option<String> {
        None
    }
    fn viewer(&self, _daemon: &Arc<Daemon>) -> Option<Box<dyn Send>> {
        None
    }
}

pub type BuildRuntime = dyn Fn(Weak<Daemon>, &Paths, Store) -> Arc<dyn Runtime> + Send + Sync;

pub struct Registration {
    pub id: ExtensionId,
    pub name: &'static str,
    pub description: &'static str,
    pub version: &'static str,
    pub capabilities: &'static [&'static str],
    pub build: Arc<BuildRuntime>,
}

struct Entry {
    registration: Registration,
    runtime: Arc<dyn Runtime>,
    gate: RwLock<()>,
}

pub struct Extensions {
    entries: Vec<Entry>,
}

pub fn validate(registrations: &[Registration]) -> anyhow::Result<()> {
    for (index, registration) in registrations.iter().enumerate() {
        anyhow::ensure!(
            !registrations[..index]
                .iter()
                .any(|other| other.id == registration.id),
            "extension {:?} is registered twice",
            registration.id
        );
    }
    Ok(())
}

fn key(id: ExtensionId) -> &'static str {
    match id {
        ExtensionId::Database => "extension_database_enabled",
        ExtensionId::Office => "extension_office_enabled",
    }
}

fn name(id: ExtensionId) -> &'static str {
    match id {
        ExtensionId::Database => "Database",
        ExtensionId::Office => "Office",
    }
}

/// Existing installations keep registered extensions until their owner chooses otherwise.
pub fn enabled(daemon: &Daemon, id: ExtensionId) -> Result<bool> {
    Ok(daemon.extensions.entry(id).is_some()
        && daemon.store.kv_get(key(id))?.as_deref() != Some("false"))
}

pub fn list(daemon: &Daemon) -> Result<Vec<ExtensionInfo>> {
    daemon
        .extensions
        .entries
        .iter()
        .map(|entry| {
            let registration = &entry.registration;
            Ok(ExtensionInfo {
                id: registration.id,
                name: registration.name.into(),
                description: registration.description.into(),
                publisher: "Chauffeur".into(),
                version: entry
                    .runtime
                    .version()
                    .unwrap_or_else(|| registration.version.into()),
                enabled: enabled(daemon, registration.id)?,
                capabilities: registration
                    .capabilities
                    .iter()
                    .map(|s| String::from(*s))
                    .collect(),
                package: entry.runtime.package_info(),
            })
        })
        .collect()
}

impl Extensions {
    fn managed(&self, id: ExtensionId) -> Result<&managed::Managed> {
        self.get(id).ok_or_else(|| {
            Error::Unavailable(format!(
                "{} does not support independent packages in this build; update Chauffeur",
                name(id)
            ))
        })
    }

    pub fn configure(
        &self,
        daemon: &Arc<Daemon>,
        params: chauffeur_protocol::extensions::ExtensionConfigureParams,
    ) -> Result<Vec<ExtensionInfo>> {
        self.managed(params.id)?.configure(params.auto_update)?;
        list(daemon)
    }

    pub fn manage(
        &self,
        daemon: &Arc<Daemon>,
        params: chauffeur_protocol::extensions::ExtensionManageParams,
    ) -> Result<Vec<ExtensionInfo>> {
        self.managed(params.id)?.manage(params.command)?;
        list(daemon)
    }

    pub fn watch(&self, id: ExtensionId) -> Result<Box<dyn Send>> {
        self.managed(id)?.watch()
    }

    pub fn asset(
        &self,
        id: ExtensionId,
        revision: &str,
        nonce: &str,
    ) -> Option<assets::VerifiedRevision> {
        self.get::<managed::Managed>(id)?.asset(revision, nonce)
    }

    pub async fn call(
        &self,
        params: chauffeur_protocol::extensions::ExtensionCallParams,
    ) -> Result<Value> {
        self.managed(params.id)?
            .call(params.method, params.params, params.workspace)
            .await
    }
    pub fn new(
        registrations: Vec<Registration>,
        daemon: Weak<Daemon>,
        paths: &Paths,
        store: Store,
    ) -> Self {
        Self {
            entries: registrations
                .into_iter()
                .map(|registration| {
                    let runtime = (registration.build)(daemon.clone(), paths, store.clone());
                    Entry {
                        registration,
                        runtime,
                        gate: RwLock::new(()),
                    }
                })
                .collect(),
        }
    }

    fn entry(&self, id: ExtensionId) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|entry| entry.registration.id == id)
    }

    /// A registered implementation can recover its own state without Core depending on its type.
    pub fn get<T: Any>(&self, id: ExtensionId) -> Option<&T> {
        self.entry(id)?.runtime.as_any().downcast_ref()
    }

    pub fn resume(&self) {
        for entry in &self.entries {
            entry.runtime.resume();
        }
    }
    pub fn identity_changed(&self) {
        for entry in &self.entries {
            entry.runtime.identity_changed();
        }
    }
    pub fn viewer(&self, id: ExtensionId, daemon: &Arc<Daemon>) -> Option<Box<dyn Send>> {
        self.entry(id)?.runtime.viewer(daemon)
    }

    pub fn set(
        &self,
        daemon: &Arc<Daemon>,
        params: ExtensionSetParams,
    ) -> Result<Vec<ExtensionInfo>> {
        let entry = self.entry(params.id).ok_or_else(|| {
            Error::Unavailable(format!(
                "{} is not included in this build; install Chauffeur with this official extension",
                name(params.id)
            ))
        })?;
        // Never cut off a SQL commit or a sign-in halfway through an operation.
        let _exclusive = entry.gate.try_write().map_err(|_| {
            Error::Conflict(format!(
                "{} is busy; finish or cancel its current operation before changing its activation",
                name(params.id)
            ))
        })?;
        let _runtime_lease = entry.runtime.activation_lease()?;
        daemon.store.kv_set(
            key(params.id),
            if params.enabled { "true" } else { "false" },
        )?;
        entry.runtime.activation_changed();
        let extensions = list(daemon)?;
        daemon.events.registry(RegistryEvent::ExtensionsChanged {
            extensions: extensions.clone(),
        });
        Ok(extensions)
    }

    pub async fn dispatch(
        &self,
        daemon: &Arc<Daemon>,
        id: ExtensionId,
        call: ClientCall,
    ) -> Result<Value> {
        let entry = self.entry(id);
        // Older clients may ask for Office state even when its runtime is absent or disabled.
        if matches!(call, ClientCall::OfficeState(_)) && entry.is_none() {
            return crate::rpc::to_value(chauffeur_protocol::office::OfficeState::default());
        }
        let entry = entry.ok_or_else(|| {
            Error::Unavailable(format!("{} is not included in this build", name(id)))
        })?;
        let _lease = entry.gate.read().await;
        if !matches!(call, ClientCall::OfficeState(_)) && !enabled(daemon, id)? {
            return Err(Error::Unavailable(format!(
                "{} is disabled; enable it in Settings → Extensions",
                name(id)
            )));
        }
        entry.runtime.dispatch(daemon, call).await
    }
}

pub fn owner(method: ClientMethod) -> Option<ExtensionId> {
    match method {
        ClientMethod::DatabaseList
        | ClientMethod::DatabaseSave
        | ClientMethod::DatabaseDelete
        | ClientMethod::DatabaseTest
        | ClientMethod::DatabaseSchema
        | ClientMethod::DatabaseQuery
        | ClientMethod::DatabaseCancel
        | ClientMethod::DatabaseApply
        | ClientMethod::DatabaseHistory => Some(ExtensionId::Database),
        ClientMethod::OfficeState
        | ClientMethod::OfficeSignIn
        | ClientMethod::OfficeSignOut
        | ClientMethod::OfficeSetVisibility
        | ClientMethod::OfficeMove
        | ClientMethod::OfficeSetMedia
        | ClientMethod::OfficeSetSkin
        | ClientMethod::OfficeSignal
        | ClientMethod::OfficeSay
        | ClientMethod::OfficeClear
        | ClientMethod::OfficeJukebox
        | ClientMethod::OfficeJukeboxResolve
        | ClientMethod::OfficeSpend
        | ClientMethod::OfficeUpgrade
        | ClientMethod::OfficeChooseClass
        | ClientMethod::OfficeChangeClass
        | ClientMethod::OfficeAttack
        | ClientMethod::OfficeStrikeDummy
        | ClientMethod::OfficeOpenInBrowser
        | ClientMethod::OfficeBrowserLink
        | ClientMethod::WorkspaceMoveHouse => Some(ExtensionId::Office),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Pending;
    impl Runtime for Pending {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn dispatch<'a>(
            &'a self,
            _: &'a Arc<Daemon>,
            _: ClientCall,
        ) -> BoxFuture<'a, Result<Value>> {
            Box::pin(std::future::pending())
        }
    }
    fn registration() -> Registration {
        Registration {
            id: ExtensionId::Database,
            name: "Database",
            version: "test",
            description: "Test runtime",
            capabilities: &[],
            build: Arc::new(|_, _, _| Arc::new(Pending)),
        }
    }
    #[tokio::test]
    async fn an_operation_in_progress_prevents_disabling_until_it_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(
            dir.path().join("home"),
            dir.path().join("workspaces"),
            dir.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(&paths.settings, "linear_enabled = false\n").unwrap();
        let daemon = Daemon::open_with_extensions(paths, vec![registration()]).unwrap();
        let operation = daemon
            .extensions
            .entry(ExtensionId::Database)
            .unwrap()
            .gate
            .read()
            .await;
        let params = || ExtensionSetParams {
            id: ExtensionId::Database,
            enabled: false,
        };
        assert!(matches!(
            daemon.extensions.set(&daemon, params()),
            Err(Error::Conflict(_))
        ));
        assert!(enabled(&daemon, ExtensionId::Database).unwrap());
        drop(operation);
        daemon.extensions.set(&daemon, params()).unwrap();
        assert!(!enabled(&daemon, ExtensionId::Database).unwrap());
    }
    #[test]
    fn a_distribution_cannot_register_the_same_extension_twice() {
        assert!(validate(&[registration(), registration()]).is_err());
    }
}
