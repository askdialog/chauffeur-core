//! User settings (`~/.chauffeur/settings.toml`) and repository settings
//! (`<repo>/.chauffeur/settings.toml`, falling back to Conductor's `.conductor/settings.toml`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context;
use chauffeur_protocol::client::ShortcutId;
use chauffeur_protocol::model::{ErrorReports, KeepAwake};
use serde::Deserialize;

use crate::paths::Paths;

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct UserSettings {
    pub daily_review: chauffeur_protocol::client::DailyReviewSettings,
    /// The main GitHub organization: its repos are named without their owner. Empty: none.
    pub github_org: String,
    /// Also list the repos of every other owner the GitHub account reaches: its own, its other
    /// organizations', the ones it collaborates on. False: only `github_org`'s.
    pub github_sources: bool,
    /// Repos preselected for a new workspace.
    pub default_repos: Vec<String>,
    /// Preferred display order; unlisted repos follow in automatic order.
    pub repo_order: Vec<String>,
    /// Prefix of workspace branches, e.g. `justincuzin` gives `justincuzin/<slug>`.
    /// Empty: the GitHub login.
    pub branch_prefix: String,
    /// Folders searched for existing clones, by repo name.
    pub clone_roots: Vec<String>,
    /// First port handed out to workspaces.
    pub port_range_start: u16,
    pub port_block_size: u16,
    /// Active child workspaces one workspace may have.
    pub max_children: usize,
    /// The agent and model a chat started without one gets (the CLI's `chauffeur new`). Unset:
    /// the first favorite of the app's model picker (the loadout), else Claude.
    pub default_agent: Option<String>,
    pub default_model: Option<String>,
    /// TCP port of the daemon's WebSocket endpoint.
    pub daemon_port: u16,
    /// Seconds an archive script may run.
    pub archive_script_timeout_secs: u64,
    /// Days an archive snapshot is kept.
    pub snapshot_retention_days: i64,
    /// Minutes without activity after which a chat's agent session is closed (the next message
    /// resumes it) and a workspace's Nx daemons stopped; 0 keeps them.
    pub idle_session_minutes: u64,
    /// Keep the machine awake while agents work: `never`, `while_working` (no idle sleep) or
    /// `lid_closed` (macOS: no sleep at all, the lid closed included; see `power`).
    pub keep_awake: KeepAwake,
    /// In `lid_closed` mode, on battery below this charge (percent), sleep is given back.
    pub keep_awake_min_battery: u8,
    /// After each turn, Claude suggests the user's next message, offered in the empty composer.
    pub prompt_suggestions: bool,
    /// A chat whose turn a daemon restart or a sidecar crash cut, or that failed on a network
    /// error (when the user is back), resumes it on its own, once.
    pub resume_interrupted_turns: bool,
    /// Report the app's error toasts to Chauffeur's Sentry: `always`, `ask` (each toast offers it)
    /// or `never`. Only packaged builds can (docs/adr/error-reports-to-sentry.md).
    pub error_reports: ErrorReports,
    /// Talk to Linear at all (tests and offline setups turn it off).
    pub linear_enabled: bool,
    /// Move attached Linear tickets to In Progress when a workspace starts on them.
    pub linear_start_tickets: bool,
    /// Explicit clone paths and default branches, by repo name.
    pub repos: BTreeMap<String, RepoOverride>,
    /// The app's fonts.
    pub appearance: AppearanceSettings,
    pub notifications: NotificationsSettings,
    pub keyboard: KeyboardSettings,
    pub office: OfficeSettings,
    pub news: NewsSettings,
    pub design_share: DesignShareSettings,
    pub slack: SlackSettings,
    pub mobile: MobileSettings,
    pub transfer: TransferSettings,
    pub workers: WorkerSettings,
    pub distribution: DistributionSettings,
    pub search: chauffeur_protocol::client::SearchSettings,
}

/// `[distribution]`: what a distribution says of its own users.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct DistributionSettings {
    /// The domain of the distribution's members: a user signed in with a Google account of it is
    /// a member and sees the features that rely on the distribution's hosted services (the
    /// Office, Slack, the phone). Empty, the default: nobody is a member.
    pub member_domain: String,
}

impl DistributionSettings {
    /// Whether `email` is a member's: its domain is `member_domain`, case aside.
    pub fn is_member(&self, email: &str) -> bool {
        let domain = self.member_domain.trim();
        !domain.is_empty()
            && email
                .rsplit_once('@')
                .is_some_and(|(_, of)| of.eq_ignore_ascii_case(domain))
    }
}

/// `[transfer]`: the machines a workspace can be transferred to, each a Chauffeur daemon reached
/// on a Unix socket (another machine's, forwarded over SSH), and the controller daemon keeping
/// the transfers' journal (decided with Justin 2026-10-26, before GCP workers).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TransferSettings {
    /// A custom controller socket; empty, the daemon starts and manages a local controller.
    pub controller: String,
    pub machines: Vec<TransferMachine>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TransferMachine {
    pub name: String,
    pub socket: String,
}

/// `[news]`: what the Office's news kiosk shows (docs/office.md), the user's own.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct NewsSettings {
    /// Hacker News's top stories, beside the feeds.
    pub hacker_news: bool,
    /// The user's RSS and Atom feeds, `https://…`.
    pub feeds: Vec<String>,
}

