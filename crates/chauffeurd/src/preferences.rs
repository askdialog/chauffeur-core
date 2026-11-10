//! The app's settings window: what it shows (`settings.get`) and changes (`settings.set`). Workspace
//! defaults and Linear's switches go to `settings.toml` (its comments kept), the API keys to the
//! user's secrets, Jev to the store, starting at login to a LaunchAgent or a systemd unit.

use std::sync::Arc;

use chauffeur_protocol::client::{
    AppSettings, Appearance, AppearanceUpdate, KeyboardSettings, KeyboardUpdate, LinearSettings,
    NotificationSettings, PowerSettings, PowerUpdate, RepoSetting, SearchUpdate, SettingsUpdate,
    WorkspaceDefaults, WorkspaceDefaultsUpdate,
};
use chauffeur_protocol::model::{AgentKind, KeepAwake};

use crate::linear::Linear;
use crate::login::Login;
use crate::{Daemon, Error, Result};

pub async fn get(d: &Arc<Daemon>) -> Result<AppSettings> {
    let settings = d.settings.get();
    // Read on every call: the user's tools rewrite the theme when the wallpaper changes.
    let theme = settings.appearance.terminal_theme.trim();
    let (palette, palette_error) = if theme.is_empty() {
        (None, None)
    } else {
        match crate::palette::read(theme) {
            Ok(palette) => (Some(palette), None),
            Err(error) => (None, Some(error)),
        }
    };
    Ok(AppSettings {
        daily_review: settings.daily_review.clone(),
        daily_review_error: d
            .store
            .kv_get("daily_review.error")?
            .or(d.store.kv_get("daily_review.wake_error")?),
        workspaces: WorkspaceDefaults {
            default_repos: settings.default_repos.clone(),
            repo_order: settings.repo_order.clone(),
            branch_prefix: settings.branch_prefix.clone(),
            // Known once a workspace was created: `gh` is not asked here.
            github_login: d.store.kv_get("gh_login")?,
            clone_roots: settings.clone_roots.clone(),
            github_org: settings.github_org.clone(),
            github_sources: settings.github_sources,
            max_children: settings.max_children as u32,
            port_range_start: settings.port_range_start,
            port_block_size: settings.port_block_size,
            archive_script_timeout_secs: settings.archive_script_timeout_secs.min(u32::MAX.into())
                as u32,
            snapshot_retention_days: settings.snapshot_retention_days.max(0) as u32,
            idle_session_minutes: settings.idle_session_minutes.min(u32::MAX.into()) as u32,
            repos: settings
                .repos
                .iter()
                .map(|(name, repo)| RepoSetting {
                    name: name.clone(),
                    path: repo.path.clone(),
                    default_branch: repo.default_branch.clone(),
                })
                .collect(),
        },
        linear: LinearSettings {
            enabled: settings.linear_enabled,
            start_tickets: settings.linear_start_tickets,
            key_saved: d.linear.key_saved(),
            key_from_env: Linear::key_from_env().is_some(),
        },
        jev: d.jev.settings(&d.store)?,
        appearance: Appearance {
            interface_font: settings.appearance.interface_font.clone(),
            code_font: settings.appearance.code_font.clone(),
            code_size: settings.appearance.code_size,
            terminal_font: settings.appearance.terminal_font.clone(),
            terminal_size: settings.appearance.terminal_size,
            terminal_theme: settings.appearance.terminal_theme.clone(),
            theme_interface: settings.appearance.theme_interface,
            palette,
            palette_error,
        },
        notifications: NotificationSettings {
            sound: settings.notifications.sound.clone(),
            sound_error: match crate::sound::kind(&settings.notifications.sound) {
                crate::sound::Sound::File(path) => crate::sound::read(path).err(),
                _ => None,
            },
        },
        keyboard: {
            let (bindings, binding_errors) = settings.keyboard.overrides();
            KeyboardSettings {
                vim_mode: settings.keyboard.vim_mode,
                bindings,
                binding_errors,
            }
        },
        power: PowerSettings {
            keep_awake: settings.keep_awake,
            min_battery_percent: settings.keep_awake_min_battery,
            lid_closed_available: d.power.lid_closed_available(),
            sudoers_rule: d.power.sudoers_rule(),
        },
        search: settings.search.clone(),
        default_agent: settings.default_agent.clone(),
        default_model: settings.default_model.clone(),
        daemon_port: settings.daemon_port,
        start_at_login: Login::for_paths(&d.paths).enabled(),
        prompt_suggestions: settings.prompt_suggestions,
        workers: Some(chauffeur_protocol::client::WorkerConfiguration {
            project: settings.workers.project.clone(),
            zone: settings.workers.zone.clone(),
        }),
        transfer: chauffeur_protocol::client::TransferSettings {
            controller: settings.transfer.controller.clone(),
            machines: settings
                .transfer
                .machines
                .iter()
                .map(|m| chauffeur_protocol::client::TransferMachine {
                    name: m.name.clone(),
                    socket: m.socket.clone(),
                })
                .collect(),
        },
        error_reports: settings.error_reports,
        error_reports_available: d.error_reports.available(),
        file: d.settings.path().display().to_string(),
        file_error: d.settings.error(),
    })
}

