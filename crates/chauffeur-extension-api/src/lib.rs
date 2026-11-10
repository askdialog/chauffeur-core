//! Official extension processes communicate exclusively through their parent's private pipes.
//! Host requests have their own IDs, so a runtime can call Core while Core awaits its result.
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::time::Duration;

pub use chauffeur_protocol::extension_host::{
    Failure, Frame, HOST_API_VERSION, Init, MAX_FRAME_BYTES, UI_API_VERSION,
};

/// Reads one bounded, newline-delimited JSON frame; a partial frame at EOF is invalid.
pub fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Frame>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "unfinished extension frame",
                ))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(available.len(), |index| index + 1);
        if bytes.len() + count > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "extension frame is too large",
            ));
        }
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if newline.is_some() {
            return serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
        }
    }
}

pub fn write_frame(writer: &mut impl Write, frame: &Frame) -> io::Result<()> {
    let bytes = serde_json::to_vec(frame).map_err(io::Error::other)?;
    if bytes.len() + 1 > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "extension frame is too large",
        ));
    }
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

type Reply = std::result::Result<Value, Failure>;
type Pending = HashMap<u64, mpsc::SyncSender<Reply>>;

struct Connection {
    writer: Mutex<Box<dyn Write + Send>>,
    pending: Mutex<Pending>,
    next: AtomicU64,
    closed: AtomicBool,
}

#[derive(Clone)]
pub struct Host(Arc<Connection>);

impl Host {
    fn send(&self, frame: &Frame) -> io::Result<()> {
        write_frame(&mut *self.0.writer.lock().unwrap(), frame)
    }

    /// Blocking callbacks use the reader's separate thread, including from an async runtime.
    pub fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<T, Failure> {
        let params = serde_json::to_value(params).map_err(Failure::internal)?;
        let id = self.0.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::sync_channel(1);
        {
            let mut pending = self.0.pending.lock().unwrap();
            if self.0.closed.load(Ordering::Acquire) {
                return Err(Failure::unavailable(
                    "Core disconnected; reconnect this extension",
                ));
            }
            pending.insert(id, sender);
        }
        if let Err(error) = self.send(&Frame::HostCall {
            id,
            method: method.into(),
            params,
        }) {
            self.0.pending.lock().unwrap().remove(&id);
            return Err(Failure::unavailable(format!("Core disconnected: {error}")));
        }
        let result = receiver.recv_timeout(Duration::from_secs(10 * 60));
        self.0.pending.lock().unwrap().remove(&id);
        let value = result
            .map_err(|_| Failure::unavailable("Core did not answer; reconnect this extension"))??;
        serde_json::from_value(value).map_err(Failure::internal)
    }

    pub fn publish(&self, event: impl Serialize) -> Result<(), Failure> {
        let event = serde_json::to_value(event).map_err(Failure::internal)?;
        self.send(&Frame::Event { event })
            .map_err(Failure::internal)
    }

    fn answer(&self, id: u64, outcome: Reply) {
        if let Some(sender) = self.0.pending.lock().unwrap().remove(&id) {
            let _ = sender.send(outcome);
        }
    }

    fn disconnect(&self) {
        self.0.closed.store(true, Ordering::Release);
        let pending = std::mem::take(&mut *self.0.pending.lock().unwrap());
        for (_, sender) in pending {
            let _ = sender.send(Err(Failure::unavailable(
                "Core disconnected; reconnect this extension",
            )));
        }
    }
}

pub trait Runtime: Send + Sync + 'static {
    fn call(&self, method: String, params: Value) -> futures_util::future::BoxFuture<'_, Reply>;
    fn notify(&self, _method: &str, _params: Value) {}
    fn shutdown(&self) {}
}

/// Stdout belongs to the protocol. Extension diagnostics must go to stderr.
pub async fn run<R: Runtime>(
    factory: impl FnOnce(Host, &Init) -> Result<R, Failure>,
) -> io::Result<()> {
    serve(io::BufReader::new(io::stdin()), io::stdout(), factory).await
}