impl Default for NewsSettings {
    fn default() -> Self {
        NewsSettings {
            hacker_news: true,
            feeds: Vec::new(),
        }
    }
}

/// `[workers]`: where GCP workers are created (docs/cloud-workers.md).
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct WorkerSettings {
    pub project: String,
    pub zone: String,
    /// A Linux bundle (`scripts/package-linux.sh`) installed instead of the release's, for a
    /// build that is not released.
    pub bundle: String,
    /// The GCS bucket a workspace running on a worker is backed up to, every five minutes
    /// (docs/cloud-workspace-backups.md); none: `<project>-chauffeur-backups`, made with the
    /// first worker.
    pub backup_bucket: String,
}

impl Default for WorkerSettings {
    fn default() -> Self {
        WorkerSettings {
            project: String::new(),
            zone: "europe-west9-a".into(),
            bundle: String::new(),
            backup_bucket: String::new(),
        }
    }
}

/// `[design_share]` (story 51): the share service a design chat, a workspace share and an import
/// go through. Empty, the default: sharing is off. The user's identity is the Google sign-in.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct DesignShareSettings {
    /// The service, `https://…` (no path): its API, and the address of its design links.
    pub url: String,
    /// The service's other addresses, `https://…` (no path), whose share links the daemon takes
    /// as its own: where its workspace links live when that is elsewhere, and addresses it had
    /// before. The daemon still only calls `url`.
    pub link_urls: Vec<String>,
}

/// `[mobile]`: the relay the mobile companion goes through, offered until the user names one in
/// the settings window (which keeps it in the store). Empty, the default: no relay is offered.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MobileSettings {
    /// The relay, `wss://…`.
    pub relay: String,
}

/// `[slack]`: the service the Slack integration signs in through and reads context from. Empty,
/// the default: the integration is off.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SlackSettings {
    /// The service, `https://…` (no path).
    pub url: String,
}

/// `[office]` (docs/office.md): the office server, and the Google OAuth client every sign-in to
/// Chauffeur uses (the keys kept their first home). Empty, the default: no Office and no sign-in;
/// a distribution supplies its own (`set_distribution_defaults`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct OfficeSettings {
    /// Unset: on when `url` names a server. The daemon hooks the Office in at its start only, so
    /// turning it on takes a restart.
    pub enabled: Option<bool>,
    /// The office server, `wss://…/ws`.
    pub url: String,
    /// The Google OAuth client (a desktop app) to sign in with; Google does not treat a desktop
    /// app's secret as confidential.
    pub google_client_id: String,
    pub google_client_secret: String,
    /// The Google Workspace domain sign-in is limited to (Google's `hd`); empty, any account.
    pub google_hosted_domain: String,
    /// For a server started with `--insecure-dev`: the e-mail to say, without Google.
    pub dev_email: String,
    /// The repositories of `github_org` whose pull requests the EOW board shows, by name; empty,
    /// every repository of the organization but Chauffeur's own.
    pub board_repos: Vec<String>,
}

impl OfficeSettings {
    /// Off without a server, whatever `enabled` says.
    pub fn enabled(&self) -> bool {
        !self.url.trim().is_empty() && self.enabled.unwrap_or(true)
    }
}

/// `[notifications]`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct NotificationsSettings {
    /// Empty (none), `chime`, `bell`, `station`, or an audio file's path.
    pub sound: String,
}

/// `[keyboard]`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct KeyboardSettings {
    /// The Files editor takes Vim's keys.
    pub vim_mode: bool,
    /// `[keyboard.bindings]`: other keys for the app's shortcuts, by id (`"chat.new" = "⇧⌘T"`).
    /// Read as written, any value: `overrides` leaves out what the app could not use, so that one
    /// bad entry never fails the whole file.
    pub bindings: BTreeMap<String, toml::Value>,
}

impl KeyboardSettings {
    /// The bindings the app can use, and why each other one is left out: an id that names no
    /// shortcut, a value that is not keys in quotes, keys that are not a binding or are reserved.
    pub fn overrides(&self) -> (BTreeMap<ShortcutId, String>, Vec<String>) {
        let mut overrides = BTreeMap::new();
        let mut errors = Vec::new();
        for (name, value) in &self.bindings {
            let Some(keys) = value.as_str() else {
                errors.push(not_keys(name, value));
                continue;
            };
            match (ShortcutId::parse(name), binding_error(keys)) {
                (None, _) => errors.push(format!(
                    "[keyboard.bindings] \"{name}\" is not a shortcut the app lets you change: ignored"
                )),
                (Some(_), Some(error)) => {
                    errors.push(format!("[keyboard.bindings] \"{name}\": {error}: ignored"))
                }
                (Some(id), None) => {
                    overrides.insert(id, keys.to_string());
                }
            }
        }
        (overrides, errors)
    }
}

/// Why a value of `[keyboard.bindings]` that is not a string is left out. An id written without
/// quotes (`chat.new = "⇧⌘J"`) is a TOML dotted key: it reads as a table `chat` holding `new`.
fn not_keys(name: &str, value: &toml::Value) -> String {
    let Some(table) = value.as_table() else {
        return format!(
            "[keyboard.bindings] \"{name}\" = {value}: keys go in quotes, as in \"⇧⌘J\": ignored"
        );
    };
    let example = match table.iter().next() {
        Some((key, keys)) if table.len() == 1 && keys.is_str() => {
            format!("\"{name}.{key}\" = {keys}")
        }
        _ => "\"chat.new\" = \"⇧⌘J\"".to_string(),
    };
    format!("[keyboard.bindings] {name}.…: an id goes in quotes, as in {example}: ignored")
}

