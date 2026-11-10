//! Exports a PostgreSQL service's declared databases, stops it, and proves the cluster took no
//! write between the exports and its shutdown. Every transactional write assigns a transaction
//! id. Before the exports, with no other client connected, nothing in progress or prepared, the
//! server's next id is recorded; after a clean stop, the shutdown checkpoint's next id, read from
//! the volume by the image's own `pg_controldata`, must be the same. Writes that assign no id
//! (sequence values handed out from the cached range, hint bits, `ALTER SYSTEM`) are outside the
//! proof; a write it cannot tell from an automatic `ANALYZE` refuses, and a retry decides.
use std::{collections::BTreeSet, path::Path, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{
    Daemon, Error, Result,
    cloud_workspace_handoff::State,
    cloud_workspace_package::FileEntry,
    shellenv::ShellEnv,
    workspace_development_profile::{
        self as profiles, Disposition, MeaningfulKind, PostgresExport, ProfileHead, Service,
        StoredProfile,
    },
    workspace_docker_command::DockerCommand,
    workspace_postgres_export as exports,
    workspace_service_stop::{self as service_stop, ServiceMount, StoppedService},
};

/// One JSON row; `pg_control_system()` and `data_directory` need a superuser or explicit grants.
const QUIET: &str = "SELECT json_build_object(\
'system',(SELECT system_identifier::text FROM pg_control_system()),\
'next_xid',pg_snapshot_xmax(pg_current_snapshot())::text,\
'running',(SELECT count(*) FROM pg_stat_activity WHERE backend_xid IS NOT NULL AND pid<>pg_backend_pid())\
+(SELECT count(*) FROM pg_snapshot_xip(pg_current_snapshot())),\
'prepared',(SELECT count(*) FROM pg_prepared_xacts),\
'others',(SELECT count(*) FROM pg_stat_activity WHERE pid<>pg_backend_pid() AND backend_type IN \
('client backend','background worker','logical replication worker','parallel worker','walsender')),\
'data_directory',current_setting('data_directory'),\
'version',current_setting('server_version_num')::int,\
'databases',(SELECT json_agg(datname ORDER BY datname) FROM pg_database WHERE datallowconn AND NOT datistemplate))";

/// The attempt is still this fence's pending preparation.
const CURRENT: &str = "EXISTS(SELECT 1 FROM workspace_preparation_attempts p JOIN workspaces w ON w.id=p.workspace_id WHERE p.attempt_id=?1 AND p.workspace_id=?2 AND p.fence=?4 AND p.settled=0 AND w.execution_fence=?4)";

/// User relations of the maintenance database, which no export carries.
const MAINTENANCE_RELATIONS: &str = "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace \
WHERE c.relkind IN ('r','p','m','S','f') AND n.nspname NOT IN ('pg_catalog','information_schema') \
AND n.nspname NOT LIKE 'pg_toast%' AND n.nspname NOT LIKE 'pg_temp%'";

/// The cluster's state when its exports began.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuietStart {
    pub system: String,
    pub next_xid: String,
    pub running: u64,
    pub prepared: u64,
    pub others: u64,
    pub data_directory: String,
    pub version: u32,
    pub databases: Vec<String>,
}

/// The shutdown checkpoint, as `pg_controldata` reads it from the stopped service's volume.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shutdown {
    pub system: String,
    pub state: String,
    pub next_xid: u64,
    /// Every start and stop writes a new checkpoint, even with no transaction id assigned.
    pub checkpoint: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportedFile {
    pub export_name: String,
    pub file: FileEntry,
}

/// A PostgreSQL service stopped with no write after its exports.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuietDatabase {
    pub version: u8,
    pub source: State,
    pub profile_head: ProfileHead,
    pub service: String,
    pub start: QuietStart,
    pub exports: Vec<ExportedFile>,
    pub stopped: StoppedService,
    pub shutdown: Shutdown,
}

fn uncertain() -> Error {
    Error::Unavailable("The PostgreSQL service's stop cannot be verified; retain its data, exports and the workspace fence before reconciling it".into())
}