pub async fn set(d: &Arc<Daemon>, update: SettingsUpdate) -> Result<AppSettings> {
    if let Some(workspaces) = &update.workspaces {
        set_workspace_limits(d, workspaces)?;
    }
    set_agent_and_port(d, &update)?;
    if let Some(workspaces) = update.workspaces {
        let repos = workspaces.default_repos.map(cleaned);
        let order = workspaces.repo_order.map(cleaned);
        let roots = workspaces.clone_roots.map(cleaned);
        let prefix = workspaces.branch_prefix.map(|p| p.trim().to_string());
        if let Some(prefix) = &prefix {
            check_prefix(prefix)?;
        }
        d.settings
            .update(|doc| {
                if let Some(repos) = &repos {
                    doc["default_repos"] = strings(repos);
                }
                if let Some(order) = &order {
                    doc["repo_order"] = strings(order);
                }
                if let Some(roots) = &roots {
                    doc["clone_roots"] = strings(roots);
                }
                match prefix.as_deref() {
                    Some("") => {
                        doc.remove("branch_prefix");
                    }
                    Some(prefix) => doc["branch_prefix"] = toml_edit::value(prefix),
                    None => {}
                }
            })
            .map_err(refused)?;
    }
    if let Some(linear) = update.linear {
        if let Some(key) = &linear.api_key {
            d.linear.save_key(key).map_err(refused)?;
        }
        if linear.enabled.is_some() || linear.start_tickets.is_some() {
            d.settings
                .update(|doc| {
                    if let Some(enabled) = linear.enabled {
                        doc["linear_enabled"] = toml_edit::value(enabled);
                    }
                    if let Some(start) = linear.start_tickets {
                        doc["linear_start_tickets"] = toml_edit::value(start);
                    }
                })
                .map_err(refused)?;
        }
    }
    if let Some(jev) = update.jev {
        let before = d.jev.session_params(&d.store);
        d.jev.update(&d.store, jev)?;
        // Idle chats take the change at their next message.
        if d.jev.session_params(&d.store) != before {
            d.chats.restart_idle(AgentKind::Claude).await;
            d.chats.restart_idle(AgentKind::Codex).await;
        }
    }
    if let Some(appearance) = update.appearance {
        set_appearance(d, appearance)?;
    }
    if let Some(sound) = update.notifications.and_then(|n| n.sound) {
        let sound = sound.trim().to_string();
        if sound.len() > 1024 || sound.contains('\n') {
            return Err(Error::BadRequest("not a sound or a file path".into()));
        }
        d.settings
            .update(|doc| {
                if !doc.contains_key("notifications") {
                    doc["notifications"] = toml_edit::table();
                }
                if sound.is_empty() || sound == "none" {
                    doc["notifications"]
                        .as_table_like_mut()
                        .map(|t| t.remove("sound"));
                } else {
                    doc["notifications"]["sound"] = toml_edit::value(sound.as_str());
                }
            })
            .map_err(refused)?;
    }
    if let Some(keyboard) = update.keyboard {
        set_keyboard(d, keyboard)?;
    }
    if let Some(search) = update.search {
        set_search(d, search)?;
    }
    if let Some(power) = update.power {
        set_power(d, power).await?;
    }
    if let Some(suggest) = update.prompt_suggestions
        && suggest != d.settings.get().prompt_suggestions
    {
        d.settings
            .update(|doc| doc["prompt_suggestions"] = toml_edit::value(suggest))
            .map_err(refused)?;
        // Idle Claude chats take the change at their next message.
        d.chats.restart_idle(AgentKind::Claude).await;
    }
    if let Some(mode) = update.error_reports
        && mode != d.settings.get().error_reports
    {
        d.settings
            .update(|doc| doc["error_reports"] = toml_edit::value(mode.as_str()))
            .map_err(refused)?;
    }
    if let Some(review) = update.daily_review {
        crate::daily_review::set_settings(d, review).await?;
    }
    if let Some(transfer) = update.transfer {
        set_transfer(d, transfer)?;
    }
    if let Some(workers) = update.workers {
        set_workers(d, workers)?;
    }
    if let Some(start) = update.start_at_login {
        let login = Login::for_paths(&d.paths);
        if start {
            login.enable()
        } else {
            login.disable()
        }
        .map_err(refused)?;
    }
    get(d).await
}

