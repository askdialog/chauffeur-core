//! Keeping the machine awake while agents work (`keep_awake` in `settings.toml`).
//!
//! - `never`: nothing; the machine sleeps as it would without Chauffeur.
//! - `while_working` (the default): while an agent works, and [`LINGER`] after, the daemon holds a
//!   power assertion against idle system sleep. On macOS a `caffeinate -i -w <daemon pid>` child:
//!   simpler than IOKit through FFI, visible in `pmset -g assertions`, and `-w` makes it exit with
//!   the daemon, a crash or a kill included, so the assertion never outlives it. On Linux
//!   `systemd-inhibit --what=idle:sleep … tail --pid=<daemon pid> -f /dev/null`, when logind is
//!   there (logged once otherwise). The display may sleep; closing a MacBook's lid still sleeps it.
//! - `lid_closed` (macOS): the same assertion, and `sudo -n /usr/bin/pmset -a disablesleep 1` (no
//!   sleep at all, the lid closed included), allowed by a sudoers rule installed once behind an
//!   administrator prompt (`install_rule`). Sleep comes back (`disablesleep 0`) when the work stops,
//!   when the daemon stops, at the next start after a crash (a marker file in the state folder says
//!   it was left on), and on battery below `keep_awake_min_battery` (back on above it or on AC).
//!   When `sudo -n` fails (the rule is missing), the daemon holds the assertion only and says so.
//!
//! "An agent works": a chat has a turn running (starting or working; one waiting for the user's
//! answer does not count), background tasks of its agent run, or messages wait for its workspace's
//! creation to end (a queue left after an interrupt waits for the user, so it does not count); a
//! workspace being created or transferred counts too.

use std::collections::HashSet;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chauffeur_protocol::model::{ChatStatus, KeepAwake, PowerHold, PowerState};

use crate::Daemon;

/// The hold is kept this long after the last work ended: an agent's next turn, a queued message
/// or a child workspace's agent often follows within a minute.
pub const LINGER: Duration = Duration::from_secs(120);
/// How often the daemon looks at what its agents do.
const TICK: Duration = Duration::from_secs(5);
/// How often the battery is read in lid-closed mode (`pmset -g batt`, read-only).
const BATTERY_EVERY: Duration = Duration::from_secs(60);
/// The assertion process is not spawned again sooner than this after it exited on its own.
const RESPAWN_AFTER: Duration = Duration::from_secs(60);
/// The sudoers rule names this path: the commands run through sudo must use it.
pub const PMSET: &str = "/usr/bin/pmset";
/// The file in the sudoers folder.
pub const SUDOERS_FILE: &str = "chauffeur-pmset";
/// Where the rule is written before `visudo` checks it; a name with a dot, which sudo's
/// `#includedir` skips, should it be left behind.
const SUDOERS_STAGING: &str = ".chauffeur-pmset.new";
/// In the state folder: the daemon turned sleep off and has not turned it back on.
const MARKER: &str = "power-disablesleep";

// --- what the agents do ---

/// One chat, as far as keeping awake goes.
#[derive(Clone, Copy, Debug)]
pub struct ChatWork {
    pub status: ChatStatus,
    pub tasks: u32,
    /// Messages wait in its queue.
    pub queued: bool,
    /// Its workspace is being created.
    pub creating: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work {
    /// Chats whose agent works.
    pub chats: u32,
    pub tasks: u32,
    pub creating: u32,
    pub transfers: u32,
}

impl Work {
    pub fn busy(&self) -> bool {
        self.chats > 0 || self.creating > 0 || self.transfers > 0
    }

    /// The tooltip's reason: "2 agents working", "1 workspace being created".
    pub fn reason(&self) -> String {
        let plural =
            |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut parts = Vec::new();
        if self.chats > 0 {
            parts.push(plural(self.chats, "agent working", "agents working"));
        }
        if self.creating > 0 {
            parts.push(plural(
                self.creating,
                "workspace being created",
                "workspaces being created",
            ));
        }
        if self.transfers > 0 {
            parts.push(plural(
                self.transfers,
                "workspace transferring",
                "workspaces transferring",
            ));
        }
        parts.join(", ")
    }
}

pub fn work_of(chats: impl IntoIterator<Item = ChatWork>, creating_workspaces: u32) -> Work {
    let mut work = Work {
        creating: creating_workspaces,
        ..Default::default()
    };
    for chat in chats {
        work.tasks += chat.tasks;
        let turn = matches!(chat.status, ChatStatus::Starting | ChatStatus::Working);
        if turn || chat.tasks > 0 || (chat.queued && chat.creating) {
            work.chats += 1;
        }
    }
    work
}

/// What the daemon's agents do now. Read every few seconds: only the chats that may be at work.
pub fn gather(d: &Daemon) -> Work {
    let (chats, creating) = d.store.chats_at_work().unwrap_or_default();
    let creating: HashSet<String> = creating.into_iter().collect();
    let mut tasks = d.chats.tasks_by_chat();
    let mut work: Vec<ChatWork> = chats
        .iter()
        .map(|chat| {
            let creating = creating.contains(&chat.workspace_id);
            ChatWork {
                status: chat.status,
                tasks: tasks.remove(&chat.id).unwrap_or(0),
                // Only read where it matters: elsewhere a busy chat counts anyway.
                queued: creating && d.store.queue_of(&chat.id).is_ok_and(|q| !q.is_empty()),
                creating,
            }
        })
        .collect();
    // An idle chat counts for its tasks alone.
    work.extend(tasks.into_values().map(|tasks| ChatWork {
        status: ChatStatus::Idle,
        tasks,
        queued: false,
        creating: false,
    }));
    Work {
        transfers: d.transfers.active_count(),
        ..work_of(work, creating.len() as u32)
    }
}

// --- debounce ---

/// Holds on for `linger` after the work ended.
#[derive(Debug)]
pub struct Linger {
    linger: Duration,
    last_busy: Option<Instant>,
}

impl Linger {
    pub fn new(linger: Duration) -> Linger {
        Linger {
            linger,
            last_busy: None,
        }
    }

    /// Whether to hold now, and, once the work ended, for how long still.
    pub fn update(&mut self, busy: bool, now: Instant) -> (bool, Option<Duration>) {
        if busy {
            self.last_busy = Some(now);
            return (true, None);
        }
        match self.last_busy {
            Some(at) if now.saturating_duration_since(at) < self.linger => {
                (true, Some(self.linger - now.saturating_duration_since(at)))
            }
            _ => {
                self.last_busy = None;
                (false, None)
            }
        }
    }