/// Keys a binding may name besides a printable ASCII character, as the app's `shortcuts.ts` reads
/// them (`NAMED` there), F1 to F12 aside.
const NAMED_KEYS: &[&str] = &[
    "Tab", "PgUp", "PgDn", "Home", "End", "Space", "Del", "↑", "↓", "←", "→", "↵",
];

/// Keys no shortcut may take, written the one way `canonical` writes them, and what they are: the
/// Edit menu's and macOS's own, and the terminal's that a running program needs (outside macOS ⌘
/// is Ctrl: Ctrl+C interrupts, Ctrl+D ends input, Ctrl+Z suspends; on macOS that is ⌃). The app's
/// `RESERVED` in `shortcuts.ts` holds the same.
const RESERVED: &[(&str, &str)] = &[
    ("⌘A", "Select All"),
    ("⌘C", "Copy, and the terminal's interrupt outside macOS"),
    ("⌘D", "the terminal's end of input outside macOS"),
    ("⌘H", "macOS's Hide"),
    ("⌘M", "macOS's Minimize"),
    ("⌘Q", "Quit"),
    ("⌘V", "Paste"),
    ("⌘X", "Cut"),
    ("⌘Z", "Undo, and the terminal's suspend outside macOS"),
    ("⇧⌘Z", "Redo"),
    ("⌃C", "the terminal's interrupt on macOS"),
    ("⌃D", "the terminal's end of input on macOS"),
    ("⌃Z", "the terminal's suspend on macOS"),
    ("⌃⌘F", "macOS's Full Screen"),
];

/// Why `keys` is not a binding the app can match, or None. A binding is written the macOS way:
/// modifiers among ⌃ ⌥ ⇧ ⌘, ⌘, ⌃ or ⌥ among them, then one key (`⇧⌘T`, `⌘/`, `⌘↑`, `⌃Tab`, `⌥⌘1`,
/// `⌘F5`): a printable ASCII character or a named key, never a reserved one.
pub fn binding_error(keys: &str) -> Option<String> {
    let mut modifiers = String::new();
    let mut rest = keys;
    while let Some(symbol) = rest.chars().next().filter(|c| "⌃⌥⇧⌘".contains(*c)) {
        if modifiers.contains(symbol) {
            return Some(format!("\"{keys}\" has {symbol} twice"));
        }
        modifiers.push(symbol);
        rest = &rest[symbol.len_utf8()..];
    }
    if !modifiers.contains(['⌘', '⌃', '⌥']) {
        return Some(format!("\"{keys}\" needs ⌘, ⌃ or ⌥"));
    }
    let mut chars = rest.chars();
    let ascii = matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_graphic());
    let function = (1..=12).any(|n| rest == format!("F{n}"));
    if !(ascii || function || NAMED_KEYS.contains(&rest)) {
        return Some(format!(
            "\"{keys}\": \"{rest}\" is not a key (a letter, a digit, an ASCII sign or a named key)"
        ));
    }
    let canonical = format!(
        "{}{}{}{}{}",
        if modifiers.contains('⌃') { "⌃" } else { "" },
        if modifiers.contains('⌥') { "⌥" } else { "" },
        if modifiers.contains('⇧') { "⇧" } else { "" },
        if modifiers.contains('⌘') { "⌘" } else { "" },
        rest.to_ascii_uppercase()
    );
    RESERVED
        .iter()
        .find(|(reserved, _)| *reserved == canonical)
        .map(|(_, what)| format!("\"{keys}\" stays {what}: choose other keys"))
}

/// `[appearance]`: font families as CSS names them, sizes in pixels; empty or 0: the app's own.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    pub interface_font: String,
    pub code_font: String,
    pub code_size: u16,
    pub terminal_font: String,
    pub terminal_size: u16,
    /// A colour theme file the user's tools write (pywal, kitty, Ghostty, foot, Alacritty; wallust
    /// from a template).
    pub terminal_theme: String,
    /// The theme colours the interface too.
    pub theme_interface: bool,
}

/// `[repos.<name>]`: what the user sets for one repo on this machine, never in the repo.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RepoOverride {
    pub path: Option<String>,
    pub default_branch: Option<String>,
    /// Scripts the user typed (the import from Conductor writes those of its UI), over the
    /// repo's committed settings and under its `settings.local.toml`.
    pub scripts: Option<UserScripts>,
    /// Variables given to the repo's scripts and to the agents of a workspace holding it.
    pub env: RepoEnv,
}

/// `[repos.<name>.scripts]`, keyed as a repo's `settings.toml` `[scripts]`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(transparent)]
pub struct UserScripts(RawScripts);

impl UserScripts {
    /// Whether it sets `script` (`setup`, `run`, `archive` or `run_mode`).
    pub fn has(&self, script: &str) -> bool {
        match script {
            "setup" => self.0.setup.is_some(),
            "run" => self.0.run.is_some(),
            "archive" => self.0.archive.is_some(),
            "run_mode" => self.0.run_mode.is_some(),
            _ => false,
        }
    }
}