/// `keep_awake` and its battery threshold. Choosing `lid_closed` installs the sudoers rule that
/// lets the daemon run `pmset -a disablesleep 1|0`, and leaving it removes the rule once sleep is
/// back on, each behind one administrator password prompt, which the daemon asks macOS to show
/// (`osascript`, in the user's session: the daemon owns every write, and the app waits on this
/// call meanwhile). A prompt cancelled or failed leaves the setting as it was, and says why.
fn set_search(d: &Arc<Daemon>, update: SearchUpdate) -> Result<()> {
    let model = update.embedding_model.map(|model| model.trim().to_string());
    if let Some(model) = model.as_deref().filter(|model| !model.is_empty()) {
        crate::ollama::check_name(model)?;
    }
    d.settings
        .update(|doc| {
            if !doc.contains_key("search") {
                doc["search"] = toml_edit::table();
            }
            if let Some(semantic) = update.semantic {
                doc["search"]["semantic"] = toml_edit::value(semantic);
            }
            match model.as_deref() {
                Some("") => {
                    doc["search"]
                        .as_table_like_mut()
                        .map(|t| t.remove("embedding_model"));
                }
                Some(model) => doc["search"]["embedding_model"] = toml_edit::value(model),
                None => {}
            }
        })
        .map_err(refused)?;
    d.search.wake();
    Ok(())
}

