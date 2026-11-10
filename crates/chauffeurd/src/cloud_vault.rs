//! The endpoint root's pin and the transfers' handoff state, in a database of their own,
//! independent of untrusted GCS objects. No credential, data key or recovery key is stored here.
//! This is not an execution lease.
use std::{path::PathBuf, time::Duration};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use crate::{Error, Result};

pub struct CloudVaultHeads {
    path: PathBuf,
}

impl CloudVaultHeads {
    pub fn new(home: &std::path::Path) -> Self {
        Self {
            path: home.join("cloud-vault-heads.sqlite"),
        }
    }

    // Separate FULL-synchronous database: do not silently inherit the general store's NORMAL
    // setting or restore this pointer from encrypted-object listings/ordinary workspace backups.
    pub(super) fn connect(&self) -> Result<Connection> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path)?;
        let conn = Connection::open(&self.path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "synchronous", "EXTRA")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS endpoint_root (id INTEGER PRIMARY KEY CHECK(id=1), digest TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS archive_preparation_keys (operation_id TEXT PRIMARY KEY,context TEXT NOT NULL,local_peer_id TEXT NOT NULL,digest TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS workspace_handoffs (workspace_id TEXT PRIMARY KEY, state TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS workspace_handoff_operations (operation_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, source_epoch INTEGER NOT NULL, revision INTEGER NOT NULL, state TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS workspace_handoff_revisions ON workspace_handoff_operations (workspace_id, revision);
        -- Left by the archive job pipeline and the credential vault, removed: an existing store
        -- loses them here.
        DROP TABLE IF EXISTS archive_jobs;
        DROP TABLE IF EXISTS archive_host_configurations;
        DROP TABLE IF EXISTS archive_transfer_approvals;
        DROP TABLE IF EXISTS archive_job_processes;
        DROP TABLE IF EXISTS heads;
        DROP TABLE IF EXISTS refresh_states;
        DROP TABLE IF EXISTS controller_profile;
        DROP TABLE IF EXISTS cloud_runtime_sessions;
        DROP TABLE IF EXISTS cloud_native_sessions;
        DROP TABLE IF EXISTS cloud_account_reviews;
        DROP TABLE IF EXISTS cloud_account_selection;",
        )?;
        Ok(conn)
    }

    pub(super) fn root_state(&self) -> Result<(Option<String>, bool)> {
        let conn = self.connect()?;
        let digest = conn
            .query_row("SELECT digest FROM endpoint_root WHERE id=1", [], |row| {
                row.get(0)
            })
            .optional()?;
        let has_heads: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM archive_preparation_keys) OR EXISTS(SELECT 1 FROM workspace_handoffs) OR EXISTS(SELECT 1 FROM workspace_handoff_operations)",
            [],
            |row| row.get(0),
        )?;
        Ok((digest, has_heads))
    }

    pub(super) fn pin_root(&self, digest: &str) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: Option<String> = tx
            .query_row("SELECT digest FROM endpoint_root WHERE id=1", [], |row| {
                row.get(0)
            })
            .optional()?;
        if let Some(previous) = previous {
            if previous != digest {
                return Err(Error::Unavailable(
                    "Vault root changed; recovery required".into(),
                ));
            }
        } else {
            let has_heads: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM archive_preparation_keys) OR EXISTS(SELECT 1 FROM workspace_handoffs) OR EXISTS(SELECT 1 FROM workspace_handoff_operations)",
                [],
                |row| row.get(0),
            )?;
            if has_heads {
                return Err(Error::Unavailable(
                    "Vault root anchor missing; recovery required".into(),
                ));
            }
            tx.execute(
                "INSERT INTO endpoint_root(id, digest) VALUES(1, ?1)",
                [digest],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_pin_survives_reopen_and_refuses_another_root() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudVaultHeads::new(temp.path());
        assert_eq!(store.root_state().unwrap(), (None, false));
        store.pin_root("first").unwrap();
        drop(store);
        let store = CloudVaultHeads::new(temp.path());
        assert_eq!(store.root_state().unwrap().0.as_deref(), Some("first"));
        store.pin_root("first").unwrap();
        assert!(store.pin_root("second").is_err());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&store.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn corrupt_database_is_not_treated_as_an_empty_store() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudVaultHeads::new(temp.path());
        std::fs::write(&store.path, b"not a sqlite database").unwrap();
        assert!(store.root_state().is_err());
        assert!(store.pin_root("first").is_err());
    }
}