/// `[repos.<name>.env]`. Its values may be secrets: its `Debug` names the variables only, so
/// that no log carries a value.
#[derive(Clone, Default, Deserialize)]
#[serde(transparent)]
pub struct RepoEnv(pub BTreeMap<String, String>);

impl std::fmt::Debug for RepoEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

impl Default for UserSettings {
    fn default() -> Self {
        UserSettings {
            daily_review: Default::default(),
            github_org: String::new(),
            github_sources: true,
            default_repos: Vec::new(),
            repo_order: Vec::new(),
            branch_prefix: String::new(),
            clone_roots: Vec::new(),
            port_range_start: 41000,
            port_block_size: 10,
            max_children: 5,
            default_agent: None,
            default_model: None,
            daemon_port: 47833,
            archive_script_timeout_secs: 300,
            snapshot_retention_days: 30,
            idle_session_minutes: 30,
            keep_awake: KeepAwake::default(),
            keep_awake_min_battery: 20,
            prompt_suggestions: true,
            resume_interrupted_turns: true,
            error_reports: ErrorReports::default(),
            linear_enabled: true,
            linear_start_tickets: true,
            repos: BTreeMap::new(),
            appearance: AppearanceSettings::default(),
            notifications: NotificationsSettings::default(),
            keyboard: KeyboardSettings::default(),
            office: OfficeSettings::default(),
            news: NewsSettings::default(),
            design_share: DesignShareSettings::default(),
            slack: SlackSettings::default(),
            mobile: MobileSettings::default(),
            transfer: TransferSettings::default(),
            workers: WorkerSettings::default(),
            distribution: DistributionSettings::default(),
            search: Default::default(),
        }
    }
}

/// The settings a distribution ships under the user's `settings.toml` (its office server, share
/// service, Google client…): Core itself names no service of anyone's. Set once, by the
/// distribution's daemon, before it opens.
static DISTRIBUTION_DEFAULTS: OnceLock<toml::Table> = OnceLock::new();

/// Sets the distribution's default settings, a `settings.toml` of its own; refused when they do
/// not parse as settings, or were set already.
pub fn set_distribution_defaults(text: &str) -> anyhow::Result<()> {
    let defaults: toml::Table =
        toml::from_str(text).context("parsing the distribution's default settings")?;
    parse_with(Some(&defaults), "").context("the distribution's default settings are not valid")?;
    DISTRIBUTION_DEFAULTS
        .set(defaults)
        .map_err(|_| anyhow::anyhow!("the distribution's default settings are already set"))
}

/// A `settings.toml`, over the distribution's defaults when there are some.
fn parse(text: &str) -> anyhow::Result<UserSettings> {
    parse_with(DISTRIBUTION_DEFAULTS.get(), text)
}

fn parse_with(defaults: Option<&toml::Table>, text: &str) -> anyhow::Result<UserSettings> {
    // Parsed alone first: its errors keep their line and column.
    let settings: UserSettings = toml::from_str(text)?;
    let Some(defaults) = defaults else {
        return Ok(settings);
    };
    let user: toml::Table = toml::from_str(text)?;
    Ok(toml::Value::Table(overlay(defaults.clone(), user)).try_into()?)
}

/// `over`'s keys over `base`'s, table by table: a key the user set, even to `""`, wins.
fn overlay(mut base: toml::Table, over: toml::Table) -> toml::Table {
    for (key, value) in over {
        let merged = match (base.remove(&key), value) {
            (Some(toml::Value::Table(base)), toml::Value::Table(over)) => {
                toml::Value::Table(overlay(base, over))
            }
            (_, value) => value,
        };
        base.insert(key, merged);
    }
    base
}

impl UserSettings {
    pub fn load(paths: &Paths) -> anyhow::Result<UserSettings> {
        Self::read(&paths.settings)
    }

