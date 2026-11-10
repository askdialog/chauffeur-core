//! A workspace's development services on the machine it arrives at (docs/workspace-development-
//! services.md): while it is still quarantined, each declared service is started again from its
//! compose file under its compose project, on fresh volumes, and each database the source exported
//! is restored into its server. Activation then makes those containers the workspace's declared
//! services here. A retry or a cancellation removes what an earlier attempt started (`down -v`):
//! the source still holds everything.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{OptionalExtension, params};

use crate::cloud_workspace_handoff::State;
use crate::workspace_development_profile::{
    self as profiles, DockerBinding, Profile, StoredProfile,
};
use crate::workspace_docker_command::DockerCommand;
use crate::workspace_postgres_export::ExportReceipt;
use crate::workspace_postgres_restore::{self as restore, Target};
use crate::workspace_service_stop::DockerServices;
use crate::{Daemon, Error, Result};

/// How long `docker compose up` may take, image pulls included.
const UP: Duration = Duration::from_secs(300);
/// How long a PostgreSQL server may take to accept connections once started.
const READY: Duration = Duration::from_secs(90);

fn failed(what: impl std::fmt::Display) -> Error {
    Error::Unavailable(format!(
        "The workspace's services could not start on this machine: {what}. The source still holds everything; fix it here, then resume or cancel the transfer"
    ))
}

/// The engine services start on: the first local one that answers.
async fn engine(daemon: &Daemon) -> Result<(DockerCommand, String)> {
    let vars = daemon.shell_env.vars();
    let home = vars
        .get("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_default();
    for socket in crate::workspace_containers::sockets(&vars, &home, None)? {
        if std::os::unix::net::UnixStream::connect(&socket).is_err() {
            continue;
        }
        let Ok(command) = DockerCommand::new(&daemon.shell_env, &socket) else {
            continue;
        };
        let Ok(raw) = command
            .run(
                &["system", "info", "--format", "{{json .ID}}"],
                Duration::from_secs(10),
                4096,
            )
            .await
        else {
            continue;
        };
        if let Ok(id) = serde_json::from_slice::<String>(&raw) {
            return Ok((command, id));
        }
    }
    Err(failed(
        "no Docker engine answers here; install or start Docker",
    ))
}

/// The engine a declaration here pins its services on, which must still be that engine.
async fn pinned_engine(daemon: &Daemon, docker: &DockerBinding) -> Result<(DockerCommand, String)> {
    let gone = || {
        failed(format!(
            "Docker at {}, where this machine kept the workspace's services, does not answer or is another engine now; start it, then resume",
            docker.socket
        ))
    };
    let command =
        DockerCommand::new(&daemon.shell_env, Path::new(&docker.socket)).map_err(|_| gone())?;
    let raw = command
        .run(
            &["system", "info", "--format", "{{json .ID}}"],
            Duration::from_secs(10),
            4096,
        )
        .await
        .map_err(|_| gone())?;
    let id: String = serde_json::from_slice(&raw).map_err(|_| gone())?;
    if id != docker.engine_id {
        return Err(gone());
    }
    Ok((command, id))
}

/// What a destination needs before anything stops on the source: an engine with compose, and
/// none of these compose projects already there. Returns the problems found, in words.
/// Coming back (`returning`), the containers it left here may stay, stopped: they are set aside
/// when it arrives.
pub async fn check(
    daemon: &Daemon,
    workspace: &str,
    projects: &[String],
    returning: bool,
) -> Vec<String> {
    if projects.is_empty() {
        return Vec::new();
    }
    // Coming back, the engine this machine's own declaration pins its services on.
    let here = if returning {
        profiles::load(&daemon.store, workspace).ok().flatten()
    } else {
        None
    };
    let pinned = here.iter().flat_map(|p| &p.profile.services).find(|s| {
        s.compose
            .as_ref()
            .is_some_and(|c| projects.contains(&c.project))
    });
    let found = match pinned {
        Some(service) => pinned_engine(daemon, &service.docker).await,
        None => engine(daemon).await,
    };
    let (command, _) = match found {
        Ok(found) => found,
        Err(_) => {
            return vec![
                "Docker does not answer there, and the workspace declares development services: install or start Docker there".into(),
            ];
        }
    };
    if command
        .run(&["compose", "version"], Duration::from_secs(10), 4096)
        .await
        .is_err()
    {
        return vec!["Docker Compose is not installed there: install the compose plugin".into()];
    }
    let mut problems = Vec::new();
    for project in projects {
        if returning {
            match containers_in(&command, project, Some("running")).await {
                Ok(ids) if ids.is_empty() => {}
                Ok(_) => problems.push(format!(
                    "a container of compose project {project} runs there: stop it (`docker compose -p {project} stop`), it is set aside when the workspace comes back"
                )),
                Err(_) => problems.push("Docker there does not answer".into()),
            }
            continue;
        }
        match project_resources(&command, project).await {
            Ok(0) => {}
            Ok(_) => problems.push(format!(
                "Docker there already has containers or volumes of compose project {project}: remove them (`docker compose -p {project} down -v`) or move them elsewhere"
            )),
            Err(_) => problems.push("Docker there does not answer".into()),
        }
    }
    problems
}

/// How many containers and volumes of `project` Docker holds: services start on fresh ones only,
/// and `down -v` on a retry or a cancellation never reaches data that was there before.
async fn project_resources(command: &DockerCommand, project: &str) -> Result<usize> {
    let filter = format!("label=com.docker.compose.project={project}");
    let raw = command
        .run(
            &[
                "volume",
                "ls",
                "--filter",
                &filter,
                "--format",
                "{{json .Name}}",
            ],
            Duration::from_secs(10),
            256 * 1024,
        )
        .await?;
    let volumes = serde_json::Deserializer::from_slice(&raw)
        .into_iter::<String>()
        .count();
    Ok(volumes + containers_of(command, project, None).await?.len())
}

/// The containers of `project`, only those in `status` when given.
async fn containers_in(
    command: &DockerCommand,
    project: &str,
    status: Option<&str>,
) -> Result<Vec<String>> {
    let project_filter = format!("label=com.docker.compose.project={project}");
    let status_filter = status.map(|s| format!("status={s}"));
    let mut args = vec![
        "container",
        "ls",
        "--all",
        "--no-trunc",
        "--filter",
        &project_filter,
    ];
    if let Some(filter) = &status_filter {
        args.extend(["--filter", filter]);
    }
    args.extend(["--format", "{{json .ID}}"]);
    listed(command, &args).await
}

/// One JSON string per line, as `--format {{json …}}` prints.
async fn listed(command: &DockerCommand, args: &[&str]) -> Result<Vec<String>> {
    let raw = command
        .run(args, Duration::from_secs(10), 256 * 1024)
        .await?;
    serde_json::Deserializer::from_slice(&raw)
        .into_iter()
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| failed("Docker listed its resources unreadably"))
}

