//! Checks, once a source's declared services stopped, that no running container still writes in
//! the workspace: a writable bind mount of one of its folders, a compose project defined in it,
//! or a volume shared with a service declared as holding data. Read-only; it stops nothing.
//! Every engine the user's `docker` could reach is checked: the profile's; `DOCKER_HOST` when
//! set (as for the docker CLI, it overrides the rest); else the current docker context's and
//! every known engine socket (Docker Desktop, OrbStack, Colima, Rancher, rootless, default).
//! An engine on another host (`tcp://`, `ssh://`) refuses: its bind mounts are not these folders.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    shellenv::ShellEnv,
    workspace_development_profile::{Disposition, StoredProfile},
    workspace_docker_command::DockerCommand,
    workspace_service_stop::ServiceMount,
};

const INSPECT: &str = r#"{"id":{{json .Id}},"name":{{json .Name}},"mounts":{{json .Mounts}},"project":{{json (index .Config.Labels "com.docker.compose.project")}},"config_files":{{json (index .Config.Labels "com.docker.compose.project.config_files")}}}"#;
const MAX_CONTAINERS: usize = 512;

/// What the check saw, per engine socket found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerCheck {
    pub engines: Vec<EngineSeen>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineSeen {
    pub socket: String,
    /// None when nothing listens at the socket: no container can run there.
    pub engine_id: Option<String>,
    pub running: usize,
}

#[derive(Deserialize)]
pub(crate) struct Inspection {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) mounts: Vec<ServiceMount>,
    pub(crate) project: Option<String>,
    pub(crate) config_files: Option<String>,
}

fn unanswered(socket: &Path) -> Error {
    Error::Unavailable(format!(
        "Docker at {} does not answer: start it so that Chauffeur can check no container writes in the workspace, then retry the transfer",
        socket.display()
    ))
}

/// The engine sockets to check, canonical and distinct, existing or not. `home` is the user's.
pub fn sockets(
    vars: &BTreeMap<String, String>,
    home: &Path,
    profile: Option<&StoredProfile>,
) -> Result<Vec<PathBuf>> {
    let mut found: Vec<PathBuf> = profile
        .iter()
        .flat_map(|p| &p.profile.services)
        .map(|service| PathBuf::from(&service.docker.socket))
        .collect();
    let unix = |host: &str| -> Result<PathBuf> {
        host.strip_prefix("unix://").map(PathBuf::from).ok_or_else(|| {
            Error::Conflict(format!(
                "Docker points to {host}, an engine Chauffeur cannot check for containers writing in the workspace; use a local engine, then retry the transfer"
            ))
        })
    };
    if let Some(host) = vars.get("DOCKER_HOST").filter(|h| !h.is_empty()) {
        found.push(unix(host)?);
    } else {
        let config = vars
            .get("DOCKER_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".docker"));
        if let Some(host) = context_host(&config) {
            found.push(unix(&host)?);
        }
        found.extend(
            [
                ".docker/run/docker.sock",
                ".orbstack/run/docker.sock",
                ".colima/default/docker.sock",
                ".colima/docker.sock",
                ".rd/docker.sock",
            ]
            .iter()
            .map(|path| home.join(path)),
        );
        found.push(PathBuf::from("/var/run/docker.sock"));
        if let Some(runtime) = vars.get("XDG_RUNTIME_DIR") {
            found.push(Path::new(runtime).join("docker.sock"));
        }
    }
    let mut distinct = Vec::new();
    for socket in found {
        let socket = socket.canonicalize().unwrap_or(socket);
        if !distinct.contains(&socket) {
            distinct.push(socket);
        }
    }
    Ok(distinct)
}

/// The Unix endpoint of the docker CLI's current context, from its own files.
fn context_host(config: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config.join("config.json")).ok()?).ok()?;
    let name = settings.get("currentContext")?.as_str()?;
    if name.is_empty() || name == "default" {
        return None;
    }
    let meta = config
        .join("contexts/meta")
        .join(hex::encode(Sha256::digest(name.as_bytes())))
        .join("meta.json");
    let meta: serde_json::Value = serde_json::from_slice(&std::fs::read(meta).ok()?).ok()?;
    Some(meta.pointer("/Endpoints/docker/Host")?.as_str()?.to_owned())
}

pub(crate) fn canonical(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    path.canonicalize().unwrap_or(path)
}

