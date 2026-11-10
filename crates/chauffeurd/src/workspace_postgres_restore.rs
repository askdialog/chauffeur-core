//! Restore one trusted source dump into a separate quarantined PostgreSQL service.
use std::{
    collections::BTreeSet,
    fs::File,
    io::{Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{
    Daemon, Error, Result,
    cloud_workspace_handoff::{Phase, State},
    cloud_workspace_profile, cloud_workspace_snapshot as snapshot,
    shellenv::ShellEnv,
    workspace_database_restore,
    workspace_development_profile::{self as profiles, DockerBinding},
    workspace_docker_command::DockerCommand,
    workspace_postgres_export::{self as export, ExportReceipt},
    workspace_service_stop::DockerServices,
};

const DATABASES: &str =
    "SELECT coalesce(json_agg(datname ORDER BY datname),'[]'::json) FROM pg_database";
const MAX_PLAN: usize = 512 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub docker: DockerBinding,
    pub username: String,
    pub socket_dir: String,
    pub port: u16,
    /// Only the standard maintenance database is supported; never a connection string.
    pub maintenance_database: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub version: u8,
    pub attempt_id: String,
    pub published: State,
    pub source: ExportReceipt,
    pub target: Target,
    pub database: String,
}

pub struct RestoredDatabase {
    pub receipt: Receipt,
}

fn uncertain() -> Error {
    Error::Unavailable("PostgreSQL restoration is incomplete or changed; retain its dump, destination database and quarantine before reconciling the operation".into())
}

fn imported(daemon: &Daemon, published: &State, index: usize) -> Result<(PathBuf, ExportReceipt)> {
    let workspace = &published.binding.workspace_id;
    let transfer = published.transfer.as_ref().ok_or_else(uncertain)?;
    if !crate::cloud_workspace_handoff::uuid(workspace)
        || index >= 32
        || transfer.phase != Phase::SnapshotReady
    {
        return Err(uncertain());
    }
    let row: Option<(Option<String>, Option<String>)> = daemon.store.with(|c| {
        c.query_row(
            "SELECT CASE WHEN length(CAST(root AS BLOB))<=8192 THEN root END,
             CASE WHEN length(CAST(receipt AS BLOB))<=?3 THEN receipt END
             FROM workspace_imported_database_exports WHERE workspace_id=?1 AND ordinal=?2",
            params![workspace, index as i64, MAX_PLAN as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
    })?;
    let Some((Some(root), Some(raw))) = row else {
        return Err(uncertain());
    };
    let expected: ExportReceipt = serde_json::from_str(&raw).map_err(|_| uncertain())?;
    if expected.source.binding.workspace_id != *workspace
        || serde_json::to_string(&expected)? != raw
    {
        return Err(uncertain());
    }
    let root = PathBuf::from(root);
    let home = daemon.paths.home.canonicalize().map_err(|_| uncertain())?;
    let selected = home
        .join("restorations")
        .join(&transfer.operation_id)
        .join("database_exports")
        .join(index.to_string());
    if root != selected || root.canonicalize().map_err(|_| uncertain())? != root {
        return Err(uncertain());
    }
    Ok((root, expected))
}

/// Selects one daemon-owned export retained by workspace restoration. Historical exports remain
/// provenance across later hops; only the exact current published source predecessor can restore.
/// No SQL runs unless the existing restore path verifies a completed matching quarantine.
pub async fn restore_packaged(
    daemon: Arc<Daemon>,
    published: State,
    target_peer: String,
    index: usize,
    target: Target,
) -> Result<RestoredDatabase> {
    restore_packaged_beside(daemon, published, target_peer, index, target, Vec::new()).await
}

/// As `restore_packaged`, into a server already holding `present`, databases restored there by
/// this same transfer: a server carrying several exported databases receives them one by one.
pub(crate) async fn restore_packaged_beside(
    daemon: Arc<Daemon>,
    published: State,
    target_peer: String,
    index: usize,
    target: Target,
    present: Vec<String>,
) -> Result<RestoredDatabase> {
    published.validate()?;
    let (root, expected) = imported(&daemon, &published, index)?;
    restore_beside(
        daemon,
        root,
        expected,
        published,
        target_peer,
        target,
        present,
    )
    .await
}

fn validate(source: &ExportReceipt, published: &State, target: &Target) -> Result<String> {
    published.validate()?;
    source.source.validate()?;
    source
        .profile
        .validate(&source.source.binding.workspace_id)?;
    if published.transfer.as_ref().map(|t| t.phase) != Some(Phase::SnapshotReady)
        || source.source.transfer.as_ref().map(|t| t.phase) != Some(Phase::Stopping)
        || !profiles::database_identifier(&target.username)
        || !profiles::postgres_endpoint(&target.socket_dir, target.port)
        || target.maintenance_database != "postgres"
    {
        return Err(uncertain());
    }
    let mut expected = source.source.clone();
    expected.revision = expected.revision.checked_add(2).ok_or_else(uncertain)?;
    let transfer = expected.transfer.as_mut().ok_or_else(uncertain)?;
    let published_transfer = published.transfer.as_ref().ok_or_else(uncertain)?;
    transfer.phase = Phase::SnapshotReady;
    transfer.source_stop_digest = published_transfer.source_stop_digest.clone();
    transfer.snapshot_digest = published_transfer.snapshot_digest.clone();
    if expected != *published {
        return Err(uncertain());
    }
    let declaration = source
        .profile
        .profile
        .postgres_exports
        .iter()
        .find(|entry| entry.name == source.export_name)
        .ok_or_else(uncertain)?;
    let service = source
        .profile
        .profile
        .services
        .iter()
        .find(|service| service.name == declaration.service)
        .ok_or_else(uncertain)?;
    if ["postgres", "template0", "template1"].contains(&declaration.database.as_str())
        || service.docker.container != source.container
    {
        return Err(uncertain());
    }
    if service.docker.engine_id == target.docker.engine_id {
        if source.container.id == target.docker.container.id {
            return Err(uncertain());
        }
        for source_mount in &source.container.mounts {
            if target.docker.container.mounts.iter().any(|mount| {
                !source_mount.source.is_empty()
                    && !mount.source.is_empty()
                    && (Path::new(&source_mount.source).starts_with(&mount.source)
                        || Path::new(&mount.source).starts_with(&source_mount.source))
                    || !source_mount.name.is_empty() && source_mount.name == mount.name
            }) {
                return Err(uncertain());
            }
        }
    }
    Ok(declaration.database.clone())
}

fn verified_input(root: &Path, expected: &ExportReceipt, staging: &Path) -> anyhow::Result<File> {
    anyhow::ensure!(
        export::verify(root)? == *expected,
        "PostgreSQL export reference changed"
    );
    let source = snapshot::open_file(root, "database.dump")?
        .ok_or_else(|| anyhow::anyhow!("PostgreSQL export input missing"))?;
    // This call's unlinked temporary inode has no pathname for an editor to replace or reopen.
    // Verify its complete bytes before any target command; the source artifact stays untouched.
    let mut input = tempfile::tempfile_in(staging)?;
    let (hash, bytes) = snapshot::copy_hash(source, Some(&mut input), expected.file.size)?;
    anyhow::ensure!(
        hash == expected.file.sha256 && bytes == expected.file.size,
        "PostgreSQL export input changed while it was staged"
    );
    input.sync_all()?;
    input.seek(SeekFrom::Start(0))?;
    Ok(input)
}

async fn observe(env: &ShellEnv, command: &DockerCommand, target: &Target) -> Result<()> {
    let observed = DockerServices::observe(
        env,
        command.socket_path(),
        target.docker.engine_id.clone(),
        vec![target.docker.container.id.clone()],
    )
    .await?;
    if observed.containers != [target.docker.container.clone()] {
        return Err(uncertain());
    }
    Ok(())
}

fn command_args(target: &Target, interactive: bool) -> Vec<&str> {
    let mut args = vec!["exec"];
    if interactive {
        args.push("-i");
    }
    args.push(&target.docker.container.id);
    args.extend(crate::workspace_postgres_export::CLEAN_LIBPQ);
    args
}

async fn databases(command: &DockerCommand, target: &Target, expected: &[&str]) -> Result<()> {
    let port = target.port.to_string();
    let mut args = command_args(target, false);
    args.extend([
        "psql",
        "--no-psqlrc",
        "--no-password",
        "--tuples-only",
        "--no-align",
        "--set",
        "ON_ERROR_STOP=1",
        "-h",
        &target.socket_dir,
        "-p",
        &port,
        "-U",
        &target.username,
        "-d",
        &target.maintenance_database,
        "--command",
        DATABASES,
    ]);
    let raw = command
        .run_checked(&args, Duration::from_secs(10), 64 * 1024)
        .await?;
    let names: Vec<String> = serde_json::from_slice(&raw).map_err(|_| uncertain())?;
    let actual = names.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if names.len() != expected.len() || actual != expected.iter().copied().collect() {
        return Err(uncertain());
    }
    Ok(())
}

/// The coordinator authenticates both the published journal and the separately bound source
/// export receipt. Dumps execute trusted source SQL. No public RPC accepts an arbitrary archive
/// or target. Successful restoration leaves quarantine installed and grants no execution permit.
pub async fn restore(
    daemon: Arc<Daemon>,
    root: PathBuf,
    expected: ExportReceipt,
    published: State,
    target_peer: String,
    target: Target,
) -> Result<RestoredDatabase> {
    restore_beside(
        daemon,
        root,
        expected,
        published,
        target_peer,
        target,
        Vec::new(),
    )
    .await
}

async fn restore_beside(
    daemon: Arc<Daemon>,
    root: PathBuf,
    expected: ExportReceipt,
    published: State,
    target_peer: String,
    target: Target,
    present: Vec<String>,
) -> Result<RestoredDatabase> {
    let database = validate(&expected, &published, &target)?;
    if present.contains(&database)
        || present.iter().any(|name| {
            !profiles::database_identifier(name)
                || ["postgres", "template0", "template1"].contains(&name.as_str())
        })
    {
        return Err(uncertain());
    }
    let check_root = root.clone();
    let check_expected = expected.clone();
    let staging = daemon.paths.home.clone();
    let input =
        tokio::task::spawn_blocking(move || verified_input(&check_root, &check_expected, &staging))
            .await
            .map_err(|error| Error::Internal(error.into()))??;
    if !cloud_workspace_profile::capture(&daemon.store, &published.binding.workspace_id)?
        .contains(&expected.profile)
    {
        return Err(uncertain());
    }
    let plan = serde_json::to_string(
        &serde_json::json!({"version":1,"published":published,"source":expected,"root":root,"target":target}),
    )?;
    if plan.len() > MAX_PLAN {
        return Err(uncertain());
    }
    let attempt = workspace_database_restore::reserve(
        &daemon.store,
        &published,
        &target_peer,
        &expected.export_name,
        &plan,
    )?;
    tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(600), async {
            attempt.require_quarantined()?;
            let env = ShellEnv::new();
            env.set_vars(daemon.shell_env.vars());
            let command = DockerCommand::new(&env, std::path::Path::new(&target.docker.socket))?;
            if command.socket_path().to_str() != Some(target.docker.socket.as_str()) {
                return Err(uncertain());
            }
            observe(&env, &command, &target).await?;
            let mut before = vec!["postgres", "template0", "template1"];
            before.extend(present.iter().map(String::as_str));
            databases(&command, &target, &before).await?;
            attempt.require_quarantined()?;
            let port = target.port.to_string();
            let mut args = command_args(&target, true);
            // --create preserves database-level ACLs/settings and refuses an existing DB;
            // it cannot run inside --single-transaction. Errors retain the partial target.
            args.extend([
                "pg_restore",
                "--create",
                "--exit-on-error",
                "--no-password",
                "-h",
                &target.socket_dir,
                "-p",
                &port,
                "-U",
                &target.username,
                "-d",
                &target.maintenance_database,
            ]);
            let output = command
                .run_with_input(
                    &args,
                    Duration::from_secs(300),
                    input,
                    expected.file.size,
                    &expected.file.sha256,
                    64 * 1024,
                )
                .await?;
            if !output.is_empty() {
                return Err(uncertain());
            }
            observe(&env, &command, &target).await?;
            let mut after = before.clone();
            after.push(&database);
            databases(&command, &target, &after).await?;
            observe(&env, &command, &target).await?;
            attempt.require_quarantined()?;
            if !cloud_workspace_profile::capture(&daemon.store, &published.binding.workspace_id)?
                .contains(&expected.profile)
            {
                return Err(uncertain());
            }
            let receipt = Receipt {
                version: 1,
                attempt_id: attempt.id().to_owned(),
                published,
                source: expected,
                target,
                database,
            };
            let raw = serde_json::to_string(&receipt)?;
            attempt.finish(&raw)?;
            Ok(RestoredDatabase { receipt })
        })
        .await
        .map_err(|_| uncertain())?
    })
    .await
    .map_err(|error| Error::Internal(error.into()))?
}
