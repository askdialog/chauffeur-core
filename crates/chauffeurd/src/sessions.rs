//! Logins a workspace's work needs, declared by its repos and the user: their checks and logins
//! (docs/adr/declared-sessions.md). Chauffeur runs the declared commands and reads only a check's
//! exit status and `expires_at=` line, and what a login prints: it never reads a credential.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chauffeur_protocol::client::{
    LoginSessionEnsureParams, LoginSessionInfo, LoginSessionLogin, LoginSessionState,
};
use chauffeur_protocol::events::{ChatEvent, WorkspaceEvent};
use chauffeur_protocol::model::{ChatInfo, ChatKind, ChatStatus};
use tokio::io::AsyncBufReadExt;

use crate::error::{Error, Result};
use crate::settings::{SessionDecl, SessionSpec, merge_sessions};
use crate::store::WorkspaceRow;
use crate::{Daemon, chat_shell};
use chauffeur_protocol::model::WorktreeOrigin;

/// How long a check may run.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// A check this recent is not run again by `session.list`.
const CHECK_FRESH: Duration = Duration::from_secs(60);
/// A login still running past this is stopped: it waits for a keyboard it will never get.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);
/// How long `session.login` keeps reading once the link appeared, for a device code printed after.
const AFTER_LINK: Duration = Duration::from_secs(2);
/// What a login printed, at most, shown with its link.
const LOGIN_OUTPUT_LINES: usize = 20;
/// How long a request to log in waits for the user.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);

type Key = (String, String);

#[derive(Default)]
pub struct Sessions {
    checked: Mutex<HashMap<Key, Checked>>,
    logging_in: Mutex<HashSet<Key>>,
    /// The open request of each session that does not hold.
    requests: Mutex<HashMap<Key, Arc<Request>>>,
    /// Woken at each check, for the requests waiting on one.
    checked_again: tokio::sync::Notify,
}

/// A request asking the user to log in to a session: open until the session holds, the user
/// declines, or nobody answers within `REQUEST_TIMEOUT`.
struct Request {
    id: String,
    declined: tokio::sync::Notify,
    outcome: tokio::sync::watch::Sender<Option<Outcome>>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Outcome {
    Holds,
    Declined,
    Unanswered,
}

#[derive(Clone)]
struct Checked {
    at: Instant,
    stamp: String,
    holds: bool,
    expires_at: Option<String>,
}

/// A session of a workspace, with where its commands run.
#[derive(Clone)]
struct Declared {
    name: String,
    spec: SessionSpec,
    repo: Option<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
}

impl Sessions {
    pub async fn list(&self, d: &Arc<Daemon>, workspace: &str) -> Result<Vec<LoginSessionInfo>> {
        let (row, declared) = declared(d, workspace)?;
        let checks = declared.iter().map(|session| async {
            let fresh = self
                .checked(&row.id, &session.name)
                .is_some_and(|checked| checked.at.elapsed() < CHECK_FRESH);
            if !fresh {
                self.run_check(d, &row, session).await;
            }
            self.info(&row.id, session)
        });
        Ok(futures_util::future::join_all(checks).await)
    }

    pub async fn check(
        &self,
        d: &Arc<Daemon>,
        workspace: &str,
        name: &str,
    ) -> Result<LoginSessionInfo> {
        let (row, declared) = declared(d, workspace)?;
        let session = find(&row, declared, name)?;
        self.run_check(d, &row, &session).await;
        Ok(self.info(&row.id, &session))
    }

