//! A workspace's development services, as the user declares them (docs/workspace-development-
//! services.md): detection finds the Docker containers that work or write in the workspace and
//! the PostgreSQL databases they hold; saving turns the user's choices (data that follows the
//! workspace, or a cache rebuilt elsewhere) into its development profile, pinned to the
//! containers as they run now. Detection only reads; Docker is never changed here.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chauffeur_protocol::dev_services::{
    ComposeService, DeclaredService, DetectedPostgres, DetectedService, DevServices,
    DevServicesDetected, DevServicesSaveParams,
};
use serde::Deserialize;

use crate::workspace_containers::{self as containers, canonical};
use crate::workspace_development_profile::{
    self as profiles, Disposition, DockerBinding, MeaningfulKind, PostgresExport, Profile, Service,
    StoredProfile,
};
use crate::workspace_docker_command::DockerCommand;
use crate::workspace_service_stop::DockerServices;
use crate::{Daemon, Error, Result};

const INSPECT: &str = r#"{"id":{{json .Id}},"name":{{json .Name}},"image":{{json .Config.Image}},"running":{{json .State.Running}},"mounts":{{json .Mounts}},"env":{{json .Config.Env}},"auto_remove":{{json .HostConfig.AutoRemove}},"restart_policy":{{json .HostConfig.RestartPolicy.Name}},"has_tmpfs":{{if index .HostConfig "Tmpfs"}}true{{else}}false{{end}},"project":{{json (index .Config.Labels "com.docker.compose.project")}},"config_files":{{json (index .Config.Labels "com.docker.compose.project.config_files")}},"service":{{json (index .Config.Labels "com.docker.compose.service")}}}"#;
const MAX_CONTAINERS: usize = 512;
/// Where the official PostgreSQL images listen.
const POSTGRES_SOCKET_DIR: &str = "/var/run/postgresql";

#[derive(Deserialize)]
struct Inspection {
    id: String,
    name: String,
    image: String,
    running: bool,
    #[serde(default)]
    env: Option<Vec<String>>,
    auto_remove: bool,
    restart_policy: String,
    has_tmpfs: bool,
    project: Option<String>,
    config_files: Option<String>,
    service: Option<String>,
}

impl Inspection {
    fn env(&self, key: &str) -> Option<&str> {
        self.env
            .iter()
            .flatten()
            .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
    }

    /// Why Chauffeur could not stop and start it again exactly, if it could not.
    fn pinning_problem(&self) -> Option<String> {
        if self.restart_policy != "no" && !self.restart_policy.is_empty() {
            return Some(format!(
                "Docker restarts it on its own (restart: {}); set `restart: \"no\"` for it, recreate it, then detect again",
                self.restart_policy
            ));
        }
        if self.auto_remove {
            return Some(
                "Docker removes it when it stops (--rm); start it without, then detect again"
                    .into(),
            );
        }
        if self.has_tmpfs {
            return Some(
                "it keeps files in a tmpfs, lost when it stops; remove the tmpfs, then detect again"
                    .into(),
            );
        }
        None
    }

    fn postgres(&self) -> bool {
        let image = self.image.rsplit('/').next().unwrap_or_default();
        image.starts_with("postgres")
            || image.starts_with("postgis")
            || self.env("PG_MAJOR").is_some()
    }

    fn username(&self) -> String {
        self.env("POSTGRES_USER")
            .filter(|user| profiles::database_identifier(user))
            .unwrap_or("postgres")
            .to_owned()
    }

    fn port(&self) -> u16 {
        self.env("PGPORT")
            .and_then(|port| port.parse().ok())
            .filter(|port| *port != 0)
            .unwrap_or(5432)
    }
}

/// A detected container with what saving it needs.
struct Found {
    shown: DetectedService,
    id: String,
    socket: PathBuf,
    engine: String,
    port: u16,
    /// A declared service whose container no engine shows now.
    gone: bool,
}

/// A service name a profile accepts: letters, digits, `_` and `-`.
fn service_name(raw: &str) -> String {
    let name: String = raw
        .trim_start_matches('/')
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    if name.is_empty() {
        "service".into()
    } else {
        name
    }
}

/// The declared services of `workspace`.
pub fn get(daemon: &Daemon, workspace: &str) -> Result<DevServices> {
    let row = daemon.workspaces.row(workspace)?;
    declared(
        daemon,
        &row.id,
        profiles::load(&daemon.store, &row.id)?.as_ref(),
    )
}

