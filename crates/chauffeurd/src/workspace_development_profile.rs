//! Daemon-owned declarations, never executable configuration or permission to stop a service.
use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use chauffeur_protocol::dev_services::ComposeService;

use crate::{Daemon, Error, Result, store::Store, workspace_service_stop::SelectedService};

const MAX_BYTES: usize = 256 * 1024;
const MAX_REVISION: u64 = 9_007_199_254_740_991;
/// A preview's place in a port block, bounded as blocks are.
pub const MAX_PREVIEW_OFFSET: u16 = 1024;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub version: u8,
    pub workspace_id: String,
    pub linux_setup: Vec<String>,
    pub linux_startup: Vec<String>,
    pub services: Vec<Service>,
    pub preview_ports: Vec<PreviewPort>,
    /// Empty declarations keep the original v1 canonical encoding and trusted digests unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub postgres_exports: Vec<PostgresExport>,
}

impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Profile")
            .field("version", &self.version)
            .field("workspace_id", &self.workspace_id)
            .field(
                "linux_setup",
                &format_args!("[{} redacted commands]", self.linux_setup.len()),
            )
            .field(
                "linux_startup",
                &format_args!("[{} redacted commands]", self.linux_startup.len()),
            )
            .field("services", &self.services)
            .field("preview_ports", &self.preview_ports)
            .field("postgres_exports", &self.postgres_exports)
            .finish()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub name: String,
    pub required: bool,
    pub docker: DockerBinding,
    pub runtime_memory: Disposition,
    pub writable_layer: Disposition,
    pub mounts: BTreeMap<String, Disposition>,
    /// The compose file and service it was started from, which a destination starts again.
    /// Absent ones keep earlier profiles' canonical encoding and digests unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose: Option<ComposeService>,
    /// Its image as named (`postgres:16`), pulled again on a destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DockerBinding {
    pub socket: String,
    pub engine_id: String,
    pub container: SelectedService,
}

/// Preserve remains a valid declaration even when its export adapter is unsupported.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Disposition {
    Reconstructible,
    Preserve {
        data_kind: MeaningfulKind,
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MeaningfulKind {
    Database,
    Files,
    Tasks,
    Other,
}

/// A web preview: a port of the workspace's block, by its place in it, so that it names the
/// same service on every machine the workspace runs on, whatever block it has there.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PreviewPort {
    pub name: String,
    pub offset: u16,
}