    /// Starts the login and answers once it printed a link, or ended. It keeps running up to
    /// `LOGIN_TIMEOUT`, then the session is checked again.
    pub async fn login(
        self: &Arc<Self>,
        d: &Arc<Daemon>,
        workspace: &str,
        name: &str,
    ) -> Result<LoginSessionLogin> {
        let (row, declared) = declared(d, workspace)?;
        let session = find(&row, declared, name)?;
        let key = (row.id.clone(), session.name.clone());
        if !self.logging_in.lock().unwrap().insert(key.clone()) {
            return Err(Error::Conflict(format!(
                "a login of {} is already running",
                session.spec.label
            )));
        }
        let started = match Login::start(&session.spec.login, &session.cwd, &session.env) {
            Ok(started) => started,
            Err(error) => {
                self.logging_in.lock().unwrap().remove(&key);
                return Err(Error::Internal(error.context("starting the login")));
            }
        };
        self.publish(d, &row.id, &session);
        let (printed, finished) = started.first_words().await;
        let info = self.info(&row.id, &session);
        let sessions = self.clone();
        let daemon = d.clone();
        tokio::spawn(async move {
            finished.await;
            sessions.logging_in.lock().unwrap().remove(&key);
            sessions.run_check(&daemon, &row, &session).await;
        });
        Ok(LoginSessionLogin {
            session: info,
            url: printed.url,
            output: printed.lines.join("\n"),
        })
    }

    async fn run_check(&self, d: &Arc<Daemon>, row: &WorkspaceRow, session: &Declared) {
        let outcome = match chat_shell::spawn(&session.spec.check, &session.cwd, &session.env) {
            Ok(running) => running.wait(CHECK_TIMEOUT).await,
            Err(error) => Err(error),
        };
        let checked = match outcome {
            Ok(outcome) => Checked {
                at: Instant::now(),
                stamp: crate::store::now(),
                holds: outcome.exit_code == Some(0) && !outcome.timed_out,
                expires_at: expires_at(&outcome.output),
            },
            Err(error) => {
                tracing::warn!(%error, session = session.name, "could not run a session's check");
                Checked {
                    at: Instant::now(),
                    stamp: crate::store::now(),
                    holds: false,
                    expires_at: None,
                }
            }
        };
        self.checked
            .lock()
            .unwrap()
            .insert((row.id.clone(), session.name.clone()), checked);
        self.publish(d, &row.id, session);
        self.checked_again.notify_waiters();
    }

    fn holds(&self, workspace_id: &str, name: &str) -> bool {
        self.checked(workspace_id, name)
            .is_some_and(|checked| checked.holds)
    }

    /// Answers once the session holds, asking the user to log in when it does not.
    pub async fn ensure(
        self: &Arc<Self>,
        d: &Arc<Daemon>,
        params: LoginSessionEnsureParams,
    ) -> Result<LoginSessionInfo> {
        let (row, declared) = declared(d, &params.workspace)?;
        let session = find(&row, declared, &params.name)?;
        self.run_check(d, &row, &session).await;
        if self.holds(&row.id, &session.name) {
            return Ok(self.info(&row.id, &session));
        }
        let mut outcome = self.request(d, &row, &session, params.chat.as_deref())?;
        let outcome = loop {
            if let Some(outcome) = *outcome.borrow_and_update() {
                break outcome;
            }
            if outcome.changed().await.is_err() {
                break Outcome::Unanswered;
            }
        };
        let label = &session.spec.label;
        match outcome {
            Outcome::Holds => Ok(self.info(&row.id, &session)),
            Outcome::Declined => Err(Error::Conflict(format!(
                "the user declined to log in to {label}: stop what needed it, without working around it"
            ))),
            Outcome::Unanswered => Err(Error::Conflict(format!(
                "nobody logged in to {label} within 15 minutes: stop what needed it, and say so"
            ))),
        }
    }

