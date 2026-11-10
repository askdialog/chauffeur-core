//! Explicit Docker selections only. Preserve containers/volumes and report observations, never
//! complete workspace quiescence, consistent database exports or permission to transfer.
use std::{collections::BTreeSet, path::Path, time::Duration};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Error, Result,
    cloud_workspace_handoff::{Phase, State},
    shellenv::ShellEnv,
    store::Store,
    workspace_development_profile::{
        self as workspace_profile, Disposition, ProfileHead, StoredProfile,
    },
    workspace_docker_command::DockerCommand,
};

const MAX_SERVICES: usize = 32;
const MAX_OUTPUT: usize = 1024 * 1024;
// `index` because Docker formats a map: `.HostConfig.Tmpfs` fails when no tmpfs is declared.
const INSPECT: &str = r#"{"id":{{json .Id}},"image":{{json .Image}},"created":{{json .Created}},"status":{{json .State.Status}},"running":{{json .State.Running}},"paused":{{json .State.Paused}},"restarting":{{json .State.Restarting}},"dead":{{json .State.Dead}},"pid":{{json .State.Pid}},"exit_code":{{json .State.ExitCode}},"auto_remove":{{json .HostConfig.AutoRemove}},"restart_policy":{{json .HostConfig.RestartPolicy.Name}},"has_tmpfs":{{if index .HostConfig "Tmpfs"}}true{{else}}false{{end}},"mounts":{{json .Mounts}}}"#;
fn uncertain() -> Error {
    Error::Unavailable("Selected Docker services cannot be verified; retain their data and the workspace fence before reconciling the service plan".into())
}
fn id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn engine_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-:".contains(&b))
}
fn path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 8192
        && !value.contains('\0')
        && !value.split('/').any(|part| part == "." || part == "..")
}