/// The volume a project's volume is set aside in when the workspace comes back: named after the
/// operation, labelled so that activation finds it.
fn aside_name(operation: &str, volume: &str) -> String {
    let tail = &operation[operation.len().saturating_sub(12)..];
    format!("chauffeur-aside-{tail}-{volume}")
}

/// The containers and volumes this machine's own declaration of the workspace pins: the only
/// ones a return may set aside and remove.
#[derive(Default)]
struct Owned {
    containers: BTreeSet<String>,
    volumes: BTreeSet<String>,
}

fn owned_here(profile: Option<&StoredProfile>, project: &str) -> Owned {
    let mut owned = Owned::default();
    for service in profile.iter().flat_map(|p| &p.profile.services) {
        if service
            .compose
            .as_ref()
            .is_none_or(|c| c.project != project)
        {
            continue;
        }
        let pin = &service.docker.container;
        owned.containers.insert(pin.id.clone());
        owned.volumes.extend(
            pin.mounts
                .iter()
                .filter(|m| m.kind == "volume")
                .map(|m| m.name.clone()),
        );
    }
    owned
}

/// The image a volume is copied with when the project's own lacks `cp`.
const COPIER: &str = "busybox:1.37";

/// Coming back to a machine that kept the workspace's containers: they must be stopped, and be
/// the ones its declaration here pins (a container or volume of the same compose project that
/// it does not declare, another clone's, refuses). The volumes are copied into aside volumes,
/// then the containers and volumes go, so the services start again on fresh volumes. The aside
/// volumes go at activation, unless the user asked to keep the local copy.
async fn set_aside(
    command: &DockerCommand,
    project: &str,
    workspace: &str,
    operation: &str,
    declared_image: Option<&str>,
    owned: &Owned,
) -> Result<()> {
    if !containers_in(command, project, Some("running"))
        .await?
        .is_empty()
    {
        return Err(failed(format!(
            "a container of compose project {project} runs here; stop it (`docker compose -p {project} stop`)"
        )));
    }
    let project_filter = format!("label=com.docker.compose.project={project}");
    let containers = containers_in(command, project, None).await?;
    let images = listed(
        command,
        &[
            "container",
            "ls",
            "--all",
            "--filter",
            &project_filter,
            "--format",
            "{{json .Image}}",
        ],
    )
    .await?;
    let volumes = listed(
        command,
        &[
            "volume",
            "ls",
            "--filter",
            &project_filter,
            "--format",
            "{{json .Name}}",
        ],
    )
    .await?;
    // A service without a named volume keeps its data in an anonymous one, which carries no
    // compose label: the declaration's pins name it. Only anonymous ones are added, never an
    // external volume a pin also names, which may serve something else.
    let mut volumes = volumes;
    let anonymous = listed(
        command,
        &[
            "volume",
            "ls",
            "--filter",
            "label=com.docker.volume.anonymous",
            "--format",
            "{{json .Name}}",
        ],
    )
    .await?;
    let pinned: Vec<String> = anonymous
        .into_iter()
        .filter(|v| owned.volumes.contains(v) && !volumes.contains(v))
        .collect();
    volumes.extend(pinned);
    let foreign: Vec<String> = containers
        .iter()
        .filter(|id| !owned.containers.contains(*id))
        .map(|id| format!("container {}", id.get(..12).unwrap_or(id)))
        .chain(
            volumes
                .iter()
                .filter(|v| !owned.volumes.contains(*v))
                .map(|v| format!("volume {v}")),
        )
        .collect();
    if !foreign.is_empty() {
        return Err(failed(format!(
            "compose project {project} here holds what this workspace does not declare ({}); remove or rename them, they are never set aside",
            foreign.join(", ")
        )));
    }
    let workspace_label = format!("chauffeur.workspace={workspace}");
    let operation_label = format!("chauffeur.operation={operation}");
    for volume in &volumes {
        let image = images
            .first()
            .map(String::as_str)
            .or(declared_image)
            .ok_or_else(|| {
                failed(format!(
                    "compose project {project} keeps volume {volume} but no image to copy it with"
                ))
            })?;
        let aside = aside_name(operation, volume);
        // A copy an interrupted attempt began is made again: the original is still there.
        let _ = command.run_reporting(&["volume", "rm", &aside], UP).await;
        command
            .run_reporting(
                &[
                    "volume",
                    "create",
                    "--label",
                    &workspace_label,
                    "--label",
                    &operation_label,
                    &aside,
                ],
                UP,
            )
            .await
            .map_err(|why| failed(format!("setting {volume} aside: {why}")))?;
        let from = format!("{volume}:/from:ro");
        let to = format!("{aside}:/to");
        let copy = |image: &'static str| -> Vec<String> {
            [
                "run",
                "--rm",
                "--network",
                "none",
                "--entrypoint",
                "cp",
                "-v",
                &from,
                "-v",
                &to,
                image,
                "-a",
                "/from/.",
                "/to/",
            ]
            .iter()
            .map(|a| a.to_string())
            .collect()
        };
        let own = {
            let mut args = copy(COPIER);
            args[10] = image.to_owned();
            args
        };
        let run = |args: Vec<String>| async move {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            command.run_reporting(&args, UP).await
        };
        // The project's image may lack `cp` (a minimal one): a small copier is pulled then.
        if run(own).await.is_err() {
            run(copy(COPIER))
                .await
                .map_err(|why| failed(format!("copying {volume} aside: {why}")))?;
        }
    }
    if !containers.is_empty() {
        let mut args = vec!["container", "rm"];
        args.extend(containers.iter().map(String::as_str));
        command.run_reporting(&args, UP).await.map_err(|why| {
            failed(format!(
                "removing compose project {project}'s containers: {why}"
            ))
        })?;
    }
    if !volumes.is_empty() {
        let mut args = vec!["volume", "rm"];
        args.extend(volumes.iter().map(String::as_str));
        command.run_reporting(&args, UP).await.map_err(|why| {
            failed(format!(
                "removing compose project {project}'s volumes: {why}"
            ))
        })?;
    }
    Ok(())
}

