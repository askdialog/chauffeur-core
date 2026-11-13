//! Human-readable output. `--json` bypasses all of this.
//!
//! Agent text, ticket titles, branch names and the like come from outside and reach the user's
//! terminal: every public function here returns text passed through [`sanitize_terminal`], so an
//! escape sequence in them cannot retitle the terminal, clear it or write its clipboard.

use std::borrow::Cow;
use std::collections::HashSet;

use chauffeur_protocol::client::{
    ContextUsageResult, EnvReloadResult, GithubConnecting, GithubStatus, JevSettings,
    OllamaInstalling, OllamaPull, OllamaRunning, OllamaStatus, OpenRequest, ProcessInfo,
    SubagentMessagesResult,
};
use chauffeur_protocol::events::{
    ChatEvent, CompactionBy, ContextCategoryKind, Event, EventEnvelope, RegistryEvent, TaskKind,
    WorkspaceEvent,
};
use chauffeur_protocol::model::{
    Activity, AgentModels, ChatInfo, ChatStatus, KeepAwake, PowerHold, PowerState, RepoInfo,
    SectionInfo, SlashCommand, UpdatePhase, UpdateStatus, WorkspaceInfo, WorkspaceState,
    WorktreeOrigin,
};

/// Text safe to print to a terminal: without the C0 controls other than `\n` and `\t` (so no
/// ESC, BEL or `\r`), DEL, and the C1 controls U+0080–U+009F. Letters, emoji and layout stay.
pub fn sanitize_terminal(text: &str) -> Cow<'_, str> {
    fn dangerous(c: char) -> bool {
        (c.is_control() && c != '\n' && c != '\t') || ('\u{80}'..='\u{9f}').contains(&c)
    }
    if text.contains(dangerous) {
        Cow::Owned(text.chars().filter(|c| !dangerous(*c)).collect())
    } else {
        Cow::Borrowed(text)
    }
}

fn sanitized(text: String) -> String {
    match sanitize_terminal(&text) {
        Cow::Borrowed(_) => text,
        Cow::Owned(clean) => clean,
    }
}

/// `Jev compaction: on, from 60% of the context, key saved`.
pub fn jev(settings: &JevSettings) -> String {
    format!(
        "Jev compaction of Claude and Codex chats: {}, from {}% of the context, {}",
        if settings.enabled { "on" } else { "off" },
        settings.threshold_percent,
        if settings.key_saved {
            "TypeSafe key saved"
        } else {
            "no TypeSafe key (chauffeur jev key)"
        }
    )
}

/// `chauffeur subagent`: the subagent's conversation as `chauffeur attach` prints a chat's, its
/// report in full. `chat_hint` names the chat, for the commands it suggests.
pub fn subagent_messages(result: &SubagentMessagesResult, chat_hint: &str) -> String {
    if !result.found {
        return "This subagent's conversation is not available.".into();
    }
    let mut out = format!(
        "subagent {}",
        result.agent_id.as_deref().unwrap_or("(unknown id)")
    );
    let mut renderer = EventRenderer::default();
    for event in &result.events {
        if let Some(line) = renderer.chat_event(event, chat_hint) {
            out.push('\n');
            out.push_str(&line);
        }
    }
    out
}

/// `chauffeur context`: `15% used · 154.2k / 1M tokens (claude-opus-5-5)`, then a row per
/// category and the compaction threshold.
pub fn context_usage(result: &ContextUsageResult) -> String {
    let Some(usage) = &result.usage else {
        return "No reading yet: the chat's context is read after each turn".to_string();
    };
    let mut out = format!(
        "{}{}% used · {} / {} tokens{}{}{}",
        if usage.estimated { "≈" } else { "" },
        usage.percent.round(),
        tokens(usage.used_tokens),
        tokens(usage.max_tokens),
        usage
            .model
            .as_ref()
            .map(|m| format!(" ({m})"))
            .unwrap_or_default(),
        if usage.over_limit {
            ", over the window"
        } else {
            ""
        },
        if usage.estimated {
            ", estimated after a compaction until the agent's next response"
        } else {
            ""
        }
    );
    for category in usage.categories.iter().filter(|c| c.tokens > 0) {
        let share = match category.kind {
            ContextCategoryKind::Deferred => "—".to_string(),
            _ if usage.max_tokens > 0 => format!(
                "{:.1}%",
                category.tokens as f64 * 100.0 / usage.max_tokens as f64
            ),
            _ => String::new(),
        };
        out.push_str(&format!(
            "\n  {:<28} {:>8} {:>6}",
            category.name,
            tokens(category.tokens),
            share
        ));
    }
    if let Some(compaction) = &usage.compaction {
        out.push_str(&format!(
            "\nCompacts at {:.0}% ({})",
            compaction.threshold_percent,
            match compaction.by {
                CompactionBy::Jev => "Jev",
                CompactionBy::ClaudeCode => "Claude Code's autocompact",
            }
        ));
    }
    if !result.live {
        out.push_str("\nLast reading");
        if let Some(at) = &result.read_at {
            out.push_str(&format!(", at {at}"));
        }
        out.push_str(": the chat has no running session");
    }
    sanitized(out)
}