async fn set_power(d: &Arc<Daemon>, update: PowerUpdate) -> Result<()> {
    let min_battery = update
        .min_battery_percent
        .map(|v| in_range("Battery threshold", v, 5, 95))
        .transpose()?;
    let current = d.settings.get().keep_awake;
    let mode = update.keep_awake.filter(|mode| *mode != current);
    if mode == Some(KeepAwake::LidClosed) && !d.power.lid_closed_available() {
        return Err(Error::BadRequest(
            "keeping the machine awake with the lid closed works on macOS only".into(),
        ));
    }
    let power = d.clone();
    let changed = tokio::task::spawn_blocking(move || -> Result<()> {
        match mode {
            Some(KeepAwake::LidClosed) if !power.power.sudoers_rule() => {
                power.power.install_rule().map_err(|error| {
                    Error::BadRequest(format!("Lid closed mode was not turned on: {error}"))
                })
            }
            Some(_) if current == KeepAwake::LidClosed => {
                // Sleep back on first, while the rule still allows it; kept on until the end.
                power.power.give_sleep_back().map_err(|error| {
                    Error::BadRequest(format!(
                        "Lid closed mode was kept: sleep could not be turned back on ({error})"
                    ))
                })?;
                if power.power.sudoers_rule() {
                    power.power.remove_rule().map_err(|error| {
                        Error::BadRequest(format!(
                            "Lid closed mode was kept, its sudoers rule could not be removed: {error}"
                        ))
                    })?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    })
    .await
    .map_err(|error| Error::Internal(anyhow::anyhow!("{error}")));
    let written = changed.and_then(|result| result).and_then(|()| {
        if mode.is_none() && min_battery.is_none() {
            return Ok(());
        }
        d.settings
            .update(|doc| {
                if let Some(mode) = mode {
                    doc["keep_awake"] = toml_edit::value(mode.as_str());
                }
                if let Some(min) = min_battery {
                    doc["keep_awake_min_battery"] = toml_edit::value(i64::from(min));
                }
            })
            .map_err(refused)
    });
    d.power.unblock_lid();
    written
}

/// What `settings.toml` starts as: every key the window does not show, commented out at its
/// default value.
const TEMPLATE: &str = r#"# Chauffeur's settings. The app's Settings (⌘,) change some of them here; a change made by hand
# applies within a second. Each key below is commented out at its default value.

# The main GitHub organization: its repos are named without their owner.
# github_org = ""
# Also list the repos of every other owner your GitHub account reaches (yours, your other
# organizations', the ones you collaborate on).
# github_sources = true
# default_repos = []
# clone_roots = []

# The agent and model of a chat started without one (`chauffeur new`). Unset: the first favorite
# of the app's model picker.
# default_agent = "claude"
# default_model = "claude-opus-5-5"

# Child workspaces one workspace may have at once.
# max_children = 5

# First port handed out to workspaces, and ports per workspace.
# port_range_start = 41000
# port_block_size = 10

# Port of the daemon's WebSocket.
# daemon_port = 47833

# Seconds a repo's archive script may run.
# archive_script_timeout_secs = 300

# Days an archived workspace's snapshot is kept.
# snapshot_retention_days = 30

# Minutes without activity after which an agent's session is closed, to free its memory (the next
# message reopens it with the whole conversation), and the workspace's Nx daemons are stopped when
# nothing else runs there (the next nx command starts one). 0 keeps them.
# idle_session_minutes = 30

# Keep the machine awake while agents work: "never"; "while_working" (no idle sleep, the display
# may sleep, closing a laptop's lid still sleeps it); or "lid_closed" (macOS: no sleep at all,
# even with the lid closed, through a sudoers rule the settings window installs with your
# administrator password; given back on battery below keep_awake_min_battery percent).
# keep_awake = "while_working"
# keep_awake_min_battery = 20

# After each turn, Claude suggests your next message, offered in the empty composer (Tab takes it).
# prompt_suggestions = true

# Report the errors the app shows to Chauffeur's developers (Sentry): "always", "ask" (each error
# offers to send its report) or "never". A report carries the error, the versions and the daemon's
# log lines of the failed request, without your home folder nor anything like a token, and an
# anonymous id. Only packaged builds send any.
# error_reports = "ask"

# Daily session review at 18:00 local time. Model unset: the usual default.
# [daily_review]
# enabled = true
# agent = "claude"
# model = "claude-opus-5-5"
# wake = false

# When Chauffeur restarts, or its agent process crashes, in the middle of an agent's turn, the agent
# resumes the turn on its own, told that its commands were killed; once only, so a turn that keeps
# crashing waits for your message. A turn that failed on a network or overloaded-API error is
# resumed the same way when you come back to the app. false: every such chat waits for your message.
# resume_interrupted_turns = true

# The app's fonts: a family as CSS names it ("JetBrains Mono", or several separated by commas)
# and a size in pixels; unset, the app's own. The interface's size is the app's zoom (⌘+ / ⌘−).
# [appearance]
# interface_font = "Inter"
# code_font = "JetBrains Mono"        # the Files editor, the Diff and the code in chats
# code_size = 12
# terminal_font = "JetBrains Mono"
# terminal_size = 12
# A colour theme your tools write (pywal JSON, kitty, Ghostty, foot or Alacritty; wallust from a
# template), read again
# when it changes; the terminal takes its colours, the code its 16 ANSI colours.
# terminal_theme = "~/.cache/wal/colors.json"
# The interface too (background, panels, text, accent; a light theme is ignored, the interface is
# dark only):
# theme_interface = true

# The sound played when a chat finishes or needs you while the app is not in front: one of the
# app's own ("chime", "bell", "station"), or an audio file of yours (MP3, WAV, AIFF, M4A, OGG,
# FLAC, at most 5 MB). Unset: no sound.
# [notifications]
# sound = "station"

# The Files editor takes Vim's keys (normal, insert and visual modes, :w saves).
# [keyboard]
# vim_mode = false

# Other keys for the app's shortcuts (Settings › Keyboard lists them), written the macOS way:
# ⌘ is Ctrl elsewhere, ⌃ Ctrl+Alt there, ⌥ Alt, ⇧ Shift.
# [keyboard.bindings]
# "chat.new" = "⇧⌘T"

# Cloud workers need your own Google Cloud project before one can be created.
# [workers]
# project = ""
# zone = "europe-west9-a"

# A clone outside the clone folders, or another default branch, for one repo:
# [repos.my-app]
# path = "~/code/my-app"
# default_branch = "main"
"#;

/// Writes `settings.toml` from the template when it does not exist.
pub fn write_template(d: &Daemon) -> Result<()> {
    let path = d.settings.path();
    if !path.exists() {
        std::fs::write(path, TEMPLATE)?;
    }
    Ok(())
}

/// Within reach of what the daemon can use: a number out of range is refused, not clamped.
fn in_range<T: PartialOrd + std::fmt::Display + Copy>(
    what: &str,
    value: T,
    min: T,
    max: T,
) -> Result<T> {
    if value < min || value > max {
        return Err(Error::BadRequest(format!(
            "{what}: {value} is not between {min} and {max}"
        )));
    }
    Ok(value)
}

/// The organization, the limits, the ports, the delays and the `[repos.*]` tables.
fn set_workspace_limits(d: &Daemon, update: &WorkspaceDefaultsUpdate) -> Result<()> {
    let mut numbers: Vec<(&str, i64)> = Vec::new();
    if let Some(v) = update.max_children {
        numbers.push((
            "max_children",
            in_range("Child workspaces", v, 0, 50)?.into(),
        ));
    }
    if let Some(v) = update.port_range_start {
        numbers.push((
            "port_range_start",
            in_range("First port", v, 1024, 64000)?.into(),
        ));
    }
    if let Some(v) = update.port_block_size {
        numbers.push((
            "port_block_size",
            in_range("Ports per workspace", v, 1, 100)?.into(),
        ));
    }
    if let Some(v) = update.archive_script_timeout_secs {
        let v = in_range("Archive script timeout", v, 10, 3600)?;
        numbers.push(("archive_script_timeout_secs", v as i64));
    }
    if let Some(v) = update.snapshot_retention_days {
        numbers.push((
            "snapshot_retention_days",
            in_range("Snapshot retention", v, 1, 365)?.into(),
        ));
    }
    if let Some(v) = update.idle_session_minutes {
        let v = in_range("Idle agents", v, 0, 1440)?;
        numbers.push(("idle_session_minutes", v as i64));
    }
    let org = update.github_org.as_ref().map(|o| o.trim().to_string());
    if let Some(org) = &org
        && !org.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(Error::BadRequest(format!(
            "\"{org}\" is not a GitHub organization name"
        )));
    }
    let repos = update
        .repos
        .as_ref()
        .map(|repos| check_repos(repos))
        .transpose()?;
    let sources = update.github_sources;
    if numbers.is_empty() && org.is_none() && repos.is_none() && sources.is_none() {
        return Ok(());
    }
    d.settings
        .update(|doc| {
            for (key, value) in &numbers {
                doc[key] = toml_edit::value(*value);
            }
            if let Some(org) = &org {
                // Empty: only the repos named in [repos.*] can be picked.
                doc["github_org"] = toml_edit::value(org.as_str());
            }
            if let Some(sources) = sources {
                doc["github_sources"] = toml_edit::value(sources);
            }
            if let Some(repos) = &repos {
                doc.remove("repos");
                if !repos.is_empty() {
                    let mut tables = toml_edit::Table::new();
                    tables.set_implicit(true);
                    for repo in repos {
                        let mut table = toml_edit::Table::new();
                        if let Some(path) = &repo.path {
                            table["path"] = toml_edit::value(path.as_str());
                        }
                        if let Some(branch) = &repo.default_branch {
                            table["default_branch"] = toml_edit::value(branch.as_str());
                        }
                        tables.insert(&repo.name, toml_edit::Item::Table(table));
                    }
                    doc["repos"] = toml_edit::Item::Table(tables);
                }
            }
        })
        .map_err(refused)
}

/// A repo's name, clone path and default branch, trimmed; blanks dropped, a name only once.
fn check_repos(repos: &[RepoSetting]) -> Result<Vec<RepoSetting>> {
    let mut out: Vec<RepoSetting> = Vec::new();
    for repo in repos {
        let name = repo.name.trim().to_string();
        let path = repo
            .path
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty());
        let branch = repo
            .default_branch
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty());
        if name.is_empty() {
            continue;
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            || name.starts_with('.')
        {
            return Err(Error::BadRequest(format!("\"{name}\" is not a repo name")));
        }
        if out.iter().any(|r| r.name == name) {
            return Err(Error::BadRequest(format!("{name} is listed twice")));
        }
        if path.is_some_and(|p| p.len() > 1024 || p.contains('\n')) {
            return Err(Error::BadRequest(format!("{name}: not a folder path")));
        }
        if let Some(branch) = branch
            && (check_prefix(branch).is_err() || branch.contains(' '))
        {
            return Err(Error::BadRequest(format!(
                "{name}: \"{branch}\" is not a branch name"
            )));
        }
        out.push(RepoSetting {
            name,
            path: path.map(str::to_string),
            default_branch: branch.map(str::to_string),
        });
    }
    Ok(out)
}

/// The agent and model of chats started without one, and the daemon's port.
fn set_agent_and_port(d: &Daemon, update: &SettingsUpdate) -> Result<()> {
    let agent = update.default_agent.as_ref().map(|a| a.trim().to_string());
    if let Some(agent) = agent.as_deref()
        && !agent.is_empty()
        && AgentKind::parse(agent).is_none()
    {
        return Err(Error::BadRequest(format!(
            "\"{agent}\" is not an agent: claude or codex"
        )));
    }
    let model = update.default_model.as_ref().map(|m| m.trim().to_string());
    if let Some(model) = model.as_deref()
        && (model.len() > 200 || model.contains(['\n', '"']))
    {
        return Err(Error::BadRequest("not a model name".into()));
    }
    let port = update
        .daemon_port
        .map(|p| in_range("Daemon port", p, 1024, 65535))
        .transpose()?;
    if agent.is_none() && model.is_none() && port.is_none() {
        return Ok(());
    }
    d.settings
        .update(|doc| {
            for (key, value) in [("default_agent", &agent), ("default_model", &model)] {
                match value.as_deref() {
                    Some("") => {
                        doc.remove(key);
                    }
                    Some(value) => doc[key] = toml_edit::value(value),
                    None => {}
                }
            }
            if let Some(port) = port {
                doc["daemon_port"] = toml_edit::value(i64::from(port));
            }
        })
        .map_err(refused)
}

/// Writes the fonts to `[appearance]`; an empty font or a 0 size removes its key.
fn set_appearance(d: &Daemon, update: AppearanceUpdate) -> Result<()> {
    let mut fonts = [
        ("interface_font", update.interface_font),
        ("code_font", update.code_font),
        ("terminal_font", update.terminal_font),
    ]
    .into_iter()
    .filter_map(|(key, value)| value.map(|v| font_family(&v).map(|v| (key, v))))
    .collect::<Result<Vec<_>>>()?;
    // A path, kept as the user wrote it (`~/` allowed); whether it reads is shown, not enforced:
    // their tools may write it later.
    if let Some(theme) = update.terminal_theme {
        let theme = theme.trim().to_string();
        if theme.len() > 1024 || theme.contains('\n') {
            return Err(Error::BadRequest("not a file path".into()));
        }
        fonts.push(("terminal_theme", theme));
    }
    let sizes = [
        ("code_size", update.code_size),
        ("terminal_size", update.terminal_size),
    ]
    .into_iter()
    .filter_map(|(key, value)| value.map(|v| font_size(v).map(|v| (key, v))))
    .collect::<Result<Vec<_>>>()?;
    if fonts.is_empty() && sizes.is_empty() && update.theme_interface.is_none() {
        return Ok(());
    }
    d.settings
        .update(|doc| {
            if !doc.contains_key("appearance") {
                doc["appearance"] = toml_edit::table();
            }
            let table = &mut doc["appearance"];
            for (key, font) in &fonts {
                if font.is_empty() {
                    table.as_table_like_mut().map(|t| t.remove(key));
                } else {
                    table[key] = toml_edit::value(font.as_str());
                }
            }
            for (key, size) in &sizes {
                if *size == 0 {
                    table.as_table_like_mut().map(|t| t.remove(key));
                } else {
                    table[key] = toml_edit::value(i64::from(*size));
                }
            }
            match update.theme_interface {
                Some(true) => table["theme_interface"] = toml_edit::value(true),
                Some(false) => {
                    table
                        .as_table_like_mut()
                        .map(|t| t.remove("theme_interface"));
                }
                None => {}
            }
        })
        .map_err(refused)
}

/// A font family as CSS names it, one or several separated by commas: letters, digits, spaces,
/// quotes, dashes and dots only, so that it cannot carry other CSS.
fn font_family(value: &str) -> Result<String> {
    let value = value.trim();
    let fine = value.len() <= 200
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || " ,-_.'\"".contains(c));
    if !fine {
        return Err(Error::BadRequest(format!(
            "{value:?} is not a font name: letters, digits, spaces, commas and quotes only"
        )));
    }
    Ok(value.to_string())
}

