//! Callbacks authenticated by the engine's private pipe, scoped to its official extension ID.
use super::storage;
use crate::{
    Daemon, Error, Result,
    secrets::{SecretName, Secrets},
};
use chauffeur_protocol::extension_host::ExtensionHostCall;
use chauffeur_protocol::extensions::ExtensionId;
use futures_util::future::BoxFuture;
use rusqlite::{
    Connection, OptionalExtension,
    hooks::{AuthAction, AuthContext, Authorization},
};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

pub struct Broker {
    daemon: Weak<Daemon>,
    id: ExtensionId,
    store: Mutex<Connection>,
    /// Whether its engine may change what no rollback snapshot holds: protected credentials and
    /// Core's sign-in. A candidate version has not this until it is the installed one.
    active: AtomicBool,
}

fn authorize(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Attach { .. }
        | AuthAction::Detach { .. }
        | AuthAction::Pragma {
            pragma_value: Some(_),
            ..
        }
        | AuthAction::CreateVtable { .. }
        | AuthAction::DropVtable { .. } => Authorization::Deny,
        AuthAction::Function { function_name }
            if ["load_extension", "readfile", "writefile"]
                .iter()
                .any(|name| name.eq_ignore_ascii_case(function_name)) =>
        {
            Authorization::Deny
        }
        _ => Authorization::Allow,
    }
}

impl Broker {
    /// The broker of an installed engine, with every capability of its extension.
    pub fn open(daemon: Weak<Daemon>, id: ExtensionId, dir: &Path) -> Result<Arc<Self>> {
        let broker = Self::open_preflight(daemon, id, dir)?;
        broker.active.store(true, Ordering::Release);
        Ok(broker)
    }

    /// The broker of a candidate engine starting: its state and preferences, which a failed
    /// switch restores, but no credential nor sign-in change until `activate`.
    pub fn open_preflight(daemon: Weak<Daemon>, id: ExtensionId, dir: &Path) -> Result<Arc<Self>> {
        std::fs::create_dir_all(dir)?;
        crate::paths::private_dir(dir)?;
        let path = dir.join("state.sqlite");
        let mut store = Connection::open(&path)?;
        crate::paths::private_file(&path)?;
        store.pragma_update(None, "journal_mode", "WAL")?;
        store.busy_timeout(std::time::Duration::from_secs(5))?;
        store.authorizer(Some(authorize))?;
        Ok(Arc::new(Self {
            daemon,
            id,
            store: Mutex::new(store),
            active: AtomicBool::new(false),
        }))
    }

    fn daemon(&self) -> Result<Arc<Daemon>> {
        self.daemon
            .upgrade()
            .ok_or_else(|| Error::Unavailable("Core stopped; restart Chauffeur".into()))
    }

    fn database(&self) -> Result<()> {
        if self.id != ExtensionId::Database {
            return Err(Error::BadRequest(
                "This extension cannot access Database credentials; use its own host capabilities"
                    .into(),
            ));
        }
        Ok(())
    }

    fn office(&self) -> Result<()> {
        if self.id != ExtensionId::Office {
            return Err(Error::BadRequest(
                "This extension cannot access Office host services; use its own host capabilities"
                    .into(),
            ));
        }
        Ok(())
    }
    fn office_key(&self, key: &str, write: bool) -> Result<()> {
        self.office()?;
        if !key.starts_with("office_") || (write && key == crate::identity::EMAIL_KEY) {
            return Err(Error::BadRequest(
                "This state belongs to Core; use the identity service to change sign-in".into(),
            ));
        }
        Ok(())
    }

    fn owner_key(&self, key: &str) -> Result<String> {
        self.database()?;
        uuid::Uuid::parse_str(key).map_err(|_| {
            Error::BadRequest("The credential ID is invalid; save the connection again".into())
        })?;
        Ok(format!("extension_database_credential_{key}"))
    }

    fn credential(&self, key: &str) -> Result<()> {
        let owner = self.owner_key(key)?;
        if self.daemon()?.store.kv_get(&owner)?.as_deref() != Some("true") {
            return Err(Error::BadRequest(
                "This credential does not belong to Database; save the connection again".into(),
            ));
        }
        Ok(())
    }