/// Why `container` writes in the workspace, if it does. `roots` must be canonical.
pub(crate) fn writes_in(
    container: &Inspection,
    roots: &[PathBuf],
    data: &DataMounts,
) -> Option<String> {
    let inside = |path: &Path| roots.iter().any(|root| path.starts_with(root));
    // A bind of the workspace's parent (the home folder, ~/chauffeur) writes in it too.
    let overlaps = |path: &Path| inside(path) || roots.iter().any(|root| root.starts_with(path));
    if let Some(mount) = container
        .mounts
        .iter()
        .find(|m| m.kind == "bind" && m.writable && overlaps(&canonical(&m.source)))
    {
        return Some(format!("mounts {} writable", mount.source));
    }
    if let Some(files) = &container.config_files
        && files.split(',').any(|file| inside(&canonical(file.trim())))
    {
        return Some(format!(
            "belongs to compose project {} defined in the workspace",
            container.project.as_deref().unwrap_or("?")
        ));
    }
    container
        .mounts
        .iter()
        .find(|m| m.kind == "volume" && data.volumes.contains(&m.name))
        .map(|mount| format!("shares the data volume {}", mount.name))
        .or_else(|| {
            let shared = |path: &Path| {
                data.binds
                    .iter()
                    .any(|bind| path.starts_with(bind) || bind.starts_with(path))
            };
            container
                .mounts
                .iter()
                .find(|m| m.kind == "bind" && m.writable && shared(&canonical(&m.source)))
                .map(|mount| format!("shares the data folder {}", mount.source))
        })
}

pub(crate) fn data_mounts(profile: Option<&StoredProfile>) -> DataMounts {
    let mut data = DataMounts::default();
    for service in profile.iter().flat_map(|p| &p.profile.services) {
        for mount in &service.docker.container.mounts {
            let declared = service.mounts.get(&mount.destination);
            if !declared.is_some_and(|value| *value != Disposition::Reconstructible) {
                continue;
            }
            match mount.kind.as_str() {
                "volume" => {
                    data.volumes.insert(mount.name.clone());
                }
                "bind" => data.binds.push(canonical(&mount.source)),
                _ => {}
            }
        }
    }
    data
}

/// Where declared services keep data a running container could change.
#[derive(Default)]
pub(crate) struct DataMounts {
    volumes: BTreeSet<String>,
    binds: Vec<PathBuf>,
}

/// Refuses, naming them, while running containers write in the workspace. A socket where
/// nothing listens is no engine; one that listens but does not answer refuses.
pub async fn check(
    env: &ShellEnv,
    profile: Option<&StoredProfile>,
    roots: &[PathBuf],
) -> Result<ContainerCheck> {
    let vars = env.vars();
    let home = vars
        .get("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_default();
    let mut engines = Vec::new();
    let mut seen_ids = BTreeSet::new();
    let mut writers = Vec::new();
    for socket in sockets(&vars, &home, profile)? {
        if let Err(error) = std::os::unix::net::UnixStream::connect(&socket) {
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) {
                engines.push(EngineSeen {
                    socket: socket.to_string_lossy().into_owned(),
                    engine_id: None,
                    running: 0,
                });
                continue;
            }
            return Err(unanswered(&socket));
        }
        let (engine, running, found) = check_engine(env, &socket, profile, roots).await?;
        if let Some(declared) = profile
            .iter()
            .flat_map(|p| &p.profile.services)
            .find(|service| canonical(&service.docker.socket) == socket)
            .filter(|service| service.docker.engine_id != engine)
        {
            return Err(Error::Conflict(format!(
                "Docker at {} is not the engine its development profile declares ({}); declare the services again on this engine, then retry the transfer",
                socket.display(),
                declared.docker.engine_id
            )));
        }
        // One engine can listen on several sockets (a link the canonical path did not join).
        if seen_ids.insert(engine.clone()) {
            writers.extend(found);
            engines.push(EngineSeen {
                socket: socket.to_string_lossy().into_owned(),
                engine_id: Some(engine),
                running,
            });
        }
    }
    if !writers.is_empty() {
        return Err(Error::Conflict(format!(
            "Containers still run and write in this workspace: {}. Stop them, or declare them as services of its development profile, then retry the transfer",
            writers.join("; ")
        )));
    }
    Ok(ContainerCheck { engines })
}