/// A font size in pixels, 0 for the app's own.
fn font_size(size: u16) -> Result<u16> {
    if size == 0 || (8..=32).contains(&size) {
        Ok(size)
    } else {
        Err(Error::BadRequest(format!(
            "a font size of {size} px: from 8 to 32"
        )))
    }
}

/// Vim mode, and the shortcuts' keys: the whole `[keyboard.bindings]` table at once, refused when
/// one of them is not a binding.
fn set_keyboard(d: &Daemon, keyboard: KeyboardUpdate) -> Result<()> {
    for (id, keys) in keyboard.bindings.iter().flatten() {
        if let Some(error) = crate::settings::binding_error(keys) {
            return Err(Error::BadRequest(format!("{}: {error}", id.name())));
        }
    }
    d.settings
        .update(|doc| {
            if !doc.contains_key("keyboard") {
                doc["keyboard"] = toml_edit::table();
            }
            if let Some(vim) = keyboard.vim_mode {
                doc["keyboard"]["vim_mode"] = toml_edit::value(vim);
            }
            match keyboard.bindings {
                Some(bindings) if bindings.is_empty() => {
                    if let Some(table) = doc["keyboard"].as_table_like_mut() {
                        table.remove("bindings");
                    }
                }
                Some(bindings) => {
                    let mut table = toml_edit::Table::new();
                    for (id, keys) in bindings {
                        table[&id.name()] = toml_edit::value(keys);
                    }
                    doc["keyboard"]["bindings"] = toml_edit::Item::Table(table);
                }
                None => {}
            }
        })
        .map_err(refused)
}