/// The service, its exports and the mount holding the cluster. Every resource must be either
/// reconstructible or database data these exports carry.
fn declaration<'a>(
    profile: &'a StoredProfile,
    name: &str,
) -> Result<(&'a Service, Vec<&'a PostgresExport>)> {
    let service = profile
        .profile
        .services
        .iter()
        .find(|service| service.name == name)
        .ok_or_else(|| Error::NotFound(format!("service {name} in the development profile")))?;
    let declared: Vec<&PostgresExport> = profile
        .profile
        .postgres_exports
        .iter()
        .filter(|export| export.service == name)
        .collect();
    let database = |value: &Disposition| {
        matches!(
            value,
            Disposition::Preserve {
                data_kind: MeaningfulKind::Database,
                ..
            }
        )
    };
    let covered = |value: &Disposition| *value == Disposition::Reconstructible || database(value);
    if declared.is_empty()
        || service.writable_layer != Disposition::Reconstructible
        || !covered(&service.runtime_memory)
        || !service.mounts.values().all(covered)
        || !service.mounts.values().any(database)
    {
        return Err(Error::Conflict(format!(
            "Service {name} is not a PostgreSQL service whose data its exports carry: declare its database mount and at least one export, everything else reconstructible"
        )));
    }
    Ok((service, declared))
}

/// The declared mount that holds the cluster's data directory, which must be database data.
fn data_mount<'a>(service: &'a Service, data_directory: &str) -> Result<&'a ServiceMount> {
    let mount = service
        .docker
        .container
        .mounts
        .iter()
        .filter(|mount| Path::new(data_directory).starts_with(&mount.destination))
        .max_by_key(|mount| mount.destination.len())
        .ok_or_else(|| {
            Error::Conflict(format!(
                "PostgreSQL keeps its data in {data_directory}, outside every mount of service {}; mount a volume there",
                service.name
            ))
        })?;
    let preserved = service.mounts.get(&mount.destination).is_some_and(|value| {
        matches!(
            value,
            Disposition::Preserve {
                data_kind: MeaningfulKind::Database,
                ..
            }
        )
    });
    if !preserved || mount.source.contains(',') || mount.name.contains(',') {
        return Err(uncertain());
    }
    Ok(mount)
}

async fn psql(
    command: &DockerCommand,
    container: &str,
    export: &PostgresExport,
    database: &str,
    sql: &str,
) -> Result<Vec<u8>> {
    let port = export.port.to_string();
    command
        .run_checked(
            &psql_args(container, export, database, &port, sql),
            Duration::from_secs(10),
            64 * 1024,
        )
        .await
}

fn psql_args<'a>(
    container: &'a str,
    export: &'a PostgresExport,
    database: &'a str,
    port: &'a str,
    sql: &'a str,
) -> Vec<&'a str> {
    let mut args = vec!["exec", container];
    args.extend(exports::CLEAN_LIBPQ);
    args.extend([
        "psql",
        "--no-psqlrc",
        "--no-password",
        "--tuples-only",
        "--no-align",
        "--set",
        "ON_ERROR_STOP=1",
        "-h",
        &export.socket_dir,
        "-p",
        port,
        "-U",
        &export.username,
        "-d",
        database,
        "--command",
        sql,
    ]);
    args
}

/// Checks the cluster is idle and every database a client can use is exported. The maintenance
/// database `postgres` and global objects (roles, tablespaces) are not carried.
fn check_start(start: &QuietStart, service: &str, exported: &BTreeSet<&str>) -> Result<()> {
    if start.others > 0 || start.running > 0 || start.prepared > 0 {
        return Err(Error::Conflict(format!(
            "Service {service} still has {} other connection(s), {} transaction(s) in progress and {} prepared; close them, then retry the transfer",
            start.others, start.running, start.prepared
        )));
    }
    let missing: Vec<&str> = start
        .databases
        .iter()
        .map(String::as_str)
        .filter(|name| *name != "postgres" && !exported.contains(name))
        .collect();
    if !missing.is_empty() {
        return Err(Error::Conflict(format!(
            "Service {service} holds databases no export declares: {}; declare an export for each, then retry the transfer",
            missing.join(", ")
        )));
    }
    if start.next_xid.parse::<u64>().is_err() || start.system.parse::<u64>().is_err() {
        return Err(uncertain());
    }
    Ok(())
}