    /// The session's open request, or a new one asked in `chat`.
    fn request(
        self: &Arc<Self>,
        d: &Arc<Daemon>,
        row: &WorkspaceRow,
        session: &Declared,
        chat: Option<&str>,
    ) -> Result<tokio::sync::watch::Receiver<Option<Outcome>>> {
        let key = (row.id.clone(), session.name.clone());
        let mut requests = self.requests.lock().unwrap();
        if let Some(open) = requests.get(&key) {
            return Ok(open.outcome.subscribe());
        }
        let chat = asked_in(d, row, chat)?;
        let (outcome, receiver) = tokio::sync::watch::channel(None);
        let request = Arc::new(Request {
            id: format!("session-{}", uuid::Uuid::new_v4()),
            declined: tokio::sync::Notify::new(),
            outcome,
        });
        requests.insert(key.clone(), request.clone());
        drop(requests);
        d.chats.on_event(
            &chat.id,
            ChatEvent::SessionRequest {
                request_id: request.id.clone(),
                session: session.name.clone(),
                label: session.spec.label.clone(),
            },
        );
        let (sessions, d, row, session) = (self.clone(), d.clone(), row.clone(), session.clone());
        tokio::spawn(async move {
            let outcome = sessions.wait_for(&d, &row, &session, &request).await;
            sessions.requests.lock().unwrap().remove(&key);
            let resolved = match outcome {
                Outcome::Holds => "answered",
                Outcome::Declined => "denied",
                Outcome::Unanswered => "cancelled",
            };
            d.chats.on_event(
                &chat.id,
                ChatEvent::RequestResolved {
                    request_id: request.id.clone(),
                    outcome: resolved.into(),
                    rules: None,
                },
            );
            request.outcome.send_replace(Some(outcome));
        });
        Ok(receiver)
    }

    /// Waits for the session to hold, checking it again each minute: the user may log in from
    /// a terminal of their own.
    async fn wait_for(
        &self,
        d: &Arc<Daemon>,
        row: &WorkspaceRow,
        session: &Declared,
        request: &Request,
    ) -> Outcome {
        let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
        loop {
            let checked = self.checked_again.notified();
            if self.holds(&row.id, &session.name) {
                return Outcome::Holds;
            }
            tokio::select! {
                _ = checked => {}
                _ = request.declined.notified() => return Outcome::Declined,
                _ = tokio::time::sleep(CHECK_FRESH) => self.run_check(d, row, session).await,
                _ = tokio::time::sleep_until(deadline) => return Outcome::Unanswered,
            }
        }
    }

    /// Whether a chat's request is a session request, which the daemon answers itself.
    pub fn is_request(&self, request_id: &str) -> bool {
        self.requests
            .lock()
            .unwrap()
            .values()
            .any(|request| request.id == request_id)
    }

    /// The user declined a session request; false when it is not open.
    pub fn decline(&self, request_id: &str) -> bool {
        let requests = self.requests.lock().unwrap();
        let Some(request) = requests.values().find(|request| request.id == request_id) else {
            return false;
        };
        request.declined.notify_one();
        true
    }

    fn checked(&self, workspace_id: &str, name: &str) -> Option<Checked> {
        self.checked
            .lock()
            .unwrap()
            .get(&(workspace_id.to_string(), name.to_string()))
            .cloned()
    }

    fn info(&self, workspace_id: &str, session: &Declared) -> LoginSessionInfo {
        let checked = self.checked(workspace_id, &session.name);
        let logging_in = self
            .logging_in
            .lock()
            .unwrap()
            .contains(&(workspace_id.to_string(), session.name.clone()));
        LoginSessionInfo {
            name: session.name.clone(),
            label: session.spec.label.clone(),
            repo: session.repo.clone(),
            state: match &checked {
                None => LoginSessionState::Unchecked,
                Some(checked) if checked.holds => LoginSessionState::Holds,
                Some(_) => LoginSessionState::Expired,
            },
            expires_at: checked
                .as_ref()
                .filter(|checked| checked.holds)
                .and_then(|checked| checked.expires_at.clone()),
            checked_at: checked.map(|checked| checked.stamp),
            logging_in,
        }
    }

