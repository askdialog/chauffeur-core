//! The agent sidecar process: spawned on first use, spoken to over stdio JSON-RPC, restarted
//! after a crash (sessions are then resumed lazily by the chat manager).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::Context;
use chauffeur_protocol::rpc::{Incoming, Request, Response, RpcError, codes};
use chauffeur_protocol::sidecar::{
    Empty, SetEnvParams, SidecarCall, SidecarToDaemon, WorktreeCreateResult,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};

use crate::{Daemon, Error, Result};

type Pending =
    std::sync::Mutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, RpcError>>>>;

pub struct Sidecar {
    daemon: Weak<Daemon>,
    process: Mutex<Option<Arc<Process>>>,
    /// The program run instead of the sidecar found on disk: a stand-in, for tests.
    binary: std::sync::Mutex<Option<PathBuf>>,
    /// Every sidecar this daemon started, dead ones included: an agent process may outlive its
    /// sidecar, and is told from the user's by descending from one (`access::is_agent`).
    pids: std::sync::Mutex<Vec<u32>>,
}

struct Process {
    /// The sidecar's pid, which is also its process group's (`process_group(0)`).
    pid: Option<u32>,
    pin: Option<crate::cloud_process::NativeProcessPin>,
    generation: String,
    stdin: Mutex<ChildStdin>,
    pending: Pending,
    next_id: AtomicU64,
    alive: AtomicBool,
    child: Mutex<Child>,
}

impl Sidecar {
    pub fn new(daemon: Weak<Daemon>) -> Sidecar {
        Sidecar {
            daemon,
            process: Mutex::new(None),
            binary: Default::default(),
            pids: Default::default(),
        }
    }

    /// The pids of every sidecar started by this daemon.
    pub fn pids(&self) -> Vec<u32> {
        self.pids.lock().unwrap().clone()
    }

    /// Counts `pid` as a sidecar's from now on: what `spawn` does with its child, and what a
    /// test does with itself to connect as an agent would.
    pub fn adopt(&self, pid: u32) {
        self.pids.lock().unwrap().push(pid);
    }

    /// Runs `program` as the sidecar from now on: tests give a stand-in that starts no agent.
    pub fn use_binary(&self, program: PathBuf) {
        *self.binary.lock().unwrap() = Some(program);
    }

    /// Whether there is a sidecar to run: a stand-in, or one found on disk.
    pub fn available(&self) -> bool {
        self.binary.lock().unwrap().is_some() || sidecar_binary().is_some()
    }

    /// Whether a sidecar process is currently running.
    pub async fn is_running(&self) -> bool {
        self.process
            .lock()
            .await
            .as_ref()
            .is_some_and(|p| p.alive.load(Ordering::SeqCst))
    }

    async fn ensure(&self) -> Result<Arc<Process>> {
        let mut slot = self.process.lock().await;
        if let Some(process) = slot.as_ref()
            && process.alive.load(Ordering::SeqCst)
        {
            return Ok(process.clone());
        }
        // A sidecar started now would die with the daemon, taking the session it starts along:
        // the next daemon resumes the chat instead.
        if self
            .daemon
            .upgrade()
            .is_none_or(|daemon| daemon.shutdown.is_cancelled())
        {
            return Err(Error::Unavailable(
                "Chauffeur is stopping: try again once it is back".into(),
            ));
        }
        let process = self.spawn().await?;
        *slot = Some(process.clone());
        Ok(process)
    }