    fn read(path: &Path) -> anyhow::Result<UserSettings> {
        if !path.exists() {
            return parse("");
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let settings: UserSettings =
            parse(&text).with_context(|| format!("parsing {}", path.display()))?;
        for error in settings.keyboard.overrides().1 {
            tracing::warn!(%error, "keyboard binding not applied");
        }
        Ok(settings)
    }
}

/// How often `settings.toml` is checked for changes made by hand.
const RECHECK: Duration = Duration::from_secs(1);

/// The user settings in force: `settings.toml` as last read. A change made by hand applies within
/// a second; one that does not parse leaves the previous settings in force (see `error`). The
/// app's settings window writes through `update`, which keeps the file's comments and layout.
pub struct LiveSettings {
    path: PathBuf,
    state: Mutex<Live>,
}

struct Live {
    settings: Arc<UserSettings>,
    modified: Option<SystemTime>,
    checked: Instant,
    error: Option<String>,
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl LiveSettings {
    /// Reads the file; one that does not parse stops the daemon from starting, as it always has.
    pub fn load(paths: &Paths) -> anyhow::Result<LiveSettings> {
        let settings = UserSettings::load(paths)?;
        Ok(LiveSettings {
            path: paths.settings.clone(),
            state: Mutex::new(Live {
                settings: Arc::new(settings),
                modified: modified(&paths.settings),
                checked: Instant::now(),
                error: None,
            }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The settings in force, read again when the file changed.
    pub fn get(&self) -> Arc<UserSettings> {
        let mut live = self.state.lock().unwrap();
        if live.checked.elapsed() >= RECHECK {
            live.checked = Instant::now();
            let now = modified(&self.path);
            if now != live.modified {
                live.modified = now;
                match UserSettings::read(&self.path) {
                    Ok(settings) => {
                        live.settings = Arc::new(settings);
                        live.error = None;
                    }
                    Err(error) => {
                        tracing::warn!(error = format!("{error:#}"), "settings.toml not applied");
                        live.error = Some(format!("{error:#}"));
                    }
                }
            }
        }
        live.settings.clone()
    }

    /// Why the file's current content is not in force, if it is not.
    pub fn error(&self) -> Option<String> {
        self.get();
        self.state.lock().unwrap().error.clone()
    }

    /// Changes the file through `edit` (comments and layout kept) and applies it at once. Refused
    /// when the file, or the result, is not valid.
    pub fn update(&self, edit: impl FnOnce(&mut toml_edit::DocumentMut)) -> anyhow::Result<()> {
        let mut live = self.state.lock().unwrap();
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", self.path.display()));
            }
        };
        let mut document: toml_edit::DocumentMut = text
            .parse()
            .with_context(|| format!("{} is not valid TOML: fix it first", self.path.display()))?;
        edit(&mut document);
        let text = document.to_string();
        let settings: UserSettings =
            parse(&text).context("the settings would no longer be valid")?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let staging = self.path.with_extension("toml.new");
        std::fs::write(&staging, &text)
            .with_context(|| format!("writing {}", staging.display()))?;
        std::fs::rename(&staging, &self.path)
            .with_context(|| format!("writing {}", self.path.display()))?;
        *live = Live {
            settings: Arc::new(settings),
            modified: modified(&self.path),
            checked: Instant::now(),
            error: None,
        };
        Ok(())
    }
}

/// Expands a leading `~/`.
pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(rest),
        None => PathBuf::from(path),
    }
}

/// A repo's scripts and files to copy, merged from its settings layers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepoSettings {
    pub setup: Option<String>,
    pub archive: Option<String>,
    pub run: Vec<RunScript>,
    /// `false` when only one workspace may run this repo's scripts at a time.
    pub run_concurrent: bool,
    /// Gitignored files copied from the clone into each new worktree (gitignore-style globs).
    pub files_to_copy: Vec<String>,
    /// The file the settings came from, for display.
    pub source: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunScript {
    pub name: String,
    pub command: String,
    pub default: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawRepoSettings {
    scripts: RawScripts,
    file_include_globs: Option<Vec<String>>,
    files: Option<RawFiles>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawFiles {
    include: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct RawScripts {
    setup: Option<String>,
    archive: Option<String>,
    run_mode: Option<String>,
    run: Option<RawRun>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum RawRun {
    Single(String),
    Named(BTreeMap<String, RawRunEntry>),
}

#[derive(Clone, Debug, Deserialize)]
struct RawRunEntry {
    command: String,
    #[serde(default)]
    default: bool,
    #[serde(default)]
    available_in: Option<Vec<String>>,
}

impl RepoSettings {
    /// Reads a repo's settings. Shared settings come from the worktree (the committed state of the
    /// base branch); machine-local ones from the clone, where they live untracked, and from the
    /// user's `[repos.<name>.scripts]` (`user`).
    ///
    /// Layers, first found wins per key: `<clone>/.chauffeur/settings.local.toml`, `user`,
    /// `<worktree>/.chauffeur/settings.toml`, `<clone>/.chauffeur/settings.toml`, then Conductor's
    /// committed `<worktree>/.conductor/settings.toml`. Conductor's machine-local files are
    /// ignored on purpose: they hold Conductor workarounds (such as the companion worktree
    /// script) that Chauffeur makes unnecessary. Files to copy default to `.worktreeinclude`,
    /// then `.env*`.
    pub fn load(
        clone: &Path,
        worktree: &Path,
        user: Option<&UserScripts>,
    ) -> anyhow::Result<RepoSettings> {
        let committed = [
            worktree.join(".conductor/settings.toml"),
            clone.join(".chauffeur/settings.toml"),
            worktree.join(".chauffeur/settings.toml"),
        ];
        let mut merged = RepoSettings {
            run_concurrent: true,
            ..Default::default()
        };
        let mut files_set = false;
        // Lowest priority first: each layer overwrites the keys it sets.
        for path in &committed {
            if let Some(raw) = read_layer(path)? {
                files_set |= merged.apply(raw, path);
            }
        }
        if let Some(UserScripts(scripts)) = user {
            merged.apply_scripts(scripts.clone());
        }
        let local = clone.join(".chauffeur/settings.local.toml");
        if let Some(raw) = read_layer(&local)? {
            files_set |= merged.apply(raw, &local);
        }
        if !files_set {
            let include_file = [
                worktree.join(".worktreeinclude"),
                clone.join(".worktreeinclude"),
            ]
            .into_iter()
            .find(|p| p.exists());
            merged.files_to_copy = match include_file {
                Some(file) => std::fs::read_to_string(&file)?
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty() && !line.starts_with('#'))
                    .map(String::from)
                    .collect(),
                None => vec![".env*".into(), "**/.env*".into()],
            };
        }
        Ok(merged)
    }

    /// Takes the keys `raw` sets; returns whether it set the files to copy.
    fn apply(&mut self, raw: RawRepoSettings, path: &Path) -> bool {
        self.apply_scripts(raw.scripts);
        self.source = Some(path.to_path_buf());
        let globs = raw
            .files
            .and_then(|files| files.include)
            .or(raw.file_include_globs);
        match globs {
            Some(globs) => {
                self.files_to_copy = globs;
                true
            }
            None => false,
        }
    }

    fn apply_scripts(&mut self, scripts: RawScripts) {
        if scripts.setup.is_some() {
            self.setup = scripts.setup;
        }
        if scripts.archive.is_some() {
            self.archive = scripts.archive;
        }
        if let Some(mode) = scripts.run_mode {
            self.run_concurrent = mode != "nonconcurrent";
        }
        if let Some(run) = scripts.run {
            self.run = match run {
                RawRun::Single(command) => vec![RunScript {
                    name: "run".into(),
                    command,
                    default: true,
                }],
                RawRun::Named(entries) => entries
                    .into_iter()
                    .filter(|(_, entry)| {
                        entry
                            .available_in
                            .as_ref()
                            .is_none_or(|places| places.iter().any(|p| p == "local"))
                    })
                    .map(|(name, entry)| RunScript {
                        name,
                        command: entry.command,
                        default: entry.default,
                    })
                    .collect(),
            };
        }
    }

    pub fn default_run(&self) -> Option<&RunScript> {
        self.run
            .iter()
            .find(|script| script.default)
            .or_else(|| self.run.first())
    }
}

/// A settings file, when it exists.
fn read_layer(path: &Path) -> anyhow::Result<Option<RawRepoSettings>> {
    if !path.exists() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text)
        .map(Some)
        .with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_users_have_no_team_repos_and_explicit_team_settings_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let settings = UserSettings::read(&dir.path().join("settings.toml")).unwrap();
        assert!(settings.github_org.is_empty());
        assert!(settings.default_repos.is_empty());
        assert!(settings.clone_roots.is_empty());
        assert!(settings.workers.project.is_empty());

        let configured: UserSettings = toml::from_str(
            r#"github_org = "example"
                default_repos = ["my-app"]
                clone_roots = ["~/code"]
                [workers]
                project = "my-cloud-project"
            "#,
        )
        .unwrap();
        assert_eq!(configured.github_org, "example");
        assert_eq!(configured.default_repos, ["my-app"]);
        assert_eq!(configured.clone_roots, ["~/code"]);
        assert_eq!(configured.workers.project, "my-cloud-project");
    }

    #[test]
    fn the_app_changes_settings_toml_keeping_its_comments_and_applies_it_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(
            dir.path().join("home"),
            dir.path().join("workspaces"),
            dir.path().join("repos"),
        );
        std::fs::create_dir_all(&paths.home).unwrap();
        std::fs::write(
            &paths.settings,
            "# my repos\ndefault_repos = [\"web\"] # the usual\n\n[repos.web]\npath = \"~/code/web\"\n",
        )
        .unwrap();
        let live = LiveSettings::load(&paths).unwrap();
        assert_eq!(live.get().default_repos, ["web"]);
        live.update(|doc| {
            doc["default_repos"] = toml_edit::value(toml_edit::Array::from_iter(["a", "b"]));
            doc["branch_prefix"] = toml_edit::value("jc");
        })
        .unwrap();
        assert_eq!(live.get().default_repos, ["a", "b"]);
        assert_eq!(live.get().branch_prefix, "jc");
        let text = std::fs::read_to_string(&paths.settings).unwrap();
        assert!(text.starts_with("# my repos\n"), "{text}");
        assert!(text.contains("[repos.web]"), "{text}");
        // Before the [repos.*] tables, so that it stays a top-level key.
        assert!(
            text.find("branch_prefix").unwrap() < text.find("[repos").unwrap(),
            "{text}"
        );

        // A change by hand applies; one that does not parse leaves the settings in force.
        std::thread::sleep(RECHECK + Duration::from_millis(50));
        std::fs::write(&paths.settings, "max_children = 2\n").unwrap();
        bump_modified(&paths.settings);
        std::thread::sleep(RECHECK + Duration::from_millis(50));
        assert_eq!(live.get().max_children, 2);
        std::fs::write(&paths.settings, "max_children = [").unwrap();
        bump_modified(&paths.settings);
        std::thread::sleep(RECHECK + Duration::from_millis(50));
        assert_eq!(live.get().max_children, 2);
        assert!(live.error().is_some());
        assert!(live.update(|_| {}).is_err());
    }

    /// Some file systems keep a modification time to the second.
    fn bump_modified(path: &Path) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
    }

