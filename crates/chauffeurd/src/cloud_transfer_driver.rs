//! The transfer driver: this daemon moves one workspace from a source daemon to a destination
//! daemon, a controller daemon keeping the journal, in the background (decided with Justin
//! 2026-10-26: the Mac's daemon drives what its app starts, so closing the app does not stop it).
//! Each end is this daemon, called in process, a machine of `[transfer]` or a daemon socket (a
//! socket forwarded with `ssh -L` reaches another machine). The daemons check every step; the
//! driver only carries journal states, digests, ciphertext blocks and the archive key. Its
//! progress is kept per workspace and sent as `WorkspaceEvent::Transfer`.
//!
//! A transfer stops the source only once the workspace's agents ended their turn (or were
//! interrupted, when the user asks), and a few transfers run past that point at once: a batch is
//! several transfers started together, each with its own outcome.
pub(crate) mod archive_bridge;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail};
use chauffeur_protocol::MIN_PROTOCOL_VERSION;
use chauffeur_protocol::client::{
    ChatListParams, ChatParams, ChatsResult, ClientCall, EmptyParams, HelloParams, HelloResult,
    ProcessesResult, WorkspaceParams,
};
use chauffeur_protocol::cloud_archive::{
    CloudArchiveBinding, CloudArchiveBlock, CloudArchiveHandle, CloudArchiveReadParams,
    CloudArchiveReceiveParams, CloudArchiveReceiveResult, CloudArchiveWriteParams,
};
use chauffeur_protocol::cloud_transfer::{
    CloudTransferActivateParams, CloudTransferActivated, CloudTransferCaptureParams,
    CloudTransferCaptured, CloudTransferCheckParams, CloudTransferDigest, CloudTransferDiscarded,
    CloudTransferJournal, CloudTransferJournalParams, CloudTransferMachines, CloudTransferPeer,
    CloudTransferReadiness, CloudTransferRestoreParams, CloudTransferRestored, CloudTransferSealed,
    CloudTransferSetup, CloudTransferSetupParams, CloudTransferStartParams, CloudTransferState,
    CloudTransferStep, TransferEndpoint, TransferInfo, TransferLogin, TransferPhase,
    TransferPreparation, TransferProgress, TransferStage,
};
use chauffeur_protocol::events::{ProcessKind, RegistryEvent, WorkspaceEvent};
use chauffeur_protocol::model::{AgentKind, ChatStatus, WorkspaceInfo};
use chauffeur_protocol::peer::{PEER_PROTOCOL_VERSION, PeerCall, PeerHelloParams};
use chauffeur_protocol::rpc::{Incoming, Request};
use chauffeur_protocol::worker::{
    WorkerClaudeConfigChunk, WorkerClaudeConfigInstallParams, WorkerClaudeConfigInstalled,
    WorkerClaudeConfigStatus, WorkerCredentialsInstalled,
};
use futures_util::{SinkExt, StreamExt};
use rusqlite::{OptionalExtension, params};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::net::UnixStream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::{Daemon, Error, Result};

/// The line a transfer logs once the journal committed it.
const COMMITTED: &str = "committed; activating the destination and retiring the source";

/// The archive bridge's largest block.
const BLOCK: u64 = 256 * 1024;

/// How many transfers this daemon drives past the wait for their agents at once: stopping,
/// sealing, sending and restoring weigh on both machines' disks and network.
const AT_ONCE: usize = 2;

/// How often a transfer waiting for the end of its agents' turns looks at them again.
const TURNS_POLL: Duration = Duration::from_secs(1);
/// Capture and restoration can run for minutes. A deadline ends only this driver's wait,
/// never grants ownership or cancels the peer's operation.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const SETUP_WAIT: Duration = Duration::from_secs(15 * 60);

/// The workspaces whose transfer this daemon is driving.
pub struct Transfers {
    running: Mutex<BTreeSet<String>>,
    /// When a source was last asked about the copy it holds, per workspace, for an app's read.
    asked: Mutex<HashMap<String, std::time::Instant>>,
    /// What the user asks of each transfer that has not begun stopping its source, by workspace.
    asks: Mutex<HashMap<String, watch::Sender<Ask>>>,
    /// The places of the transfers past the wait for their agents.
    at_once: Arc<Semaphore>,
}

impl Default for Transfers {
    fn default() -> Self {
        Transfers {
            running: Mutex::default(),
            asked: Mutex::default(),
            asks: Mutex::default(),
            at_once: Arc::new(Semaphore::new(AT_ONCE)),
        }
    }
}

/// What the user asks of a transfer before it stops the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    /// Wait for the end of the agents' turns.
    Wait,
    /// Interrupt the agents in a turn, then go on.
    Interrupt,
    /// Leave the workspace as it was.
    Cancel,
}

impl Transfers {
    pub(crate) fn active_count(&self) -> u32 {
        self.running.lock().unwrap().len() as u32
    }

    /// Whether a cancellation was asked, as the transfer of `workspace` leaves the window where
    /// one leaves the workspace as it was: none is taken afterwards.
    fn leave_waiting(&self, workspace: &str) -> bool {
        let asked = self.asks.lock().unwrap().remove(workspace);
        asked.is_some_and(|ask| *ask.borrow() == Ask::Cancel)
    }

    /// Asks the transfer of `workspace` that has not begun stopping its source; false when none.
    fn ask(&self, workspace: &str, ask: Ask) -> bool {
        let asks = self.asks.lock().unwrap();
        let Some(sender) = asks.get(workspace) else {
            return false;
        };
        sender.send_if_modified(|current| {
            let changed = *current != Ask::Cancel && *current != ask;
            if changed {
                *current = ask;
            }
            changed
        });
        true
    }
}

/// A daemon the driver calls: this one, in process, or another over its socket. This one keeps
/// its own archive bridge, as a connection does, closed with the transfer.
enum Endpoint {
    Here(Arc<Daemon>, Arc<archive_bridge::ArchiveBridge>),
    Socket(Box<Peer>),
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        if let Endpoint::Here(_, archives) = self {
            archives.close();
        }
    }
}

impl Endpoint {
    async fn open(daemon: &Arc<Daemon>, end: &TransferEndpoint) -> anyhow::Result<Endpoint> {
        Ok(match end {
            TransferEndpoint::Cloud { .. } => {
                bail!("Cloud must be prepared as a destination first")
            }
            TransferEndpoint::Here => Endpoint::Here(
                daemon.clone(),
                archive_bridge::ArchiveBridge::new(&daemon.paths.home),
            ),
            TransferEndpoint::Machine { name } => Endpoint::Socket(Box::new(
                Peer::connect(&crate::workers::machine_socket(daemon, name)?).await?,
            )),
            TransferEndpoint::Socket { path } => Endpoint::Socket(Box::new(
                Peer::connect(&crate::settings::expand_home(path)).await?,
            )),
        })
    }

    /// The Chauffeur version of another daemon; none for this one.
    fn version(&self) -> Option<&str> {
        match self {
            Endpoint::Here(..) => None,
            Endpoint::Socket(peer) => Some(&peer.version),
        }
    }

    /// `opened`, unless it is a GCP worker of this daemon on another Chauffeur version: that one is
    /// updated to this version first, then opened again once its new daemon answers.
    async fn up_to_date(
        daemon: &Arc<Daemon>,
        end: &TransferEndpoint,
        opened: Endpoint,
    ) -> anyhow::Result<Endpoint> {
        let ours = env!("CARGO_PKG_VERSION");
        let TransferEndpoint::Machine { name } = end else {
            return Ok(opened);
        };
        if opened.version().is_none_or(|v| v == ours) || !crate::workers::is_worker(daemon, name) {
            return Ok(opened);
        }
        drop(opened);
        crate::workers::update(daemon, name).await?;
        for _ in 0..120 {
            if let Ok(endpoint) = Endpoint::open(daemon, end).await
                && endpoint.version() == Some(ours)
            {
                return Ok(endpoint);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        bail!(
            "{name} was updated to Chauffeur {ours} but its daemon does not answer; see Settings → Transfers, then retry"
        )
    }

    async fn call_value(&mut self, call: ClientCall) -> anyhow::Result<Value> {
        match self {
            Endpoint::Here(daemon, _) => handle_here(daemon.clone(), call)
                .await
                .map_err(|error| anyhow::anyhow!(error.to_rpc().message)),
            Endpoint::Socket(peer) => peer.call_value(call).await,
        }
    }

    async fn call<T: DeserializeOwned>(&mut self, call: ClientCall) -> anyhow::Result<T> {
        let value = self.call_value(call).await?;
        serde_json::from_value(value).context("unexpected answer from a daemon")
    }

    async fn peer_value(&mut self, call: PeerCall) -> anyhow::Result<Value> {
        match self {
            Endpoint::Here(daemon, archives) => {
                let result = if archive_bridge::is_archive_call(&call) {
                    archives.handle(call).await
                } else {
                    crate::peer::handle(daemon, call).await
                };
                result.map_err(|error| crate::Error::Remote(error.to_rpc()).into())
            }
            Endpoint::Socket(peer) => peer.peer_value(call).await,
        }
    }

    async fn peer<T: DeserializeOwned>(&mut self, call: PeerCall) -> anyhow::Result<T> {
        let value = self.peer_value(call).await?;
        serde_json::from_value(value).context("unexpected answer from a daemon")
    }

    /// Sends a peer call whose answer `peer_receive` reads later; this daemon answers at once.
    async fn peer_send(&mut self, call: PeerCall) -> anyhow::Result<Pending> {
        Ok(match self {
            Endpoint::Here(..) => Pending::Answered(self.peer_value(call).await),
            Endpoint::Socket(peer) => Pending::Sent(peer.peer_send(call).await?),
        })
    }

    async fn peer_receive(&mut self, pending: Pending) -> anyhow::Result<Value> {
        match (self, pending) {
            (_, Pending::Answered(result)) => result,
            (Endpoint::Socket(peer), Pending::Sent(id)) => peer.peer_receive(id).await,
            (Endpoint::Here(..), Pending::Sent(_)) => bail!("this daemon sends no peer call"),
        }
    }
}

/// A peer call sent ahead: its answer, or the id it will come back with.
enum Pending {
    Answered(anyhow::Result<Value>),
    Sent(u64),
}

/// This daemon's own dispatch, boxed: `rpc::handle` reaches the driver again through
/// `cloud.transfer.start`, a cycle the compiler cannot type unboxed.
fn handle_here(
    daemon: Arc<Daemon>,
    call: ClientCall,
) -> std::pin::Pin<Box<dyn Future<Output = Result<Value>> + Send>> {
    Box::pin(async move { crate::rpc::handle(&daemon, call).await })
}

/// Another daemon over its Unix socket: its client methods on `/ws`, its peer methods on
/// `/peer`, opened on the first; their events are ignored.
pub(crate) struct Peer {
    path: std::path::PathBuf,
    client: Connection,
    peer: Option<Connection>,
    /// The Chauffeur version it runs.
    pub(crate) version: String,
}

impl Peer {
    pub(crate) async fn connect(path: &std::path::Path) -> anyhow::Result<Peer> {
        let mut client = Connection::open(path, "/ws").await?;
        let hello: HelloResult = serde_json::from_value(
            client
                .call_value(ClientCall::Hello(HelloParams {
                    // What a daemon of any version answers: the transfer's own check of versions
                    // follows, and says which side to update.
                    protocol_version: MIN_PROTOCOL_VERSION,
                    client: format!("chauffeurd {} transfer", env!("CARGO_PKG_VERSION")),
                }))
                .await?,
        )?;
        Ok(Peer {
            path: path.to_owned(),
            client,
            peer: None,
            version: hello.daemon_version,
        })
    }

    pub(crate) async fn call_value(&mut self, call: ClientCall) -> anyhow::Result<Value> {
        self.client.call_value(call).await
    }

    pub(crate) async fn peer_value(&mut self, call: PeerCall) -> anyhow::Result<Value> {
        let id = self.peer_send(call).await?;
        self.peer_receive(id).await
    }

    /// Sends a peer call without waiting for its answer, which `peer_receive` reads.
    async fn peer_send(&mut self, call: PeerCall) -> anyhow::Result<u64> {
        if self.peer.is_none() {
            let mut peer = Connection::open(&self.path, "/peer").await?;
            peer.call_value(PeerCall::Hello(PeerHelloParams {
                protocol_version: PEER_PROTOCOL_VERSION,
                client: format!("chauffeurd {} transfer", env!("CARGO_PKG_VERSION")),
            }))
            .await?;
            self.peer = Some(peer);
        }
        self.peer.as_mut().expect("opened above").send(call).await
    }

    async fn peer_receive(&mut self, id: u64) -> anyhow::Result<Value> {
        let peer = self.peer.as_mut().context("no peer call was sent")?;
        peer.receive(id).await
    }
}

/// One WebSocket to a daemon's Unix socket: a call and its answer, or calls sent ahead whose
/// answers are read later.
struct Connection {
    socket: WebSocketStream<UnixStream>,
    next_id: u64,
    /// Answers read while waiting for another call's.
    answers: HashMap<u64, Incoming>,
}

impl Connection {
    async fn open(path: &std::path::Path, route: &str) -> anyhow::Result<Connection> {
        let stream = tokio::time::timeout(Duration::from_secs(10), UnixStream::connect(path))
            .await
            .context("the destination connection timed out; check its SSH tunnel")?
            .with_context(|| format!("no Chauffeur daemon answers on {}", path.display()))?;
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::client_async(format!("ws://localhost{route}"), stream),
        )
        .await
        .context("the daemon did not complete its handshake; check the destination connection")?
        .with_context(|| format!("connecting to the daemon on {}", path.display()))?;
        Ok(Connection {
            socket,
            next_id: 1,
            answers: HashMap::new(),
        })
    }

    async fn call_value(&mut self, call: impl serde::Serialize) -> anyhow::Result<Value> {
        let id = self.send(call).await?;
        self.receive(id).await
    }

    async fn send(&mut self, call: impl serde::Serialize) -> anyhow::Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let text = serde_json::to_string(&Request::new(id, call))?;
        tokio::time::timeout(
            Duration::from_secs(30),
            self.socket.send(Message::Text(text.into())),
        )
        .await
        .context(
            "the transfer connection stopped accepting data; reconnect and resume the transfer",
        )??;
        Ok(id)
    }