    async fn spawn(&self) -> Result<Arc<Process>> {
        let daemon = self.daemon.upgrade().expect("daemon alive");
        let chosen = self.binary.lock().unwrap().clone();
        let binary = chosen.or_else(sidecar_binary).ok_or_else(|| {
            Error::Unavailable(
                "the agent sidecar is not built: run `bun run build` in sidecar/ or set CHAUFFEUR_SIDECAR".into(),
            )
        })?;
        let log_path = daemon.paths.logs_dir.join("sidecar.log");
        let log = crate::paths::open_private_log(&log_path)?;
        let mut command = Command::new(&binary);
        // Never the folder the daemon was started from: a sidecar working in a workspace would
        // count as one of its processes, or as a program writing in it, when it is transferred.
        command
            .current_dir(&daemon.paths.home)
            .env_clear()
            .envs(daemon.shell_env.vars())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            // Its own group: the agents it starts are killed with it (`kill_group`), so that none
            // outlives it under PID 1, where `access::is_agent` could not tell it from the user.
            .process_group(0)
            .kill_on_drop(true);
        let mut child = crate::procs::spawn_when_free(|| command.spawn())
            .await
            .with_context(|| format!("spawning {}", binary.display()))?;
        tracing::info!(binary = %binary.display(), "agent sidecar started");
        if let Some(pid) = child.id() {
            self.adopt(pid);
        }
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let pin = child
            .id()
            .and_then(|pid| crate::cloud_process::inspect(pid).ok().flatten());
        let process = Arc::new(Process {
            pid: child.id(),
            pin,
            generation: uuid::Uuid::now_v7().to_string(),
            stdin: Mutex::new(stdin),
            pending: Default::default(),
            next_id: AtomicU64::new(1),
            alive: AtomicBool::new(true),
            child: Mutex::new(child),
        });
        let reader_process = process.clone();
        let weak = self.daemon.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let incoming: Incoming = match serde_json::from_str(&line) {
                    Ok(incoming) => incoming,
                    Err(error) => {
                        tracing::warn!(%error, "unreadable line from the sidecar: {line}");
                        continue;
                    }
                };
                if incoming.is_response() {
                    let id = incoming.id.as_ref().and_then(Value::as_u64).unwrap_or(0);
                    let waiter = reader_process.pending.lock().unwrap().remove(&id);
                    if let Some(waiter) = waiter {
                        let outcome = match incoming.error {
                            Some(error) => Err(error),
                            None => Ok(incoming.result.unwrap_or(Value::Null)),
                        };
                        let _ = waiter.send(outcome);
                    }
                    continue;
                }
                let Some(daemon) = weak.upgrade() else { break };
                let call: SidecarToDaemon = match serde_json::from_value(incoming.call_value()) {
                    Ok(call) => call,
                    Err(error) => {
                        tracing::warn!(%error, "unknown message from the sidecar: {line}");
                        if let Some(id) = incoming.id {
                            let _ = reader_process
                                .write(&Response::err(
                                    id,
                                    RpcError {
                                        code: codes::METHOD_NOT_FOUND,
                                        message: error.to_string(),
                                        data: None,
                                        failure: None,
                                    },
                                ))
                                .await;
                        }
                        continue;
                    }
                };
                match (call, incoming.id) {
                    (SidecarToDaemon::SessionEvent(params), _) => {
                        daemon.chats.on_event(&params.chat_id, params.event);
                    }
                    (SidecarToDaemon::AgentLoginEnded(params), _) => {
                        daemon.usage.login_ended(&daemon, params);
                    }
                    (call, Some(id)) => {
                        let process = reader_process.clone();
                        tokio::spawn(async move {
                            let response = match handle_hook(&daemon, call).await {
                                Ok(value) => Response::ok(id, value),
                                Err(error) => Response::err(id, error.to_rpc()),
                            };
                            let _ = process.write(&response).await;
                        });
                    }
                    (_, None) => {}
                }
            }
            reader_process.alive.store(false, Ordering::SeqCst);
            if let Some(pid) = reader_process.pid {
                kill_group(pid);
            }
            let waiters: Vec<_> = reader_process.pending.lock().unwrap().drain().collect();
            for (_, waiter) in waiters {
                let _ = waiter.send(Err(RpcError {
                    code: codes::UNAVAILABLE,
                    message: "the agent sidecar exited".into(),
                    data: None,
                    failure: None,
                }));
            }
            tracing::warn!("agent sidecar exited");
            if let Some(daemon) = weak.upgrade() {
                daemon.chats.on_sidecar_exit();
            }
        });
        Ok(process)
    }

    /// Sends a request and waits for its result.
    pub async fn call(&self, mut call: SidecarCall) -> Result<Value> {
        let process = self.ensure().await?;
        let daemon = self.daemon.upgrade().expect("daemon alive");
        let launch = match &mut call {
            SidecarCall::SessionStart(params) => {
                let intent = crate::workspace_launches::begin(
                    &daemon.store,
                    &params.workspace_id,
                    &params.chat_id,
                    &process.generation,
                )?;
                params.launch_id = Some(intent.id.clone());
                Some(intent)
            }
            _ => None,
        };
        let stop = match &call {
            SidecarCall::SessionStop(params) => Some((params.chat_id.clone(), false)),
            SidecarCall::SessionCloseIdle(params) => Some((params.chat_id.clone(), true)),
            _ => None,
        };
        let id = process.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        process.pending.lock().unwrap().insert(id, tx);
        process
            .write(&Request::new(id, call))
            .await
            .map_err(|e| Error::Unavailable(format!("writing to the sidecar: {e}")))?;
        let result = match tokio::time::timeout(Duration::from_secs(120), rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(error))) => Err(Error::Unavailable(error.message)),
            Ok(Err(_)) => Err(Error::Unavailable("the agent sidecar exited".into())),
            Err(_) => {
                process.pending.lock().unwrap().remove(&id);
                Err(Error::Unavailable(
                    "the agent sidecar did not answer in 120 s".into(),
                ))
            }
        }?;
        if let Some(launch) = launch {
            if let (Some(parent), Some(pid)) = (
                &process.pin,
                result
                    .get("native_pid")
                    .and_then(Value::as_u64)
                    .and_then(|pid| u32::try_from(pid).ok()),
            ) {
                // Missing/failed registration leaves uncertainty; it must not replay a successful start.
                if crate::workspace_launches::register_root(&daemon.store, &launch, parent, pid)
                    .is_err()
                {
                    tracing::warn!(launch_id = %launch.id, "native root registration remains uncertain");
                }
            }
            crate::workspace_launches::acknowledged(&daemon.store, &launch)?;
        }
        if let Some((chat, idle_only)) = stop {
            // Legacy empty replies, missing sessions and a new sidecar cannot settle old intents.
            if (!idle_only || result.get("closed").and_then(Value::as_bool) == Some(true))
                && let Some(ids) = result.get("confirmed_launch_ids")
            {
                let ids: Vec<String> = serde_json::from_value(ids.clone())?;
                crate::workspace_launches::confirmed(
                    &daemon.store,
                    &chat,
                    &process.generation,
                    &ids,
                )?;
            }
        }
        Ok(result)
    }

    /// Gives a running sidecar the environment agents get from now on (one started later takes
    /// the daemon's at its spawn). Its running sessions keep theirs.
    pub async fn set_env(&self, env: std::collections::BTreeMap<String, String>) -> Result<()> {
        if !self.is_running().await {
            return Ok(());
        }
        self.call(SidecarCall::SetEnv(SetEnvParams { env }))
            .await
            .map(|_| ())
    }

    /// Stops the sidecar: closing stdin ends every session.
    pub async fn shutdown(&self) {
        let process = self.process.lock().await.take();
        if let Some(process) = process {
            let _ = process.stdin.lock().await.shutdown().await;
            let mut child = process.child.lock().await;
            if tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .is_err()
            {
                let _ = child.kill().await;
            }
            if let Some(pid) = process.pid {
                kill_group(pid);
            }
        }
    }
}

