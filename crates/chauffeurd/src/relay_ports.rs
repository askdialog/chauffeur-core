//! A moved workspace's ports, reachable here (docs/workspace-relay.md, decided with Justin
//! 2026-10-26): while a workspace runs on a machine of `[transfer]`, this daemon listens on
//! 127.0.0.1 on the workspace's own port block and passes each connection to the same port of
//! its block on that machine, over that daemon's socket (`/tunnel/{workspace}/{offset}`). Its dev
//! servers open in this machine's browser as if they ran here; nothing listens beyond loopback.
//! The listeners close once the workspace is back, or that machine no longer holds it.
//!
//! The phone reaches a moved workspace's declared previews the same way, one connection at a time,
//! through the mobile host on this daemon's Unix socket (`/preview/{workspace}/{offset}`,
//! [`preview`]), never through a listener: nothing it opens is reachable from this machine.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::Daemon;
use crate::relay::Machine;

/// How often the workspaces to relay are looked for again.
const WATCH_EVERY: Duration = Duration::from_secs(5);
/// Bytes read from a connection at a time.
const CHUNK: usize = 64 << 10;

/// Keeps the ports of every moved workspace relayed, for as long as it runs there.
pub fn spawn(daemon: Arc<Daemon>) {
    tokio::spawn(async move {
        // Cancelled: its listeners and the connections they took end.
        let mut relayed: HashMap<String, (Machine, CancellationToken)> = HashMap::new();
        loop {
            let moved = crate::relay_events::moved_workspaces(&daemon);
            relayed.retain(|workspace, (machine, relay)| {
                let keep = moved.get(workspace) == Some(machine);
                if !keep {
                    relay.cancel();
                }
                keep
            });
            for (workspace, machine) in moved {
                if relayed.contains_key(&workspace) {
                    continue;
                }
                let relay = daemon.shutdown.child_token();
                listen(&daemon, &workspace, &machine, &relay).await;
                relayed.insert(workspace, (machine, relay));
            }
            tokio::select! {
                _ = tokio::time::sleep(WATCH_EVERY) => {}
                _ = daemon.shutdown.cancelled() => return,
            }
        }
    });
}

/// Listens on each port of `workspace`'s block here, passing connections to `machine`, until
/// `relay` is cancelled.
async fn listen(daemon: &Daemon, workspace: &str, machine: &Machine, relay: &CancellationToken) {
    let Ok(Some(block)) = daemon.store.port_block(workspace) else {
        return;
    };
    for offset in 0..block.size {
        let port = block.base + offset;
        let listener = match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(port, %error, workspace, "a moved workspace's port is not relayed");
                continue;
            }
        };
        let workspace = workspace.to_owned();
        let machine = machine.clone();
        let relay = relay.clone();
        tokio::spawn(async move {
            loop {
                let client = tokio::select! {
                    accepted = listener.accept() => match accepted {
                        Ok((client, _)) => client,
                        Err(_) => return,
                    },
                    _ = relay.cancelled() => return,
                };
                let (workspace, machine, relay) =
                    (workspace.clone(), machine.clone(), relay.clone());
                tokio::spawn(async move {
                    tokio::select! {
                        result = forward(client, &machine, &workspace, offset) => {
                            if let Err(error) = result {
                                tracing::debug!(port, %error, "a relayed connection ended");
                            }
                        }
                        // Back here, or gone elsewhere: that machine no longer serves it.
                        _ = relay.cancelled() => {}
                    }
                });
            }
        });
    }
}

/// A connection to port `offset` of `workspace`'s block on `machine`, as its daemon serves it.
pub(crate) type Tunnel = tokio_tungstenite::WebSocketStream<UnixStream>;

/// Opens a connection to port `offset` of `workspace`'s block on `machine`; refused when nothing
/// listens there.
pub(crate) async fn open(
    machine: &Machine,
    workspace: &str,
    offset: u16,
) -> anyhow::Result<Tunnel> {
    let stream = UnixStream::connect(&machine.socket).await?;
    let url = format!("ws://localhost/tunnel/{workspace}/{offset}");
    let (socket, _) = tokio_tungstenite::client_async(url, stream).await?;
    Ok(socket)
}

/// Passes one connection to port `offset` of `workspace`'s block on `machine`.
async fn forward(
    client: TcpStream,
    machine: &Machine,
    workspace: &str,
    offset: u16,
) -> anyhow::Result<()> {
    let socket = open(machine, workspace, offset).await?;
    let (sink, source) = socket.split();
    let sink = sink.with(|frame: Frame| {
        futures_util::future::ok::<_, tokio_tungstenite::tungstenite::Error>(match frame {
            Frame::Bytes(bytes) => Message::Binary(bytes.into()),
            Frame::Eof => Message::Binary(Vec::new().into()),
            Frame::Closed => Message::Close(None),
        })
    });
    let source = source.filter_map(|message| {
        futures_util::future::ready(match message {
            Ok(Message::Binary(bytes)) if bytes.is_empty() => Some(Frame::Eof),
            Ok(Message::Binary(bytes)) => Some(Frame::Bytes(bytes.to_vec())),
            Ok(Message::Close(_)) | Err(_) => Some(Frame::Closed),
            Ok(_) => None,
        })
    });
    pipe(client, Box::pin(sink), Box::pin(source)).await;
    Ok(())
}