/// `154.2k`, `1M`, `812`.
fn tokens(n: u64) -> String {
    if n >= 1_000_000 {
        let m = n as f64 / 1_000_000.0;
        if (m - m.round()).abs() < 0.05 {
            format!("{}M", m.round())
        } else {
            format!("{m:.1}M")
        }
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// `chauffeur power`: whether the machine is kept awake, why, and anything wrong.
pub fn update(status: &UpdateStatus) -> String {
    match (status.phase, &status.available) {
        (UpdatePhase::Installed, Some(version)) => format!(
            "Chauffeur {version} is installed: quit and reopen the app to run it (this daemon is {})",
            status.current
        ),
        (UpdatePhase::Failed, _) => format!(
            "could not update Chauffeur: {}",
            status.error.as_deref().unwrap_or("unknown error")
        ),
        (_, Some(version)) => format!(
            "Chauffeur {version} is available (this is {}): `chauffeur update --install`",
            status.current
        ),
        (_, None) => format!("Chauffeur {} is up to date", status.current),
    }
}

pub fn github(status: &GithubStatus) -> String {
    let git = if status.git {
        "git is installed".to_string()
    } else if status.git_install == "xcode" {
        "git is missing: install Apple's command line developer tools with `xcode-select --install`"
            .into()
    } else {
        format!("git is missing: install it with {}", status.git_install)
    };
    let account = match (&status.account, &status.gh) {
        (Some(login), Some(gh)) => {
            let whose = if status.own {
                "Chauffeur's own GitHub CLI"
            } else {
                gh
            };
            format!("GitHub: connected as {login}, through {whose}")
        }
        (None, _) => "GitHub: not connected, `chauffeur github login` connects it".into(),
        (Some(login), None) => format!("GitHub: connected as {login}"),
    };
    let mut out = format!("{git}\n{account}");
    if !status.organizations.is_empty() {
        out.push_str(&format!(
            "\n  organizations: {}",
            status.organizations.join(", ")
        ));
    }
    if let Some(step) = &status.connecting {
        out.push_str(&format!("\n  {}", github_connecting(step)));
    }
    sanitize_terminal(&out).into_owned()
}

/// A connection under way: installing GitHub CLI by tenths, then the code to enter on GitHub.
pub fn github_connecting(step: &GithubConnecting) -> String {
    if let Some(error) = &step.error {
        return format!("connecting GitHub failed: {error}");
    }
    match (step.phase.as_str(), &step.code) {
        ("downloading", _) if step.total > 0 => format!(
            "installing GitHub CLI: downloading {}% of {} MB",
            step.completed * 10 / step.total * 10,
            step.total / 1_000_000
        ),
        ("downloading" | "checking" | "unpacking", _) => {
            format!("installing GitHub CLI: {}", step.phase)
        }
        ("waiting", Some(code)) => format!(
            "enter the code {code} on {} (opened in your browser), then authorize",
            step.url
                .as_deref()
                .unwrap_or("https://github.com/login/device")
        ),
        ("configuring", _) => "letting git push through GitHub CLI".into(),
        _ => "connecting GitHub".into(),
    }
}

pub fn ollama(status: &OllamaStatus) -> String {
    let mut out = match (&status.executable, status.running) {
        (None, OllamaRunning::Stopped) if status.download_size.is_some() => {
            "Ollama is not installed: `chauffeur ollama install` installs it to run local models"
                .into()
        }
        (None, OllamaRunning::Stopped) => format!(
            "Ollama is not installed: install it with `{}` to run local models",
            status.install
        ),
        (_, OllamaRunning::Daemon) => "Ollama runs, started by Chauffeur".into(),
        (_, OllamaRunning::External) => "Ollama runs, started outside Chauffeur".into(),
        (Some(path), OllamaRunning::Stopped) => {
            format!("Ollama is installed ({path}), not running: Chauffeur starts it when needed")
        }
    };
    if status.own {
        out.push_str(&format!(" (Chauffeur's own, {})", status.version));
    }
    if let Some(step) = &status.installing {
        out.push_str(&format!("\n  {}", ollama_installing(step)));
    }
    if status.models.is_empty() {
        out.push_str("\nNo local model: `chauffeur ollama pull qwen3.8` downloads one (18 GB)");
    }
    for model in &status.models {
        out.push_str(&format!(
            "\n  {:<32} {:>8.1} GB",
            model.name,
            model.size as f64 / 1e9
        ));
    }
    for pull in &status.pulls {
        out.push_str(&format!("\n  {}", ollama_pull(pull)));
    }
    out
}

/// Installing Ollama: its phase, and the download by tenths.
pub fn ollama_installing(step: &OllamaInstalling) -> String {
    if let Some(error) = &step.error {
        return format!("installing Ollama failed: {error}");
    }
    if step.phase != "downloading" || step.total == 0 {
        return format!("installing Ollama: {}", step.phase);
    }
    let tenths = step.completed * 10 / step.total;
    let size = if step.total < 1_000_000_000 {
        format!("{} MB", step.total / 1_000_000)
    } else {
        format!("{:.1} GB", step.total as f64 / 1e9)
    };
    format!("installing Ollama: downloading {}% of {size}", tenths * 10)
}

/// A download's progress, by tenths so that a poll does not print a line per percent.
pub fn ollama_pull(pull: &OllamaPull) -> String {
    if let Some(error) = &pull.error {
        return format!("{}: failed, {error}", pull.model);
    }
    if pull.total == 0 {
        return format!("{}: {}", pull.model, pull.status);
    }
    let tenths = pull.completed * 10 / pull.total;
    format!("{}: {} {}%", pull.model, pull.status, tenths * 10)
}

pub fn power(state: &PowerState) -> String {
    let mode = match state.keep_awake {
        KeepAwake::Never => "never",
        KeepAwake::WhileWorking => "while agents work",
        KeepAwake::LidClosed => "while agents work, even with the lid closed",
    };
    let mut why = Vec::new();
    if state.working_chats > 0 {
        why.push(format!(
            "{} agent{} working",
            state.working_chats,
            if state.working_chats == 1 { "" } else { "s" }
        ));
    }
    if state.running_tasks > 0 {
        why.push(format!("{} background task(s)", state.running_tasks));
    }
    if state.creating_workspaces > 0 {
        why.push(format!(
            "{} workspace(s) being created",
            state.creating_workspaces
        ));
    }
    if let Some(secs) = state.lingering_secs {
        why.push(format!("work ended, kept {secs} s more"));
    }
    let now = match state.holding {
        PowerHold::None => "the machine may sleep".to_string(),
        PowerHold::IdleSleep => "no idle sleep (closing the lid still sleeps a laptop)".into(),
        PowerHold::AllSleep => "sleep turned off, the lid closed included".into(),
    };
    let mut lines = vec![format!("Keep awake: {mode}"), format!("Now: {now}")];
    if !why.is_empty() {
        lines.push(format!("Why: {}", why.join(", ")));
    }
    if state.low_battery {
        lines.push(format!(
            "Battery low ({}%): sleep given back until it charges",
            state.battery_percent.map_or("?".into(), |p| p.to_string())
        ));
    }
    if let Some(warning) = &state.warning {
        lines.push(format!("Warning: {warning}"));
    }
    lines.join("\n")
}

/// What `chauffeur env reload` changed: variables by name only.
pub fn env_reload(result: &EnvReloadResult) -> String {
    let names = |what: &str, vars: &[String]| {
        (!vars.is_empty()).then(|| format!("{what}: {}", vars.join(", ")))
    };
    let mut lines = vec![format!("Read the environment of {} again.", result.shell)];
    let changes: Vec<String> = [
        names("Added", &result.added),
        names("Changed", &result.changed),
        names("Removed", &result.removed),
    ]
    .into_iter()
    .flatten()
    .collect();
    if changes.is_empty() {
        lines.push("Nothing changed.".into());
    }
    lines.extend(changes);
    if !result.restarted.is_empty() {
        lines.push(format!(
            "Restarted {} idle chat(s) whose MCP servers failed: {}",
            result.restarted.len(),
            result.restarted.join(", ")
        ));
    }
    if !result.deferred.is_empty() {
        lines.push(format!(
            "Busy chat(s) that get it at their next message once idle: {}",
            result.deferred.join(", ")
        ));
    }
    for name in &result.still_missing {
        lines.push(format!(
            "{name} is still not set: add `export {name}=…` to {} and reload again.",
            result.profile
        ));
    }
    sanitize_terminal(&lines.join("\n")).into_owned()
}

pub fn workspace_line(w: &WorkspaceInfo) -> String {
    let repos: Vec<&str> = w
        .worktrees
        .iter()
        .filter(|t| !t.removed && t.origin == WorktreeOrigin::Workspace)
        .map(|t| t.repo.as_str())
        .collect();
    let tickets: Vec<&str> = w.tickets.iter().map(|t| t.identifier.as_str()).collect();
    let mut line = format!(
        "{:<36} {:<14} {:<11} {}",
        w.slug,
        state_label(w),
        activity_label(w.activity),
        repos.join(", ")
    );
    if !tickets.is_empty() {
        line.push_str(&format!("  [{}]", tickets.join(", ")));
    }
    if let Some(title) = &w.title {
        line.push_str(&format!("  “{title}”"));
    }
    if w.unread {
        line.push_str("  • unread");
    }
    if w.explore {
        line.push_str("  (explore)");
    }
    if w.parent_id.is_some() {
        line = format!("  ↳ {line}");
    }
    sanitized(line)
}

/// A sidebar section: its name, folded or not, and its id.
pub fn section_line(section: &SectionInfo) -> String {
    let folded = if section.folded { "  (folded)" } else { "" };
    sanitized(format!("{:<30} {}{folded}", section.name, section.id))
}

fn state_label(w: &WorkspaceInfo) -> &'static str {
    match w.state {
        WorkspaceState::Creating => "creating",
        WorkspaceState::Ready => "ready",
        WorkspaceState::Archiving => "archiving",
        WorkspaceState::Archived => "archived",
        WorkspaceState::CleanupFailed => "CLEANUP FAILED",
    }
}

fn activity_label(activity: Activity) -> &'static str {
    match activity {
        Activity::Idle => "idle",
        Activity::Working => "working",
        Activity::NeedsInput => "NEEDS INPUT",
        Activity::Error => "error",
    }
}

