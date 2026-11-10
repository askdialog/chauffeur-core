//! A JSON-RPC client of the daemon over its Unix socket, starting the daemon when it is not
//! running.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, bail};
use chauffeur_protocol::PROTOCOL_VERSION;
use chauffeur_protocol::client::{ClientCall, EventNotification, HelloParams, HelloResult};
use chauffeur_protocol::rpc::{Incoming, Request, RpcError};
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::net::UnixStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

pub struct Client {
    socket: WebSocketStream<UnixStream>,
    next_id: u64,
    notifications: VecDeque<EventNotification>,
}

/// Mirrors the daemon's `Paths` for the files the CLI needs.
pub struct Home {
    pub home: PathBuf,
}

impl Home {
    pub fn from_env() -> Home {
        let home = std::env::var_os("CHAUFFEUR_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".chauffeur")
            });
        Home { home }
    }

    pub fn socket(&self) -> PathBuf {
        self.home.join("chauffeurd.sock")
    }

    pub fn log(&self) -> PathBuf {
        self.home.join("logs").join("chauffeurd.log")
    }
}

impl Client {
    /// Connects, starting the daemon if needed, and checks protocol versions.
    pub async fn connect(autostart: bool) -> anyhow::Result<Client> {
        let home = Home::from_env();
        let stream = match UnixStream::connect(home.socket()).await {
            Ok(stream) => stream,
            Err(error) if sandbox_refused(&error, &home.socket()) => {
                return Err(sandbox_error(error, &home.socket()));
            }
            // Inside a workspace (an agent's tool call) never start a second, empty daemon.
            Err(_) if autostart && std::env::var_os("CHAUFFEUR_WORKSPACE_ID").is_none() => {
                start_daemon(&home)?;
                wait_for_socket(&home).await?
            }
            Err(error) => {
                return Err(error)
                    .context("the Chauffeur daemon is not running (`chauffeur daemon start`)");
            }
        };
        let (client, hello) = Client::handshake(stream).await?;
        if let Some(warning) = version_warning(&hello) {
            eprintln!("chauffeur: {warning}");
        }
        Ok(client)
    }

    /// Connects to the daemon listening on `socket`, never starting one: another daemon's home,
    /// or a socket forwarded from another machine.
    pub async fn connect_socket(socket: &Path) -> anyhow::Result<Client> {
        let stream = UnixStream::connect(socket)
            .await
            .with_context(|| format!("no Chauffeur daemon answers on {}", socket.display()))?;
        let (client, hello) = Client::handshake(stream).await?;
        if let Some(warning) = version_warning(&hello) {
            eprintln!("chauffeur: {} on {warning}", socket.display());
        }
        Ok(client)
    }

    async fn handshake(stream: UnixStream) -> anyhow::Result<(Client, HelloResult)> {
        let (socket, _) = tokio_tungstenite::client_async("ws://localhost/ws", stream)
            .await
            .context("connecting to the daemon")?;
        let mut client = Client {
            socket,
            next_id: 1,
            notifications: VecDeque::new(),
        };
        let hello: HelloResult = client
            .call(ClientCall::Hello(HelloParams {
                protocol_version: PROTOCOL_VERSION,
                client: format!("chauffeur-cli {}", env!("CARGO_PKG_VERSION")),
            }))
            .await
            .context(format!(
                "the daemon did not take the hello of CLI {}: restart Chauffeur or run \
                 `chauffeur daemon stop`",
                env!("CARGO_PKG_VERSION")
            ))?;
        Ok((client, hello))
    }

    pub async fn call<T: DeserializeOwned>(&mut self, call: ClientCall) -> anyhow::Result<T> {
        let value = self.call_value(call).await?;
        serde_json::from_value(value).context("unexpected result from the daemon")
    }

    pub async fn call_value(&mut self, call: ClientCall) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let text = serde_json::to_string(&Request::new(id, call))?;
        self.socket.send(Message::Text(text.into())).await?;
        loop {
            let incoming = self.read().await?;
            if incoming.is_response() && incoming.id.as_ref().and_then(Value::as_u64) == Some(id) {
                if let Some(error) = incoming.error {
                    return Err(RpcFailure(error).into());
                }
                return Ok(incoming.result.unwrap_or(Value::Null));
            }
            self.keep_notification(incoming);
        }
    }

    /// The next event of any subscription of this connection.
    pub async fn next_event(&mut self) -> anyhow::Result<EventNotification> {
        if let Some(event) = self.notifications.pop_front() {
            return Ok(event);
        }
        loop {
            let incoming = self.read().await?;
            if let Some(event) = parse_notification(&incoming) {
                return Ok(event);
            }
        }
    }

    fn keep_notification(&mut self, incoming: Incoming) {
        if let Some(event) = parse_notification(&incoming) {
            self.notifications.push_back(event);
        }
    }

    async fn read(&mut self) -> anyhow::Result<Incoming> {
        loop {
            let message = self
                .socket
                .next()
                .await
                .ok_or_else(|| anyhow::anyhow!("the daemon closed the connection"))??;
            let text = match message {
                Message::Text(text) => text.to_string(),
                Message::Binary(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Message::Close(_) => bail!("the daemon closed the connection"),
                _ => continue,
            };
            return serde_json::from_str(&text).context("unreadable message from the daemon");
        }
    }
}

/// An error the daemon answered, kept whole for `--json` and the exit code; shown as its message.
#[derive(Debug)]
pub struct RpcFailure(pub RpcError);