    fn import_legacy(&self) -> Result<()> {
        self.database()?;
        let daemon = self.daemon()?;
        let store = self.store.lock().unwrap();
        let imported: Option<String> = store
            .query_row(
                "SELECT value FROM extension_meta WHERE key='legacy_imported'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if imported.is_some() {
            return Ok(());
        }
        let (connections, history) = daemon.store.with(|connection| {
            Ok((
                storage::query(
                    connection,
                    "SELECT id,config,password_key FROM database_connections",
                    &[],
                ),
                storage::query(
                    connection,
                    "SELECT id,connection,sql,at FROM database_history",
                    &[],
                ),
            ))
        })?;
        let transaction = store.unchecked_transaction()?;
        for row in connections? {
            let (id, config, key): (String, String, Option<String>) =
                serde_json::from_value(Value::Array(row))?;
            if let Some(key) = &key {
                daemon.store.kv_set(&self.owner_key(key)?, "true")?;
            }
            transaction.execute("INSERT OR IGNORE INTO database_connections(id,config,password_key) VALUES(?1,?2,?3)", rusqlite::params![id, config, key])?;
        }
        for row in history? {
            let (id, connection, sql, at): (i64, String, String, String) =
                serde_json::from_value(Value::Array(row))?;
            transaction.execute(
                "INSERT OR IGNORE INTO database_history(id,connection,sql,at) VALUES(?1,?2,?3,?4)",
                rusqlite::params![id, connection, sql, at],
            )?;
        }
        transaction.execute(
            "INSERT INTO extension_meta(key,value) VALUES('legacy_imported','true')",
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    async fn handle(&self, call: ExtensionHostCall) -> Result<Value> {
        if !self.active.load(Ordering::Acquire)
            && matches!(
                call,
                ExtensionHostCall::CredentialsCreate(_)
                    | ExtensionHostCall::CredentialsDelete(_)
                    | ExtensionHostCall::IdentitySignIn(_)
                    | ExtensionHostCall::IdentitySignOut(_)
            )
        {
            return Err(Error::Unavailable(
                "An extension version that is starting cannot change credentials or sign-in; do it once the extension runs".into(),
            ));
        }
        let daemon = self.daemon()?;
        match call {
            ExtensionHostCall::StorageBootstrap(params) => {
                self.store.lock().unwrap().execute_batch(&params.schema)?;
                Ok(Value::Null)
            }
            ExtensionHostCall::StorageImportLegacy(_) => {
                self.import_legacy()?;
                Ok(Value::Null)
            }
            ExtensionHostCall::StorageQuery(params) => serde_json::to_value(storage::query(
                &self.store.lock().unwrap(),
                &params.sql,
                &params.params,
            )?)
            .map_err(Into::into),
            ExtensionHostCall::StorageBatch(statements) => {
                storage::batch(&self.store.lock().unwrap(), &statements)?;
                Ok(Value::Null)
            }
            ExtensionHostCall::WorkspaceGet(params) => {
                crate::rpc::to_value(daemon.workspaces.get(&params.workspace)?)
            }
            ExtensionHostCall::WorkspaceList(params) => {
                self.office()?;
                crate::rpc::to_value(
                    daemon
                        .workspaces
                        .list(params.include_archived, params.parent.as_deref())?,
                )
            }
            ExtensionHostCall::ChatList(params) => {
                self.office()?;
                crate::rpc::to_value(
                    daemon
                        .chats
                        .list(&params.workspace, params.include_closed)?,
                )
            }
            ExtensionHostCall::WorkspaceMoveHouse(params) => {
                self.office()?;
                crate::rpc::to_value(daemon.workspaces.move_house(
                    &params.workspace,
                    params.before.as_deref(),
                    |a, b| {
                        (a.house_position, std::cmp::Reverse(&a.created_at))
                            .cmp(&(b.house_position, std::cmp::Reverse(&b.created_at)))
                    },
                )?)
            }
            ExtensionHostCall::KvGet(params) => {
                self.office_key(&params.key, false)?;
                crate::rpc::to_value(daemon.store.kv_get(&params.key)?)
            }
            ExtensionHostCall::KvSet(params) => {
                self.office_key(&params.key, true)?;
                daemon.store.kv_set(&params.key, &params.value)?;
                Ok(Value::Null)
            }
            ExtensionHostCall::KvDelete(params) => {
                self.office_key(&params.key, true)?;
                daemon.store.kv_delete(&params.key)?;
                Ok(Value::Null)
            }
            ExtensionHostCall::OfficeSettings(_) => {
                self.office()?;
                let settings = daemon.settings.get().office.clone();
                crate::rpc::to_value(
                    chauffeur_protocol::extension_host::ExtensionOfficeSettings {
                        enabled: settings.enabled,
                        url: settings.url,
                        dev_email: settings.dev_email,
                    },
                )
            }
            ExtensionHostCall::ExtensionEnabled(_) => {
                crate::rpc::to_value(super::enabled(&daemon, self.id)?)
            }
            ExtensionHostCall::IdentitySignIn(_) => {
                self.office()?;
                crate::rpc::to_value(crate::identity::sign_in(&daemon).await?)
            }
            ExtensionHostCall::IdentitySignOut(_) => {
                self.office()?;
                crate::rpc::to_value(crate::identity::sign_out(&daemon)?)
            }
            ExtensionHostCall::IdentityToken(_) => {
                self.office()?;
                let settings = daemon.settings.get().office.clone();
                let token = if !settings.dev_email.trim().is_empty() {
                    chauffeur_protocol::extension_host::ExtensionIdentityToken {
                        id_token: None,
                        dev_email: Some(settings.dev_email.trim().to_lowercase()),
                    }
                } else {
                    let client = crate::google::Client::from_settings(&settings).ok_or_else(|| Error::Unavailable("Google sign-in is not configured; configure Chauffeur's sign-in settings".into()))?;
                    let refresh = Secrets::for_paths(&daemon.paths)
                        .read(&crate::google::REFRESH_TOKEN)
                        .ok_or_else(|| {
                            Error::Unavailable("Chauffeur sign-in expired; sign in again".into())
                        })?;
                    let token = client.id_token(&refresh).await.map_err(|error| {
                        Error::Unavailable(format!(
                            "Could not refresh Chauffeur sign-in: {error}; sign in again"
                        ))
                    })?;
                    chauffeur_protocol::extension_host::ExtensionIdentityToken {
                        id_token: Some(token),
                        dev_email: None,
                    }
                };
                crate::rpc::to_value(token)
            }
            ExtensionHostCall::ShellEnvironment(_) => {
                self.office()?;
                crate::rpc::to_value(daemon.shell_env.vars())
            }
            ExtensionHostCall::CredentialsRead(params) => {
                self.credential(&params.key)?;
                let paths = daemon.paths.clone();
                let value = tokio::task::spawn_blocking(move || {
                    Secrets::for_paths(&paths).read_protected(&password_name(&params.key))
                })
                .await
                .map_err(|error| Error::Internal(error.into()))?
                .map_err(credential_error)?;
                Ok(json!(value))
            }
            ExtensionHostCall::CredentialsCreate(params) => {
                let owner = self.owner_key(&params.key)?;
                let paths = daemon.paths.clone();
                let key = params.key.clone();
                tokio::task::spawn_blocking(move || {
                    Secrets::for_paths(&paths).create_protected(&password_name(&key), &params.value)
                })
                .await
                .map_err(|error| Error::Internal(error.into()))?
                .map_err(credential_error)?;
                if let Err(error) = daemon.store.kv_set(&owner, "true") {
                    Secrets::for_paths(&daemon.paths)
                        .delete(&password_name(&params.key))
                        .map_err(credential_error)?;
                    return Err(error);
                }
                Ok(Value::Null)
            }
            ExtensionHostCall::CredentialsDelete(params) => {
                self.credential(&params.key)?;
                let owner = self.owner_key(&params.key)?;
                let paths = daemon.paths.clone();
                tokio::task::spawn_blocking(move || {
                    Secrets::for_paths(&paths).delete(&password_name(&params.key))
                })
                .await
                .map_err(|error| Error::Internal(error.into()))?
                .map_err(credential_error)?;
                daemon.store.kv_delete(&owner)?;
                Ok(Value::Null)
            }
        }
    }
}

fn password_name(key: &str) -> SecretName<'_> {
    SecretName {
        service: key,
        file: key,
        label: "Chauffeur PostgreSQL password",
    }
}
fn credential_error(error: anyhow::Error) -> Error {
    crate::secrets::protected_failure(|| {
        Error::Unavailable(
            "Could not access the database password; unlock the system keyring and try again"
                .into(),
        )
    })(error)
}

impl super::process::Services for Broker {
    fn activate(&self) {
        self.active.store(true, Ordering::Release);
    }
    fn call(&self, method: String, params: Value) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            let call = serde_json::from_value(json!({"method": method, "params": params}))
                .map_err(|_| {
                    Error::BadRequest(
                        "This extension requires an unsupported host callback; update Chauffeur"
                            .into(),
                    )
                })?;
            self.handle(call).await
        })
    }
    fn event(&self, event: Value) -> Result<()> {
        use chauffeur_protocol::events::Event;
        let daemon = self.daemon()?;
        // Preserve the opaque payload even if a newer package adds an event Core does not know.
        let payload =
            if event.get("scope").and_then(Value::as_str) == Some(super::packages::slug(self.id)) {
                event.get("event").cloned().unwrap_or(event.clone())
            } else {
                event.clone()
            };
        daemon.events.publish(Event::Extension {
            id: self.id,
            event: payload,
        });
        if self.id == ExtensionId::Office
            && let Ok(Event::Office { event }) = serde_json::from_value(event)
        {
            daemon.events.publish(Event::Office { event });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daemon(dir: &Path) -> Arc<Daemon> {
        let paths =
            crate::paths::Paths::new(dir.join("home"), dir.join("workspaces"), dir.join("repos"));
        paths.ensure().unwrap();
        std::fs::write(&paths.settings, "linear_enabled = false\n").unwrap();
        Daemon::open(paths).unwrap()
    }

    #[tokio::test]
    async fn a_starting_engine_cannot_change_credentials_or_sign_in_until_activated() {
        use super::super::process::Services;
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        let key = uuid::Uuid::now_v7().to_string();
        let owner = format!("extension_database_credential_{key}");
        daemon.store.kv_set(&owner, "true").unwrap();
        let database = Broker::open_preflight(
            Arc::downgrade(&daemon),
            ExtensionId::Database,
            &dir.path().join("database"),
        )
        .unwrap();
        for (method, params) in [
            ("credentials.delete", json!({ "key": key })),
            (
                "credentials.create",
                json!({ "key": key, "value": "secret" }),
            ),
        ] {
            let refused = database.call(method.into(), params).await.unwrap_err();
            assert!(
                refused.to_string().contains("starting"),
                "{method}: {refused}"
            );
        }
        assert_eq!(
            daemon.store.kv_get(&owner).unwrap().as_deref(),
            Some("true")
        );
        let office = Broker::open_preflight(
            Arc::downgrade(&daemon),
            ExtensionId::Office,
            &dir.path().join("office"),
        )
        .unwrap();
        for method in ["identity.sign_in", "identity.sign_out"] {
            let refused = office.call(method.into(), json!({})).await.unwrap_err();
            assert!(
                refused.to_string().contains("starting"),
                "{method}: {refused}"
            );
        }
        // Its own state and preferences stay open: a failed switch restores them.
        office
            .call(
                "kv.set".into(),
                json!({ "key": "office_skin", "value": "candidate" }),
            )
            .await
            .unwrap();
        database.activate();
        database
            .call("credentials.delete".into(), json!({ "key": key }))
            .await
            .unwrap();
        assert!(daemon.store.kv_get(&owner).unwrap().is_none());
    }
    #[test]
    fn extension_sql_cannot_attach_or_write_another_database_file() {
        let mut database = Connection::open_in_memory().unwrap();
        database.authorizer(Some(authorize)).unwrap();
        assert!(
            database
                .execute_batch("ATTACH ':memory:' AS other")
                .is_err()
        );
        assert!(database.execute_batch("PRAGMA writable_schema=ON").is_err());
        database
            .execute_batch("CREATE TABLE example(value TEXT); INSERT INTO example VALUES('ok');")
            .unwrap();
        assert_eq!(
            storage::query(&database, "SELECT value FROM example", &[]).unwrap(),
            vec![vec![json!("ok")]]
        );
    }

    fn fixture() -> (tempfile::TempDir, Arc<Daemon>) {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(
            dir.path().join("home"),
            dir.path().join("workspaces"),
            dir.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(
            &paths.settings,
            "github_org = \"\"\ngithub_sources = false\nclone_roots = []\nlinear_enabled = false\n",
        )
        .unwrap();
        (dir, Daemon::open(paths).unwrap())
    }

    #[tokio::test]
    async fn legacy_database_records_import_once_with_their_credential_ownership() {
        let (_dir, daemon) = fixture();
        let key = uuid::Uuid::new_v4().to_string();
        daemon.store.with(|connection| {
            connection.execute("INSERT INTO database_connections(id,config,password_key) VALUES('old','original',?1)", [&key])?;
            connection.execute("INSERT INTO database_history(id,connection,sql,at) VALUES(42,'old','SELECT 1','fixture')", [])?;
            Ok(())
        }).unwrap();
        let broker = Broker::open(
            Arc::downgrade(&daemon),
            ExtensionId::Database,
            &daemon.paths.home.join("extension-data"),
        )
        .unwrap();
        broker.handle(ExtensionHostCall::StorageBootstrap(chauffeur_protocol::extension_host::ExtensionSchema { schema: "CREATE TABLE database_connections(id TEXT PRIMARY KEY,config TEXT,password_key TEXT); CREATE TABLE database_history(id INTEGER PRIMARY KEY,connection TEXT,sql TEXT,at TEXT); CREATE TABLE extension_meta(key TEXT PRIMARY KEY,value TEXT);".into() })).await.unwrap();
        broker.import_legacy().unwrap();
        daemon
            .store
            .with(|connection| {
                connection.execute("UPDATE database_connections SET config='changed'", [])?;
                Ok(())
            })
            .unwrap();
        broker.import_legacy().unwrap();
        let store = broker.store.lock().unwrap();
        assert_eq!(
            storage::query(
                &store,
                "SELECT id,config,password_key FROM database_connections",
                &[]
            )
            .unwrap(),
            vec![vec![json!("old"), json!("original"), json!(key)]]
        );
        assert_eq!(
            storage::query(&store, "SELECT id,sql FROM database_history", &[]).unwrap(),
            vec![vec![json!(42), json!("SELECT 1")]]
        );
        assert_eq!(
            daemon
                .store
                .kv_get(&format!("extension_database_credential_{key}"))
                .unwrap()
                .as_deref(),
            Some("true")
        );
    }

    #[tokio::test]
    async fn a_host_callback_cannot_read_another_extensions_capabilities() {
        let (_dir, daemon) = fixture();
        let database = Broker::open(
            Arc::downgrade(&daemon),
            ExtensionId::Database,
            &daemon.paths.home.join("database"),
        )
        .unwrap();
        let office = Broker::open(
            Arc::downgrade(&daemon),
            ExtensionId::Office,
            &daemon.paths.home.join("office"),
        )
        .unwrap();
        assert!(database.office_key("office_skin", true).is_err());
        assert!(office.owner_key(&uuid::Uuid::new_v4().to_string()).is_err());
        assert!(office.office_key("office_email", true).is_err());
        assert!(
            database
                .credential(&uuid::Uuid::new_v4().to_string())
                .is_err()
        );
    }

    #[tokio::test]
    async fn an_unknown_office_event_keeps_its_fields_without_a_core_release() {
        use super::super::process::Services;
        let (_dir, daemon) = fixture();
        let broker = Broker::open(
            Arc::downgrade(&daemon),
            ExtensionId::Office,
            &daemon.paths.home.join("office"),
        )
        .unwrap();
        let mut events = daemon.events.subscribe();
        let event = json!({"type":"future_feature","nested":{"new_field":17}});
        broker
            .event(json!({"scope":"office","event":event}))
            .unwrap();
        assert_eq!(
            events.recv().await.unwrap().event,
            chauffeur_protocol::events::Event::Extension {
                id: ExtensionId::Office,
                event
            }
        );
        assert!(events.try_recv().is_err());
    }
}