/// A preview's connection for the phone: the mobile host's WebSocket passed message for message
/// (bytes, an empty one for the end of writes, a close) to `tunnel`, until either side closes,
/// the workspace leaves `machine` (checked every 5 s: back here, or gone elsewhere) or the
/// daemon stops.
pub(crate) async fn preview(
    daemon: Arc<Daemon>,
    client: axum::extract::ws::WebSocket,
    tunnel: Tunnel,
    machine: Machine,
    workspace: String,
) {
    use axum::extract::ws::Message as WsMessage;
    let (mut to_client, mut from_client) = client.split();
    let (mut to_tunnel, mut from_tunnel) = tunnel.split();
    let up = async {
        while let Some(Ok(message)) = from_client.next().await {
            let message = match message {
                WsMessage::Binary(bytes) => Message::Binary(bytes.to_vec().into()),
                WsMessage::Close(_) => break,
                _ => continue,
            };
            if to_tunnel.send(message).await.is_err() {
                return;
            }
        }
        let _ = to_tunnel.send(Message::Close(None)).await;
    };
    let down = async {
        while let Some(Ok(message)) = from_tunnel.next().await {
            let message = match message {
                Message::Binary(bytes) => WsMessage::Binary(bytes.to_vec().into()),
                Message::Close(_) => break,
                _ => continue,
            };
            if to_client.send(message).await.is_err() {
                return;
            }
        }
        let _ = to_client.send(WsMessage::Close(None)).await;
    };
    let gone = async {
        loop {
            tokio::time::sleep(WATCH_EVERY).await;
            if crate::relay::machine_of(&daemon, &workspace).as_ref() != Some(&machine) {
                return;
            }
        }
    };
    tokio::select! {
        _ = up => {}
        _ = down => {}
        _ = gone => {}
        _ = daemon.shutdown.cancelled() => {}
    }
}

/// The machine's side: a tunnel's WebSocket and the local port it reaches.
pub(crate) async fn pipe_socket(socket: axum::extract::ws::WebSocket, stream: TcpStream) {
    use axum::extract::ws::Message as WsMessage;
    let (sink, source) = socket.split();
    let sink = sink.with(|frame: Frame| {
        futures_util::future::ok::<_, axum::Error>(match frame {
            Frame::Bytes(bytes) => WsMessage::Binary(bytes.into()),
            Frame::Eof => WsMessage::Binary(Vec::new().into()),
            Frame::Closed => WsMessage::Close(None),
        })
    });
    let source = source.filter_map(|message| {
        futures_util::future::ready(match message {
            Ok(WsMessage::Binary(bytes)) if bytes.is_empty() => Some(Frame::Eof),
            Ok(WsMessage::Binary(bytes)) => Some(Frame::Bytes(bytes.to_vec())),
            Ok(WsMessage::Close(_)) | Err(_) => Some(Frame::Closed),
            Ok(_) => None,
        })
    });
    pipe(stream, Box::pin(sink), Box::pin(source)).await;
}

/// What a tunnel carries: bytes, the end of one side's writes (an empty message: the other side
/// shuts its own writes and goes on reading), or the end of the connection.
enum Frame {
    Bytes(Vec<u8>),
    Eof,
    Closed,
}

/// Copies a TCP connection and a tunnel into each other until both directions ended, a side's
/// end of writes passed on as one (a client that sends its request then shuts its writes still
/// reads the answer), or until either side goes.
async fn pipe<S, R>(tcp: TcpStream, mut sink: S, mut source: R)
where
    S: futures_util::Sink<Frame> + Unpin,
    R: futures_util::Stream<Item = Frame> + Unpin,
{
    let (mut read, mut write) = tcp.into_split();
    let up = async move {
        let mut buffer = vec![0u8; CHUNK];
        loop {
            match read.read(&mut buffer).await {
                Ok(0) => {
                    let sent = sink.send(Frame::Eof).await.is_ok();
                    return (sink, sent);
                }
                Ok(n) => {
                    if sink.send(Frame::Bytes(buffer[..n].to_vec())).await.is_err() {
                        return (sink, false);
                    }
                }
                Err(_) => return (sink, false),
            }
        }
    };
    // True when the other side ended its writes only.
    let down = async move {
        while let Some(frame) = source.next().await {
            match frame {
                Frame::Bytes(bytes) => {
                    if write.write_all(&bytes).await.is_err() {
                        return false;
                    }
                }
                Frame::Eof => return write.shutdown().await.is_ok(),
                Frame::Closed => return false,
            }
        }
        false
    };
    tokio::pin!(up, down);
    let mut written: Option<S> = None;
    let mut read_all = false;
    loop {
        tokio::select! {
            (sink, sent) = &mut up, if written.is_none() => {
                if !sent {
                    return;
                }
                written = Some(sink);
            }
            half = &mut down, if !read_all => {
                if !half {
                    return;
                }
                read_all = true;
            }
        }
        if read_all && let Some(mut sink) = written.take() {
            let _ = sink.send(Frame::Closed).await;
            return;
        }
    }
}