impl std::fmt::Display for RpcFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.message)
    }
}

impl std::error::Error for RpcFailure {}

/// What to tell the user when the daemon runs another version than this CLI: a daemon left
/// running across an update answers with its old code.
fn version_warning(hello: &HelloResult) -> Option<String> {
    let ours = env!("CARGO_PKG_VERSION");
    let restart = "restart Chauffeur or run `chauffeur daemon stop`";
    if hello.daemon_version != ours {
        Some(format!(
            "daemon {}, CLI {ours}: {restart}",
            hello.daemon_version
        ))
    } else if hello.protocol_version != PROTOCOL_VERSION {
        Some(format!(
            "daemon protocol {}, CLI protocol {PROTOCOL_VERSION}: {restart}",
            hello.protocol_version
        ))
    } else {
        None
    }
}

/// The socket is there but connecting to it was refused (EPERM, EACCES): an agent's sandbox (Codex's,
/// Claude Code's) forbids it, not a stopped daemon.
fn sandbox_refused(error: &std::io::Error, socket: &Path) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied && socket.exists()
}

/// Said so that an agent reruns the command outside its sandbox rather than give up.
fn sandbox_error(error: std::io::Error, socket: &Path) -> anyhow::Error {
    anyhow::Error::new(error).context(format!(
        "a sandbox blocked the connection to the Chauffeur daemon's socket {}: \
         run this command again with escalated permissions, or outside the sandbox",
        socket.display()
    ))
}

fn parse_notification(incoming: &Incoming) -> Option<EventNotification> {
    if incoming.method.as_deref() != Some("event") {
        return None;
    }
    serde_json::from_value(incoming.params.clone()?).ok()
}

/// `chauffeurd` next to this binary, else on PATH.
fn daemon_binary() -> PathBuf {
    std::env::var_os("CHAUFFEURD")
        .map(PathBuf::from)
        .or_else(|| {
            // Through symlinks (~/.local/bin/chauffeur → ~/.chauffeur/bin/chauffeur).
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.canonicalize().ok())
                .and_then(|exe| exe.parent().map(|dir| dir.join("chauffeurd")))
                .filter(|path| path.exists())
        })
        .unwrap_or_else(|| PathBuf::from("chauffeurd"))
}

pub fn start_daemon(home: &Home) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;
    std::fs::create_dir_all(home.log().parent().unwrap())?;
    // Agents' output lands in the log: the user's alone, like the rest of the state folder.
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(home.log())?;
    let binary = daemon_binary();
    std::process::Command::new(&binary)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .with_context(|| format!("starting {}", binary.display()))?;
    Ok(())
}

async fn wait_for_socket(home: &Home) -> anyhow::Result<UnixStream> {
    for _ in 0..200 {
        if let Ok(stream) = UnixStream::connect(home.socket()).await {
            return Ok(stream);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    bail!(
        "the daemon did not start within 10 s; see {}",
        home.log().display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file standing for the daemon's socket, in a folder of its own.
    fn socket_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chauffeur-cli-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("chauffeurd.sock");
        std::fs::write(&socket, "").unwrap();
        socket
    }

    fn hello(daemon_version: &str, protocol_version: u32) -> HelloResult {
        serde_json::from_value(serde_json::json!({
            "protocol_version": protocol_version,
            "daemon_version": daemon_version,
            "home": "/h",
            "workspaces_dir": "/w",
        }))
        .unwrap()
    }

    #[test]
    fn a_daemon_of_another_version_is_named_with_the_way_to_restart_it() {
        assert_eq!(
            version_warning(&hello("0.1.0", PROTOCOL_VERSION)).unwrap(),
            format!(
                "daemon 0.1.0, CLI {}: restart Chauffeur or run `chauffeur daemon stop`",
                env!("CARGO_PKG_VERSION")
            )
        );
        let ours = env!("CARGO_PKG_VERSION");
        assert!(
            version_warning(&hello(ours, PROTOCOL_VERSION - 1))
                .unwrap()
                .starts_with(&format!("daemon protocol {}", PROTOCOL_VERSION - 1))
        );
        assert_eq!(version_warning(&hello(ours, PROTOCOL_VERSION)), None);
    }

    #[test]
    fn a_refused_socket_that_exists_names_the_sandbox() {
        let socket = socket_file("refused");
        for code in [libc::EPERM, libc::EACCES] {
            let error = std::io::Error::from_raw_os_error(code);
            assert!(sandbox_refused(&error, &socket));
            let message = format!("{:#}", sandbox_error(error, &socket));
            assert!(message.contains("a sandbox blocked"), "{message}");
            assert!(message.contains("escalated permissions"), "{message}");
            assert!(message.contains(&socket.display().to_string()), "{message}");
            assert!(!message.contains("not running"), "{message}");
        }
        std::fs::remove_dir_all(socket.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_missing_or_closed_socket_is_a_stopped_daemon() {
        let socket = socket_file("closed");
        for code in [libc::ENOENT, libc::ECONNREFUSED] {
            assert!(!sandbox_refused(
                &std::io::Error::from_raw_os_error(code),
                &socket
            ));
        }
        std::fs::remove_dir_all(socket.parent().unwrap()).unwrap();
        // No socket: a refusal cannot come from a sandbox guarding it.
        let error = std::io::Error::from_raw_os_error(libc::EPERM);
        assert!(!sandbox_refused(&error, &socket));
    }
}