fn declared(
    daemon: &Daemon,
    workspace_id: &str,
    profile: Option<&StoredProfile>,
) -> Result<DevServices> {
    let Some(stored) = profile else {
        return Ok(DevServices {
            revision: None,
            services: Vec::new(),
            previews: Vec::new(),
        });
    };
    let block = daemon.store.port_block(workspace_id)?;
    let exports = &stored.profile.postgres_exports;
    Ok(DevServices {
        revision: Some(stored.head.revision),
        services: stored
            .profile
            .services
            .iter()
            .map(|service| {
                let databases: Vec<String> = exports
                    .iter()
                    .filter(|export| export.service == service.name)
                    .map(|export| export.database.clone())
                    .collect();
                DeclaredService {
                    name: service.name.clone(),
                    image: service.image.clone(),
                    compose: service.compose.clone(),
                    data: !databases.is_empty(),
                    databases,
                }
            })
            .collect(),
        previews: crate::workspace_previews::declared(
            block.as_ref(),
            &stored.profile.preview_ports,
        ),
    })
}

/// The containers that work or write in `workspace`, stopped ones included, and the ports of
/// its block that answer HTTP.
pub async fn detect(daemon: &Arc<Daemon>, workspace: &str) -> Result<DevServicesDetected> {
    let row = daemon.workspaces.row(workspace)?;
    let found = find(daemon, &row.id).await?;
    let profile = profiles::load(&daemon.store, &row.id)?;
    let declared = profile
        .as_ref()
        .map(|p| p.profile.preview_ports.as_slice())
        .unwrap_or_default();
    Ok(DevServicesDetected {
        services: found.into_iter().map(|f| f.shown).collect(),
        previews: crate::workspace_previews::detect(daemon, &row.id, declared).await?,
    })
}