    #[test]
    fn a_binding_the_app_cannot_use_is_left_out_with_its_reason_and_the_file_still_applies() {
        let settings: UserSettings = toml::from_str(
            "max_children = 2\n[keyboard.bindings]\n\"chat.new\" = \"⇧⌘T\"\n\"tab.next\" = \"⌃Tab\"\n\
             \"chat.nwe\" = \"⌘J\"\n\"chat.close\" = \"⌘Foo\"\n\"chat.reopen\" = \"⇧T\"\n",
        )
        .unwrap();
        assert_eq!(settings.max_children, 2);
        let (overrides, errors) = settings.keyboard.overrides();
        assert_eq!(
            overrides.into_iter().collect::<Vec<_>>(),
            [
                (ShortcutId::ChatNew, "⇧⌘T".to_string()),
                (ShortcutId::TabNext, "⌃Tab".to_string())
            ]
        );
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors.iter().any(|e| e.contains("chat.nwe")), "{errors:?}");
        assert!(
            errors.iter().any(|e| e.contains("\"Foo\" is not a key")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|e| e.contains("needs ⌘, ⌃ or ⌥")),
            "{errors:?}"
        );
        // Without the table, nothing changes.
        let settings: UserSettings = toml::from_str("[keyboard]\nvim_mode = true\n").unwrap();
        assert_eq!(settings.keyboard.overrides(), (BTreeMap::new(), vec![]));
    }