/// One explicit database export; it neither covers cluster state nor authorizes service stops.
/// Export adapters retain database ownership and ACLs rather than silently stripping them.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PostgresExport {
    pub name: String,
    pub service: String,
    pub database: String,
    pub username: String,
    pub socket_dir: String,
    pub port: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileHead {
    pub revision: u64,
    pub digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StoredProfile {
    pub head: ProfileHead,
    pub profile: Profile,
}

fn invalid() -> Error {
    Error::BadRequest("Development profile is invalid or exceeds its limits; declare every service resource and preview explicitly".into())
}
fn changed() -> Error {
    Error::Conflict("Development profile changed or its history is incomplete; reload its retained declaration before editing".into())
}
fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.contains('\0')
}
fn path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 8192
        && !value.contains('\0')
        && !value.split('/').any(|p| matches!(p, "." | ".."))
}
fn hex_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
/// A compose file inside the workspace, named relative to its folder, and a compose service.
pub(crate) fn compose(value: &ComposeService) -> bool {
    // Compose's own rule for project names, which sets no length (the profile's size bounds it);
    // empty in declarations saved before it was recorded.
    (value.project.is_empty()
        || value
            .project
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && value
                .project
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b)))
        && !value.file.is_empty()
        && value.file.len() <= 4096
        && !value.file.starts_with('/')
        && !value.file.contains(['\\', '\0'])
        && value.file.split('/').all(|p| !matches!(p, "" | "." | ".."))
        && !value.service.is_empty()
        && value.service.len() <= 128
        && value
            .service
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && value
            .service
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
/// An image as `docker run` names it, without spaces or controls.
pub(crate) fn image_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.starts_with('-')
        && value.bytes().all(|b| b.is_ascii_graphic())
}
fn disposition(value: &Disposition) -> Result<()> {
    if let Disposition::Preserve { reason, .. } = value
        && !text(reason, 4096)
    {
        return Err(invalid());
    }
    Ok(())
}
pub(crate) fn database_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
pub(crate) fn postgres_endpoint(socket_dir: &str, port: u16) -> bool {
    path(socket_dir)
        && !socket_dir.contains(',')
        && !socket_dir.bytes().any(|b| b.is_ascii_control())
        && port != 0
}
fn preserves_database(value: &Disposition) -> bool {
    matches!(
        value,
        Disposition::Preserve {
            data_kind: MeaningfulKind::Database,
            ..
        }
    )
}
impl Profile {
    pub fn validate(&self) -> Result<()> {
        if !((self.version == 1 && self.postgres_exports.is_empty())
            || (self.version == 2 && !self.postgres_exports.is_empty()))
            || !crate::cloud_workspace_handoff::uuid(&self.workspace_id)
            || self.services.len() > 32
            || self.preview_ports.len() > 64
            || self.postgres_exports.len() > 32
            || [&self.linux_setup, &self.linux_startup]
                .iter()
                .any(|commands| commands.len() > 64 || commands.iter().any(|c| !text(c, 8192)))
        {
            return Err(invalid());
        }
        let mut names = BTreeSet::new();
        let mut containers = BTreeSet::new();
        for service in &self.services {
            let docker = &service.docker;
            let pin = &docker.container;
            if !name(&service.name)
                || !names.insert(service.name.as_str())
                || !path(&docker.socket)
                || docker.engine_id.is_empty()
                || docker.engine_id.len() > 128
                || !docker
                    .engine_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-:".contains(&b))
                || !hex_id(&pin.id)
                || !pin.image.strip_prefix("sha256:").is_some_and(hex_id)
                || pin.created.len() > 128
                || chrono::DateTime::parse_from_rfc3339(&pin.created).is_err()
                || !containers.insert((&docker.socket, &docker.engine_id, &pin.id))
                || pin.mounts.len() > 128
                || service.mounts.len() > 128
            {
                return Err(invalid());
            }
            if service.compose.as_ref().is_some_and(|c| !compose(c))
                || service
                    .image
                    .as_ref()
                    .is_some_and(|image| !image_reference(image))
            {
                return Err(invalid());
            }
            disposition(&service.runtime_memory)?;
            disposition(&service.writable_layer)?;
            let mut mounts = pin.mounts.clone();
            mounts.sort_by(|a, b| a.destination.cmp(&b.destination));
            if mounts != pin.mounts
                || URL_SAFE_NO_PAD.encode(Sha256::digest(serde_json::to_vec(&mounts)?))
                    != pin.mounts_digest
            {
                return Err(invalid());
            }
            let mut destinations = BTreeSet::new();
            for mount in &mounts {
                if !path(&mount.destination)
                    || !destinations.insert(mount.destination.as_str())
                    || !name(&mount.kind)
                    || mount.source.len() > 8192
                    || (!mount.source.is_empty() && !path(&mount.source))
                    || [&mount.name, &mount.driver, &mount.mode, &mount.propagation]
                        .iter()
                        .any(|v| v.len() > 256 || v.contains('\0'))
                {
                    return Err(invalid());
                }
                disposition(service.mounts.get(&mount.destination).ok_or_else(invalid)?)?;
            }
            if service
                .mounts
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != destinations
            {
                return Err(invalid());
            }
        }
        let mut offsets = BTreeSet::new();
        let mut previews = BTreeSet::new();
        for preview in &self.preview_ports {
            if !name(&preview.name)
                || !previews.insert(&preview.name)
                || preview.offset >= MAX_PREVIEW_OFFSET
                || !offsets.insert(preview.offset)
            {
                return Err(invalid());
            }
        }
        let mut exports = BTreeSet::new();
        let mut databases = BTreeSet::new();
        for export in &self.postgres_exports {
            let service = self
                .services
                .iter()
                .find(|s| s.name == export.service)
                .ok_or_else(invalid)?;
            if !name(&export.name)
                || !exports.insert(&export.name)
                || !databases.insert((&export.service, &export.database))
                || !database_identifier(&export.database)
                || !database_identifier(&export.username)
                || !postgres_endpoint(&export.socket_dir, export.port)
                || !std::iter::once(&service.runtime_memory)
                    .chain(std::iter::once(&service.writable_layer))
                    .chain(service.mounts.values())
                    .any(preserves_database)
            {
                return Err(invalid());
            }
        }
        if serde_json::to_vec(self)?.len() > MAX_BYTES {
            return Err(invalid());
        }
        Ok(())
    }
}

