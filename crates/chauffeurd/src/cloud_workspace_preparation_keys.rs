//! Source-only key reservation before encryption; never an archive head or execution grant.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chauffeur_protocol::sidecar::PackageTransferContext;
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    Error, Result,
    cloud_vault::CloudVaultHeads,
    cloud_vault_keys::CloudVaultKeys,
    cloud_workspace_archive::ArchiveKey,
    secrets::{SecretName, Secrets},
};

const MAX_ENCODED: usize = 8192;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u8,
    root_digest: String,
    context: PackageTransferContext,
    local_peer_id: String,
    key: String,
}
struct Pin {
    context: Option<String>,
    local: Option<String>,
    digest: Option<String>,
}
impl Drop for Saved {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}
fn refused() -> Error {
    Error::Unavailable("Archive preparation key unavailable or already reserved; retain recovery data and inspect its protected operation before retrying".into())
}
fn validate(context: &PackageTransferContext, local: &str) -> Result<()> {
    use crate::cloud_workspace_handoff::{digest, peer, uuid};
    if !uuid(&context.operation_id)
        || !uuid(&context.workspace_id)
        || ![
            &context.owner_peer_id,
            &context.controller_peer_id,
            &context.source_peer_id,
            &context.target_peer_id,
        ]
        .iter()
        .all(|p| peer(p))
        || context.owner_peer_id == context.controller_peer_id
        || context.source_peer_id == context.target_peer_id
        || context.source_peer_id == context.controller_peer_id
        || context.target_peer_id == context.controller_peer_id
        || local != context.source_peer_id
        || context.source_epoch == 0
        || context.source_epoch >= 9_007_199_254_740_991
        || !digest(&context.snapshot_digest)
    {
        return Err(refused());
    }
    Ok(())
}
fn root(heads: &CloudVaultHeads, keys: &CloudVaultKeys) -> Result<String> {
    let material = keys.read(heads).map_err(|_| refused())?;
    let _identity = Zeroizing::new(material.identity_seed);
    let _recovery = Zeroizing::new(material.recovery_key);
    heads.root_state()?.0.ok_or_else(refused)
}
fn digest(encoded: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"chauffeur-archive-preparation-key-v1");
    h.update(encoded);
    URL_SAFE_NO_PAD.encode(h.finalize())
}
fn name(operation: &str) -> String {
    format!("chauffeur-archive-preparation-v1-{operation}")
}
fn secret_name(name: &str) -> SecretName<'_> {
    SecretName {
        service: name,
        file: name,
        label: "Chauffeur: workspace archive preparation key",
    }
}
fn check_root(tx: &rusqlite::Transaction<'_>, selected: &str) -> Result<()> {
    let pinned: Option<String> = tx
        .query_row("SELECT digest FROM endpoint_root WHERE id=1", [], |r| {
            r.get(0)
        })
        .optional()?;
    if pinned.as_deref() != Some(selected) {
        return Err(refused());
    }
    Ok(())
}