async fn serve<R: Runtime>(
    mut input: impl BufRead + Send + 'static,
    output: impl Write + Send + 'static,
    factory: impl FnOnce(Host, &Init) -> Result<R, Failure>,
) -> io::Result<()> {
    let init = match read_frame(&mut input)? {
        Some(Frame::Initialize { init }) if init.api == HOST_API_VERSION => init,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported extension initialization",
            ));
        }
    };
    let host = Host(Arc::new(Connection {
        writer: Mutex::new(Box::new(output)),
        pending: Mutex::new(HashMap::new()),
        next: AtomicU64::new(1),
        closed: AtomicBool::new(false),
    }));
    // Unbounded, so that the reader keeps reading: the runtime handles requests one at a time,
    // each perhaps waiting on host results only the reader delivers, while Core may send a burst
    // (the activity it kept while the engine was stopped). Core bounds what it keeps unsent.
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let reader_host = host.clone();
    std::thread::spawn(move || {
        let mut reader = input;
        while let Ok(Some(frame)) = read_frame(&mut reader) {
            match frame {
                Frame::Ping { id } => {
                    if reader_host.send(&Frame::Pong { id }).is_err() {
                        break;
                    }
                }
                Frame::HostResult { id, outcome } => reader_host.answer(id, outcome),
                // A shutdown is queued behind what came before it, and the reader keeps answering
                // host calls: the requests sent before it are handled in order, then `run` ends.
                Frame::Shutdown | Frame::Call { .. } | Frame::Notify { .. } => {
                    if sender.send(frame).is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
        // Core is gone (or broke the protocol): what is queued is dropped.
        reader_host.disconnect();
    });
    let runtime = Arc::new(factory(host.clone(), &init).map_err(io::Error::other)?);
    host.send(&Frame::Ready {
        api: HOST_API_VERSION,
        extension: init.extension,
        version: init.version,
    })?;
    let mut calls = tokio::task::JoinSet::new();
    while let Some(frame) = receiver.recv().await {
        if host.0.closed.load(Ordering::Acquire) {
            break;
        }
        match frame {
            Frame::Call { id, method, params } => {
                let runtime = runtime.clone();
                let host = host.clone();
                calls.spawn(async move {
                    let outcome = runtime.call(method, params).await;
                    host.send(&Frame::Result { id, outcome })
                });
            }
            Frame::Notify { method, params } => runtime.notify(&method, params),
            Frame::Shutdown => break,
            _ => unreachable!("the pipe reader forwards only runtime requests"),
        }
        while let Some(result) = calls.try_join_next() {
            result.map_err(io::Error::other)??;
        }
    }
    runtime.shutdown();
    calls.abort_all();
    host.disconnect();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Recorder {
        host: Host,
        heard: Arc<Mutex<Vec<String>>>,
    }
    impl Runtime for Recorder {
        fn call(&self, _: String, _: Value) -> futures_util::future::BoxFuture<'_, Reply> {
            Box::pin(async { Ok(Value::Null) })
        }
        fn notify(&self, method: &str, _: Value) {
            let answer = self
                .host
                .call::<String>("kv.get", serde_json::json!({}))
                .unwrap_or_else(|failure| failure.message);
            self.heard
                .lock()
                .unwrap()
                .push(format!("{method}:{answer}"));
        }
        fn shutdown(&self) {
            self.heard.lock().unwrap().push("shutdown".into());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_shutdown_lets_the_notifications_before_it_finish_in_order_with_their_host_calls() {
        use std::os::unix::net::UnixStream;
        let (input, mut core_writes) = UnixStream::pair().unwrap();
        let (output, core_reads) = UnixStream::pair().unwrap();
        let mut core_reads = io::BufReader::new(core_reads);
        let send = |writer: &mut UnixStream, frame: Frame| write_frame(writer, &frame).unwrap();
        send(
            &mut core_writes,
            Frame::Initialize {
                init: Init {
                    api: HOST_API_VERSION,
                    extension: "office".into(),
                    version: "1.0.0".into(),
                },
            },
        );
        for method in ["first", "second"] {
            send(
                &mut core_writes,
                Frame::Notify {
                    method: method.into(),
                    params: Value::Null,
                },
            );
        }
        send(&mut core_writes, Frame::Shutdown);
        // Core answers each host call the notifications make, after it asked for the shutdown.
        let core = std::thread::spawn(move || {
            while let Ok(Some(frame)) = read_frame(&mut core_reads) {
                if let Frame::HostCall { id, .. } = frame {
                    send(
                        &mut core_writes,
                        Frame::HostResult {
                            id,
                            outcome: Ok(serde_json::json!(format!("answer{id}"))),
                        },
                    );
                }
            }
        });
        let heard = Arc::new(Mutex::new(Vec::new()));
        let recorded = heard.clone();
        tokio::time::timeout(
            Duration::from_secs(10),
            serve(io::BufReader::new(input), output, move |host, _| {
                Ok(Recorder {
                    host,
                    heard: recorded,
                })
            }),
        )
        .await
        .expect("the runtime ends after its shutdown")
        .unwrap();
        assert_eq!(
            *heard.lock().unwrap(),
            ["first:answer1", "second:answer2", "shutdown"]
        );
        drop(core);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_burst_of_notifications_longer_than_any_queue_is_handled_whole_with_its_host_calls() {
        use std::os::unix::net::UnixStream;
        const BURST: usize = 1024;
        let (input, core_writes) = UnixStream::pair().unwrap();
        let (output, core_reads) = UnixStream::pair().unwrap();
        let core_writes = Arc::new(Mutex::new(core_writes));
        let send = |writer: &Mutex<UnixStream>, frame: Frame| {
            write_frame(&mut *writer.lock().unwrap(), &frame).unwrap()
        };
        let burst = core_writes.clone();
        // Core sends the whole burst before the runtime is done with its first notification.
        let sender = std::thread::spawn(move || {
            send(
                &burst,
                Frame::Initialize {
                    init: Init {
                        api: HOST_API_VERSION,
                        extension: "office".into(),
                        version: "1.0.0".into(),
                    },
                },
            );
            for _ in 0..BURST {
                send(
                    &burst,
                    Frame::Notify {
                        method: "core.activity".into(),
                        params: Value::Null,
                    },
                );
            }
            send(&burst, Frame::Shutdown);
        });
        let core = std::thread::spawn(move || {
            let mut core_reads = io::BufReader::new(core_reads);
            while let Ok(Some(frame)) = read_frame(&mut core_reads) {
                if let Frame::HostCall { id, .. } = frame {
                    send(
                        &core_writes,
                        Frame::HostResult {
                            id,
                            outcome: Ok(serde_json::json!("answer")),
                        },
                    );
                }
            }
        });
        let heard = Arc::new(Mutex::new(Vec::new()));
        let recorded = heard.clone();
        tokio::time::timeout(
            Duration::from_secs(20),
            serve(io::BufReader::new(input), output, move |host, _| {
                Ok(Recorder {
                    host,
                    heard: recorded,
                })
            }),
        )
        .await
        .expect("the runtime ends after its shutdown")
        .unwrap();
        let heard = heard.lock().unwrap();
        assert_eq!(heard.len(), BURST + 1);
        assert!(
            heard[..BURST]
                .iter()
                .all(|entry| entry == "core.activity:answer"),
            "{:?}",
            heard.iter().find(|entry| *entry != "core.activity:answer")
        );
        assert_eq!(heard[BURST], "shutdown");
        sender.join().unwrap();
        drop(core);
    }

    #[test]
    fn an_unfinished_frame_is_not_accepted_as_a_request() {
        let mut input = io::Cursor::new(br#"{"type":"shutdown"}"#);
        assert_eq!(
            read_frame(&mut input).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert!(read_frame(&mut io::Cursor::new(b"")).unwrap().is_none());
    }
    #[test]
    fn the_frame_limit_is_checked_before_accumulating_unbounded_input() {
        let bytes = vec![b'a'; MAX_FRAME_BYTES + 1];
        assert_eq!(
            read_frame(&mut io::Cursor::new(bytes)).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
    #[test]
    fn host_callbacks_and_runtime_results_have_distinct_envelopes() {
        let frame = Frame::HostResult {
            id: 9,
            outcome: Ok(serde_json::json!({"answer": 42})),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).unwrap();
        let decoded = read_frame(&mut io::Cursor::new(bytes)).unwrap().unwrap();
        assert!(matches!(
            decoded,
            Frame::HostResult {
                id: 9,
                outcome: Ok(_)
            }
        ));
    }
}