    fn publish(&self, d: &Daemon, workspace_id: &str, session: &Declared) {
        d.events.workspace(
            workspace_id,
            WorkspaceEvent::SessionChanged {
                session: self.info(workspace_id, session),
            },
        );
    }
}

/// The workspace's sessions: each repo's, under the user's `[repos.<name>.sessions]`, then the
/// user's own `[sessions]` over all. A name two repos declare is one session, the first repo's.
fn declared(d: &Arc<Daemon>, workspace: &str) -> Result<(WorkspaceRow, Vec<Declared>)> {
    let row = d.workspaces.row(workspace)?;
    let settings = d.settings.get();
    let mut merged: BTreeMap<String, (SessionDecl, Option<String>, PathBuf, BTreeMap<_, _>)> =
        BTreeMap::new();
    for worktree in d.store.worktrees_of(&row.id)? {
        if worktree.removed || worktree.origin != WorktreeOrigin::Workspace {
            continue;
        }
        let mut sessions = d
            .workspaces
            .repo_settings(
                &worktree.repo,
                Path::new(&worktree.clone_path),
                Path::new(&worktree.path),
            )?
            .sessions;
        if let Some(user) = settings.repos.get(&worktree.repo) {
            merge_sessions(&mut sessions, &user.sessions);
        }
        for (name, decl) in sessions {
            if merged.contains_key(&name) {
                continue;
            }
            let env = env_of(d, d.workspaces.script_env(&row, &worktree)?);
            merged.insert(
                name,
                (
                    decl,
                    Some(worktree.repo.clone()),
                    PathBuf::from(&worktree.path),
                    env,
                ),
            );
        }
    }
    for (name, decl) in &settings.sessions {
        match merged.get_mut(name) {
            Some(entry) => entry.0.merge(decl),
            None => {
                let env = env_of(d, d.workspaces.agent_env(&row, false)?);
                merged.insert(
                    name.clone(),
                    (decl.clone(), None, PathBuf::from(&row.path), env),
                );
            }
        }
    }
    let declared = merged
        .into_iter()
        .filter_map(|(name, (decl, repo, cwd, env))| {
            Some(Declared {
                name,
                spec: decl.spec()?,
                repo,
                cwd,
                env,
            })
        })
        .collect();
    Ok((row, declared))
}

/// The commands start from the login shell's environment, as scripts do.
fn env_of(d: &Daemon, own: BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut env = d.shell_env.vars();
    env.extend(own);
    env
}

/// The chat a request is asked in: the caller's, or the workspace's chat that waits on nothing
/// and was active last.
fn asked_in(d: &Daemon, row: &WorkspaceRow, chat: Option<&str>) -> Result<ChatInfo> {
    if let Some(key) = chat {
        let chat = d.chats.resolve(key)?;
        if chat.workspace_id != row.id {
            return Err(Error::BadRequest(format!(
                "chat {key} is not a chat of {}",
                row.slug
            )));
        }
        return Ok(chat);
    }
    d.store
        .chats_of(&row.id)?
        .into_iter()
        .filter(|chat| {
            !chat.closed && chat.kind == ChatKind::Chat && chat.status != ChatStatus::NeedsInput
        })
        .max_by(|a, b| {
            let at = |chat: &ChatInfo| {
                chat.last_activity_at
                    .clone()
                    .unwrap_or(chat.created_at.clone())
            };
            at(a).cmp(&at(b))
        })
        .ok_or_else(|| {
            Error::Conflict(format!(
                "{} has no chat to ask the user in: log in with chauffeur session login",
                row.slug
            ))
        })
}

fn find(row: &WorkspaceRow, declared: Vec<Declared>, name: &str) -> Result<Declared> {
    declared
        .into_iter()
        .find(|session| session.name == name)
        .ok_or_else(|| Error::not_found(format!("session {name} in {}", row.slug)))
}

/// A check's last `expires_at=<RFC 3339>` line, when it is a valid time.
fn expires_at(output: &str) -> Option<String> {
    let value = output
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("expires_at="))?;
    chrono::DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|time| time.to_rfc3339())
}

/// The first `https://` link of a line.
fn link(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let link: String = line[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '<' | '>'))
        .collect();
    Some(link.trim_end_matches(['.', ',', ';', ')', ']']).to_string())
}