async fn find(daemon: &Arc<Daemon>, workspace: &str) -> Result<Vec<Found>> {
    let row = daemon.workspaces.row(workspace)?;
    let folder = PathBuf::from(&row.path).canonicalize()?;
    let roots = crate::workspace_source_stop::workspace_roots(daemon, &row.id)?;
    let profile = profiles::load(&daemon.store, &row.id)?;
    let declared: BTreeSet<&str> = profile
        .iter()
        .flat_map(|p| &p.profile.services)
        .map(|s| s.docker.container.id.as_str())
        .collect();
    let env = &daemon.shell_env;
    let vars = env.vars();
    let home = vars
        .get("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_default();
    let mut found = Vec::new();
    let mut names = BTreeSet::new();
    let mut engines = BTreeSet::new();
    for socket in containers::sockets(&vars, &home, profile.as_ref())? {
        // Nothing listens there: no engine, nothing to detect.
        if std::os::unix::net::UnixStream::connect(&socket).is_err() {
            continue;
        }
        let unanswered = || {
            Error::Unavailable(format!(
                "Docker at {} does not answer; start it, then detect again",
                socket.display()
            ))
        };
        let command = DockerCommand::new(env, &socket).map_err(|_| unanswered())?;
        let engine: String = serde_json::from_slice(
            &command
                .run(
                    &["system", "info", "--format", "{{json .ID}}"],
                    Duration::from_secs(10),
                    4096,
                )
                .await
                .map_err(|_| unanswered())?,
        )
        .map_err(|_| unanswered())?;
        // One engine can listen on several sockets.
        if !engines.insert(engine.clone()) {
            continue;
        }
        let listed = command
            .run(
                &[
                    "container",
                    "ls",
                    "--all",
                    "--no-trunc",
                    "--format",
                    "{{json .ID}}",
                ],
                Duration::from_secs(10),
                256 * 1024,
            )
            .await
            .map_err(|_| unanswered())?;
        let ids: Vec<String> = serde_json::Deserializer::from_slice(&listed)
            .into_iter()
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| unanswered())?;
        if ids.is_empty() {
            continue;
        }
        if ids.len() > MAX_CONTAINERS {
            return Err(unanswered());
        }
        let mut args = vec!["container", "inspect", "--format", INSPECT];
        args.extend(ids.iter().map(String::as_str));
        let output = command
            .run(&args, Duration::from_secs(10), 1024 * 1024)
            .await
            .map_err(|_| unanswered())?;
        let data = containers::data_mounts(profile.as_ref());
        for raw in serde_json::Deserializer::from_slice(&output).into_iter::<serde_json::Value>() {
            let raw = raw.map_err(|_| unanswered())?;
            let seen: Inspection = serde_json::from_value(raw.clone()).map_err(|_| unanswered())?;
            let check: containers::Inspection =
                serde_json::from_value(raw).map_err(|_| unanswered())?;
            if !declared.contains(seen.id.as_str())
                && containers::writes_in(&check, &roots, &data).is_none()
            {
                continue;
            }
            let compose = compose_of(&seen, &folder);
            let mut problem = seen.pinning_problem();
            if compose.is_none() && problem.is_none() {
                problem = Some(
                    "not started from a compose file of the workspace: another machine cannot start it again, so only a cache rebuilt there can be declared".into(),
                );
            }
            let postgres = if seen.postgres() {
                Some(DetectedPostgres {
                    username: seen.username(),
                    databases: if seen.running {
                        databases(&command, &seen).await?
                    } else {
                        Vec::new()
                    },
                })
            } else {
                None
            };
            let mut name = service_name(
                seen.service
                    .as_deref()
                    .filter(|_| compose.is_some())
                    .unwrap_or(&seen.name),
            );
            if !names.insert(name.clone()) {
                name = service_name(&format!("{name}-{}", &seen.id[..8.min(seen.id.len())]));
                names.insert(name.clone());
            }
            found.push(Found {
                shown: DetectedService {
                    name,
                    container: seen.id[..12.min(seen.id.len())].to_owned(),
                    image: seen.image.clone(),
                    running: seen.running,
                    compose,
                    data: postgres.is_some(),
                    postgres,
                    problem,
                },
                port: seen.port(),
                id: seen.id,
                socket: socket.clone(),
                engine: engine.clone(),
                gone: false,
            });
        }
    }
    // A declared service no engine shows now (its engine stopped, its container removed) is
    // listed as gone rather than left out, which saving would turn into dropping it unseen.
    for service in profile.iter().flat_map(|p| &p.profile.services) {
        let pin = &service.docker.container;
        // Recreated by compose (a new container under the same service): the live one replaces
        // it, and saving pins that one.
        // The same compose project, file and service on the same engine; a container elsewhere
        // that only shares its name is another service.
        if found.iter().any(|f| {
            f.id == pin.id
                || (f.engine == service.docker.engine_id
                    && service.compose.is_some()
                    && f.shown.compose == service.compose)
        }) {
            continue;
        }
        let short = pin.id.get(..12).unwrap_or(&pin.id).to_owned();
        let data = profile
            .iter()
            .flat_map(|p| &p.profile.postgres_exports)
            .any(|e| e.service == service.name);
        found.push(Found {
            shown: DetectedService {
                name: service.name.clone(),
                container: short.clone(),
                image: service.image.clone().unwrap_or_else(|| pin.image.clone()),
                running: false,
                compose: service.compose.clone(),
                postgres: None,
                data,
                problem: Some(format!(
                    "its container {short} is not found on Docker at {}: start that engine and detect again, or leave it out to stop declaring it",
                    service.docker.socket
                )),
            },
            id: pin.id.clone(),
            socket: PathBuf::from(&service.docker.socket),
            engine: service.docker.engine_id.clone(),
            port: 5432,
            gone: true,
        });
    }
    found.sort_by(|a, b| a.shown.name.cmp(&b.shown.name));
    Ok(found)
}

/// Its compose file, relative to the workspace folder, when it lies in it.
fn compose_of(seen: &Inspection, folder: &Path) -> Option<ComposeService> {
    let service = seen.service.as_ref()?;
    let file = seen
        .config_files
        .as_deref()?
        .split(',')
        .map(|file| canonical(file.trim()))
        .find(|file| file.starts_with(folder))?;
    let compose = ComposeService {
        project: seen.project.clone()?,
        file: file.strip_prefix(folder).ok()?.to_str()?.to_owned(),
        service: service.clone(),
    };
    profiles::compose(&compose).then_some(compose)
}