/// A workspace's tickets, one per line; one Linear never described says so.
pub fn tickets(w: &WorkspaceInfo) -> String {
    if w.tickets.is_empty() {
        return sanitized(format!("{} has no ticket", w.slug));
    }
    let lines: Vec<String> = w
        .tickets
        .iter()
        .map(|t| match &t.title {
            Some(title) => format!("{:10} {title}", t.identifier),
            None => format!("{:10} (not read from Linear)", t.identifier),
        })
        .collect();
    sanitized(lines.join("\n"))
}

pub fn workspace_details(w: &WorkspaceInfo) -> String {
    let mut out = format!("{}  ({})\n  path     {}\n", w.slug, w.id, w.path);
    if w.explore {
        out.push_str("  explore  read-only, each repo at its base branch\n");
    } else {
        out.push_str(&format!("  branch   {}\n", w.branch));
    }
    out.push_str(&format!(
        "  state    {}\n  activity {}\n",
        state_label(w),
        activity_label(w.activity)
    ));
    if w.port_count > 0 {
        out.push_str(&format!(
            "  ports    {}–{}\n",
            w.port_base,
            w.port_base + w.port_count - 1
        ));
    }
    if !w.tickets.is_empty() {
        let tickets: Vec<String> = w
            .tickets
            .iter()
            .map(|t| match &t.title {
                Some(title) => format!("{} {title}", t.identifier),
                None => t.identifier.clone(),
            })
            .collect();
        out.push_str(&format!("  tickets  {}\n", tickets.join("; ")));
    }
    if let Some(error) = &w.last_error {
        out.push_str(&format!("  error    {error}\n"));
    }
    out.push_str("  worktrees\n");
    for t in &w.worktrees {
        if t.removed {
            continue;
        }
        let kind = match (&t.origin, &t.group) {
            (WorktreeOrigin::Agent, Some(group)) => format!(" (agent group {group})"),
            (WorktreeOrigin::Agent, None) => " (agent)".to_string(),
            _ => String::new(),
        };
        let at = if t.branch.is_empty() {
            format!("at origin/{}", t.base_branch)
        } else {
            format!("{} ← {}", t.branch, t.base_branch)
        };
        out.push_str(&format!("    {:<22} {}  [{at}]{kind}\n", t.repo, t.path));
    }
    sanitized(out)
}

/// A design chat's canvas: each screen on a line, by page and section, with its file under the
/// design folder.
pub fn canvas(c: &chauffeur_protocol::client::CanvasResult) -> String {
    use chauffeur_protocol::client::CanvasScreen;
    let screen = |s: &CanvasScreen| {
        format!(
            "    {}{}  {}×{}{}",
            s.file,
            s.label
                .as_deref()
                .map(|l| format!("  {l}"))
                .unwrap_or_default(),
            s.width,
            s.height,
            if s.modified_ms.is_none() {
                "  [missing]"
            } else {
                ""
            }
        )
    };
    let mut lines = vec![format!("folder {}", c.dir)];
    if let Some(error) = &c.error {
        lines.push(format!("canvas.json is invalid: {error}"));
    }
    for page in &c.pages {
        lines.push(page.name.clone());
        for section in &page.sections {
            lines.push(format!("  {}", section.title));
            lines.extend(section.screens.iter().map(screen));
        }
    }
    if !c.unsorted.is_empty() {
        lines.push("Unsorted".into());
        lines.extend(c.unsorted.iter().map(screen));
    }
    if c.pages.is_empty() && c.unsorted.is_empty() {
        lines.push("no screens yet".into());
    }
    sanitized(lines.join("\n"))
}

pub fn chat_line(c: &ChatInfo) -> String {
    sanitized(format!(
        "{}  {:<6} {:<11} {}{}{}{}",
        c.id,
        c.agent.as_str(),
        chat_status_label(c.status),
        c.title.clone().unwrap_or_default(),
        match &c.parent_chat {
            Some(parent) => format!("  (delegated by {parent})"),
            None => String::new(),
        },
        match c.running_tasks {
            0 => String::new(),
            1 => "  (1 task running)".to_string(),
            n => format!("  ({n} tasks running)"),
        },
        match (&c.closed, &c.last_activity_at) {
            (false, _) => String::new(),
            (true, Some(at)) => format!("  [closed, last active {}]", short_time(at)),
            (true, None) => "  [closed]".to_string(),
        }
    ))
}

/// The daemon's `2026-10-18T09:41:07.123Z` as `2026-10-18 09:41 UTC`.
fn short_time(rfc3339: &str) -> String {
    match rfc3339.get(..16) {
        Some(minutes) if rfc3339.ends_with('Z') => format!("{} UTC", minutes.replace('T', " ")),
        _ => rfc3339.to_string(),
    }
}

fn chat_status_label(status: ChatStatus) -> &'static str {
    match status {
        ChatStatus::Starting => "starting",
        ChatStatus::Idle => "idle",
        ChatStatus::Working => "working",
        ChatStatus::NeedsInput => "NEEDS INPUT",
        ChatStatus::Error => "error",
        ChatStatus::Closed => "closed",
    }
}

pub fn repo_line(r: &RepoInfo) -> String {
    sanitized(format!(
        "{} {:<36} {:<10} {}",
        if r.is_default { "*" } else { " " },
        r.key,
        r.default_branch.clone().unwrap_or_default(),
        // Cloned on first use otherwise.
        r.clone_path.clone().unwrap_or_default()
    ))
}

pub fn process_line(p: &ProcessInfo) -> String {
    let state = if p.running {
        "running".to_string()
    } else {
        format!(
            "exited {}",
            p.exit_code.map(|c| c.to_string()).unwrap_or("?".into())
        )
    };
    sanitized(format!("{}  {:<28} {}", p.id, p.name, state))
}

/// The tool a subagent hands its report back with (`{"message": "<report>"}`), in Claude Code
/// versions where its plain text no longer reaches the main agent.
const HANDBACK: &str = "SubagentHandback";

/// A chat's events, in order, as terminal lines. It remembers what an earlier event makes
/// redundant: the result of a `SubagentHandback` call (`Report delivered to your caller.`) only
/// acknowledges the report printed just above it.
#[derive(Default)]
pub struct EventRenderer {
    handbacks: HashSet<String>,
}