    pub fn reset(&mut self) {
        self.last_busy = None;
    }
}

// --- the tools, injectable for tests ---

/// What runs the administrator's part (installing and removing the sudoers rule).
#[derive(Clone, Debug)]
pub enum Admin {
    /// `osascript -e 'do shell script "…" with administrator privileges'`: macOS's own password
    /// prompt, shown in the user's session.
    Osascript(PathBuf),
    /// Runs the script with `sh`, as the daemon's user: tests, with a temporary folder standing in
    /// for `/etc/sudoers.d`.
    Shell,
}

#[derive(Clone, Debug)]
pub struct Tools {
    /// The program holding the assertion: `caffeinate` on macOS, `systemd-inhibit` on Linux;
    /// none when the system has neither.
    pub inhibitor: Option<Inhibitor>,
    pub sudo: PathBuf,
    /// For `pmset -g batt` (read-only). What runs through sudo always names [`PMSET`].
    pub pmset: PathBuf,
    pub admin: Admin,
    pub visudo: PathBuf,
    pub chown: PathBuf,
    pub sudoers_dir: PathBuf,
    /// `lid_closed` is offered (macOS).
    pub lid_closed: bool,
}

#[derive(Clone, Debug)]
pub enum Inhibitor {
    Caffeinate(PathBuf),
    SystemdInhibit(PathBuf),
}

impl Tools {
    pub fn system() -> Tools {
        let macos = cfg!(target_os = "macos");
        let inhibitor = if macos {
            Some(Inhibitor::Caffeinate("/usr/bin/caffeinate".into()))
        } else {
            ["/usr/bin/systemd-inhibit", "/bin/systemd-inhibit"]
                .into_iter()
                .map(PathBuf::from)
                .find(|path| path.exists())
                .map(Inhibitor::SystemdInhibit)
        };
        Tools {
            inhibitor,
            sudo: "/usr/bin/sudo".into(),
            pmset: PMSET.into(),
            admin: Admin::Osascript("/usr/bin/osascript".into()),
            visudo: "/usr/sbin/visudo".into(),
            chown: "/usr/sbin/chown".into(),
            sudoers_dir: "/etc/sudoers.d".into(),
            lid_closed: macos,
        }
    }

    fn assertion_command(&self) -> Option<Command> {
        let pid = std::process::id().to_string();
        match self.inhibitor.as_ref()? {
            Inhibitor::Caffeinate(path) => {
                let mut command = Command::new(path);
                command.args(["-i", "-w", &pid]);
                Some(command)
            }
            Inhibitor::SystemdInhibit(path) => {
                let mut command = Command::new(path);
                command.args([
                    "--what=idle:sleep",
                    "--who=Chauffeur",
                    "--why=Agents are working",
                    "--mode=block",
                    "tail",
                    &format!("--pid={pid}"),
                    "-f",
                    "/dev/null",
                ]);
                Some(command)
            }
        }
    }

    fn sudoers_file(&self) -> PathBuf {
        self.sudoers_dir.join(SUDOERS_FILE)
    }
}

// --- the assertion process ---

/// The process holding the assertion: spawned once while held, killed (its process group) when
/// released; never two.
#[derive(Debug, Default)]
pub struct Assertion {
    child: Option<Child>,
    spawned_at: Option<Instant>,
}

impl Assertion {
    /// Holds it, spawning `command` unless it already runs. One that exited on its own is spawned
    /// again, at most every [`RESPAWN_AFTER`].
    pub fn hold(&mut self, command: Option<Command>) -> std::io::Result<()> {
        if self.held() {
            return Ok(());
        }
        if self
            .spawned_at
            .is_some_and(|at| at.elapsed() < RESPAWN_AFTER)
        {
            return Err(std::io::Error::other("it exited on its own"));
        }
        let Some(mut command) = command else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no caffeinate or systemd-inhibit on this system",
            ));
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        self.spawned_at = Some(Instant::now());
        self.child = Some(command.spawn()?);
        Ok(())
    }

    /// Whether the process runs.
    pub fn held(&mut self) -> bool {
        match &mut self.child {
            Some(child) => match child.try_wait() {
                Ok(None) => true,
                _ => {
                    self.child = None;
                    false
                }
            },
            None => false,
        }
    }

    pub fn release(&mut self) {
        if let Some(mut child) = self.child.take() {
            // SAFETY: a signal to the process group the child leads; ESRCH once it is gone.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGTERM);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        self.spawned_at = None;
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
}

impl Drop for Assertion {
    fn drop(&mut self) {
        self.release();
    }
}

// --- the battery ---

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub percent: Option<u8>,
    pub on_battery: bool,
}

/// Reads `pmset -g batt`: "Now drawing from 'Battery Power'" then " -InternalBattery-0 (id=…)\t
/// 85%; discharging; …". A Mac without a battery draws from 'AC Power' and lists none.
pub fn parse_batt(text: &str) -> Battery {
    let on_battery = text.contains("'Battery Power'");
    let percent = text.lines().find_map(|line| {
        let end = line.find('%')?;
        let digits: String = line[..end]
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        digits.parse::<u8>().ok()
    });
    Battery {
        percent,
        on_battery,
    }
}

pub(crate) fn read_battery(tools: &Tools) -> Option<Battery> {
    let output = Command::new(&tools.pmset)
        .args(["-g", "batt"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| parse_batt(&String::from_utf8_lossy(&output.stdout)))
}

/// Gives sleep back on battery below `min` percent (0: never).
pub fn battery_low(battery: Option<Battery>, min: u8) -> bool {
    battery.is_some_and(|b| b.on_battery && b.percent.is_some_and(|p| p < min))
}

// --- disablesleep through sudo ---

fn pmset_disablesleep(tools: &Tools, on: bool) -> Result<(), String> {
    let output = Command::new(&tools.sudo)
        .args([
            "-n",
            PMSET,
            "-a",
            "disablesleep",
            if on { "1" } else { "0" },
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not run sudo: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("sudo -n pmset failed ({})", output.status)
        } else {
            stderr
        })
    }
}

// --- the controller ---

