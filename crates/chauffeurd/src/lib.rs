//! The Chauffeur daemon: sole owner of workspaces, worktrees, processes and agent sessions.
//! Clients (CLI, app) talk to it over JSON-RPC; see `chauffeur-protocol`.

/// This build: `scripts/package.sh` sets a unique `CHAUFFEUR_BUILD` (version, commit, date) so that
/// an updated app replaces the daemon even when the version number did not change.
pub const BUILD: &str = match option_env!("CHAUFFEUR_BUILD") {
    Some(build) => build,
    None => env!("CARGO_PKG_VERSION"),
};

pub mod access;
pub mod attachments;
pub mod canvas;
pub mod chat_sends;
pub mod chat_shell;
pub mod chats;
pub mod claude_artifact;
pub mod cloud_backup_recovery;
pub mod cloud_process;
pub mod cloud_transfer;
pub mod cloud_transfer_driver;
pub mod cloud_vault;
pub mod cloud_vault_keys;
pub mod cloud_workspace_archive;
pub mod cloud_workspace_artifact;
pub mod cloud_workspace_backup;
pub mod cloud_workspace_data;
pub mod cloud_workspace_discard;
pub mod cloud_workspace_handoff;
pub mod cloud_workspace_package;
pub mod cloud_workspace_preparation;
pub mod cloud_workspace_preparation_keys;
mod cloud_workspace_profile;
pub mod cloud_workspace_restore;
mod cloud_workspace_return;
pub mod cloud_workspace_snapshot;
pub mod comments;
pub mod conductor;
pub mod daily_review;
pub mod design;
pub mod design_export;
pub mod diff;
pub mod downloads;
pub mod envreload;
pub mod error;
pub mod error_reports;
pub mod events;
pub mod explore_tmp;
pub mod extensions;
pub mod file_comments;
pub mod files;
pub mod find_routines;
pub mod genconfig;
pub mod git;
pub mod github;
pub mod google;
pub mod identity;
pub mod idle;
pub mod idle_reminders;
pub mod jev;
pub mod linear;
pub mod linked_files;
pub mod login;
pub mod mentions;
pub mod mobile;
pub mod models;
pub mod news;
pub mod ollama;
pub mod open;
pub mod palette;
pub mod paths;
pub mod peer;
pub mod plan_progress;
pub mod ports;
pub mod power;
pub mod preferences;
pub mod procs;
pub mod prs;
pub mod relay;
pub mod relay_events;
pub mod relay_ports;
pub mod remote;
pub mod repos;
pub mod review_wake;
pub mod routines;
pub mod rpc;
pub mod runtime;
pub mod search;
pub mod secrets;
pub mod sections;
pub mod server;
pub mod settings;
pub mod share;
pub mod shellenv;
pub mod shipped;
pub mod sidecar;
pub mod slack;
pub mod slug;
pub mod sound;
pub mod store;
pub mod terminals;
pub mod transfer_controller;
pub mod update;
pub mod usage;
mod worker_claude_config;
mod worker_credentials;
pub mod worker_lifecycle;
pub mod worker_login;
pub mod worker_ssh;
mod worker_toolchains;
pub mod workers;
pub mod workspace_containers;
mod workspace_database_restore;
pub mod workspace_dev_services;
pub mod workspace_development_profile;
mod workspace_docker_command;
pub mod workspace_execution;
pub mod workspace_launches;
pub mod workspace_postgres_export;
pub mod workspace_postgres_quiescence;
pub mod workspace_postgres_restore;
pub mod workspace_previews;
pub mod workspace_processes;
pub mod workspace_service_provision;
pub mod workspace_service_stop;
pub mod workspace_share;
pub mod workspace_source_stop;
pub mod workspaces;
pub mod worktree_sync;

use std::sync::Arc;

pub use error::{Error, Result};

/// Everything a request handler can reach.
pub struct Daemon {
    pub extensions: extensions::Extensions,
    /// Installed extension UIs over HTTP, and the Office's browser gateway.
    pub extension_web: extensions::web::ExtensionWeb,
    pub paths: paths::Paths,
    pub settings: Arc<settings::LiveSettings>,
    pub(crate) review_settings_lock: tokio::sync::Mutex<()>,
    pub store: store::Store,
    pub remote: remote::RemoteTrust,
    pub mobile: mobile::MobileHost,
    pub slack: slack::Slack,
    pub cloud_vault: cloud_vault::CloudVaultHeads,
    pub cloud_vault_keys: cloud_vault_keys::CloudVaultKeys,
    pub preparation_keys: cloud_workspace_preparation_keys::PreparationKeys,
    pub execution: workspace_execution::ExecutionGate,
    /// What tells a source workspace's processes from the user's; None when this daemon cannot.
    pub process_owner: std::sync::Mutex<Option<workspace_processes::Owner>>,
    pub transfers: cloud_transfer_driver::Transfers,
    /// Calls about the chats of workspaces that run on another machine, sent there.
    pub relay: relay::Relay,
    pub workers: workers::Workers,
    pub transfer_controller: transfer_controller::TransferController,
    pub idle_reminders: idle_reminders::IdleReminders,
    /// Encrypted backups of workspaces running here on a worker, and the keys of those moved to one.
    pub backups: cloud_workspace_backup::Backups,
    pub transfer_captures: cloud_transfer::Captures,
    pub events: events::EventBus,
    pub repos: repos::RepoRegistry,
    pub procs: procs::ProcessManager,
    pub workspaces: workspaces::WorkspaceManager,
    pub chats: chats::ChatManager,
    pub terminals: terminals::TerminalManager,
    pub linear: linear::Linear,
    pub news: news::News,
    pub jev: jev::Jev,
    pub models: models::ModelCatalog,
    /// Ollama, which serves the local models (story 52).
    pub ollama: ollama::Ollama,
    /// GitHub through `gh`, Chauffeur's own when the user has none.
    pub github: github::Github,
    /// Search of every workspace, by keywords and by meaning.
    pub search: search::Search,
    pub usage: usage::Usage,
    pub updates: update::Updates,
    pub power: power::Power,
    pub shell_env: shellenv::ShellEnv,
    /// Where design chats' screens are served (story 47).
    pub designs: canvas::DesignServer,
    pub downloads: downloads::Downloads,
    /// Design shares (story 51).
    pub shares: Arc<share::Shares>,
    /// Workspace shares (story 53).
    pub workspace_shares: Arc<workspace_share::WorkspaceShares>,
    /// The user's mentions in shares' comments.
    pub mentions: Arc<mentions::Mentions>,
    /// Reports of the app's error toasts to Chauffeur's Sentry.
    pub error_reports: Arc<error_reports::ErrorReports>,
    pub shutdown: tokio_util::sync::CancellationToken,
}