impl EventRenderer {
    /// One event as terminal lines, or `None` when it is not worth printing.
    pub fn event(&mut self, envelope: &EventEnvelope, chat_hint: &str) -> Option<String> {
        if let Event::Chat { event, .. } = &envelope.event
            && self.acknowledges_report(event)
        {
            return None;
        }
        raw_event(envelope, chat_hint).map(sanitized)
    }

    fn chat_event(&mut self, event: &ChatEvent, chat_hint: &str) -> Option<String> {
        if self.acknowledges_report(event) {
            return None;
        }
        chat_event(event, chat_hint).map(sanitized)
    }

    fn acknowledges_report(&mut self, event: &ChatEvent) -> bool {
        match event {
            ChatEvent::ToolCall {
                tool_use_id,
                name,
                input,
                ..
            } if name == HANDBACK && handback_report(input).is_some() => {
                self.handbacks.insert(tool_use_id.clone());
                false
            }
            ChatEvent::ToolResult {
                tool_use_id,
                is_error: false,
                ..
            } => self.handbacks.remove(tool_use_id),
            _ => false,
        }
    }
}

/// The report of a `SubagentHandback` call: its `message`, or `None` when the input has none.
fn handback_report(input: &serde_json::Value) -> Option<&str> {
    input.get("message").and_then(|m| m.as_str())
}

/// A subagent's report, handed back: in full in its own conversation, as its text is; in the
/// main chat, where a subagent's text is cut, its first lines.
fn report(message: &str, parent_tool_use_id: Option<&str>) -> String {
    let message = message.trim();
    let message = if message.is_empty() {
        "(empty report)"
    } else {
        message
    };
    match parent_tool_use_id {
        None => format!("\n── Report\n{message}"),
        Some(_) => format!("  (subagent report) {}", first_lines(message, 3)),
    }
}

