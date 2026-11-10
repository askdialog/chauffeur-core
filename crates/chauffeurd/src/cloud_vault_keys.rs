//! Stable root identity + recovery key, protected independently of encrypted objects.
//! Never initialize implicitly after an unreadable/missing secret or broken trust anchor.
use std::sync::Mutex;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{
    Error, Result,
    cloud_vault::CloudVaultHeads,
    secrets::{SecretName, Secrets, protected_failure},
};

const ROOT: SecretName<'static> = SecretName {
    service: "chauffeur-cloud-vault-root-v1",
    file: "cloud-vault-root-v1",
    label: "Chauffeur: cloud vault endpoint root",
};

/// Root material: never forward, log or persist it in SQLite or GCS.
pub struct CloudVaultEndpointMaterial {
    pub identity_seed: String,
    pub recovery_key: String,
}

pub struct CloudVaultKeys {
    secrets: Secrets,
    lock: Mutex<()>,
}
impl CloudVaultKeys {
    pub fn new(secrets: Secrets) -> Self {
        Self {
            secrets,
            lock: Mutex::new(()),
        }
    }
    pub fn initialize(&self, heads: &CloudVaultHeads) -> Result<CloudVaultEndpointMaterial> {
        let _guard = self.lock.lock().unwrap();
        self.load(heads, true)
    }
    pub fn read(&self, heads: &CloudVaultHeads) -> Result<CloudVaultEndpointMaterial> {
        let _guard = self.lock.lock().unwrap();
        self.load(heads, false)
    }
    /// Derives the local libp2p Ed25519 identity only from an existing protected endpoint root.
    pub fn peer_id(&self, heads: &CloudVaultHeads) -> Result<String> {
        let material = self.read(heads)?;
        let seed = Zeroizing::new(material.identity_seed);
        let _recovery = Zeroizing::new(material.recovery_key);
        let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(&*seed).map_err(|_| unavailable())?);
        let seed_bytes =
            Zeroizing::new(<[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| unavailable())?);
        let key = ed25519_dalek::SigningKey::from_bytes(&seed_bytes);
        // Ed25519 public-key protobuf inside an identity multihash (libp2p peer-id spec).
        // https://github.com/libp2p/specs/blob/master/peer-ids/peer-ids.md
        let mut identity = [0; 38];
        identity[..6].copy_from_slice(&[0, 36, 8, 1, 18, 32]);
        identity[6..].copy_from_slice(key.verifying_key().as_bytes());
        Ok(bs58::encode(identity).into_string())
    }
    fn load(
        &self,
        heads: &CloudVaultHeads,
        initialize: bool,
    ) -> Result<CloudVaultEndpointMaterial> {
        let (pin, has_heads) = heads.root_state()?;
        if pin.is_none() && (!initialize || has_heads) {
            return Err(unavailable());
        }
        let saved = self
            .secrets
            .read_protected(&ROOT)
            .map_err(protected_failure(unavailable))?
            .map(Zeroizing::new);
        let encoded = match saved {
            Some(value) => value,
            None if initialize && pin.is_none() => {
                let bytes = Zeroizing::new(rand::random::<[u8; 64]>());
                let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_slice()));
                self.secrets
                    .create_protected(&ROOT, &encoded)
                    .map_err(protected_failure(unavailable))?;
                encoded
            }
            None => return Err(unavailable()),
        };
        let bytes = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(encoded.as_bytes())
                .map_err(|_| unavailable())?,
        );
        if bytes.len() != 64 || URL_SAFE_NO_PAD.encode(&*bytes) != *encoded {
            return Err(unavailable());
        }
        let mut hash = Sha256::new();
        hash.update(b"chauffeur-cloud-endpoint-root-v1");
        hash.update(&*bytes);
        let digest = URL_SAFE_NO_PAD.encode(hash.finalize());
        if pin.as_ref().is_some_and(|pin| pin != &digest) {
            return Err(unavailable());
        }
        if pin.is_none() {
            heads.pin_root(&digest)?;
        }
        Ok(CloudVaultEndpointMaterial {
            identity_seed: URL_SAFE_NO_PAD.encode(&bytes[..32]),
            recovery_key: URL_SAFE_NO_PAD.encode(&bytes[32..]),
        })
    }
}
fn unavailable() -> Error {
    Error::Unavailable("Vault root unavailable or changed; explicit setup/recovery required".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    fn fixture() -> (tempfile::TempDir, Paths, CloudVaultHeads) {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::new(
            temp.path().join("home"),
            temp.path().join("workspaces"),
            temp.path().join("repos"),
        );
        paths.ensure().unwrap();
        let heads = CloudVaultHeads::new(&paths.home);
        (temp, paths, heads)
    }
    #[test]
    fn the_root_peer_is_the_libp2p_identity_of_its_seed() {
        let (_temp, paths, heads) = fixture();
        let secrets = Secrets::for_paths(&paths);
        secrets
            .create_protected(&ROOT, &URL_SAFE_NO_PAD.encode([7; 64]))
            .unwrap();
        let keys = CloudVaultKeys::new(secrets);
        assert!(keys.peer_id(&heads).is_err());
        keys.initialize(&heads).unwrap();
        // libp2p's peer ID of the Ed25519 key whose seed is 32 bytes of 7.
        assert_eq!(
            keys.peer_id(&heads).unwrap(),
            "12D3KooWRawPbxPtP1eZaJpumGnyWX2DcUyd3RQnydr3eAto4Az7"
        );
        std::fs::remove_file(paths.home.join("secrets").join(ROOT.file)).unwrap();
        assert!(keys.peer_id(&heads).is_err());
        assert!(!paths.home.join("secrets").join(ROOT.file).exists());
    }
    #[test]
    fn explicit_setup_survives_manager_restart_and_does_not_replace_keys() {
        let (_temp, paths, heads) = fixture();
        let keys = CloudVaultKeys::new(Secrets::for_paths(&paths));
        assert!(keys.read(&heads).is_err());
        let first = keys.initialize(&heads).unwrap();
        assert_ne!(first.identity_seed, first.recovery_key);
        drop(keys);
        let reopened = CloudVaultKeys::new(Secrets::for_paths(&paths));
        let next = reopened.read(&heads).unwrap();
        assert_eq!(first.identity_seed, next.identity_seed);
        assert_eq!(first.recovery_key, next.recovery_key);
        assert_eq!(
            reopened.initialize(&heads).unwrap().recovery_key,
            first.recovery_key
        );
        let file = paths.home.join("secrets").join(ROOT.file);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[test]
    fn missing_corrupt_or_changed_protected_material_never_regenerates() {
        let (_temp, paths, heads) = fixture();
        let keys = CloudVaultKeys::new(Secrets::for_paths(&paths));
        keys.initialize(&heads).unwrap();
        let file = paths.home.join("secrets").join(ROOT.file);
        let original = std::fs::read(&file).unwrap();
        for bytes in [
            b"malformed".to_vec(),
            URL_SAFE_NO_PAD.encode([7u8; 64]).into_bytes(),
        ] {
            std::fs::write(&file, &bytes).unwrap();
            assert!(keys.read(&heads).is_err());
            assert!(keys.initialize(&heads).is_err());
            assert_eq!(std::fs::read(&file).unwrap(), bytes);
        }
        std::fs::remove_file(&file).unwrap();
        assert!(keys.initialize(&heads).is_err());
        assert!(!file.exists());
        std::fs::write(&file, original).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(keys.read(&heads).is_ok());
    }
    #[test]
    fn interrupted_setup_can_only_adopt_the_same_existing_protected_root_explicitly() {
        let (_temp, paths, heads) = fixture();
        let secrets = Secrets::for_paths(&paths);
        let encoded = URL_SAFE_NO_PAD.encode([8u8; 64]);
        secrets.create_protected(&ROOT, &encoded).unwrap();
        assert!(
            secrets
                .create_protected(&ROOT, &URL_SAFE_NO_PAD.encode([9u8; 64]))
                .is_err()
        );
        let keys = CloudVaultKeys::new(secrets);
        assert!(keys.read(&heads).is_err());
        assert!(keys.initialize(&heads).is_ok());
        assert_eq!(
            std::fs::read_to_string(paths.home.join("secrets").join(ROOT.file)).unwrap(),
            encoded
        );
    }
    #[test]
    fn protected_store_io_errors_are_not_treated_as_missing_keys() {
        let (_temp, paths, heads) = fixture();
        let file = paths.home.join("secrets").join(ROOT.file);
        std::fs::create_dir_all(&file).unwrap();
        let keys = CloudVaultKeys::new(Secrets::for_paths(&paths));
        assert!(keys.initialize(&heads).is_err());
        assert!(file.is_dir());
    }
    #[test]
    fn a_missing_root_anchor_with_an_existing_handoff_blocks_setup() {
        let (_temp, paths, heads) = fixture();
        heads
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO workspace_handoffs(workspace_id, state) VALUES('w', '{}')",
                [],
            )
            .unwrap();
        let keys = CloudVaultKeys::new(Secrets::for_paths(&paths));
        assert!(keys.initialize(&heads).is_err());
        assert!(!paths.home.join("secrets").join(ROOT.file).exists());
    }
}