fn refused(error: anyhow::Error) -> Error {
    Error::BadRequest(format!("{error:#}"))
}

/// Trimmed, without blanks or repeats, in order.
fn cleaned(values: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in values {
        let value = value.trim().to_string();
        if !value.is_empty() && !out.contains(&value) {
            out.push(value);
        }
    }
    out
}

fn strings(values: &[String]) -> toml_edit::Item {
    toml_edit::value(toml_edit::Array::from_iter(
        values.iter().map(String::as_str),
    ))
}

/// What git accepts at the head of a branch name, kept simple: `jc`, `team/jc`.
fn check_prefix(prefix: &str) -> Result<()> {
    let fine = prefix.is_empty()
        || (prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
            && !prefix.starts_with(['/', '.', '-'])
            && !prefix.ends_with(['/', '.'])
            && !prefix.contains("//")
            && !prefix.contains(".."));
    if fine {
        Ok(())
    } else {
        Err(Error::BadRequest(format!(
            "\"{prefix}\" cannot start a branch name: use letters, digits, - _ . and /"
        )))
    }
}

/// `[transfer]` replaced whole: each machine named once, every socket absolute or under `~/`.
fn set_transfer(d: &Daemon, transfer: chauffeur_protocol::client::TransferSettings) -> Result<()> {
    let socket_ok = |s: &str| s.starts_with('/') || s.starts_with("~/");
    let workers = crate::workers::list(d)?.workers;
    let mut names = std::collections::BTreeSet::new();
    for machine in &transfer.machines {
        let name = machine.name.trim();
        if name.is_empty() || !names.insert(name.to_lowercase()) {
            return Err(Error::BadRequest(format!(
                "Name each machine once: \"{name}\" is empty or used twice"
            )));
        }
        if workers.iter().any(|w| w.name.eq_ignore_ascii_case(name)) {
            return Err(Error::BadRequest(format!(
                "{name} is a GCP worker already; name the machine otherwise"
            )));
        }
        if !socket_ok(machine.socket.trim()) {
            return Err(Error::BadRequest(format!(
                "The socket of {name} must be an absolute path or start with ~/"
            )));
        }
    }
    let controller = transfer.controller.trim();
    if !controller.is_empty() && !socket_ok(controller) {
        return Err(Error::BadRequest(
            "The controller's socket must be an absolute path or start with ~/".into(),
        ));
    }
    d.settings
        .update(|doc| {
            let mut table = toml_edit::Table::new();
            if !controller.is_empty() {
                table["controller"] = toml_edit::value(controller);
            }
            let mut machines = toml_edit::ArrayOfTables::new();
            for machine in &transfer.machines {
                let mut row = toml_edit::Table::new();
                row["name"] = toml_edit::value(machine.name.trim());
                row["socket"] = toml_edit::value(machine.socket.trim());
                machines.push(row);
            }
            if !machines.is_empty() {
                table.insert("machines", toml_edit::Item::ArrayOfTables(machines));
            }
            doc["transfer"] = toml_edit::Item::Table(table);
        })
        .map_err(refused)
}