fn digest(revision: u64, profile: &Profile) -> Result<String> {
    let mut h = Sha256::new();
    h.update(b"chauffeur-workspace-development-profile-v1");
    h.update(serde_json::to_vec(&(revision, profile))?);
    Ok(URL_SAFE_NO_PAD.encode(h.finalize()))
}
impl StoredProfile {
    /// Scope and content verification only; an imported declaration is never a current head.
    pub fn validate(&self, workspace: &str) -> Result<()> {
        self.profile.validate()?;
        if self.profile.workspace_id != workspace
            || self.head.revision == 0
            || self.head.revision > MAX_REVISION
            || self.head.digest != digest(self.head.revision, &self.profile)?
        {
            return Err(changed());
        }
        Ok(())
    }
}

fn read(conn: &Connection, workspace: &str) -> Result<Option<StoredProfile>> {
    let head: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT revision,CASE WHEN length(CAST(digest AS BLOB))=43 THEN digest END
         FROM workspace_development_profile_heads WHERE workspace_id=?1",
            [workspace],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (count, minimum, maximum): (i64, Option<i64>, Option<i64>) = conn.query_row(
        "SELECT count(*),min(revision),max(revision) FROM workspace_development_profiles WHERE workspace_id=?1",
        [workspace],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let Some((revision, Some(digest))) = head else {
        return if head.is_none() && count == 0 {
            Ok(None)
        } else {
            Err(changed())
        };
    };
    if revision <= 0
        || revision as u64 > MAX_REVISION
        || minimum != Some(1)
        || maximum != Some(revision)
        || count != revision
    {
        return Err(changed());
    }
    let row: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT CASE WHEN length(CAST(digest AS BLOB))=43 THEN digest END,
            CASE WHEN length(CAST(profile AS BLOB))<=?3 THEN profile END
         FROM workspace_development_profiles WHERE workspace_id=?1 AND revision=?2",
            params![workspace, revision, MAX_BYTES as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((Some(saved_digest), Some(raw))) = row else {
        return Err(changed());
    };
    if saved_digest != digest {
        return Err(changed());
    }
    let profile = serde_json::from_str(&raw).map_err(|_| changed())?;
    let stored = StoredProfile {
        head: ProfileHead {
            revision: revision as u64,
            digest,
        },
        profile,
    };
    stored.validate(workspace).map_err(|_| changed())?;
    if serde_json::to_string(&stored.profile)? != raw {
        return Err(changed());
    }
    Ok(Some(stored))
}

/// Missing means neither a head nor retained versions exists; corruption is never an empty profile.
pub fn load(store: &Store, workspace: &str) -> Result<Option<StoredProfile>> {
    if !crate::cloud_workspace_handoff::uuid(workspace) {
        return Err(invalid());
    }
    store.durable(|conn| {
        let tx = conn.transaction()?;
        let value = read(&tx, workspace)?;
        tx.commit()?;
        Ok(value)
    })
}

fn persist(
    store: &Store,
    profile: Profile,
    expected: Option<&ProfileHead>,
) -> Result<StoredProfile> {
    profile.validate()?;
    store.durable(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state: Option<String> = tx.query_row("SELECT state FROM workspaces WHERE id=?1", [&profile.workspace_id], |r|r.get(0)).optional()?;
        if state.as_deref() != Some("ready") { return Err(changed()); }
        let current = read(&tx, &profile.workspace_id)?;
        if current.as_ref().map(|v| &v.head) != expected { return Err(changed()); }
        let revision = current.as_ref().map_or(1, |v| v.head.revision + 1);
        if revision > MAX_REVISION { return Err(changed()); }
        let stored = StoredProfile { head: ProfileHead { revision, digest: digest(revision, &profile)? }, profile };
        let workspace = &stored.profile.workspace_id;
        tx.execute("INSERT INTO workspace_development_profiles(workspace_id,revision,digest,profile) VALUES(?1,?2,?3,?4)", params![workspace,revision as i64,stored.head.digest,serde_json::to_string(&stored.profile)?])?;
        tx.execute("INSERT INTO workspace_development_profile_heads(workspace_id,revision,digest) VALUES(?1,?2,?3) ON CONFLICT(workspace_id) DO UPDATE SET revision=excluded.revision,digest=excluded.digest", params![workspace,revision as i64,stored.head.digest])?;
        tx.commit()?;
        Ok(stored)
    })
}

/// At a transfer's activation, the services started here for the workspace become its current
/// declaration, over whatever this machine declared before. Only the activation calls it.
pub(crate) fn install_arrived(store: &Store, profile: Profile) -> Result<StoredProfile> {
    let current = load(store, &profile.workspace_id)?;
    persist(store, profile, current.as_ref().map(|p| &p.head))
}

/// The admission spans validation and durable CAS; stopped/quarantined workspaces cannot edit it.
pub fn save(
    daemon: &Daemon,
    profile: Profile,
    expected: Option<&ProfileHead>,
) -> Result<StoredProfile> {
    let _admission = daemon.execution.admit(&profile.workspace_id)?;
    persist(&daemon.store, profile, expected)
}

#[cfg(test)]
pub(crate) fn save_fixture(
    store: &Store,
    profile: Profile,
    expected: Option<&ProfileHead>,
) -> Result<StoredProfile> {
    persist(store, profile, expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compose_reference_saved_without_its_project_keeps_its_encoding() {
        let legacy = r#"{"file":"shop/docker-compose.yml","service":"db"}"#;
        let compose: ComposeService = serde_json::from_str(legacy).unwrap();
        assert!(super::compose(&compose));
        assert_eq!(serde_json::to_string(&compose).unwrap(), legacy);
        let mut named = compose.clone();
        named.project = "Shop".into();
        assert!(
            !super::compose(&named),
            "compose project names are lowercase"
        );
    }

    #[test]
    fn original_profiles_keep_their_exact_canonical_encoding_and_trusted_digest() {
        let original = r#"{"version":1,"workspace_id":"00000000-0000-0000-0000-000000000001","linux_setup":["bun install --frozen-lockfile"],"linux_startup":["bun run dev"],"services":[],"preview_ports":[{"name":"web","offset":0}]}"#;
        let profile: Profile = serde_json::from_str(original).unwrap();
        profile.validate().unwrap();
        assert!(profile.postgres_exports.is_empty());
        assert_eq!(serde_json::to_string(&profile).unwrap(), original);
        let old_digest = "rR4wDUXn8--AhttRvC8kFqV0vZ8SCBwh6vR5h_7bA1U";
        assert_eq!(digest(1, &profile).unwrap(), old_digest);
        let store = Store::open_in_memory().unwrap();
        ready(&store, &profile.workspace_id);
        store
            .with(|c| {
                c.execute(
                    "INSERT INTO workspace_development_profiles VALUES(?1,1,?2,?3)",
                    params![profile.workspace_id, old_digest, original],
                )?;
                c.execute(
                    "INSERT INTO workspace_development_profile_heads VALUES(?1,1,?2)",
                    params![profile.workspace_id, old_digest],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            load(&store, &profile.workspace_id)
                .unwrap()
                .unwrap()
                .profile,
            profile
        );
    }

    fn postgres_profile() -> Profile {
        let mut profile = declaration();
        profile.version = 2;
        profile.services.push(service());
        profile.postgres_exports.push(PostgresExport {
            name: "main-database".into(),
            service: "database".into(),
            database: "app_database".into(),
            username: "app-owner".into(),
            socket_dir: "/var/run/postgresql".into(),
            port: 5432,
        });
        profile
    }

    #[test]
    fn explicit_database_exports_round_trip_without_reclassifying_meaningful_state() {
        let store = Store::open_in_memory().unwrap();
        let profile = postgres_profile();
        ready(&store, &profile.workspace_id);
        let saved = persist(&store, profile.clone(), None).unwrap();
        assert_eq!(load(&store, &profile.workspace_id).unwrap(), Some(saved));
        assert_eq!(
            serde_json::from_str::<Profile>(&serde_json::to_string(&profile).unwrap()).unwrap(),
            profile
        );
        assert!(matches!(
            profile.services[0].mounts["/data"],
            Disposition::Preserve {
                data_kind: MeaningfulKind::Database,
                ..
            }
        ));
        for resource in 0..2 {
            let mut declared = profile.clone();
            let data = declared.services[0]
                .mounts
                .insert("/data".into(), Disposition::Reconstructible)
                .unwrap();
            if resource == 0 {
                declared.services[0].runtime_memory = data;
            } else {
                declared.services[0].writable_layer = data;
            }
            declared.validate().unwrap();
        }
    }

    #[test]
    fn export_declarations_refuse_ambiguous_connections_missing_classifications_and_version_mismatches()
     {
        let profile = postgres_profile();
        for mutation in 0..19 {
            let mut invalid = profile.clone();
            match mutation {
                0 => invalid.version = 1,
                1 => invalid.postgres_exports.clear(),
                2 => invalid.postgres_exports[0].service = "missing".into(),
                3 => invalid.postgres_exports[0].database = "postgresql://host/database".into(),
                4 => invalid.postgres_exports[0].database = "host=elsewhere".into(),
                5 => invalid.postgres_exports[0].database = "--other-option".into(),
                6 => invalid.postgres_exports[0].username = "-option".into(),
                7 => invalid.postgres_exports[0].database = "a".repeat(64),
                8 => invalid.postgres_exports[0].username.clear(),
                9 => invalid.postgres_exports[0].socket_dir = "relative/socket".into(),
                10 => invalid.postgres_exports[0].socket_dir = "/run/../elsewhere".into(),
                11 => invalid.postgres_exports[0].socket_dir = "/run/./postgres".into(),
                12 => invalid.postgres_exports[0].socket_dir = "/run/postgres\0".into(),
                13 => invalid.postgres_exports[0].port = 0,
                14 => {
                    invalid.services[0]
                        .mounts
                        .insert("/data".into(), Disposition::Reconstructible);
                }
                15 => {
                    invalid.services[0].mounts.insert(
                        "/data".into(),
                        Disposition::Preserve {
                            data_kind: MeaningfulKind::Files,
                            reason: "Meaningful files are not a database declaration".into(),
                        },
                    );
                }
                16 => invalid.postgres_exports[0].socket_dir = format!("/{}", "x".repeat(8192)),
                17 => invalid.postgres_exports[0].socket_dir = "/run/postgres,/tmp/fallback".into(),
                _ => invalid.postgres_exports[0].socket_dir = "/run/postgres\n".into(),
            }
            assert!(invalid.validate().is_err(), "mutation {mutation}");
        }
        let mut raw = serde_json::to_value(&profile).unwrap();
        raw["postgres_exports"][0]["no_owner"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Profile>(raw).is_err());
        let mut raw = serde_json::to_value(&profile).unwrap();
        raw["postgres_exports"][0]["port"] = serde_json::json!(65536);
        assert!(serde_json::from_value::<Profile>(raw).is_err());
    }

    #[test]
    fn export_names_and_service_database_pairs_are_unique_with_a_bounded_inventory() {
        let mut profile = postgres_profile();
        let mut duplicate = profile.postgres_exports[0].clone();
        duplicate.name = "another-name".into();
        profile.postgres_exports.push(duplicate);
        assert!(profile.validate().is_err());
        profile.postgres_exports[1].database = "another_database".into();
        profile.postgres_exports[1].name = profile.postgres_exports[0].name.clone();
        assert!(profile.validate().is_err());
        let export = profile.postgres_exports[0].clone();
        profile.postgres_exports = (0..32)
            .map(|n| PostgresExport {
                name: format!("export-{n}"),
                database: format!("database_{n}"),
                ..export.clone()
            })
            .collect();
        profile.validate().unwrap();
        profile.postgres_exports.push(PostgresExport {
            name: "one-too-many".into(),
            database: "extra_database".into(),
            ..export
        });
        assert!(profile.validate().is_err());
        profile.postgres_exports.truncate(1);
        profile.postgres_exports[0].database = "a".repeat(63);
        profile.postgres_exports[0].username = "u".repeat(63);
        profile.validate().unwrap();
    }

    fn declaration() -> Profile {
        Profile {
            version: 1,
            workspace_id: uuid::Uuid::now_v7().to_string(),
            linux_setup: vec!["bun install --frozen-lockfile".into()],
            linux_startup: vec!["bun run dev".into()],
            services: vec![],
            preview_ports: vec![PreviewPort {
                name: "web".into(),
                offset: 0,
            }],
            postgres_exports: vec![],
        }
    }
    fn ready(store: &Store, workspace: &str) {
        store.with(|c| {
            c.execute("INSERT INTO workspaces(id,slug,path,branch,state,created_at) VALUES(?1,?1,'/fixture','fixture','ready','2026-10-23T00:00:00Z')", [workspace])?;
            Ok(())
        }).unwrap();
    }
    fn fixture() -> (Store, Profile) {
        let store = Store::open_in_memory().unwrap();
        let profile = declaration();
        ready(&store, &profile.workspace_id);
        (store, profile)
    }
    fn service() -> Service {
        let mounts = vec![crate::workspace_service_stop::ServiceMount {
            kind: "volume".into(),
            name: "data".into(),
            source: "/var/lib/docker/volumes/data/_data".into(),
            destination: "/data".into(),
            driver: "local".into(),
            mode: "".into(),
            writable: true,
            propagation: "".into(),
        }];
        Service {
            name: "database".into(),
            required: true,
            docker: DockerBinding {
                socket: "/var/run/docker.sock".into(),
                engine_id: "fixture-engine".into(),
                container: SelectedService {
                    id: "a".repeat(64),
                    image: format!("sha256:{}", "b".repeat(64)),
                    created: "2026-10-23T00:00:00Z".into(),
                    mounts_digest: URL_SAFE_NO_PAD
                        .encode(Sha256::digest(serde_json::to_vec(&mounts).unwrap())),
                    mounts,
                },
            },
            runtime_memory: Disposition::Reconstructible,
            writable_layer: Disposition::Reconstructible,
            mounts: BTreeMap::from([(
                "/data".into(),
                Disposition::Preserve {
                    data_kind: MeaningfulKind::Database,
                    reason: "User records need a coordinated export".into(),
                },
            )]),
            compose: None,
            image: None,
        }
    }

    #[test]
    fn profile_edits_compare_exact_heads_and_retain_immutable_versions() {
        let (store, profile) = fixture();
        assert_eq!(load(&store, &profile.workspace_id).unwrap(), None);
        let first = persist(&store, profile.clone(), None).unwrap();
        let mut edited = profile.clone();
        edited.linux_startup = vec!["bun run preview".into()];
        let second = persist(&store, edited.clone(), Some(&first.head)).unwrap();
        assert_eq!(second.head.revision, 2);
        assert_ne!(first.head.digest, second.head.digest);
        assert_eq!(
            load(&store, &profile.workspace_id).unwrap(),
            Some(second.clone())
        );
        assert!(persist(&store, profile.clone(), None).is_err());
        assert!(persist(&store, profile.clone(), Some(&first.head)).is_err());
        let original: String = store.with(|c| c.query_row("SELECT profile FROM workspace_development_profiles WHERE workspace_id=?1 AND revision=1", [&profile.workspace_id], |r| r.get(0))).unwrap();
        assert_eq!(
            serde_json::from_str::<Profile>(&original).unwrap(),
            first.profile
        );
        let other = declaration();
        ready(&store, &other.workspace_id);
        assert_eq!(load(&store, &other.workspace_id).unwrap(), None);
        assert!(persist(&store, other.clone(), Some(&second.head)).is_err());
        assert!(second.validate(&other.workspace_id).is_err());
    }

    #[test]
    fn missing_corrupt_oversized_and_rolled_back_heads_never_reset_the_profile() {
        let mutations = [
            "DELETE FROM workspace_development_profile_heads",
            "DELETE FROM workspace_development_profiles WHERE revision=1",
            "DELETE FROM workspace_development_profiles WHERE revision=2",
            "UPDATE workspace_development_profile_heads SET revision=1,digest=(SELECT digest FROM workspace_development_profiles WHERE revision=1)",
            "UPDATE workspace_development_profile_heads SET digest='broken'",
            "UPDATE workspace_development_profiles SET profile='{}' WHERE revision=2",
            "UPDATE workspace_development_profiles SET digest='broken' WHERE revision=2",
            "PRAGMA ignore_check_constraints=ON; UPDATE workspace_development_profiles SET revision=-1 WHERE revision=1",
        ];
        for mutation in mutations {
            let (store, profile) = fixture();
            let first = persist(&store, profile.clone(), None).unwrap();
            persist(&store, profile.clone(), Some(&first.head)).unwrap();
            store.with(|c| c.execute_batch(mutation)).unwrap();
            assert!(load(&store, &profile.workspace_id).is_err(), "{mutation}");
            assert!(persist(&store, profile, None).is_err(), "{mutation}");
        }
        let (store, profile) = fixture();
        persist(&store, profile.clone(), None).unwrap();
        // Multibyte text proves the SQL bound is bytes rather than SQLite character count.
        store
            .with(|c| {
                c.execute(
                    "UPDATE workspace_development_profiles SET profile=?1",
                    ["é".repeat(MAX_BYTES / 2 + 1)],
                )
            })
            .unwrap();
        assert!(load(&store, &profile.workspace_id).is_err());
    }

    #[test]
    fn meaningful_data_remains_explicit_and_every_mount_needs_one_classification() {
        let (store, mut profile) = fixture();
        profile.services.push(service());
        let saved = persist(&store, profile.clone(), None).unwrap();
        assert_eq!(load(&store, &profile.workspace_id).unwrap(), Some(saved));
        for mutation in 0..6 {
            let mut invalid = profile.clone();
            match mutation {
                0 => invalid.services[0].mounts.clear(),
                1 => {
                    invalid.services[0]
                        .mounts
                        .insert("/other".into(), Disposition::Reconstructible);
                }
                2 => {
                    invalid.services[0].docker.container.mounts_digest =
                        URL_SAFE_NO_PAD.encode([0; 32])
                }
                3 => {
                    invalid.services[0].runtime_memory = Disposition::Preserve {
                        data_kind: MeaningfulKind::Tasks,
                        reason: "".into(),
                    }
                }
                4 => invalid.services[0].docker.socket = "relative.sock".into(),
                _ => invalid.services.push(invalid.services[0].clone()),
            }
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn profiles_are_strict_bounded_and_commands_do_not_enter_debug_output() {
        let mut profile = declaration();
        profile.linux_setup = vec!["synthetic-private-command".into()];
        assert!(!format!("{profile:?}").contains("synthetic-private-command"));
        let mut raw = serde_json::to_value(&profile).unwrap();
        raw["unknown"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Profile>(raw).is_err());
        let mut invalid = profile.clone();
        invalid.linux_setup = vec!["x".repeat(8192); 64];
        assert!(invalid.validate().is_err());
        invalid = profile.clone();
        invalid.preview_ports.push(invalid.preview_ports[0].clone());
        assert!(invalid.validate().is_err());
        invalid = profile.clone();
        invalid.preview_ports[0].offset = MAX_PREVIEW_OFFSET;
        assert!(invalid.validate().is_err());
        invalid = profile;
        invalid.linux_startup = vec!["bad\0command".into()];
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn concurrent_edits_of_one_head_have_one_winner() {
        let (store, profile) = fixture();
        let first = persist(&store, profile.clone(), None).unwrap();
        std::thread::scope(|scope| {
            let a = scope.spawn(|| persist(&store, profile.clone(), Some(&first.head)));
            let b = scope.spawn(|| persist(&store, profile.clone(), Some(&first.head)));
            assert_ne!(a.join().unwrap().is_ok(), b.join().unwrap().is_ok());
        });
        assert_eq!(
            load(&store, &profile.workspace_id)
                .unwrap()
                .unwrap()
                .head
                .revision,
            2
        );
    }

    #[test]
    fn imported_profiles_survive_repeated_capture_without_becoming_local_authority() {
        use crate::cloud_workspace_profile;
        let (source, profile) = fixture();
        let saved = persist(&source, profile.clone(), None).unwrap();
        let captured = cloud_workspace_profile::capture(&source, &profile.workspace_id).unwrap();
        assert_eq!(captured, vec![saved.clone()]);
        let target = Store::open_in_memory().unwrap();
        ready(&target, &profile.workspace_id);
        target
            .durable(|c| {
                let tx = c.transaction()?;
                cloud_workspace_profile::install(&tx, &profile.workspace_id, &captured)?;
                tx.commit()?;
                Ok(())
            })
            .unwrap();
        assert!(load(&target, &profile.workspace_id).unwrap().is_none());
        assert_eq!(
            cloud_workspace_profile::capture(&target, &profile.workspace_id).unwrap(),
            captured
        );
        let mut local = profile.clone();
        local.linux_startup = vec!["bun run cloud".into()];
        let local = persist(&target, local, None).unwrap();
        assert_eq!(
            cloud_workspace_profile::capture(&target, &profile.workspace_id).unwrap(),
            vec![saved.clone(), local]
        );
        assert!(
            cloud_workspace_profile::validate(
                &[saved.clone(), saved.clone()],
                &profile.workspace_id
            )
            .is_err()
        );
        assert!(
            cloud_workspace_profile::validate(&[saved], &uuid::Uuid::now_v7().to_string()).is_err()
        );
    }
}