struct Controller {
    tools: Tools,
    marker: PathBuf,
    linger: Linger,
    assertion: Assertion,
    /// Sleep is off (`disablesleep 1` ran, `disablesleep 0` not since).
    sleep_disabled: bool,
    /// `sudo -n` failed: the assertion only, until the setting changes.
    sudo_failed: Option<String>,
    /// Why sleep could not be given back; tried again every tick.
    restore_failed: Option<String>,
    /// Why the assertion could not be held.
    assertion_failed: Option<String>,
    assertion_logged: bool,
    battery: Option<Battery>,
    battery_read: Option<Instant>,
    low_battery: bool,
    /// While the setting leaves `lid_closed`: sleep stays on whatever the setting says.
    lid_blocked: bool,
    mode: KeepAwake,
    stopped: bool,
}

impl Controller {
    fn tick(&mut self, mode: KeepAwake, min_battery: u8, work: Work, now: Instant) -> PowerState {
        let mode = if mode == KeepAwake::LidClosed && !self.tools.lid_closed {
            KeepAwake::WhileWorking
        } else {
            mode
        };
        if mode != self.mode {
            self.mode = mode;
            self.sudo_failed = None;
        }
        let (hold, lingering) = if mode == KeepAwake::Never || self.stopped {
            self.linger.reset();
            (false, None)
        } else {
            self.linger.update(work.busy(), now)
        };

        if hold {
            match self.assertion.hold(self.tools.assertion_command()) {
                Ok(()) => self.assertion_failed = None,
                Err(error) => {
                    // Logged once per daemon: a system without the tool stays without it.
                    if !self.assertion_logged {
                        self.assertion_logged = true;
                        tracing::warn!(%error, "could not keep the machine awake");
                    }
                    self.assertion_failed =
                        Some(format!("Could not keep the machine awake: {error}"));
                }
            }
        } else {
            self.assertion.release();
            self.assertion_failed = None;
        }

        let lid = hold && mode == KeepAwake::LidClosed && self.sudo_failed.is_none();
        if lid {
            if self
                .battery_read
                .is_none_or(|at| now.saturating_duration_since(at) >= BATTERY_EVERY)
            {
                self.battery = read_battery(&self.tools);
                self.battery_read = Some(now);
            }
            let low = battery_low(self.battery, min_battery);
            if low != self.low_battery {
                if low {
                    tracing::info!(
                        percent = ?self.battery.and_then(|b| b.percent),
                        min_battery,
                        "battery low: sleep given back while agents work"
                    );
                } else {
                    tracing::info!("battery fine again: sleep turned off while agents work");
                }
            }
            self.low_battery = low;
        } else {
            self.low_battery = false;
            self.battery = None;
            self.battery_read = None;
        }

        let disable = lid && !self.low_battery && !self.lid_blocked;
        if disable && !self.sleep_disabled {
            match self.disable_sleep() {
                Ok(()) => {
                    self.sleep_disabled = true;
                    tracing::info!("sleep turned off while agents work (lid closed mode)");
                }
                Err(error) => {
                    tracing::warn!(%error, "sudo -n pmset failed: keeping the machine awake as in while_working mode");
                    self.sudo_failed = Some(error);
                }
            }
        } else if !disable && self.sleep_disabled {
            self.give_back();
        }
        self.state(mode, work, lingering)
    }

    /// Writes the marker first: a crash right after still gets sleep back at the next start.
    fn disable_sleep(&mut self) -> Result<(), String> {
        std::fs::write(&self.marker, b"1\n")
            .map_err(|error| format!("could not write {}: {error}", self.marker.display()))?;
        let result = pmset_disablesleep(&self.tools, true);
        if result.is_err() {
            let _ = std::fs::remove_file(&self.marker);
        }
        result
    }

    /// `disablesleep 0`; the marker goes once it worked.
    fn give_back(&mut self) {
        match pmset_disablesleep(&self.tools, false) {
            Ok(()) => {
                self.sleep_disabled = false;
                self.restore_failed = None;
                let _ = std::fs::remove_file(&self.marker);
                tracing::info!("sleep given back");
            }
            Err(error) => {
                if self.restore_failed.is_none() {
                    tracing::error!(%error, "could not give sleep back: run `sudo pmset -a disablesleep 0`");
                }
                self.restore_failed = Some(error);
            }
        }
    }

    fn state(&mut self, mode: KeepAwake, work: Work, lingering: Option<Duration>) -> PowerState {
        let holding = if self.sleep_disabled {
            PowerHold::AllSleep
        } else if self.assertion.held() {
            PowerHold::IdleSleep
        } else {
            PowerHold::None
        };
        let warning = if let Some(error) = &self.restore_failed {
            Some(format!(
                "Sleep is still turned off: Chauffeur could not turn it back on ({error}). Run `sudo pmset -a disablesleep 0`."
            ))
        } else if let Some(error) = &self.sudo_failed {
            Some(format!(
                "Lid closed mode could not turn sleep off ({error}): the machine is kept awake while agents work, but closing the lid sleeps it. Choose lid closed again in Settings to reinstall the rule."
            ))
        } else {
            self.assertion_failed.clone()
        };
        PowerState {
            keep_awake: mode,
            holding,
            working_chats: work.chats,
            running_tasks: work.tasks,
            creating_workspaces: work.creating,
            lingering_secs: lingering
                .filter(|_| holding != PowerHold::None)
                .map(|d| d.as_secs().max(1) as u32),
            low_battery: self.low_battery,
            battery_percent: self.battery.and_then(|b| b.percent),
            warning,
        }
    }
}

/// Keeps the machine awake while agents work; see the module's doc.
pub struct Power {
    controller: Mutex<Controller>,
    state: Mutex<PowerState>,
}

impl Power {
    pub fn new(state_dir: &Path) -> Power {
        Power::with_tools(state_dir, Tools::system())
    }

    pub fn with_tools(state_dir: &Path, tools: Tools) -> Power {
        Power {
            controller: Mutex::new(Controller {
                tools,
                marker: state_dir.join(MARKER),
                linger: Linger::new(LINGER),
                assertion: Assertion::default(),
                sleep_disabled: false,
                sudo_failed: None,
                restore_failed: None,
                assertion_failed: None,
                assertion_logged: false,
                battery: None,
                battery_read: None,
                low_battery: false,
                lid_blocked: false,
                mode: KeepAwake::default(),
                stopped: false,
            }),
            state: Mutex::new(PowerState::default()),
        }
    }

    /// Tests: fake tools, a temporary sudoers folder.
    pub fn set_tools(&self, tools: Tools) {
        self.controller.lock().unwrap().tools = tools;
    }

    /// Tests: a shorter debounce.
    pub fn set_linger(&self, linger: Duration) {
        self.controller.lock().unwrap().linger = Linger::new(linger);
    }