/// The databases a running PostgreSQL container holds, besides its maintenance one.
async fn databases(command: &DockerCommand, seen: &Inspection) -> Result<Vec<String>> {
    let port = seen.port().to_string();
    let username = seen.username();
    let mut args = vec!["exec", &seen.id];
    args.extend(crate::workspace_postgres_export::CLEAN_LIBPQ);
    args.extend([
        "psql", "-X", "-A", "-t", "--no-password", "-h", POSTGRES_SOCKET_DIR, "-p", &port, "-U",
        &username, "-d", "postgres", "--command",
        "SELECT datname FROM pg_database WHERE NOT datistemplate AND datname <> 'postgres' ORDER BY 1",
    ]);
    let output = command
        .run(&args, Duration::from_secs(15), 64 * 1024)
        .await
        .map_err(|_| {
            Error::Unavailable(format!(
                "Could not list the databases of {} as {username}; check it accepts local connections, then detect again",
                seen.name.trim_start_matches('/')
            ))
        })?;
    let text = String::from_utf8_lossy(&output);
    let mut names = Vec::new();
    for name in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !profiles::database_identifier(name) {
            return Err(Error::Conflict(format!(
                "Database {name:?} has a name Chauffeur cannot export (letters, digits, _ and - only); rename it, then detect again"
            )));
        }
        names.push(name.to_owned());
    }
    Ok(names)
}