/// Reads `pg_controldata`'s English output: system identifier, cluster state, and the shutdown
/// checkpoint's next transaction id as `epoch:xid`, combined into a 64-bit id.
pub fn parse_controldata(output: &str) -> Option<Shutdown> {
    let field = |label: &str| {
        output
            .lines()
            .find_map(|line| line.strip_prefix(label)?.strip_prefix(':'))
            .map(str::trim)
    };
    let (epoch, xid) = field("Latest checkpoint's NextXID")?.split_once(':')?;
    let next_xid = (epoch.parse::<u64>().ok()? << 32) | xid.parse::<u32>().ok()? as u64;
    Some(Shutdown {
        system: field("Database system identifier")?.to_owned(),
        state: field("Database cluster state")?.to_owned(),
        next_xid,
        checkpoint: field("Latest checkpoint location")?.to_owned(),
    })
}

/// Runs the image's own `pg_controldata` on the stopped service's data, mounted read-only, in a
/// throwaway container without network or capabilities but the one to read the data folder.
async fn read_shutdown(
    command: &DockerCommand,
    service: &Service,
    mount: &ServiceMount,
    data_directory: &str,
) -> Result<Shutdown> {
    let source = match mount.kind.as_str() {
        "volume" => format!("type=volume,src={}", mount.name),
        "bind" => format!("type=bind,src={}", mount.source),
        _ => return Err(uncertain()),
    };
    let spec = format!("{source},dst={},readonly", mount.destination);
    let args = [
        "run",
        "--rm",
        "--pull=never",
        "--network",
        "none",
        "--read-only",
        "--cap-drop",
        "ALL",
        "--cap-add",
        "DAC_READ_SEARCH",
        "--security-opt",
        "no-new-privileges",
        "--env",
        "LC_ALL=C",
        "--env",
        "LANG=C",
        "--entrypoint",
        "pg_controldata",
        "--mount",
        &spec,
        &service.docker.container.image,
        data_directory,
    ];
    let output = command
        .run_checked(&args, Duration::from_secs(60), 64 * 1024)
        .await?;
    parse_controldata(std::str::from_utf8(&output).map_err(|_| uncertain())?).ok_or_else(uncertain)
}

