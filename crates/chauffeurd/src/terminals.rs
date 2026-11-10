//! Terminals of a workspace: a login shell in a pseudo-terminal owned by the daemon, so it
//! survives the app closing. Output is fanned out as base64 events and kept as scrollback for
//! clients that attach later. Whether a command runs in a terminal is read from its foreground
//! process group, without shell integration: no prompt marks, so no exit code of a command. A
//! terminal keeps the name the user gave its tab for as long as it lives; its current folder is
//! read from its shell's process on demand, never followed.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use chauffeur_protocol::events::WorkspaceEvent;
use chauffeur_protocol::model::TerminalInfo;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::events::EventBus;
use crate::{Error, Result};

const SCROLLBACK_BYTES: usize = 1 << 20;
/// A shell says nothing when a command starts or ends, and a command may print nothing: the
/// foreground is polled, cheaply (one `tcgetpgrp` per open terminal).
const ACTIVITY_POLL: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct TerminalManager {
    events: EventBus,
    execution: crate::workspace_execution::ExecutionGate,
    terminals: Arc<Mutex<HashMap<String, Arc<Terminal>>>>,
}

struct Terminal {
    info: Mutex<TerminalInfo>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    /// The shell's process, whose current folder `cwd` reads.
    pid: Option<u32>,
    exited: tokio::sync::watch::Receiver<Option<std::result::Result<(), String>>>,
    /// Scrollback and the total number of bytes ever output, updated together.
    scrollback: Mutex<(VecDeque<u8>, u64)>,
    /// Its tab's place once one of its workspace's was moved (`move_tab`); those without one
    /// come after, by creation.
    tab_position: Mutex<Option<u32>>,
}

impl Terminal {
    fn request_stop(&self) -> Result<()> {
        let mut child = self.child.lock().unwrap();
        if child.try_wait()?.is_none() {
            child.kill().map_err(|error| {
                Error::Unavailable(format!(
                    "Could not stop terminal ({error}); keep the workspace fenced and retry"
                ))
            })?;
        }
        Ok(())
    }
}

pub struct OpenSpec {
    pub workspace_id: String,
    pub cwd: PathBuf,
    pub title: String,
    pub cols: u16,
    pub rows: u16,
    pub command: Option<String>,
    pub env: BTreeMap<String, String>,
}

impl TerminalManager {
    pub fn new(
        events: EventBus,
        execution: crate::workspace_execution::ExecutionGate,
    ) -> TerminalManager {
        TerminalManager {
            events,
            execution,
            terminals: Arc::default(),
        }
    }

    pub fn open(&self, spec: OpenSpec) -> Result<TerminalInfo> {
        let _admission = self.execution.admit(&spec.workspace_id)?;
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: spec.rows.max(2),
                cols: spec.cols.max(2),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Internal(anyhow::anyhow!("opening a pty: {e}")))?;
        let shell = spec
            .env
            .get("SHELL")
            .cloned()
            .unwrap_or_else(crate::shellenv::login_shell);
        let mut command = match &spec.command {
            Some(line) => {
                let mut c = CommandBuilder::new(&shell);
                c.args(["-l", "-c", line]);
                c
            }
            None => {
                let mut c = CommandBuilder::new(&shell);
                c.arg("-l");
                c
            }
        };
        command.cwd(&spec.cwd);
        command.env_clear();
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        if !spec.env.contains_key("LANG") {
            command.env("LANG", "en_US.UTF-8");
        }
        let child = pty
            .slave
            .spawn_command(command)
            .map_err(|e| Error::Internal(anyhow::anyhow!("starting the shell: {e}")))?;
        let pid = child.process_id();
        // The shell leads its own session, so its process group is its PID.
        let shell_group = pid.map(|pid| pid as libc::pid_t);
        drop(pty.slave);
        let reader = pty
            .master
            .try_clone_reader()
            .map_err(|e| Error::Internal(anyhow::anyhow!("{e}")))?;
        let writer = pty
            .master
            .take_writer()
            .map_err(|e| Error::Internal(anyhow::anyhow!("{e}")))?;
        let id = uuid::Uuid::now_v7().to_string();
        let info = TerminalInfo {
            id: id.clone(),
            workspace_id: spec.workspace_id.clone(),
            cwd: spec.cwd.to_string_lossy().into_owned(),
            title: spec.title,
            name: None,
            cols: spec.cols,
            rows: spec.rows,
            running: true,
            exit_code: None,
            command_running: false,
            command_finished_at: None,
        };
        let (exit_tx, exit_rx) = tokio::sync::watch::channel(None);
        let terminal = Arc::new(Terminal {
            info: Mutex::new(info.clone()),
            writer: Mutex::new(writer),
            master: Mutex::new(pty.master),
            child: Mutex::new(child),
            pid,
            exited: exit_rx,
            scrollback: Mutex::new((VecDeque::new(), 0)),
            tab_position: Mutex::new(None),
        });
        self.terminals
            .lock()
            .unwrap()
            .insert(id.clone(), terminal.clone());