/// Declares the chosen services, as detected now, as the workspace's development services.
pub async fn save(daemon: &Arc<Daemon>, params: DevServicesSaveParams) -> Result<DevServices> {
    let row = daemon.workspaces.row(&params.workspace)?;
    let current = profiles::load(&daemon.store, &row.id)?;
    if current.as_ref().map(|p| p.head.revision) != params.revision {
        return Err(Error::Conflict(
            "The development services changed since they were shown; detect them again".into(),
        ));
    }
    let found = find(daemon, &row.id).await?;
    let mut chosen = Vec::new();
    let mut seen = BTreeSet::new();
    for choice in &params.services {
        if !seen.insert(choice.name.as_str()) {
            return Err(Error::BadRequest(format!(
                "{} is chosen twice",
                choice.name
            )));
        }
        let service = found
            .iter()
            .find(|f| f.shown.name == choice.name)
            .ok_or_else(|| {
                Error::Conflict(format!(
                    "{} is no longer detected in the workspace; detect again",
                    choice.name
                ))
            })?;
        if let Some(problem) = &service.shown.problem
            && (service.gone || choice.data || service.shown.compose.is_some())
        {
            return Err(Error::Conflict(format!("{}: {problem}", choice.name)));
        }
        if choice.data {
            let Some(postgres) = &service.shown.postgres else {
                return Err(Error::Conflict(format!(
                    "Only a PostgreSQL server's data can follow the workspace for now; declare {} as a cache",
                    choice.name
                )));
            };
            if !service.shown.running {
                return Err(Error::Conflict(format!(
                    "Start {} so that Chauffeur can read its databases, then detect again",
                    choice.name
                )));
            }
            if postgres.databases.is_empty() {
                return Err(Error::Conflict(format!(
                    "{} holds no database besides postgres; declare it as a cache",
                    choice.name
                )));
            }
        }
        chosen.push((choice.data, service));
    }
    let mut services = Vec::new();
    let mut exports = Vec::new();
    if let Some((_, first)) = chosen.first() {
        if chosen
            .iter()
            .any(|(_, f)| f.socket != first.socket || f.engine != first.engine)
        {
            return Err(Error::Conflict(
                "The chosen services run on different Docker engines; keep those of one engine"
                    .into(),
            ));
        }
        let observed = DockerServices::observe(
            &daemon.shell_env,
            &first.socket,
            first.engine.clone(),
            chosen.iter().map(|(_, f)| f.id.clone()).collect(),
        )
        .await?;
        let pins: BTreeMap<&str, _> = observed
            .containers
            .iter()
            .map(|pin| (pin.id.as_str(), pin))
            .collect();
        for (data, found) in &chosen {
            let pin = (*pins.get(found.id.as_str()).ok_or_else(|| {
                Error::Conflict("A chosen container changed while it was read; detect again".into())
            })?)
            .clone();
            let kept = || {
                if *data {
                    Disposition::Preserve {
                        data_kind: MeaningfulKind::Database,
                        reason: "PostgreSQL data, exported when the workspace moves".into(),
                    }
                } else {
                    Disposition::Reconstructible
                }
            };
            let mounts: BTreeMap<String, Disposition> = pin
                .mounts
                .iter()
                .map(|mount| (mount.destination.clone(), kept()))
                .collect();
            let writable_layer = if mounts.is_empty() {
                kept()
            } else {
                Disposition::Reconstructible
            };
            if *data && let Some(postgres) = &found.shown.postgres {
                for database in &postgres.databases {
                    exports.push(PostgresExport {
                        name: service_name(&format!("{}-{database}", found.shown.name)),
                        service: found.shown.name.clone(),
                        database: database.clone(),
                        username: postgres.username.clone(),
                        socket_dir: POSTGRES_SOCKET_DIR.into(),
                        port: found.port,
                    });
                }
            }
            services.push(Service {
                name: found.shown.name.clone(),
                required: *data,
                docker: DockerBinding {
                    socket: observed.socket.clone(),
                    engine_id: observed.engine_id.clone(),
                    container: pin,
                },
                runtime_memory: Disposition::Reconstructible,
                writable_layer,
                mounts,
                compose: found.shown.compose.clone(),
                image: profiles::image_reference(&found.shown.image)
                    .then(|| found.shown.image.clone()),
            });
        }
    }
    let profile = Profile {
        version: if exports.is_empty() { 1 } else { 2 },
        workspace_id: row.id.clone(),
        linux_setup: current
            .as_ref()
            .map(|p| p.profile.linux_setup.clone())
            .unwrap_or_default(),
        linux_startup: current
            .as_ref()
            .map(|p| p.profile.linux_startup.clone())
            .unwrap_or_default(),
        services,
        preview_ports: match &params.previews {
            Some(choices) => crate::workspace_previews::chosen(daemon, &row.id, choices)?,
            None => current
                .as_ref()
                .map(|p| p.profile.preview_ports.clone())
                .unwrap_or_default(),
        },
        postgres_exports: exports,
    };
    let saved = profiles::save(daemon, profile, current.as_ref().map(|p| &p.head))?;
    declared(daemon, &row.id, Some(&saved))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inspection(value: serde_json::Value) -> Inspection {
        let mut base = serde_json::json!({"id": "a".repeat(64), "name": "/shop-db-1",
            "image": "postgres:16", "running": true, "mounts": [], "env": ["POSTGRES_USER=shop", "PG_MAJOR=16"],
            "auto_remove": false, "restart_policy": "no", "has_tmpfs": false,
            "project": "shop", "config_files": null, "service": "db"});
        for (key, field) in value.as_object().unwrap() {
            base[key] = field.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn a_postgres_container_is_recognised_with_its_user_and_port() {
        let db = inspection(serde_json::json!({}));
        assert!(db.postgres());
        assert_eq!(db.username(), "shop");
        assert_eq!(db.port(), 5432);
        let redis = inspection(serde_json::json!({"image": "redis:7", "env": []}));
        assert!(!redis.postgres());
        assert_eq!(redis.username(), "postgres");
    }

    #[test]
    fn a_container_docker_restarts_or_removes_itself_cannot_be_pinned() {
        assert!(
            inspection(serde_json::json!({}))
                .pinning_problem()
                .is_none()
        );
        let restarts = inspection(serde_json::json!({"restart_policy": "unless-stopped"}));
        assert!(
            restarts
                .pinning_problem()
                .unwrap()
                .contains("restart: \"no\"")
        );
        assert!(
            inspection(serde_json::json!({"auto_remove": true}))
                .pinning_problem()
                .is_some()
        );
    }

    #[test]
    fn its_compose_file_is_named_relative_to_the_workspace_and_must_lie_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(folder.join("shop-api")).unwrap();
        let file = folder.join("shop-api/docker-compose.yml");
        std::fs::write(&file, "services: {}\n").unwrap();
        let inside = inspection(serde_json::json!({"config_files": file.to_str().unwrap()}));
        assert_eq!(
            compose_of(&inside, &folder),
            Some(ComposeService {
                project: "shop".into(),
                file: "shop-api/docker-compose.yml".into(),
                service: "db".into()
            })
        );
        let elsewhere =
            inspection(serde_json::json!({"config_files": "/srv/other/docker-compose.yml"}));
        assert_eq!(compose_of(&elsewhere, &folder), None);
    }

    #[test]
    fn names_keep_only_what_a_profile_accepts() {
        assert_eq!(service_name("/shop.db_1"), "shop-db_1");
        assert_eq!(service_name(""), "service");
    }
}