impl Daemon {
    /// Opens the store and wires the managers. Nothing is spawned until used.
    pub fn open(paths: paths::Paths) -> anyhow::Result<Arc<Daemon>> {
        Self::open_with_extensions(paths, Vec::new())
    }

    /// Opens Core with the official runtimes selected by the distribution.
    pub fn open_with_extensions(
        paths: paths::Paths,
        extensions: Vec<extensions::Registration>,
    ) -> anyhow::Result<Arc<Daemon>> {
        extensions::validate(&extensions)?;
        paths.ensure()?;
        let settings = Arc::new(settings::LiveSettings::load(&paths)?);
        let store = store::Store::open(&paths.db)?;
        let events = events::EventBus::new(store.clone());
        let shell_env = shellenv::ShellEnv::new();
        shell_env.use_tools(github::tools_dir(&paths.home));
        let execution = workspace_execution::ExecutionGate::new(store.clone());
        let shutdown = tokio_util::sync::CancellationToken::new();
        let error_reports = Arc::new(error_reports::ErrorReports::new(
            &paths.home,
            error_reports::compiled_dsn(),
        ));
        let daemon = Arc::new_cyclic(|weak| Daemon {
            extensions: extensions::Extensions::new(
                extensions,
                weak.clone(),
                &paths,
                store.clone(),
            ),
            remote: remote::RemoteTrust::new(store.clone()),
            slack: slack::Slack::new(secrets::Secrets::for_paths(&paths), settings.clone()),
            mobile: mobile::MobileHost::new(secrets::Secrets::for_paths(&paths), settings.clone()),
            cloud_vault: cloud_vault::CloudVaultHeads::new(&paths.home),
            cloud_vault_keys: cloud_vault_keys::CloudVaultKeys::new(secrets::Secrets::for_paths(
                &paths,
            )),
            preparation_keys: cloud_workspace_preparation_keys::PreparationKeys::new(
                secrets::Secrets::for_paths(&paths),
            ),
            execution: execution.clone(),
            process_owner: std::sync::Mutex::new(workspace_processes::Owner::of_daemon()),
            transfers: Default::default(),
            relay: Default::default(),
            workers: Default::default(),
            transfer_controller: Default::default(),
            idle_reminders: Default::default(),
            backups: cloud_workspace_backup::Backups::new(secrets::Secrets::for_paths(&paths)),
            transfer_captures: Default::default(),
            repos: repos::RepoRegistry::new(paths.clone(), settings.clone(), store.clone()),
            procs: procs::ProcessManager::new(events.clone(), shell_env.clone(), execution.clone()),
            workspaces: workspaces::WorkspaceManager::new(weak.clone()),
            chats: chats::ChatManager::new(weak.clone()),
            terminals: terminals::TerminalManager::new(events.clone(), execution.clone()),
            linear: linear::Linear::new(secrets::Secrets::for_paths(&paths)),
            news: news::News::new(),
            jev: jev::Jev::new(secrets::Secrets::for_paths(&paths)),
            models: models::ModelCatalog::default(),
            ollama: ollama::Ollama::new(weak.clone()),
            github: github::Github::new(weak.clone()),
            usage: usage::Usage::default(),
            updates: update::Updates::new(),
            shares: Arc::new(share::Shares::new(
                weak.clone(),
                secrets::Secrets::for_paths(&paths),
            )),
            workspace_shares: Arc::new(workspace_share::WorkspaceShares::new(weak.clone())),
            mentions: Arc::new(mentions::Mentions::new(weak.clone())),
            power: power::Power::new(&paths.home),
            search: search::Search::new(weak.clone(), &paths),
            paths,
            review_settings_lock: tokio::sync::Mutex::new(()),
            settings,
            store,
            events,
            shell_env,
            designs: canvas::DesignServer::default(),
            downloads: downloads::Downloads::default(),
            extension_web: Default::default(),
            error_reports,
            shutdown,
        });
        Ok(daemon)
    }
}
