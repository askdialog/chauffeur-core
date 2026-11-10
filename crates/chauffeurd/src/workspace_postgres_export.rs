//! Retained single-database logical exports, not a complete data inventory or a restore proof.
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Daemon, Error, Result,
    cloud_workspace_handoff::{Phase, State},
    cloud_workspace_package::FileEntry,
    cloud_workspace_snapshot as snapshot,
    workspace_development_profile::{self as profiles, ProfileHead, StoredProfile},
    workspace_docker_command::DockerCommand,
    workspace_service_stop::{DockerServices, SelectedService},
};

const MAX_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_MANIFEST: u64 = 512 * 1024;
const DATA: &str = "database.dump";
const MANIFEST: &str = "manifest.json";
/// Runs a PostgreSQL client in the container without the libpq overrides its environment may
/// set. `env -u`, not `--env NAME=`: libpq looks an empty `PGSERVICE` up as a service named "".
pub(crate) const CLEAN_LIBPQ: [&str; 9] = [
    "env",
    "-u",
    "PGSERVICE",
    "-u",
    "PGSERVICEFILE",
    "-u",
    "PGOPTIONS",
    "-u",
    "PGHOSTADDR",
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExportReceipt {
    pub version: u8,
    pub source: State,
    pub profile: StoredProfile,
    pub export_name: String,
    pub container: SelectedService,
    pub file: FileEntry,
}

pub struct ExportedDatabase {
    pub root: PathBuf,
    pub receipt: ExportReceipt,
}

fn uncertain() -> Error {
    Error::Unavailable("PostgreSQL export is incomplete or changed; retain its source, files and workspace fence before reconciling the export".into())
}

fn declaration<'a>(
    profile: &'a StoredProfile,
    name: &str,
) -> Result<(&'a profiles::PostgresExport, &'a profiles::Service)> {
    let export = profile
        .profile
        .postgres_exports
        .iter()
        .find(|export| export.name == name)
        .ok_or_else(uncertain)?;
    let service = profile
        .profile
        .services
        .iter()
        .find(|service| service.name == export.service)
        .ok_or_else(uncertain)?;
    Ok((export, service))
}

/// The coordinator must authenticate the exact original stopping state first. Registered
/// launchers must already be stopped; PostgreSQL remains running for its logical snapshot.
/// Losing the observer never cancels host work, and errors never delete or adopt partial files.
pub async fn capture(
    daemon: Arc<Daemon>,
    stopping: State,
    head: ProfileHead,
    export_name: String,
) -> Result<ExportedDatabase> {
    daemon.execution.require_settled_stop(&stopping)?;
    let profile =
        profiles::load(&daemon.store, &stopping.binding.workspace_id)?.ok_or_else(uncertain)?;
    if profile.head != head {
        return Err(uncertain());
    }
    declaration(&profile, &export_name)?;
    let attempt = daemon.execution.begin_preparation(&stopping)?;
    tokio::spawn(async move {
        let result = tokio::time::timeout(Duration::from_secs(600), async {
            daemon.execution.require_settled_stop(&stopping)?;
            if profiles::load(&daemon.store, &stopping.binding.workspace_id)?.as_ref() != Some(&profile) {
                return Err(uncertain());
            }
            let (export, service) = declaration(&profile, &export_name)?;
            let root = daemon.paths.home.canonicalize()?
                .join(format!("workspace-data-export-{}", attempt.id()));
            let plan = serde_json::to_string(&serde_json::json!({
                "version": 1, "source": stopping, "profile": profile,
                "export_name": export_name, "root": root
            }))?;
            if plan.len() as u64 > MAX_MANIFEST {
                return Err(uncertain());
            }
            let fence = serde_json::to_string(&stopping)?;
            daemon.store.durable(|c| {
                let changed = c.execute("INSERT INTO workspace_database_exports(attempt_id,workspace_id,plan) SELECT ?1,?2,?3 WHERE EXISTS(SELECT 1 FROM workspace_preparation_attempts p JOIN workspaces w ON w.id=p.workspace_id WHERE p.attempt_id=?1 AND p.workspace_id=?2 AND p.fence=?4 AND p.settled=0 AND w.execution_fence=?4)", rusqlite::params![attempt.id(),stopping.binding.workspace_id,plan,fence])?;
                if changed != 1 { return Err(uncertain()); }
                Ok(())
            })?;
            // A login-environment reload must not select a different inspection wrapper.
            let env = crate::shellenv::ShellEnv::new();
            env.set_vars(daemon.shell_env.vars());
            let command = DockerCommand::new(&env, Path::new(&service.docker.socket))?;
            if command.socket_path().to_str() != Some(service.docker.socket.as_str()) {
                return Err(uncertain());
            }
            let observe = || async {
                let observed = DockerServices::observe(
                    &env, command.socket_path(), service.docker.engine_id.clone(),
                    vec![service.docker.container.id.clone()],
                ).await?;
                if observed.containers != [service.docker.container.clone()] {
                    return Err(uncertain());
                }
                Ok(())
            };
            observe().await?;
            snapshot::private_dir(&root)?;
            fs::File::open(root.parent().context("Export parent missing")?)?.sync_all()?;
            let file = snapshot::private_file(&root.join(DATA))?;
            let port = export.port.to_string();
            // Explicit local Unix database endpoint; never a URI/connection string or host shell.
            // Owners/ACLs stay in the dump. A later restore must handle their prerequisites.
            let mut args = vec!["exec", &service.docker.container.id];
            args.extend(CLEAN_LIBPQ);
            args.extend([
                "pg_dump", "--format=custom", "--no-password",
                "-h", &export.socket_dir, "-p", &port, "-U", &export.username,
                "-d", &export.database,
            ]);
            let streamed = command.run_to_file(
                &args, Duration::from_secs(300), MAX_BYTES, file,
            ).await?;
            observe().await?;
            daemon.execution.require_settled_stop(&stopping)?;
            if profiles::load(&daemon.store, &stopping.binding.workspace_id)?.as_ref() != Some(&profile) {
                return Err(uncertain());
            }
            let receipt = ExportReceipt {
                version: 1, source: stopping.clone(), profile: profile.clone(),
                export_name: export_name.clone(), container: service.docker.container.clone(),
                file: FileEntry { path: DATA.into(), sha256: streamed.sha256, size: streamed.bytes },
            };
            let bytes = serde_json::to_vec(&receipt)?;
            if bytes.len() as u64 > MAX_MANIFEST {
                return Err(uncertain());
            }
            let mut manifest = snapshot::private_file(&root.join(MANIFEST))?;
            manifest.write_all(&bytes)?;
            manifest.sync_all()?;
            snapshot::sync_directories(&root)?;
            let check_root = root.clone();
            let checked = tokio::task::spawn_blocking(move || verify(&check_root))
                .await.map_err(|error| Error::Internal(error.into()))??;
            if checked != receipt {
                return Err(uncertain());
            }
            daemon.execution.require_settled_stop(&stopping)?;
            let raw = serde_json::to_string(&receipt)?;
            daemon.store.durable(|c| {
                let changed = c.execute("UPDATE workspace_database_exports SET receipt=?3 WHERE attempt_id=?1 AND workspace_id=?2 AND receipt IS NULL AND EXISTS(SELECT 1 FROM workspace_preparation_attempts p JOIN workspaces w ON w.id=p.workspace_id WHERE p.attempt_id=?1 AND p.workspace_id=?2 AND p.fence=?4 AND p.settled=0 AND w.execution_fence=?4)",rusqlite::params![attempt.id(),stopping.binding.workspace_id,raw,fence])?;
                if changed != 1 { return Err(uncertain()); }
                Ok(())
            })?;
            Ok(ExportedDatabase{root,receipt})
        }).await.map_err(|_| uncertain())?;
        let exported = result?;
        attempt.finish()?;
        Ok(exported)
    }).await.map_err(|error| Error::Internal(error.into()))?
}