/// Ends what a sidecar left running in its process group: its agents and their commands. TERM
/// first, KILL a moment later for what ignored it.
fn kill_group(pid: u32) {
    let Ok(group) = i32::try_from(pid) else {
        return;
    };
    // SAFETY: killpg has no preconditions; a group that is gone answers ESRCH.
    if unsafe { libc::killpg(group, libc::SIGTERM) } != 0 {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        // SAFETY: as above.
        unsafe { libc::killpg(group, libc::SIGKILL) };
    });
}

impl Process {
    async fn write<T: serde::Serialize>(&self, message: &T) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(message)?;
        line.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&line).await?;
        stdin.flush().await
    }
}

/// Boxed, its future named `Send`: a tool call can reach the sidecar again (adding a repo closes
/// the workspace's idle sessions), which would make the sidecar's own future type recursive.
fn handle_hook(
    daemon: &Arc<Daemon>,
    call: SidecarToDaemon,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>> {
    Box::pin(handle_hook_call(daemon, call))
}

async fn handle_hook_call(daemon: &Arc<Daemon>, call: SidecarToDaemon) -> Result<Value> {
    match call {
        SidecarToDaemon::WorktreeCreate(params) => {
            let path = daemon
                .chats
                .worktree_create(&params.chat_id, &params.name)
                .await?;
            Ok(serde_json::to_value(WorktreeCreateResult { path })?)
        }
        SidecarToDaemon::WorktreeRemove(params) => {
            daemon
                .chats
                .worktree_remove(&params.chat_id, &params.path)
                .await?;
            Ok(serde_json::to_value(Empty {})?)
        }
        SidecarToDaemon::ToolListRepos(params) => Ok(serde_json::to_value(
            daemon.chats.tool_list_repos(&params.chat_id).await?,
        )?),
        SidecarToDaemon::ToolAddRepo(params) => Ok(serde_json::to_value(
            daemon
                .chats
                .tool_add_repo(&params.chat_id, &params.repo)
                .await?,
        )?),
        SidecarToDaemon::ToolDelegate(params) => Ok(serde_json::to_value(
            daemon.chats.tool_delegate(params).await?,
        )?),
        SidecarToDaemon::ToolDelegateWait(params) => Ok(serde_json::to_value(
            daemon.chats.tool_delegate_wait(params).await?,
        )?),
        SidecarToDaemon::ToolDelegateStop(params) => {
            daemon.chats.tool_delegate_stop(params).await?;
            Ok(serde_json::to_value(Empty {})?)
        }
        SidecarToDaemon::ToolDelegateDetach(params) => {
            daemon.chats.tool_delegate_detach(params).await?;
            Ok(serde_json::to_value(Empty {})?)
        }
        SidecarToDaemon::SessionEvent(_) | SidecarToDaemon::AgentLoginEnded(_) => Ok(Value::Null),
    }
}

/// `CHAUFFEUR_SIDECAR`, else `chauffeur-sidecar` next to the daemon, else the dev build in the
/// source tree.
pub fn sidecar_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CHAUFFEUR_SIDECAR") {
        return Some(PathBuf::from(path));
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("chauffeur-sidecar")));
    let dev =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sidecar/dist/chauffeur-sidecar");
    [beside, Some(dev)]
        .into_iter()
        .flatten()
        .find(|path| path.exists())
}