/// One engine: its id, its running containers and those writing in the workspace.
async fn check_engine(
    env: &ShellEnv,
    socket: &Path,
    profile: Option<&StoredProfile>,
    roots: &[PathBuf],
) -> Result<(String, usize, Vec<String>)> {
    let mut writers = Vec::new();
    let command = DockerCommand::new(env, socket).map_err(|_| unanswered(socket))?;
    let engine = command
        .run(
            &["system", "info", "--format", "{{json .ID}}"],
            Duration::from_secs(10),
            4096,
        )
        .await
        .map_err(|_| unanswered(socket))?;
    let engine: String = serde_json::from_slice(&engine).map_err(|_| unanswered(socket))?;
    let listed = command
        .run(
            &["container", "ls", "--no-trunc", "--format", "{{json .ID}}"],
            Duration::from_secs(10),
            256 * 1024,
        )
        .await
        .map_err(|_| unanswered(socket))?;
    let ids: Vec<String> = serde_json::Deserializer::from_slice(&listed)
        .into_iter()
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| unanswered(socket))?;
    if ids.len() > MAX_CONTAINERS {
        return Err(unanswered(socket));
    }
    if !ids.is_empty() {
        let mut args = vec!["container", "inspect", "--format", INSPECT];
        args.extend(ids.iter().map(String::as_str));
        let output = command
            .run(&args, Duration::from_secs(10), 1024 * 1024)
            .await
            .map_err(|_| unanswered(socket))?;
        let data = data_mounts(profile);
        for container in serde_json::Deserializer::from_slice(&output).into_iter::<Inspection>() {
            let container = container.map_err(|_| unanswered(socket))?;
            if let Some(why) = writes_in(&container, roots, &data) {
                writers.push(format!(
                    "{} ({}) {why}",
                    container.name.trim_start_matches('/'),
                    &container.id[..container.id.len().min(12)]
                ));
            }
        }
    }
    Ok((engine, ids.len(), writers))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(mounts: serde_json::Value, files: Option<&str>) -> Inspection {
        serde_json::from_value(serde_json::json!({"id": "a".repeat(64), "name": "/app",
            "mounts": mounts, "project": files.map(|_| "web"), "config_files": files}))
        .unwrap()
    }

    #[test]
    fn docker_host_overrides_the_context_and_known_sockets_and_a_remote_one_refuses() {
        use sha2::{Digest, Sha256};
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        let meta = home
            .join(".docker/contexts/meta")
            .join(hex::encode(Sha256::digest(b"orbstack")));
        std::fs::create_dir_all(&meta).unwrap();
        std::fs::write(
            home.join(".docker/config.json"),
            r#"{"currentContext":"orbstack"}"#,
        )
        .unwrap();
        std::fs::write(
            meta.join("meta.json"),
            r#"{"Name":"orbstack","Endpoints":{"docker":{"Host":"unix:///context/docker.sock"}}}"#,
        )
        .unwrap();
        let found = sockets(&BTreeMap::new(), &home, None).unwrap();
        assert_eq!(found[0], PathBuf::from("/context/docker.sock"));
        assert!(found.contains(&home.join(".orbstack/run/docker.sock")));
        assert!(found.len() >= 6, "{found:?}");
        let only = BTreeMap::from([("DOCKER_HOST".to_string(), "unix:///chosen.sock".to_string())]);
        assert_eq!(
            sockets(&only, &home, None).unwrap(),
            vec![PathBuf::from("/chosen.sock")]
        );
        let remote =
            BTreeMap::from([("DOCKER_HOST".to_string(), "tcp://10.0.0.2:2376".to_string())]);
        assert!(
            sockets(&remote, &home, None)
                .unwrap_err()
                .to_string()
                .contains("tcp://10.0.0.2:2376")
        );
    }

    #[test]
    fn a_container_writes_in_the_workspace_by_bind_mount_compose_project_or_data_volume() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let roots = [root.clone()];
        let volumes = DataMounts {
            volumes: BTreeSet::from(["db-data".to_string()]),
            binds: vec![PathBuf::from("/srv/pgdata")],
        };
        let bind = |rw: bool, source: &Path| serde_json::json!([{"Type": "bind", "Source": source, "Destination": "/w", "RW": rw}]);
        // A non-canonical source (macOS /var → /private/var) still counts.
        let raw = dir.path().join("repo");
        std::fs::create_dir(&raw).unwrap();
        assert!(writes_in(&container(bind(true, &raw), None), &roots, &volumes).is_some());
        // The workspace's parent folder, mounted writable, reaches it too.
        let parent = dir.path().parent().unwrap();
        assert!(writes_in(&container(bind(true, parent), None), &roots, &volumes).is_some());
        assert!(writes_in(&container(bind(false, &raw), None), &roots, &volumes).is_none());
        // A declared service's data kept in a host folder, mounted by another container.
        let data = writes_in(
            &container(bind(true, Path::new("/srv/pgdata/base")), None),
            &roots,
            &volumes,
        );
        assert!(data.unwrap().contains("data folder"));
        assert!(
            writes_in(
                &container(bind(true, Path::new("/opt")), None),
                &roots,
                &volumes
            )
            .is_none()
        );
        let compose = root.join("repo/compose.yaml");
        let project = container(serde_json::json!([]), Some(compose.to_str().unwrap()));
        assert!(
            writes_in(&project, &roots, &volumes)
                .unwrap()
                .contains("compose project web")
        );
        let volume = |name: &str| {
            container(
                serde_json::json!([{"Type": "volume", "Name": name, "Source": "/var/lib/docker/volumes/x",
                "Destination": "/data", "Driver": "local", "RW": true}]),
                None,
            )
        };
        assert!(writes_in(&volume("db-data"), &roots, &volumes).is_some());
        assert!(writes_in(&volume("cache"), &roots, &volumes).is_none());
    }
}