    pub(crate) fn tools(&self) -> Tools {
        self.controller.lock().unwrap().tools.clone()
    }

    /// The state as of the last look (every few seconds).
    pub fn state(&self) -> PowerState {
        self.state.lock().unwrap().clone()
    }

    /// Looks at the work and holds or releases accordingly. Blocking (it may run sudo).
    pub fn tick(&self, mode: KeepAwake, min_battery: u8, work: Work, now: Instant) -> PowerState {
        let state = self
            .controller
            .lock()
            .unwrap()
            .tick(mode, min_battery, work, now);
        let mut last = self.state.lock().unwrap();
        if last.holding != state.holding {
            match state.holding {
                PowerHold::None => tracing::info!("the machine may sleep again"),
                PowerHold::IdleSleep => {
                    tracing::info!(reason = work.reason(), "keeping the machine awake")
                }
                PowerHold::AllSleep => {}
            }
        }
        *last = state.clone();
        state
    }

    /// At the daemon's start: sleep turned off by a daemon that crashed or was killed comes back.
    pub fn restore_at_start(&self) {
        let mut c = self.controller.lock().unwrap();
        if c.marker.exists() {
            tracing::warn!("sleep was left turned off by the last daemon: turning it back on");
            c.sleep_disabled = true;
            c.give_back();
        }
    }

    /// The daemon stops: the assertion goes and sleep comes back; nothing is held after.
    pub fn shutdown(&self) {
        let mut c = self.controller.lock().unwrap();
        c.stopped = true;
        c.assertion.release();
        if c.sleep_disabled {
            c.give_back();
        }
    }

    pub fn lid_closed_available(&self) -> bool {
        self.controller.lock().unwrap().tools.lid_closed
    }

    /// The sudoers rule is in place (its file exists; only root reads it).
    pub fn sudoers_rule(&self) -> bool {
        self.tools().sudoers_file().exists()
    }

    /// The setting is leaving `lid_closed`: sleep comes back now and stays on until `unblock_lid`.
    pub fn give_sleep_back(&self) -> Result<(), String> {
        let mut c = self.controller.lock().unwrap();
        c.lid_blocked = true;
        if c.sleep_disabled {
            c.give_back();
        }
        match &c.restore_failed {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    pub fn unblock_lid(&self) {
        let mut c = self.controller.lock().unwrap();
        c.lid_blocked = false;
        // A rule just installed or removed: try sudo again.
        c.sudo_failed = None;
    }

    /// Installs the sudoers rule behind one administrator prompt. Blocking: the prompt waits for
    /// the user.
    pub fn install_rule(&self) -> Result<(), String> {
        let tools = self.tools();
        let user = current_user().ok_or("could not tell who the user is")?;
        let rule = sudoers_rule(&user)?;
        run_admin(&tools, &install_script(&tools, &rule))?;
        if !tools.sudoers_file().exists() {
            return Err("the rule was not installed".into());
        }
        tracing::info!(file = %tools.sudoers_file().display(), "sudoers rule installed");
        Ok(())
    }

    /// Removes the sudoers rule behind one administrator prompt.
    pub fn remove_rule(&self) -> Result<(), String> {
        let tools = self.tools();
        run_admin(&tools, &remove_script(&tools))?;
        if tools.sudoers_file().exists() {
            return Err("the rule is still there".into());
        }
        tracing::info!(file = %tools.sudoers_file().display(), "sudoers rule removed");
        Ok(())
    }
}

/// Runs until the daemon stops; `Power::shutdown` then gives everything back.
pub fn spawn(daemon: Arc<Daemon>) {
    tokio::spawn(async move {
        {
            let d = daemon.clone();
            let _ = tokio::task::spawn_blocking(move || d.power.restore_at_start()).await;
        }
        loop {
            let work = gather(&daemon);
            let settings = daemon.settings.get();
            let d = daemon.clone();
            let _ = tokio::task::spawn_blocking(move || {
                d.power.tick(
                    settings.keep_awake,
                    settings.keep_awake_min_battery,
                    work,
                    Instant::now(),
                )
            })
            .await;
            tokio::select! {
                _ = daemon.shutdown.cancelled() => return,
                _ = tokio::time::sleep(TICK) => {}
            }
        }
    });
}

// --- the sudoers rule ---

/// The account's short name, from the password database (not `$USER`).
pub(crate) fn current_user() -> Option<String> {
    // SAFETY: getpwuid returns a pointer into static storage, or null; copied out at once.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        (!entry.is_null() && !(*entry).pw_name.is_null()).then(|| {
            std::ffi::CStr::from_ptr((*entry).pw_name)
                .to_string_lossy()
                .into_owned()
        })
    }
}

/// The rule's one line, for a user name made of letters, digits, `_`, `-` and `.` only (no space,
/// comma, `%`, `#`, quote or anything sudoers or a shell would read).
pub fn sudoers_rule(user: &str) -> Result<String, String> {
    let fine = !user.is_empty()
        && user.len() <= 64
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
        && !user.starts_with(['-', '.'])
        && user != "ALL";
    if !fine {
        return Err(format!(
            "the user name {user:?} cannot go in a sudoers rule"
        ));
    }
    Ok(format!(
        "{user} ALL=(root) NOPASSWD: {PMSET} -a disablesleep 1, {PMSET} -a disablesleep 0"
    ))
}

/// Single-quoted for `sh`.
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Run as root: writes the rule next to its place, owned by root:wheel and 0440, has `visudo`
/// check it, then moves it into place (an atomic rename); a rule `visudo` refuses is deleted, so
/// no invalid sudoers file is ever left.
pub fn install_script(tools: &Tools, rule: &str) -> String {
    install_named_rule(tools, rule, SUDOERS_FILE, SUDOERS_STAGING)
}

pub(crate) fn install_named_rule(tools: &Tools, rule: &str, name: &str, staged: &str) -> String {
    let dir = tools.sudoers_dir.to_string_lossy();
    let staging = tools.sudoers_dir.join(staged);
    let target = tools.sudoers_dir.join(name);
    format!(
        "set -e\n\
         umask 077\n\
         staging={staging}\n\
         trap '/bin/rm -f \"$staging\"' EXIT\n\
         /bin/mkdir -p {dir}\n\
         /bin/rm -f \"$staging\"\n\
         printf '%s\\n' {rule} > \"$staging\"\n\
         {chown} root:wheel \"$staging\"\n\
         /bin/chmod 0440 \"$staging\"\n\
         if ! {visudo} -cf \"$staging\" >/dev/null 2>&1; then echo 'visudo refused the rule' >&2; exit 1; fi\n\
         /bin/mv -f \"$staging\" {target}\n",
        staging = sh_quote(&staging.to_string_lossy()),
        dir = sh_quote(&dir),
        rule = sh_quote(rule),
        chown = sh_quote(&tools.chown.to_string_lossy()),
        visudo = sh_quote(&tools.visudo.to_string_lossy()),
        target = sh_quote(&target.to_string_lossy()),
    )
}