/// Data inventory only. Required exports/classification are still checked separately; retaining
/// a stopped volume is not a consistent database snapshot and does not make it portable.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "PascalCase")]
pub struct ServiceMount {
    #[serde(rename = "Type")]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub source: String,
    pub destination: String,
    #[serde(default)]
    pub driver: String,
    #[serde(default)]
    pub mode: String,
    #[serde(rename = "RW")]
    pub writable: bool,
    #[serde(default)]
    pub propagation: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SelectedService {
    pub id: String,
    pub image: String,
    pub created: String,
    pub mounts_digest: String,
    pub mounts: Vec<ServiceMount>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inspection {
    id: String,
    image: String,
    created: String,
    status: String,
    running: bool,
    paused: bool,
    restarting: bool,
    dead: bool,
    pid: u32,
    exit_code: i32,
    auto_remove: bool,
    has_tmpfs: bool,
    restart_policy: String,
    mounts: Vec<ServiceMount>,
}
impl Inspection {
    fn selection(&self) -> Result<SelectedService> {
        if !id(&self.id)
            || !self.image.strip_prefix("sha256:").is_some_and(id)
            || self.created.len() > 128
            || chrono::DateTime::parse_from_rfc3339(&self.created).is_err()
            || self.auto_remove
            || self.has_tmpfs
            || self.restart_policy != "no"
            || self.paused
            || self.restarting
            || self.dead
            || self.mounts.len() > 128
            || !(self.inactive() || self.status == "running" && self.running && self.pid > 0)
        {
            return Err(uncertain());
        }
        let mut mounts = self.mounts.clone();
        mounts.sort_by(|a, b| a.destination.cmp(&b.destination));
        let mut destinations = BTreeSet::new();
        for mount in &mounts {
            if !path(&mount.destination)
                || !destinations.insert(&mount.destination)
                || mount.name.len() > 256
                || mount.driver.len() > 256
                || mount.mode.len() > 256
                || mount.propagation.len() > 256
                || match mount.kind.as_str() {
                    "bind" => !path(&mount.source) || !mount.name.is_empty(),
                    "volume" => {
                        !path(&mount.source) || mount.name.is_empty() || mount.driver != "local"
                    }
                    _ => true,
                }
            {
                return Err(uncertain());
            }
        }
        let digest = URL_SAFE_NO_PAD.encode(Sha256::digest(serde_json::to_vec(&mounts)?));
        Ok(SelectedService {
            id: self.id.clone(),
            image: self.image.clone(),
            created: self.created.clone(),
            mounts_digest: digest,
            mounts,
        })
    }
    fn inactive(&self) -> bool {
        matches!(self.status.as_str(), "created" | "exited")
            && !self.running
            && !self.paused
            && !self.restarting
            && !self.dead
            && self.pid == 0
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoppedService {
    pub selection: SelectedService,
    pub status: String,
    pub exit_code: i32,
}
/// Local scoped observations only; neither graceful shutdown nor a complete source-stop receipt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceStop {
    pub stopping: State,
    pub engine_id: String,
    pub reconstructible_profile_digest: String,
    pub profile_head: ProfileHead,
    pub containers: Vec<StoppedService>,
}

/// A live, explicit selection pins the Docker socket/environment and exact container metadata.
/// Only a trusted coordinator may assign these private services to the authenticated workspace.
pub struct DockerServices {
    stopping: State,
    probe: DockerProbe,
    profile: StoredProfile,
    selected: Vec<SelectedService>,
}
impl DockerServices {
    /// Read-only observations for building a custody declaration; never authorizes a stop.
    pub async fn observe(
        env: &ShellEnv,
        socket: &Path,
        engine: String,
        containers: Vec<String>,
    ) -> Result<DockerObservation> {
        let (probe, selected) = DockerProbe::select(env, socket, engine, containers).await?;
        Ok(DockerObservation {
            socket: probe
                .command
                .socket_path()
                .to_str()
                .ok_or_else(uncertain)?
                .into(),
            engine_id: probe.engine,
            containers: selected,
        })
    }

    /// The exact current daemon-owned profile must classify every selected service resource as
    /// reconstructible. A service its PostgreSQL exports cover is left out, for
    /// `workspace_postgres_quiescence`; any other preserve declaration refuses.
    pub async fn from_profile(
        store: &Store,
        env: &ShellEnv,
        stopping: State,
        head: ProfileHead,
    ) -> Result<Self> {
        stopping.validate()?;
        if stopping.transfer.as_ref().map(|t| t.phase) != Some(Phase::Stopping) {
            return Err(uncertain());
        }
        let profile = workspace_profile::load(store, &stopping.binding.workspace_id)?
            .ok_or_else(uncertain)?;
        if profile.head != head
            || profile.profile.workspace_id != stopping.binding.workspace_id
            || profile.profile.services.is_empty()
        {
            return Err(uncertain());
        }
        let first = profile.profile.services.first().ok_or_else(uncertain)?;
        let exported: BTreeSet<&str> = profile
            .profile
            .postgres_exports
            .iter()
            .map(|export| export.service.as_str())
            .collect();
        let chosen: Vec<&workspace_profile::Service> = profile
            .profile
            .services
            .iter()
            .filter(|service| !exported.contains(service.name.as_str()))
            .collect();
        if chosen.is_empty()
            || profile.profile.services.iter().any(|service| {
                service.docker.socket != first.docker.socket
                    || service.docker.engine_id != first.docker.engine_id
            })
        {
            return Err(uncertain());
        }
        for service in &chosen {
            if service.runtime_memory != Disposition::Reconstructible
                || service.writable_layer != Disposition::Reconstructible
                || service
                    .mounts
                    .values()
                    .any(|value| *value != Disposition::Reconstructible)
                || service.docker.socket != first.docker.socket
                || service.docker.engine_id != first.docker.engine_id
                || service.mounts.keys().collect::<BTreeSet<_>>()
                    != service
                        .docker
                        .container
                        .mounts
                        .iter()
                        .map(|mount| &mount.destination)
                        .collect::<BTreeSet<_>>()
            {
                return Err(uncertain());
            }
        }
        let containers = chosen
            .iter()
            .map(|service| service.docker.container.id.clone())
            .collect();
        let (probe, selected) = DockerProbe::select(
            env,
            Path::new(&first.docker.socket),
            first.docker.engine_id.clone(),
            containers,
        )
        .await?;
        if probe.command.socket_path().to_str() != Some(first.docker.socket.as_str())
            || selected.iter().any(|observed| {
                !chosen
                    .iter()
                    .any(|service| service.docker.container == *observed)
            })
        {
            return Err(uncertain());
        }
        let services = Self {
            stopping,
            probe,
            profile,
            selected,
        };
        services.require_current_profile(store)?;
        Ok(services)
    }
    pub(crate) fn require_current_profile(&self, store: &Store) -> Result<()> {
        let current = workspace_profile::load(store, &self.stopping.binding.workspace_id)?
            .ok_or_else(uncertain)?;
        if current != self.profile {
            return Err(uncertain());
        }
        Ok(())
    }
    pub fn selected(&self) -> &[SelectedService] {
        &self.selected
    }
    pub(crate) fn matches(&self, stopping: &State) -> bool {
        self.stopping == *stopping
    }
    pub(crate) fn evidence(&self) -> Result<String> {
        Ok(serde_json::to_string(
            &serde_json::json!({"version":1,"stopping":self.stopping,
            "socket":self.probe.command.socket_path(),"engine_id":self.probe.engine,"profile_head":self.profile.head,"containers":self.selected}),
        )?)
    }
    fn verify(&self, inspections: &[Inspection], stopped: bool) -> Result<()> {
        for selected in &self.selected {
            let current = inspections
                .iter()
                .find(|v| v.id == selected.id)
                .ok_or_else(uncertain)?;
            if current.selection()? != *selected || stopped && !current.inactive() {
                return Err(uncertain());
            }
        }
        Ok(())
    }
    pub(crate) async fn stop(self) -> Result<ServiceStop> {
        tokio::time::timeout(Duration::from_secs(300), self.stop_inner())
            .await
            .map_err(|_| uncertain())?
    }
    async fn stop_inner(&self) -> Result<ServiceStop> {
        let ids: Vec<_> = self.selected.iter().map(|v| v.id.clone()).collect();
        self.probe.check_engine().await?;
        let before = self.probe.inspect(&ids).await?;
        self.verify(&before, false)?;
        self.probe.check_engine().await?;
        for container in before.iter().filter(|v| !v.inactive()) {
            self.probe.check_engine().await?;
            self.probe
                .command
                .run(
                    &["container", "stop", "--timeout", "5", &container.id],
                    Duration::from_secs(15),
                    4096,
                )
                .await?;
            self.probe.check_engine().await?;
        }
        // All possible registered launchers have already drained. Inspect every selected service
        // again after the last stop; an early per-container observation would become stale.
        let after = self.probe.inspect(&ids).await?;
        self.verify(&after, true)?;
        self.probe.check_engine().await?;
        Ok(ServiceStop {
            stopping: self.stopping.clone(),
            engine_id: self.probe.engine.clone(),
            reconstructible_profile_digest: self.profile.head.digest.clone(),
            profile_head: self.profile.head.clone(),
            containers: after
                .into_iter()
                .map(|v| {
                    Ok(StoppedService {
                        selection: v.selection()?,
                        status: v.status,
                        exit_code: v.exit_code,
                    })
                })
                .collect::<Result<_>>()?,
        })
    }
}

/// Stops one container whose data its caller has already carried (a PostgreSQL service after its
/// exports), under the same pins as a selected stop. The longer timeout lets the database shut
/// down cleanly; whether it did, and took no write, is the caller's proof, not this observation.
pub(crate) async fn stop_exported(
    env: &ShellEnv,
    socket: &Path,
    engine: String,
    container: &SelectedService,
) -> Result<StoppedService> {
    let (probe, selected) =
        DockerProbe::select(env, socket, engine, vec![container.id.clone()]).await?;
    if selected != [container.clone()] {
        return Err(uncertain());
    }
    tokio::time::timeout(Duration::from_secs(120), async {
        probe.check_engine().await?;
        probe
            .command
            .run(
                &["container", "stop", "--timeout", "30", &container.id],
                Duration::from_secs(60),
                4096,
            )
            .await?;
        let after = probe.inspect(std::slice::from_ref(&container.id)).await?;
        let current = after.first().ok_or_else(uncertain)?;
        if current.selection()? != *container || !current.inactive() {
            return Err(uncertain());
        }
        probe.check_engine().await?;
        Ok(StoppedService {
            selection: current.selection()?,
            status: current.status.clone(),
            exit_code: current.exit_code,
        })
    })
    .await
    .map_err(|_| uncertain())?
}

/// Whether the pinned container is inactive now, its pins unchanged. Read-only.
pub(crate) async fn is_stopped(
    env: &ShellEnv,
    socket: &Path,
    engine: String,
    container: &SelectedService,
) -> Result<bool> {
    let (probe, selected) =
        DockerProbe::select(env, socket, engine, vec![container.id.clone()]).await?;
    if selected != [container.clone()] {
        return Err(uncertain());
    }
    let current = probe.inspect(std::slice::from_ref(&container.id)).await?;
    Ok(current.first().ok_or_else(uncertain)?.inactive())
}

/// An explicit runtime inventory without a data classification or permission to mutate Docker.
pub struct DockerObservation {
    pub socket: String,
    pub engine_id: String,
    pub containers: Vec<SelectedService>,
}
struct DockerProbe {
    command: DockerCommand,
    engine: String,
}
impl DockerProbe {
    async fn select(
        env: &ShellEnv,
        socket: &Path,
        engine: String,
        containers: Vec<String>,
    ) -> Result<(Self, Vec<SelectedService>)> {
        let ids: BTreeSet<_> = containers.iter().collect();
        if !engine_id(&engine)
            || containers.is_empty()
            || containers.len() > MAX_SERVICES
            || ids.len() != containers.len()
            || containers.iter().any(|s| !id(s))
        {
            return Err(uncertain());
        }
        let probe = Self {
            command: DockerCommand::new(env, socket)?,
            engine,
        };
        let selected = tokio::time::timeout(Duration::from_secs(30), async {
            probe.check_engine().await?;
            let selected = probe
                .inspect(&containers)
                .await?
                .iter()
                .map(Inspection::selection)
                .collect::<Result<_>>()?;
            probe.check_engine().await?;
            Ok::<_, Error>(selected)
        })
        .await
        .map_err(|_| uncertain())??;
        Ok((probe, selected))
    }
    async fn check_engine(&self) -> Result<()> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Engine {
            id: String,
            os: String,
        }
        let output = self
            .command
            .run(
                &[
                    "system",
                    "info",
                    "--format",
                    r#"{"id":{{json .ID}},"os":{{json .OSType}}}"#,
                ],
                Duration::from_secs(10),
                4096,
            )
            .await?;
        let engine: Engine = serde_json::from_slice(&output).map_err(|_| uncertain())?;
        if engine.id != self.engine || engine.os != "linux" {
            return Err(uncertain());
        }
        Ok(())
    }
    async fn inspect(&self, ids: &[String]) -> Result<Vec<Inspection>> {
        let mut args = vec!["container", "inspect", "--format", INSPECT];
        args.extend(ids.iter().map(String::as_str));
        let output = self
            .command
            .run(&args, Duration::from_secs(10), MAX_OUTPUT)
            .await?;
        let values: Vec<Inspection> = serde_json::Deserializer::from_slice(&output)
            .into_iter()
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| uncertain())?;
        let wanted: BTreeSet<_> = ids.iter().map(String::as_str).collect();
        let actual: BTreeSet<_> = values.iter().map(|v| v.id.as_str()).collect();
        if values.len() != ids.len() || actual != wanted {
            return Err(uncertain());
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inspection() -> Inspection {
        serde_json::from_value(serde_json::json!({"id":"a".repeat(64),"image":format!("sha256:{}","b".repeat(64)),
            "created":"2026-10-23T00:00:00Z","status":"running","running":true,"paused":false,"dead":false,
            "restarting":false,"pid":12,"exit_code":0,"auto_remove":false,"has_tmpfs":false,"restart_policy":"no",
            "mounts":[{"Type":"volume","Name":"kept-data","Source":"/var/lib/docker/volumes/kept-data/_data",
                "Destination":"/data","Driver":"local","RW":true}]})).unwrap()
    }
    #[test]
    fn automatic_deletion_restart_and_unsupported_container_states_refuse_selection() {
        assert!(inspection().selection().is_ok());
        for n in 0..9 {
            let mut value = inspection();
            match n {
                0 => value.auto_remove = true,
                1 => value.restart_policy = "always".into(),
                2 => value.paused = true,
                3 => value.restarting = true,
                4 => value.dead = true,
                5 => value.status = "removing".into(),
                6 => value.pid = 0,
                7 => value.has_tmpfs = true,
                _ => value.id = "--other-container".into(),
            }
            assert!(value.selection().is_err());
        }
    }
    #[test]
    fn mount_layout_pins_real_data_and_rejects_unsupported_or_ambiguous_mounts() {
        let original = inspection().selection().unwrap();
        let mut changed = inspection();
        changed.mounts[0].writable = false;
        assert_ne!(
            original.mounts_digest,
            changed.selection().unwrap().mounts_digest
        );
        for n in 0..5 {
            let mut value = inspection();
            match n {
                0 => value.mounts[0].kind = "cluster".into(),
                1 => value.mounts[0].driver = "nfs".into(),
                2 => value.mounts[0].source = "/data/../elsewhere".into(),
                3 => value.mounts[0].name.clear(),
                _ => value.mounts.push(value.mounts[0].clone()),
            }
            assert!(value.selection().is_err());
        }
    }

    struct Fixture {
        root: tempfile::TempDir,
        _socket: std::os::unix::net::UnixListener,
        env: ShellEnv,
        state: State,
        ids: Vec<String>,
        store: Store,
    }
    impl Fixture {
        fn new() -> Self {
            use crate::cloud_workspace_handoff::{Binding, Transfer};
            use std::os::unix::fs::PermissionsExt;
            let root = tempfile::tempdir().unwrap();
            let socket =
                std::os::unix::net::UnixListener::bind(root.path().join("docker.sock")).unwrap();
            std::fs::create_dir(root.path().join("bin")).unwrap();
            let script = format!(
                "#!/usr/bin/env python3\nROOT = {}\n",
                serde_json::to_string(root.path().to_str().unwrap()).unwrap()
            ) + r#"
import sys,json,pathlib
root=pathlib.Path(ROOT);args=sys.argv[3:]
assert sys.argv[1]=='--host' and sys.argv[2].startswith('unix:///')
with (root/'calls').open('a') as log:log.write(json.dumps(args)+'\n')
if args[:2]==['system','info']:
    print((root/'engine').read_text());sys.exit(0)
data=json.loads((root/'containers').read_text())
if args[:2]==['container','inspect']:
    if (root/'malformed').exists(): print('{bad');sys.exit(0)
    if (root/'missing').exists():sys.exit(1)
    for id in args[4:]:print(json.dumps(data[args[4] if (root/'duplicate').exists() else id]))
    sys.exit(0)
if args[:4]==['container','stop','--timeout','5']:
    id=args[4]
    if (root/'fail-stop').exists():print('private-provider-value');sys.exit(1)
    if not (root/'stay-running').exists():data[id].update(status='exited',running=False,pid=0)
    if (root/'change-image').exists():data[id]['image']='sha256:'+'c'*64
    if (root/'restart-first').exists() and id=='b'*64:data['a'*64].update(status='running',running=True,pid=42)
    if (root/'switch-engine').exists():(root/'engine').write_text(json.dumps({'id':'other-engine','os':'linux'}))
    (root/'containers').write_text(json.dumps(data));print(id);sys.exit(0)
sys.exit(1)
"#;
            let binary = root.path().join("bin/docker");
            std::fs::write(&binary, script).unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
            let env = ShellEnv::new();
            env.set_vars(std::collections::BTreeMap::from([(
                "PATH".into(),
                format!("{}:/usr/bin:/bin", root.path().join("bin").display()),
            )]));
            let peer = |n: char| format!("12D3KooW{}", n.to_string().repeat(44));
            let state = State {
                version: 1,
                binding: Binding {
                    workspace_id: uuid::Uuid::now_v7().to_string(),
                    owner_peer_id: peer('1'),
                    controller_peer_id: peer('2'),
                },
                revision: 3,
                epoch: 1,
                execution_peer_id: peer('1'),
                transfer: Some(Transfer {
                    operation_id: uuid::Uuid::now_v7().to_string(),
                    source_peer_id: peer('1'),
                    target_peer_id: peer('3'),
                    source_epoch: 1,
                    phase: Phase::Stopping,
                    source_stop_digest: None,
                    snapshot_digest: None,
                    destination_receipt_digest: None,
                }),
            };
            let ids = vec!["a".repeat(64), "b".repeat(64)];
            let mut data = serde_json::Map::new();
            for id in &ids {
                let mut raw = serde_json::json!({"id":id,"image":format!("sha256:{}","b".repeat(64)),"created":"2026-10-23T00:00:00Z",
                    "status":"running","running":true,"paused":false,"restarting":false,"dead":false,"pid":12,"exit_code":0,"auto_remove":false,
                    "has_tmpfs":false,"restart_policy":"no","mounts":[]});
                raw["mounts"] = serde_json::json!([{"Type":"volume","Name":"reconstructible-cache","Source":"/var/lib/docker/volumes/cache/_data","Destination":"/cache","Driver":"local","RW":true}]);
                data.insert(id.clone(), raw);
            }
            std::fs::write(
                root.path().join("containers"),
                serde_json::to_vec(&data).unwrap(),
            )
            .unwrap();
            std::fs::write(
                root.path().join("engine"),
                r#"{"id":"test-engine","os":"linux"}"#,
            )
            .unwrap();
            std::fs::write(
                root.path().join("retained-volume"),
                "last recoverable cache fixture",
            )
            .unwrap();
            let store = Store::open_in_memory().unwrap();
            store.with(|c| { c.execute("INSERT INTO workspaces(id,slug,path,branch,state,created_at) VALUES(?1,?1,?2,'fixture','ready','2026-10-23T00:00:00Z')", rusqlite::params![state.binding.workspace_id, root.path().to_str().unwrap()])?; Ok(()) }).unwrap();
            Self {
                store,
                root,
                _socket: socket,
                env,
                state,
                ids,
            }
        }
        async fn profile(&self) -> Result<workspace_profile::Profile> {
            let observed = DockerServices::observe(
                &self.env,
                &self.root.path().join("docker.sock"),
                "test-engine".into(),
                self.ids.clone(),
            )
            .await?;
            Ok(workspace_profile::Profile {
                version: 1,
                workspace_id: self.state.binding.workspace_id.clone(),
                linux_setup: vec![],
                linux_startup: vec![],
                preview_ports: vec![],
                postgres_exports: vec![],
                services: observed
                    .containers
                    .into_iter()
                    .enumerate()
                    .map(|(index, container)| workspace_profile::Service {
                        name: format!("service-{index}"),
                        required: true,
                        runtime_memory: Disposition::Reconstructible,
                        writable_layer: Disposition::Reconstructible,
                        mounts: container
                            .mounts
                            .iter()
                            .map(|mount| (mount.destination.clone(), Disposition::Reconstructible))
                            .collect(),
                        docker: workspace_profile::DockerBinding {
                            socket: observed.socket.clone(),
                            engine_id: observed.engine_id.clone(),
                            container,
                        },
                        compose: None,
                        image: None,
                    })
                    .collect(),
            })
        }
        async fn select(&self) -> Result<DockerServices> {
            let profile = self.profile().await?;
            let saved = workspace_profile::save_fixture(&self.store, profile, None)?;
            DockerServices::from_profile(&self.store, &self.env, self.state.clone(), saved.head)
                .await
        }
        fn edit(&self, change: impl FnOnce(&mut serde_json::Value)) {
            let path = self.root.path().join("containers");
            let mut values = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            change(&mut values);
            std::fs::write(path, serde_json::to_vec(&values).unwrap()).unwrap();
        }
        fn stops(&self) -> usize {
            std::fs::read_to_string(self.root.path().join("calls"))
                .unwrap_or_default()
                .lines()
                .filter(|line| line.contains("\"stop\""))
                .count()
        }
    }
    #[tokio::test]
    async fn exact_selected_services_stop_without_removing_their_containers_or_volume_data() {
        let f = Fixture::new();
        let plan = f.select().await.unwrap();
        let evidence = plan.evidence().unwrap();
        assert!(evidence.contains("profile_head"));
        let result = plan.stop().await.unwrap();
        assert_eq!(result.containers.len(), 2);
        assert_eq!(f.stops(), 2);
        assert!(result.containers.iter().all(|v| v.status == "exited"));
        assert_eq!(
            std::fs::read_to_string(f.root.path().join("retained-volume")).unwrap(),
            "last recoverable cache fixture"
        );
        let calls = std::fs::read_to_string(f.root.path().join("calls")).unwrap();
        for line in calls.lines() {
            let args: Vec<String> = serde_json::from_str(line).unwrap();
            for forbidden in ["down", "--volumes", "rm", "prune", "update", "compose"] {
                assert!(!args.iter().any(|arg| arg == forbidden));
            }
        }
    }
    #[tokio::test]
    async fn every_pin_is_preflighted_before_the_first_service_can_be_stopped() {
        for field in ["image", "created", "mounts"] {
            let f = Fixture::new();
            let plan = f.select().await.unwrap();
            f.edit(|v| {
                let target = &mut v[&f.ids[1]];
                match field {
                    "image" => target[field] = format!("sha256:{}", "d".repeat(64)).into(),
                    "created" => target[field] = "2026-10-23T00:01:00Z".into(),
                    _ => target[field][0]["RW"] = false.into(),
                }
            });
            assert!(plan.stop().await.is_err());
            assert_eq!(f.stops(), 0);
        }
    }
    #[tokio::test]
    async fn destructive_or_ephemeral_services_and_bad_evidence_never_reach_a_stop_command() {
        for kind in [
            "auto_remove",
            "has_tmpfs",
            "tmpfs-mount",
            "restart",
            "malformed",
            "missing",
            "duplicate",
        ] {
            let f = Fixture::new();
            match kind {
                "malformed" | "missing" | "duplicate"=>{std::fs::write(f.root.path().join(kind),"").unwrap();},
                _=>f.edit(|v| {
                    let target=&mut v[&f.ids[0]];
                    match kind {
                        "auto_remove" | "has_tmpfs"=>target[kind]=true.into(),
                        "restart"=>target["restart_policy"]="unless-stopped".into(),
                        _=>target["mounts"]=serde_json::json!([{"Type":"tmpfs","Destination":"/runtime","RW":true}]),
                    }
                })
            }
            assert!(f.select().await.is_err());
            assert_eq!(f.stops(), 0);
        }
    }
    #[tokio::test]
    async fn final_all_service_inspection_catches_restarts_and_identity_changes() {
        for hook in [
            "stay-running",
            "restart-first",
            "change-image",
            "switch-engine",
            "fail-stop",
        ] {
            let f = Fixture::new();
            let plan = f.select().await.unwrap();
            std::fs::write(f.root.path().join(hook), "").unwrap();
            let result = plan.stop().await.unwrap_err();
            assert!(!result.to_string().contains("private-provider-value"));
            assert!(f.stops() > 0);
            assert!(f.root.path().join("retained-volume").is_file());
        }
    }
    #[tokio::test]
    async fn missing_classification_duplicate_ids_and_unsupported_engines_are_never_empty_success()
    {
        let f = Fixture::new();
        for ids in [
            Vec::new(),
            vec![f.ids[0].clone(), f.ids[0].clone()],
            vec!["--all".into()],
        ] {
            assert!(
                DockerServices::observe(
                    &f.env,
                    &f.root.path().join("docker.sock"),
                    "test-engine".into(),
                    ids
                )
                .await
                .is_err()
            );
        }
        assert!(
            DockerServices::from_profile(
                &f.store,
                &f.env,
                f.state.clone(),
                ProfileHead {
                    revision: 1,
                    digest: URL_SAFE_NO_PAD.encode([1; 32])
                }
            )
            .await
            .is_err()
        );
        std::fs::write(
            f.root.path().join("engine"),
            r#"{"id":"test-engine","os":"windows"}"#,
        )
        .unwrap();
        assert!(f.select().await.is_err());
        assert_eq!(f.stops(), 0);
    }

    #[tokio::test]
    async fn meaningful_memory_layers_and_mounts_refuse_even_when_the_service_is_optional() {
        for resource in ["memory", "layer", "mount"] {
            let f = Fixture::new();
            let mut profile = f.profile().await.unwrap();
            let service = &mut profile.services[0];
            service.required = false;
            let preserve = Disposition::Preserve {
                data_kind: workspace_profile::MeaningfulKind::Database,
                reason: "retain the synthetic database until a consistent export exists".into(),
            };
            match resource {
                "memory" => service.runtime_memory = preserve,
                "layer" => service.writable_layer = preserve,
                _ => {
                    service.mounts.insert("/cache".into(), preserve);
                }
            }
            let saved = workspace_profile::save_fixture(&f.store, profile, None).unwrap();
            let calls = std::fs::read(f.root.path().join("calls")).unwrap();
            assert!(
                DockerServices::from_profile(&f.store, &f.env, f.state.clone(), saved.head)
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read(f.root.path().join("calls")).unwrap(), calls);
            assert_eq!(f.stops(), 0);
        }
    }

    #[tokio::test]
    async fn a_saved_profile_cannot_substitute_the_exact_observed_container_pins() {
        for field in ["image", "created", "mount"] {
            let f = Fixture::new();
            let mut profile = f.profile().await.unwrap();
            let service = &mut profile.services[0];
            match field {
                "image" => service.docker.container.image = format!("sha256:{}", "d".repeat(64)),
                "created" => service.docker.container.created = "2026-10-23T01:00:00Z".into(),
                _ => {
                    service.docker.container.mounts[0].writable = false;
                    service.docker.container.mounts_digest =
                        URL_SAFE_NO_PAD.encode(Sha256::digest(
                            serde_json::to_vec(&service.docker.container.mounts).unwrap(),
                        ));
                }
            }
            let saved = workspace_profile::save_fixture(&f.store, profile, None).unwrap();
            assert!(
                DockerServices::from_profile(&f.store, &f.env, f.state.clone(), saved.head)
                    .await
                    .is_err()
            );
            assert_eq!(f.stops(), 0);
        }
    }

    #[tokio::test]
    async fn a_selected_plan_cannot_adopt_a_new_profile_revision_or_a_different_workspace() {
        let f = Fixture::new();
        let selected = f.select().await.unwrap();
        let saved = workspace_profile::load(&f.store, &f.state.binding.workspace_id)
            .unwrap()
            .unwrap();
        let mut next = saved.profile.clone();
        next.linux_setup.push("printf synthetic-setup".into());
        let next = workspace_profile::save_fixture(&f.store, next, Some(&saved.head)).unwrap();
        assert!(selected.require_current_profile(&f.store).is_err());
        let calls = std::fs::read(f.root.path().join("calls")).unwrap();
        assert!(
            DockerServices::from_profile(&f.store, &f.env, f.state.clone(), saved.head)
                .await
                .is_err()
        );
        let mut wrong_workspace = f.state.clone();
        wrong_workspace.binding.workspace_id = uuid::Uuid::now_v7().to_string();
        assert!(
            DockerServices::from_profile(&f.store, &f.env, wrong_workspace, next.head)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(f.root.path().join("calls")).unwrap(), calls);
        assert_eq!(f.stops(), 0);
    }
}
