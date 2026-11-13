//! `chauffeur`: every action of the app, from a terminal or from an agent.

mod client;
mod cloud;
mod daemon_guard;
mod database;
mod dev_services;
mod hooks;
mod import;
mod linear_key;
mod render;
mod routine;
mod service;
mod transfer;
mod worker;

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Context;
use chauffeur_protocol::client::*;
use chauffeur_protocol::model::{
    AccountStatus, AgentKind, BriefState, ChatInfo, ChatKind, DesignBrief, ImageInput, LinearIssue,
    PermissionMode, RequestResponse, SectionInfo, SendMode, UpdatePhase, UpdateStatus,
    WorkspaceInfo,
};
use chauffeur_protocol::workspace_share::{Guest, GuestRole};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::Value;

use crate::client::{Client, Home};

#[derive(Parser)]
#[command(
    name = "chauffeur",
    version,
    about = "Multi-repo workspaces for coding agents",
    after_help = "With --json, a failed command prints {\"error\":{\"code\",\"message\",\"data\"}} on \
                  stdout (code and data as the daemon gave them, null when it did not answer) and \
                  exits with 2 for a bad request, 3 for something not found, 4 for a conflict, 5 \
                  when something is unavailable, 1 otherwise. A command that reports a state with \
                  its exit code (`daemon status` when the daemon is stopped, `doctor`, `wait` on its \
                  timeout) prints its result, not an error."
)]
struct Cli {
    /// Print raw JSON results instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// PostgreSQL connections, SQL consoles and table edits.
    Database {
        #[command(subcommand)]
        action: database::Action,
    },
    /// Routines: prompts Chauffeur runs by itself at set times, made from a chat, refined, then
    /// activated.
    Routine {
        #[command(subcommand)]
        action: routine::Action,
    },
    /// Start, stop or inspect the daemon.
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Import from another tool: `chauffeur import conductor`.
    Import {
        #[command(subcommand)]
        source: import::Source,
    },
    /// Transfer workspaces to other machines, and recover or back up those on a GCP worker.
    Cloud {
        #[command(subcommand)]
        action: cloud::Action,
    },
    /// GCP workers a workspace can be transferred to.
    Worker {
        #[command(subcommand)]
        action: worker::Action,
    },
    /// Check what Chauffeur needs on this machine.
    Doctor,
    /// Repos a workspace can include, `owner/name` outside the main organization (* = default selection).
    Repos {
        #[arg(long)]
        refresh: bool,
    },
    /// Create a workspace: one worktree per repo, on one branch.
    New {
        /// Copy conversations and notes from a shared workspace link (no synchronization).
        #[arg(long)]
        workspace_share: Option<String>,
        /// Repo to include (repeatable); the default set when absent.
        #[arg(short, long = "repo")]
        repos: Vec<String>,
        /// Linear ticket to attach (repeatable).
        #[arg(short, long = "ticket")]
        tickets: Vec<String>,
        /// Attach the open issues of a Linear project milestone (id, URL or name).
        #[arg(long)]
        milestone: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        /// Create a child workspace of this one (id or slug).
        #[arg(long, env = "CHAUFFEUR_PARENT")]
        parent: Option<String>,
        /// First message of a chat started in the workspace.
        #[arg(short, long)]
        prompt: Option<String>,
        /// Image or file to send with the first message (repeatable).
        #[arg(short, long = "image", visible_alias = "file")]
        images: Vec<PathBuf>,
        #[arg(long, value_enum)]
        agent: Option<Agent>,
        /// Model id, as `chauffeur models` lists them.
        #[arg(long)]
        model: Option<String>,
        /// Effort level, among the model's (`chauffeur models`).
        #[arg(long)]
        effort: Option<String>,
        /// Fast mode, for models that have one.
        #[arg(long)]
        fast: bool,
        /// Permission mode of the chat (default: auto).
        #[arg(long, value_enum)]
        permission_mode: Option<Mode>,
        /// An exploration: each repo checked out at its base branch (origin/staging…), no branch,
        /// setup or ports; its chats are read-only.
        #[arg(long, conflicts_with_all = ["branch", "parent", "permission_mode"])]
        explore: bool,
        /// Return at once, as the app does: the workspace is set up in the background and its
        /// first chat, shown at once, starts once it is ready.
        #[arg(long)]
        background: bool,
        /// A design brought into the workspace's context: a public Claude artifact, a share's
        /// link, or one of your design chats (id); copied into a design chat of the workspace.
        #[arg(long)]
        design: Option<String>,
    },
    /// Open a workspace folder (or one repo's worktree) in an editor, a terminal or the file manager.
    Open {
        /// Workspace id or slug; without it, lists the apps.
        workspace: Option<String>,
        /// Only this repo's worktree.
        #[arg(short, long)]
        repo: Option<String>,
        /// cursor, vscode, zed, ghostty…, finder (files on Linux); the last one used when absent.
        #[arg(short, long)]
        app: Option<String>,
        /// A design chat's folder instead (an editor or the file manager).
        #[arg(long, value_name = "CHAT", conflicts_with = "repo")]
        design: Option<String>,
    },
    /// List the commands a chat runs with `/name` in a workspace: skills and commands (yours, the
    /// plugins', each repo's) and the agent's own that work in a chat.
    Commands {
        /// Workspace id or slug.
        #[arg(env = "CHAUFFEUR_WORKSPACE_ID")]
        workspace: String,
        #[arg(long, value_enum, default_value = "claude")]
        agent: Agent,
    },
    /// List the models each agent offers, with their effort levels.
    Models {
        /// Ask the agents again instead of the daemon's cached lists.
        #[arg(long)]
        refresh: bool,
        /// Those of the machine this workspace runs on (one transferred there).
        #[arg(long)]
        workspace: Option<String>,
    },
    /// List workspaces.
    Ls {
        /// Include archived workspaces.
        #[arg(short, long)]
        all: bool,
        /// Only the children of this workspace.
        #[arg(long)]
        parent: Option<String>,
    },
    /// Show a workspace.
    Show { workspace: String },
    /// File a workspace in a sidebar section (`chauffeur section ls`), or in none.
    Move {
        workspace: String,
        /// Section name or id.
        #[arg(required_unless_present = "none", conflicts_with = "none")]
        section: Option<String>,
        /// Back to no section: the "Workspaces" list.
        #[arg(long)]
        none: bool,
    },
    /// The sidebar's sections, to file workspaces by theme: list, create, rename, reorder, fold,
    /// delete.
    Section {
        #[command(subcommand)]
        action: Option<SectionAction>,
    },
    /// Add a repo to a workspace, on its branch.
    AddRepo { workspace: String, repo: String },
    /// Archive a workspace: stop everything, snapshot uncommitted work, remove the worktrees. One
    /// that runs on another machine comes back first, its agents interrupted, and is archived here
    /// once back; that machine stops if no other workspace runs there.
    Archive {
        workspace: String,
        /// Archive even while an agent works, and despite failed archive scripts.
        #[arg(long)]
        force: bool,
    },
    /// Restore an archived workspace from its snapshots.
    Restore { workspace: String },
    /// Change the name a workspace is shown with (its folder and branch keep theirs); no name
    /// shows the folder name again.
    Rename {
        workspace: String,
        #[arg(num_args = 0.., trailing_var_arg = true)]
        title: Vec<String>,
    },
    /// Turn an exploration into a workspace: a branch per repo, ports, setup scripts; its chats
    /// can write from their next message, their conversation kept. Refused while its checkouts
    /// hold files their commit does not (what its commands wrote), unless --discard.
    Promote {
        workspace: String,
        /// Delete for good what the checkouts hold beyond their commit (changes, new and ignored
        /// files) before continuing.
        #[arg(long)]
        discard: bool,
    },
    /// Wait until the workspace's agents are done (or one chat's agent, given its id), then print
    /// the last answer.
    Wait {
        /// Workspace id or slug, or a chat's id.
        workspace: String,
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// The workspace's state, its chats and the last answer, without waiting (or one chat's,
    /// given its id).
    Status { workspace: String },
    /// Send a message to a chat (or to a workspace's first chat).
    Send {
        target: String,
        #[arg(required_unless_present = "images", num_args = 1..)]
        text: Vec<String>,
        /// Image or file to send along (repeatable): PNG, JPEG, GIF and WebP images are shown to
        /// the agent, any other file (a zip, an HTML page) is saved for it to open.
        #[arg(short, long = "image", visible_alias = "file")]
        images: Vec<PathBuf>,
        /// While the agent works, wait in the chat's queue for the end of its turn instead of
        /// steering it.
        #[arg(long)]
        queue: bool,
    },
    /// Interrupt the agent of a chat.
    Interrupt { target: String },
    /// Stop one task of a chat's agent: a background command or a subagent (its id is in the
    /// chat's `task started` line).
    StopTask { target: String, task_id: String },
    /// How full a chat's context window is (Claude Code's `/context`): from its running session,
    /// else its last reading.
    Context {
        target: String,
        /// Count each category exactly and list what it holds (MCP tools, memory files, skills,
        /// agents): slower.
        #[arg(long)]
        full: bool,
    },
    /// A design chat's canvas: its pages, sections and screens, with their files.
    Canvas { chat: String },
    /// A design chat's screens as one PNG, in a row with their labels: kept in the design
    /// folder's exports/ and put on the clipboard.
    Export {
        chat: String,
        /// The screens' files, as `chauffeur canvas` lists them, in the order they go in the row.
        #[arg(required = true)]
        files: Vec<String>,
        /// What the PNG is named after (the first screen's label when not given).
        #[arg(long)]
        name: Option<String>,
    },
    /// A design chat's comments, by screen, with their replies and whether they were resolved
    /// or sent to the agent.
    Comments { chat: String },
    /// A design chat's share: its state; `--with` shares it (or changes its reviewers), `--team`
    /// opens it to every member of the team (their Office's Design window lists it), `--stop`
    /// ends it.
    Share {
        chat: String,
        /// The reviewers' e-mails, comma-separated: shares the design with them.
        #[arg(long, value_delimiter = ',')]
        with: Option<Vec<String>>,
        /// Open to the whole team too.
        #[arg(long, conflicts_with = "no_team")]
        team: bool,
        /// Closed to the team: the named reviewers only.
        #[arg(long)]
        no_team: bool,
        #[arg(long)]
        stop: bool,
    },
    /// A workspace's share: its state; `--with` and `--suggest` share its chats and notes with
    /// colleagues (or replace its guests), viewers or able to suggest messages to its agents;
    /// `--team` opens it to every member as viewers; `--chats` shares only some chats, `--all-chats`
    /// all of them again; `--stop` ends it.
    ShareWorkspace {
        workspace: String,
        /// Share only these chats (ids, comma-separated), on a new or a live share; a chat created
        /// later stays private.
        #[arg(long, value_delimiter = ',', conflicts_with = "all_chats")]
        chats: Option<Vec<String>>,
        /// Share all chats, those created later too (the default of a new share).
        #[arg(long)]
        all_chats: bool,
        /// The viewers' e-mails, comma-separated.
        #[arg(long, value_delimiter = ',')]
        with: Option<Vec<String>>,
        /// The e-mails of those who may suggest messages, comma-separated.
        #[arg(long, value_delimiter = ',')]
        suggest: Option<Vec<String>>,
        /// Open to the whole team too, as viewers.
        #[arg(long, conflicts_with = "no_team")]
        team: bool,
        /// Closed to the team: the named guests only.
        #[arg(long)]
        no_team: bool,
        #[arg(long, conflicts_with_all = ["with", "suggest", "team", "no_team", "chats", "all_chats"])]
        stop: bool,
    },
    /// A design shared with you, taken over: its screens and comments into a new design chat
    /// of a workspace, for your own agent (the share's page gives this command).
    ImportDesign {
        workspace: String,
        /// The share's link, `https://<share service>/s/<id>`.
        url: String,
    },
    /// Comments on a screen of a design chat (or replies to a comment with --reply-to).
    Comment {
        chat: String,
        /// The screen's file, as `chauffeur canvas` lists it.
        screen: String,
        text: String,
        /// Where on the screen, in percent: `x,y` (the whole screen when not given).
        #[arg(long)]
        at: Option<String>,
        /// The comment this one replies to (its id, as `chauffeur comments` shows it).
        #[arg(long)]
        reply_to: Option<String>,
        /// Someone the comment mentions, by e-mail (repeatable): a person the design's share is
        /// open to, told in their Chauffeur and on the share's page. Name them in the text too,
        /// as `@Name`.
        #[arg(long = "mention", value_name = "EMAIL")]
        mentions: Vec<String>,
    },
    /// A markdown file's comments (a note, a plan, a `.md` of a repo), with their replies and
    /// whether they were resolved or sent; `--send` sends the new ones to the chat whose agent
    /// last wrote the file, or to `--to`.
    FileComments {
        workspace: String,
        /// The file, as `chauffeur files` names it: `<repo>/<path>` or `.chauffeur/<path>`.
        path: String,
        #[arg(long)]
        send: bool,
        /// The chat the comments go to, instead of the one that last wrote the file.
        #[arg(long, requires = "send")]
        to: Option<String>,
    },
    /// A markdown file's share (a note, a plan, a `.md` of a repo): its state; `--with` shares
    /// the file alone with reviewers (or changes them), `--team` opens it to every member of the
    /// team, `--stop` ends it.
    ShareFile {
        workspace: String,
        /// The file, as `chauffeur files` names it: `<repo>/<path>` or `.chauffeur/<path>`.
        path: String,
        /// The reviewers' e-mails, comma-separated: shares the file with them.
        #[arg(long, value_delimiter = ',')]
        with: Option<Vec<String>>,
        /// Open to the whole team too.
        #[arg(long, conflicts_with = "no_team")]
        team: bool,
        /// Closed to the team: the named reviewers only.
        #[arg(long)]
        no_team: bool,
        #[arg(long, conflicts_with_all = ["with", "team", "no_team"])]
        stop: bool,
    },
    /// Comments on a passage of a markdown file (or replies to a comment with --reply-to).
    FileComment {
        workspace: String,
        /// The file, as `chauffeur files` names it: `<repo>/<path>` or `.chauffeur/<path>`.
        path: String,
        text: String,
        /// The passage, as the rendered file reads (the whole file when not given).
        #[arg(long, conflicts_with = "reply_to")]
        quote: Option<String>,
        /// The comment this one replies to (its id, as `chauffeur file-comments` shows it).
        #[arg(long)]
        reply_to: Option<String>,
    },
    /// Your mentions in the comments of shares open to you: the unread ones first, each with the
    /// link that opens its thread.
    Mentions,
    /// A subagent's conversation (Claude), read from its transcript: running or finished, with
    /// or without a running session.
    Subagent {
        target: String,
        /// Its agent id (the `task_id` of its `task started` line), or the id of the tool call
        /// that started it (`toolu_…`).
        subagent: String,
    },
    /// Answer a permission request or a question.
    Respond {
        target: String,
        request_id: String,
        #[command(subcommand)]
        answer: Reply,
    },
    /// Chats of a workspace: its open ones, the closed ones too with --all.
    Chats {
        workspace: String,
        #[arg(long)]
        all: bool,
    },
    /// Start a new chat in a workspace (`chauffeur chat <workspace>`), or rename, close, reopen,
    /// fork or delete one.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Chat {
        #[command(subcommand)]
        action: Option<ChatAction>,
        /// The workspace to start a chat in.
        #[arg(required = true)]
        workspace: Option<String>,
        #[arg(long, value_enum)]
        agent: Option<Agent>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        effort: Option<String>,
        #[arg(long)]
        fast: bool,
        /// Permission mode of the chat (default: auto).
        #[arg(long, value_enum)]
        permission_mode: Option<Mode>,
        #[arg(short, long)]
        prompt: Option<String>,
        /// Image or file to send with the prompt (repeatable).
        #[arg(short, long = "image", visible_alias = "file")]
        images: Vec<PathBuf>,
        /// A design chat: its agent designs screens in its own folder of the notes and never
        /// changes code (Claude Code unless `--agent codex`).
        #[arg(long, conflicts_with = "permission_mode")]
        design: bool,
        /// The chat a design chat starts from: its design brief is written from it and printed
        /// once ready, for you to edit and send (`chauffeur send`).
        #[arg(
            long = "from",
            value_name = "CHAT",
            requires = "design",
            conflicts_with = "prompt"
        )]
        from: Option<String>,
    },
    /// Follow a workspace or a chat as it happens.
    Attach {
        target: String,
        /// Replay the history first.
        #[arg(long)]
        history: bool,
    },
    /// Linear tickets of a workspace: attach, detach, read again from Linear.
    Ticket {
        #[command(subcommand)]
        action: TicketAction,
    },
    /// Worktrees agents create inside a workspace.
    Worktree {
        #[command(subcommand)]
        action: WorktreeAction,
    },
    /// Start a repo's run script.
    Run {
        workspace: String,
        repo: String,
        script: Option<String>,
    },
    /// Processes (setup, run, archive scripts) of a workspace.
    Ps { workspace: String },
    /// A workspace's development services: the Docker services whose data follows it to another
    /// machine, and those rebuilt there.
    Services {
        #[arg(env = "CHAUFFEUR_WORKSPACE_ID")]
        workspace: String,
        #[command(subcommand)]
        action: Option<dev_services::Action>,
    },
    /// What each repo of a workspace changed against its base branch.
    Diff {
        workspace: String,
        #[arg(long)]
        repo: Option<String>,
        /// Files and counts only.
        #[arg(long)]
        stat: bool,
    },
    /// Files of a workspace: each repo's (tracked, and new ones git does not ignore) and its notes.
    Files {
        workspace: String,
        /// Only this repo, or `.chauffeur` for the notes.
        #[arg(long)]
        root: Option<String>,
    },
    /// Search the files of a workspace's repos and notes.
    Search {
        workspace: String,
        query: String,
        /// The query is an extended regular expression.
        #[arg(short = 'E', long)]
        regex: bool,
        #[arg(short = 's', long)]
        case_sensitive: bool,
        /// Only this repo, or `.chauffeur` for the notes.
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        max: Option<u32>,
    },
    /// Pull requests of a workspace, one per repo.
    Prs { workspace: String },
    /// Ask the workspace's agent to open its PRs (one per repo, linked together).
    Pr {
        workspace: String,
        #[arg(long)]
        draft: bool,
        /// Extra instructions for the agent.
        #[arg(long)]
        instructions: Option<String>,
    },
    /// Merge the workspace's open pull requests, one repo after the other; the first failure stops
    /// the rest.
    Merge {
        workspace: String,
        #[arg(long, value_enum, default_value_t = Method::Squash)]
        method: Method,
        /// A PR to merge, as its repo and head branch, in order (repeatable; `chauffeur prs`
        /// lists them); every open PR when absent, APIs first.
        #[arg(long = "pr", value_name = "REPO:BRANCH", value_parser = parse_pr_ref)]
        prs: Vec<PrRef>,
    },
    /// What is left of Claude Code's and Codex's plan limits (5-hour and weekly windows).
    Usage {
        /// Ask the agents now instead of the reading of the last few minutes.
        #[arg(long)]
        refresh: bool,
    },
    /// Logs Claude Code or Codex back in to its account: prints the page to open in a browser,
    /// and waits for the login to complete.
    Login {
        #[arg(value_enum)]
        agent: Agent,
    },
    /// Whether Chauffeur keeps the machine awake now (`keep_awake` in settings.toml), and why.
    Power,
    /// Opens the Office in your default browser, where calls work when the app's web view has no
    /// WebRTC (Linux); prints its one-time link.
    Office {
        #[arg(long, required = true)]
        browser: bool,
    },
    /// Whether a newer Chauffeur.app is published; with --install, puts it in place of this one.
    Update {
        /// Download, check and install it, then wait until it is in place.
        #[arg(long)]
        install: bool,
    },
    /// The environment agents, terminals and scripts get: the login shell's.
    Env {
        #[command(subcommand)]
        action: EnvAction,
    },
    /// GitHub: whether git is installed and which account Chauffeur reaches GitHub with; connect
    /// one in the browser (installing GitHub CLI when missing) or disconnect it.
    Github {
        #[command(subcommand)]
        action: Option<GithubAction>,
    },
    /// Ollama, which serves the local models: its state and models; download or delete one.
    Ollama {
        #[command(subcommand)]
        action: Option<OllamaAction>,
    },
    /// Compaction of Claude and Codex chats through Jev (TypeSafe): status, on, off, threshold, API key.
    Jev {
        #[command(subcommand)]
        action: Option<JevAction>,
    },
    /// Linear: store the API key, search and show issues.
    Linear {
        #[command(subcommand)]
        action: LinearAction,
    },
    /// Stop a process.
    Stop { process: String },
    #[command(hide = true)]
    HookRun {
        #[arg(long)]
        repo: PathBuf,
        #[arg(long)]
        shell: String,
    },
    #[command(hide = true)]
    GitContext,
    /// Linear's custom script entry point: open (or reuse) the workspace of an issue.
    #[command(hide = true)]
    OpenIssue { identifier: String },
}