#[derive(Default)]
struct Printed {
    url: Option<String>,
    lines: Vec<String>,
}

/// A running login: its lines as they come, and its end.
struct Login {
    lines: tokio::sync::mpsc::UnboundedReceiver<String>,
    child: tokio::process::Child,
}

impl Login {
    fn start(command: &str, cwd: &Path, env: &BTreeMap<String, String>) -> anyhow::Result<Login> {
        let (reader, writer) = std::io::pipe()?;
        let child = tokio::process::Command::new(crate::shellenv::script_shell())
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .env_clear()
            .envs(env)
            .stdin(Stdio::null())
            .stdout(writer.try_clone()?)
            .stderr(writer)
            .process_group(0)
            .kill_on_drop(true)
            .spawn()?;
        let (send, lines) = tokio::sync::mpsc::unbounded_channel();
        let pipe = tokio::net::unix::pipe::Receiver::from_owned_fd(reader.into())?;
        tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(pipe).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Login { lines, child })
    }

    /// What it printed until its link and a moment after, or until it ended; then a future that
    /// ends with it, stopping it past `LOGIN_TIMEOUT`. Its output is never logged: a device code
    /// is a secret until used.
    async fn first_words(mut self) -> (Printed, impl Future<Output = ()> + Send + 'static) {
        let deadline = tokio::time::Instant::now() + LOGIN_TIMEOUT;
        let mut printed = Printed::default();
        let mut until = deadline;
        loop {
            let line = tokio::select! {
                line = self.lines.recv() => line,
                _ = tokio::time::sleep_until(until) => break,
            };
            let Some(line) = line else { break };
            if printed.url.is_none()
                && let Some(url) = link(&line)
            {
                printed.url = Some(url);
                until = (tokio::time::Instant::now() + AFTER_LINK).min(deadline);
            }
            if printed.lines.len() < LOGIN_OUTPUT_LINES {
                printed.lines.push(line);
            }
        }
        let mut child = self.child;
        let finished = async move {
            let pgid = child.id().map(|pid| pid as i32);
            let ended = tokio::time::timeout_at(deadline, child.wait())
                .await
                .is_ok();
            if let Some(pgid) = pgid {
                // SAFETY: killpg only sends a signal, to the login's own group.
                unsafe {
                    libc::killpg(pgid, libc::SIGKILL);
                }
            }
            if !ended {
                let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            }
        };
        (printed, finished)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_says_when_the_session_ends_on_its_last_line() {
        assert_eq!(
            expires_at("ok\nexpires_at=2026-11-18T20:00:00Z\n"),
            Some("2026-11-18T20:00:00+00:00".into())
        );
        assert_eq!(expires_at("expires_at=tomorrow"), None);
        assert_eq!(expires_at("nothing"), None);
    }

    #[test]
    fn a_login_link_is_taken_without_its_punctuation() {
        assert_eq!(
            link("Open https://device.sso.example/?user_code=ABCD-EFGH."),
            Some("https://device.sso.example/?user_code=ABCD-EFGH".into())
        );
        assert_eq!(
            link("Authorize: \"https://app.example/cli\""),
            Some("https://app.example/cli".into())
        );
        assert_eq!(link("http://insecure.example"), None);
    }

    #[tokio::test]
    async fn a_login_answers_with_its_link_and_what_follows_it() {
        let dir = tempfile::tempdir().unwrap();
        let login = Login::start(
            "echo 'Open https://sso.example/device'; echo 'Code: WXYZ'; sleep 5",
            dir.path(),
            &std::env::vars().collect(),
        )
        .unwrap();
        let started = Instant::now();
        let (printed, finished) = login.first_words().await;
        assert_eq!(printed.url.as_deref(), Some("https://sso.example/device"));
        assert_eq!(
            printed.lines,
            ["Open https://sso.example/device", "Code: WXYZ"]
        );
        assert!(started.elapsed() < Duration::from_secs(4));
        drop(finished);
    }
}