    #[test]
    fn one_binding_that_is_not_keys_in_quotes_is_left_out_and_the_rest_kept() {
        // `chat.new` without quotes is a dotted key: a table `chat` holding `new`.
        let settings: UserSettings = toml::from_str(
            "max_children = 2\n[keyboard.bindings]\nchat.new = \"⇧⌘J\"\n\"chat.close\" = 5\n\
             \"tab.next\" = \"⌃Tab\"\n",
        )
        .unwrap();
        assert_eq!(settings.max_children, 2);
        let (overrides, errors) = settings.keyboard.overrides();
        assert_eq!(
            overrides.into_iter().collect::<Vec<_>>(),
            [(ShortcutId::TabNext, "⌃Tab".to_string())]
        );
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(
            errors
                .iter()
                .any(|e| e.contains("an id goes in quotes") && e.contains("\"chat.new\" = \"⇧⌘J\"")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("\"chat.close\" = 5") && e.contains("keys go in quotes")),
            "{errors:?}"
        );
    }

    // The same cases as "a binding is modifiers once, …" and "reserved keys" in the app's
    // shortcuts.test.ts.
    #[test]
    fn bindings_name_modifiers_once_then_one_ascii_or_named_key() {
        for keys in [
            "⌘T", "⇧⌘t", "⌃⌘1", "⌥⌘1", "⌘\\", "⇧⌘/", "⌘↑", "⌃Tab", "⌘PgDn", "⌘F5", "⌘,", "⌘+",
            "⌘↵", "⌃`", "⌥1",
        ] {
            assert_eq!(binding_error(keys), None, "{keys}");
        }
        for keys in [
            "T", "⇧T", "⌥⌥T", "⌘⌘T", "⌘", "⌘TT", "⌘ ", "⌘F13", "⌘F01", "Ctrl+T", "⌘é", "⌘⌥", "⌘€",
        ] {
            assert!(binding_error(keys).is_some(), "{keys}");
        }
    }

    #[test]
    fn reserved_keys_are_refused_however_they_are_written() {
        for keys in [
            "⌘C", "⌘c", "⌘V", "⌘X", "⌘A", "⌘Z", "⇧⌘Z", "⌘⇧Z", "⌘Q", "⌘D", "⌘H", "⌘M", "⌃C", "⌃D",
            "⌃Z", "⌃⌘F", "⌘⌃F",
        ] {
            let error = binding_error(keys);
            assert!(
                error.as_deref().is_some_and(|e| e.contains("stays")),
                "{keys}: {error:?}"
            );
        }
        for keys in ["⇧⌘C", "⌃⌘C", "⇧⌘V", "⌃⇧C", "⌘F", "⌃⌘Z", "⌥⌘C"] {
            assert_eq!(binding_error(keys), None, "{keys}");
        }
        assert_eq!(
            binding_error("⌘C").as_deref(),
            Some(
                "\"⌘C\" stays Copy, and the terminal's interrupt outside macOS: choose other keys"
            )
        );
    }

    #[test]
    fn keep_awake_defaults_to_while_working_and_reads_its_three_values() {
        let settings: UserSettings = toml::from_str("").unwrap();
        assert_eq!(settings.keep_awake, KeepAwake::WhileWorking);
        assert_eq!(settings.keep_awake_min_battery, 20);
        for (text, mode) in [
            ("never", KeepAwake::Never),
            ("while_working", KeepAwake::WhileWorking),
            ("lid_closed", KeepAwake::LidClosed),
        ] {
            let settings: UserSettings = toml::from_str(&format!(
                "keep_awake = \"{text}\"\nkeep_awake_min_battery = 35\n"
            ))
            .unwrap();
            assert_eq!(settings.keep_awake, mode);
            assert_eq!(settings.keep_awake_min_battery, 35);
        }
        assert!(toml::from_str::<UserSettings>("keep_awake = \"always\"").is_err());
    }