#[derive(Subcommand)]
enum DaemonAction {
    Start,
    Stop {
        /// Stop the default daemon even from a workspace or an agent's session. Hidden, and never
        /// named by the refusal: an agent must not take it by reflex (see daemon_guard).
        #[arg(long, hide = true)]
        real_daemon: bool,
    },
    Status,
    /// Run the daemon at login (a LaunchAgent on macOS, a systemd user service on Linux), restarted if it crashes.
    Install {
        #[arg(long, hide = true)]
        real_daemon: bool,
    },
    /// Stop running the daemon at login.
    Uninstall {
        #[arg(long, hide = true)]
        real_daemon: bool,
    },
    /// Print the daemon's log file path.
    Logs,
}

#[derive(Subcommand)]
enum LinearAction {
    /// Save a personal Linear API key, as Settings (⌘,) does (prompted, never on the command line).
    Login,
    /// Search issues; without a query, your open issues.
    Search { query: Vec<String> },
    /// Show an issue.
    Show { identifier: String },
    /// Make Linear's "Work on issue → Custom script" open the issue in Chauffeur.
    InstallTool,
}

#[derive(Subcommand)]
enum EnvAction {
    /// Read the login shell's environment again (a variable exported in ~/.zshenv since the
    /// daemon started): what starts afterwards gets it, and idle chats whose MCP servers failed
    /// start again with it.
    Reload,
}

#[derive(Subcommand)]
enum GithubAction {
    /// Whether git is installed, and the GitHub account connected.
    Status,
    /// Connect a GitHub account in the browser, installing GitHub CLI first when missing; no SSH
    /// key needed: git pushes over HTTPS through it.
    Login,
    /// Disconnect the GitHub account.
    Logout,
}

#[derive(Subcommand)]
enum OllamaAction {
    /// Whether Ollama is installed and running, its models and the downloads in progress.
    Status,
    /// Install Chauffeur's own Ollama (macOS and Linux), then wait until it is in place.
    Install,
    /// Download a model (e.g. `qwen3.8`), then wait until it is on disk.
    Pull { model: String },
    /// Delete a model from disk.
    Rm { model: String },
}

#[derive(Subcommand)]
enum JevAction {
    /// Whether Jev is on, its threshold and whether a key is saved.
    Status,
    /// Compact Claude and Codex chats through Jev (needs a saved key).
    On,
    Off,
    /// Compact a chat once its context reaches this share (%), as Claude Code's /context counts it.
    Threshold {
        percent: u8,
    },
    /// Save a TypeSafe API key (prompted, or read from standard input; never on the command line).
    Key {
        /// Forget the saved key (turns Jev off).
        #[arg(long)]
        remove: bool,
    },
}