pub fn remove_script(tools: &Tools) -> String {
    format!(
        "/bin/rm -f {} {}\n",
        sh_quote(&tools.sudoers_file().to_string_lossy()),
        sh_quote(&tools.sudoers_dir.join(SUDOERS_STAGING).to_string_lossy())
    )
}

/// A string literal of AppleScript.
pub fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The AppleScript that asks for an administrator's password and runs `script` as root.
pub fn applescript(script: &str) -> String {
    format!(
        "do shell script {} with prompt {} with administrator privileges",
        applescript_string(script),
        applescript_string(
            "Chauffeur needs an administrator's password once, to be allowed to turn sleep off while agents work with the lid closed."
        )
    )
}

fn run_admin(tools: &Tools, script: &str) -> Result<(), String> {
    let output = match &tools.admin {
        Admin::Osascript(osascript) => Command::new(osascript)
            .arg("-e")
            .arg(applescript(script))
            .stdin(Stdio::null())
            .output(),
        Admin::Shell => Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .output(),
    }
    .map_err(|error| format!("could not ask for the administrator's password: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.contains("-128") {
        "the administrator prompt was cancelled".into()
    } else if stderr.contains("-1713") || stderr.contains("No user interaction allowed") {
        "no administrator prompt can be shown: the daemon does not run in your desktop session"
            .into()
    } else if stderr.is_empty() {
        format!("the administrator's part failed ({})", output.status)
    } else {
        stderr
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn chat(status: ChatStatus, tasks: u32, queued: bool, creating: bool) -> ChatWork {
        ChatWork {
            status,
            tasks,
            queued,
            creating,
        }
    }

    #[test]
    fn an_agent_works_while_its_turn_runs_its_tasks_run_or_its_messages_wait_for_the_workspace() {
        use ChatStatus::*;
        let idle = work_of(
            [chat(Idle, 0, false, false), chat(Closed, 0, false, false)],
            0,
        );
        assert!(!idle.busy());
        assert_eq!(idle.reason(), "");
        // Waiting for the user, or a queue left after an interrupt: nothing to keep awake for.
        assert!(!work_of([chat(NeedsInput, 0, false, false)], 0).busy());
        assert!(!work_of([chat(Idle, 0, true, false)], 0).busy());
        assert!(!work_of([chat(Error, 0, false, false)], 0).busy());

        let work = work_of(
            [
                chat(Working, 0, false, false),
                chat(Starting, 0, false, false),
                // Background tasks keep an idle chat working, and one waiting for an answer.
                chat(Idle, 2, false, false),
                chat(NeedsInput, 1, false, false),
                chat(Idle, 0, true, true),
            ],
            0,
        );
        assert_eq!(
            work,
            Work {
                chats: 5,
                tasks: 3,
                creating: 0,
                transfers: 0,
            }
        );
        assert_eq!(work.reason(), "5 agents working");
        let creating = work_of([], 1);
        assert!(creating.busy());
        assert_eq!(creating.reason(), "1 workspace being created");
        assert_eq!(
            work_of([chat(Working, 0, false, false)], 2).reason(),
            "1 agent working, 2 workspaces being created"
        );
    }

    #[test]
    fn a_transfer_keeps_an_idle_machine_awake_until_it_finishes() {
        let work = Work {
            transfers: 1,
            ..Work::default()
        };
        assert!(work.busy());
        assert_eq!(work.reason(), "1 workspace transferring");
        let mut linger = Linger::new(LINGER);
        let now = Instant::now();
        assert!(linger.update(work.busy(), now).0);
        assert!(linger.update(false, now + Duration::from_secs(1)).0);
        assert!(!linger.update(false, now + LINGER).0);
    }

    #[test]
    fn the_hold_lingers_after_the_work_ends_then_goes() {
        let mut linger = Linger::new(Duration::from_secs(120));
        let t0 = Instant::now();
        assert_eq!(linger.update(false, t0), (false, None));
        assert_eq!(linger.update(true, t0), (true, None));
        let (hold, left) = linger.update(false, t0 + Duration::from_secs(30));
        assert!(hold);
        assert_eq!(left, Some(Duration::from_secs(90)));
        // Work again within the delay: it starts over from the new end.
        assert_eq!(
            linger.update(true, t0 + Duration::from_secs(60)),
            (true, None)
        );
        assert!(linger.update(false, t0 + Duration::from_secs(179)).0);
        assert_eq!(
            linger.update(false, t0 + Duration::from_secs(180)),
            (false, None)
        );
        assert_eq!(
            linger.update(false, t0 + Duration::from_secs(181)),
            (false, None)
        );
    }

    #[test]
    fn the_battery_is_read_from_pmset() {
        let on_battery = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=4653155)\t17%; discharging; 1:02 remaining present: true\n";
        assert_eq!(
            parse_batt(on_battery),
            Battery {
                percent: Some(17),
                on_battery: true
            }
        );
        let charging = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=4653155)\t100%; charged; 0:00 remaining present: true\n";
        assert_eq!(
            parse_batt(charging),
            Battery {
                percent: Some(100),
                on_battery: false
            }
        );
        // A Mac without a battery.
        assert_eq!(
            parse_batt("Now drawing from 'AC Power'\n"),
            Battery {
                percent: None,
                on_battery: false
            }
        );
        assert!(battery_low(Some(parse_batt(on_battery)), 20));
        assert!(!battery_low(Some(parse_batt(on_battery)), 17));
        assert!(!battery_low(Some(parse_batt(charging)), 20));
        assert!(!battery_low(None, 20));
    }

    #[test]
    fn the_sudoers_rule_names_exactly_the_two_commands_for_a_plain_user_name() {
        assert_eq!(
            sudoers_rule("alice").unwrap(),
            "alice ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 1, /usr/bin/pmset -a disablesleep 0"
        );
        assert!(sudoers_rule("jean.dupont_2-x").is_ok());
        for bad in [
            "", "ALL", "a b", "a,b", "%admin", "#0", "a'b", "a\"b", "a\nb", "a\\b", "-x", ".x",
            "a=b", "a:b", "é", "a;b", "$(id)",
        ] {
            assert!(sudoers_rule(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn applescript_strings_escape_quotes_and_backslashes() {
        assert_eq!(applescript_string(r#"a "b" \c"#), r#""a \"b\" \\c""#);
        let script = applescript("printf '%s\\n' 'x' > \"$f\"");
        assert!(
            script.starts_with(r#"do shell script "printf '%s\\n' 'x' > \"$f\"" with prompt ""#)
        );
        assert!(script.ends_with("with administrator privileges"));
    }

    /// A script standing in for a system tool: records its arguments, exits with `code`.
    ///
    /// On Linux, running a file while a forked child still holds a write descriptor to it fails
    /// with ETXTBSY ("Text file busy"). Tests run in parallel and fork, one of them right while
    /// another writes its script, so the first run of a script can fail at random and the code
    /// under test then sees a tool that cannot start. The script is therefore run once, doing
    /// nothing, until it starts: no descriptor to it is open after that, and it stays so.
    fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\n[ -z \"$FAKE_PROBE\" ] || exit 0\n{body}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        for _ in 0..500 {
            match Command::new(&path).env("FAKE_PROBE", "1").status() {
                Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                result => {
                    assert!(result.unwrap().success(), "the fake {name} does not start");
                    return path;
                }
            }
        }
        panic!("the fake {name} stays busy");
    }

    struct Fakes {
        _dir: tempfile::TempDir,
        root: PathBuf,
        tools: Tools,
    }

    impl Fakes {
        fn new() -> Fakes {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().to_path_buf();
            let log = root.join("calls.log");
            let log = log.display();
            // sudo fails while `sudo-fails` exists; pmset prints `batt.txt`.
            let sudo = fake(
                &root,
                "sudo",
                &format!(
                    "echo \"sudo $*\" >> {log}\nif [ -e {root}/sudo-fails ]; then echo 'sudo: a password is required' >&2; exit 1; fi",
                    root = root.display()
                ),
            );
            let pmset = fake(
                &root,
                "pmset",
                &format!(
                    "echo \"pmset $*\" >> {log}\ncat {}/batt.txt 2>/dev/null",
                    root.display()
                ),
            );
            let caffeinate = fake(
                &root,
                "caffeinate",
                &format!("echo \"caffeinate $*\" >> {log}\nexec sleep 600"),
            );
            let visudo = fake(
                &root,
                "visudo",
                &format!(
                    "echo \"visudo $*\" >> {log}\nif [ -e {root}/visudo-refuses ]; then exit 1; fi",
                    root = root.display()
                ),
            );
            let chown = fake(&root, "chown", &format!("echo \"chown $*\" >> {log}"));
            let sudoers_dir = root.join("sudoers.d");
            std::fs::create_dir(&sudoers_dir).unwrap();
            let tools = Tools {
                inhibitor: Some(Inhibitor::Caffeinate(caffeinate)),
                sudo,
                pmset,
                admin: Admin::Shell,
                visudo,
                chown,
                sudoers_dir,
                lid_closed: true,
            };
            Fakes {
                _dir: dir,
                root,
                tools,
            }
        }

        fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(self.root.join("calls.log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }

        fn clear(&self) {
            let _ = std::fs::remove_file(self.root.join("calls.log"));
        }

        fn sudo_calls(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .filter(|c| c.starts_with("sudo"))
                .collect()
        }

        fn power(&self) -> Power {
            let power = Power::with_tools(&self.root, self.tools.clone());
            power.set_linger(Duration::from_secs(60));
            power
        }

        fn marker(&self) -> PathBuf {
            self.root.join(MARKER)
        }
    }

    const BUSY: Work = Work {
        chats: 2,
        tasks: 1,
        creating: 0,
        transfers: 0,
    };
    const IDLE: Work = Work {
        chats: 0,
        tasks: 0,
        creating: 0,
        transfers: 0,
    };

    fn alive(pid: u32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    fn wait_gone(pid: u32) -> bool {
        for _ in 0..50 {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn while_working_spawns_the_assertion_once_and_kills_it_when_idle() {
        let f = Fakes::new();
        let power = f.power();
        let t0 = Instant::now();
        let state = power.tick(KeepAwake::WhileWorking, 20, BUSY, t0);
        assert_eq!(state.holding, PowerHold::IdleSleep);
        assert_eq!(state.working_chats, 2);
        let pid = power.controller.lock().unwrap().assertion.pid().unwrap();
        // Ticks while working: the same process, never a second one.
        for s in 1..4 {
            power.tick(
                KeepAwake::WhileWorking,
                20,
                BUSY,
                t0 + Duration::from_secs(s),
            );
        }
        assert_eq!(power.controller.lock().unwrap().assertion.pid(), Some(pid));
        // The fake writes its line once started.
        let expected = format!("caffeinate -i -w {}", std::process::id());
        for _ in 0..100 {
            if f.calls().contains(&expected) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let spawned: Vec<_> = f
            .calls()
            .into_iter()
            .filter(|c| c.starts_with("caffeinate"))
            .collect();
        assert_eq!(spawned, [expected]);
        // Idle: kept for the debounce, then killed.
        let state = power.tick(
            KeepAwake::WhileWorking,
            20,
            IDLE,
            t0 + Duration::from_secs(20),
        );
        assert_eq!(state.holding, PowerHold::IdleSleep);
        assert_eq!(state.lingering_secs, Some(43));
        assert!(alive(pid));
        let state = power.tick(
            KeepAwake::WhileWorking,
            20,
            IDLE,
            t0 + Duration::from_secs(64),
        );
        assert_eq!(state.holding, PowerHold::None);
        assert_eq!(state.lingering_secs, None);
        assert!(wait_gone(pid), "the assertion process still runs");
        // No sudo in this mode.
        assert!(f.sudo_calls().is_empty());

        // `never`: nothing, even while working.
        let state = power.tick(KeepAwake::Never, 20, BUSY, t0 + Duration::from_secs(70));
        assert_eq!(state.holding, PowerHold::None);
        assert!(power.controller.lock().unwrap().assertion.pid().is_none());

        // Switching to `never` while holding releases at once.
        power.tick(
            KeepAwake::WhileWorking,
            20,
            BUSY,
            t0 + Duration::from_secs(80),
        );
        let pid = power.controller.lock().unwrap().assertion.pid().unwrap();
        power.tick(KeepAwake::Never, 20, BUSY, t0 + Duration::from_secs(81));
        assert!(wait_gone(pid));

        // The daemon stops: released, and nothing is held after.
        power.tick(
            KeepAwake::WhileWorking,
            20,
            BUSY,
            t0 + Duration::from_secs(90),
        );
        let pid = power.controller.lock().unwrap().assertion.pid().unwrap();
        power.shutdown();
        assert!(wait_gone(pid));
        let state = power.tick(
            KeepAwake::WhileWorking,
            20,
            BUSY,
            t0 + Duration::from_secs(91),
        );
        assert_eq!(state.holding, PowerHold::None);
    }

    #[test]
    fn an_assertion_process_that_died_is_spawned_again_but_not_in_a_loop() {
        let f = Fakes::new();
        let power = f.power();
        let t0 = Instant::now();
        power.tick(KeepAwake::WhileWorking, 20, BUSY, t0);
        let pid = power.controller.lock().unwrap().assertion.pid().unwrap();
        // SAFETY: killing the fake caffeinate we spawned.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        for _ in 0..50 {
            if !power.controller.lock().unwrap().assertion.held() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        let state = power.tick(
            KeepAwake::WhileWorking,
            20,
            BUSY,
            t0 + Duration::from_secs(1),
        );
        assert_eq!(state.holding, PowerHold::None);
        assert!(state.warning.unwrap().contains("exited on its own"));
        // A system without caffeinate or systemd-inhibit: said, not retried in a loop.
        let mut tools = f.tools.clone();
        tools.inhibitor = None;
        let power = Power::with_tools(&f.root, tools);
        let state = power.tick(KeepAwake::WhileWorking, 20, BUSY, t0);
        assert_eq!(state.holding, PowerHold::None);
        assert!(state.warning.unwrap().contains("no caffeinate"));
    }

    #[test]
    fn lid_closed_turns_sleep_off_while_working_and_back_on_when_idle() {
        let f = Fakes::new();
        let power = f.power();
        let t0 = Instant::now();
        let state = power.tick(KeepAwake::LidClosed, 20, BUSY, t0);
        assert_eq!(state.holding, PowerHold::AllSleep);
        assert_eq!(f.sudo_calls(), ["sudo -n /usr/bin/pmset -a disablesleep 1"]);
        assert!(f.marker().exists(), "the marker says sleep is off");
        // The assertion is held too.
        assert!(power.controller.lock().unwrap().assertion.pid().is_some());
        // Still working: sudo is not run again.
        power.tick(KeepAwake::LidClosed, 20, BUSY, t0 + Duration::from_secs(5));
        assert_eq!(f.sudo_calls().len(), 1);
        // Idle, past the debounce: sleep comes back, the marker goes.
        power.tick(KeepAwake::LidClosed, 20, IDLE, t0 + Duration::from_secs(30));
        assert_eq!(f.sudo_calls().len(), 1);
        let state = power.tick(KeepAwake::LidClosed, 20, IDLE, t0 + Duration::from_secs(70));
        assert_eq!(state.holding, PowerHold::None);
        assert_eq!(
            f.sudo_calls(),
            [
                "sudo -n /usr/bin/pmset -a disablesleep 1",
                "sudo -n /usr/bin/pmset -a disablesleep 0"
            ]
        );
        assert!(!f.marker().exists());
    }

    #[test]
    fn leaving_lid_closed_or_stopping_the_daemon_gives_sleep_back() {
        let f = Fakes::new();
        let power = f.power();
        let t0 = Instant::now();
        power.tick(KeepAwake::LidClosed, 20, BUSY, t0);
        // The setting changed by hand to while_working: back to the assertion only.
        let state = power.tick(
            KeepAwake::WhileWorking,
            20,
            BUSY,
            t0 + Duration::from_secs(1),
        );
        assert_eq!(state.holding, PowerHold::IdleSleep);
        assert_eq!(
            f.sudo_calls().last().unwrap(),
            "sudo -n /usr/bin/pmset -a disablesleep 0"
        );

        f.clear();
        power.tick(KeepAwake::LidClosed, 20, BUSY, t0 + Duration::from_secs(2));
        assert!(f.marker().exists());
        power.shutdown();
        assert_eq!(
            f.sudo_calls(),
            [
                "sudo -n /usr/bin/pmset -a disablesleep 1",
                "sudo -n /usr/bin/pmset -a disablesleep 0"
            ]
        );
        assert!(!f.marker().exists());

        // The settings window leaving lid_closed: sleep back at once, and kept on meanwhile.
        f.clear();
        let power = f.power();
        power.tick(KeepAwake::LidClosed, 20, BUSY, t0);
        power.give_sleep_back().unwrap();
        let state = power.tick(KeepAwake::LidClosed, 20, BUSY, t0 + Duration::from_secs(1));
        assert_eq!(state.holding, PowerHold::IdleSleep);
        assert_eq!(f.sudo_calls().len(), 2);
        power.unblock_lid();
        let state = power.tick(KeepAwake::LidClosed, 20, BUSY, t0 + Duration::from_secs(2));
        assert_eq!(state.holding, PowerHold::AllSleep);
    }

    #[test]
    fn a_daemon_starting_after_a_crash_gives_sleep_back() {
        let f = Fakes::new();
        // Nothing to do without the marker.
        f.power().restore_at_start();
        assert!(f.sudo_calls().is_empty());
        // A daemon killed while sleep was off left the marker.
        let crashed = f.power();
        crashed.tick(KeepAwake::LidClosed, 20, BUSY, Instant::now());
        // Killed: nothing of it runs on (its assertion dies with it), the marker stays.
        crashed.controller.lock().unwrap().assertion.release();
        std::mem::forget(crashed);
        assert!(f.marker().exists());
        f.clear();
        let power = f.power();
        power.restore_at_start();
        assert_eq!(f.sudo_calls(), ["sudo -n /usr/bin/pmset -a disablesleep 0"]);
        assert!(!f.marker().exists());
        assert_eq!(
            power
                .tick(KeepAwake::Never, 20, IDLE, Instant::now())
                .holding,
            PowerHold::None
        );

        // sudo failing then: said, the marker kept, tried again at each look.
        crashed_marker(&f);
        std::fs::write(f.root.join("sudo-fails"), "").unwrap();
        let power = f.power();
        power.restore_at_start();
        let state = power.tick(KeepAwake::Never, 20, IDLE, Instant::now());
        assert_eq!(state.holding, PowerHold::AllSleep);
        assert!(
            state
                .warning
                .unwrap()
                .contains("sudo pmset -a disablesleep 0")
        );
        assert!(f.marker().exists());
        std::fs::remove_file(f.root.join("sudo-fails")).unwrap();
        let state = power.tick(KeepAwake::Never, 20, IDLE, Instant::now());
        assert_eq!(state.holding, PowerHold::None);
        assert_eq!(state.warning, None);
        assert!(!f.marker().exists());
    }

    fn crashed_marker(f: &Fakes) {
        std::fs::write(f.marker(), "1\n").unwrap();
    }

    #[test]
    fn low_battery_gives_sleep_back_until_it_charges() {
        let f = Fakes::new();
        let power = f.power();
        let t0 = Instant::now();
        let batt = |text: &str| std::fs::write(f.root.join("batt.txt"), text).unwrap();
        batt(
            "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t15%; discharging; 0:40 remaining present: true\n",
        );
        let state = power.tick(KeepAwake::LidClosed, 20, BUSY, t0);
        assert!(state.low_battery);
        assert_eq!(state.battery_percent, Some(15));
        // The assertion only: no disablesleep.
        assert_eq!(state.holding, PowerHold::IdleSleep);
        assert!(f.sudo_calls().is_empty());
        assert!(f.calls().contains(&"pmset -g batt".to_string()));

        // On AC: sleep turned off, at the next reading (every minute).
        batt(
            "Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t16%; charging; present: true\n",
        );
        let state = power.tick(KeepAwake::LidClosed, 20, BUSY, t0 + Duration::from_secs(10));
        assert!(state.low_battery);
        let state = power.tick(KeepAwake::LidClosed, 20, BUSY, t0 + Duration::from_secs(61));
        assert!(!state.low_battery);
        assert_eq!(state.holding, PowerHold::AllSleep);
        assert_eq!(f.sudo_calls(), ["sudo -n /usr/bin/pmset -a disablesleep 1"]);

        // Unplugged below the threshold: given back while the agents still work.
        batt(
            "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t19%; discharging; present: true\n",
        );
        let state = power.tick(
            KeepAwake::LidClosed,
            20,
            BUSY,
            t0 + Duration::from_secs(122),
        );
        assert!(state.low_battery);
        assert_eq!(state.holding, PowerHold::IdleSleep);
        assert_eq!(
            f.sudo_calls().last().unwrap(),
            "sudo -n /usr/bin/pmset -a disablesleep 0"
        );
        // Above the threshold again (a lower threshold here): off again.
        let state = power.tick(
            KeepAwake::LidClosed,
            10,
            BUSY,
            t0 + Duration::from_secs(183),
        );
        assert!(!state.low_battery);
        assert_eq!(state.holding, PowerHold::AllSleep);
    }

    #[test]
    fn without_the_sudoers_rule_lid_closed_falls_back_to_the_assertion_and_says_it_once() {
        let f = Fakes::new();
        std::fs::write(f.root.join("sudo-fails"), "").unwrap();
        let power = f.power();
        let t0 = Instant::now();
        let state = power.tick(KeepAwake::LidClosed, 20, BUSY, t0);
        assert_eq!(state.holding, PowerHold::IdleSleep);
        let warning = state.warning.unwrap();
        assert!(warning.contains("a password is required"), "{warning}");
        assert!(!f.marker().exists());
        // Not tried again at every look.
        for s in 1..5 {
            power.tick(KeepAwake::LidClosed, 20, BUSY, t0 + Duration::from_secs(s));
        }
        assert_eq!(f.sudo_calls().len(), 1);
        // The rule back (installed from the settings window): tried again.
        std::fs::remove_file(f.root.join("sudo-fails")).unwrap();
        power.unblock_lid();
        let state = power.tick(KeepAwake::LidClosed, 20, BUSY, t0 + Duration::from_secs(6));
        assert_eq!(state.holding, PowerHold::AllSleep);
        assert_eq!(state.warning, None);
    }

    #[test]
    fn lid_closed_is_while_working_where_it_is_not_offered() {
        let f = Fakes::new();
        let mut tools = f.tools.clone();
        tools.lid_closed = false;
        let power = Power::with_tools(&f.root, tools);
        let state = power.tick(KeepAwake::LidClosed, 20, BUSY, Instant::now());
        assert_eq!(state.keep_awake, KeepAwake::WhileWorking);
        assert_eq!(state.holding, PowerHold::IdleSleep);
        assert!(f.sudo_calls().is_empty());
    }

    #[test]
    fn the_rule_is_installed_validated_0440_root_wheel_and_removed() {
        let f = Fakes::new();
        let power = f.power();
        assert!(!power.sudoers_rule());
        power.install_rule().unwrap();
        let file = f.tools.sudoers_dir.join(SUDOERS_FILE);
        let text = std::fs::read_to_string(&file).unwrap();
        let user = current_user().unwrap();
        assert_eq!(
            text,
            format!(
                "{user} ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 1, /usr/bin/pmset -a disablesleep 0\n"
            )
        );
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o440
        );
        let staging = f.tools.sudoers_dir.join(SUDOERS_STAGING);
        let calls = f.calls();
        assert_eq!(
            calls,
            [
                format!("chown root:wheel {}", staging.display()),
                format!("visudo -cf {}", staging.display()),
            ]
        );
        assert!(!staging.exists());
        assert!(power.sudoers_rule());

        power.remove_rule().unwrap();
        assert!(!file.exists());
        assert!(!power.sudoers_rule());

        // visudo refusing it: nothing installed, nothing left behind, and the reason.
        std::fs::write(f.root.join("visudo-refuses"), "").unwrap();
        let error = power.install_rule().unwrap_err();
        assert!(error.contains("visudo refused"), "{error}");
        assert!(!file.exists());
        assert!(!staging.exists());
        assert_eq!(std::fs::read_dir(&f.tools.sudoers_dir).unwrap().count(), 0);
    }

    #[test]
    fn a_cancelled_administrator_prompt_is_said_so() {
        let f = Fakes::new();
        let osascript = fake(
            &f.root,
            "osascript",
            "echo \"0:200: execution error: User canceled. (-128)\" >&2\nexit 1",
        );
        let mut tools = f.tools.clone();
        tools.admin = Admin::Osascript(osascript);
        let power = Power::with_tools(&f.root, tools);
        assert_eq!(
            power.install_rule().unwrap_err(),
            "the administrator prompt was cancelled"
        );
        assert!(!power.sudoers_rule());
    }
}
