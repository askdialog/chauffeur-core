//! Piloting a moved workspace's chats from here (docs/workspace-relay.md, decided with Justin
//! 2026-10-26): a call about the chats of a workspace that runs on a machine of
//! `[transfer]` is sent to that machine's daemon, over its socket, and its answer returned as
//! that daemon gave it. Nothing local is bypassed: a call the relay does not take reaches the
//! local code, which still refuses a moved workspace. The other daemon knows the workspace and
//! its chats by the same ids.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use chauffeur_protocol::MIN_PROTOCOL_VERSION;
use chauffeur_protocol::access;
use chauffeur_protocol::client::{
    ClientCall, ClientMethod, EventNotification, HelloParams, HelloResult, SubscribeParams,
    SubscribeResult, UnsubscribeParams,
};
use chauffeur_protocol::events::EventEnvelope;
use chauffeur_protocol::methods::Method;
use chauffeur_protocol::model::Activity;
use chauffeur_protocol::rpc::{Incoming, Request, RpcError};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::{Daemon, Error, Result};

/// Where a call about a moved workspace goes.
#[derive(Debug)]
pub struct Target {
    pub machine: Machine,
    pub workspace: String,
    /// The user acts on the workspace: not a read a client makes on its own.
    pub acts: bool,
}

/// The connections to other machines' daemons, and the chats learnt to live on one.
#[derive(Default)]
pub struct Relay {
    daemons: Mutex<HashMap<PathBuf, Arc<RemoteDaemon>>>,
    /// Chats this daemon has no row of (created there), by id: their workspace.
    chats: Mutex<HashMap<String, String>>,
    /// Terminals opened there, by id: their workspace.
    terminals: Mutex<HashMap<String, String>>,
    /// Scripts run there, by process id: their workspace.
    processes: Mutex<HashMap<String, String>>,
    /// What the machine a moved workspace runs on says of its chats, by workspace id
    /// (`relay_events::watch`): shown instead of this daemon's own, which no longer change.
    remote: Mutex<HashMap<String, RemoteState>>,
}

/// A moved workspace's chats as the machine it runs on sees them.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteState {
    pub activity: Activity,
    pub unread: bool,
    pub running_tasks: u32,
}

/// A machine a moved workspace runs on: its name in `[transfer]` or a GCP worker's, and its
/// daemon's socket.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Machine {
    pub name: String,
    pub socket: PathBuf,
}

/// The machine `workspace` runs on, when it moved to a machine of `[transfer]` or a ready GCP
/// worker; a workspace moved by another daemon, or to a socket only the CLI named, is not relayed.
pub fn machine_of(daemon: &Daemon, workspace: &str) -> Option<Machine> {
    let name = crate::cloud_transfer_driver::moved_to(daemon, workspace)?;
    // A ready GCP worker is reached on its forwarded socket, as a declared machine on its own.
    let socket = crate::workers::machine_socket(daemon, &name).ok()?;
    Some(Machine { name, socket })
}