#[derive(Subcommand)]
enum TicketAction {
    /// Attach tickets (moved to In Progress as at creation); the agents read them from their next
    /// message.
    Add {
        #[arg(required = true)]
        identifiers: Vec<String>,
        #[arg(long, env = "CHAUFFEUR_WORKSPACE_ID")]
        workspace: String,
    },
    /// Detach tickets.
    Remove {
        #[arg(required = true)]
        identifiers: Vec<String>,
        #[arg(long, env = "CHAUFFEUR_WORKSPACE_ID")]
        workspace: String,
    },
    /// Read the workspace's tickets from Linear again (one it could not read, a description that
    /// changed).
    Refresh {
        #[arg(long, env = "CHAUFFEUR_WORKSPACE_ID")]
        workspace: String,
    },
}

#[derive(Subcommand)]
enum SectionAction {
    /// List the sections, in the sidebar's order.
    Ls,
    /// Create a section, last of them.
    New {
        #[arg(required = true, num_args = 1..)]
        name: Vec<String>,
        /// A workspace to file in it.
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Rename a section.
    Rename {
        section: String,
        #[arg(required = true, num_args = 1.., trailing_var_arg = true)]
        name: Vec<String>,
    },
    /// Move a section above another one, or last.
    Move {
        section: String,
        #[arg(long)]
        before: Option<String>,
    },
    /// Fold a section in the sidebar.
    Fold { section: String },
    /// Unfold a section in the sidebar.
    Unfold { section: String },
    /// Delete a section; its workspaces go back to the "Workspaces" list.
    Rm { section: String },
}

#[derive(Subcommand)]
enum ChatAction {
    /// Change the title of a chat's tab; without a title, the automatic one comes back.
    Rename {
        chat: String,
        #[arg(num_args = 0.., trailing_var_arg = true)]
        title: Vec<String>,
    },
    /// Take a chat out of the tabs, its conversation kept; its agent session stops.
    Close {
        chat: String,
        /// Stop its agent first if it works (a turn, tasks, queued messages, a question).
        #[arg(long)]
        force: bool,
    },
    /// Put a closed chat back among the tabs; its agent resumes at its next message.
    Reopen { chat: String },
    /// Fork a chat into a new tab: same agent and settings, its conversation so far (or up to the
    /// end of a turn it finished), then each goes its own way.
    Fork {
        chat: String,
        /// Fork the conversation as it stood at the end of this turn (counted from 1), even while
        /// the agent runs a later one.
        #[arg(long)]
        turn: Option<u32>,
        /// Fork into this agent: into the other one, it reads the conversation as text with the
        /// fork's next message, on that agent's default model.
        #[arg(long, value_enum)]
        agent: Option<Agent>,
    },
    /// Delete a chat and its conversation for good (by its id; asks first unless --yes).
    Delete {
        chat: String,
        #[arg(short, long)]
        yes: bool,
        /// Stop its agent first if it works (a turn, tasks, queued messages, a question).
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum WorktreeAction {
    /// Create a worktree group (one worktree per repo) and print its folder.
    Add {
        #[arg(long, env = "CHAUFFEUR_WORKSPACE_ID")]
        workspace: String,
        /// Repo (repeatable); every repo of the workspace when absent.
        #[arg(short, long = "repo")]
        repos: Vec<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        branch: Option<String>,
    },
    /// Remove agent worktrees by group name, folder, path or id.
    Remove {
        target: String,
        #[arg(long, env = "CHAUFFEUR_WORKSPACE_ID")]
        workspace: String,
    },
}

#[derive(Subcommand)]
enum Reply {
    /// Answer an MCP confirmation with schema-validated JSON content.
    Elicitation {
        #[arg(value_parser = ["accept", "decline", "cancel"])]
        action: String,
        #[arg(long)]
        content: Option<String>,
        /// Keep accepting this confirmation for the chat (one with nothing to fill in).
        #[arg(long)]
        always: bool,
    },
    Allow {
        /// Keep allowing this tool for the chat.
        #[arg(long)]
        always: bool,
    },
    Deny {
        #[arg(long)]
        message: Option<String>,
    },
    /// Answers as `question=answer` pairs.
    Answer {
        #[arg(required = true)]
        pairs: Vec<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Agent {
    Claude,
    Codex,
}

impl From<Agent> for AgentKind {
    fn from(agent: Agent) -> Self {
        match agent {
            Agent::Claude => AgentKind::Claude,
            Agent::Codex => AgentKind::Codex,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Method {
    /// Squash and merge.
    Squash,
    /// A merge commit.
    Merge,
    Rebase,
}

impl From<Method> for MergeMethod {
    fn from(method: Method) -> Self {
        match method {
            Method::Squash => MergeMethod::Squash,
            Method::Merge => MergeMethod::Merge,
            Method::Rebase => MergeMethod::Rebase,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    /// Claude Code's auto mode: a classifier runs safe actions and asks only for risky ones.
    Auto,
    /// Asks before edits and commands.
    Default,
    /// Edits freely, asks before commands.
    AcceptEdits,
    /// Reads and plans, changes nothing.
    Plan,
    /// Never asks.
    BypassPermissions,
}

impl From<Mode> for PermissionMode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Auto => PermissionMode::Auto,
            Mode::Default => PermissionMode::Default,
            Mode::AcceptEdits => PermissionMode::AcceptEdits,
            Mode::Plan => PermissionMode::Plan,
            Mode::BypassPermissions => PermissionMode::BypassPermissions,
        }
    }
}

fn main() {
    // `chauffeur files | head` closes the pipe early: end quietly, as Unix tools do, rather than
    // panic on the next print.
    // SAFETY: setting a signal's disposition before any thread starts.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    // Hook commands must not touch the daemon or tokio: they run on every tool call.
    match &cli.command {
        Command::HookRun { repo, shell } => {
            let code = hooks::hook_run(repo, shell).unwrap_or_else(|error| {
                eprintln!("chauffeur hook-run: {error:#}");
                1
            });
            std::process::exit(code);
        }
        Command::GitContext => {
            let _ = hooks::git_context();
            return;
        }
        _ => {}
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let json = cli.json;
    let code = match runtime.block_on(run(cli)) {
        Ok(code) => code,
        Err(error) if json => {
            let (failure, code) = json_failure(&error);
            println!("{failure}");
            code
        }
        Err(error) => {
            eprintln!("chauffeur: {error:#}");
            1
        }
    };
    std::process::exit(code);
}

/// A failure as `--json` prints it, and the exit code of its kind (see the help).
fn json_failure(error: &anyhow::Error) -> (Value, i32) {
    use chauffeur_protocol::rpc::codes;
    let rpc = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<client::RpcFailure>())
        .map(|failure| &failure.0);
    let exit = match rpc.map(|e| e.code) {
        Some(codes::BAD_REQUEST | codes::INVALID_PARAMS) => 2,
        Some(codes::NOT_FOUND) => 3,
        Some(codes::CONFLICT) => 4,
        Some(codes::UNAVAILABLE) => 5,
        _ => 1,
    };
    let failure = serde_json::json!({"error": {
        "code": rpc.map(|e| e.code),
        "message": format!("{error:#}"),
        "data": rpc.and_then(|e| e.data.clone()),
    }});
    (failure, exit)
}

async fn run(cli: Cli) -> anyhow::Result<i32> {
    let json = cli.json;
    match cli.command {
        Command::Daemon { action } => return daemon(action).await,
        Command::Doctor => return doctor().await,
        Command::Linear {
            action: LinearAction::InstallTool,
        } => return linear_install_tool(),
        Command::HookRun { .. } | Command::GitContext => unreachable!(),
        // Another daemon is the source: this one need not run.
        Command::Cloud {
            action:
                cloud::Action::Transfer {
                    workspaces,
                    to,
                    controller,
                    from: Some(from),
                    keep_local_copy,
                    cancel,
                    interrupt,
                    yes,
                    status,
                    ..
                },
        } => {
            let mut source = Client::connect_socket(&from).await?;
            if status {
                cloud::transfer_status(&mut source, workspaces, json).await?;
                return Ok(0);
            }
            let ask = transfer::Ask {
                to: to.as_deref(),
                controller: controller.as_deref(),
                keep_local_copy,
                cancel,
                interrupt,
                yes,
                beyond_limit: false,
            };
            cloud::transfer(&mut source, workspaces, &ask, json).await?;
            return Ok(0);
        }
        _ => {}
    }
    let mut client = Client::connect(true).await?;
    match cli.command {
        Command::Cloud { action } => cloud::run(action, &mut client, json).await?,
        Command::Import { source } => import::run(source, &mut client, json).await?,
        Command::Worker { action } => worker::run(action, &mut client, json).await?,
        Command::Repos { refresh } => {
            let result: ReposListResult = client
                .call(ClientCall::ReposList(ReposListParams { refresh }))
                .await?;
            print_or(json, &result, || {
                result
                    .repos
                    .iter()
                    .map(render::repo_line)
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            if let (false, Some(error)) = (json, &result.github_error) {
                eprintln!(
                    "GitHub could not list the repos ({error}): these are the cached ones and the local clones."
                );
            }
        }
        Command::Open {
            workspace: None, ..
        } => {
            let result: OpenAppsResult = client.call(ClientCall::OpenApps(EmptyParams {})).await?;
            print_or(json, &result, || {
                result
                    .apps
                    .iter()
                    .map(|a| {
                        format!(
                            "{:10} {}{}",
                            a.id,
                            a.name,
                            if a.is_default { "  (default)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        Command::Open {
            workspace: Some(workspace),
            repo,
            app,
            design,
        } => {
            let result: OpenResult = client
                .call(ClientCall::WorkspaceOpen(OpenParams {
                    workspace,
                    repo,
                    app,
                    design_chat: design,
                    terminal: None,
                }))
                .await?;
            print_or(json, &result, || {
                format!("opened {} in {}", result.path, result.app.name)
            });
        }
        Command::Commands { workspace, agent } => {
            let result: CommandsListResult = client
                .call(ClientCall::CommandsList(CommandsListParams {
                    workspace,
                    agent: agent.into(),
                }))
                .await?;
            print_or(json, &result, || render::commands(&result.commands));
        }
        Command::Models { refresh, workspace } => {
            let result: ModelsListResult = client
                .call(ClientCall::ModelsList(ModelsListParams {
                    refresh,
                    workspace,
                }))
                .await?;
            print_or(json, &result, || render::models(&result.agents));
        }
        Command::New {
            workspace_share,
            repos,
            tickets,
            milestone,
            name,
            branch,
            parent,
            prompt,
            images,
            agent,
            model,
            effort,
            fast,
            permission_mode,
            explore,
            background,
            design,
        } => {
            let result: CreateWorkspaceResult = client
                .call(ClientCall::WorkspaceCreate(CreateWorkspaceParams {
                    workspace_share,
                    repos,
                    no_repos: false,
                    tickets,
                    milestone,
                    name,
                    branch,
                    parent,
                    prompt,
                    images: image_inputs(&images)?,
                    chat_kind: None,
                    agent: agent.map(Into::into),
                    model,
                    effort,
                    fast: fast.then_some(true),
                    permission_mode: permission_mode.map(Into::into),
                    background,
                    explore,
                    design: design.map(|design| {
                        if design.starts_with("https://claude.ai/artifact/") {
                            WorkspaceDesign::Claude { url: design }
                        } else if design.contains("/s/") {
                            WorkspaceDesign::Share { url: design }
                        } else {
                            WorkspaceDesign::Chat { chat: design }
                        }
                    }),
                }))
                .await?;
            print_or(json, &result, || {
                let mut out = render::workspace_details(&result.workspace);
                if let Some(chat) = &result.chat {
                    out.push_str(&format!(
                        "  chat     {} — follow it with `chauffeur attach {}`\n",
                        chat.id, result.workspace.slug
                    ));
                }
                out.trim_end().to_string()
            });
        }
        Command::Ls { all, parent } => {
            let grouped = parent.is_none();
            let result: WorkspacesResult = client
                .call(ClientCall::WorkspaceList(ListWorkspacesParams {
                    include_archived: all,
                    parent,
                }))
                .await?;
            print_or(json, &result, || {
                if result.workspaces.is_empty() && (!grouped || result.sections.is_empty()) {
                    return "no workspaces — create one with `chauffeur new`".into();
                }
                ls_text(&result, grouped)
            });
        }
        Command::Move {
            workspace,
            section,
            none: _,
        } => {
            let result: WorkspaceInfo = client
                .call(ClientCall::WorkspaceSetSection(SetSectionParams {
                    workspace,
                    section,
                }))
                .await?;
            print_or(json, &result, || {
                let line = render::workspace_line(&result);
                match &result.section {
                    Some(section) => format!("{line}\n  in section {section}"),
                    None => format!("{line}\n  in no section"),
                }
            });
        }
        Command::Section { action } => {
            let sections = |result: &SectionsResult| -> String {
                if result.sections.is_empty() {
                    return "no sections — create one with `chauffeur section new <name>`".into();
                }
                result
                    .sections
                    .iter()
                    .map(render::section_line)
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            match action.unwrap_or(SectionAction::Ls) {
                SectionAction::Ls => {
                    let result: SectionsResult =
                        client.call(ClientCall::SectionList(EmptyParams {})).await?;
                    print_or(json, &result, || sections(&result));
                }
                SectionAction::New { name, workspace } => {
                    let result: SectionInfo = client
                        .call(ClientCall::SectionCreate(CreateSectionParams {
                            name: name.join(" "),
                            workspace,
                        }))
                        .await?;
                    print_or(json, &result, || render::section_line(&result));
                }
                SectionAction::Rename { section, name } => {
                    let result: SectionInfo = client
                        .call(ClientCall::SectionRename(RenameSectionParams {
                            section,
                            name: name.join(" "),
                        }))
                        .await?;
                    print_or(json, &result, || render::section_line(&result));
                }
                action @ (SectionAction::Fold { .. } | SectionAction::Unfold { .. }) => {
                    let (section, folded) = match action {
                        SectionAction::Fold { section } => (section, true),
                        SectionAction::Unfold { section } => (section, false),
                        _ => unreachable!(),
                    };
                    let result: SectionInfo = client
                        .call(ClientCall::SectionFold(FoldSectionParams {
                            section,
                            folded,
                        }))
                        .await?;
                    print_or(json, &result, || render::section_line(&result));
                }
                SectionAction::Move { section, before } => {
                    let result: SectionsResult = client
                        .call(ClientCall::SectionMove(MoveSectionParams {
                            section,
                            before,
                        }))
                        .await?;
                    print_or(json, &result, || sections(&result));
                }
                SectionAction::Rm { section } => {
                    let result: SectionsResult = client
                        .call(ClientCall::SectionDelete(SectionParams { section }))
                        .await?;
                    print_or(json, &result, || sections(&result));
                }
            }
        }
        Command::Show { workspace } => {
            let result: WorkspaceInfo = client
                .call(ClientCall::WorkspaceGet(WorkspaceParams { workspace }))
                .await?;
            print_or(json, &result, || {
                render::workspace_details(&result).trim_end().to_string()
            });
        }
        Command::AddRepo { workspace, repo } => {
            let result: WorkspaceInfo = client
                .call(ClientCall::WorkspaceAddRepo(AddRepoParams {
                    workspace,
                    repo,
                }))
                .await?;
            print_or(json, &result, || {
                render::workspace_details(&result).trim_end().to_string()
            });
        }
        Command::Rename { workspace, title } => {
            let result: WorkspaceInfo = client
                .call(ClientCall::WorkspaceRename(RenameWorkspaceParams {
                    workspace,
                    title: title.join(" "),
                }))
                .await?;
            print_or(json, &result, || render::workspace_line(&result));
        }
        Command::Promote { workspace, discard } => {
            let result: WorkspaceInfo = client
                .call(ClientCall::WorkspacePromote(PromoteParams {
                    workspace,
                    discard,
                }))
                .await?;
            print_or(json, &result, || render::workspace_line(&result));
        }
        Command::Archive { workspace, force } => {
            let result: WorkspaceInfo = client
                .call(ClientCall::WorkspaceArchive(ArchiveParams {
                    workspace,
                    force,
                    background: false,
                }))
                .await?;
            print_or(json, &result, || match &result.moved_to {
                Some(machine) => format!(
                    "{}\ncoming back from {machine} first, its agents interrupted; archived here once back (`chauffeur cloud transfer {} --status`)",
                    render::workspace_line(&result),
                    result.slug
                ),
                None => render::workspace_line(&result),
            });
            if result.state == chauffeur_protocol::model::WorkspaceState::CleanupFailed {
                return Ok(1);
            }
        }
        Command::Restore { workspace } => {
            let result: WorkspaceInfo = client
                .call(ClientCall::WorkspaceRestore(WorkspaceParams { workspace }))
                .await?;
            print_or(json, &result, || {
                render::workspace_details(&result).trim_end().to_string()
            });
        }
        Command::Wait { workspace, timeout } => {
            let result: WaitResult = client
                .call(ClientCall::WorkspaceWait(WaitParams {
                    workspace,
                    timeout_secs: timeout,
                }))
                .await?;
            print_or(json, &result, || wait_text(&result));
            if result.timed_out {
                return Ok(2);
            }
        }
        Command::Status { workspace } => {
            let result: WaitResult = client
                .call(ClientCall::WorkspaceWait(WaitParams {
                    workspace,
                    timeout_secs: Some(0),
                }))
                .await?;
            print_or(json, &result, || wait_text(&result));
        }
        Command::Send {
            target,
            text,
            images,
            queue,
        } => {
            let result: ChatInfo = client
                .call(ClientCall::ChatSend(SendParams {
                    client_id: None,
                    chat: target,
                    text: text.join(" "),
                    images: image_inputs(&images)?,
                    mode: queue.then_some(SendMode::Queue),
                    suggestion: None,
                }))
                .await?;
            print_or(json, &result, || render::chat_line(&result));
        }
        Command::Interrupt { target } => {
            let result: ChatInfo = client
                .call(ClientCall::ChatInterrupt(ChatParams { chat: target }))
                .await?;
            print_or(json, &result, || render::chat_line(&result));
        }
        Command::StopTask { target, task_id } => {
            let result: ChatInfo = client
                .call(ClientCall::ChatStopTask(StopTaskParams {
                    chat: target,
                    task_id,
                }))
                .await?;
            print_or(json, &result, || render::chat_line(&result));
        }
        Command::Context { target, full } => {
            let result: ContextUsageResult = client
                .call(ClientCall::ChatContextUsage(ContextUsageParams {
                    chat: target,
                    detail: if full {
                        ContextDetail::Full
                    } else {
                        ContextDetail::Summary
                    },
                }))
                .await?;
            print_or(json, &result, || render::context_usage(&result));
        }
        Command::Canvas { chat } => {
            let result: CanvasResult = client
                .call(ClientCall::DesignCanvas(ChatParams { chat }))
                .await?;
            print_or(json, &result, || render::canvas(&result));
        }
        Command::Export { chat, files, name } => {
            let result: DesignExportResult = client
                .call(ClientCall::DesignExport(DesignExportParams {
                    chat,
                    files,
                    name,
                }))
                .await?;
            print_or(json, &result, || {
                format!(
                    "{}  {}×{}{}",
                    result.path,
                    result.width,
                    result.height,
                    if result.copied {
                        "  copied to the clipboard"
                    } else {
                        ""
                    }
                )
            });
        }
        Command::Share {
            chat,
            with,
            team,
            no_team,
            stop,
        } => {
            let team = (team || no_team).then_some(team);
            let result: DesignShareState = if stop {
                client
                    .call(ClientCall::DesignShareStop(ChatParams { chat }))
                    .await?
            } else if with.is_some() || team.is_some() {
                let share = client
                    .call::<DesignShareState>(ClientCall::DesignShareState(ChatParams {
                        chat: chat.clone(),
                    }))
                    .await?
                    .share
                    .filter(|s| !s.stopped);
                let live = share.is_some();
                // `--team` alone keeps the reviewers named already.
                let reviewers = with
                    .or_else(|| share.map(|s| s.reviewers))
                    .unwrap_or_default();
                let params = ShareStartParams {
                    chat,
                    reviewers,
                    team,
                };
                if live {
                    client.call(ClientCall::DesignShareUpdate(params)).await?
                } else {
                    client.call(ClientCall::DesignShareStart(params)).await?
                }
            } else {
                client
                    .call(ClientCall::DesignShareState(ChatParams { chat }))
                    .await?
            };
            print_or(json, &result, || render::share(&result));
        }
        Command::ShareFile {
            workspace,
            path,
            with,
            team,
            no_team,
            stop,
        } => {
            let team = (team || no_team).then_some(team);
            let file = FileCommentsParams { workspace, path };
            let result: DesignShareState = if stop {
                client.call(ClientCall::FilesShareStop(file)).await?
            } else if with.is_some() || team.is_some() {
                let share = client
                    .call::<DesignShareState>(ClientCall::FilesShareState(file.clone()))
                    .await?
                    .share
                    .filter(|s| !s.stopped);
                let live = share.is_some();
                // `--team` alone keeps the reviewers named already.
                let reviewers = with
                    .or_else(|| share.map(|s| s.reviewers))
                    .unwrap_or_default();
                let params = FileShareStartParams {
                    workspace: file.workspace,
                    path: file.path,
                    reviewers,
                    team,
                };
                if live {
                    client.call(ClientCall::FilesShareUpdate(params)).await?
                } else {
                    client.call(ClientCall::FilesShareStart(params)).await?
                }
            } else {
                client.call(ClientCall::FilesShareState(file)).await?
            };
            print_or(json, &result, || render::share(&result));
        }
        Command::ShareWorkspace {
            workspace,
            chats,
            all_chats,
            with,
            suggest,
            team,
            no_team,
            stop,
        } => {
            let team = (team || no_team).then_some(team);
            let named = with.is_some() || suggest.is_some();
            let result: WorkspaceShareState = if stop {
                client
                    .call(ClientCall::WorkspaceShareStop(WorkspaceParams {
                        workspace,
                    }))
                    .await?
            } else if named || team.is_some() || chats.is_some() || all_chats {
                let share = client
                    .call::<WorkspaceShareState>(ClientCall::WorkspaceShareState(WorkspaceParams {
                        workspace: workspace.clone(),
                    }))
                    .await?
                    .share
                    .filter(|s| !s.stopped);
                let live = share.is_some();
                // `--team` alone keeps the guests named already.
                let guests = if named {
                    guests_of(with.unwrap_or_default(), suggest.unwrap_or_default())
                } else {
                    share.map(|s| s.guests).unwrap_or_default()
                };
                let params = WorkspaceShareStartParams {
                    chat_ids: chats,
                    all_chats: all_chats.then_some(true),
                    workspace,
                    guests,
                    team,
                };
                if live {
                    client
                        .call(ClientCall::WorkspaceShareUpdate(params))
                        .await?
                } else {
                    client.call(ClientCall::WorkspaceShareStart(params)).await?
                }
            } else {
                client
                    .call(ClientCall::WorkspaceShareState(WorkspaceParams {
                        workspace,
                    }))
                    .await?
            };
            print_or(json, &result, || render::workspace_share(&result));
        }
        Command::ImportDesign { workspace, url } => {
            let chat: ChatInfo = client
                .call(ClientCall::DesignImport(DesignImportParams {
                    workspace,
                    url,
                }))
                .await?;
            print_or(json, &chat, || {
                format!(
                    "{}\nOpen it in Chauffeur and tell its agent what to change; the comments are there to hand to it.",
                    render::chat_line(&chat)
                )
            });
        }
        Command::Comments { chat } => {
            let result: CommentsResult = client
                .call(ClientCall::DesignComments(ChatParams { chat }))
                .await?;
            print_or(json, &result, || render::comments(&result));
        }
        Command::Comment {
            chat,
            screen,
            text,
            at,
            reply_to,
            mentions,
        } => {
            let pin = match at {
                Some(at) => {
                    let (x, y) = at
                        .split_once(',')
                        .and_then(|(x, y)| Some((x.trim().parse().ok()?, y.trim().parse().ok()?)))
                        .ok_or_else(|| {
                            anyhow::anyhow!("--at takes `x,y` in percent, as in 48,83")
                        })?;
                    Some(CommentPin {
                        x,
                        y,
                        element: None,
                    })
                }
                None => None,
            };
            let result: DesignComment = client
                .call(ClientCall::DesignCommentAdd(CommentAddParams {
                    chat,
                    screen,
                    pin,
                    text,
                    reply_to,
                    mentions: (!mentions.is_empty()).then(|| {
                        mentions
                            .into_iter()
                            .map(|email| Mention {
                                email,
                                name: String::new(),
                            })
                            .collect()
                    }),
                }))
                .await?;
            print_or(json, &result, || {
                format!("{}  {}", result.id, result.screen)
            });
        }
        Command::FileComments {
            workspace,
            path,
            send,
            to,
        } => {
            let params = FileCommentsParams { workspace, path };
            let result: FileCommentsResult = client
                .call(ClientCall::FilesComments(params.clone()))
                .await?;
            if !send {
                print_or(json, &result, || render::file_comments(&result));
                return Ok(0);
            }
            let Some(chat) = to.or(result.writer) else {
                anyhow::bail!(
                    "no open chat's agent wrote {}: say which chat with --to",
                    params.path
                );
            };
            let sent: CommentsSendResult =
                client.call(ClientCall::FilesCommentsSend(params)).await?;
            if sent.sent > 0 {
                let _: ChatInfo = client
                    .call(ClientCall::ChatSend(SendParams {
                        client_id: None,
                        chat: chat.clone(),
                        text: sent.message.clone(),
                        images: None,
                        mode: Some(SendMode::Queue),
                        suggestion: None,
                    }))
                    .await?;
            }
            print_or(json, &sent, || match sent.sent {
                0 => "every comment was sent already".to_string(),
                n => format!("{n} sent to chat {chat}"),
            });
        }
        Command::FileComment {
            workspace,
            path,
            text,
            quote,
            reply_to,
        } => {
            let result: DesignComment = client
                .call(ClientCall::FilesCommentAdd(FileCommentAddParams {
                    workspace,
                    path,
                    quote,
                    text,
                    reply_to,
                }))
                .await?;
            print_or(json, &result, || {
                format!("{}  {}", result.id, result.screen)
            });
        }
        Command::Mentions => {
            let result: MentionsResult = client
                .call(ClientCall::MentionsList(EmptyParams {}))
                .await?;
            print_or(json, &result, || render::mentions(&result));
        }
        Command::Subagent { target, subagent } => {
            let by_tool = subagent.starts_with("toolu_");
            let result: SubagentMessagesResult = client
                .call(ClientCall::ChatSubagentMessages(SubagentMessagesParams {
                    chat: target.clone(),
                    tool_use_id: by_tool.then(|| subagent.clone()),
                    agent_id: (!by_tool).then(|| subagent.clone()),
                }))
                .await?;
            print_or(json, &result, || {
                render::subagent_messages(&result, &target)
            });
        }
        Command::Respond {
            target,
            request_id,
            answer,
        } => {
            let response = match answer {
                Reply::Elicitation {
                    action,
                    content,
                    always,
                } => RequestResponse::Elicitation {
                    action: serde_json::from_value(serde_json::Value::String(action))?,
                    content: content
                        .map(|text| serde_json::from_str(&text))
                        .transpose()
                        .context("MCP content must be valid JSON")?
                        .unwrap_or(serde_json::Value::Null),
                    always,
                },
                Reply::Allow { always } => RequestResponse::Allow { always },
                Reply::Deny { message } => RequestResponse::Deny { message },
                Reply::Answer { pairs } => {
                    let mut answers = BTreeMap::new();
                    for pair in pairs {
                        let (question, answer) = pair
                            .split_once('=')
                            .context("answers are `question=answer`")?;
                        answers.insert(question.to_string(), answer.to_string());
                    }
                    RequestResponse::Answer { answers }
                }
            };
            let result: ChatInfo = client
                .call(ClientCall::ChatRespond(RespondParams {
                    chat: target,
                    request_id,
                    response,
                }))
                .await?;
            print_or(json, &result, || render::chat_line(&result));
        }
        Command::Chats { workspace, all } => {
            let result: ChatsResult = client
                .call(ClientCall::ChatList(ChatListParams { workspace, all }))
                .await?;
            print_or(json, &result, || {
                result
                    .chats
                    .iter()
                    .map(render::chat_line)
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        Command::Chat {
            action: Some(action),
            ..
        } => {
            let call = match action {
                ChatAction::Rename { chat, title } => ClientCall::ChatRename(RenameChatParams {
                    chat,
                    title: title.join(" "),
                }),
                ChatAction::Close { chat, force } => {
                    ClientCall::ChatClose(CloseChatParams { chat, force })
                }
                ChatAction::Reopen { chat } => ClientCall::ChatReopen(ChatParams { chat }),
                ChatAction::Fork { chat, turn, agent } => ClientCall::ChatFork(ChatForkParams {
                    chat,
                    turn,
                    agent: agent.map(Into::into),
                }),
                ChatAction::Delete { chat, yes, force } => {
                    if !yes
                        && !confirm(&format!(
                            "Delete chat {chat}? Its conversation is erased; this cannot be undone."
                        ))?
                    {
                        eprintln!("not deleted");
                        return Ok(1);
                    }
                    ClientCall::ChatDelete(CloseChatParams { chat, force })
                }
            };
            let result: ChatInfo = client.call(call).await?;
            print_or(json, &result, || render::chat_line(&result));
        }
        Command::Chat {
            action: None,
            workspace: Some(workspace),
            agent,
            model,
            effort,
            fast,
            permission_mode,
            prompt,
            images,
            design,
            from,
        } => {
            let result: ChatInfo = client
                .call(ClientCall::ChatCreate(CreateChatParams {
                    workspace,
                    agent: agent.map(Into::into),
                    model,
                    effort,
                    fast: fast.then_some(true),
                    permission_mode: permission_mode.map(Into::into),
                    prompt,
                    images: image_inputs(&images)?,
                    title: None,
                    kind: design.then_some(ChatKind::Design),
                    source_chat: from,
                }))
                .await?;
            if result.brief.is_none() {
                print_or(json, &result, || render::chat_line(&result));
                return Ok(0);
            }
            let chat = wait_for_brief(&mut client, &result).await?;
            print_or(json, &chat, || match &chat.brief {
                Some(DesignBrief {
                    state: BriefState::Ready,
                    text: Some(text),
                    ..
                }) => format!("{}\n\n{text}", render::chat_line(&chat)),
                Some(DesignBrief { error, .. }) => format!(
                    "{}\n\nThe brief could not be written: {}",
                    render::chat_line(&chat),
                    error.as_deref().unwrap_or("no reason given")
                ),
                None => render::chat_line(&chat),
            });
            if !matches!(&chat.brief, Some(brief) if brief.state == BriefState::Ready) {
                return Ok(1);
            }
        }
        Command::Chat {
            action: None,
            workspace: None,
            ..
        } => anyhow::bail!("name the workspace to start a chat in"),
        Command::Attach { target, history } => return attach(client, &target, history, json).await,
        Command::Ticket { action } => {
            let (workspace, add, remove) = match action {
                TicketAction::Add {
                    identifiers,
                    workspace,
                } => (workspace, identifiers, Vec::new()),
                TicketAction::Remove {
                    identifiers,
                    workspace,
                } => (workspace, Vec::new(), identifiers),
                TicketAction::Refresh { workspace } => (workspace, Vec::new(), Vec::new()),
            };
            let current: WorkspaceInfo = client
                .call(ClientCall::WorkspaceGet(WorkspaceParams {
                    workspace: workspace.clone(),
                }))
                .await?;
            let tickets = ticket_list(&current, &add, &remove)?;
            let result: SetTicketsResult = client
                .call(ClientCall::WorkspaceSetTickets(SetTicketsParams {
                    workspace,
                    tickets,
                }))
                .await?;
            for warning in &result.warnings {
                eprintln!("warning: {}", render::sanitize_terminal(warning));
            }
            print_or(json, &result, || render::tickets(&result.workspace));
        }
        Command::Worktree { action } => match action {
            WorktreeAction::Add {
                workspace,
                repos,
                name,
                branch,
            } => {
                let result: AddWorktreeResult = client
                    .call(ClientCall::WorktreeAdd(AddWorktreeParams {
                        workspace,
                        repos,
                        name,
                        branch,
                    }))
                    .await?;
                print_or(json, &result, || {
                    let mut out = result.path.clone();
                    for t in &result.worktrees {
                        out.push_str(&format!("\n  {}  {}  [{}]", t.repo, t.path, t.branch));
                    }
                    out
                });
            }
            WorktreeAction::Remove { target, workspace } => {
                let result: RemoveWorktreeResult = client
                    .call(ClientCall::WorktreeRemove(RemoveWorktreeParams {
                        workspace,
                        target,
                    }))
                    .await?;
                print_or(json, &result, || {
                    format!("removed {} worktree(s)", result.removed.len())
                });
            }
        },
        Command::Run {
            workspace,
            repo,
            script,
        } => {
            let result: ProcessInfo = client
                .call(ClientCall::ProcessRun(RunScriptParams {
                    workspace,
                    repo,
                    name: script,
                }))
                .await?;
            print_or(json, &result, || render::process_line(&result));
        }
        Command::Database { action } => database::run(&mut client, action).await?,
        Command::Routine { action } => routine::run(&mut client, action, json).await?,
        Command::Services { workspace, action } => {
            dev_services::run(&mut client, workspace, action, json).await?
        }
        Command::Ps { workspace } => {
            let result: ProcessesResult = client
                .call(ClientCall::ProcessList(WorkspaceParams { workspace }))
                .await?;
            print_or(json, &result, || {
                let mut lines: Vec<String> =
                    result.processes.iter().map(render::process_line).collect();
                if !result.scripts.is_empty() {
                    lines.push(format!("run scripts: {}", result.scripts.join(", ")));
                }
                lines.join("\n")
            });
        }
        Command::Stop { process } => {
            let result: ProcessInfo = client
                .call(ClientCall::ProcessStop(StopProcessParams { process }))
                .await?;
            print_or(json, &result, || render::process_line(&result));
        }
        Command::Diff {
            workspace,
            repo,
            stat,
        } => {
            let result: DiffResult = client
                .call(ClientCall::DiffGet(DiffParams {
                    workspace,
                    repo,
                    stat_only: stat,
                }))
                .await?;
            print_or(json, &result, || {
                let mut out = String::new();
                for repo in &result.repos {
                    out.push_str(&format!(
                        "## {}  {} ← {}  ({} commit(s) ahead)\n",
                        repo.repo, repo.branch, repo.base_branch, repo.ahead
                    ));
                    for file in &repo.files {
                        out.push_str(&format!(
                            "  {:<9} {} +{} -{}\n",
                            format!("{:?}", file.status).to_lowercase(),
                            file.path,
                            file.additions,
                            file.deletions
                        ));
                    }
                    if !stat && !repo.patch.is_empty() {
                        out.push('\n');
                        out.push_str(&repo.patch);
                        out.push('\n');
                    }
                }
                out.trim_end().to_string()
            });
        }
        Command::Files { workspace, root } => {
            let mut result: FilesListResult = client
                .call(ClientCall::FilesList(WorkspaceParams { workspace }))
                .await?;
            if let Some(root) = &root {
                result.roots.retain(|r| &r.name == root);
            }
            print_or(json, &result, || {
                let mut out = String::new();
                for root in &result.roots {
                    for file in &root.files {
                        out.push_str(&format!("{}/{file}\n", root.name));
                    }
                    if root.truncated {
                        out.push_str(&format!("{}/… (list truncated)\n", root.name));
                    }
                }
                out.trim_end().to_string()
            });
        }
        Command::Search {
            workspace,
            query,
            regex,
            case_sensitive,
            root,
            max,
        } => {
            let result: FilesSearchResult = client
                .call(ClientCall::FilesSearch(FilesSearchParams {
                    workspace,
                    query,
                    regex,
                    case_sensitive,
                    root,
                    max_results: max,
                }))
                .await?;
            print_or(json, &result, || {
                let mut out: String = result
                    .matches
                    .iter()
                    .map(|m| format!("{}:{}:{}: {}\n", m.path, m.line, m.column, m.text))
                    .collect();
                if result.truncated {
                    out.push_str("… more matches (use --max)\n");
                }
                out.trim_end().to_string()
            });
        }
        Command::Prs { workspace } => {
            let result: PrStatusResult = client
                .call(ClientCall::PrStatus(WorkspaceParams { workspace }))
                .await?;
            print_or(json, &result, || {
                result
                    .prs
                    .iter()
                    .map(|pr| {
                        format!(
                            "{:<24} {} ← {}  ahead {}{}  {}",
                            match &pr.group {
                                Some(group) => format!("{} ({group})", pr.repo),
                                None => pr.repo.clone(),
                            },
                            pr.branch,
                            pr.base_branch,
                            pr.ahead,
                            if pr.dirty { ", dirty" } else { "" },
                            match (&pr.url, &pr.state) {
                                (Some(url), Some(state)) => format!(
                                    "{state}{} {url}",
                                    pr.checks
                                        .as_ref()
                                        .map(|c| format!(" checks {c}"))
                                        .unwrap_or_default()
                                ),
                                _ => "no PR".to_string(),
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        Command::Merge {
            workspace,
            method,
            prs,
        } => {
            let result: MergePrsResult = client
                .call(ClientCall::PrMerge(MergePrsParams {
                    workspace,
                    method: method.into(),
                    prs,
                }))
                .await?;
            let failed = result.outcomes.iter().any(|o| !o.merged);
            print_or(json, &result, || {
                result
                    .outcomes
                    .iter()
                    .map(|o| match &o.error {
                        None => format!("{:<24} {} merged", o.repo, o.branch),
                        Some(error) => format!("{:<24} {} {error}", o.repo, o.branch),
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            if failed {
                return Ok(1);
            }
        }
        Command::Pr {
            workspace,
            draft,
            instructions,
        } => {
            let result: ChatInfo = client
                .call(ClientCall::PrCreate(CreatePrParams {
                    workspace,
                    draft,
                    instructions,
                }))
                .await?;
            print_or(json, &result, || {
                format!(
                    "asked the agent to open the PRs: {}",
                    render::chat_line(&result)
                )
            });
        }
        Command::OpenIssue { identifier } => {
            let identifier = identifier.to_uppercase();
            let existing: WorkspacesResult = client
                .call(ClientCall::WorkspaceList(ListWorkspacesParams::default()))
                .await?;
            let found = existing.workspaces.into_iter().find(|w| {
                w.parent_id.is_none() && w.tickets.iter().any(|t| t.identifier == identifier)
            });
            let workspace = match found {
                Some(workspace) => workspace,
                None => {
                    let prompt = std::env::var("LINEAR_PROMPT")
                        .ok()
                        .filter(|p| !p.trim().is_empty());
                    let result: CreateWorkspaceResult = client
                        .call(ClientCall::WorkspaceCreate(CreateWorkspaceParams {
                            tickets: vec![identifier],
                            prompt,
                            ..Default::default()
                        }))
                        .await?;
                    result.workspace
                }
            };
            println!("{}", render::workspace_details(&workspace).trim_end());
            // Bring the app forward when it is installed; the CLI output is enough otherwise.
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open")
                .args(["-a", "Chauffeur"])
                .stderr(std::process::Stdio::null())
                .status();
        }
        Command::Office { .. } => {
            let link: chauffeur_protocol::office::OfficeBrowserLink = client
                .call(ClientCall::OfficeOpenInBrowser(EmptyParams {}))
                .await?;
            print_or(json, &link, || {
                format!(
                    "asked your browser to open the Office; if none showed up, open within a minute: {}",
                    link.url
                )
            });
        }
        Command::Power => {
            let state: chauffeur_protocol::model::PowerState =
                client.call(ClientCall::PowerGet(EmptyParams {})).await?;
            print_or(json, &state, || render::power(&state));
        }
        Command::Update { install } => {
            // Asks GitHub now rather than showing the daemon's last look.
            let mut status: UpdateStatus =
                client.call(ClientCall::UpdateCheck(EmptyParams {})).await?;
            if install {
                status = client
                    .call(ClientCall::UpdateInstall(EmptyParams {}))
                    .await?;
            }
            while matches!(
                status.phase,
                UpdatePhase::Downloading | UpdatePhase::Installing
            ) {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                status = client
                    .call(ClientCall::UpdateStatus(EmptyParams {}))
                    .await?;
            }
            print_or(json, &status, || render::update(&status));
            if status.phase == UpdatePhase::Failed {
                std::process::exit(1);
            }
        }
        Command::Env {
            action: EnvAction::Reload,
        } => {
            let result: chauffeur_protocol::client::EnvReloadResult =
                client.call(ClientCall::EnvReload(EmptyParams {})).await?;
            print_or(json, &result, || render::env_reload(&result));
        }
        Command::Login { agent } => login(&mut client, agent.into(), json).await?,
        Command::Usage { refresh } => {
            let result: UsageResult = client
                .call(ClientCall::UsageGet(ModelsListParams {
                    refresh,
                    workspace: None,
                }))
                .await?;
            print_or(json, &result, || {
                result
                    .agents
                    .iter()
                    .map(|usage| {
                        let name = match usage.agent {
                            AgentKind::Claude => "Claude Code",
                            AgentKind::Codex => "Codex",
                        };
                        let windows = usage
                            .windows
                            .iter()
                            .map(|w| format!("{} {:.0}%", w.label, w.used_percent))
                            .collect::<Vec<_>>()
                            .join("  ");
                        let detail = match (&usage.error, windows.is_empty()) {
                            _ if usage.account == AccountStatus::SignedOut => format!(
                                "signed out ({}): `chauffeur login {}`",
                                usage.account_error.as_deref().unwrap_or("logged out"),
                                usage.agent.as_str()
                            ),
                            (Some(error), true) => error.clone(),
                            (_, true) => "no plan limits".into(),
                            (_, false) => windows,
                        };
                        format!(
                            "{:<12} {:<9} {detail}",
                            name,
                            usage.plan.as_deref().unwrap_or("-")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        Command::Github { action } => match action.unwrap_or(GithubAction::Status) {
            GithubAction::Status => {
                let status: GithubStatus = client
                    .call(ClientCall::GithubStatus(EmptyParams {}))
                    .await?;
                print_or(json, &status, || render::github(&status));
            }
            GithubAction::Login => {
                let _: EmptyParams = client
                    .call(ClientCall::GithubConnect(EmptyParams {}))
                    .await?;
                let mut shown = String::new();
                let mut opened = false;
                let status = loop {
                    let status: GithubStatus = client
                        .call(ClientCall::GithubStatus(EmptyParams {}))
                        .await?;
                    let Some(step) = status.connecting.as_ref().filter(|s| s.error.is_none())
                    else {
                        break status;
                    };
                    let line = render::github_connecting(step);
                    if !json && line != shown {
                        eprintln!("{}", render::sanitize_terminal(&line));
                        shown = line;
                    }
                    if let (Some(_), Some(url), false) = (&step.code, &step.url, opened) {
                        opened = true;
                        open_in_browser(url);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                };
                if let Some(error) = status
                    .connecting
                    .as_ref()
                    .and_then(|step| step.error.as_deref())
                {
                    anyhow::bail!("could not connect GitHub: {error}");
                }
                print_or(json, &status, || render::github(&status));
            }
            GithubAction::Logout => {
                let done: EmptyParams = client
                    .call(ClientCall::GithubDisconnect(EmptyParams {}))
                    .await?;
                print_or(json, &done, || "GitHub is disconnected".into());
            }
        },
        Command::Ollama { action } => match action.unwrap_or(OllamaAction::Status) {
            OllamaAction::Status => {
                let status: OllamaStatus = client
                    .call(ClientCall::OllamaStatus(EmptyParams {}))
                    .await?;
                print_or(json, &status, || render::ollama(&status));
            }
            OllamaAction::Install => {
                let _: EmptyParams = client
                    .call(ClientCall::OllamaInstall(EmptyParams {}))
                    .await?;
                let mut shown = String::new();
                let status = loop {
                    let status: OllamaStatus = client
                        .call(ClientCall::OllamaStatus(EmptyParams {}))
                        .await?;
                    match &status.installing {
                        Some(step) if step.error.is_none() => {
                            let line = render::ollama_installing(step);
                            if !json && line != shown {
                                eprintln!("{line}");
                                shown = line;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        }
                        _ => break status,
                    }
                };
                if let Some(error) = status
                    .installing
                    .as_ref()
                    .and_then(|step| step.error.as_deref())
                {
                    anyhow::bail!("could not install Ollama: {error}");
                }
                print_or(json, &status, || render::ollama(&status));
            }
            OllamaAction::Pull { model } => {
                let _: EmptyParams = client
                    .call(ClientCall::OllamaPull(OllamaModelParams {
                        model: model.clone(),
                    }))
                    .await?;
                let mut shown = String::new();
                let status = loop {
                    let status: OllamaStatus = client
                        .call(ClientCall::OllamaStatus(EmptyParams {}))
                        .await?;
                    match status.pulls.iter().find(|pull| pull.model == model) {
                        Some(pull) if pull.error.is_none() => {
                            let line = render::ollama_pull(pull);
                            if !json && line != shown {
                                eprintln!("{line}");
                                shown = line;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        }
                        _ => break status,
                    }
                };
                if let Some(error) = status
                    .pulls
                    .iter()
                    .find(|pull| pull.model == model)
                    .and_then(|pull| pull.error.as_deref())
                {
                    anyhow::bail!("could not download {model}: {error}");
                }
                print_or(json, &status, || {
                    format!("{model} is downloaded: pick it in a Codex chat's model picker")
                });
            }
            OllamaAction::Rm { model } => {
                let deleted: EmptyParams = client
                    .call(ClientCall::OllamaDelete(OllamaModelParams {
                        model: model.clone(),
                    }))
                    .await?;
                print_or(json, &deleted, || format!("{model} is deleted"));
            }
        },
        Command::Jev { action } => {
            let update = match action.unwrap_or(JevAction::Status) {
                JevAction::Status => None,
                JevAction::On => Some(JevUpdate {
                    enabled: Some(true),
                    ..Default::default()
                }),
                JevAction::Off => Some(JevUpdate {
                    enabled: Some(false),
                    ..Default::default()
                }),
                JevAction::Threshold { percent } => Some(JevUpdate {
                    threshold_percent: Some(percent),
                    ..Default::default()
                }),
                JevAction::Key { remove } => {
                    let key = if remove {
                        String::new()
                    } else {
                        let key = linear_key::read_hidden("TypeSafe API key: ")?;
                        anyhow::ensure!(!key.is_empty(), "no key given");
                        key
                    };
                    Some(JevUpdate {
                        api_key: Some(key),
                        ..Default::default()
                    })
                }
            };
            let result: AppSettings = match update {
                None => client.call(ClientCall::SettingsGet(EmptyParams {})).await?,
                Some(jev) => {
                    client
                        .call(ClientCall::SettingsSet(Box::new(SettingsUpdate {
                            jev: Some(jev),
                            ..Default::default()
                        })))
                        .await?
                }
            };
            print_or(json, &result, || render::jev(&result.jev));
        }
        Command::Linear { action } => match action {
            LinearAction::InstallTool => unreachable!(),
            LinearAction::Login => {
                println!(
                    "Paste a personal Linear API key (Linear → Settings → Security & access → API keys)."
                );
                let key = linear_key::read_hidden("API key: ")?;
                anyhow::ensure!(!key.is_empty(), "no key given");
                let _: AppSettings = client
                    .call(ClientCall::SettingsSet(Box::new(SettingsUpdate {
                        linear: Some(LinearUpdate {
                            api_key: Some(key),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })))
                    .await?;
                println!("Saved: Chauffeur reads Linear with it from now on.");
            }
            LinearAction::Search { query } => {
                let query = query.join(" ");
                let result: LinearSearchResult = client
                    .call(ClientCall::LinearSearch(LinearSearchParams {
                        mine: query.is_empty(),
                        query: Some(query).filter(|q| !q.is_empty()),
                        limit: None,
                        milestones: None,
                    }))
                    .await?;
                print_or(json, &result, || {
                    result
                        .issues
                        .iter()
                        .map(|i| {
                            format!(
                                "{:<10} {:<14} {}{}",
                                i.identifier,
                                i.state.clone().unwrap_or_default(),
                                i.title,
                                if i.repos.is_empty() {
                                    String::new()
                                } else {
                                    format!("  [{}]", i.repos.join(", "))
                                }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                });
            }
            LinearAction::Show { identifier } => {
                let issue: LinearIssue = client
                    .call(ClientCall::LinearIssue(LinearIssueParams { identifier }))
                    .await?;
                print_or(json, &issue, || {
                    format!(
                        "{} — {}\n{}\nbranch {}\nrepos {}\n\n{}",
                        issue.identifier,
                        issue.title,
                        issue.url,
                        issue.branch_name.clone().unwrap_or_default(),
                        issue.repos.join(", "),
                        issue.description.clone().unwrap_or_default()
                    )
                });
            }
        },
        Command::Daemon { .. }
        | Command::Doctor
        | Command::HookRun { .. }
        | Command::GitContext => unreachable!(),
    }
    Ok(0)
}

/// One line per prerequisite; exit code 1 when one that matters is missing.
async fn doctor() -> anyhow::Result<i32> {
    let run = |program: &str, args: &[&str]| -> Option<String> {
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let mut failed = false;
    let mut check = |ok: bool, required: bool, what: &str, detail: String| {
        let mark = if ok {
            "ok  "
        } else if required {
            "FAIL"
        } else {
            "warn"
        };
        println!("{mark}  {what}: {detail}");
        failed |= !ok && required;
    };
    let git = run("git", &["--version"]);
    check(
        git.is_some(),
        true,
        "git",
        git.unwrap_or("not found".into()),
    );
    // The user's gh, else the copy `chauffeur github login` installs.
    let own_gh = Home::from_env().home.join("bin").join("gh");
    let gh = run("gh", &["auth", "status", "--active"])
        .or_else(|| run(&own_gh.to_string_lossy(), &["auth", "status", "--active"]))
        .is_some();
    check(
        gh,
        true,
        "gh",
        if gh {
            "logged in".into()
        } else {
            "run `chauffeur github login`".into()
        },
    );
    let claude = run("claude", &["--version"]);
    check(
        claude.is_some(),
        false,
        "claude",
        claude.unwrap_or(
            "not on PATH (the sidecar embeds its own; log in once with `claude`)".into(),
        ),
    );
    let codex = run("codex", &["--version"]);
    check(
        codex.is_some(),
        false,
        "codex",
        codex.unwrap_or("not found (Codex chats unavailable)".into()),
    );
    let docker = run("docker", &["info", "--format", "{{.ServerVersion}}"]);
    check(
        docker.is_some(),
        false,
        "docker",
        docker
            .map(|v| format!("engine {v}"))
            .unwrap_or("not running (archive cannot stop compose stacks)".into()),
    );
    #[cfg(target_os = "linux")]
    {
        for (program, args, remedy) in [
            (
                "bwrap",
                vec!["--version"],
                "install bubblewrap for the agent sandbox",
            ),
            (
                "socat",
                vec!["-V"],
                "install socat for sandbox communication",
            ),
            (
                "xdg-open",
                vec!["--version"],
                "install xdg-utils to open browsers and files",
            ),
        ] {
            let found = run(program, &args);
            check(
                found.is_some(),
                false,
                program,
                found
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| remedy.into()),
            );
        }
        let session = run("systemctl", &["--user", "show-environment"]);
        check(
            session.is_some(),
            false,
            "user service manager",
            if session.is_some() {
                "reachable; use `chauffeur daemon install` to start Chauffeur at login".into()
            } else {
                "no systemd user session; the desktop can still start the daemon on demand".into()
            },
        );
        let secrets = run("busctl", &["--user", "--no-pager", "list"])
            .is_some_and(|services| services.contains("org.freedesktop.secrets"));
        check(
            secrets,
            false,
            "Secret Service",
            if secrets {
                "available; unlock your desktop keyring if a login cannot be saved".into()
            } else {
                "start and unlock GNOME Keyring or another Secret Service provider to save logins"
                    .into()
            },
        );
        let display = std::env::var_os("WAYLAND_DISPLAY").or_else(|| std::env::var_os("DISPLAY"));
        check(
            display.is_some(),
            false,
            "desktop session",
            display
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| "no graphical session; the CLI remains available".into()),
        );
        check(
            true,
            false,
            "Office calls",
            "open Office in a browser when WebKitGTK lacks WebRTC".into(),
        );
    }
    let linear = std::env::var("CHAUFFEUR_LINEAR_API_KEY").is_ok() || linear_key::stored();
    check(
        linear,
        false,
        "linear",
        if linear {
            "API key found".into()
        } else {
            "run `chauffeur linear login`".into()
        },
    );
    let exe_dir = std::env::current_exe()?
        .canonicalize()?
        .parent()
        .map(PathBuf::from)
        .unwrap_or_default();
    let sidecar = [exe_dir.join("chauffeur-sidecar")]
        .into_iter()
        .find(|p| p.exists());
    check(
        sidecar.is_some() || std::env::var_os("CHAUFFEUR_SIDECAR").is_some(),
        false,
        "sidecar",
        sidecar.map(|p| p.display().to_string()).unwrap_or(
            "not next to the CLI; the daemon falls back to the source tree's sidecar/dist".into(),
        ),
    );
    match Client::connect(false).await {
        Ok(_) => check(true, false, "daemon", "running".into()),
        Err(_) => check(
            false,
            false,
            "daemon",
            "not running (starts on first use)".into(),
        ),
    }
    Ok(i32::from(failed))
}

/// Points Linear's custom coding tool at `chauffeur open-issue`, keeping the rest of the file.
fn linear_install_tool() -> anyhow::Result<i32> {
    let path = PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join(".linear/coding-tools.json");
    let mut config: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if path.exists() {
        std::fs::copy(&path, path.with_extension("json.bak"))?;
    }
    let exe = std::env::current_exe()?.canonicalize()?;
    config["openIssue"] = serde_json::json!({
        "path": exe,
        "args": ["open-issue", "{{issue.identifier}}"],
        "env": ["LINEAR_PROMPT", "LINEAR_ISSUE_BRANCH_NAME"],
    });
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, serde_json::to_string_pretty(&config)? + "\n")?;
    println!(
        "{} now opens issues in Chauffeur. In Linear: Settings → Code & reviews → Configure coding tools → Custom script, then \"Work on issue → Custom script\".",
        path.display()
    );
    Ok(0)
}

/// In the daemon's order, each workspace whose parent is listed right after that parent.
fn ordered(workspaces: &[WorkspaceInfo]) -> Vec<&WorkspaceInfo> {
    let listed = |id: &str| workspaces.iter().any(|w| w.id == id);
    let mut out = Vec::new();
    for top in workspaces
        .iter()
        .filter(|w| w.parent_id.as_deref().is_none_or(|p| !listed(p)))
    {
        out.push(top);
        out.extend(
            workspaces
                .iter()
                .filter(|w| w.parent_id.as_deref() == Some(top.id.as_str())),
        );
    }
    out
}

/// `chauffeur ls`: as the sidebar shows them, under a header per group (pinned, each section,
/// the others, archived) once there are sections or pinned workspaces; children under their
/// parent, in its group.
fn ls_text(result: &WorkspacesResult, grouped: bool) -> String {
    let rows = ordered(&result.workspaces);
    let lines = |rows: &[&WorkspaceInfo]| -> Vec<String> {
        rows.iter().map(|w| render::workspace_line(w)).collect()
    };
    if !grouped || (result.sections.is_empty() && !rows.iter().any(|w| w.pinned)) {
        return lines(&rows).join("\n");
    }
    // Each row in the group of the top-level workspace it comes under.
    #[derive(PartialEq)]
    enum Group<'a> {
        Pinned,
        Section(&'a str),
        Others,
        Archived,
    }
    let mut placed: Vec<(Group, &WorkspaceInfo)> = Vec::new();
    for w in &rows {
        let top = w
            .parent_id
            .as_deref()
            .and_then(|p| rows.iter().find(|r| r.id == p))
            .unwrap_or(w);
        let group = if top.state == chauffeur_protocol::model::WorkspaceState::Archived {
            Group::Archived
        } else if top.pinned {
            Group::Pinned
        } else if let Some(section) = top.section_id.as_deref() {
            Group::Section(section)
        } else {
            Group::Others
        };
        placed.push((group, w));
    }
    let of = |group: &Group| -> Vec<&WorkspaceInfo> {
        placed
            .iter()
            .filter(|(g, _)| g == group)
            .map(|(_, w)| *w)
            .collect()
    };
    let mut out: Vec<String> = Vec::new();
    let mut block = |header: String, rows: Vec<&WorkspaceInfo>| {
        if !out.is_empty() {
            out.push(String::new());
        }
        out.push(header);
        out.extend(lines(&rows));
    };
    let pinned = of(&Group::Pinned);
    if !pinned.is_empty() {
        block("── Pinned".into(), pinned);
    }
    for section in &result.sections {
        let rows = of(&Group::Section(&section.id));
        block(format!("── {} ({})", section.name, rows.len()), rows);
    }
    let others = of(&Group::Others);
    if !others.is_empty() {
        block("── Workspaces".into(), others);
    }
    let archived = of(&Group::Archived);
    if !archived.is_empty() {
        block("── Archived".into(), archived);
    }
    out.join("\n")
}

fn wait_text(result: &WaitResult) -> String {
    let mut out = render::workspace_line(&result.workspace);
    for chat in &result.chats {
        out.push_str(&format!("\n  {}", render::chat_line(chat)));
    }
    if result.timed_out {
        out.push_str("\n(timed out: still working)");
    }
    for request in &result.open_requests {
        out.push_str(&format!("\n\n{}", render::open_request(request)));
    }
    if let Some(text) = &result.last_text {
        out.push_str(&format!("\n\n{text}"));
    }
    out
}

/// `alpha:jc/abc-1`: a PR named by its repo and head branch.
/// Starts an agent's login, prints the page to open, and waits for the daemon to tell the agent
/// is signed in (or the sidecar gave up on the login).
async fn login(client: &mut Client, agent: AgentKind, json: bool) -> anyhow::Result<()> {
    use chauffeur_protocol::events::{Event, RegistryEvent};
    use chauffeur_protocol::sidecar::AgentLoginResult;
    let _: SubscribeResult = client
        .call(ClientCall::EventsSubscribe(SubscribeParams {
            registry: true,
            ..Default::default()
        }))
        .await?;
    let started: AgentLoginResult = client
        .call(ClientCall::AgentLogin(AgentLoginParams {
            agent,
            headless: false,
        }))
        .await?;
    if json {
        println!("{}", serde_json::to_string(&started)?);
    } else {
        println!("Open this page to sign in:\n{}", started.url);
    }
    let wait = async {
        loop {
            let notification = client.next_event().await?;
            match notification.envelope.event {
                Event::Registry {
                    event:
                        RegistryEvent::AccountChanged {
                            agent: changed,
                            account: AccountStatus::SignedIn,
                            ..
                        },
                } if changed == agent => return Ok::<_, anyhow::Error>(()),
                Event::Registry {
                    event:
                        RegistryEvent::LoginEnded {
                            agent: ended,
                            worker: None,
                            success: false,
                            error,
                        },
                } if ended == agent => anyhow::bail!(
                    "the login did not complete: {}",
                    error.as_deref().unwrap_or("no reason given")
                ),
                _ => {}
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(600), wait)
        .await
        .map_err(|_| anyhow::anyhow!("the login did not complete in 10 minutes"))??;
    if !json {
        println!("Signed in.");
    }
    Ok(())
}

fn parse_pr_ref(value: &str) -> Result<PrRef, String> {
    match value.split_once(':') {
        Some((repo, branch)) if !repo.is_empty() && !branch.is_empty() => Ok(PrRef {
            repo: repo.into(),
            branch: branch.into(),
        }),
        _ => Err("expected REPO:BRANCH, as `chauffeur prs` lists them".into()),
    }
}

/// The value as JSON, or the text for a terminal: the text can carry what an agent or a ticket
/// said, so its escape sequences are dropped (serde escapes them in the JSON).
/// A workspace's ticket identifiers with `add` appended and `remove` taken out (any case).
fn ticket_list(
    workspace: &WorkspaceInfo,
    add: &[String],
    remove: &[String],
) -> anyhow::Result<Vec<String>> {
    let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    let mut tickets: Vec<String> = workspace
        .tickets
        .iter()
        .map(|t| t.identifier.clone())
        .collect();
    for identifier in remove {
        if !tickets.iter().any(|t| same(t, identifier)) {
            anyhow::bail!("{} has no ticket {identifier}", workspace.slug);
        }
        tickets.retain(|t| !same(t, identifier));
    }
    for identifier in add {
        if !tickets.iter().any(|t| same(t, identifier)) {
            tickets.push(identifier.trim().to_string());
        }
    }
    Ok(tickets)
}

/// Asks a yes/no question on the terminal; refuses (rather than guesses) when nobody is there to
/// answer.
fn confirm(question: &str) -> anyhow::Result<bool> {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("{question} Pass --yes to go ahead without being asked.");
    }
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes"))
}

/// A new design chat once its brief is written, or could not be (the daemon gives up first).
async fn wait_for_brief(client: &mut Client, chat: &ChatInfo) -> anyhow::Result<ChatInfo> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    loop {
        let chats: ChatsResult = client
            .call(ClientCall::ChatList(ChatListParams {
                workspace: chat.workspace_id.clone(),
                all: true,
            }))
            .await?;
        let current = chats
            .chats
            .into_iter()
            .find(|c| c.id == chat.id)
            .ok_or_else(|| anyhow::anyhow!("the chat {} is gone", chat.id))?;
        let writing = current
            .brief
            .as_ref()
            .is_some_and(|brief| brief.state == BriefState::Writing);
        if !writing || std::time::Instant::now() > deadline {
            return Ok(current);
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Opens a page in the user's browser; the page's address is printed beside it in case it fails.
fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn print_or<T: serde::Serialize>(json: bool, value: &T, text: impl FnOnce() -> String) {
    if json {
        println!("{}", serde_json::to_string_pretty(value).unwrap());
    } else {
        println!("{}", render::sanitize_terminal(&text()));
    }
}

async fn attach(
    mut client: Client,
    target: &str,
    history: bool,
    json: bool,
) -> anyhow::Result<i32> {
    // A chat id, or a workspace (then every chat of it plus its scripts).
    let workspace: Result<WorkspaceInfo, _> = client
        .call(ClientCall::WorkspaceGet(WorkspaceParams {
            workspace: target.to_string(),
        }))
        .await;
    let mut params = SubscribeParams {
        ephemeral: true,
        ..Default::default()
    };
    let hint = match &workspace {
        Ok(w) => {
            params.workspaces.push(w.id.clone());
            w.slug.clone()
        }
        Err(_) => {
            params.chats.push(target.to_string());
            target.to_string()
        }
    };
    if history {
        params.after_seq = Some(0);
    }
    let _: SubscribeResult = client.call(ClientCall::EventsSubscribe(params)).await?;
    // On standard error, with --json too: a script waits for it before acting.
    eprintln!(
        "following {} — Ctrl-C to stop",
        render::sanitize_terminal(&hint)
    );
    let mut streamed_text = false;
    let mut renderer = render::EventRenderer::default();
    loop {
        let notification = client.next_event().await?;
        let envelope = notification.envelope;
        if json {
            println!("{}", serde_json::to_string(&envelope)?);
            continue;
        }
        use chauffeur_protocol::events::{ChatEvent, Event};
        match &envelope.event {
            Event::Chat {
                event:
                    ChatEvent::TextDelta {
                        text,
                        parent_tool_use_id: None,
                        ..
                    },
                ..
            } => {
                use std::io::Write;
                if !streamed_text {
                    println!();
                }
                print!("{}", render::sanitize_terminal(text));
                let _ = std::io::stdout().flush();
                streamed_text = true;
            }
            Event::Chat {
                event:
                    ChatEvent::AssistantText {
                        parent_tool_use_id: None,
                        ..
                    },
                ..
            } if streamed_text => {
                // Already streamed: just end the line.
                println!();
                streamed_text = false;
            }
            _ => {
                if let Some(line) = renderer.event(&envelope, &hint) {
                    if streamed_text {
                        println!();
                        streamed_text = false;
                    }
                    println!("{line}");
                }
            }
        }
    }
}

async fn daemon(action: DaemonAction) -> anyhow::Result<i32> {
    let home = Home::from_env();
    let stopping = match action {
        DaemonAction::Stop { real_daemon: false } => Some(daemon_guard::Stopping::Stop),
        DaemonAction::Install { real_daemon: false } => Some(daemon_guard::Stopping::Install),
        DaemonAction::Uninstall { real_daemon: false } => Some(daemon_guard::Stopping::Uninstall),
        _ => None,
    };
    if let Some(stopping) = stopping {
        daemon_guard::check(
            stopping,
            &home.home,
            &daemon_guard::default_homes(),
            |name| std::env::var_os(name),
        )?;
    }
    match action {
        DaemonAction::Start => match Client::connect(false).await {
            Ok(_) => println!("chauffeurd is already running"),
            // `connect` would refuse too, saying only that the daemon is not running.
            Err(_) if std::env::var_os("CHAUFFEUR_WORKSPACE_ID").is_some() => {
                anyhow::bail!("{}", daemon_guard::start_refusal(&home.home))
            }
            Err(_) => {
                Client::connect(true).await?;
                println!("chauffeurd started (log: {})", home.log().display());
            }
        },
        DaemonAction::Stop { .. } => match service::stop_daemon(&home).await {
            true => println!("chauffeurd stopped"),
            false => println!("chauffeurd is not running"),
        },
        DaemonAction::Status => match Client::connect(false).await {
            Ok(mut client) => {
                let info: WorkspacesResult = client
                    .call(ClientCall::WorkspaceList(ListWorkspacesParams::default()))
                    .await?;
                let daemon_info =
                    std::fs::read_to_string(home.home.join("daemon.json")).unwrap_or_default();
                println!(
                    "chauffeurd is running: {} active workspace(s)",
                    info.workspaces.len()
                );
                if !daemon_info.is_empty() {
                    println!("{daemon_info}");
                }
            }
            Err(_) => {
                println!("chauffeurd is not running");
                return Ok(1);
            }
        },
        DaemonAction::Logs => println!("{}", home.log().display()),
        DaemonAction::Install { .. } => service::install(&home).await?,
        DaemonAction::Uninstall { .. } => service::uninstall(&home).await?,
    }
    Ok(0)
}

/// Images and files given on the command line: the daemon copies them from their absolute path.
fn image_inputs(paths: &[PathBuf]) -> anyhow::Result<Option<Vec<ImageInput>>> {
    if paths.is_empty() {
        return Ok(None);
    }
    paths
        .iter()
        .map(|path| {
            let path = std::fs::canonicalize(path)
                .with_context(|| format!("reading {}", path.display()))?;
            Ok(ImageInput {
                path: Some(path.to_string_lossy().into_owned()),
                ..Default::default()
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()
        .map(Some)
}

/// The guests of `share-workspace`: the viewers, then those who may suggest; someone named in
/// both may suggest.
fn guests_of(viewers: Vec<String>, suggesting: Vec<String>) -> Vec<Guest> {
    let mut guests: Vec<Guest> = Vec::new();
    let named = viewers
        .into_iter()
        .map(|email| (email, GuestRole::Viewer))
        .chain(
            suggesting
                .into_iter()
                .map(|email| (email, GuestRole::Suggest)),
        );
    for (email, role) in named {
        let email = email.trim().to_lowercase();
        if email.is_empty() {
            continue;
        }
        match guests.iter_mut().find(|g| g.email == email) {
            Some(guest) if role == GuestRole::Suggest => guest.role = role,
            Some(_) => {}
            None => guests.push(Guest { email, role }),
        }
    }
    guests
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_failure_keeps_the_daemon_s_code_and_data_and_exits_by_its_kind() {
        use chauffeur_protocol::rpc::{RpcError, codes};
        let refused = anyhow::Error::new(client::RpcFailure(RpcError {
            code: codes::CONFLICT,
            message: "it runs on vm".into(),
            data: Some(serde_json::json!({"machine": "vm"})),
            failure: None,
        }))
        .context("moving it");
        let (failure, exit) = json_failure(&refused);
        assert_eq!(exit, 4);
        assert_eq!(
            failure,
            serde_json::json!({"error": {"code": 409, "message": "moving it: it runs on vm", "data": {"machine": "vm"}}})
        );
        let (failure, exit) = json_failure(&anyhow::anyhow!("the daemon closed the connection"));
        assert_eq!(exit, 1);
        assert_eq!(failure["error"]["code"], Value::Null);
    }

    #[test]
    fn someone_named_to_view_and_to_suggest_may_suggest_once() {
        let guests = guests_of(
            vec![
                "Bob@example.com".into(),
                "carol@example.com".into(),
                " ".into(),
            ],
            vec!["bob@example.com".into()],
        );
        assert_eq!(
            guests,
            vec![
                Guest {
                    email: "bob@example.com".into(),
                    role: GuestRole::Suggest
                },
                Guest {
                    email: "carol@example.com".into(),
                    role: GuestRole::Viewer
                },
            ]
        );
    }

    #[test]
    fn workspaces_are_listed_parents_first() {
        let make = |id: &str, parent: Option<&str>| WorkspaceInfo {
            id: id.into(),
            slug: id.into(),
            title: None,
            path: String::new(),
            branch: String::new(),
            parent_id: parent.map(String::from),
            state: chauffeur_protocol::model::WorkspaceState::Ready,
            activity: chauffeur_protocol::model::Activity::Idle,
            explore: false,
            daily_review_date: None,
            tickets: vec![],
            worktrees: vec![],
            port_base: 0,
            port_count: 0,
            created_at: String::new(),
            archived_at: None,
            last_error: None,
            unread: false,
            pinned: false,
            position: 0.0,
            house_position: None,
            running_tasks: 0,
            section_id: None,
            section: None,
            moved_to: None,
            idle_since: None,
            backup: None,
        };
        let list = vec![
            make("child", Some("p")),
            make("other", None),
            make("p", None),
        ];
        let order: Vec<&str> = ordered(&list).iter().map(|w| w.slug.as_str()).collect();
        assert_eq!(order, vec!["other", "p", "child"]);
    }
}