        // Reader thread: blocking reads, batched into events every ~16 ms.
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buffer = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buffer[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let events = self.events.clone();
        let workspace_id = spec.workspace_id.clone();
        let batch_terminal = terminal.clone();
        let terminal_id = id.clone();
        std::thread::spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut chunk = first;
                std::thread::sleep(Duration::from_millis(16));
                while let Ok(more) = rx.try_recv() {
                    chunk.extend(more);
                }
                // Published under the lock: a snapshot never races a chunk's offset.
                let mut scrollback = batch_terminal.scrollback.lock().unwrap();
                let offset = scrollback.1;
                scrollback.0.extend(&chunk);
                scrollback.1 += chunk.len() as u64;
                let excess = scrollback.0.len().saturating_sub(SCROLLBACK_BYTES);
                scrollback.0.drain(..excess);
                events.workspace(
                    &workspace_id,
                    WorkspaceEvent::TerminalOutput {
                        terminal_id: terminal_id.clone(),
                        data: base64::engine::general_purpose::STANDARD.encode(&chunk),
                        offset,
                    },
                );
                drop(scrollback);
            }
        });
        let events = self.events.clone();
        let workspace_id = spec.workspace_id.clone();
        let activity_terminal = terminal.clone();
        let terminal_id = id.clone();
        std::thread::spawn(move || {
            let mut previous = None;
            while activity_terminal.exited.borrow().is_none() {
                std::thread::sleep(ACTIVITY_POLL);
                let foreground = activity_terminal
                    .master
                    .lock()
                    .unwrap()
                    .process_group_leader();
                let running = command_running(shell_group, previous, foreground);
                previous = foreground;
                {
                    let mut info = activity_terminal.info.lock().unwrap();
                    if !info.running || info.command_running == running {
                        continue;
                    }
                    info.command_running = running;
                    if !running {
                        info.command_finished_at = Some(crate::store::now());
                    }
                }
                events.workspace(
                    &workspace_id,
                    WorkspaceEvent::TerminalActivity {
                        terminal_id: terminal_id.clone(),
                        command_running: running,
                    },
                );
            }
        });
        let events = self.events.clone();
        let workspace_id = spec.workspace_id;
        let exit_terminal = terminal;
        let terminal_id = id;
        std::thread::spawn(move || {
            let code = loop {
                // Reaping and signalling share a lock, so a historical PID is never killed.
                let outcome = exit_terminal.child.lock().unwrap().try_wait();
                match outcome {
                    Ok(Some(status)) => break Some(status.exit_code() as i32),
                    Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                    Err(error) => {
                        tracing::warn!(%error, terminal_id, "could not confirm terminal exit");
                        let _ = exit_tx.send(Some(Err(error.to_string())));
                        return;
                    }
                }
            };
            {
                let mut info = exit_terminal.info.lock().unwrap();
                info.running = false;
                info.exit_code = code;
                info.command_running = false;
            }
            let _ = exit_tx.send(Some(Ok(())));
            events.workspace(
                &workspace_id,
                WorkspaceEvent::TerminalExited {
                    terminal_id,
                    exit_code: code,
                },
            );
        });
        Ok(info)
    }

    fn get(&self, id: &str) -> Result<Arc<Terminal>> {
        self.terminals
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| Error::not_found(format!("terminal {id}")))
    }

    /// The workspace of a terminal this daemon runs, if it runs it.
    pub fn workspace_of(&self, id: &str) -> Option<String> {
        let terminal = self.get(id).ok()?;
        let workspace = terminal.info.lock().unwrap().workspace_id.clone();
        Some(workspace)
    }

    pub fn input(&self, id: &str, data: &str) -> Result<()> {
        let terminal = self.get(id)?;
        let workspace = terminal.info.lock().unwrap().workspace_id.clone();
        let _admission = self.execution.admit(&workspace)?;
        let mut writer = terminal.writer.lock().unwrap();
        writer.write_all(data.as_bytes())?;
        writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<TerminalInfo> {
        let terminal = self.get(id)?;
        terminal
            .master
            .lock()
            .unwrap()
            .resize(PtySize {
                rows: rows.max(2),
                cols: cols.max(2),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Internal(anyhow::anyhow!("{e}")))?;
        let mut info = terminal.info.lock().unwrap();
        info.cols = cols;
        info.rows = rows;
        Ok(info.clone())
    }

    /// Names a terminal's tab, its spaces collapsed and its control and bidi characters left out (a
    /// right-to-left override would turn the tab's text around); empty gives back its title.
    pub fn rename(&self, id: &str, name: &str) -> Result<TerminalInfo> {
        let terminal = self.get(id)?;
        let shown: String = name
            .chars()
            .filter(|c| c.is_whitespace() || !(c.is_control() || is_bidi_control(*c)))
            .collect();
        let name: String = shown
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(200)
            .collect();
        let mut info = terminal.info.lock().unwrap();
        info.name = (!name.is_empty()).then_some(name);
        Ok(info.clone())
    }

    /// The folder a terminal's shell is in now; the one it started in once the shell exited or
    /// when the system does not say.
    pub fn cwd(&self, id: &str) -> Result<PathBuf> {
        let terminal = self.get(id)?;
        let info = terminal.info.lock().unwrap().clone();
        let now = terminal.pid.filter(|_| info.running).and_then(process_cwd);
        Ok(now.unwrap_or_else(|| PathBuf::from(info.cwd)))
    }

    pub fn close(&self, id: &str) -> Result<TerminalInfo> {
        let terminal = self.get(id)?;
        let workspace = terminal.info.lock().unwrap().workspace_id.clone();
        // Hold admission through removal so installing a fence must drain this close.
        // Stops remain usable under a fence, but only checked reaping may forget evidence.
        let admission = self.execution.admit(&workspace).ok();
        terminal.request_stop()?;
        if admission.is_some() {
            self.terminals.lock().unwrap().remove(id);
        }
        let info = terminal.info.lock().unwrap().clone();
        Ok(info)
    }

    /// Remove a terminal only after its child was successfully reaped. Errors/timeouts keep
    /// its handle available for retry. A terminal child exit says nothing about detached jobs.
    pub async fn close_and_wait(&self, id: &str, timeout: Duration) -> Result<TerminalInfo> {
        let terminal = self.get(id)?;
        let mut exited = terminal.exited.clone();
        if exited.borrow().is_none() {
            let stopping = terminal.clone();
            tokio::task::spawn_blocking(move || stopping.request_stop())
                .await
                .map_err(|error| Error::Internal(error.into()))??;
        }
        let outcome = tokio::time::timeout(timeout, exited.wait_for(|exit| exit.is_some()))
            .await
            .map_err(|_| {
                Error::Unavailable(format!(
                    "Terminal {id} did not confirm exit; keep the workspace fenced and retry"
                ))
            })?
            .map_err(|_| {
                Error::Unavailable(format!(
                    "Terminal {id} lost its exit watcher; reconcile it before transferring"
                ))
            })?
            .clone();
        match outcome {
            Some(Ok(())) => {
                let info = terminal.info.lock().unwrap().clone();
                self.terminals.lock().unwrap().remove(id);
                Ok(info)
            }
            Some(Err(error)) => Err(Error::Unavailable(format!(
                "Terminal {id} exit could not be verified ({error}); reconcile it before transferring"
            ))),
            None => Err(Error::Unavailable(format!(
                "Terminal {id} has no exit confirmation; reconcile it before transferring"
            ))),
        }
    }

    pub async fn close_workspace_and_wait(&self, workspace: &str, timeout: Duration) -> Result<()> {
        let terminals = self.list(workspace);
        let results = futures_util::future::join_all(
            terminals
                .iter()
                .map(|t| self.close_and_wait(&t.id, timeout)),
        )
        .await;
        for result in results {
            result?;
        }
        Ok(())
    }

    /// A workspace's terminals in the order of their tabs.
    pub fn list(&self, workspace_id: &str) -> Vec<TerminalInfo> {
        self.ordered(workspace_id)
            .into_iter()
            .map(|t| t.info.lock().unwrap().clone())
            .collect()
    }

    fn ordered(&self, workspace_id: &str) -> Vec<Arc<Terminal>> {
        let mut list: Vec<(Option<u32>, String, Arc<Terminal>)> = self
            .terminals
            .lock()
            .unwrap()
            .values()
            .filter_map(|t| {
                let info = t.info.lock().unwrap();
                (info.workspace_id == workspace_id)
                    .then(|| (*t.tab_position.lock().unwrap(), info.id.clone(), t.clone()))
            })
            .collect();
        // Ids are UUIDv7: by creation.
        list.sort_by(|a, b| (a.0.unwrap_or(u32::MAX), &a.1).cmp(&(b.0.unwrap_or(u32::MAX), &b.1)));
        list.into_iter().map(|(_, _, t)| t).collect()
    }

    /// Moves a terminal's tab before another terminal of its workspace, or last; returns the
    /// workspace's terminals in their new order.
    pub fn move_tab(&self, id: &str, before: Option<&str>) -> Result<Vec<TerminalInfo>> {
        let workspace = self.get(id)?.info.lock().unwrap().workspace_id.clone();
        let terminals = self.ordered(&workspace);
        let ids: Vec<String> = terminals
            .iter()
            .map(|t| t.info.lock().unwrap().id.clone())
            .collect();
        if let Some(before) = before
            && !ids.iter().any(|other| other == before)
        {
            return Err(Error::not_found(format!(
                "terminal {before} among the terminals of its workspace"
            )));
        }
        let order = crate::chats::moved_before(ids.clone(), id, before);
        for (terminal, id) in terminals.iter().zip(&ids) {
            let at = order.iter().position(|other| other == id);
            *terminal.tab_position.lock().unwrap() = at.map(|i| i as u32);
        }
        Ok(self.list(&workspace))
    }

    /// The terminal, its scrollback (base64) and the output offset right after it.
    pub fn snapshot(&self, id: &str) -> Result<(TerminalInfo, String, u64)> {
        let terminal = self.get(id)?;
        let (bytes, end) = {
            let scrollback = terminal.scrollback.lock().unwrap();
            (
                scrollback.0.iter().copied().collect::<Vec<u8>>(),
                scrollback.1,
            )
        };
        let info = terminal.info.lock().unwrap().clone();
        Ok((
            info,
            base64::engine::general_purpose::STANDARD.encode(bytes),
            end,
        ))
    }

    pub fn close_workspace(&self, workspace_id: &str) {
        let ids: Vec<String> = self.list(workspace_id).into_iter().map(|t| t.id).collect();
        for id in ids {
            let _ = self.close(&id);
        }
    }
}

/// A command runs when the terminal's foreground process group is not the shell's, and the same
/// at two polls in a row: what a prompt runs itself (a git status) never counts.
fn command_running(
    shell: Option<libc::pid_t>,
    previous: Option<libc::pid_t>,
    foreground: Option<libc::pid_t>,
) -> bool {
    foreground.is_some() && foreground != shell && foreground == previous
}

/// Unicode's marks, embeddings, overrides and isolates of text direction (UAX #9), which
/// `char::is_control` does not count.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// The current folder of process `pid`, if the system tells it and it still exists: Linux reads a
/// removed folder as "/path (deleted)".
fn process_cwd(pid: u32) -> Option<PathBuf> {
    crate::workspace_processes::working_directory(pid)
        .filter(|path| path.is_absolute() && path.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn manager() -> TerminalManager {
        let store = crate::store::Store::open_in_memory().unwrap();
        store
            .insert_workspace(&crate::store::WorkspaceRow {
                id: "w1".into(),
                slug: "w1".into(),
                path: "/nowhere".into(),
                branch: "fixture".into(),
                parent_id: None,
                state: chauffeur_protocol::model::WorkspaceState::Ready,
                tickets: vec![],
                created_at: crate::store::now(),
                archived_at: None,
                last_error: None,
                attention_at: None,
                seen_at: None,
                explore: false,
                title: None,
                auto_title: false,
                title_source: None,
                pinned: false,
                position: 0.0,
                section_id: None,
                house_position: None,
            })
            .unwrap();
        TerminalManager::new(
            EventBus::new(store.clone()),
            crate::workspace_execution::ExecutionGate::new(store),
        )
    }

    fn spec(cwd: &Path, command: &str) -> OpenSpec {
        OpenSpec {
            workspace_id: "w1".into(),
            cwd: cwd.to_path_buf(),
            title: "web".into(),
            cols: 80,
            rows: 24,
            command: Some(command.into()),
            env: BTreeMap::from([("PATH".into(), std::env::var("PATH").unwrap_or_default())]),
        }
    }

    #[test]
    fn a_command_runs_once_another_group_holds_the_foreground_for_two_polls() {
        let shell = Some(100);
        assert!(!command_running(shell, None, shell), "the prompt");
        assert!(!command_running(shell, shell, Some(200)), "seen once");
        assert!(command_running(shell, Some(200), Some(200)));
        assert!(
            !command_running(shell, Some(200), shell),
            "back to the prompt"
        );
        assert!(!command_running(shell, Some(200), Some(300)), "another one");
        assert!(!command_running(shell, None, None), "unknown foreground");
    }

    #[test]
    fn a_process_s_current_folder_is_read_from_the_system() {
        let folder = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .current_dir(folder.path())
            .spawn()
            .unwrap();
        let cwd = process_cwd(child.id());
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(
            cwd.map(|path| path.canonicalize().unwrap()),
            Some(folder.path().canonicalize().unwrap())
        );
    }

    #[test]
    fn a_shell_in_a_removed_folder_falls_back_on_where_it_started() {
        let start = tempfile::tempdir().unwrap();
        let removed = tempfile::tempdir().unwrap();
        let terminals = manager();
        let command = format!("cd '{}' && sleep 30", removed.path().display());
        let info = terminals.open(spec(start.path(), &command)).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let inside = removed.path().canonicalize().unwrap();
        while terminals.cwd(&info.id).unwrap().canonicalize().ok() != Some(inside.clone()) {
            assert!(
                std::time::Instant::now() < deadline,
                "the shell never reached {}",
                inside.display()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        removed.close().unwrap();
        assert_eq!(terminals.cwd(&info.id).unwrap(), start.path());
        terminals.close(&info.id).unwrap();
    }

    #[test]
    fn a_terminal_follows_its_shell_into_another_folder_and_falls_back_on_where_it_started() {
        let start = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let terminals = manager();
        let command = format!("cd '{}' && sleep 30", elsewhere.path().display());
        let info = terminals.open(spec(start.path(), &command)).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let moved = elsewhere.path().canonicalize().unwrap();
        while terminals.cwd(&info.id).unwrap().canonicalize().ok() != Some(moved.clone()) {
            assert!(
                std::time::Instant::now() < deadline,
                "the shell never reached {}",
                moved.display()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        terminals.close(&info.id).unwrap();

        // Exited: where it started.
        let done = terminals.open(spec(start.path(), "true")).unwrap();
        while terminals
            .list("w1")
            .iter()
            .any(|t| t.id == done.id && t.running)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the shell never exited"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(terminals.cwd(&done.id).unwrap(), start.path());
        assert!(terminals.cwd("nope").is_err());
    }

    #[test]
    fn a_terminal_keeps_the_name_its_tab_was_given_without_control_characters_and_empty_gives_back_its_title()
     {
        let folder = tempfile::tempdir().unwrap();
        let terminals = manager();
        let info = terminals.open(spec(folder.path(), "sleep 30")).unwrap();
        assert_eq!(info.name, None);

        let renamed = terminals.rename(&info.id, "  dev   server ").unwrap();
        assert_eq!(renamed.name.as_deref(), Some("dev server"));
        assert_eq!(renamed.title, "web");
        assert_eq!(terminals.list("w1")[0].name.as_deref(), Some("dev server"));
        assert_eq!(
            terminals.snapshot(&info.id).unwrap().0.name.as_deref(),
            Some("dev server")
        );

        let cleaned = terminals
            .rename(&info.id, "dev\u{1b}[31m\tserver\u{202E}lmth.exe\u{7}")
            .unwrap();
        assert_eq!(cleaned.name.as_deref(), Some("dev[31m serverlmth.exe"));
        assert_eq!(
            terminals.rename(&info.id, "\u{2066}\u{0}").unwrap().name,
            None
        );
        assert_eq!(terminals.rename(&info.id, "   ").unwrap().name, None);
        assert!(terminals.rename("nope", "x").is_err());
        terminals.close(&info.id).unwrap();
    }
}