/// The agent id in an Agent call's result that only says where the report went:
/// `This agent's report was delivered to you as a message from "a1b2c3" (its SubagentHandback
/// call). Read it there; it is not repeated here.`
fn handed_back_to(output: &str) -> Option<&str> {
    let (_, rest) = output.split_once("report was delivered to you as a message from \"")?;
    let id = rest.split('"').next()?;
    (!id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    .then_some(id)
}

fn raw_event(envelope: &EventEnvelope, chat_hint: &str) -> Option<String> {
    match &envelope.event {
        Event::Chat { event, .. } => chat_event(event, chat_hint),
        Event::Workspace { event, .. } => match event {
            WorkspaceEvent::Progress { message } => Some(format!("· {message}")),
            WorkspaceEvent::Warning { message } => Some(format!("! {message}")),
            WorkspaceEvent::StateChanged { state, error } => Some(format!(
                "· workspace {}{}",
                state.as_str(),
                error.as_ref().map(|e| format!(": {e}")).unwrap_or_default()
            )),
            WorkspaceEvent::ProcessStarted { name, command, .. } => {
                Some(format!("▶ {name}: {command}"))
            }
            WorkspaceEvent::ProcessExited { exit_code, .. } => Some(format!(
                "■ process exited {}",
                exit_code.map(|c| c.to_string()).unwrap_or("?".into())
            )),
            WorkspaceEvent::ProcessOutput { line, .. } => Some(format!("  │ {line}")),
            WorkspaceEvent::WorktreeAdded { worktree } => {
                Some(format!("· worktree {} at {}", worktree.repo, worktree.path))
            }
            WorkspaceEvent::Transfer { transfer } => transfer.log.last().map(|l| format!("· {l}")),
            WorkspaceEvent::Relay {
                machine,
                connected: true,
                ..
            } => Some(format!("· {machine} answers again")),
            WorkspaceEvent::Relay { machine, error, .. } => Some(format!(
                "· {}: its events are back when it answers",
                error
                    .clone()
                    .unwrap_or_else(|| format!("{machine} does not answer"))
            )),
            WorkspaceEvent::WorktreeRemoved { .. }
            | WorkspaceEvent::Replayed
            | WorkspaceEvent::Share { .. }
            | WorkspaceEvent::FileShare { .. }
            | WorkspaceEvent::TerminalOutput { .. }
            | WorkspaceEvent::TerminalExited { .. }
            | WorkspaceEvent::TerminalActivity { .. } => None,
        },
        Event::Registry { event } => match event {
            RegistryEvent::WorkspaceUpserted { .. }
            | RegistryEvent::ChatUpserted { .. }
            | RegistryEvent::LoadoutChanged { .. }
            | RegistryEvent::ExtensionsChanged { .. }
            | RegistryEvent::SectionsChanged { .. }
            | RegistryEvent::AccountChanged { .. }
            | RegistryEvent::IdentityChanged { .. }
            | RegistryEvent::MentionsChanged { .. }
            | RegistryEvent::LoginEnded { .. }
            | RegistryEvent::IdleReminder { .. }
            | RegistryEvent::TransferEnded { .. }
            | RegistryEvent::WorkspaceRemoved { .. }
            | RegistryEvent::ChatRemoved { .. }
            | RegistryEvent::RoutineUpserted { .. }
            | RegistryEvent::RoutineRemoved { .. }
            | RegistryEvent::RoutineSuggestionsChanged { .. } => None,
        },
        Event::Office { .. } | Event::Extension { .. } => None,
    }
}

/// A request a chat waits on, with the `chauffeur respond` line that answers it.
pub fn open_request(request: &OpenRequest) -> String {
    chat_event(&request.request, &request.chat_id)
        .map(sanitized)
        .unwrap_or_default()
}

fn chat_event(event: &ChatEvent, chat_hint: &str) -> Option<String> {
    Some(match event {
        ChatEvent::AgentChanged { from, to } => format!(
            "Switched from {} to {}: the conversation will be handed over as text with your next message.",
            from.as_str(),
            to.as_str()
        ),
        ChatEvent::UserMessage {
            text,
            queued,
            attachments,
            ..
        } => {
            let images: String = attachments
                .iter()
                .flatten()
                .map(|a| format!("\n  [image] {}", a.name))
                .collect();
            format!(
                "\n› {}{}{}",
                text,
                if *queued { "  (queued)" } else { "" },
                images
            )
        }
        ChatEvent::AssistantText {
            text,
            parent_tool_use_id,
            ..
        } => {
            if parent_tool_use_id.is_some() {
                format!("  (subagent) {}", first_lines(text, 3))
            } else {
                format!("\n{text}")
            }
        }
        ChatEvent::Thinking { text, .. } => format!("  (thinking) {}", first_lines(text, 2)),
        ChatEvent::ToolCall {
            name,
            input,
            parent_tool_use_id,
            ..
        } => match handback_report(input) {
            Some(message) if name == HANDBACK => report(message, parent_tool_use_id.as_deref()),
            _ => format!("⏺ {name}({})", summarize_input(input)),
        },
        ChatEvent::ToolResult {
            output,
            is_error: false,
            ..
        } if handed_back_to(output).is_some() => {
            let id = handed_back_to(output).unwrap_or_default();
            if chat_hint.is_empty() {
                format!("  ⎿ report handed back by subagent {id}")
            } else {
                format!(
                    "  ⎿ report handed back by subagent {id}; in full: chauffeur subagent {chat_hint} {id}"
                )
            }
        }
        ChatEvent::ToolResult {
            output, is_error, ..
        } => format!(
            "  ⎿ {}{}",
            if *is_error { "error: " } else { "" },
            first_lines(output, 2)
        ),
        ChatEvent::PermissionRequest {
            request_id,
            tool_name,
            input,
            title,
            agent,
        } => {
            let asker = agent.as_ref().map(|agent| {
                format!(
                    " (subagent {}{})",
                    agent.subagent_type.as_deref().unwrap_or(&agent.id),
                    if agent.background { ", background" } else { "" }
                )
            });
            format!(
                "? {}{}\n  {tool_name}({})\n  answer: chauffeur respond {chat_hint} {request_id} allow|deny",
                title
                    .clone()
                    .unwrap_or_else(|| format!("{tool_name} wants permission")),
                asker.unwrap_or_default(),
                summarize_input(input)
            )
        }
        ChatEvent::McpElicitation {
            request_id,
            server_name,
            message,
            schema,
            url,
            error,
            rule,
        } => {
            let details = if let Some(error) = error {
                format!("\n  {error}")
            } else if let Some(url) = url {
                format!("\n  Open: {url}")
            } else {
                format!(
                    "\n  Schema: {}",
                    schema.as_ref().unwrap_or(&serde_json::Value::Null)
                )
            };
            let content = if schema.is_some() {
                " --content '{}'"
            } else {
                ""
            };
            let always = if rule.is_some() { " [--always]" } else { "" };
            format!(
                "? {server_name}: {message}{details}\n  answer: chauffeur respond {chat_hint} {request_id} elicitation accept|decline|cancel{content}{always}"
            )
        }
        ChatEvent::Question {
            request_id,
            questions,
        } => {
            let mut out = String::from("? the agent asks:");
            for q in questions {
                out.push_str(&format!("\n  {}", q.question));
                for option in &q.options {
                    out.push_str(&format!("\n    - {}: {}", option.label, option.description));
                }
            }
            out.push_str(&format!(
                "\n  answer: chauffeur respond {chat_hint} {request_id} answer '<question>=<answer>'"
            ));
            out
        }
        ChatEvent::RequestResolved { outcome, rules, .. } => match rules {
            Some(rules) if !rules.is_empty() => {
                format!("  ({outcome}, always: {})", rules.join(", "))
            }
            _ => format!("  ({outcome})"),
        },
        ChatEvent::TurnEnded {
            outcome,
            duration_ms,
            cost_usd,
            error,
            ..
        } => format!(
            "— turn {:?}{}{}{}",
            outcome,
            duration_ms
                .map(|ms| format!(", {:.1} s", ms as f64 / 1000.0))
                .unwrap_or_default(),
            cost_usd.map(|c| format!(", ${c:.3}")).unwrap_or_default(),
            error.as_ref().map(|e| format!(": {e}")).unwrap_or_default()
        )
        .to_lowercase(),
        ChatEvent::Notice {
            level,
            message,
            badge,
        } => match badge {
            Some(badge) => format!("! {level:?}: {message} [{badge}]"),
            None => format!("! {level:?}: {message}"),
        },
        ChatEvent::SessionStarted { model, .. } => format!(
            "· agent session started{}",
            model
                .as_ref()
                .map(|m| format!(" ({m})"))
                .unwrap_or_default()
        ),
        ChatEvent::TaskStarted {
            task_id,
            kind,
            description,
            subagent_type,
            background,
            ..
        } => format!(
            "· task started: {description}{}{} [{task_id}]\n  stop: chauffeur stop-task {chat_hint} {task_id}",
            match (kind, subagent_type) {
                (TaskKind::Subagent, Some(kind)) => format!(" (subagent {kind})"),
                (TaskKind::Subagent, None) => " (subagent)".to_string(),
                (TaskKind::Command, _) => " (command)".to_string(),
                (TaskKind::Other, _) => String::new(),
            },
            if *background {
                ", in the background"
            } else {
                ""
            }
        ),
        ChatEvent::PlanUpdated { steps } => format!(
            "· plan {}/{}",
            steps
                .iter()
                .filter(|s| s.status == chauffeur_protocol::events::PlanStepStatus::Completed)
                .count(),
            steps.len()
        ),
        ChatEvent::TaskEnded {
            task_id,
            status,
            summary,
        } => format!(
            "· task {} [{task_id}]{}",
            format!("{status:?}").to_lowercase(),
            summary
                .as_ref()
                .map(|s| format!(": {}", first_lines(s, 2)))
                .unwrap_or_default()
        ),
        ChatEvent::ShellStarted { command, .. } => format!("\n$ {command}  (running)"),
        ChatEvent::ShellEnded {
            command,
            exit_code,
            output,
            timed_out,
            ..
        } => format!(
            "\n$ {command}  ({})\n    {}",
            shell_ending(*exit_code, *timed_out),
            first_lines(output.trim_end(), 5)
        ),
        ChatEvent::TurnStarted
        | ChatEvent::StatusChanged { .. }
        | ChatEvent::TextDelta { .. }
        | ChatEvent::ThinkingDelta { .. }
        | ChatEvent::TaskProgress { .. }
        | ChatEvent::PromptSuggestion { .. }
        | ChatEvent::PermissionCheck { .. }
        | ChatEvent::DesignShare { .. }
        | ChatEvent::Suggestions { .. }
        | ChatEvent::ShareComments { .. }
        | ChatEvent::ContextUsage { .. }
        | ChatEvent::PermissionModeEffective { .. }
        | ChatEvent::McpStatus { .. }
        | ChatEvent::AccountLost { .. }
        | ChatEvent::UserMessageDelivered { .. } => return None,
        ChatEvent::QueueChanged { messages } => match messages.len() {
            0 => "· queue empty".to_string(),
            1 => "· 1 message queued for the end of the turn".to_string(),
            n => format!("· {n} messages queued for the end of the turn"),
        },
    })
}

/// How a chat's shell command ended: `exit 0`, `timed out`, `killed`.
fn shell_ending(exit_code: Option<i32>, timed_out: bool) -> String {
    match (exit_code, timed_out) {
        (_, true) => "timed out".to_string(),
        (Some(code), false) => format!("exit {code}"),
        (None, false) => "killed".to_string(),
    }
}

fn first_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = lines
        .iter()
        .take(n)
        .copied()
        .collect::<Vec<_>>()
        .join("\n    ");
    if lines.len() > n {
        out.push_str(&format!(" … (+{} lines)", lines.len() - n));
    }
    out
}

fn summarize_input(input: &serde_json::Value) -> String {
    for key in [
        "command",
        "file_path",
        "pattern",
        "url",
        "description",
        "prompt",
    ] {
        if let Some(value) = input.get(key).and_then(|v| v.as_str()) {
            let value: String = value.chars().take(120).collect();
            return value.replace('\n', " ");
        }
    }
    let text = input.to_string();
    text.chars().take(120).collect()
}