    async fn receive(&mut self, id: u64) -> anyhow::Result<Value> {
        let incoming = match self.answers.remove(&id) {
            Some(incoming) => incoming,
            None => tokio::time::timeout(RESPONSE_TIMEOUT, self.read(id)).await
                .context("the destination did not confirm its operation; reconnect and resume to reconcile ownership")??,
        };
        if let Some(error) = incoming.error {
            // Kept whole: its code and data say what kind of refusal it is.
            return Err(crate::Error::Remote(error).into());
        }
        Ok(incoming.result.unwrap_or(Value::Null))
    }

    /// Reads until the answer to `id`, keeping the other calls' answers.
    async fn read(&mut self, id: u64) -> anyhow::Result<Incoming> {
        loop {
            let message = self
                .socket
                .next()
                .await
                .ok_or_else(|| anyhow::anyhow!("the other daemon closed the connection"))??;
            let text = match message {
                Message::Text(text) => text.to_string(),
                Message::Binary(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Message::Close(_) => bail!("the other daemon closed the connection"),
                _ => continue,
            };
            let incoming: Incoming =
                serde_json::from_str(&text).context("unreadable message from another daemon")?;
            if !incoming.is_response() {
                continue;
            }
            match incoming.id.as_ref().and_then(Value::as_u64) {
                Some(answered) if answered == id => return Ok(incoming),
                Some(answered) => {
                    self.answers.insert(answered, incoming);
                }
                None => {}
            }
        }
    }
}

/// Starts a transfer in the background and returns it as started. What can be checked at once
/// (an unknown machine, a daemon that does not answer, an unknown workspace, a transfer of it
/// already running here) refuses here.
pub async fn start(daemon: &Arc<Daemon>, params: CloudTransferStartParams) -> Result<TransferInfo> {
    if matches!(params.source, TransferEndpoint::Cloud { .. })
        || (matches!(params.destination, TransferEndpoint::Cloud { .. })
            && params.source != TransferEndpoint::Here)
    {
        return Err(Error::BadRequest(
            "Cloud can only be the destination of an outgoing workspace on this computer".into(),
        ));
    }
    let admission = daemon.workers.lifecycle.clone().lock_owned().await;
    let controller = daemon
        .transfer_controller
        .socket(daemon, params.controller.as_deref())
        .await
        .map_err(|error| {
            Error::Unavailable(format!(
                "Could not prepare the transfer controller: {error:#}"
            ))
        })?;
    let refuse = |error: anyhow::Error| Error::BadRequest(format!("{error:#}"));
    let source = Endpoint::open(daemon, &params.source)
        .await
        .map_err(refuse)?;
    let mut source = Endpoint::up_to_date(daemon, &params.source, source)
        .await
        .map_err(refuse)?;
    let destination = if matches!(params.destination, TransferEndpoint::Cloud { .. }) {
        None
    } else {
        let destination = Endpoint::open(daemon, &params.destination)
            .await
            .map_err(refuse)?;
        Some(
            Endpoint::up_to_date(daemon, &params.destination, destination)
                .await
                .map_err(refuse)?,
        )
    };
    let controller = Peer::connect(&controller).await.map_err(refuse)?;
    // The transfer's own messages change between versions: every daemon runs this one's.
    let ours = env!("CARGO_PKG_VERSION");
    for (name, version) in [
        ("The source", source.version()),
        (
            "The destination",
            destination.as_ref().and_then(Endpoint::version),
        ),
        ("The controller", Some(controller.version.as_str())),
    ] {
        if let Some(version) = version.filter(|v| *v != ours) {
            return Err(Error::BadRequest(format!(
                "{name} runs Chauffeur {version} and this one {ours}; update them to the same version, then transfer"
            )));
        }
    }
    let workspace: WorkspaceInfo = source
        .call(ClientCall::WorkspaceGet(WorkspaceParams {
            workspace: params.workspace.clone(),
        }))
        .await
        .map_err(refuse)?;
    if matches!(params.destination, TransferEndpoint::Cloud { .. })
        && (workspace.state != chauffeur_protocol::model::WorkspaceState::Ready
            || workspace.moved_to.is_some())
    {
        return Err(Error::Conflict(format!(
            "{} is not ready here; bring it back or finish its current operation before sending to Cloud",
            workspace.slug
        )));
    }
    let slot = Slot::claim(daemon, &workspace.id).ok_or_else(|| {
        Error::Conflict(format!("{} is already being transferred", workspace.slug))
    })?;
    let label = |end: &TransferEndpoint| match end {
        TransferEndpoint::Cloud { .. } => "Cloud".into(),
        TransferEndpoint::Here => crate::cloud_transfer::host_name(),
        TransferEndpoint::Machine { name } => name.clone(),
        TransferEndpoint::Socket { path } => path.clone(),
    };
    let mut report = Report {
        daemon: daemon.clone(),
        info: TransferInfo {
            revision: None,
            id: uuid::Uuid::now_v7().to_string(),
            workspace_id: workspace.id.clone(),
            slug: workspace.slug.clone(),
            source: label(&params.source),
            destination: label(&params.destination),
            phase: TransferPhase::Running,
            log: Vec::new(),
            error: None,
            workspace_path: None,
            kept_copy: None,
            returned: false,
            started_at: chrono::Utc::now().to_rfc3339(),
            finished_at: None,
            from: named(&params.source),
            to: named(&params.destination),
            keep_local_copy: params.keep_local_copy,
            journal_owner: None,
            discard_pending: None,
            stage: Some(
                if matches!(params.destination, TransferEndpoint::Cloud { .. }) {
                    TransferStage::PreparingWorker
                } else {
                    TransferStage::Preparing
                },
            ),
            working_agents: 0,
            progress: None,
            preparation: None,
            interrupt: params.interrupt,
            archive: params.archive,
            needs_login: None,
            local_copy_changed: false,
            untouched: false,
        },
        source: params.source.clone(),
        journaled: false,
    };
    crate::worker_lifecycle::admit(daemon, &params.source, &workspace.id, &report.info.id)?;
    crate::worker_lifecycle::admit(daemon, &params.destination, &workspace.id, &report.info.id)?;
    report.save();
    let started = report.info.clone();
    let keep = params.keep_local_copy;
    let ask = if params.interrupt {
        Ask::Interrupt
    } else {
        Ask::Wait
    };
    let (sender, mut asks) = watch::channel(ask);
    daemon
        .transfers
        .asks
        .lock()
        .unwrap()
        .insert(workspace.id.clone(), sender);
    drop(admission);
    tokio::spawn(async move {
        let outcome = async {
            let destination = match destination {
                Some(destination) => destination,
                None => {
                    if *asks.borrow() == Ask::Cancel { return Ok(None); }
                    let TransferEndpoint::Cloud { beyond_limit } = params.destination else { unreachable!("only Cloud is deferred") };
                    // A folder the capture would refuse fails at once, not after a worker started.
                    if let Endpoint::Here(daemon, _) = &source {
                        crate::cloud_workspace_package::check_folder(daemon, &workspace.id)
                            .await
                            .map_err(|error| anyhow::anyhow!("{error}; nothing was stopped"))?;
                    }
                    report.line("preparing an on-demand GCP worker; it is stopped, its disk kept, after its last workspace returns here".into());
                    let daemon = report.daemon.clone();
                    let transfer = report.info.id.clone();
                    let prepared = tokio::select! {
                        result = crate::worker_lifecycle::destination(&daemon, &workspace.id, &transfer, beyond_limit, |name| {
                            report.info.destination = name.to_owned(); report.save();
                        }) => result.map_err(|e| anyhow::anyhow!(e))?,
                        _ = wait_for_cancel(&mut asks) => return Ok(None),
                    };
                    let (name, held) = prepared;
                    report.info.to = Some(TransferEndpoint::Machine { name });
                    report.stage(TransferStage::Preparing);
                    if *asks.borrow() == Ask::Cancel { return Ok(None); }
                    let chosen = report.info.to.clone().expect("chosen worker");
                    let destination = Endpoint::open(&daemon, &chosen).await?;
                    if destination.version() != Some(env!("CARGO_PKG_VERSION")) {
                        report.line(format!("updating {} to Chauffeur {}", report.info.destination, env!("CARGO_PKG_VERSION")));
                    }
                    let destination = Endpoint::up_to_date(&daemon, &chosen, destination).await?;
                    drop(held);
                    destination
                }
            };
            let mut parties = Parties { source, destination, controller };
            run(&mut parties, &workspace, keep, &mut asks, &mut report).await
        }.await;
        report.finish(outcome);
        archive_back(&mut report);
        if let Err(error) = crate::worker_lifecycle::finished(&report.daemon, &report.info) {
            tracing::warn!(%error, workspace=workspace.id, "could not settle a worker reservation");
        }
        report
            .daemon
            .transfers
            .asks
            .lock()
            .unwrap()
            .remove(&workspace.id);
        drop(slot);
        if let Err(error) = crate::worker_lifecycle::cleanup(&report.daemon).await {
            tracing::warn!(%error, "could not clean unused workers");
        }
    });
    Ok(started)
}

/// Archiving a workspace that moved to another machine this daemon sent it to: it comes back first,
/// its agents interrupted since archiving stops them anyway, then is archived here
/// (`archive_back`). Archived there, its snapshot would live on a disk that stops with the machine,
/// and a deleted worker would take it along. The machine stops once its last workspace is back,
/// as after any return. None when it runs here.
pub async fn archive_moved(daemon: &Arc<Daemon>, workspace: &str) -> Result<Option<WorkspaceInfo>> {
    let Some(row) = daemon.store.find_workspace(workspace)? else {
        return Ok(None);
    };
    let Some(moved) = moving(daemon, &row.id) else {
        return Ok(None);
    };
    let Some(source) = moved.and_then(|t| t.to) else {
        return Err(Error::Conflict(format!(
            "{} runs on a machine this one did not send it to: bring it back from there, then archive it",
            row.slug
        )));
    };
    start(
        daemon,
        CloudTransferStartParams {
            workspace: row.id.clone(),
            source,
            destination: TransferEndpoint::Here,
            controller: None,
            keep_local_copy: false,
            interrupt: true,
            archive: true,
        },
    )
    .await
    .map_err(|error| {
        let says = |why: String| {
            format!(
                "Could not bring {} back to archive it: {why}; archive it again once it can come back",
                row.slug
            )
        };
        match error {
            Error::BadRequest(why) => Error::BadRequest(says(why)),
            Error::Conflict(why) => Error::Conflict(says(why)),
            other => other,
        }
    })?;
    daemon.workspaces.get(&row.id).map(Some)
}

/// Once a transfer asked by `archive_moved` brought its workspace back, archives it here, in the
/// background as the app's archive does; said in the transfer's log. A transfer that failed or was
/// cancelled archives nothing: a retry asks it again.
fn archive_back(report: &mut Report) {
    let info = &report.info;
    if !info.archive || info.phase != TransferPhase::Done || !info.returned {
        return;
    }
    let archived = report
        .daemon
        .workspaces
        .archive_in_background(&report.info.workspace_id, true);
    report.line(match archived {
        Ok(_) => "archiving it here, as asked".into(),
        Err(error) => format!("could not archive it here: {error}; archive it again"),
    });
}

async fn wait_for_cancel(asks: &mut watch::Receiver<Ask>) {
    loop {
        if *asks.borrow_and_update() == Ask::Cancel {
            return;
        }
        if asks.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// A transfer of `workspace` waiting for the end of its agents' turns interrupts them now, then
/// goes on; refused when none waits.
pub fn interrupt(daemon: &Daemon, workspace: &str) -> Result<TransferInfo> {
    let latest = status(daemon, workspace)?
        .filter(|t| t.phase == TransferPhase::Running)
        .ok_or_else(|| Error::Conflict(format!("No transfer of {workspace} is running here")))?;
    if !daemon.transfers.ask(&latest.workspace_id, Ask::Interrupt) {
        return Err(Error::Conflict(format!(
            "The transfer of {} already stopped its agents",
            latest.slug
        )));
    }
    Ok(TransferInfo {
        revision: None,
        interrupt: true,
        ..latest
    })
}

/// Cancels a transfer of `workspace` that has not begun stopping its source: it ends as soon as
/// it hears it, the workspace left as it was. None when no such transfer runs, or it went past
/// that point meanwhile (the usual cancellation follows).
async fn cancel_waiting(daemon: &Daemon, workspace: &str) -> Result<Option<TransferInfo>> {
    let Some(latest) = status(daemon, workspace)?.filter(|t| t.phase == TransferPhase::Running)
    else {
        return Ok(None);
    };
    if !daemon.transfers.ask(&latest.workspace_id, Ask::Cancel) {
        return Ok(None);
    }
    // It hears it at its next look at the agents, or before it would stop the source.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while daemon
        .transfers
        .running
        .lock()
        .expect("transfers poisoned")
        .contains(&latest.workspace_id)
    {
        if tokio::time::Instant::now() > deadline {
            return Err(Error::Conflict(format!(
                "The transfer of {} has not heard the cancellation yet (its source does not answer); cancel again in a moment",
                latest.slug
            )));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(status(daemon, &latest.workspace_id)?
        .filter(|t| t.id == latest.id && t.phase == TransferPhase::Cancelled))
}

/// Cancels a transfer of `params.workspace` that did not commit, from wherever it was left: the
/// controller's journal records it cancelled, the source runs the workspace again, and the
/// destination drops what it received or restored, never activated. A destination that does not
/// answer keeps those files until the cancellation is asked again; the source never waits for it.
pub async fn cancel(
    daemon: &Arc<Daemon>,
    mut params: CloudTransferStartParams,
) -> Result<TransferInfo> {
    if let Some(cancelled) = cancel_waiting(daemon, &params.workspace).await? {
        return Ok(cancelled);
    }
    let admission = daemon.workers.lifecycle.clone().lock_owned().await;
    if matches!(params.destination, TransferEndpoint::Cloud { .. }) {
        let row = daemon
            .store
            .find_workspace(&params.workspace)?
            .ok_or_else(|| Error::NotFound("No such workspace".into()))?;
        let name = crate::worker_lifecycle::reserved(daemon, &row.id)?.ok_or_else(|| {
            Error::Conflict(
                "No worker preparation is reserved for this workspace; read its transfer again"
                    .into(),
            )
        })?;
        if crate::worker_lifecycle::preparation_only(daemon, &name, &row.id)? {
            let mut info = status(daemon, &row.id)?
                .ok_or_else(|| Error::Conflict("No preparation to cancel".into()))?;
            if info.phase == TransferPhase::Running {
                return Err(Error::Conflict(
                    "Preparation is still running; cancel again after it settles".into(),
                ));
            }
            info.phase = TransferPhase::Cancelled;
            info.stage = None;
            info.error = None;
            info.finished_at = Some(chrono::Utc::now().to_rfc3339());
            info.to = Some(TransferEndpoint::Machine { name: name.clone() });
            info.log
                .push("cancelled worker preparation; the workspace stays here".into());
            save(daemon, &info);
            crate::worker_lifecycle::finished(daemon, &info)?;
            crate::worker_lifecycle::request_cleanup(daemon, &name)?;
            daemon.events.workspace(
                &row.id,
                WorkspaceEvent::Transfer {
                    transfer: Box::new(info.clone()),
                },
            );
            drop(admission);
            return Ok(info);
        }
        params.destination = TransferEndpoint::Machine { name };
    }
    let controller = daemon
        .transfer_controller
        .socket(daemon, params.controller.as_deref())
        .await
        .map_err(|error| {
            Error::Unavailable(format!(
                "Could not prepare the transfer controller: {error:#}"
            ))
        })?;
    let refuse = |error: anyhow::Error| Error::BadRequest(format!("{error:#}"));
    let mut source = Endpoint::open(daemon, &params.source)
        .await
        .map_err(refuse)?;
    let mut controller = Peer::connect(&controller).await.map_err(refuse)?;
    let workspace: WorkspaceInfo = source
        .call(ClientCall::WorkspaceGet(WorkspaceParams {
            workspace: params.workspace.clone(),
        }))
        .await
        .map_err(refuse)?;
    // Held for the whole cancellation: no transfer of it starts or resumes meanwhile.
    let _slot = Slot::claim(daemon, &workspace.id).ok_or_else(|| {
        Error::Conflict(format!(
            "{} is being transferred; cancel it once it stops or fails",
            workspace.slug
        ))
    })?;
    let mut destination = Endpoint::open(daemon, &params.destination).await;
    let mut log = Vec::new();
    let owners = journal_owners(daemon, &workspace.id);
    let untouched = status(daemon, &workspace.id)?
        .is_some_and(|last| last.phase == TransferPhase::Failed && last.untouched);
    let owner = cancel_run(
        &mut source,
        &mut destination,
        &mut controller,
        &workspace,
        &owners,
        untouched,
        &mut log,
    )
    .await
    .map_err(|error| Error::Conflict(format!("{error:#}")))?;
    let now = chrono::Utc::now().to_rfc3339();
    let label = |end: &TransferEndpoint| match end {
        TransferEndpoint::Cloud { .. } => "Cloud".into(),
        TransferEndpoint::Here => crate::cloud_transfer::host_name(),
        TransferEndpoint::Machine { name } => name.clone(),
        TransferEndpoint::Socket { path } => path.clone(),
    };
    let info = TransferInfo {
        revision: None,
        id: uuid::Uuid::now_v7().to_string(),
        workspace_id: workspace.id.clone(),
        slug: workspace.slug.clone(),
        source: label(&params.source),
        destination: label(&params.destination),
        phase: TransferPhase::Cancelled,
        log,
        error: None,
        workspace_path: Some(workspace.path.clone()),
        kept_copy: None,
        returned: false,
        started_at: now.clone(),
        finished_at: Some(now),
        from: named(&params.source),
        to: named(&params.destination),
        keep_local_copy: params.keep_local_copy,
        journal_owner: Some(owner),
        discard_pending: None,
        stage: None,
        working_agents: 0,
        progress: None,
        preparation: None,
        interrupt: params.interrupt,
        archive: params.archive,
        needs_login: None,
        local_copy_changed: false,
        untouched: false,
    };
    save(daemon, &info);
    crate::worker_lifecycle::finished(daemon, &info)?;
    drop(admission);
    daemon.events.workspace(
        &info.workspace_id,
        WorkspaceEvent::Transfer {
            transfer: Box::new(info.clone()),
        },
    );
    Ok(info)
}

/// A workspace's transfer slot in this daemon: a running transfer or a cancellation.
struct Slot(Arc<Daemon>, String);

impl Slot {
    fn claim(daemon: &Arc<Daemon>, workspace: &str) -> Option<Slot> {
        daemon
            .transfers
            .running
            .lock()
            .expect("transfers poisoned")
            .insert(workspace.to_owned())
            .then(|| Slot(daemon.clone(), workspace.to_owned()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut running) = self.0.transfers.running.lock() {
            running.remove(&self.1);
        }
    }
}

/// The journal owners this daemon's transfers of `workspace` read.
fn journal_owners(daemon: &Daemon, workspace: &str) -> Vec<String> {
    let transfers: Vec<String> = daemon
        .store
        .with(|c| {
            c.prepare("SELECT info FROM workspace_transfers WHERE workspace_id=?1")?
                .query_map([workspace], |r| r.get(0))?
                .collect()
        })
        .unwrap_or_default();
    let mut owners: Vec<String> = transfers
        .iter()
        .filter_map(|raw| {
            serde_json::from_str::<TransferInfo>(raw)
                .ok()?
                .journal_owner
        })
        .collect();
    owners.sort();
    owners.dedup();
    owners
}

/// Cancels it and returns the journal owner it was read under. `untouched`: the last transfer of
/// the workspace here failed before it asked the controller anything (`TransferInfo::untouched`).
async fn cancel_run(
    source: &mut Endpoint,
    destination: &mut anyhow::Result<Endpoint>,
    controller: &mut Peer,
    info: &WorkspaceInfo,
    owners: &[String],
    untouched: bool,
    log: &mut Vec<String>,
) -> anyhow::Result<String> {
    let source_peer = peer(source).await?;
    let destination_peer = match destination {
        Ok(destination) => Some(peer(destination).await?),
        Err(_) => None,
    };
    let mut journal = Journal {
        controller,
        workspace_id: info.id.clone(),
        owner_peer_id: source_peer.clone(),
    };
    // The journal keeps the workspace's first owner: on a way back, that is the destination,
    // which an earlier run here recorded in case it does not answer now.
    let mut known = None;
    for owner in std::iter::once(&source_peer)
        .chain(destination_peer.as_ref())
        .chain(owners)
    {
        let read = CloudTransferJournalParams::Read {
            workspace_id: info.id.clone(),
            owner_peer_id: owner.clone(),
        };
        if let Ok(Some(state)) = journal.call(read).await {
            journal.owner_peer_id = owner.clone();
            known = Some(state);
            break;
        }
    }
    // One the controller never prepared stopped, fenced and staged nothing: it only ends here. A
    // transfer from this source the journal holds in flight, or cancelled without its ends told,
    // still goes through the controller.
    let pending = known.as_ref().is_some_and(|state| {
        state["transfer"]["source_peer_id"].as_str() == Some(source_peer.as_str())
            && matches!(
                state["transfer"]["phase"].as_str(),
                Some(
                    "preparing"
                        | "stopping"
                        | "stopped"
                        | "snapshot_ready"
                        | "restored"
                        | "cancelled"
                )
            )
    });
    if !pending && (untouched || known.is_none()) {
        log.push(format!(
            "nothing of {} had stopped: it runs on the source as it was",
            info.slug
        ));
        return Ok(journal.owner_peer_id);
    }
    let state = known
        .with_context(|| format!("The controller has no transfer of {} to cancel", info.slug))?;
    let transfer = &state["transfer"];
    let from_here = transfer["source_peer_id"].as_str() == Some(source_peer.as_str());
    let cancelled = match transfer["phase"].as_str() {
        Some("preparing" | "stopping" | "stopped" | "snapshot_ready" | "restored") if from_here => {
            journal.advance(&state, CloudTransferStep::Cancel).await?
        }
        // Cancelled already, its source or destination not yet told: tell them again.
        Some("cancelled") if from_here => state,
        Some("committed") => bail!(
            "{} already moved; it runs on the destination now, transfer it back instead",
            info.slug
        ),
        _ => bail!("No transfer of {} from this source is under way", info.slug),
    };
    source
        .peer::<Value>(PeerCall::CloudTransferSourceRelease(CloudTransferState {
            state: cancelled.clone(),
        }))
        .await?;
    log.push(format!("{} runs on the source again", info.slug));
    let dropped = match destination {
        Ok(destination) => destination
            .peer::<Value>(PeerCall::CloudTransferDestinationAbandon(
                CloudTransferState { state: cancelled },
            ))
            .await
            .map(drop),
        Err(error) => Err(anyhow::anyhow!("{error:#}")),
    };
    log.push(match dropped {
        Ok(()) => "the destination dropped what it had received".into(),
        Err(error) => {
            format!("the destination keeps what it received until you cancel again ({error:#})")
        }
    });
    Ok(journal.owner_peer_id)
}

/// Whether a transfer this daemon drove of `workspace` before `current` committed and none has
/// ended done since (attempts refused since are transfers too, and never commit).
fn previous_unfinished(daemon: &Daemon, workspace: &str, current: &str) -> bool {
    let previous: Vec<String> = daemon
        .store
        .with(|c| {
            c.prepare(
                "SELECT info FROM workspace_transfers WHERE workspace_id=?1 AND id!=?2 ORDER BY started_at DESC, rowid DESC",
            )?
            .query_map([workspace, current], |r| r.get(0))?
            .collect()
        })
        .unwrap_or_default();
    for transfer in previous
        .iter()
        .filter_map(|raw| serde_json::from_str::<TransferInfo>(raw).ok())
    {
        if transfer.phase == TransferPhase::Done {
            return false;
        }
        if transfer.log.iter().any(|line| line == COMMITTED) {
            return true;
        }
    }
    false
}

/// An end an app can name again: this daemon or a declared machine, not a socket.
fn named(end: &TransferEndpoint) -> Option<TransferEndpoint> {
    (!matches!(end, TransferEndpoint::Socket { .. })).then(|| end.clone())
}

/// Hands a worker this computer's GitHub and AWS SSO access before its check, which then finds gh signed
/// in there. What fails is said and the transfer goes on: a worker without them still runs the
/// workspace.
async fn give_access(destination: &mut Endpoint, worker: &str, report: &mut Report) {
    let (credentials, mut warnings) = crate::worker_credentials::gather(&report.daemon).await;
    if !credentials.github.is_empty()
        || credentials.aws_config.is_some()
        || !credentials.aws_sso_cache.is_empty()
    {
        match destination
            .peer::<WorkerCredentialsInstalled>(PeerCall::WorkerCredentialsInstall(credentials))
            .await
        {
            Ok(installed) => warnings.extend(installed.warnings),
            Err(error) => warnings.push(format!(
                "Could not give {worker} this computer's GitHub and AWS access: {error:#}"
            )),
        }
    }
    match give_claude_config(destination, &report.daemon).await {
        Ok(installed) => warnings.extend(installed),
        Err(error) => warnings.push(format!(
            "Could not give {worker} this computer's Claude Code configuration: {error:#}"
        )),
    }
    for warning in warnings {
        report.line(format!("  {warning}"));
    }
}

/// Hands a worker this computer's Claude Code configuration, in chunks, unless it installed the same
/// last time; what it left out, as warnings.
async fn give_claude_config(
    destination: &mut Endpoint,
    daemon: &Arc<Daemon>,
) -> anyhow::Result<Vec<String>> {
    use crate::worker_claude_config::{CHUNK, Locations, SENDING, pack};
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use tokio::io::AsyncReadExt;
    let Some(at) = Locations::of(&daemon.shell_env.vars()) else {
        return Ok(Vec::new());
    };
    let _sending = SENDING.lock().await;
    let dir = daemon.paths.home.clone();
    let Some(archive) = tokio::task::spawn_blocking(move || pack(&at, &dir)).await?? else {
        return Ok(Vec::new());
    };
    let status: WorkerClaudeConfigStatus = destination
        .peer(PeerCall::WorkerClaudeConfigStatus(EmptyParams {}))
        .await?;
    if status.digest.as_deref() == Some(archive.digest.as_str()) {
        tracing::info!("a worker has this Claude Code configuration already");
        return Ok(Vec::new());
    }
    let mut file = tokio::fs::File::open(archive.file.path()).await?;
    let mut buffer = vec![0; CHUNK];
    let mut offset = 0;
    while offset < archive.bytes {
        let length = CHUNK.min((archive.bytes - offset) as usize);
        file.read_exact(&mut buffer[..length]).await?;
        destination
            .peer_value(PeerCall::WorkerClaudeConfigWrite(WorkerClaudeConfigChunk {
                digest: archive.digest.clone(),
                offset,
                bytes: URL_SAFE_NO_PAD.encode(&buffer[..length]),
            }))
            .await?;
        offset += length as u64;
    }
    let installed: WorkerClaudeConfigInstalled = destination
        .peer(PeerCall::WorkerClaudeConfigInstall(
            WorkerClaudeConfigInstallParams {
                digest: archive.digest.clone(),
                bytes: archive.bytes,
            },
        ))
        .await?;
    tracing::info!(
        bytes = archive.bytes,
        "gave a worker the Claude Code configuration"
    );
    Ok(installed.warnings)
}

/// What to do about a worker with no agent signed in: sign one in from this machine.
fn logged_out_problem(worker: &str) -> String {
    format!(
        "no agent is signed in on {worker}: sign Claude Code or Codex in from Settings → Transfers (or `chauffeur worker login {worker}`); the transfer resumes once one is"
    )
}

/// The transfers this daemon is driving.
pub fn running(daemon: &Daemon) -> Vec<TransferInfo> {
    let running: Vec<String> = daemon
        .transfers
        .running
        .lock()
        .expect("transfers poisoned")
        .iter()
        .cloned()
        .collect();
    running
        .iter()
        .filter_map(|workspace| status(daemon, workspace).ok().flatten())
        .collect()
}

/// The latest transfer of `workspace` this daemon drove, by id or name.
pub fn status(daemon: &Daemon, workspace: &str) -> Result<Option<TransferInfo>> {
    let raw: Option<String> = daemon.store.with(|c| {
        c.query_row(
            "SELECT info FROM workspace_transfers WHERE workspace_id=?1 OR slug=?1 ORDER BY started_at DESC, rowid DESC LIMIT 1",
            [workspace],
            |r| r.get(0),
        )
        .optional()
    })?;
    raw.map(|raw| serde_json::from_str(&raw).map_err(|e| Error::Internal(e.into())))
        .transpose()
}

/// The transfers to `worker` that failed for want of a login there: each one starts again, as
/// Resume would, once one of the agents it waits for is `signed_in`.
pub(crate) async fn resume_after_login(
    daemon: &Arc<Daemon>,
    worker: &str,
    signed_in: impl Fn(AgentKind) -> bool,
) -> Result<()> {
    let workspaces: Vec<String> = daemon.store.with(|c| {
        c.prepare("SELECT DISTINCT workspace_id FROM workspace_transfers")?
            .query_map([], |r| r.get(0))?
            .collect()
    })?;
    for workspace in workspaces {
        let Some(info) = status(daemon, &workspace)? else {
            continue;
        };
        if !resumes_after_login(&info, worker, &signed_in) {
            continue;
        }
        let (Some(source), Some(destination)) = (info.from, info.to) else {
            continue;
        };
        let resumed = start(
            daemon,
            CloudTransferStartParams {
                workspace: info.workspace_id,
                source,
                destination,
                controller: None,
                keep_local_copy: info.keep_local_copy,
                interrupt: info.interrupt,
                archive: info.archive,
            },
        )
        .await;
        if let Err(error) = resumed {
            tracing::warn!(%error, workspace = info.slug, "could not resume a transfer after its worker's login");
        }
    }
    Ok(())
}

/// Whether `info` is a transfer refused for want of a login on `worker` that one of the agents
/// now `signed_in` there lets go.
fn resumes_after_login(
    info: &TransferInfo,
    worker: &str,
    signed_in: impl Fn(AgentKind) -> bool,
) -> bool {
    info.needs_login.as_ref().is_some_and(|login| {
        info.phase == TransferPhase::Failed
            && login.worker == worker
            && login.agents.iter().any(|agent| signed_in(*agent))
    })
}

/// The machines a transfer can go to now, by name: none without a controller; the declared ones,
/// then the ready workers.
pub fn machines(daemon: &Daemon) -> CloudTransferMachines {
    let settings = daemon.settings.get();
    let transfer = &settings.transfer;
    CloudTransferMachines {
        machines: transfer
            .machines
            .iter()
            .filter(|m| !m.name.trim().is_empty() && !m.socket.trim().is_empty())
            .map(|m| m.name.clone())
            .chain(crate::workers::ready(daemon))
            .collect(),
    }
}

/// The copy a return left on its source while its deletion there is pending: the source to ask
/// again, and the commit that retired the copy, which the deletion is bound to.
#[derive(serde::Serialize, serde::Deserialize)]
struct PendingDiscard {
    source: TransferEndpoint,
    committed: Value,
}

/// Records, or forgets with `None`, the copy transfer `id` left pending on its source.
fn follow_discard(daemon: &Daemon, id: &str, pending: Option<&PendingDiscard>) {
    let raw = pending.and_then(|pending| serde_json::to_string(pending).ok());
    if let Err(error) = daemon.store.with(|c| {
        c.execute(
            "UPDATE workspace_transfers SET discard=?2 WHERE id=?1",
            params![id, raw],
        )
    }) {
        tracing::warn!(%error, "could not record a copy whose deletion is pending");
    }
}

/// Asks each source still holding the copy a return left whether it is deleted now (the ask is
/// itself a try), and tells the apps when that changed. Only a workspace's latest transfer is
/// followed; `workspace` limits it to one. A source that does not answer is asked next time.
pub async fn check_discards(daemon: &Arc<Daemon>, workspace: Option<&str>) {
    let rows: Vec<(String, String, String, String)> = daemon
        .store
        .with(|c| {
            c.prepare(
                "SELECT id,workspace_id,info,discard FROM workspace_transfers WHERE discard IS NOT NULL AND (?1 IS NULL OR workspace_id=?1)",
            )?
            .query_map([workspace], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect()
        })
        .unwrap_or_default();
    for (id, workspace, info, pending) in rows {
        let latest = status(daemon, &workspace).ok().flatten().map(|t| t.id);
        let (Some(mut info), Some(pending), true) = (
            serde_json::from_str::<TransferInfo>(&info).ok(),
            serde_json::from_str::<PendingDiscard>(&pending).ok(),
            latest.as_deref() == Some(id.as_str()),
        ) else {
            follow_discard(daemon, &id, None);
            continue;
        };
        // Still running its last steps, the transfer reports for itself.
        if info.phase == TransferPhase::Running {
            continue;
        }
        let answer = async {
            let mut source = Endpoint::open(daemon, &pending.source).await?;
            source
                .peer::<CloudTransferDiscarded>(PeerCall::CloudTransferSourceDiscard(
                    CloudTransferState {
                        state: pending.committed.clone(),
                    },
                ))
                .await
        }
        .await;
        let discarded = match answer {
            Ok(discarded) => discarded,
            Err(error) => {
                tracing::warn!(%error, workspace, "could not ask a source about the copy it still holds");
                continue;
            }
        };
        if discarded.pending.is_some() && discarded.pending == info.discard_pending {
            continue;
        }
        info.discard_pending = discarded.pending;
        if info.discard_pending.is_none() {
            info.log
                .push(format!("deleted its copy on {}", discarded.machine));
            follow_discard(daemon, &id, None);
        }
        if let Err(error) = crate::worker_lifecycle::finished(daemon, &info) {
            tracing::warn!(%error, "could not settle a returned worker reservation");
        }
        save(daemon, &info);
        daemon.events.workspace(
            &workspace,
            WorkspaceEvent::Transfer {
                transfer: Box::new(info),
            },
        );
    }
}

/// Follows pending deletions at start, then every ten minutes, as the sources retry them.
pub fn spawn_discard_checks(daemon: Arc<Daemon>) {
    tokio::spawn(async move {
        loop {
            check_discards(&daemon, None).await;
            tokio::time::sleep(std::time::Duration::from_secs(10 * 60)).await;
        }
    });
}

/// When a client reads a transfer whose copy is still pending on its source: the source is asked
/// again in the background, at most every ten seconds per workspace, and the answer sent as an
/// event.
pub fn refresh_discard(daemon: &Arc<Daemon>, transfer: Option<&TransferInfo>) {
    let Some(transfer) =
        transfer.filter(|t| t.discard_pending.is_some() && t.phase != TransferPhase::Running)
    else {
        return;
    };
    let now = std::time::Instant::now();
    let mut asked = daemon.transfers.asked.lock().expect("transfers poisoned");
    if asked
        .get(&transfer.workspace_id)
        .is_some_and(|at| now.duration_since(*at) < std::time::Duration::from_secs(10))
    {
        return;
    }
    asked.insert(transfer.workspace_id.clone(), now);
    let (daemon, workspace) = (daemon.clone(), transfer.workspace_id.clone());
    tokio::spawn(async move { check_discards(&daemon, Some(&workspace)).await });
}

/// Where `workspace` went, when a committed transfer retired it here: the destination of the
/// transfer this daemon drove, or "another machine" when another daemon drove it.
pub fn moved_to(daemon: &Daemon, workspace: &str) -> Option<String> {
    if let Some(worker) = crate::cloud_backup_recovery::pending_worker(daemon, workspace) {
        return Some(worker);
    }
    Some(moving(daemon, workspace)?.map_or_else(|| "another machine".into(), |t| t.destination))
}

/// Where `workspace` is on the machine a transfer this daemon drove moved it to.
pub fn moved_path(daemon: &Daemon, workspace: &str) -> Option<String> {
    moving(daemon, workspace)??.workspace_path
}

/// When a committed transfer retired `workspace` here: the transfer this daemon drove that moved
/// it, none when another daemon drove it.
fn moving(daemon: &Daemon, workspace: &str) -> Option<Option<TransferInfo>> {
    crate::cloud_workspace_return::committed_fence(daemon, workspace)
        .ok()
        .flatten()?;
    let transfers: Vec<String> = daemon
        .store
        .with(|c| {
            c.prepare(
                "SELECT info FROM workspace_transfers WHERE workspace_id=?1 ORDER BY started_at DESC, rowid DESC",
            )?
            .query_map([workspace], |r| r.get(0))?
            .collect()
        })
        .unwrap_or_default();
    Some(
        transfers
            .iter()
            .filter_map(|raw| serde_json::from_str::<TransferInfo>(raw).ok())
            .find(|t| t.phase == TransferPhase::Done && !t.returned),
    )
}

/// At start: a transfer that was running when the daemon stopped did not finish here; the
/// daemons it called keep their own state, and the user retries.
pub fn recover(daemon: &Daemon) {
    let running: Vec<String> = daemon
        .store
        .with(|c| {
            c.prepare("SELECT info FROM workspace_transfers")?
                .query_map([], |r| r.get(0))?
                .collect()
        })
        .unwrap_or_default();
    for raw in running {
        let Ok(mut info) = serde_json::from_str::<TransferInfo>(&raw) else {
            continue;
        };
        if info.phase == TransferPhase::Running {
            info.phase = TransferPhase::Failed;
            info.stage = None;
            info.working_agents = 0;
            info.error = Some(
                "Chauffeur stopped during the transfer; each machine kept where it was: transfer again to go on, or cancel it".into(),
            );
            info.finished_at = Some(chrono::Utc::now().to_rfc3339());
            save(daemon, &info);
        }
    }
}

fn save(daemon: &Daemon, info: &TransferInfo) {
    let Ok(raw) = serde_json::to_string(info) else {
        return;
    };
    let saved = daemon.store.with(|c| {
        c.execute(
            "INSERT INTO workspace_transfers(id,workspace_id,slug,started_at,info) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(id) DO UPDATE SET info=excluded.info",
            params![info.id, info.workspace_id, info.slug, info.started_at, raw],
        )
    });
    if let Err(error) = saved {
        tracing::warn!(%error, "could not record a transfer's progress");
    }
}

/// The transfer's progress: kept, and sent to the apps, at each line.
struct Report {
    daemon: Arc<Daemon>,
    info: TransferInfo,
    /// Where it comes from, socket included, to ask again about a copy left there.
    source: TransferEndpoint,
    /// It asked the controller to prepare it, or went on with a transfer of the workspace in
    /// flight: failing from then on, a cancellation goes through the controller.
    journaled: bool,
}

impl Report {
    fn line(&mut self, line: String) {
        tracing::info!(workspace = %self.info.slug, step = %line, "transfer step");
        self.info.log.push(line);
        self.save();
    }

    fn stage(&mut self, stage: TransferStage) {
        if self.info.stage != Some(stage) {
            self.info.stage = Some(stage);
            self.info.progress = Some(TransferProgress {
                stage_started_at: chrono::Utc::now().to_rfc3339(),
                confirmed_bytes: 0,
                total_bytes: 0,
            });
            self.info.working_agents = 0;
            self.save();
        }
    }

    /// Waiting for `working` agents to end their turn, interrupting them or not: said once each
    /// time it changes.
    fn waiting(&mut self, working: u32, interrupt: bool) {
        let info = &self.info;
        if info.stage == Some(TransferStage::WaitingForTurns)
            && info.working_agents == working
            && info.interrupt == interrupt
        {
            return;
        }
        let agents = if working == 1 {
            "1 agent".to_owned()
        } else {
            format!("{working} agents")
        };
        self.stage(TransferStage::WaitingForTurns);
        self.info.working_agents = working;
        self.info.interrupt = interrupt;
        self.line(if interrupt {
            format!("interrupting {agents} of the workspace")
        } else {
            format!("waiting for {agents} of the workspace to end their turn")
        });
    }

    fn save(&mut self) {
        self.info.revision = Some(self.info.revision.unwrap_or(0).saturating_add(1));
        save(&self.daemon, &self.info);
        self.daemon.events.workspace(
            &self.info.workspace_id,
            WorkspaceEvent::Transfer {
                transfer: Box::new(self.info.clone()),
            },
        );
    }

    /// Records how it ended: moved, cancelled before the source stopped (`None`), or failed.
    fn finish(&mut self, outcome: anyhow::Result<Option<CloudTransferRestored>>) {
        self.info.stage = None;
        self.info.working_agents = 0;
        match outcome {
            Ok(None) => {
                self.info.phase = TransferPhase::Cancelled;
                self.info.log.push(format!(
                    "cancelled before anything stopped; {} runs on {} as it was",
                    self.info.slug, self.info.source
                ));
            }
            Ok(Some(restored)) => {
                self.info.phase = TransferPhase::Done;
                self.info.workspace_path = Some(restored.workspace_path);
                self.info.kept_copy = restored.kept_copy;
                self.info.returned = restored.returned;
            }
            Err(error) => {
                self.info.phase = TransferPhase::Failed;
                self.info.local_copy_changed |=
                    crate::cloud_workspace_return::is_local_copy_changed(&error);
                self.info.error = Some(format!("{error:#}"));
                self.info.untouched = !self.journaled;
            }
        }
        self.info.finished_at = Some(chrono::Utc::now().to_rfc3339());
        tracing::info!(
            workspace = %self.info.slug,
            phase = ?self.info.phase,
            started_at = %self.info.started_at,
            "transfer ended"
        );
        self.save();
        // Its `moved_to` reads this transfer now that it is recorded done.
        self.daemon.workspaces.publish(&self.info.workspace_id);
        if self.info.phase != TransferPhase::Cancelled {
            self.daemon.events.registry(RegistryEvent::TransferEnded {
                transfer: Box::new(self.info.clone()),
            });
        }
    }
}

struct Parties {
    source: Endpoint,
    destination: Endpoint,
    controller: Peer,
}

struct Journal<'a> {
    controller: &'a mut Peer,
    workspace_id: String,
    owner_peer_id: String,
}

impl Journal<'_> {
    async fn call(&mut self, params: CloudTransferJournalParams) -> anyhow::Result<Option<Value>> {
        let result: CloudTransferJournal = serde_json::from_value(
            self.controller
                .peer_value(PeerCall::CloudTransferJournal(params))
                .await?,
        )?;
        Ok(result.state)
    }

    async fn advance(&mut self, state: &Value, step: CloudTransferStep) -> anyhow::Result<Value> {
        let transfer = &state["transfer"];
        self.call(CloudTransferJournalParams::Advance {
            workspace_id: self.workspace_id.clone(),
            owner_peer_id: self.owner_peer_id.clone(),
            revision: number(state, "revision")?,
            operation_id: text(transfer, "operation_id")?,
            source_epoch: number(transfer, "source_epoch")?,
            step,
        })
        .await?
        .context("the controller's journal changed under this transfer; read it before retrying")
    }
}

fn number(value: &Value, field: &str) -> anyhow::Result<u64> {
    value[field]
        .as_u64()
        .with_context(|| format!("the journal state has no {field}"))
}

fn text(value: &Value, field: &str) -> anyhow::Result<String> {
    Ok(value[field]
        .as_str()
        .with_context(|| format!("the journal state has no {field}"))?
        .to_owned())
}

async fn peer(endpoint: &mut Endpoint) -> anyhow::Result<String> {
    let peer: CloudTransferPeer = endpoint
        .peer(PeerCall::CloudTransferPeer(EmptyParams {}))
        .await?;
    Ok(peer.peer_id)
}

/// The source's agents, as a transfer waiting for the end of their turns sees them.
trait Agents {
    /// The chats of `workspace` whose agent is in a turn: working, starting, or waiting for the
    /// user's answer, which ends the turn no sooner.
    async fn in_turn(&mut self, workspace: &str) -> anyhow::Result<Vec<String>>;
    async fn interrupt(&mut self, chat: &str) -> anyhow::Result<()>;
}

impl Agents for Endpoint {
    async fn in_turn(&mut self, workspace: &str) -> anyhow::Result<Vec<String>> {
        let chats: ChatsResult = self
            .call(ClientCall::ChatList(ChatListParams {
                workspace: workspace.to_owned(),
                all: true,
            }))
            .await?;
        Ok(chats
            .chats
            .into_iter()
            .filter(|chat| {
                matches!(
                    chat.status,
                    ChatStatus::Working | ChatStatus::Starting | ChatStatus::NeedsInput
                )
            })
            .map(|chat| chat.id)
            .collect())
    }

    async fn interrupt(&mut self, chat: &str) -> anyhow::Result<()> {
        self.call_value(ClientCall::ChatInterrupt(ChatParams {
            chat: chat.to_owned(),
        }))
        .await
        .map(drop)
    }
}

/// Waits until none of the workspace's agents is in a turn, interrupting each once when asked,
/// then for a place among the transfers running at once; looks again once placed, since a turn
/// may have started meanwhile. Returns the place, or none when the transfer was cancelled.
async fn settle(
    agents: &mut impl Agents,
    workspace: &str,
    asks: &mut watch::Receiver<Ask>,
    at_once: &Arc<Semaphore>,
    report: &mut Report,
) -> anyhow::Result<Option<OwnedSemaphorePermit>> {
    let mut interrupted = HashSet::new();
    loop {
        if *asks.borrow() == Ask::Cancel {
            return Ok(None);
        }
        let working = agents.in_turn(workspace).await?;
        interrupted.retain(|chat| working.contains(chat));
        if !working.is_empty() {
            let interrupt = *asks.borrow() == Ask::Interrupt;
            if interrupt {
                for chat in &working {
                    if interrupted.insert(chat.clone())
                        && let Err(error) = agents.interrupt(chat).await
                    {
                        report.line(format!("  could not interrupt chat {chat}: {error:#}"));
                    }
                }
            }
            report.waiting(working.len() as u32, interrupt);
            tokio::select! {
                _ = asks.changed() => {}
                _ = tokio::time::sleep(TURNS_POLL) => {}
            }
            continue;
        }
        let place = match at_once.clone().try_acquire_owned() {
            Ok(place) => place,
            Err(_) => {
                report.stage(TransferStage::Queued);
                report.line("waiting for another transfer to end".into());
                tokio::select! {
                    place = at_once.clone().acquire_owned() => place.context("the transfers' places closed")?,
                    _ = asks.changed() => continue,
                }
            }
        };
        if agents.in_turn(workspace).await?.is_empty() {
            return Ok(Some(place));
        }
    }
}

/// Waits for a place among the transfers running at once, for one that already stopped its source.
async fn place(
    at_once: &Arc<Semaphore>,
    report: &mut Report,
) -> anyhow::Result<OwnedSemaphorePermit> {
    if let Ok(place) = at_once.clone().try_acquire_owned() {
        return Ok(place);
    }
    report.stage(TransferStage::Queued);
    report.line("waiting for another transfer to end".into());
    at_once
        .clone()
        .acquire_owned()
        .await
        .context("the transfers' places closed")
}

/// Moves the workspace, or goes on with a transfer left part way; none when it was cancelled
/// before the source stopped.
async fn run(
    parties: &mut Parties,
    info: &WorkspaceInfo,
    keep_local_copy: bool,
    asks: &mut watch::Receiver<Ask>,
    report: &mut Report,
) -> anyhow::Result<Option<CloudTransferRestored>> {
    let Parties {
        source,
        destination,
        controller,
    } = parties;
    let source_peer = peer(source).await?;
    let destination_peer = peer(destination).await?;
    if source_peer == destination_peer {
        bail!("The source and the destination are the same daemon; name another destination");
    }
    let mut journal = Journal {
        controller,
        workspace_id: info.id.clone(),
        owner_peer_id: source_peer.clone(),
    };
    // The journal keeps the workspace's first owner: on a way back, that is the destination.
    let mut known = None;
    for owner in [&source_peer, &destination_peer] {
        let read = CloudTransferJournalParams::Read {
            workspace_id: info.id.clone(),
            owner_peer_id: owner.clone(),
        };
        if let Ok(Some(state)) = journal.call(read).await {
            journal.owner_peer_id = owner.clone();
            known = Some(state);
            break;
        }
    }
    // Kept so that cancelling it later can read the journal without the destination.
    report.info.journal_owner = Some(journal.owner_peer_id.clone());
    report.save();
    // A transfer between these two that did not finish goes on from where the journal says it is;
    // another one in flight must end first.
    let phase = known
        .as_ref()
        .and_then(|s| s["transfer"]["phase"].as_str())
        .map(str::to_owned);
    let ours = known.as_ref().is_some_and(|s| {
        s["transfer"]["source_peer_id"].as_str() == Some(source_peer.as_str())
            && s["transfer"]["target_peer_id"].as_str() == Some(destination_peer.as_str())
    });
    let in_flight = matches!(
        phase.as_deref(),
        Some("preparing" | "stopping" | "stopped" | "snapshot_ready" | "restored")
    );
    // Committed, but the transfer that committed it did not end here (its driver stopped, even
    // after retiring the source): activation, retirement and a return's discard are left to
    // finish, each idempotent. One that ended is not run again: the workspace has moved.
    let unfinished = phase.as_deref() == Some("committed")
        && ours
        && known.as_ref().and_then(|s| s["execution_peer_id"].as_str())
            == Some(destination_peer.as_str())
        && previous_unfinished(&report.daemon, &info.id, &report.info.id);
    // A transfer of it in flight, this one or another: cancelling goes through the controller.
    report.journaled = in_flight || unfinished;
    if in_flight && !ours {
        bail!(
            "Another transfer of {} is under way; finish or cancel it before this one",
            info.slug
        );
    }
    let mut state = if in_flight || unfinished {
        report.line(format!(
            "going on with the transfer left at {}",
            phase.as_deref().unwrap_or_default().replace('_', " ")
        ));
        known.clone().context("the journal changed")?
    } else {
        // Before anything stops: the source's folder must be one the capture takes (the source
        // checks again before stopping), and the destination able to take it and run its agents.
        report.line("checking the destination".into());
        let chats: ChatsResult = source
            .call(ClientCall::ChatList(ChatListParams {
                workspace: info.id.clone(),
                all: true,
            }))
            .await?;
        let services: chauffeur_protocol::dev_services::DevServices = source
            .call(ClientCall::ServicesGet(WorkspaceParams {
                workspace: info.id.clone(),
            }))
            .await?;
        let mut compose_projects: Vec<String> = services
            .services
            .iter()
            .filter_map(|s| s.compose.as_ref().map(|c| c.project.clone()))
            .filter(|project| !project.is_empty())
            .collect();
        compose_projects.sort();
        compose_projects.dedup();
        // A worker of this daemon has its agents signed in from here, never from a terminal there.
        let worker = match &report.info.to {
            Some(TransferEndpoint::Machine { name })
                if crate::workers::is_worker(&report.daemon, name) =>
            {
                Some(name.clone())
            }
            _ => None,
        };
        if let Some(worker) = &worker {
            give_access(destination, worker, report).await;
        }
        let readiness: CloudTransferReadiness = destination
            .peer(PeerCall::CloudTransferDestinationCheck(
                CloudTransferCheckParams {
                    workspace_id: info.id.clone(),
                    slug: info.slug.clone(),
                    source_peer_id: source_peer.clone(),
                    estimated_bytes: source
                        .peer::<chauffeur_protocol::cloud_transfer::TransferSizeEstimate>(
                            PeerCall::CloudTransferSourceEstimate(WorkspaceParams {
                                workspace: info.id.clone(),
                            }),
                        )
                        .await
                        .map_err(|error| anyhow::anyhow!("{error:#}; nothing was stopped"))?
                        .bytes,
                    // On a worker, one agent signed in is enough, whichever the chats use.
                    agents: match worker {
                        Some(_) => crate::usage::AGENTS.to_vec(),
                        None => chats.chats.iter().map(|chat| chat.agent).collect(),
                    },
                    journal: known.clone(),
                    keep_local_copy,
                    compose_projects,
                    sign_in_from_caller: worker.is_some(),
                },
            ))
            .await?;
        for warning in readiness.warnings {
            report.line(format!("  {warning}"));
        }
        let mut problems = readiness.problems;
        if let Some(worker) = worker {
            if readiness.logged_out.len() == crate::usage::AGENTS.len() {
                problems.push(logged_out_problem(&worker));
                report.info.needs_login = Some(TransferLogin {
                    worker,
                    agents: readiness.logged_out,
                });
            } else {
                for agent in &readiness.logged_out {
                    report.line(format!(
                        "  {} is not signed in on {worker}: its chats wait there for a sign-in (Settings → Transfers)",
                        crate::chats::agent_name(*agent)
                    ));
                }
            }
        }
        report.info.local_copy_changed = readiness.local_copy_changed;
        if !problems.is_empty() {
            let machine = match &report.info.needs_login {
                Some(login) => login.worker.clone(),
                None => readiness.machine,
            };
            bail!(
                "{machine} cannot take {} yet; nothing was stopped:\n  {}",
                info.slug,
                problems.join("\n  ")
            );
        }
        let current = match known {
            Some(state) => state,
            None => journal
                .call(CloudTransferJournalParams::Register {
                    workspace_id: info.id.clone(),
                    owner_peer_id: source_peer.clone(),
                })
                .await?
                .context("the controller did not register the workspace")?,
        };
        if text(&current, "execution_peer_id")? != source_peer {
            bail!(
                "{} does not run on the source according to the controller; transfer it from where it runs",
                info.slug
            );
        }
        report.journaled = true;
        journal
            .call(CloudTransferJournalParams::Prepare {
                workspace_id: info.id.clone(),
                owner_peer_id: journal.owner_peer_id.clone(),
                revision: number(&current, "revision")?,
                operation_id: uuid::Uuid::now_v7().to_string(),
                target_peer_id: destination_peer.clone(),
            })
            .await?
            .context("the controller's journal changed; retry the transfer")?
    };
    let mut restored: Option<CloudTransferRestored> = None;
    let daemon = report.daemon.clone();
    let at_once = &daemon.transfers.at_once;
    // Held from the source's stop to the destination's activation.
    let mut held = None;
    loop {
        let phase = text(&state["transfer"], "phase")?;
        if phase != "preparing" && held.is_none() {
            if daemon.transfers.leave_waiting(&info.id) {
                bail!(
                    "The transfer of {} had already stopped its source; cancel it again",
                    info.slug
                );
            }
            if phase != "committed" {
                held = Some(place(at_once, report).await?);
            }
        }
        state = match phase.as_str() {
            "preparing" => {
                // Prepared while the agents work; nothing stops before their turns end.
                let Some(place) = settle(source, &info.id, asks, at_once, report).await? else {
                    journal.advance(&state, CloudTransferStep::Cancel).await?;
                    return Ok(None);
                };
                held = Some(place);
                if daemon.transfers.leave_waiting(&info.id) {
                    journal.advance(&state, CloudTransferStep::Cancel).await?;
                    return Ok(None);
                }
                journal
                    .advance(&state, CloudTransferStep::BeginStop)
                    .await?
            }
            "stopping" => {
                report.stage(TransferStage::Stopping);
                report.line(format!("stopping {} on the source", info.slug));
                let stopped: CloudTransferDigest = source
                    .peer(PeerCall::CloudTransferSourceStop(CloudTransferState {
                        state: state.clone(),
                    }))
                    .await
                    .context("the source could not stop it")?;
                journal
                    .advance(
                        &state,
                        CloudTransferStep::SourceStopped {
                            peer_id: source_peer.clone(),
                            receipt_digest: stopped.digest,
                        },
                    )
                    .await?
            }
            "stopped" => {
                report.stage(TransferStage::Stopping);
                report.line("capturing and sealing it".into());
                let captured: CloudTransferCaptured = source
                    .peer(PeerCall::CloudTransferSourceCapture(
                        CloudTransferCaptureParams {
                            stopping: stopping_before(&state)?,
                            stopped: state.clone(),
                        },
                    ))
                    .await
                    .context("the source could not capture it")?;
                for (chat, reason) in &captured.blocked_chats {
                    report.line(format!(
                        "  chat {chat} cannot resume on the destination: {reason}"
                    ));
                }
                journal
                    .advance(
                        &state,
                        CloudTransferStep::SnapshotPublished {
                            peer_id: source_peer.clone(),
                            snapshot_digest: captured.snapshot_digest,
                        },
                    )
                    .await?
            }
            "snapshot_ready" | "restored" => {
                // Sealed once; asked again, the source gives the ciphertext it staged, which the
                // destination keeps once received and restores once.
                let published = if phase == "restored" {
                    published_before(&state)?
                } else {
                    state.clone()
                };
                let sealed: CloudTransferSealed = source
                    .peer(PeerCall::CloudTransferSourceSeal(CloudTransferState {
                        state: published.clone(),
                    }))
                    .await
                    .context("the source could not seal it")?;
                report.stage(TransferStage::Sending);
                report.line(format!(
                    "sending {} KiB of ciphertext",
                    sealed.binding.head.bytes.div_ceil(1024)
                ));
                let received = CloudArchiveBinding {
                    local_peer_id: destination_peer.clone(),
                    ..sealed.binding.clone()
                };
                relay(source, destination, &sealed.binding, &received, report)
                    .await
                    .context("the archive could not be sent")?;
                report.stage(TransferStage::Restoring);
                report.line("restoring it on the destination".into());
                let done: CloudTransferRestored = destination
                    .peer(PeerCall::CloudTransferDestinationRestore(
                        CloudTransferRestoreParams {
                            published: published.clone(),
                            key: sealed.key,
                            binding: received,
                            keep_local_copy,
                        },
                    ))
                    .await
                    .context("the destination could not restore it")?;
                let snapshot = text(&state["transfer"], "snapshot_digest")?;
                let next = if phase == "snapshot_ready" {
                    let restored_state = journal
                        .advance(
                            &state,
                            CloudTransferStep::DestinationRestored {
                                peer_id: destination_peer.clone(),
                                snapshot_digest: snapshot.clone(),
                                receipt_digest: done.receipt_digest.clone(),
                            },
                        )
                        .await?;
                    journal
                        .advance(
                            &restored_state,
                            CloudTransferStep::Commit {
                                snapshot_digest: snapshot,
                                destination_receipt_digest: done.receipt_digest.clone(),
                            },
                        )
                        .await?
                } else {
                    journal
                        .advance(
                            &state,
                            CloudTransferStep::Commit {
                                snapshot_digest: snapshot,
                                destination_receipt_digest: done.receipt_digest.clone(),
                            },
                        )
                        .await?
                };
                restored = Some(done);
                next
            }
            "committed" => break,
            other => bail!("The transfer is {other}; start a new one"),
        };
    }
    let committed = state;

    report.stage(TransferStage::Activating);
    report.line(COMMITTED.into());
    // A GCP worker of this daemon installs the toolchains its repos declare before their setup.
    let worker = match &report.info.to {
        Some(TransferEndpoint::Machine { name }) => crate::workers::is_worker(&report.daemon, name),
        _ => false,
    };
    let activated: CloudTransferActivated = destination
        .peer(PeerCall::CloudTransferDestinationActivate(
            CloudTransferActivateParams {
                state: committed.clone(),
                toolchains: worker,
            },
        ))
        .await?;
    source
        .peer_value(PeerCall::CloudTransferSourceRetire(CloudTransferState {
            state: committed.clone(),
        }))
        .await?;
    // On a GCP worker, it is backed up from now on, with this daemon's key for it.
    if let Some(TransferEndpoint::Machine { name }) = report.info.to.clone()
        && crate::workers::is_worker(&report.daemon, &name)
    {
        let line = crate::cloud_workspace_backup::configure_worker(
            &report.daemon,
            &info.id,
            &name,
            async |params| {
                destination
                    .peer_value(PeerCall::CloudBackupConfigure(params))
                    .await
                    .map(drop)
            },
        )
        .await;
        report.line(line);
    } else {
        // Back here, or on a machine that is not backed up: what a worker said is history.
        crate::cloud_workspace_backup::forget(&report.daemon, &info.id);
    }
    // Back where it came from: the copy it leaves is deleted. A failure only delays that; the
    // workspace already runs on the destination.
    if activated.returned {
        let discarded: CloudTransferDiscarded = source
            .peer(PeerCall::CloudTransferSourceDiscard(CloudTransferState {
                state: committed.clone(),
            }))
            .await?;
        report.info.discard_pending = discarded.pending.clone();
        if discarded.pending.is_some() {
            follow_discard(
                &report.daemon,
                &report.info.id,
                Some(&PendingDiscard {
                    source: report.source.clone(),
                    committed,
                }),
            );
        }
        report.line(match &discarded.pending {
            None => format!("deleted its copy on {}", discarded.machine),
            Some(error) => format!(
                "  its copy on {} could not be deleted yet ({error}); it is retried there",
                discarded.machine
            ),
        });
    }
    // The setup scripts run on the destination alone: another transfer may go on meanwhile.
    drop(held);
    let preparation: anyhow::Result<TransferPreparation> = async {
        let mut failures = Vec::new();
        let setup = match worker {
            true => {
                let started: CloudTransferSetup = destination
                    .peer(PeerCall::CloudTransferDestinationSetup(
                        CloudTransferSetupParams {
                            workspace_id: info.id.clone(),
                        },
                    ))
                    .await?;
                failures = started.failures;
                for line in started.toolchains {
                    report.line(line);
                }
                started.setup
            }
            false => activated.setup,
        };
        // Back to be archived, it would only stop what its setup started.
        if !setup.is_empty() && !report.info.archive {
            report.line(format!("running {} on the destination", setup.join(", ")));
            report.info.preparation = Some(TransferPreparation::Running {
                scripts: setup.clone(),
            });
            report.save();
            let outcome = setup_outcome(destination, &info.id, &setup, report).await?;
            if failures.is_empty() || matches!(outcome, TransferPreparation::NeedsAttention { .. }) {
                return Ok(outcome);
            }
        }
        if failures.is_empty() {
            Ok(TransferPreparation::Ready)
        } else {
            Ok(TransferPreparation::NeedsAttention {
                scripts: setup,
                message: format!("Required tools could not be installed: {}; inspect Processes before starting work", failures.join("; ")),
            })
        }
    }
    .await;
    report.info.preparation = Some(match preparation {
        Ok(preparation) => preparation,
        Err(error) => {
            let message = format!(
                "The workspace moved, but preparation could not be confirmed: {error:#}; inspect Processes on the destination"
            );
            report.line(message.clone());
            TransferPreparation::NeedsAttention {
                scripts: Vec::new(),
                message,
            }
        }
    });
    report.save();
    // Finishing a transfer committed earlier: where it is now, as the destination says.
    let restored = match restored {
        Some(restored) => restored,
        None => {
            let moved: WorkspaceInfo = destination
                .call(ClientCall::WorkspaceGet(WorkspaceParams {
                    workspace: info.id.clone(),
                }))
                .await?;
            CloudTransferRestored {
                receipt_digest: String::new(),
                workspace_path: moved.path,
                kept_copy: None,
                returned: activated.returned,
            }
        }
    };
    Ok(Some(restored))
}

/// The `Stopping` state a `Stopped` one followed: the source's fence, which its capture names.
fn stopping_before(stopped: &Value) -> anyhow::Result<Value> {
    let mut stopping = stopped.clone();
    stopping["revision"] = (number(stopped, "revision")? - 1).into();
    stopping["transfer"]["phase"] = "stopping".into();
    stopping["transfer"]["source_stop_digest"] = Value::Null;
    Ok(stopping)
}

/// The `SnapshotReady` state a `Restored` one followed: the one the snapshot was published under.
fn published_before(restored: &Value) -> anyhow::Result<Value> {
    let mut published = restored.clone();
    published["revision"] = (number(restored, "revision")? - 1).into();
    published["transfer"]["phase"] = "snapshot_ready".into();
    published["transfer"]["destination_receipt_digest"] = Value::Null;
    Ok(published)
}

/// Waits for the setup scripts the destination started and says how each ended; a failed one
/// leaves the workspace moved, with its output to read there.
async fn setup_outcome(
    destination: &mut Endpoint,
    workspace: &str,
    names: &[String],
    report: &mut Report,
) -> anyhow::Result<TransferPreparation> {
    let deadline = tokio::time::Instant::now() + SETUP_WAIT;
    loop {
        let result: ProcessesResult = match tokio::time::timeout_at(deadline, destination
            .call(ClientCall::ProcessList(WorkspaceParams {
                workspace: workspace.to_owned(),
            }))).await {
            Ok(result) => result?,
            Err(_) => return Ok(TransferPreparation::NeedsAttention {
                scripts: names.to_vec(),
                message: "Setup has not finished after 15 minutes; inspect or stop its processes on the destination".into(),
            }),
        };
        let setup: Vec<_> = result
            .processes
            .iter()
            .filter(|p| p.kind == ProcessKind::Setup && names.contains(&p.name))
            .collect();
        if setup.iter().any(|p| p.running) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            continue;
        }
        let failed: Vec<_> = names
            .iter()
            .filter(|name| {
                !setup
                    .iter()
                    .filter(|p| &p.name == *name)
                    .max_by_key(|p| &p.id)
                    .is_some_and(|p| p.exit_code == Some(0))
            })
            .cloned()
            .collect();
        for process in setup {
            match process.exit_code {
                Some(0) => report.line(format!("  {} succeeded", process.name)),
                code => {
                    let code = code.map_or("no exit code".into(), |c| format!("exit code {c}"));
                    report.line(format!(
                        "  {} failed ({code}); its last lines:",
                        process.name
                    ));
                    for line in &process.tail {
                        report.line(format!("    {line}"));
                    }
                }
            }
        }
        return Ok(if failed.is_empty() {
            TransferPreparation::Ready
        } else {
            TransferPreparation::NeedsAttention {
                message: format!(
                    "Setup failed or its outcome is missing for {}; inspect its log and rerun it on the destination",
                    failed.join(", ")
                ),
                scripts: failed,
            }
        });
    }
}

/// Copies the staged ciphertext block by block through both daemons' archive bridges, each of
/// which checks offsets and the full digest; a destination that already stored it is left as is.
/// Up to `IN_FLIGHT` reads and as many writes are sent ahead of their answers: a daemon runs a
/// connection's archive calls in the order they came, and waiting for each block's round trip
/// left the link idle most of the time.
async fn relay(
    source: &mut Endpoint,
    destination: &mut Endpoint,
    staged: &CloudArchiveBinding,
    received: &CloudArchiveBinding,
    report: &mut Report,
) -> anyhow::Result<()> {
    const IN_FLIGHT: usize = 16;
    let writer = match destination
        .peer(PeerCall::CloudArchiveReceiveBegin(
            CloudArchiveReceiveParams {
                binding: received.clone(),
                allow_new: true,
            },
        ))
        .await?
    {
        CloudArchiveReceiveResult::Stored { .. } => return Ok(()),
        CloudArchiveReceiveResult::New { handle } => handle,
    };
    let reader: CloudArchiveHandle = source
        .peer(PeerCall::CloudArchiveSourceOpen(staged.clone()))
        .await?;
    let total = staged.head.bytes;
    if let Some(progress) = &mut report.info.progress {
        progress.total_bytes = total;
    }
    report.save();
    let mut last_progress = std::time::Instant::now();
    let mut reads = std::collections::VecDeque::new();
    let mut writes = std::collections::VecDeque::new();
    let (mut asked, mut offset) = (0, 0);
    while offset < total {
        while asked < total && reads.len() < IN_FLIGHT {
            let length = BLOCK.min(total - asked);
            reads.push_back(
                source
                    .peer_send(PeerCall::CloudArchiveRead(CloudArchiveReadParams {
                        handle: reader.handle.clone(),
                        offset: asked,
                        length: length as u32,
                    }))
                    .await?,
            );
            asked += length;
        }
        let read = reads
            .pop_front()
            .expect("a read is asked ahead of each write");
        let block: CloudArchiveBlock = serde_json::from_value(source.peer_receive(read).await?)
            .context("unexpected answer from a daemon")?;
        writes.push_back((
            destination
                .peer_send(PeerCall::CloudArchiveWrite(CloudArchiveWriteParams {
                    handle: writer.clone(),
                    offset,
                    bytes: block.bytes,
                }))
                .await?,
            BLOCK.min(total - offset),
        ));
        offset += BLOCK.min(total - offset);
        if writes.len() >= IN_FLIGHT {
            let (write, length) = writes.pop_front().expect("writes are in flight");
            destination.peer_receive(write).await?;
            if let Some(progress) = &mut report.info.progress {
                progress.confirmed_bytes += length;
            }
            if last_progress.elapsed() >= Duration::from_millis(250) {
                report.save();
                last_progress = std::time::Instant::now();
            }
        }
    }
    for (write, length) in writes {
        destination.peer_receive(write).await?;
        if let Some(progress) = &mut report.info.progress {
            progress.confirmed_bytes += length;
        }
    }
    report.save();
    destination
        .peer_value(PeerCall::CloudArchiveCommit(CloudArchiveHandle {
            handle: writer,
        }))
        .await?;
    source
        .peer_value(PeerCall::CloudArchiveClose(reader))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::events::{Event, EventEnvelope};

    /// The source's agents as a test sets them: the chats in a turn, and those interrupted.
    #[derive(Clone, Default)]
    struct Fake {
        turns: Arc<Mutex<Vec<String>>>,
        interrupted: Arc<Mutex<Vec<String>>>,
    }

    impl Fake {
        fn set(&self, turns: &[&str]) {
            *self.turns.lock().unwrap() = turns.iter().map(|t| t.to_string()).collect();
        }
    }

    impl Agents for Fake {
        async fn in_turn(&mut self, _workspace: &str) -> anyhow::Result<Vec<String>> {
            Ok(self.turns.lock().unwrap().clone())
        }

        async fn interrupt(&mut self, chat: &str) -> anyhow::Result<()> {
            self.interrupted.lock().unwrap().push(chat.to_owned());
            Ok(())
        }
    }

    fn daemon() -> (tempfile::TempDir, Arc<Daemon>) {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(
            temp.path().join("home"),
            temp.path().join("workspaces"),
            temp.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(
            &paths.settings,
            "linear_enabled = false\ngithub_org = \"\"\ngithub_sources = false\n",
        )
        .unwrap();
        (temp, Daemon::open(paths).unwrap())
    }

    fn report(daemon: &Arc<Daemon>) -> Report {
        Report {
            daemon: daemon.clone(),
            info: TransferInfo {
                revision: None,
                id: uuid::Uuid::now_v7().to_string(),
                workspace_id: "w1".into(),
                slug: "moving".into(),
                source: "mac".into(),
                destination: "vm".into(),
                phase: TransferPhase::Running,
                log: Vec::new(),
                error: None,
                workspace_path: None,
                kept_copy: None,
                returned: false,
                started_at: chrono::Utc::now().to_rfc3339(),
                finished_at: None,
                from: None,
                to: None,
                keep_local_copy: false,
                journal_owner: None,
                discard_pending: None,
                stage: Some(TransferStage::Preparing),
                working_agents: 0,
                progress: None,
                preparation: None,
                interrupt: false,
                archive: false,
                needs_login: None,
                local_copy_changed: false,
                untouched: false,
            },
            source: TransferEndpoint::Here,
            journaled: false,
        }
    }

    type Settled = anyhow::Result<Option<OwnedSemaphorePermit>>;

    /// Runs `settle` in the background, as a transfer does.
    fn settling(
        daemon: &Arc<Daemon>,
        agents: &Fake,
        asks: watch::Receiver<Ask>,
    ) -> tokio::task::JoinHandle<Settled> {
        let (mut agents, mut asks, mut report) = (agents.clone(), asks, report(daemon));
        report.save();
        let at_once = daemon.transfers.at_once.clone();
        tokio::spawn(
            async move { settle(&mut agents, "w1", &mut asks, &at_once, &mut report).await },
        )
    }

    /// The transfer as the apps read it.
    fn read(daemon: &Daemon) -> TransferInfo {
        status(daemon, "w1").unwrap().unwrap()
    }

    #[tokio::test]
    async fn one_agent_signed_in_on_its_worker_lets_a_refused_transfer_go() {
        let (_temp, daemon) = daemon();
        let mut waiting = report(&daemon).info;
        waiting.phase = TransferPhase::Failed;
        waiting.needs_login = Some(TransferLogin {
            worker: "w1".into(),
            agents: vec![AgentKind::Claude, AgentKind::Codex],
        });
        assert!(resumes_after_login(&waiting, "w1", |agent| agent == AgentKind::Codex));
        assert!(!resumes_after_login(&waiting, "w1", |_| false));
        // Another worker's login is not this one's.
        assert!(!resumes_after_login(&waiting, "w9", |_| true));
        waiting.phase = TransferPhase::Cancelled;
        assert!(!resumes_after_login(&waiting, "w1", |_| true));
    }

    async fn until(what: &str, mut check: impl FnMut() -> bool) {
        for _ in 0..100 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for {what}");
    }

    #[tokio::test]
    async fn a_transfer_waits_for_the_end_of_its_agents_turns_then_takes_a_place() {
        let (_temp, daemon) = daemon();
        let agents = Fake::default();
        agents.set(&["a", "b"]);
        let (_asks, receiver) = watch::channel(Ask::Wait);
        let settled = settling(&daemon, &agents, receiver);
        until("the wait", || read(&daemon).working_agents == 2).await;
        let waiting = read(&daemon);
        assert_eq!(waiting.stage, Some(TransferStage::WaitingForTurns));
        assert_eq!(
            waiting.log,
            ["waiting for 2 agents of the workspace to end their turn"]
        );
        agents.set(&["b"]);
        until("one turn ended", || read(&daemon).working_agents == 1).await;
        assert!(!settled.is_finished());
        agents.set(&[]);
        let place = settled.await.unwrap().unwrap();
        assert!(place.is_some());
        assert_eq!(daemon.transfers.at_once.available_permits(), AT_ONCE - 1);
        assert!(agents.interrupted.lock().unwrap().is_empty());
    }

    /// The transfer ends told to every client, if any.
    fn ended(
        events: &mut tokio::sync::broadcast::Receiver<std::sync::Arc<EventEnvelope>>,
    ) -> Option<TransferInfo> {
        while let Ok(envelope) = events.try_recv() {
            if let Event::Registry {
                event: RegistryEvent::TransferEnded { transfer },
            } = &envelope.event
            {
                return Some(*transfer.clone());
            }
        }
        None
    }

    #[tokio::test]
    async fn every_client_is_told_a_transfer_failed_or_ended_but_not_one_cancelled() {
        let (_temp, daemon) = daemon();
        let mut events = daemon.events.subscribe();
        report(&daemon).finish(Ok(None));
        assert_eq!(ended(&mut events), None);
        report(&daemon).finish(Err(anyhow::anyhow!("vm does not answer")));
        let failed = ended(&mut events).expect("the failure told");
        assert_eq!(failed.phase, TransferPhase::Failed);
        assert_eq!(failed.error.as_deref(), Some("vm does not answer"));
    }

    #[tokio::test]
    async fn a_failure_before_the_controller_prepared_it_is_untouched() {
        let (_temp, daemon) = daemon();
        let mut before = report(&daemon);
        before.finish(Err(anyhow::anyhow!("stray file; nothing was stopped")));
        assert!(before.info.untouched);
        let mut after = report(&daemon);
        after.journaled = true;
        after.finish(Err(anyhow::anyhow!("vm does not answer")));
        assert!(!after.info.untouched);
    }

    #[tokio::test]
    async fn interrupt_and_transfer_interrupts_each_agent_in_a_turn_once() {
        let (_temp, daemon) = daemon();
        let agents = Fake::default();
        agents.set(&["a", "b"]);
        let (asks, receiver) = watch::channel(Ask::Wait);
        let settled = settling(&daemon, &agents, receiver);
        until("the wait", || read(&daemon).working_agents == 2).await;
        asks.send_replace(Ask::Interrupt);
        until("the interruption", || read(&daemon).interrupt).await;
        // Looked at again while the turns wind down: each is interrupted once.
        tokio::time::sleep(TURNS_POLL * 2).await;
        assert_eq!(*agents.interrupted.lock().unwrap(), ["a", "b"]);
        assert_eq!(
            read(&daemon).log.last().unwrap(),
            "interrupting 2 agents of the workspace"
        );
        agents.set(&[]);
        assert!(settled.await.unwrap().unwrap().is_some());
    }

    #[tokio::test]
    async fn a_transfer_cancelled_while_it_waits_ends_without_a_place() {
        let (_temp, daemon) = daemon();
        let agents = Fake::default();
        agents.set(&["a"]);
        let (asks, receiver) = watch::channel(Ask::Wait);
        let settled = settling(&daemon, &agents, receiver);
        until("the wait", || read(&daemon).working_agents == 1).await;
        asks.send_replace(Ask::Cancel);
        assert!(settled.await.unwrap().unwrap().is_none());
        assert_eq!(daemon.transfers.at_once.available_permits(), AT_ONCE);
        assert!(agents.interrupted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_turn_started_while_queued_sends_the_transfer_back_to_the_wait() {
        let (_temp, daemon) = daemon();
        let taken = daemon
            .transfers
            .at_once
            .clone()
            .acquire_many_owned(AT_ONCE as u32)
            .await
            .unwrap();
        let agents = Fake::default();
        let (_asks, receiver) = watch::channel(Ask::Wait);
        let settled = settling(&daemon, &agents, receiver);
        until("the queue", || {
            read(&daemon).stage == Some(TransferStage::Queued)
        })
        .await;
        agents.set(&["a"]);
        drop(taken);
        until("the wait again", || {
            read(&daemon).stage == Some(TransferStage::WaitingForTurns)
        })
        .await;
        // The place it took meanwhile is given back while it waits.
        assert_eq!(daemon.transfers.at_once.available_permits(), AT_ONCE);
        agents.set(&[]);
        assert!(settled.await.unwrap().unwrap().is_some());
    }

    #[test]
    fn a_cancellation_is_heard_only_before_the_transfer_stops_its_source() {
        let (_temp, daemon) = daemon();
        let transfers = &daemon.transfers;
        assert!(!transfers.ask("w1", Ask::Cancel), "no transfer waits");
        let (sender, receiver) = watch::channel(Ask::Wait);
        transfers.asks.lock().unwrap().insert("w1".into(), sender);
        assert!(transfers.ask("w1", Ask::Cancel));
        // Cancelled, it is not interrupted instead.
        assert!(transfers.ask("w1", Ask::Interrupt));
        assert_eq!(*receiver.borrow(), Ask::Cancel);
        assert!(transfers.leave_waiting("w1"));
        // Past that point, nothing more is asked of it.
        assert!(!transfers.ask("w1", Ask::Cancel));
        assert!(!transfers.leave_waiting("w1"));
    }

    #[tokio::test]
    async fn only_a_return_asked_to_archive_that_came_back_archives_its_workspace() {
        let (_temp, daemon) = daemon();
        let mut report = report(&daemon);
        let archived = |report: &mut Report| {
            report.info.log.clear();
            archive_back(report);
            report.info.log.last().cloned()
        };
        report.info.phase = TransferPhase::Done;
        report.info.returned = true;
        assert_eq!(archived(&mut report), None, "a return not asked to archive");
        report.info.archive = true;
        for phase in [TransferPhase::Failed, TransferPhase::Cancelled] {
            report.info.phase = phase;
            assert_eq!(archived(&mut report), None, "{phase:?}");
        }
        report.info.phase = TransferPhase::Done;
        report.info.returned = false;
        assert_eq!(archived(&mut report), None, "a move away");
        // Back: it archives it here, and says what failed (this daemon has no such workspace).
        report.info.returned = true;
        let said = archived(&mut report).expect("the archive said");
        assert!(said.starts_with("could not archive it here: "), "{said}");
    }
}