/// After the process sweep, exports every database the profile declares for `service`, stops it
/// and proves no write followed (see the module). The coordinator authenticates the exact
/// original stopping state. A refusal before the exports changes nothing; a write found after
/// the stop settles the attempt with its evidence and leaves the service stopped and the fence
/// installed; Docker uncertainty leaves the attempt pending, as for other service stops.
pub async fn export_and_stop(
    daemon: Arc<Daemon>,
    stopping: State,
    head: ProfileHead,
    service_name: String,
) -> Result<QuietDatabase> {
    daemon.execution.require_settled_stop(&stopping)?;
    let profile =
        profiles::load(&daemon.store, &stopping.binding.workspace_id)?.ok_or_else(uncertain)?;
    if profile.head != head {
        return Err(uncertain());
    }
    declaration(&profile, &service_name)?;
    tokio::spawn(async move {
        let (service, declared) = declaration(&profile, &service_name)?;
        // A login-environment reload must not select a different Docker wrapper midway.
        let env = ShellEnv::new();
        env.set_vars(daemon.shell_env.vars());
        let socket = Path::new(&service.docker.socket);
        let command = DockerCommand::new(&env, socket)?;
        let container = &service.docker.container;
        let first = declared[0];
        let query = |database: &'static str, sql: &'static str| {
            let database = if database.is_empty() { first.database.as_str() } else { database };
            psql(&command, &container.id, first, database, sql)
        };
        let version: u32 = String::from_utf8_lossy(&query("", "SHOW server_version_num").await?).trim().parse().map_err(|_| uncertain())?;
        if version < 130000 {
            return Err(Error::Conflict(format!(
                "Service {service_name} runs PostgreSQL older than 13, whose transaction ids Chauffeur cannot compare; upgrade it before the transfer"
            )));
        }
        let raw = query("", QUIET).await.map_err(|_| Error::Conflict(format!(
            "Role {} of service {service_name} cannot read the cluster's state (pg_control_system, data_directory, other sessions); declare a superuser role for its export, then retry the transfer",
            first.username
        )))?;
        let start: QuietStart = serde_json::from_slice(&raw).map_err(|_| uncertain())?;
        let names: BTreeSet<&str> = declared.iter().map(|e| e.database.as_str()).collect();
        check_start(&start, &service_name, &names)?;
        if start.databases.iter().any(|name| name == "postgres") && !names.contains("postgres") {
            let relations: u64 = String::from_utf8_lossy(&query("postgres", MAINTENANCE_RELATIONS).await?).trim().parse().map_err(|_| uncertain())?;
            if relations > 0 {
                return Err(Error::Conflict(format!(
                    "Service {service_name} keeps {relations} table(s) or sequence(s) in its maintenance database postgres, which no export carries; move them to an exported database, then retry the transfer"
                )));
            }
        }
        let mount = data_mount(service, &start.data_directory)?;
        // Known before anything stops: an image without it could never prove its shutdown.
        command
            .run_checked(&["exec", &container.id, "pg_controldata", "--version"], Duration::from_secs(10), 4096)
            .await
            .map_err(|_| Error::Conflict(format!(
                "The image of service {service_name} has no pg_controldata to read its shutdown checkpoint; use an official PostgreSQL image, then retry the transfer"
            )))?;
        let mut files = Vec::new();
        for export in &declared {
            let exported = exports::capture(daemon.clone(), stopping.clone(), head.clone(), export.name.clone()).await?;
            files.push(ExportedFile { export_name: export.name.clone(), file: exported.receipt.file });
        }
        let attempt = daemon.execution.begin_preparation(&stopping)?;
        if profiles::load(&daemon.store, &stopping.binding.workspace_id)?.as_ref() != Some(&profile) {
            // Nothing acted under this attempt yet.
            attempt.finish()?;
            return Err(Error::Conflict("The development profile changed during the source stop; retry it".into()));
        }
        let workspace = &stopping.binding.workspace_id;
        let fence = serde_json::to_string(&stopping)?;
        let plan = serde_json::to_string(&serde_json::json!({
            "version": 1, "source": stopping, "profile_head": head, "service": service_name,
            "start": start, "exports": files,
        }))?;
        daemon.store.durable(|c| {
            let changed = c.execute(
                &format!("INSERT INTO workspace_database_stops(attempt_id,workspace_id,plan) SELECT ?1,?2,?3 WHERE {CURRENT}"),
                rusqlite::params![attempt.id(), workspace, plan, fence],
            )?;
            if changed != 1 { return Err(uncertain()); }
            Ok(())
        })?;
        let stopped = service_stop::stop_exported(&env, socket, service.docker.engine_id.clone(), container).await?;
        let shutdown = read_shutdown(&command, service, mount, &start.data_directory).await?;
        daemon.execution.require_settled_stop(&stopping)?;
        let quiet = QuietDatabase {
            version: 1,
            source: stopping.clone(),
            profile_head: head.clone(),
            service: service_name.clone(),
            start: start.clone(),
            exports: files,
            stopped,
            shutdown: shutdown.clone(),
        };
        let written = shutdown.system != start.system
            || shutdown.state != "shut down"
            || Some(shutdown.next_xid) != start.next_xid.parse().ok();
        let receipt = serde_json::to_string(&serde_json::json!({"quiet": !written, "observation": quiet}))?;
        daemon.store.durable(|c| {
            let changed = c.execute(
                &format!("UPDATE workspace_database_stops SET receipt=?3 WHERE attempt_id=?1 AND workspace_id=?2 AND receipt IS NULL AND {CURRENT}"),
                rusqlite::params![attempt.id(), workspace, receipt, fence],
            )?;
            if changed != 1 { return Err(uncertain()); }
            Ok(())
        })?;
        attempt.finish()?;
        if written {
            return Err(Error::Conflict(format!(
                "Service {service_name} did not stop cleanly or recorded a write after its export (next transaction {} instead of {}); its exports cannot be carried. Cancel the transfer to work locally again, or restart the service and retry",
                shutdown.next_xid, start.next_xid
            )));
        }
        Ok(quiet)
    })
    .await
    .map_err(|error| Error::Internal(error.into()))?
}