async fn containers_of(
    command: &DockerCommand,
    project: &str,
    service: Option<&str>,
) -> Result<Vec<String>> {
    let project_filter = format!("label=com.docker.compose.project={project}");
    let service_filter = service.map(|s| format!("label=com.docker.compose.service={s}"));
    let mut args = vec![
        "container",
        "ls",
        "--all",
        "--no-trunc",
        "--filter",
        &project_filter,
    ];
    if let Some(filter) = &service_filter {
        args.extend(["--filter", filter]);
    }
    args.extend(["--format", "{{json .ID}}"]);
    let raw = command
        .run(&args, Duration::from_secs(10), 64 * 1024)
        .await?;
    serde_json::Deserializer::from_slice(&raw)
        .into_iter()
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| failed("Docker listed its containers unreadably"))
}

/// The provisioning recorded for `workspace`: its operation, socket, projects and, once done,
/// the profile pinned to its containers.
struct Recorded {
    operation: String,
    socket: String,
    projects: Vec<String>,
    profile: Option<Profile>,
    /// Coming back, the user asked to keep this machine's earlier copy: its volumes set aside stay.
    keep: bool,
}

fn recorded(daemon: &Daemon, workspace: &str) -> Result<Option<Recorded>> {
    let row: Option<(String, String, String, Option<String>, bool)> = daemon.store.with(|c| {
        c.query_row(
            "SELECT operation_id,socket,projects,profile,keep FROM workspace_service_provisions WHERE workspace_id=?1",
            [workspace],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
    })?;
    row.map(|(operation, socket, projects, profile, keep)| {
        Ok(Recorded {
            operation,
            socket,
            projects: serde_json::from_str(&projects)?,
            profile: profile.map(|p| serde_json::from_str(&p)).transpose()?,
            keep,
        })
    })
    .transpose()
    .map_err(|e: serde_json::Error| Error::Internal(e.into()))
}

/// Removes what a provisioning of `workspace` started (its compose projects with their volumes)
/// and its database restorations' records. The caller has made sure the workspace never ran
/// here from them: a retry before the commit, or a cancelled transfer.
pub async fn remove(daemon: &Daemon, workspace: &str) -> Result<()> {
    let Some(found) = recorded(daemon, workspace)? else {
        return Ok(());
    };
    // A provisioning that started nothing (previews only) names no engine.
    if !found.projects.is_empty() {
        let command =
            DockerCommand::new(&daemon.shell_env, Path::new(&found.socket)).map_err(|_| {
                failed(format!(
                    "Docker at {} does not answer to remove them",
                    found.socket
                ))
            })?;
        for project in &found.projects {
            command
                .run_reporting(
                    &["compose", "-p", project, "down", "-v", "--remove-orphans"],
                    UP,
                )
                .await
                .map_err(|why| failed(format!("removing compose project {project}: {why}")))?;
        }
    }
    daemon.store.with(|c| {
        c.execute(
            "DELETE FROM workspace_database_restores WHERE workspace_id=?1 AND operation_id=?2",
            params![workspace, found.operation],
        )?;
        c.execute(
            "DELETE FROM workspace_service_provisions WHERE workspace_id=?1",
            [workspace],
        )?;
        Ok(())
    })?;
    Ok(())
}

/// Starts the services the source declared, restores its databases into them, and records the
/// profile pinned to them for activation. Done already for this operation: nothing to do.
pub async fn provision(
    daemon: &Arc<Daemon>,
    package: &Path,
    published: &State,
    local: &str,
    returning: bool,
    keep_local_copy: bool,
) -> Result<()> {
    let workspace = &published.binding.workspace_id;
    let operation = &published
        .transfer
        .as_ref()
        .ok_or_else(|| failed("the transfer has no operation"))?
        .operation_id;
    if let Some(found) = recorded(daemon, workspace)? {
        if found.operation == *operation && found.profile.is_some() {
            return Ok(());
        }
        // Started by an attempt that did not finish: from fresh volumes again.
        remove(daemon, workspace).await?;
    }
    let manifest = crate::cloud_workspace_package::verify_published(package, published)
        .map_err(Error::Internal)?
        .manifest;
    let Some(head) = manifest.source_profile.or_else(|| {
        manifest
            .development_data
            .and_then(|data| data.source_profile)
    }) else {
        return Ok(());
    };
    let Some(source) = crate::cloud_workspace_profile::capture(&daemon.store, workspace)?
        .into_iter()
        .find(|profile| profile.head == head)
    else {
        return Err(failed(
            "the source's development profile is missing from the package",
        ));
    };
    let services: Vec<_> = source
        .profile
        .services
        .iter()
        // A declaration saved before its compose project was recorded cannot start here.
        .filter_map(|s| {
            s.compose
                .as_ref()
                .filter(|c| !c.project.is_empty())
                .map(|c| (s, c))
        })
        .collect();
    if services.is_empty() {
        // Nothing to start here; its previews, named by their place in the block, still follow.
        if !source.profile.preview_ports.is_empty() {
            let mut previews = source.profile.clone();
            previews.version = 1;
            previews.services = Vec::new();
            previews.postgres_exports = Vec::new();
            previews.validate()?;
            let pinned = serde_json::to_string(&previews)?;
            daemon.store.with(|c| {
                c.execute(
                    "INSERT INTO workspace_service_provisions(workspace_id,operation_id,socket,projects,keep,profile) VALUES(?1,?2,'','[]',?3,?4)",
                    params![workspace, operation, keep_local_copy, pinned],
                )?;
                Ok(())
            })?;
        }
        return Ok(());
    }
    let root = PathBuf::from(daemon.workspaces.row(workspace)?.path);
    let projects: BTreeSet<&str> = services.iter().map(|(_, c)| c.project.as_str()).collect();
    // This machine's own declaration, from before it left: what a return may set aside, on the
    // engine it pins them on.
    let here = if returning {
        profiles::load(&daemon.store, workspace)?
    } else {
        None
    };
    let pinned = here.iter().flat_map(|p| &p.profile.services).find(|s| {
        s.compose
            .as_ref()
            .is_some_and(|c| projects.contains(c.project.as_str()))
    });
    let (command, engine_id) = match pinned {
        Some(service) => pinned_engine(daemon, &service.docker).await?,
        None => engine(daemon).await?,
    };
    let socket = command.socket_path().to_string_lossy().into_owned();
    for project in &projects {
        if project_resources(&command, project).await? == 0 {
            continue;
        }
        if !returning {
            return Err(failed(format!(
                "compose project {project} already has containers or volumes here"
            )));
        }
        let declared_image = services
            .iter()
            .find(|(_, c)| c.project == *project)
            .and_then(|(s, _)| s.image.as_deref());
        let owned = owned_here(here.as_ref(), project);
        set_aside(
            &command,
            project,
            workspace,
            operation,
            declared_image,
            &owned,
        )
        .await?;
    }
    let listed = serde_json::to_string(&projects)?;
    daemon.store.with(|c| {
        c.execute(
            "INSERT INTO workspace_service_provisions(workspace_id,operation_id,socket,projects,keep) VALUES(?1,?2,?3,?4,?5)",
            params![workspace, operation, socket, listed, keep_local_copy],
        )?;
        Ok(())
    })?;

    // One `up` per compose file, with the services it declares.
    let mut files: BTreeMap<(&str, &str), Vec<&str>> = BTreeMap::new();
    for (_, compose) in &services {
        files
            .entry((compose.project.as_str(), compose.file.as_str()))
            .or_default()
            .push(compose.service.as_str());
    }
    for ((project, file), names) in &files {
        let path = root.join(file);
        let path = path.to_string_lossy();
        let mut args = vec![
            "compose", "-p", project, "-f", &path, "up", "--detach", "--wait",
        ];
        args.extend(names.iter().copied());
        command
            .run_reporting(&args, UP)
            .await
            .map_err(|why| failed(format!("`docker compose up` for {file}: {why}")))?;
    }
    let mut ids = Vec::new();
    for (service, compose) in &services {
        let found = containers_of(&command, &compose.project, Some(&compose.service)).await?;
        let [id] = found.as_slice() else {
            return Err(failed(format!(
                "compose started {} containers for service {}, not one",
                found.len(),
                service.name
            )));
        };
        ids.push(id.clone());
    }
    let observed = DockerServices::observe(
        &daemon.shell_env,
        command.socket_path(),
        engine_id,
        ids.clone(),
    )
    .await
    .map_err(|_| failed("a started container changed while it was read"))?;
    let mut rebound = source.profile.clone();
    rebound.services = Vec::new();
    for ((service, _), id) in services.iter().zip(&ids) {
        let pin = observed
            .containers
            .iter()
            .find(|pin| pin.id == *id)
            .ok_or_else(|| failed("a started container changed while it was read"))?;
        let mut here = (*service).clone();
        // Fresh volumes: what each mount holds is classified as at the source.
        let kinds: BTreeMap<&str, _> = service
            .mounts
            .iter()
            .map(|(k, v)| (k.as_str(), v))
            .collect();
        let fallback = service.writable_layer.clone();
        here.mounts = pin
            .mounts
            .iter()
            .map(|m| {
                (
                    m.destination.clone(),
                    kinds
                        .get(m.destination.as_str())
                        .map_or(fallback.clone(), |v| (*v).clone()),
                )
            })
            .collect();
        here.docker = DockerBinding {
            socket: observed.socket.clone(),
            engine_id: observed.engine_id.clone(),
            container: pin.clone(),
        };
        rebound.services.push(here);
    }
    // Services declared without a compose file did not start here; nothing pins them.
    let exported: BTreeSet<&str> = rebound.services.iter().map(|s| s.name.as_str()).collect();
    rebound
        .postgres_exports
        .retain(|e| exported.contains(e.service.as_str()));
    if rebound.postgres_exports.is_empty() {
        rebound.version = 1;
    }

    for service in rebound
        .services
        .iter()
        .filter(|s| rebound.postgres_exports.iter().any(|e| e.service == s.name))
    {
        let exports: Vec<_> = rebound
            .postgres_exports
            .iter()
            .filter(|e| e.service == service.name)
            .collect();
        let first = exports[0];
        let target = Target {
            docker: service.docker.clone(),
            username: first.username.clone(),
            socket_dir: first.socket_dir.clone(),
            port: first.port,
            maintenance_database: "postgres".into(),
        };
        ready(&command, &target).await?;
        empty_initial_databases(&command, &target).await?;
        let mut present = Vec::new();
        for export in exports {
            let index = packaged(daemon, workspace, &source, &export.name, operation)?;
            restore::restore_packaged_beside(
                daemon.clone(),
                published.clone(),
                local.to_owned(),
                index,
                target.clone(),
                present.clone(),
            )
            .await
            .map_err(|e| failed(format!("restoring database {}: {e}", export.database)))?;
            present.push(export.database.clone());
        }
    }
    rebound.validate()?;
    let pinned = serde_json::to_string(&rebound)?;
    daemon.store.with(|c| {
        c.execute(
            "UPDATE workspace_service_provisions SET profile=?2 WHERE workspace_id=?1",
            params![workspace, pinned],
        )?;
        Ok(())
    })?;
    Ok(())
}

/// The ordinal of the export `name` the source took under `profile` for this transfer
/// (`operation`): a package also carries exports earlier transfers took, under the same name.
fn packaged(
    daemon: &Daemon,
    workspace: &str,
    profile: &StoredProfile,
    name: &str,
    operation: &str,
) -> Result<usize> {
    let rows: Vec<(i64, String)> = daemon.store.with(|c| {
        c.prepare("SELECT ordinal,receipt FROM workspace_imported_database_exports WHERE workspace_id=?1 ORDER BY ordinal")?
            .query_map([workspace], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect()
    })?;
    for (ordinal, raw) in rows {
        let receipt: ExportReceipt =
            serde_json::from_str(&raw).map_err(|e| Error::Internal(e.into()))?;
        let taken_now = receipt
            .source
            .transfer
            .as_ref()
            .is_some_and(|t| t.operation_id == operation);
        if receipt.export_name == name && receipt.profile == *profile && taken_now {
            return Ok(ordinal as usize);
        }
    }
    Err(failed(format!(
        "the export {name} is missing from the package"
    )))
}

fn psql<'a>(target: &'a Target, port: &'a str, database: &'a str, sql: &'a str) -> Vec<&'a str> {
    let mut args = vec!["exec", target.docker.container.id.as_str()];
    args.extend(crate::workspace_postgres_export::CLEAN_LIBPQ);
    args.extend([
        "psql",
        "--no-psqlrc",
        "--no-password",
        "--quiet",
        "--tuples-only",
        "--no-align",
        "--set",
        "ON_ERROR_STOP=1",
        "-h",
        &target.socket_dir,
        "-p",
        port,
        "-U",
        &target.username,
        "-d",
        database,
        "--command",
        sql,
    ]);
    args
}

/// Waits until the server accepts connections on its local socket.
async fn ready(command: &DockerCommand, target: &Target) -> Result<()> {
    let port = target.port.to_string();
    let deadline = tokio::time::Instant::now() + READY;
    loop {
        if command
            .run_checked(
                &psql(target, &port, "postgres", "SELECT 1"),
                Duration::from_secs(10),
                64,
            )
            .await
            .is_ok()
        {
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            return Err(failed(format!(
                "PostgreSQL in service {} did not accept connections within {} s",
                target.docker.container.id.get(..12).unwrap_or_default(),
                READY.as_secs()
            )));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// The official image creates a database at first start (`POSTGRES_DB`, else the user's name).
/// In a server Chauffeur just started on fresh volumes, such a database holds nothing: it is
/// dropped, so the source's databases restore as they were. One holding anything refuses.
async fn empty_initial_databases(command: &DockerCommand, target: &Target) -> Result<()> {
    let port = target.port.to_string();
    let raw = command
        .run_checked(
            &psql(target, &port, "postgres", "SELECT coalesce(json_agg(datname),'[]'::json) FROM pg_database WHERE datname NOT IN ('postgres','template0','template1')"),
            Duration::from_secs(10),
            64 * 1024,
        )
        .await
        .map_err(|_| failed("its databases could not be listed"))?;
    let names: Vec<String> =
        serde_json::from_slice(&raw).map_err(|_| failed("its databases could not be listed"))?;
    for name in names {
        if !profiles::database_identifier(&name) {
            return Err(failed(format!(
                "the new server holds a database named {name:?}"
            )));
        }
        let count = command
            .run_checked(
                &psql(target, &port, &name, "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%'"),
                Duration::from_secs(10),
                64,
            )
            .await
            .map_err(|_| failed(format!("database {name} could not be read")))?;
        if String::from_utf8_lossy(&count).trim() != "0" {
            return Err(failed(format!(
                "the new server's database {name} already holds tables (an init script?); restore needs it empty"
            )));
        }
        let drop = format!("DROP DATABASE \"{name}\"");
        command
            .run_checked(
                &psql(target, &port, "postgres", &drop),
                Duration::from_secs(30),
                64,
            )
            .await
            .map_err(|_| failed(format!("the empty database {name} could not be dropped")))?;
    }
    Ok(())
}

/// At activation: the services started for this operation become the workspace's declared
/// services here, and the records that served the quarantine go. Volumes set aside when it came
/// back are removed in the background, unless the user asked to keep the local copy; a failure
/// to remove them is logged, never undoes the activation.
pub fn activate(daemon: &Arc<Daemon>, workspace: &str, operation: &str) -> Result<()> {
    let Some(found) = recorded(daemon, workspace)? else {
        return Ok(());
    };
    if found.operation != operation {
        return Ok(());
    }
    if let Some(profile) = found.profile {
        profiles::install_arrived(&daemon.store, profile)?;
    }
    daemon.store.with(|c| {
        c.execute(
            "DELETE FROM workspace_database_restores WHERE workspace_id=?1",
            [workspace],
        )?;
        c.execute(
            "DELETE FROM workspace_service_provisions WHERE workspace_id=?1",
            [workspace],
        )?;
        Ok(())
    })?;
    // Nothing was started, so nothing was set aside.
    if found.projects.is_empty() {
        return Ok(());
    }
    let label = format!("label=chauffeur.operation={operation}");
    if found.keep {
        tracing::info!(
            operation,
            "kept the volumes set aside, labelled chauffeur.operation"
        );
        return Ok(());
    }
    let daemon = daemon.clone();
    tokio::spawn(async move {
        let removed = async {
            let command = DockerCommand::new(&daemon.shell_env, Path::new(&found.socket))?;
            let volumes = listed(
                &command,
                &[
                    "volume",
                    "ls",
                    "--filter",
                    &label,
                    "--format",
                    "{{json .Name}}",
                ],
            )
            .await?;
            if !volumes.is_empty() {
                let mut args = vec!["volume", "rm"];
                args.extend(volumes.iter().map(String::as_str));
                command.run_reporting(&args, UP).await.map_err(failed)?;
            }
            Ok::<_, Error>(())
        };
        if let Err(error) = removed.await {
            tracing::warn!(%error, "could not remove the volumes set aside; remove them with `docker volume rm`");
        }
    });
    Ok(())
}

/// On the machine a workspace left for good (its copy deleted after it came back elsewhere): the
/// containers and volumes of its declared services go with it. Their data was exported when it
/// left and restored where it runs now.
pub async fn remove_declared(daemon: &Daemon, workspace: &str) -> Result<()> {
    let Some(profile) = profiles::load(&daemon.store, workspace)? else {
        return Ok(());
    };
    let mut done = BTreeSet::new();
    for service in &profile.profile.services {
        let Some(compose) = service.compose.as_ref().filter(|c| !c.project.is_empty()) else {
            continue;
        };
        if !done.insert((service.docker.socket.clone(), compose.project.clone())) {
            continue;
        }
        // Only on the engine the declaration pins: a socket that now reaches another engine
        // removes nothing.
        let (command, _) = pinned_engine(daemon, &service.docker).await?;
        let project = &compose.project;
        let filter = format!("label=com.docker.compose.project={project}");
        if !containers_in(&command, project, Some("running"))
            .await?
            .is_empty()
        {
            return Err(failed(format!(
                "a container of compose project {project} runs here, though the workspace left; stop it, the deletion is retried"
            )));
        }
        let containers = containers_in(&command, project, None).await?;
        if !containers.is_empty() {
            // `-v`: with them, their anonymous volumes (a service without a named one keeps its
            // data there), which carry no compose label for the listing below.
            let mut args = vec!["container", "rm", "-v"];
            args.extend(containers.iter().map(String::as_str));
            command.run_reporting(&args, UP).await.map_err(failed)?;
        }
        let volumes = listed(
            &command,
            &[
                "volume",
                "ls",
                "--filter",
                &filter,
                "--format",
                "{{json .Name}}",
            ],
        )
        .await?;
        if !volumes.is_empty() {
            let mut args = vec!["volume", "rm"];
            args.extend(volumes.iter().map(String::as_str));
            command.run_reporting(&args, UP).await.map_err(failed)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn docker(args: &[&str]) -> String {
        let output = Command::new("docker").args(args).output().unwrap();
        assert!(
            output.status.success(),
            "docker {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[tokio::test]
    #[ignore = "requires a local Docker engine and the busybox:1.37 image"]
    async fn a_return_sets_aside_the_anonymous_volume_its_pin_names_and_never_an_external_one() {
        let project = format!("chauffeur-aside-test-{}", std::process::id());
        let external = format!("{project}-external");
        let operation = "op-0123456789ab";
        docker(&["volume", "create", &external]);
        docker(&[
            "run",
            "--name",
            &project,
            "--label",
            &format!("com.docker.compose.project={project}"),
            "-v",
            "/data",
            "-v",
            &format!("{external}:/shared"),
            "busybox:1.37",
            "sh",
            "-c",
            "echo kept > /data/f && echo shared > /shared/f",
        ]);
        let container = docker(&["inspect", "--format", "{{.Id}}", &project]);
        let mounts = docker(&[
            "inspect",
            "--format",
            "{{range .Mounts}}{{.Name}} {{end}}",
            &project,
        ]);
        let anonymous = mounts
            .split_whitespace()
            .find(|name| *name != external)
            .unwrap()
            .to_owned();
        let owned = Owned {
            containers: [container].into(),
            volumes: [anonymous.clone(), external.clone()].into(),
        };
        let host = docker(&[
            "context",
            "inspect",
            "--format",
            "{{.Endpoints.docker.Host}}",
        ]);
        let socket = PathBuf::from(host.trim_start_matches("unix://"));
        let command = DockerCommand::new(&crate::shellenv::ShellEnv::new(), &socket).unwrap();

        let result = set_aside(
            &command,
            &project,
            "workspace",
            operation,
            Some("busybox:1.37"),
            &owned,
        )
        .await;

        let aside = aside_name(operation, &anonymous);
        let copied = Command::new("docker")
            .args(["run", "--rm", "-v", &format!("{aside}:/a"), "busybox:1.37"])
            .args(["cat", "/a/f"])
            .output()
            .unwrap();
        let left = |name: &str| {
            Command::new("docker")
                .args(["volume", "inspect", name])
                .output()
                .unwrap()
                .status
                .success()
        };
        let (anonymous_left, external_left) = (left(&anonymous), left(&external));
        for volume in [&aside, &anonymous, &external] {
            let _ = Command::new("docker")
                .args(["volume", "rm", volume])
                .output();
        }
        let _ = Command::new("docker").args(["rm", "-f", &project]).output();
        result.unwrap();
        assert_eq!(String::from_utf8_lossy(&copied.stdout).trim(), "kept");
        assert!(
            !anonymous_left,
            "the anonymous volume is removed once set aside"
        );
        assert!(external_left, "an external volume is never removed");
    }
}