pub struct PreparationKeys {
    secrets: Secrets,
}
impl PreparationKeys {
    pub fn new(secrets: Secrets) -> Self {
        Self { secrets }
    }
    /// Trusted callers supply the current authenticated snapshot-ready context. Reservation is
    /// exclusive even after interruption; use read for recovery, never regenerate or adopt a key.
    pub fn reserve(
        &self,
        heads: &CloudVaultHeads,
        keys: &CloudVaultKeys,
        context: &PackageTransferContext,
        local: &str,
    ) -> Result<ArchiveKey> {
        validate(context, local)?;
        let root_digest = root(heads, keys)?;
        let mut conn = heads.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_root(&tx, &root_digest)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM archive_preparation_keys WHERE operation_id=?1)",
            [&context.operation_id],
            |r| r.get(0),
        )?;
        let name = name(&context.operation_id);
        if exists
            || self
                .secrets
                .read_protected(&secret_name(&name))
                .map_err(|_| refused())?
                .map(Zeroizing::new)
                .is_some()
        {
            return Err(refused());
        }
        let key = ArchiveKey::generate();
        let saved = Saved {
            version: 1,
            root_digest,
            context: context.clone(),
            local_peer_id: local.into(),
            key: URL_SAFE_NO_PAD.encode(key.expose_bytes()),
        };
        let bytes = Zeroizing::new(serde_json::to_vec(&saved).map_err(|_| refused())?);
        let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(&*bytes));
        if encoded.len() > MAX_ENCODED {
            return Err(refused());
        }
        self.secrets
            .create_protected(&secret_name(&name), &encoded)
            .map_err(|_| refused())?;
        tx.execute("INSERT INTO archive_preparation_keys(operation_id,context,local_peer_id,digest) VALUES(?1,?2,?3,?4)", params![context.operation_id, serde_json::to_string(context).map_err(|_| refused())?, local, digest(&encoded)])?;
        tx.commit()?;
        Ok(key)
    }
    /// Returns only an exact, rooted, pinned reservation. Missing halves remain for recovery.
    pub fn read(
        &self,
        heads: &CloudVaultHeads,
        keys: &CloudVaultKeys,
        context: &PackageTransferContext,
        local: &str,
    ) -> Result<ArchiveKey> {
        validate(context, local)?;
        let selected_root = root(heads, keys)?;
        let mut conn = heads.connect()?;
        let tx = conn.transaction()?;
        check_root(&tx, &selected_root)?;
        let pin = tx.query_row("SELECT CASE WHEN length(context)<=4096 THEN context END,CASE WHEN length(local_peer_id)<=128 THEN local_peer_id END,CASE WHEN length(digest)=43 THEN digest END FROM archive_preparation_keys WHERE operation_id=?1", [&context.operation_id], |r| Ok(Pin { context: r.get(0)?, local: r.get(1)?, digest: r.get(2)? })).optional()?.ok_or_else(refused)?;
        let (Some(pinned_context), Some(pinned_local), Some(pinned_digest)) =
            (pin.context, pin.local, pin.digest)
        else {
            return Err(refused());
        };
        if pinned_context != serde_json::to_string(context).map_err(|_| refused())?
            || pinned_local != local
        {
            return Err(refused());
        }
        let encoded = self
            .secrets
            .read_protected(&secret_name(&name(&context.operation_id)))
            .map_err(|_| refused())?
            .map(Zeroizing::new)
            .ok_or_else(refused)?;
        if encoded.len() > MAX_ENCODED || digest(&encoded) != pinned_digest {
            return Err(refused());
        }
        let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(&*encoded).map_err(|_| refused())?);
        let saved: Saved = serde_json::from_slice(&bytes).map_err(|_| refused())?;
        let canonical = Zeroizing::new(serde_json::to_vec(&saved).map_err(|_| refused())?);
        let canonical_encoding = Zeroizing::new(URL_SAFE_NO_PAD.encode(&*bytes));
        if saved.version != 1
            || saved.root_digest != selected_root
            || saved.context != *context
            || saved.local_peer_id != local
            || *canonical != *bytes
            || *canonical_encoding != *encoded
        {
            return Err(refused());
        }
        let key = Zeroizing::new(URL_SAFE_NO_PAD.decode(&saved.key).map_err(|_| refused())?);
        let canonical_key = Zeroizing::new(URL_SAFE_NO_PAD.encode(&*key));
        if key.len() != 32 || *canonical_key != saved.key {
            return Err(refused());
        }
        Ok(ArchiveKey::from_bytes(
            key.as_slice().try_into().map_err(|_| refused())?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn fixture() -> (
        tempfile::TempDir,
        Paths,
        CloudVaultHeads,
        CloudVaultKeys,
        PreparationKeys,
        PackageTransferContext,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::new(
            temp.path().join("home"),
            temp.path().join("workspaces"),
            temp.path().join("repos"),
        );
        paths.ensure().unwrap();
        let heads = CloudVaultHeads::new(&paths.home);
        let roots = CloudVaultKeys::new(Secrets::for_paths(&paths));
        roots.initialize(&heads).unwrap();
        let keys = PreparationKeys::new(Secrets::for_paths(&paths));
        let peer = |c: char| format!("12D3KooW{}", c.to_string().repeat(44));
        let context = PackageTransferContext {
            workspace_id: uuid::Uuid::now_v7().to_string(),
            operation_id: uuid::Uuid::now_v7().to_string(),
            owner_peer_id: peer('1'),
            controller_peer_id: peer('2'),
            source_peer_id: peer('1'),
            target_peer_id: peer('3'),
            source_epoch: 1,
            snapshot_digest: URL_SAFE_NO_PAD.encode([3; 32]),
            backup: false,
        };
        (temp, paths, heads, roots, keys, context)
    }
    #[test]
    fn a_reserved_key_survives_restart_without_allowing_another_reservation() {
        let (_temp, paths, heads, roots, keys, context) = fixture();
        let key = keys
            .reserve(&heads, &roots, &context, &context.source_peer_id)
            .unwrap();
        assert!(
            keys.reserve(&heads, &roots, &context, &context.source_peer_id)
                .is_err()
        );
        drop(keys);
        let reopened = PreparationKeys::new(Secrets::for_paths(&paths));
        let read = reopened
            .read(&heads, &roots, &context, &context.source_peer_id)
            .unwrap();
        assert_eq!(key.expose_bytes(), read.expose_bytes());
        assert_eq!(format!("{read:?}"), "ArchiveKey([REDACTED])");
        let metadata: (String, String, String) = heads
            .connect()
            .unwrap()
            .query_row(
                "SELECT context,local_peer_id,digest FROM archive_preparation_keys",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert!(!format!("{metadata:?}").contains(&URL_SAFE_NO_PAD.encode(key.expose_bytes())));
    }
    #[test]
    fn independent_managers_cannot_reserve_two_keys_for_one_operation() {
        let (_temp, paths, heads, roots, _keys, context) = fixture();
        let other = PreparationKeys::new(Secrets::for_paths(&paths));
        let first = PreparationKeys::new(Secrets::for_paths(&paths));
        std::thread::scope(|scope| {
            let a =
                scope.spawn(|| first.reserve(&heads, &roots, &context, &context.source_peer_id));
            let b =
                scope.spawn(|| other.reserve(&heads, &roots, &context, &context.source_peer_id));
            let a = a.join().unwrap();
            let b = b.join().unwrap();
            assert_ne!(a.is_ok(), b.is_ok());
            let reserved = a.or(b).unwrap();
            assert_eq!(
                reserved.expose_bytes(),
                first
                    .read(&heads, &roots, &context, &context.source_peer_id)
                    .unwrap()
                    .expose_bytes()
            );
        });
    }
    #[test]
    fn a_different_context_or_destination_cannot_read_or_replace_the_source_key() {
        let (_temp, _paths, heads, roots, keys, context) = fixture();
        keys.reserve(&heads, &roots, &context, &context.source_peer_id)
            .unwrap();
        let mut variants = Vec::new();
        let mut changed = context.clone();
        changed.workspace_id = uuid::Uuid::now_v7().to_string();
        variants.push(changed);
        let mut changed = context.clone();
        changed.operation_id = uuid::Uuid::now_v7().to_string();
        variants.push(changed);
        let mut changed = context.clone();
        changed.source_epoch += 1;
        variants.push(changed);
        let mut changed = context.clone();
        changed.snapshot_digest = URL_SAFE_NO_PAD.encode([4; 32]);
        variants.push(changed);
        let mut changed = context.clone();
        std::mem::swap(&mut changed.source_peer_id, &mut changed.target_peer_id);
        variants.push(changed);
        for changed in variants {
            assert!(
                keys.read(&heads, &roots, &changed, &context.source_peer_id)
                    .is_err()
            );
        }
        assert!(
            keys.read(&heads, &roots, &context, &context.target_peer_id)
                .is_err()
        );
        let mut invalid = context.clone();
        invalid.operation_id = "../escape".into();
        assert!(
            keys.reserve(&heads, &roots, &invalid, &context.source_peer_id)
                .is_err()
        );
    }
    #[test]
    fn missing_or_corrupt_halves_are_never_regenerated_or_adopted() {
        for mutation in 0..5 {
            let (_temp, paths, heads, roots, keys, context) = fixture();
            keys.reserve(&heads, &roots, &context, &context.source_peer_id)
                .unwrap();
            let secret = paths.home.join("secrets").join(name(&context.operation_id));
            match mutation {
                0 => {
                    std::fs::remove_file(&secret).unwrap();
                }
                1 => {
                    std::fs::write(&secret, "corrupt").unwrap();
                }
                2 => {
                    heads
                        .connect()
                        .unwrap()
                        .execute("DELETE FROM archive_preparation_keys", [])
                        .unwrap();
                }
                3 => {
                    heads
                        .connect()
                        .unwrap()
                        .execute("UPDATE archive_preparation_keys SET digest='corrupt'", [])
                        .unwrap();
                }
                _ => {
                    heads
                        .connect()
                        .unwrap()
                        .execute("UPDATE archive_preparation_keys SET context='{}'", [])
                        .unwrap();
                }
            }
            assert!(
                keys.read(&heads, &roots, &context, &context.source_peer_id)
                    .is_err()
            );
            assert!(
                keys.reserve(&heads, &roots, &context, &context.source_peer_id)
                    .is_err()
            );
        }
    }
    #[test]
    fn a_missing_root_cannot_be_initialized_over_a_reservation() {
        let (_temp, paths, heads, roots, keys, context) = fixture();
        keys.reserve(&heads, &roots, &context, &context.source_peer_id)
            .unwrap();
        heads
            .connect()
            .unwrap()
            .execute("DELETE FROM endpoint_root", [])
            .unwrap();
        assert!(roots.initialize(&heads).is_err());
        assert!(heads.pin_root("replacement").is_err());
        assert!(
            keys.read(&heads, &roots, &context, &context.source_peer_id)
                .is_err()
        );
        assert!(
            keys.reserve(&heads, &roots, &context, &context.source_peer_id)
                .is_err()
        );
        assert!(
            paths
                .home
                .join("secrets")
                .join(name(&context.operation_id))
                .is_file()
        );
    }
    #[test]
    fn unprotected_files_refuse_without_exposing_or_deleting_the_key() {
        for mutation in 0..3 {
            let (_temp, paths, heads, roots, keys, context) = fixture();
            let key = keys
                .reserve(&heads, &roots, &context, &context.source_peer_id)
                .unwrap();
            let path = paths.home.join("secrets").join(name(&context.operation_id));
            let retained = paths.home.join("retained");
            match mutation {
                0 => {
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap()
                }
                1 => {
                    std::fs::rename(&path, &retained).unwrap();
                    symlink(&retained, &path).unwrap();
                }
                _ => std::fs::hard_link(&path, &retained).unwrap(),
            }
            let error = keys
                .read(&heads, &roots, &context, &context.source_peer_id)
                .err()
                .unwrap();
            assert!(!format!("{error:?}").contains(&URL_SAFE_NO_PAD.encode(key.expose_bytes())));
            assert!(
                keys.reserve(&heads, &roots, &context, &context.source_peer_id)
                    .is_err()
            );
            assert!(path.exists());
        }
    }
}