/// Verifies retained bytes and their declaration; it does not establish restoreability, capture
/// cluster-wide roles/configuration, or prove no later database writes occurred.
pub fn verify(root: &Path) -> anyhow::Result<ExportReceipt> {
    let metadata = fs::symlink_metadata(root)?;
    // SAFETY: geteuid has no arguments or preconditions.
    ensure!(
        metadata.is_dir()
            && metadata.mode() & 0o777 == 0o700
            && metadata.uid() == unsafe { libc::geteuid() },
        "PostgreSQL export folder is not private"
    );
    let file =
        snapshot::open_file(root, MANIFEST)?.context("PostgreSQL export manifest missing")?;
    ensure!(
        file.metadata()?.len() <= MAX_MANIFEST,
        "PostgreSQL export manifest too large"
    );
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_MANIFEST,
        "PostgreSQL export manifest too large"
    );
    let receipt: ExportReceipt = serde_json::from_slice(&bytes)?;
    receipt.source.validate()?;
    receipt
        .profile
        .validate(&receipt.source.binding.workspace_id)?;
    let (_, service) = declaration(&receipt.profile, &receipt.export_name)?;
    ensure!(
        receipt.version == 1
            && receipt.source.transfer.as_ref().map(|t| t.phase) == Some(Phase::Stopping)
            && service.docker.container == receipt.container,
        "PostgreSQL export declaration changed"
    );
    ensure!(
        receipt.file.path == DATA && receipt.file.size >= 5 && receipt.file.size <= MAX_BYTES,
        "Invalid PostgreSQL export file"
    );
    let mut file = snapshot::open_file(root, DATA)?.context("PostgreSQL export data missing")?;
    let mut magic = [0u8; 5];
    file.read_exact(&mut magic)?;
    ensure!(
        &magic == b"PGDMP",
        "PostgreSQL export is not a custom archive"
    );
    // Hash the same bytes used to check the header, through the same descriptor.
    let mut hash = Sha256::new();
    hash.update(magic);
    let mut size = magic.len() as u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let length = file.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        size += length as u64;
        ensure!(
            size <= MAX_BYTES,
            "PostgreSQL export data exceeds its limit"
        );
        hash.update(&buffer[..length]);
    }
    let digest = hex::encode(hash.finalize());
    ensure!(
        digest == receipt.file.sha256 && size == receipt.file.size,
        "PostgreSQL export data changed"
    );
    let mut count = 0;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        // SAFETY: geteuid has no arguments or preconditions.
        ensure!(
            metadata.is_file()
                && metadata.mode() & 0o777 == 0o600
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.nlink() == 1
                && [DATA, MANIFEST]
                    .iter()
                    .any(|name| entry.file_name() == *name),
            "Unexpected or unprotected PostgreSQL export entry"
        );
        count += 1;
    }
    ensure!(count == 2, "PostgreSQL export is incomplete");
    Ok(receipt)
}