fn set_workers(d: &Daemon, workers: chauffeur_protocol::client::WorkerConfiguration) -> Result<()> {
    let project = workers.project.trim();
    let zone = workers.zone.trim();
    for (name, value) in [("Project", project), ("Zone", zone)] {
        if value.len() > 200
            || value.starts_with('-')
            || value.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(Error::BadRequest(format!(
                "{name} contains an invalid identifier; enter its Google Cloud ID"
            )));
        }
    }
    if zone.is_empty() {
        return Err(Error::BadRequest(
            "Choose a Google Cloud zone before saving".into(),
        ));
    }
    d.settings
        .update(|doc| {
            doc["workers"]["project"] = toml_edit::value(project);
            doc["workers"]["zone"] = toml_edit::value(zone);
        })
        .map_err(refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_prefixes_git_takes() {
        for prefix in ["", "jc", "alice.martin", "team/jc", "jc_2"] {
            assert!(check_prefix(prefix).is_ok(), "{prefix}");
        }
        for prefix in ["j c", "/jc", "jc/", "a..b", "a//b", "-jc", "jc."] {
            assert!(check_prefix(prefix).is_err(), "{prefix}");
        }
    }

    #[test]
    fn the_template_parses_to_the_defaults() {
        let settings: crate::settings::UserSettings = toml::from_str(TEMPLATE).unwrap();
        assert!(settings.github_org.is_empty());
        assert!(settings.default_repos.is_empty());
        assert!(settings.clone_roots.is_empty());
        assert!(settings.workers.project.is_empty());
        assert_eq!(settings.max_children, 5);
        assert!(settings.prompt_suggestions);
    }

    #[test]
    fn lists_are_trimmed_without_blanks_or_repeats() {
        assert_eq!(
            cleaned(vec![" a ".into(), "".into(), "b".into(), "a".into()]),
            ["a", "b"]
        );
    }
}
