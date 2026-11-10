//! Supervised official engines. Process pipes are private capabilities; an app or browser can
//! never issue a host callback through the public daemon socket.
use crate::{Error, Result};
use chauffeur_extension_api::{Failure, Frame, HOST_API_VERSION, Init};
use futures_util::future::BoxFuture;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io::BufReader;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use tokio::sync::oneshot;

pub trait Services: Send + Sync + 'static {
    /// The engine became the installed one: capabilities kept from a candidate open now.
    fn activate(&self) {}
    fn call(&self, method: String, params: Value) -> BoxFuture<'_, Result<Value>>;
    fn event(&self, event: Value) -> Result<()>;
}

type Reply = Result<Value>;

pub struct Process {
    child: Mutex<Child>,
    input: Mutex<Option<mpsc::SyncSender<Frame>>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    callbacks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    next: AtomicU64,
    closed: AtomicBool,
    /// Core asked it to stop: its end is no failure to report.
    leaving: AtomicBool,
    services: Arc<dyn Services>,
}

fn wire(error: Error) -> Failure {
    let error = error.to_rpc();
    Failure {
        code: error.code,
        message: error.message,
        data: error.data,
    }
}

impl Process {
    pub async fn start(
        executable: &Path,
        init: Init,
        env: &BTreeMap<String, String>,
        services: Arc<dyn Services>,
    ) -> Result<Arc<Self>> {
        let mut child = Command::new(executable)
            .env_clear()
            .envs(env)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| {
                Error::Unavailable(format!(
                    "Could not start {}: {error}; reinstall this extension",
                    executable.display()
                ))
            })?;
        let mut input = child.stdin.take().expect("the engine's stdin is piped");
        let output = child.stdout.take().expect("the engine's stdout is piped");
        let (sender, receiver) = mpsc::sync_channel(128);
        let process = Arc::new(Self {
            child: Mutex::new(child),
            input: Mutex::new(Some(sender)),
            pending: Mutex::new(HashMap::new()),
            callbacks: Mutex::new(Vec::new()),
            next: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            leaving: AtomicBool::new(false),
            services: services.clone(),
        });
        let writer_process = Arc::downgrade(&process);
        std::thread::spawn(move || {
            for frame in receiver {
                if let Err(error) = chauffeur_extension_api::write_frame(&mut input, &frame) {
                    if let Some(process) = writer_process.upgrade()
                        && process.is_alive()
                    {
                        tracing::warn!(%error, "could not write to an extension engine: stopping it");
                        process.stop();
                    }
                    break;
                }
            }
        });
        let (ready, handshake) = oneshot::channel();
        let weak = Arc::downgrade(&process);
        let runtime = tokio::runtime::Handle::current();
        let expected = init.clone();
        std::thread::spawn(move || {
            let mut ready = Some(ready);
            let mut output = BufReader::new(output);
            let reason = loop {
                let frame = match chauffeur_extension_api::read_frame(&mut output) {
                    Ok(Some(frame)) => frame,
                    Ok(None) => break "it closed its output".to_string(),
                    Err(error) => break format!("it sent an unreadable frame: {error}"),
                };
                let Some(process) = weak.upgrade() else {
                    return;
                };
                match frame {
                    Frame::Ready {
                        api,
                        extension,
                        version,
                    } if ready.is_some()
                        && api == HOST_API_VERSION
                        && extension == expected.extension
                        && version == expected.version =>
                    {
                        let _ = ready.take().expect("handshake is pending").send(());
                    }
                    Frame::Result { id, outcome } if ready.is_none() => {
                        let pending = process.pending.lock().unwrap().remove(&id);
                        if let Some(sender) = pending {
                            let _ = sender.send(outcome.map_err(super::storage::failure));
                        }
                    }
                    Frame::Pong { id } if ready.is_none() => {
                        if let Some(sender) = process.pending.lock().unwrap().remove(&id) {
                            let _ = sender.send(Ok(Value::Null));
                        }
                    }
                    Frame::HostCall { id, method, params } => {
                        // Checked under the lock `stop` aborts under: frames a killed engine left
                        // in the pipe never write after `drain_callbacks` (a version switch
                        // snapshots and restores its state right then).
                        let mut callbacks = process.callbacks.lock().unwrap();
                        if process.closed.load(Ordering::Acquire) {
                            break "it was stopped".to_string();
                        }
                        let services = services.clone();
                        let task_process = Arc::downgrade(&process);
                        let task = runtime.spawn(async move {
                            let outcome = services.call(method, params).await.map_err(wire);
                            if let Some(process) = task_process.upgrade() {
                                let frame = Frame::HostResult { id, outcome };
                                // The engine may be waiting on this answer: one it cannot get
                                // leaves it stuck.
                                if let Err(error) = process.send(frame)
                                    && process.is_alive()
                                {
                                    tracing::warn!(%error, "could not answer an extension engine's host call: stopping it");
                                    process.stop();
                                }
                            }
                        });
                        callbacks.retain(|callback| !callback.is_finished());
                        callbacks.push(task);
                    }
                    Frame::Event { .. } if process.closed.load(Ordering::Acquire) => {
                        break "it was stopped".to_string();
                    }
                    Frame::Event { event } if ready.is_none() => {
                        if let Err(error) = services.event(event) {
                            break format!("it sent an invalid event: {error}");
                        }
                    }
                    frame => {
                        let kind =
                            serde_json::to_value(&frame).expect("frames serialize")["type"].clone();
                        break format!("it sent an unexpected {kind} frame");
                    }
                }
            };
            // The engine exited (or broke the protocol): the host calls it made before that
            // still finish, for `shutdown` or `drain_callbacks` to wait on.
            if let Some(process) = weak.upgrade()
                && let Some(status) = process.close(false)
                && !process.leaving.load(Ordering::Acquire)
            {
                tracing::warn!(reason, %status, "an extension engine stopped on its own");
            }
        });
        process.send(Frame::Initialize { init })?;
        if tokio::time::timeout(std::time::Duration::from_secs(30), handshake)
            .await
            .map_or(true, |result| result.is_err())
        {
            process.stop();
            process.drain_callbacks().await;
            return Err(Error::Unavailable("The extension did not complete its startup handshake; reinstall it or restore its previous version".into()));
        }
        Ok(process)
    }

    fn send(&self, frame: Frame) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Unavailable(
                "The extension stopped; reinstall it in Settings → Extensions".into(),
            ));
        }
        let input = self.input.lock().unwrap();
        let input = input.as_ref().ok_or_else(|| {
            Error::Unavailable(
                "The extension stopped; reinstall it in Settings → Extensions".into(),
            )
        })?;
        input.try_send(frame).map_err(|_| {
            Error::Unavailable(
                "The extension is not reading its requests; reinstall it in Settings → Extensions"
                    .into(),
            )
        })
    }

    pub async fn call(self: &Arc<Self>, method: String, params: Value) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);
        let sent = self.send(Frame::Call { id, method, params });
        if let Err(error) = sent {
            self.pending.lock().unwrap().remove(&id);
            return Err(error);
        }
        receiver.await.map_err(|_| {
            Error::Unavailable(
                "The extension disconnected; check the operation's outcome before trying again"
                    .into(),
            )
        })?
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.send(Frame::Notify {
            method: method.into(),
            params,
        })
    }

    /// Opens the capabilities a candidate engine starts without (`Services::activate`).
    pub fn activate_host(&self) {
        self.services.activate();
    }

    pub fn is_alive(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
    }

    pub async fn drain_callbacks(&self) {
        let callbacks = std::mem::take(&mut *self.callbacks.lock().unwrap());
        for callback in callbacks {
            let _ = callback.await;
        }
    }

    pub async fn ping(&self) -> Result<()> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);
        if let Err(error) = self.send(Frame::Ping { id }) {
            self.pending.lock().unwrap().remove(&id);
            return Err(error);
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), receiver).await;
        self.pending.lock().unwrap().remove(&id);
        match result {
            Ok(Ok(Ok(_))) => Ok(()),
            _ => {
                self.stop();
                self.drain_callbacks().await;
                Err(Error::Unavailable(
                    "The extension stopped answering; restart it or restore its previous version"
                        .into(),
                ))
            }
        }
    }

    /// Wait for the child to exit before releasing callers' workspace admission leases.
    pub fn stop(&self) {
        self.leaving.store(true, Ordering::Release);
        self.close(true);
    }

    /// Asks the engine to handle what it was sent, in order, and exit, as the host calls that
    /// made finish; kills it and drops what is left after `wait`. A version switch stops an engine
    /// this way, so that activity handed to it just before is not lost.
    pub async fn shutdown(&self, wait: std::time::Duration) {
        let deadline = tokio::time::Instant::now() + wait;
        self.leaving.store(true, Ordering::Release);
        if self.send(Frame::Shutdown).is_ok() {
            while self.is_alive() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            if !self.is_alive() {
                // Exited: nothing new is spawned now, so these are all its host calls.
                let callbacks = std::mem::take(&mut *self.callbacks.lock().unwrap());
                for mut callback in callbacks {
                    if tokio::time::timeout_at(deadline, &mut callback)
                        .await
                        .is_err()
                    {
                        callback.abort();
                        let _ = callback.await;
                    }
                }
            }
        }
        self.stop();
        self.drain_callbacks().await;
    }

    /// Kills and reaps the engine; its exit status, or `None` when it was already closed.
    fn close(&self, abort: bool) -> Option<ExitStatus> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return None;
        }
        self.input.lock().unwrap().take();
        if abort {
            for callback in self.callbacks.lock().unwrap().iter() {
                callback.abort();
            }
        }
        let mut child = self.child.lock().unwrap();
        // The child owns this process group. Killing it before wait also stops its subprocesses;
        // the unreaped child PID cannot have been reused for an unrelated process group.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let status = child
            .wait()
            .inspect_err(|error| tracing::warn!(%error, "could not reap an extension engine"))
            .ok();
        drop(child);
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        for (_, sender) in pending {
            let _ = sender.send(Err(Error::Unavailable("The extension stopped during this operation; check its outcome before trying again".into())));
        }
        status
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Echo;
    impl Services for Echo {
        fn call(&self, method: String, params: Value) -> BoxFuture<'_, Result<Value>> {
            Box::pin(async move {
                assert_eq!(method, "echo");
                Ok(params)
            })
        }
        fn event(&self, _: Value) -> Result<()> {
            Ok(())
        }
    }

    fn engine(version: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("engine");
        let body = r#"#!/usr/bin/python3
import json, sys
def send(frame):
    print(json.dumps(frame), flush=True)
init = json.loads(sys.stdin.readline())["init"]
send({"type": "ready", "api": 1, "extension": init["extension"], "version": VERSION})
for line in sys.stdin:
    call = json.loads(line)
    if call["type"] != "call":
        continue
    if call["method"] == "crash":
        sys.exit(3)
    send({"type": "host_call", "id": 77, "method": "echo", "params": call["params"]})
    answer = json.loads(sys.stdin.readline())
    send({"type": "result", "id": call["id"], "outcome": answer["outcome"]})
"#;
        std::fs::write(
            &script,
            body.replace("VERSION", &serde_json::to_string(version).unwrap()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        (dir, script)
    }
    fn init(version: &str) -> Init {
        Init {
            api: HOST_API_VERSION,
            extension: "database".into(),
            version: version.into(),
        }
    }

    #[tokio::test]
    async fn an_engine_can_call_the_host_while_the_host_waits_for_its_result() {
        let (_dir, executable) = engine("1.0.0");
        let process = Process::start(&executable, init("1.0.0"), &BTreeMap::new(), Arc::new(Echo))
            .await
            .unwrap();
        let params = serde_json::json!({"query": "SELECT 1"});
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            process.call("query".into(), params.clone()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, params);
        process.stop();
    }

    #[tokio::test]
    async fn a_crashed_engine_is_reaped_before_its_request_is_released() {
        let (_dir, executable) = engine("1.0.0");
        let process = Process::start(&executable, init("1.0.0"), &BTreeMap::new(), Arc::new(Echo))
            .await
            .unwrap();
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            process.call("crash".into(), Value::Null),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("check its outcome"));
        assert!(process.child.lock().unwrap().try_wait().unwrap().is_some());
    }

    /// Counts the host calls it serves.
    struct Count(Arc<AtomicU64>);
    impl Services for Count {
        fn call(&self, _: String, _: Value) -> BoxFuture<'_, Result<Value>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(Value::Null) })
        }
        fn event(&self, _: Value) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stopped_engine_s_buffered_host_calls_never_run_after_its_callbacks_drained() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("engine");
        // Floods the host with calls without waiting for their results, so the pipe still holds
        // many when the engine is killed.
        std::fs::write(
            &executable,
            r#"#!/usr/bin/python3
import json, sys
init = json.loads(sys.stdin.readline())["init"]
print(json.dumps({"type": "ready", "api": 1, "extension": init["extension"], "version": "1.0.0"}), flush=True)
sys.stdin.readline()
for i in range(100000):
    sys.stdout.write(json.dumps({"type": "host_call", "id": i, "method": "kv.set", "params": {"i": i}}) + chr(10))
sys.stdout.flush()
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let served = Arc::new(AtomicU64::new(0));
        let process = Process::start(
            &executable,
            init("1.0.0"),
            &BTreeMap::new(),
            Arc::new(Count(served.clone())),
        )
        .await
        .unwrap();
        process.notify("go", Value::Null).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while served.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        process.stop();
        process.drain_callbacks().await;
        let drained = served.load(Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(served.load(Ordering::SeqCst), drained);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_graceful_shutdown_lets_the_engine_finish_what_it_was_sent_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("engine");
        // Each notification makes a host call and waits for its answer, as the SDK does.
        std::fs::write(
            &executable,
            r#"#!/usr/bin/python3
import json, sys, time
init = json.loads(sys.stdin.readline())["init"]
print(json.dumps({"type": "ready", "api": 1, "extension": init["extension"], "version": "1.0.0"}), flush=True)
queued = []
while True:
    frame = queued.pop(0) if queued else json.loads(sys.stdin.readline() or "null")
    if frame is None or frame["type"] == "shutdown":
        sys.exit(0)
    if frame["type"] == "notify":
        time.sleep(0.01)
        print(json.dumps({"type": "host_call", "id": frame["params"], "method": "kv.set", "params": {}}), flush=True)
        while True:
            answer = json.loads(sys.stdin.readline())
            if answer["type"] == "host_result":
                break
            queued.append(answer)
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let served = Arc::new(AtomicU64::new(0));
        let process = Process::start(
            &executable,
            init("1.0.0"),
            &BTreeMap::new(),
            Arc::new(Count(served.clone())),
        )
        .await
        .unwrap();
        for n in 0..20 {
            process.notify("core.activity", Value::from(n)).unwrap();
        }
        let started = std::time::Instant::now();
        process.shutdown(std::time::Duration::from_secs(5)).await;
        assert_eq!(served.load(Ordering::SeqCst), 20);
        assert!(started.elapsed() < std::time::Duration::from_secs(4));
        assert!(!process.is_alive());
    }

    #[tokio::test]
    async fn an_engine_cannot_claim_a_version_different_from_its_verified_package() {
        let (_dir, executable) = engine("2.0.0");
        let result =
            Process::start(&executable, init("1.0.0"), &BTreeMap::new(), Arc::new(Echo)).await;
        assert!(matches!(result, Err(Error::Unavailable(_))));
    }
}