/// One block per agent: each model with its effort levels, the default marked.
pub fn commands(commands: &[SlashCommand]) -> String {
    if commands.is_empty() {
        return "no commands".into();
    }
    let width = commands
        .iter()
        .map(|c| c.name.chars().count() + 1)
        .max()
        .unwrap_or(0);
    sanitized(
        commands
            .iter()
            .map(|command| {
                let hint = if command.argument_hint.is_empty() {
                    String::new()
                } else {
                    format!("{}  ", command.argument_hint)
                };
                let description: String = command.description.chars().take(90).collect();
                format!(
                    "{:width$}  {hint}{description}",
                    format!("/{}", command.name)
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

pub fn models(agents: &[AgentModels]) -> String {
    let mut out = String::new();
    for catalog in agents {
        out.push_str(&format!("{}\n", catalog.agent.as_str()));
        if let Some(error) = &catalog.error {
            out.push_str(&format!("  could not list its models: {error}\n"));
        }
        let width = catalog.models.iter().map(|m| m.id.len()).max().unwrap_or(0);
        for model in &catalog.models {
            let mut traits = Vec::new();
            if !model.efforts.is_empty() {
                traits.push(format!("effort {}", model.efforts.join("|")));
            }
            if model.supports_fast {
                traits.push("fast".into());
            }
            if model.is_default {
                traits.push("default".into());
            }
            out.push_str(&format!(
                "  {:width$}  {:16}  {}\n",
                model.id,
                model.name,
                traits.join(" · ")
            ));
        }
    }
    sanitized(out.trim_end().to_string())
}

/// A design chat's comments: by screen, a thread and its replies indented, with its state.
/// The user's mentions, unread first (each kind newest first), with the link to each thread.
pub fn mentions(m: &chauffeur_protocol::client::MentionsResult) -> String {
    use chauffeur_protocol::client::ShareConnection;
    let mut lines = Vec::new();
    match &m.connection {
        ShareConnection::Off => lines.push(
            "Mentions need you signed in to Chauffeur and a share service (design_share.url)"
                .to_string(),
        ),
        ShareConnection::Failed { message } => {
            lines.push(format!("The share service is not reached: {message}"));
        }
        ShareConnection::Connecting => lines.push("Reaching the share service…".to_string()),
        ShareConnection::Online => {}
    }
    if m.mentions.is_empty() {
        if matches!(m.connection, ShareConnection::Online) {
            lines.push("no mentions".into());
        }
        return lines.join("\n");
    }
    let (unread, read): (Vec<_>, Vec<_>) = m.mentions.iter().partition(|x| !x.seen);
    for x in unread.iter().chain(read.iter()) {
        let screen = x
            .screen
            .as_deref()
            .map(|s| format!(" ({s})"))
            .unwrap_or_default();
        lines.push(format!(
            "{} {} on \"{}\"{screen}: {}\n  {}",
            if x.seen { " " } else { "●" },
            x.author.name,
            x.share_title,
            x.text.replace('\n', " "),
            x.url
        ));
    }
    lines.join("\n")
}

pub fn comments(c: &chauffeur_protocol::client::CommentsResult) -> String {
    comment_threads(&c.comments, c.error.as_deref(), "comments.json")
}

/// A markdown file's comments, then the chat they are sent to by default.
pub fn file_comments(c: &chauffeur_protocol::client::FileCommentsResult) -> String {
    let threads = comment_threads(&c.comments, c.error.as_deref(), ".chauffeur/comments.json");
    match &c.writer {
        Some(chat) => {
            format!("{threads}\nsent to chat {chat} by default (its agent last wrote the file)")
        }
        None => threads,
    }
}

fn comment_threads(
    comments: &[chauffeur_protocol::client::DesignComment],
    error: Option<&str>,
    file: &str,
) -> String {
    use chauffeur_protocol::client::DesignComment;
    if comments.is_empty() {
        return match error {
            Some(error) => format!("{file} is invalid: {error}"),
            None => "no comments yet".into(),
        };
    }
    let mut lines = Vec::new();
    if let Some(error) = error {
        lines.push(format!("{file} is invalid: {error}"));
    }
    let mut screens: Vec<&str> = comments.iter().map(|x| x.screen.as_str()).collect();
    screens.dedup();
    let mut seen = std::collections::HashSet::new();
    let line = |x: &DesignComment| {
        let place = match &x.pin {
            None if x.quote.is_some() => {
                let quote = x.quote.as_deref().unwrap_or_default().replace('\n', " ");
                let short: String = quote.chars().take(60).collect();
                let more = if short.len() < quote.len() { "…" } else { "" };
                format!("  on \"{short}{more}\"")
            }
            Some(pin) => match &pin.element {
                Some(element) if !element.text.is_empty() => {
                    format!(
                        "  on \"{}\" at {}%,{}%",
                        element.text,
                        pin.x.round(),
                        pin.y.round()
                    )
                }
                _ => format!("  at {}%,{}%", pin.x.round(), pin.y.round()),
            },
            None => String::new(),
        };
        let state = match (&x.resolved, &x.sent) {
            (Some(_), _) => "  [resolved]",
            (None, Some(_)) => "  [sent]",
            (None, None) => "  [new]",
        };
        format!(
            "{}  {}{place}{state}: {}",
            x.id,
            x.author.name,
            x.text.replace('\n', " ")
        )
    };
    for screen in screens {
        if !seen.insert(screen) {
            continue;
        }
        lines.push(screen.to_string());
        for thread in comments
            .iter()
            .filter(|x| x.screen == screen && x.reply_to.is_none())
        {
            lines.push(format!("  {}", line(thread)));
            for reply in comments
                .iter()
                .filter(|x| x.reply_to.as_deref() == Some(&thread.id))
            {
                lines.push(format!("      {}", line(reply)));
            }
        }
    }
    sanitized(lines.join("\n"))
}

/// A design chat's share: the link, the reviewers, who looks, the connection.
pub fn share(s: &chauffeur_protocol::client::DesignShareState) -> String {
    use chauffeur_protocol::client::ShareConnection;
    let Some(share) = &s.share else {
        return match (&s.configured, &s.email) {
            (false, _) => "sharing is off (design_share.url)".into(),
            (true, None) => {
                "not shared; sign in with Google (the Office's sign-in) to share".into()
            }
            (true, Some(_)) => "not shared".into(),
        };
    };
    let connection = match &s.connection {
        ShareConnection::Off => "off",
        ShareConnection::Connecting => "connecting",
        ShareConnection::Online => "online",
        ShareConnection::Failed { .. } => "failed",
    };
    let mut lines = vec![
        format!(
            "{}  {}",
            share.url,
            if share.stopped { "stopped" } else { connection }
        ),
        format!(
            "reviewers  {}",
            if share.reviewers.is_empty() {
                "none".into()
            } else {
                share.reviewers.join(", ")
            }
        ),
    ];
    if share.team {
        lines.push("team       open to every member".into());
    }
    if s.linked {
        lines.push("linked     carries the files it links to".into());
    }
    if !share.viewers.is_empty() {
        lines.push(format!("viewing    {}", share.viewers.join(", ")));
    }
    if let ShareConnection::Failed { message } = &s.connection {
        lines.push(format!("error      {message}"));
    }
    sanitized(lines.join("\n"))
}

/// A workspace's share: the link, the guests and what each may do, who looks, the connection.
pub fn workspace_share(s: &chauffeur_protocol::client::WorkspaceShareState) -> String {
    use chauffeur_protocol::client::ShareConnection;
    use chauffeur_protocol::workspace_share::GuestRole;
    let Some(share) = &s.share else {
        return match (&s.configured, &s.email) {
            (false, _) => "sharing is off (design_share.url)".into(),
            (true, None) => {
                "not shared; sign in with Google (the Office's sign-in) to share".into()
            }
            (true, Some(_)) => "not shared".into(),
        };
    };
    let connection = match &s.connection {
        ShareConnection::Off => "off",
        ShareConnection::Connecting => "connecting",
        ShareConnection::Online => "online",
        ShareConnection::Failed { .. } => "failed",
    };
    let mut lines = vec![format!(
        "{}  {}",
        share.url,
        if share.stopped { "stopped" } else { connection }
    )];
    if share.guests.is_empty() {
        lines.push("guests     none".into());
    }
    for (i, guest) in share.guests.iter().enumerate() {
        lines.push(format!(
            "{}{}  {}",
            if i == 0 { "guests     " } else { "           " },
            guest.email,
            match guest.role {
                GuestRole::Viewer => "views",
                GuestRole::Suggest => "suggests",
            }
        ));
    }
    if share.team {
        lines.push("team       every member views".into());
    }
    lines.push(match &s.chat_ids {
        Some(ids) => format!("chats      {}", ids.join(", ")),
        None => "chats      all, those created later too".into(),
    });
    if !share.viewers.is_empty() {
        lines.push(format!("viewing    {}", share.viewers.join(", ")));
    }
    if let ShareConnection::Failed { message } = &s.connection {
        lines.push(format!("error      {message}"));
    }
    sanitized(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_context_reading_reads_as_claude_code_shows_it() {
        use chauffeur_protocol::events::{ContextCategory, ContextCompaction, ContextUsage};
        let category = |name: &str, tokens: u64, kind| ContextCategory {
            name: name.into(),
            tokens,
            kind,
        };
        let result = ContextUsageResult {
            usage: Some(ContextUsage {
                model: Some("claude-opus-5-5".into()),
                used_tokens: 154_200,
                max_tokens: 1_000_000,
                percent: 15.0,
                over_limit: false,
                categories: vec![
                    category("Messages", 120_000, ContextCategoryKind::Used),
                    category("Skills", 0, ContextCategoryKind::Used),
                    category(
                        "MCP tools (deferred)",
                        40_000,
                        ContextCategoryKind::Deferred,
                    ),
                ],
                compaction: Some(ContextCompaction {
                    by: CompactionBy::Jev,
                    threshold_percent: 60.0,
                }),
                estimated: false,
                details: None,
            }),
            read_at: Some("2026-10-18T10:31:00Z".into()),
            live: false,
        };
        let text = context_usage(&result);
        assert!(text.starts_with("15% used · 154.2k / 1M tokens (claude-opus-5-5)"));
        assert!(text.contains("Messages") && text.contains("120.0k") && text.contains("12.0%"));
        assert!(!text.contains("Skills"), "zero rows are hidden: {text}");
        assert!(text.contains("40.0k      —"), "{text}");
        assert!(text.contains("Compacts at 60% (Jev)"));
        assert!(text.contains("Last reading, at 2026-10-18T10:31:00Z"));
        let mut estimate = result.clone();
        if let Some(usage) = estimate.usage.as_mut() {
            usage.estimated = true;
        }
        let text = context_usage(&estimate);
        assert!(
            text.starts_with(
                "≈15% used · 154.2k / 1M tokens (claude-opus-5-5), estimated after a compaction"
            ),
            "{text}"
        );
        assert_eq!(
            context_usage(&ContextUsageResult {
                usage: None,
                read_at: None,
                live: false
            }),
            "No reading yet: the chat's context is read after each turn"
        );
        assert_eq!(tokens(812), "812");
        assert_eq!(tokens(1_500_000), "1.5M");
    }

    #[test]
    fn terminal_escapes_are_removed() {
        let retitle = "hi \x1b]0;pwned\x07there";
        assert_eq!(sanitize_terminal(retitle), "hi ]0;pwnedthere");
        assert_eq!(sanitize_terminal("\x1b[2Jclear"), "[2Jclear");
        assert_eq!(
            sanitize_terminal("\x1b]52;c;cGFzcw==\x1b\\clip"),
            "]52;c;cGFzcw==\\clip"
        );
        // DEL, a C1 control, and a carriage return go too; the Unicode line separator stays.
        assert_eq!(sanitize_terminal("a\x7fb\u{9b}c\rd"), "abcd");
        assert!(matches!(sanitize_terminal("\x1b"), Cow::Owned(s) if s.is_empty()));
    }

    #[test]
    fn layout_and_letters_are_kept() {
        let text = "line one\n\tindented é — 日本 🚗\n";
        assert!(matches!(sanitize_terminal(text), Cow::Borrowed(t) if t == text));
        assert_eq!(sanitized(text.to_string()), text);
    }

    #[test]
    fn rendered_lines_are_sanitized() {
        let chat = ChatInfo {
            id: "chat-1".into(),
            progress: None,
            tab_position: None,
            workspace_id: "ws-1".into(),
            agent: chauffeur_protocol::model::AgentKind::Claude,
            model: None,
            effort: None,
            fast: false,
            permission_mode: chauffeur_protocol::model::PermissionMode::Default,
            title: Some("fix \x1b[31mred\x1b[0m".into()),
            status: ChatStatus::Idle,
            agent_session_id: None,
            created_at: String::new(),
            running_tasks: 0,
            permission_mode_effective: None,
            closed: false,
            last_activity_at: None,
            mcp_servers: Vec::new(),
            kind: Default::default(),
            source_chat: None,
            brief: None,
            parent_chat: None,
            parent_tool_use: None,
            forked_from_agent: None,
        };
        assert!(!chat_line(&chat).contains('\x1b'));
        assert!(chat_line(&chat).ends_with("fix [31mred[0m"));
        let delegated = ChatInfo {
            parent_chat: Some("chat-0".into()),
            ..chat
        };
        assert!(chat_line(&delegated).ends_with("  (delegated by chat-0)"));
    }

    #[test]
    fn url_confirmations_do_not_suggest_form_content() {
        let event = ChatEvent::McpElicitation {
            request_id: "r1".into(),
            server_name: "auth".into(),
            message: "Confirm".into(),
            schema: None,
            url: Some("https://example.com/confirm".into()),
            error: None,
            rule: None,
        };
        let line = chat_event(&event, "chat-1").expect("MCP confirmations are rendered");
        assert!(line.contains("elicitation accept|decline|cancel"));
        assert!(!line.contains("--content"));
        assert!(!line.contains("--always"));
    }

    #[test]
    fn a_confirmation_offering_a_rule_suggests_always() {
        let event = ChatEvent::McpElicitation {
            request_id: "r1".into(),
            server_name: "cua_repl".into(),
            message: "Allow Computer Use to use \"Google Chrome\"?".into(),
            schema: Some(serde_json::json!({"type":"object","properties":{}})),
            url: None,
            error: None,
            rule: Some("cua_repl: Allow Computer Use to use \"Google Chrome\"?".into()),
        };
        let line = chat_event(&event, "chat-1").expect("MCP confirmations are rendered");
        assert!(line.ends_with("elicitation accept|decline|cancel --content '{}' [--always]"));
    }

    #[test]
    fn a_subagent_permission_request_names_the_subagent() {
        let request = |agent| ChatEvent::PermissionRequest {
            request_id: "r1".into(),
            tool_name: "Bash".into(),
            input: serde_json::json!({ "command": "ls" }),
            title: Some("Claude wants to run ls".into()),
            agent,
        };
        let own = chat_event(&request(None), "c").unwrap();
        assert!(own.starts_with("? Claude wants to run ls\n"), "{own}");
        let agent = chauffeur_protocol::events::RequestAgent {
            id: "a1".into(),
            subagent_type: Some("general-purpose".into()),
            description: None,
            background: true,
        };
        let sub = chat_event(&request(Some(agent)), "c").unwrap();
        assert!(
            sub.starts_with("? Claude wants to run ls (subagent general-purpose, background)\n"),
            "{sub}"
        );
    }

    /// The live chat's envelope around a chat event, as `chauffeur attach` receives it.
    fn in_chat(event: ChatEvent) -> EventEnvelope {
        EventEnvelope {
            seq: Some(1),
            ts: String::new(),
            event: Event::Chat {
                workspace_id: "ws-1".into(),
                chat_id: "chat-1".into(),
                event,
            },
        }
    }

    fn tool_call(
        id: &str,
        name: &str,
        input: serde_json::Value,
        parent: Option<&str>,
    ) -> ChatEvent {
        ChatEvent::ToolCall {
            tool_use_id: id.into(),
            name: name.into(),
            input,
            parent_tool_use_id: parent.map(Into::into),
        }
    }

    fn tool_result(id: &str, output: &str) -> ChatEvent {
        ChatEvent::ToolResult {
            tool_use_id: id.into(),
            output: output.into(),
            is_error: false,
            images: None,
            attachments: None,
        }
    }

    /// What Claude Code answers a `SubagentHandback` call with.
    const DELIVERED: &str = r#"{"success":true,"message":"Report delivered to your caller."}"#;

    /// An Agent call's result when the subagent handed its report back (from a real transcript).
    const POINTER: &str = "  This agent's report was delivered to you as a message from \"ad34a41fa97ee1e60\" (its SubagentHandback call). Read it there; it is not repeated here.\n  \nagentId: ad34a41fa97ee1e60 (use SendMessage with to: 'ad34a41fa97ee1e60', summary: '<5-10 word recap>' to continue this agent)\n<usage>subagent_tokens: 34639\ntool_uses: 1\nduration_ms: 5301</usage>";

    fn transcript(events: Vec<ChatEvent>) -> SubagentMessagesResult {
        SubagentMessagesResult {
            agent_id: Some("ad34a41fa97ee1e60".into()),
            found: true,
            events,
        }
    }

    #[test]
    fn a_subagent_shows_its_handed_back_report_in_full() {
        let long: String = (1..=40)
            .map(|n| format!("Finding {n}: the renderer cut this report at 120 characters before."))
            .collect::<Vec<_>>()
            .join("\n");
        let text = subagent_messages(
            &transcript(vec![
                ChatEvent::AssistantText {
                    message_id: "m:0".into(),
                    text: "Reading render.rs.".into(),
                    parent_tool_use_id: None,
                },
                tool_call(
                    "toolu_hb",
                    HANDBACK,
                    serde_json::json!({ "message": long }),
                    None,
                ),
                tool_result("toolu_hb", DELIVERED),
            ]),
            "my-ws",
        );
        assert!(text.ends_with(&format!("\n\n── Report\n{long}")), "{text}");
        assert!(!text.contains("⏺ SubagentHandback"), "{text}");
        assert!(
            !text.contains("Report delivered"),
            "the acknowledgement is left out: {text}"
        );
        assert!(text.contains("\nReading render.rs."), "{text}");
    }

    #[test]
    fn a_handback_without_a_report_still_reads() {
        // An empty message: said, not an empty block.
        let empty = subagent_messages(
            &transcript(vec![tool_call(
                "toolu_hb",
                HANDBACK,
                serde_json::json!({ "message": "  \n" }),
                None,
            )]),
            "c",
        );
        assert!(empty.ends_with("── Report\n(empty report)"), "{empty}");
        // No message, or not a string: the call as any other, and its result kept.
        for input in [serde_json::json!({}), serde_json::json!({ "message": 42 })] {
            let text = subagent_messages(
                &transcript(vec![
                    tool_call("toolu_hb", HANDBACK, input.clone(), None),
                    tool_result("toolu_hb", "InputValidationError"),
                ]),
                "c",
            );
            assert!(
                text.contains(&format!("⏺ SubagentHandback({input})")),
                "{text}"
            );
            assert!(text.contains("⎿ InputValidationError"), "{text}");
            assert!(!text.contains("── Report"), "{text}");
        }
        // A failed hand-back's error is shown.
        let mut renderer = EventRenderer::default();
        renderer.chat_event(
            &tool_call(
                "toolu_hb",
                HANDBACK,
                serde_json::json!({ "message": "x" }),
                None,
            ),
            "c",
        );
        let error = ChatEvent::ToolResult {
            tool_use_id: "toolu_hb".into(),
            output: "denied".into(),
            is_error: true,
            images: None,
            attachments: None,
        };
        assert_eq!(
            renderer.chat_event(&error, "c").unwrap(),
            "  ⎿ error: denied"
        );
    }

    #[test]
    fn the_main_chat_shows_a_handed_back_report_and_where_to_read_it() {
        let mut renderer = EventRenderer::default();
        let mut lines = Vec::new();
        for event in [
            tool_call(
                "toolu_a",
                "Agent",
                serde_json::json!({ "description": "e2e lighthouse", "prompt": "Go" }),
                None,
            ),
            tool_call(
                "toolu_hb",
                HANDBACK,
                serde_json::json!({ "message": "one\ntwo\nthree\nfour\nfive" }),
                Some("toolu_a"),
            ),
            tool_result("toolu_hb", DELIVERED),
            tool_result("toolu_a", POINTER),
        ] {
            lines.extend(renderer.event(&in_chat(event), "my-ws"));
        }
        assert_eq!(
            lines,
            [
                "⏺ Agent(e2e lighthouse)",
                "  (subagent report) one\n    two\n    three … (+2 lines)",
                "  ⎿ report handed back by subagent ad34a41fa97ee1e60; in full: chauffeur subagent my-ws ad34a41fa97ee1e60",
            ]
        );
        // Without a chat to name (a nested subagent's call, in `chauffeur subagent`), no command.
        assert_eq!(
            chat_event(&tool_result("toolu_a", POINTER), "").unwrap(),
            "  ⎿ report handed back by subagent ad34a41fa97ee1e60"
        );
        // A result that only quotes the sentence with no usable id stays as it was.
        let odd = r#"report was delivered to you as a message from "" (nobody)"#;
        assert_eq!(
            chat_event(&tool_result("t", odd), "c").unwrap(),
            format!("  ⎿ {odd}")
        );
    }
}