/// A no-write proof already stored for `service` under this exact fence and profile: a retried
/// source stop reuses it, as a stopped database cannot be exported again, only while the same
/// pinned container is still stopped at the very shutdown checkpoint the proof read.
pub(crate) async fn proved_before(
    daemon: &Daemon,
    stopping: &State,
    profile: &StoredProfile,
    service_name: &str,
) -> Result<Option<QuietDatabase>> {
    let fence = serde_json::to_string(stopping)?;
    let receipts: Vec<String> = daemon.store.with(|c| {
        c.prepare("SELECT d.receipt FROM workspace_database_stops d JOIN workspace_preparation_attempts p USING(attempt_id) WHERE d.workspace_id=?1 AND p.fence=?2 AND p.settled=1 AND d.receipt IS NOT NULL ORDER BY d.attempt_id DESC")?
            .query_map(rusqlite::params![stopping.binding.workspace_id, fence], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()
    })?;
    #[derive(Deserialize)]
    struct Stored {
        quiet: bool,
        observation: QuietDatabase,
    }
    let Some(proof) = receipts
        .iter()
        .filter_map(|raw| serde_json::from_str::<Stored>(raw).ok())
        .find(|stored| stored.observation.service == service_name)
    else {
        return Ok(None);
    };
    if !proof.quiet || proof.observation.profile_head != profile.head {
        return Ok(None);
    }
    let (service, _) = declaration(profile, service_name)?;
    let env = ShellEnv::new();
    env.set_vars(daemon.shell_env.vars());
    let socket = Path::new(&service.docker.socket);
    // Started again since: its data may have changed; it runs, so it can be exported afresh.
    if !service_stop::is_stopped(
        &env,
        socket,
        service.docker.engine_id.clone(),
        &service.docker.container,
    )
    .await?
    {
        return Ok(None);
    }
    // Still stopped, but maybe started and stopped again: the shutdown checkpoint tells.
    let command = DockerCommand::new(&env, socket)?;
    let mount = data_mount(service, &proof.observation.start.data_directory)?;
    let shutdown = read_shutdown(
        &command,
        service,
        mount,
        &proof.observation.start.data_directory,
    )
    .await?;
    if shutdown != proof.observation.shutdown {
        return Err(Error::Conflict(format!(
            "Service {service_name} ran again since its export; start it so it can be exported afresh, then retry the transfer"
        )));
    }
    Ok(Some(proof.observation))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTROLDATA: &str = "pg_control version number:            1300
Database system identifier:           7699840022162239530
Database cluster state:               shut down
Latest checkpoint location:           0/1A2B3C4
Latest checkpoint's NextXID:          2:732
Latest checkpoint's NextOID:          24576
";

    #[test]
    fn the_shutdown_checkpoint_s_next_transaction_includes_its_epoch() {
        let shutdown = parse_controldata(CONTROLDATA).unwrap();
        assert_eq!(shutdown.system, "7699840022162239530");
        assert_eq!(shutdown.state, "shut down");
        assert_eq!(shutdown.next_xid, (2 << 32) | 732);
        assert!(parse_controldata("Database cluster state: shut down\n").is_none());
        assert!(parse_controldata(&CONTROLDATA.replace("2:732", "732")).is_none());
    }

    fn start() -> QuietStart {
        QuietStart {
            system: "7699840022162239530".into(),
            next_xid: "732".into(),
            running: 0,
            prepared: 0,
            others: 0,
            data_directory: "/var/lib/postgresql/data".into(),
            version: 160015,
            databases: vec!["app".into(), "postgres".into()],
        }
    }

    #[test]
    fn exports_start_only_on_an_idle_cluster_whose_databases_are_all_declared() {
        let exported = BTreeSet::from(["app"]);
        assert!(check_start(&start(), "db", &exported).is_ok());
        for n in 0..4 {
            let mut value = start();
            match n {
                0 => value.others = 1,
                1 => value.running = 1,
                2 => value.prepared = 1,
                _ => value.databases.push("analytics".into()),
            }
            let refusal = check_start(&value, "db", &exported)
                .unwrap_err()
                .to_string();
            assert!(refusal.contains("db"), "{refusal}");
        }
    }
}