    #[test]
    fn core_names_no_hosted_service_and_a_distribution_supplies_its_own_under_the_users_file() {
        let core = parse_with(None, "").unwrap();
        assert!(!core.office.enabled());
        assert!(
            core.office.google_client_id.is_empty() && core.office.google_hosted_domain.is_empty()
        );
        assert!(core.office.board_repos.is_empty());
        assert!(core.design_share.url.is_empty() && core.slack.url.is_empty());
        assert!(core.distribution.member_domain.is_empty());
        assert!(core.design_share.link_urls.is_empty());

        let defaults: toml::Table = toml::from_str(
            "[office]\nurl = \"wss://office.example/ws\"\ngoogle_client_id = \"id\"\ngoogle_hosted_domain = \"example.com\"\n[design_share]\nurl = \"https://share.example\"\nlink_urls = [\"https://old.example\"]\n",
        )
        .unwrap();
        let shipped = parse_with(Some(&defaults), "").unwrap();
        assert!(shipped.office.enabled());
        assert_eq!(shipped.office.url, "wss://office.example/ws");
        assert_eq!(shipped.design_share.url, "https://share.example");
        assert_eq!(shipped.design_share.link_urls, ["https://old.example"]);
        // A key the user set wins, even empty; the section's other keys keep the distribution's.
        let user = parse_with(Some(&defaults), "max_children = 3\n[office]\nurl = \"\"\n").unwrap();
        assert!(!user.office.enabled());
        assert_eq!(user.office.google_client_id, "id");
        assert_eq!(user.office.google_hosted_domain, "example.com");
        assert_eq!(user.max_children, 3);
        // The user's own mistakes keep their position.
        let error = parse_with(Some(&defaults), "max_children = \"three\"\n").unwrap_err();
        assert!(format!("{error:#}").contains("line 1"), "{error:#}");
    }

    #[test]
    fn a_member_is_an_email_of_the_member_domain_and_nobody_is_one_without_it() {
        let distribution = DistributionSettings {
            member_domain: "Example.com".into(),
        };
        assert!(distribution.is_member("alice@example.com"));
        assert!(distribution.is_member("Bob@EXAMPLE.COM"));
        assert!(!distribution.is_member("carol@other.com"));
        assert!(!distribution.is_member("dave@sub.example.com"));
        assert!(!distribution.is_member("example.com"));
        assert!(!DistributionSettings::default().is_member("alice@example.com"));
    }

    #[test]
    fn the_office_is_off_without_a_url_or_when_disabled() {
        let office =
            toml::from_str::<UserSettings>("[office]\nurl = \"ws://127.0.0.1:41250/ws\"\n")
                .unwrap()
                .office;
        assert!(office.enabled());
        let office = toml::from_str::<UserSettings>("[office]\nurl = \"\"\nenabled = true\n")
            .unwrap()
            .office;
        assert!(!office.enabled());
        let office = toml::from_str::<UserSettings>(
            "[office]\nurl = \"ws://127.0.0.1:41250/ws\"\nenabled = false\n",
        )
        .unwrap()
        .office;
        assert!(!office.enabled());
    }

    #[test]
    fn settings_of_older_versions_still_load() {
        // auto_archive_on_merge was removed on 2026-10-14; users may still have it.
        let settings: UserSettings =
            toml::from_str("auto_archive_on_merge = true\nmax_children = 3\n").unwrap();
        assert_eq!(settings.max_children, 3);
    }

    #[test]
    fn conductor_settings_are_read_when_chauffeur_has_none() {
        let clone = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(worktree.path().join(".conductor")).unwrap();
        std::fs::write(
            worktree.path().join(".conductor/settings.toml"),
            r#"
[scripts]
setup = "pnpm install"
run_mode = "nonconcurrent"

[scripts.run.dev]
command = "pnpm dev --port $CONDUCTOR_PORT"
default = true

[scripts.run.cloud-only]
command = "echo cloud"
available_in = ["cloud"]
"#,
        )
        .unwrap();
        let settings = RepoSettings::load(clone.path(), worktree.path(), None).unwrap();
        assert_eq!(settings.setup.as_deref(), Some("pnpm install"));
        assert!(!settings.run_concurrent);
        assert_eq!(settings.run.len(), 1);
        assert_eq!(settings.default_run().unwrap().name, "dev");
        assert_eq!(settings.files_to_copy, vec![".env*", "**/.env*"]);
    }

    #[test]
    fn conductor_machine_local_settings_are_ignored() {
        let clone = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(clone.path().join(".conductor")).unwrap();
        std::fs::write(
            clone.path().join(".conductor/settings.local.toml"),
            "[scripts]\nsetup = \"bash scripts/conductor/worktree.sh setup\"\n",
        )
        .unwrap();
        std::fs::write(
            clone.path().join(".conductor/settings.toml"),
            "[scripts]\nsetup = \"untracked in the clone\"\n",
        )
        .unwrap();
        let settings = RepoSettings::load(clone.path(), worktree.path(), None).unwrap();
        assert_eq!(settings.setup, None);
    }

    #[test]
    fn local_chauffeur_settings_win_over_shared_ones() {
        let clone = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(clone.path().join(".chauffeur")).unwrap();
        std::fs::create_dir_all(worktree.path().join(".chauffeur")).unwrap();
        std::fs::write(
            worktree.path().join(".chauffeur/settings.toml"),
            "[scripts]\nsetup = \"shared\"\narchive = \"shared-archive\"\n",
        )
        .unwrap();
        std::fs::write(
            clone.path().join(".chauffeur/settings.local.toml"),
            "[scripts]\nsetup = \"local\"\n",
        )
        .unwrap();
        std::fs::write(
            worktree.path().join(".worktreeinclude"),
            "# comment\n.env\nconfig/*.pem\n",
        )
        .unwrap();
        let settings = RepoSettings::load(clone.path(), worktree.path(), None).unwrap();
        assert_eq!(settings.setup.as_deref(), Some("local"));
        assert_eq!(settings.archive.as_deref(), Some("shared-archive"));
        assert_eq!(settings.files_to_copy, vec![".env", "config/*.pem"]);
    }
}