/// What a call is about, as the client named it.
#[derive(Debug, PartialEq)]
enum Subject<'a> {
    Workspace(&'a str),
    Chat(&'a str),
    Terminal(&'a str),
    Process(&'a str),
}

fn subject(call: &Value) -> Option<(&str, Subject<'_>)> {
    let method = call["method"].as_str()?;
    if ClientMethod::from_name(method)?.relay() == access::Relay::Local {
        return None;
    }
    let params = &call["params"];
    let subject = if let Some(workspace) = params["workspace"].as_str() {
        Subject::Workspace(workspace)
    } else if let Some(chat) = params["chat"].as_str() {
        Subject::Chat(chat)
    } else if let Some(process) = params["process"].as_str() {
        Subject::Process(process)
    } else {
        Subject::Terminal(params["terminal"].as_str()?)
    };
    Some((method, subject))
}

impl Relay {
    /// The machine to send `call` to, if it is about a moved workspace's chats.
    pub async fn target(&self, daemon: &Daemon, call: &ClientCall) -> Result<Option<Target>> {
        let value = serde_json::to_value(call)?;
        let Some((method, subject)) = subject(&value) else {
            return Ok(None);
        };
        let workspace = match subject {
            // `workspace.wait` also takes a chat's id (`chauffeur wait|status <chat>`).
            Subject::Workspace(key) if method == "workspace.wait" => {
                self.workspace_of_chat(daemon, key).await?
            }
            Subject::Workspace(key) => daemon.store.find_workspace(key)?.map(|row| row.id),
            Subject::Chat(chat) => self.workspace_of_chat(daemon, chat).await?,
            Subject::Terminal(terminal) => self.workspace_of_terminal(daemon, terminal).await,
            Subject::Process(process) => self.workspace_of_process(daemon, process).await,
        };
        let Some(workspace) = workspace else {
            return Ok(None);
        };
        let Some(machine) = machine_of(daemon, &workspace) else {
            return Ok(None);
        };
        let relay = call.method().relay();
        if relay == access::Relay::Here {
            return Err(Error::Conflict(format!(
                "This workspace runs on {}: its files are there, not on this machine",
                machine.name
            )));
        }
        // Design chats work in the app's own windows, against this machine's files.
        if method == "chat.create" && value["params"]["kind"].as_str() == Some("design") {
            return Err(Error::Conflict(format!(
                "Design chats of a workspace that runs on {} are not available here yet",
                machine.name
            )));
        }
        Ok(Some(Target {
            machine,
            workspace,
            acts: relay == access::Relay::Act,
        }))
    }

    /// The machine a subscription naming moved workspaces or their chats is relayed to, with
    /// those workspaces' ids; none when it names none. One mixing them with others, or asking
    /// for more (`all`, the registry), is refused: the app subscribes to one workspace at a time.
    pub async fn subscription_target(
        &self,
        daemon: &Daemon,
        params: &SubscribeParams,
    ) -> Result<Option<(Machine, Vec<String>)>> {
        let mut workspaces = Vec::new();
        for key in &params.workspaces {
            workspaces.push(daemon.store.find_workspace(key)?.map(|row| row.id));
        }
        for chat in &params.chats {
            workspaces.push(self.workspace_of_chat(daemon, chat).await?);
        }
        let machines: Vec<Option<Machine>> = workspaces
            .iter()
            .map(|w| w.as_deref().and_then(|w| machine_of(daemon, w)))
            .collect();
        let Some(machine) = machines.iter().flatten().next().cloned() else {
            return Ok(None);
        };
        if params.all
            || params.registry
            || params.office
            || machines.iter().any(|m| m.as_ref() != Some(&machine))
        {
            return Err(Error::BadRequest(format!(
                "A workspace that runs on {} is followed on its own: subscribe to it alone",
                machine.name
            )));
        }
        let mut ids: Vec<String> = workspaces.into_iter().flatten().collect();
        ids.dedup();
        Ok(Some((machine, ids)))
    }

    /// Opens a subscription on `machine`: its events come on the receiver, which ends when the
    /// connection does. Dropping the [`RemoteSubscription`] ends it there.
    pub(crate) async fn subscribe(
        &self,
        machine: &Machine,
        params: SubscribeParams,
    ) -> Result<RemoteSubscription> {
        let remote = self
            .connection(machine)
            .await
            .map_err(|e| Error::Unavailable(format!("{} does not answer ({e:#})", machine.name)))?;
        remote
            .subscribe(params)
            .await
            .map_err(|failure| match failure {
                Failure::Refused(error) => Error::Remote(error),
                Failure::Gone(cause) => {
                    self.daemons.lock().unwrap().remove(&machine.socket);
                    Error::Unavailable(format!("{} does not answer ({cause})", machine.name))
                }
            })
    }

    /// A file the app shows of a moved workspace (an image of a chat, a video of its files), read
    /// from `machine`'s daemon over its socket: `route` is the same route there, whose Unix side
    /// asks no token; `query` its parameters; a byte range passed on.
    pub async fn http_get(
        &self,
        machine: &Machine,
        route: &str,
        query: &[(&str, &str)],
        range: Option<&str>,
    ) -> anyhow::Result<reqwest::Response> {
        let client = reqwest::Client::builder()
            .unix_socket(machine.socket.clone())
            .build()?;
        let mut url = reqwest::Url::parse("http://localhost/")?;
        url.set_path(route);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        let mut request = client.get(url);
        if let Some(range) = range {
            request = request.header(reqwest::header::RANGE, range);
        }
        Ok(request.send().await?)
    }

    /// Calls `machine` without the chats' error wording, for this daemon's own use.
    pub(crate) async fn call(&self, machine: &Machine, call: ClientCall) -> Result<Value> {
        self.send(machine, &call).await
    }

    pub fn remote_state(&self, workspace: &str) -> Option<RemoteState> {
        self.remote.lock().unwrap().get(workspace).cloned()
    }

    /// Keeps what `machine` says of a moved workspace; true when it changed.
    pub(crate) fn set_remote_state(&self, workspace: &str, state: Option<RemoteState>) -> bool {
        let mut remote = self.remote.lock().unwrap();
        match state {
            Some(state) => remote.insert(workspace.to_owned(), state.clone()) != Some(state),
            None => remote.remove(workspace).is_some(),
        }
    }

    pub(crate) fn remote_workspaces(&self) -> Vec<String> {
        self.remote.lock().unwrap().keys().cloned().collect()
    }

    /// A chat the machine named, of a workspace that runs there.
    pub(crate) fn learn_chat(&self, chat: &str, workspace: &str) {
        self.chats
            .lock()
            .unwrap()
            .insert(chat.to_owned(), workspace.to_owned());
    }

    /// The workspace of `process`: this daemon's own script's, else one learnt from a machine,
    /// else asked of every machine a workspace moved to.
    async fn workspace_of_process(&self, daemon: &Daemon, process: &str) -> Option<String> {
        if let Some(info) = daemon.procs.get(process) {
            return Some(info.workspace_id);
        }
        if let Some(workspace) = self.processes.lock().unwrap().get(process).cloned() {
            return Some(workspace);
        }
        for row in daemon.store.list_workspaces().ok()? {
            let Some(machine) = machine_of(daemon, &row.id) else {
                continue;
            };
            let listed = ClientCall::ProcessList(chauffeur_protocol::client::WorkspaceParams {
                workspace: row.id.clone(),
            });
            if let Ok(value) = self.send(&machine, &listed).await {
                self.learn(&serde_json::json!({"method": "process.list"}), &value);
            }
        }
        self.processes.lock().unwrap().get(process).cloned()
    }

    /// The workspace of `terminal`: this daemon's own terminal's, else one learnt from a machine,
    /// else asked of every machine a workspace moved to.
    async fn workspace_of_terminal(&self, daemon: &Daemon, terminal: &str) -> Option<String> {
        if let Some(workspace) = daemon.terminals.workspace_of(terminal) {
            return Some(workspace);
        }
        if let Some(workspace) = self.terminals.lock().unwrap().get(terminal).cloned() {
            return Some(workspace);
        }
        for row in daemon.store.list_workspaces().ok()? {
            let Some(machine) = machine_of(daemon, &row.id) else {
                continue;
            };
            let listed = ClientCall::TerminalList(chauffeur_protocol::client::WorkspaceParams {
                workspace: row.id.clone(),
            });
            if let Ok(value) = self.send(&machine, &listed).await {
                self.learn(&serde_json::json!({"method": "terminal.list"}), &value);
            }
        }
        self.terminals.lock().unwrap().get(terminal).cloned()
    }

    /// The workspace of `chat`: this daemon's row of it, else one learnt from a machine, else
    /// asked of every machine a workspace moved to.
    async fn workspace_of_chat(&self, daemon: &Daemon, chat: &str) -> Result<Option<String>> {
        if let Some(info) = daemon.store.chat(chat)? {
            return Ok(Some(info.workspace_id));
        }
        // A chat named by its workspace (`chauffeur send <workspace>`): that workspace's.
        if let Some(row) = daemon.store.find_workspace(chat)? {
            return Ok(Some(row.id));
        }
        if let Some(workspace) = self.chats.lock().unwrap().get(chat).cloned() {
            return Ok(Some(workspace));
        }
        for row in daemon.store.list_workspaces()? {
            let Some(machine) = machine_of(daemon, &row.id) else {
                continue;
            };
            let listed = ClientCall::ChatList(chauffeur_protocol::client::ChatListParams {
                workspace: row.id.clone(),
                all: true,
            });
            if let Ok(value) = self.send(&machine, &listed).await {
                self.learn(&serde_json::json!({"method": "chat.list"}), &value);
            }
        }
        Ok(self.chats.lock().unwrap().get(chat).cloned())
    }

    /// Sends `call` to `machine` and returns its answer as that daemon gave it.
    pub async fn forward(
        &self,
        daemon: &Daemon,
        machine: &Machine,
        call: ClientCall,
    ) -> Result<Value> {
        let call = inline_files(call)?;
        fits_one_message(&call)?;
        let mut value = self.send(machine, &call).await?;
        let call = serde_json::to_value(&call)?;
        self.learn(&call, &value);
        // An HTML page's rendering is served on that machine's loopback, which this one cannot
        // reach: this daemon's preview listener serves it instead, reading it there.
        if (call["method"] == "files.read" || call["method"] == "files.write")
            && let Some(content) = value.as_object_mut()
            && !content.get("preview_url").is_none_or(Value::is_null)
        {
            let workspace = call["params"]["workspace"].as_str().unwrap_or_default();
            let here = daemon
                .store
                .find_workspace(workspace)?
                .zip(content.get("path").and_then(Value::as_str))
                .and_then(|(row, path)| daemon.designs.file_url(&row.id, path));
            content.insert("preview_url".into(), here.map_or(Value::Null, Value::from));
        }
        if call["method"] == "previews.list" {
            let workspace = call["params"]["workspace"].as_str().unwrap_or_default();
            let block = match daemon.store.find_workspace(workspace)? {
                Some(row) => daemon.store.port_block(&row.id)?,
                None => None,
            };
            value = serde_json::to_value(reached_here(
                serde_json::from_value(value)?,
                machine,
                block.as_ref(),
            ))?;
        }
        Ok(value)
    }

    async fn send(&self, machine: &Machine, call: &ClientCall) -> Result<Value> {
        let gone = |cause: String| {
            Error::Unavailable(format!(
                "{} does not answer ({cause}); its chats are back when it does",
                machine.name
            ))
        };
        let remote = self
            .connection(machine)
            .await
            .map_err(|e| gone(format!("{e:#}")))?;
        match remote.call(call.clone()).await {
            Ok(value) => Ok(value),
            Err(Failure::Refused(error)) => Err(Error::Remote(error)),
            Err(Failure::Gone(cause)) => {
                self.daemons.lock().unwrap().remove(&machine.socket);
                Err(gone(cause))
            }
        }
    }

    async fn connection(&self, machine: &Machine) -> anyhow::Result<Arc<RemoteDaemon>> {
        if let Some(open) = self.daemons.lock().unwrap().get(&machine.socket).cloned()
            && !open.closed.load(Ordering::SeqCst)
        {
            return Ok(open);
        }
        let opened = RemoteDaemon::connect(&machine.socket).await?;
        self.daemons
            .lock()
            .unwrap()
            .insert(machine.socket.clone(), opened.clone());
        Ok(opened)
    }

    /// Keeps which workspace the chats an answer names belong to, for later calls naming them.
    fn learn(&self, call: &Value, answer: &Value) {
        let terminal = |terminal: &Value| {
            if let (Some(id), Some(workspace)) =
                (terminal["id"].as_str(), terminal["workspace_id"].as_str())
            {
                self.terminals
                    .lock()
                    .unwrap()
                    .insert(id.to_owned(), workspace.to_owned());
            }
        };
        let process = |process: &Value| {
            if let (Some(id), Some(workspace)) =
                (process["id"].as_str(), process["workspace_id"].as_str())
            {
                self.processes
                    .lock()
                    .unwrap()
                    .insert(id.to_owned(), workspace.to_owned());
            }
        };
        match call["method"].as_str() {
            Some("process.list") => answer["processes"]
                .as_array()
                .into_iter()
                .flatten()
                .for_each(process),
            Some("process.run") => process(answer),
            _ => {}
        }
        match call["method"].as_str() {
            Some("terminal.list") => answer["terminals"]
                .as_array()
                .into_iter()
                .flatten()
                .for_each(terminal),
            Some("terminal.open") => terminal(answer),
            _ => {}
        }
        let mut chats = self.chats.lock().unwrap();
        let mut keep = |chat: &Value| {
            if let (Some(id), Some(workspace)) =
                (chat["id"].as_str(), chat["workspace_id"].as_str())
            {
                chats.insert(id.to_owned(), workspace.to_owned());
            }
        };
        match call["method"].as_str() {
            Some("chat.list") => answer["chats"]
                .as_array()
                .into_iter()
                .flatten()
                .for_each(keep),
            Some("chat.create" | "chat.fork") => keep(answer),
            _ => {}
        }
    }
}

/// Room left in a message for the request around the call.
const ENVELOPE_BYTES: usize = 4 << 10;

/// Refuses a call the other daemon would not take in one message: its connection would close.
/// A moved workspace's previews as `machine` lists them, opened here: each on the port of this
/// machine's block that `relay_ports` passes to the same place of its block there. One whose place
/// this block lacks cannot be reached from here and is left out.
fn reached_here(
    there: chauffeur_protocol::dev_services::Previews,
    machine: &Machine,
    block: Option<&crate::store::PortBlock>,
) -> chauffeur_protocol::dev_services::Previews {
    chauffeur_protocol::dev_services::Previews {
        machine: Some(machine.name.clone()),
        previews: there
            .previews
            .into_iter()
            .filter_map(|mut preview| {
                let block = block.filter(|b| preview.offset < b.size)?;
                preview.port = block.base.checked_add(preview.offset)?;
                Some(preview)
            })
            .collect(),
    }
}

fn fits_one_message(call: &ClientCall) -> Result<()> {
    let size = serde_json::to_vec(call)?.len();
    if size + ENVELOPE_BYTES > crate::server::MAX_MESSAGE_BYTES {
        return Err(Error::BadRequest(format!(
            "this message is {} MB with its files; at most {} MB goes to another machine at once",
            size >> 20,
            (crate::server::MAX_MESSAGE_BYTES - ENVELOPE_BYTES) >> 20
        )));
    }
    Ok(())
}

/// A file a local client named by its path (the CLI) is read here: the other machine cannot.
fn inline_files(call: ClientCall) -> Result<ClientCall> {
    let mut value = serde_json::to_value(&call)?;
    let Some(images) = value["params"]["images"].as_array_mut() else {
        return Ok(call);
    };
    for image in images {
        let Some(path) = image["path"].as_str().map(PathBuf::from) else {
            continue;
        };
        if image["data"].is_string() {
            continue;
        }
        let size = std::fs::metadata(&path)
            .map_err(|e| Error::BadRequest(format!("{}: {e}", path.display())))?
            .len();
        if size > crate::attachments::MAX_BYTES as u64 {
            return Err(Error::BadRequest(format!(
                "{} is over {} MB, the most a message to another machine carries",
                path.display(),
                crate::attachments::MAX_BYTES >> 20
            )));
        }
        let bytes = std::fs::read(&path)?;
        image["data"] = base64::engine::general_purpose::STANDARD
            .encode(bytes)
            .into();
        if image["name"].is_null()
            && let Some(name) = path.file_name()
        {
            image["name"] = name.to_string_lossy().into_owned().into();
        }
        if let Some(object) = image.as_object_mut() {
            object.remove("path");
        }
    }
    serde_json::from_value(value).map_err(|e| Error::BadRequest(e.to_string()))
}

/// Why a call to another daemon has no answer.
enum Failure {
    /// It answered with an error, to pass on as it is.
    Refused(RpcError),
    /// It did not answer: the connection is gone.
    Gone(String),
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, RpcError>>>>>;

/// Where the events of each subscription opened on another daemon go, by its id there.
#[derive(Default)]
struct Routes {
    live: HashMap<String, mpsc::UnboundedSender<EventEnvelope>>,
    /// Sent before the answer naming their subscription came: handed over when it does.
    early: HashMap<String, Vec<EventEnvelope>>,
}

/// Another daemon over its Unix socket, several calls at a time.
struct RemoteDaemon {
    outgoing: mpsc::UnboundedSender<Message>,
    pending: Pending,
    routes: Arc<Mutex<Routes>>,
    next: AtomicU64,
    closed: Arc<AtomicBool>,
}

/// A subscription open on another daemon: its events, until that connection closes.
pub(crate) struct RemoteSubscription {
    pub events: mpsc::UnboundedReceiver<EventEnvelope>,
    /// That daemon's last stored event when it started, in its own numbering.
    pub last_seq: i64,
    id: String,
    remote: Arc<RemoteDaemon>,
}

impl Drop for RemoteSubscription {
    fn drop(&mut self) {
        self.remote.unsubscribe(&self.id);
    }
}

impl RemoteDaemon {
    async fn connect(path: &Path) -> anyhow::Result<Arc<RemoteDaemon>> {
        let stream = UnixStream::connect(path).await?;
        // A chat's history with its files is as large as a client's message may be.
        let config = WebSocketConfig::default()
            .max_message_size(Some(crate::server::MAX_MESSAGE_BYTES))
            .max_frame_size(Some(crate::server::MAX_MESSAGE_BYTES));
        let (socket, _) =
            tokio_tungstenite::client_async_with_config("ws://localhost/ws", stream, Some(config))
                .await?;
        let (mut sink, mut stream) = socket.split();
        let (outgoing, mut queue) = mpsc::unbounded_channel::<Message>();
        let pending: Pending = Arc::default();
        let routes: Arc<Mutex<Routes>> = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        tokio::spawn(async move {
            while let Some(message) = queue.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });
        {
            let pending = pending.clone();
            let routes = routes.clone();
            let closed = closed.clone();
            tokio::spawn(async move {
                while let Some(Ok(message)) = stream.next().await {
                    let text = match message {
                        Message::Text(text) => text.to_string(),
                        Message::Binary(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                        Message::Close(_) => break,
                        _ => continue,
                    };
                    let Ok(incoming) = serde_json::from_str::<Incoming>(&text) else {
                        continue;
                    };
                    if incoming.method.as_deref() == Some("event") {
                        if let Some(event) = incoming
                            .params
                            .and_then(|p| serde_json::from_value::<EventNotification>(p).ok())
                        {
                            route(&routes, event);
                        }
                        continue;
                    }
                    if !incoming.is_response() {
                        continue;
                    }
                    let Some(id) = incoming.id.as_ref().and_then(Value::as_u64) else {
                        continue;
                    };
                    if let Some(waiting) = pending.lock().unwrap().remove(&id) {
                        let _ = waiting.send(match incoming.error {
                            Some(error) => Err(error),
                            None => Ok(incoming.result.unwrap_or(Value::Null)),
                        });
                    }
                }
                closed.store(true, Ordering::SeqCst);
                // Calls still waiting learn the connection is gone, subscriptions that it ended.
                pending.lock().unwrap().clear();
                *routes.lock().unwrap() = Routes::default();
            });
        }
        let remote = Arc::new(RemoteDaemon {
            outgoing,
            pending,
            routes,
            next: AtomicU64::new(1),
            closed,
        });
        let hello = remote
            .call(ClientCall::Hello(HelloParams {
                // A worker keeps the version it was installed with: the calls relayed are the
                // user's clients', whichever protocol they speak.
                protocol_version: MIN_PROTOCOL_VERSION,
                client: format!("chauffeurd {} relay", env!("CARGO_PKG_VERSION")),
            }))
            .await
            .map_err(|failure| match failure {
                Failure::Refused(error) => anyhow::anyhow!(error.message),
                Failure::Gone(cause) => anyhow::anyhow!(cause),
            })?;
        let _: HelloResult = serde_json::from_value(hello)?;
        Ok(remote)
    }

    async fn subscribe(
        self: &Arc<Self>,
        params: SubscribeParams,
    ) -> std::result::Result<RemoteSubscription, Failure> {
        let answer = self.call(ClientCall::EventsSubscribe(params)).await?;
        let started: SubscribeResult =
            serde_json::from_value(answer).map_err(|e| Failure::Gone(e.to_string()))?;
        let (sender, events) = mpsc::unbounded_channel();
        {
            let mut routes = self.routes.lock().unwrap();
            for envelope in routes
                .early
                .remove(&started.subscription)
                .unwrap_or_default()
            {
                let _ = sender.send(envelope);
            }
            // Closed meanwhile: the sender dropped here ends the receiver at once.
            if !self.closed.load(Ordering::SeqCst) {
                routes.live.insert(started.subscription.clone(), sender);
            }
        }
        Ok(RemoteSubscription {
            events,
            last_seq: started.last_seq,
            id: started.subscription,
            remote: self.clone(),
        })
    }

    /// Ends a subscription there, without waiting for the answer.
    fn unsubscribe(&self, subscription: &str) {
        let mut routes = self.routes.lock().unwrap();
        routes.live.remove(subscription);
        routes.early.remove(subscription);
        drop(routes);
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let call = ClientCall::EventsUnsubscribe(UnsubscribeParams {
            subscription: subscription.to_owned(),
        });
        if let Ok(text) = serde_json::to_string(&Request::new(id, call)) {
            let _ = self.outgoing.send(Message::Text(text.into()));
        }
    }

    async fn call(&self, call: ClientCall) -> std::result::Result<Value, Failure> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (answer, waiting) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, answer);
        let text = serde_json::to_string(&Request::new(id, call))
            .map_err(|e| Failure::Gone(e.to_string()))?;
        if self.closed.load(Ordering::SeqCst)
            || self.outgoing.send(Message::Text(text.into())).is_err()
        {
            self.pending.lock().unwrap().remove(&id);
            return Err(Failure::Gone("the connection closed".into()));
        }
        match waiting.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(Failure::Refused(error)),
            Err(_) => Err(Failure::Gone("the connection closed".into())),
        }
    }
}

/// Passes an event to its subscription, or keeps it until the subscription's answer came.
fn route(routes: &Mutex<Routes>, event: EventNotification) {
    /// More than a replay sends before its answer is read: the rest is dropped.
    const EARLY: usize = 100_000;
    let mut routes = routes.lock().unwrap();
    if let Some(sender) = routes.live.get(&event.subscription) {
        if sender.send(event.envelope).is_err() {
            routes.live.remove(&event.subscription);
        }
        return;
    }
    let early = routes.early.entry(event.subscription).or_default();
    if early.len() < EARLY {
        early.push(event.envelope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moved_workspaces_previews_open_on_the_ports_here_that_reach_them() {
        use chauffeur_protocol::dev_services::{Preview, PreviewState, Previews};
        let preview = |name: &str, offset: u16| Preview {
            name: name.into(),
            offset,
            port: 47300 + offset,
            state: PreviewState::Ready,
        };
        let there = Previews {
            machine: None,
            previews: vec![preview("web", 1), preview("docs", 12)],
        };
        let machine = Machine {
            name: "vm".into(),
            socket: "/vm.sock".into(),
        };
        let block = crate::store::PortBlock {
            owner: "w".into(),
            workspace_id: "w".into(),
            base: 41000,
            size: 10,
        };
        let here = reached_here(there.clone(), &machine, Some(&block));
        assert_eq!(here.machine.as_deref(), Some("vm"));
        assert_eq!(
            here.previews
                .iter()
                .map(|p| (p.name.as_str(), p.port))
                .collect::<Vec<_>>(),
            [("web", 41001)]
        );
        assert!(reached_here(there, &machine, None).previews.is_empty());
    }

    #[test]
    fn a_call_about_a_workspace_its_chat_or_its_terminal_is_relayed() {
        let call =
            |method: &str, params: Value| serde_json::json!({"method": method, "params": params});
        let send = call("chat.send", serde_json::json!({"chat": "c1", "text": "hi"}));
        assert_eq!(subject(&send), Some(("chat.send", Subject::Chat("c1"))));
        let run = call(
            "process.run",
            serde_json::json!({"workspace": "w1", "repo": "alpha"}),
        );
        assert_eq!(
            subject(&run),
            Some(("process.run", Subject::Workspace("w1")))
        );
        let input = call(
            "terminal.input",
            serde_json::json!({"terminal": "t1", "data": "ls\n"}),
        );
        assert_eq!(
            subject(&input),
            Some(("terminal.input", Subject::Terminal("t1")))
        );
        let stop = call("process.stop", serde_json::json!({"process": "p1"}));
        assert_eq!(
            subject(&stop),
            Some(("process.stop", Subject::Process("p1")))
        );
        // Archiving the workspace stays here (and is refused).
        let archive = call("workspace.archive", serde_json::json!({"workspace": "w1"}));
        assert_eq!(subject(&archive), None);
        // The models of this machine, unless a workspace is named.
        assert_eq!(subject(&call("models.list", serde_json::json!({}))), None);
        let models = call("models.list", serde_json::json!({"workspace": "w1"}));
        assert_eq!(
            subject(&models),
            Some(("models.list", Subject::Workspace("w1")))
        );
        // Opening one of its files with an app of this machine: taken, to be refused.
        let open = call(
            "files.open",
            serde_json::json!({"workspace": "w1", "path": "a"}),
        );
        assert_eq!(
            subject(&open),
            Some(("files.open", Subject::Workspace("w1")))
        );
    }

    #[test]
    fn a_file_named_by_path_travels_as_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, b"hello").unwrap();
        let call: ClientCall = serde_json::from_value(serde_json::json!({
            "method": "chat.send",
            "params": {"chat": "c1", "text": "see", "images": [{"path": path}]}
        }))
        .unwrap();
        let sent = serde_json::to_value(inline_files(call).unwrap()).unwrap();
        let image = &sent["params"]["images"][0];
        assert_eq!(image["data"], "aGVsbG8=");
        assert_eq!(image["name"], "note.txt");
        assert!(image.get("path").is_none());
    }

    #[test]
    fn a_message_over_one_connection_message_is_refused_before_it_leaves() {
        let call = |bytes: usize| -> ClientCall {
            serde_json::from_value(serde_json::json!({
                "method": "chat.send",
                "params": {"chat": "c1", "text": "x".repeat(bytes)}
            }))
            .unwrap()
        };
        assert!(fits_one_message(&call(1 << 20)).is_ok());
        let error = fits_one_message(&call(crate::server::MAX_MESSAGE_BYTES)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("goes to another machine at once"),
            "{error}"
        );
    }
}
