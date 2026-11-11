//! Dispatch of client calls. Subscriptions are handled by the server, which owns the connection.

use std::sync::Arc;
use std::time::Duration;

use chauffeur_protocol::client::{
    ChatsResult, ClientCall, EmptyParams, HelloResult, HistoryResult, ReposListResult,
    SectionsResult, WorkspacesResult,
};
use chauffeur_protocol::{MIN_PROTOCOL_VERSION, PROTOCOL_VERSION};
use serde_json::Value;

use std::path::PathBuf;

use chauffeur_protocol::client::{
    CommandsListResult, DiffResult, FileContent, FileKind, FilesListResult, LinearSearchResult,
    LinearStatesResult, LoadoutParams, MergePrsResult, ModelsListResult, OpenAppsResult,
    PrStatusResult, QueueResult, ShippedResult, TerminalCwdResult, TerminalSnapshotResult,
    TerminalsResult,
};
use chauffeur_protocol::events::RegistryEvent;

use crate::git::Git;
use crate::store::EventFilter;
use crate::terminals::OpenSpec;
use crate::{Daemon, Error, Result, diff, files, models, open, prs, sections};

pub async fn handle(daemon: &Arc<Daemon>, call: ClientCall) -> Result<Value> {
    if let Some(extension) = crate::extensions::owner(call.method()) {
        return daemon.extensions.dispatch(daemon, extension, call).await;
    }
    // Transfers reenter RPC for local endpoints. Keep their outer dispatch out of the large
    // general handler's poll frame so two copies cannot overflow a default worker stack.
    match call {
        ClientCall::CloudTransferStart(p) => {
            to_value(crate::cloud_transfer_driver::start(daemon, p).await?)
        }
        ClientCall::CloudTransferCancel(p) => {
            to_value(crate::cloud_transfer_driver::cancel(daemon, p).await?)
        }
        call => handle_other(daemon, call).await,
    }
}

async fn handle_other(daemon: &Arc<Daemon>, call: ClientCall) -> Result<Value> {
    match call {
        ClientCall::DatabaseList(..)
        | ClientCall::DatabaseSave(..)
        | ClientCall::DatabaseDelete(..)
        | ClientCall::DatabaseTest(..)
        | ClientCall::DatabaseSchema(..)
        | ClientCall::DatabaseQuery(..)
        | ClientCall::DatabaseApply(..)
        | ClientCall::DatabaseCancel(..)
        | ClientCall::DatabaseHistory(..)
        | ClientCall::OfficeState(..)
        | ClientCall::OfficeSignIn(..)
        | ClientCall::OfficeSignOut(..)
        | ClientCall::OfficeSetVisibility(..)
        | ClientCall::OfficeSetMedia(..)
        | ClientCall::OfficeSetSkin(..)
        | ClientCall::OfficeSignal(..)
        | ClientCall::OfficeSay(..)
        | ClientCall::OfficeClear(..)
        | ClientCall::OfficeJukebox(..)
        | ClientCall::OfficeJukeboxResolve(..)
        | ClientCall::OfficeSpend(..)
        | ClientCall::OfficeUpgrade(..)
        | ClientCall::OfficeChangeClass(..)
        | ClientCall::OfficeChooseClass(..)
        | ClientCall::OfficeAttack(..)
        | ClientCall::OfficeStrikeDummy(..)
        | ClientCall::OfficeOpenInBrowser(..)
        | ClientCall::OfficeBrowserLink(..)
        | ClientCall::OfficeMove(..)
        | ClientCall::WorkspaceMoveHouse(..) => {
            unreachable!("extension methods are dispatched by their host")
        }
        ClientCall::ExtensionsList(_) => to_value(crate::extensions::list(daemon)?),
        ClientCall::ExtensionsSet(p) => to_value(daemon.extensions.set(daemon, p)?),
        ClientCall::ExtensionsConfigure(p) => to_value(daemon.extensions.configure(daemon, p)?),
        ClientCall::ExtensionsManage(p) => to_value(daemon.extensions.manage(daemon, p)?),
        ClientCall::ExtensionsCall(p) => daemon.extensions.call(p).await,
        ClientCall::ExtensionsWatch(_) => Err(Error::BadRequest(
            "Extension views require a persistent connection; subscribe through the WebSocket"
                .into(),
        )),
        ClientCall::IdentityState(_) => to_value(crate::identity::state(daemon)?),
        ClientCall::IdentitySignIn(_) => to_value(crate::identity::sign_in(daemon).await?),
        ClientCall::IdentitySignOut(_) => to_value(crate::identity::sign_out(daemon)?),
        ClientCall::CloudTransferStart(_) | ClientCall::CloudTransferCancel(_) => {
            unreachable!("reentrant transfer entry points are dispatched separately")
        }
        ClientCall::CloudTransferStatus(p) => {
            let transfer = crate::cloud_transfer_driver::status(daemon, &p.workspace)?;
            crate::cloud_transfer_driver::refresh_discard(daemon, transfer.as_ref());
            to_value(chauffeur_protocol::cloud_transfer::CloudTransferStatus { transfer })
        }
        ClientCall::CloudTransferInterrupt(p) => to_value(crate::cloud_transfer_driver::interrupt(
            daemon,
            &p.workspace,
        )?),
        ClientCall::CloudTransferMachines(_) => {
            to_value(crate::cloud_transfer_driver::machines(daemon))
        }
        ClientCall::CloudBackupRecoveryPlan(p) => {
            to_value(crate::cloud_backup_recovery::plan(daemon, &p.workspace).await?)
        }
        ClientCall::CloudBackupRecover(p) => {
            to_value(crate::cloud_backup_recovery::recover(daemon, p).await?)
        }
        ClientCall::CloudBackupRun(p) => {
            to_value(crate::cloud_workspace_backup::run(daemon, &p.workspace).await?)
        }
        ClientCall::WorkerList(_) => to_value(crate::workers::list(daemon)?),
        ClientCall::WorkerCreate(p) => to_value(crate::workers::create(daemon, p).await?),
        ClientCall::WorkerDelete(p) => to_value(crate::workers::delete(daemon, p)?),
        ClientCall::WorkerLogin(p) => to_value(crate::worker_login::start(daemon, p).await?),
        ClientCall::WorkerLoginCode(p) => {
            crate::worker_login::code(daemon, p).await?;
            Ok(Value::Null)
        }
        ClientCall::WorkerLoginCancel(p) => {
            crate::worker_login::cancel(daemon, p).await?;
            Ok(Value::Null)
        }
        ClientCall::WorkerUsage(p) => to_value(crate::worker_login::usage(daemon, &p.name).await?),
        ClientCall::MobileStatus(_) => to_value(daemon.mobile.status(&daemon.store)?),
        ClientCall::MobileConfigure(params) => {
            to_value(daemon.mobile.configure(&daemon.store, params)?)
        }
        ClientCall::RemotePushGet(params) => to_value(daemon.remote.push_get(&params.device_id)?),
        ClientCall::RemotePushSet(params) => to_value(daemon.remote.push_set(params)?),
        ClientCall::RemoteState(_) => to_value(daemon.remote.state()?),
        ClientCall::RemoteDevices(_) => to_value(daemon.remote.devices()?),
        ClientCall::RemoteApprove(params) => to_value(daemon.remote.approve(params)?),
        ClientCall::RemoteRevoke(params) => {
            daemon.remote.revoke(&params.device_id)?;
            to_value(EmptyParams {})
        }
        ClientCall::RemoteCheck(params) => to_value(daemon.remote.check(&params.device_id)?),
        ClientCall::Hello(params) => {
            if !(MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&params.protocol_version) {
                return Err(Error::BadRequest(format!(
                    "protocol {} is not supported (daemon speaks {MIN_PROTOCOL_VERSION} to {PROTOCOL_VERSION}); update the client or the daemon",
                    params.protocol_version
                )));
            }
            let login_shell = daemon.shell_env.shell();
            to_value(HelloResult {
                // The client's own, once accepted: a client that checks it against its constant
                // (the 0.24 app and CLI do) keeps working against a newer daemon.
                protocol_version: params.protocol_version,
                min_protocol_version: MIN_PROTOCOL_VERSION,
                daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                home: daemon.paths.home.to_string_lossy().into_owned(),
                workspaces_dir: daemon.paths.workspaces_dir.to_string_lossy().into_owned(),
                shell_profile: crate::shellenv::shell_profile(&login_shell).to_string(),
                login_shell,
            })
        }
        ClientCall::ReposList(params) => {
            let listing = daemon
                .repos
                .listing(params.refresh, &daemon.shell_env)
                .await?;
            to_value(ReposListResult {
                repos: listing.repos,
                refreshed_at: listing.refreshed_at,
                github_error: listing.github_error,
            })
        }
        ClientCall::WorkspaceCreate(params) => to_value(daemon.workspaces.create(params).await?),
        ClientCall::WorkspaceList(params) => to_value(WorkspacesResult {
            workspaces: daemon
                .workspaces
                .list(params.include_archived, params.parent.as_deref())?,
            sections: sections::list(daemon)?,
        }),
        ClientCall::WorkspaceSearchArchived(params) => {
            to_value(daemon.search.search_archived(params.query).await?)
        }
        ClientCall::SearchQuery(params) => to_value(daemon.search.query(params).await?),
        ClientCall::SearchStatus(_) => to_value(daemon.search.status().await?),
        ClientCall::WorkspaceGet(params) => to_value(daemon.workspaces.get(&params.workspace)?),
        ClientCall::ServicesGet(p) => {
            to_value(crate::workspace_dev_services::get(daemon, &p.workspace)?)
        }
        ClientCall::ServicesDetect(p) => {
            to_value(crate::workspace_dev_services::detect(daemon, &p.workspace).await?)
        }
        ClientCall::ServicesSave(p) => {
            to_value(crate::workspace_dev_services::save(daemon, p).await?)
        }
        ClientCall::PreviewsList(p) => {
            to_value(crate::workspace_previews::list(daemon, &p.workspace).await?)
        }
        ClientCall::WorkspaceAddRepo(params) => to_value(
            daemon
                .workspaces
                .add_repo(&params.workspace, &params.repo)
                .await?,
        ),
        // Moved to another machine, it comes back first, then is archived here.
        ClientCall::WorkspaceArchive(params)
            if let Some(info) =
                crate::cloud_transfer_driver::archive_moved(daemon, &params.workspace).await? =>
        {
            to_value(info)
        }
        ClientCall::WorkspaceArchive(params) if params.background => to_value(
            daemon
                .workspaces
                .archive_in_background(&params.workspace, params.force)?,
        ),
        ClientCall::WorkspaceArchive(params) => to_value(
            daemon
                .workspaces
                .archive(&params.workspace, params.force)
                .await?,
        ),
        ClientCall::WorkspaceRestore(params) => {
            to_value(daemon.workspaces.restore(&params.workspace).await?)
        }
        // Built in a function of its own, on the heap: in a debug build each inline `.await`
        // keeps its future's slot in this handler's frame, which transfers enter twice.
        call @ (ClientCall::RoutineList(_)
        | ClientCall::RoutineGet(_)
        | ClientCall::RoutineCreate(_)
        | ClientCall::RoutineEdit(_)
        | ClientCall::RoutineRun(_)
        | ClientCall::RoutineSetActive(_)
        | ClientCall::RoutineHistory(_)
        | ClientCall::RoutineDelete(_)
        | ClientCall::RoutineSuggestions(_)
        | ClientCall::RoutineFind(_)
        | ClientCall::RoutineSuggestionCreate(_)
        | ClientCall::RoutineSuggestionSet(_)) => handle_routine(daemon, call).await,
        ClientCall::ConductorImport(params) => {
            to_value(crate::conductor::import(daemon, params).await?)
        }
        ClientCall::WorkspaceWait(params) => to_value(
            daemon
                .workspaces
                .wait(
                    &params.workspace,
                    params.timeout_secs.map(Duration::from_secs),
                )
                .await?,
        ),
        ClientCall::WorktreeAdd(params) => {
            to_value(daemon.workspaces.add_agent_worktrees(params).await?)
        }
        ClientCall::WorktreeRemove(params) => to_value(
            daemon
                .workspaces
                .remove_agent_worktrees(&params.workspace, &params.target)
                .await?,
        ),
        ClientCall::ProcessList(params) => {
            to_value(daemon.workspaces.processes(&params.workspace)?)
        }
        ClientCall::ProcessRun(params) => to_value(
            daemon
                .workspaces
                .run_script(&params.workspace, &params.repo, params.name.as_deref())
                .await?,
        ),
        ClientCall::ProcessSetup(params) => to_value(
            daemon
                .workspaces
                .retry_setup(&params.workspace, &params.process)?,
        ),
        ClientCall::ProcessStop(params) => to_value(
            daemon
                .procs
                .stop(&params.process)
                .await
                .ok_or_else(|| Error::not_found(format!("process {}", params.process)))?,
        ),
        ClientCall::ChatCreate(params) => to_value(daemon.chats.create(params).await?),
        ClientCall::ChatList(params) => to_value(ChatsResult {
            chats: daemon.chats.list(&params.workspace, params.all)?,
        }),
        ClientCall::ChatRename(params) => {
            to_value(daemon.chats.rename(&params.chat, &params.title)?)
        }
        ClientCall::ChatMove(params) => to_value(
            daemon
                .chats
                .move_tab(&params.chat, params.before.as_deref())?,
        ),
        ClientCall::ChatClose(params) => {
            to_value(daemon.chats.close(&params.chat, params.force).await?)
        }
        ClientCall::ChatReopen(params) => to_value(daemon.chats.reopen(&params.chat)?),
        ClientCall::ChatResumeFailed(_) => {
            daemon.chats.resume_failed_turns()?;
            to_value(EmptyParams {})
        }
        ClientCall::DesignClaudePreview(params) => to_value(
            crate::claude_artifact::read(daemon, &params.url)
                .await?
                .preview,
        ),
        ClientCall::DesignCanvas(params) => to_value(crate::canvas::canvas(daemon, &params.chat)?),
        ClientCall::DesignResource(params) => {
            to_value(crate::canvas::resource(daemon, params).await?)
        }
        ClientCall::DesignExport(params) => {
            to_value(crate::design_export::export(daemon, &params).await?)
        }
        ClientCall::DesignComments(params) => {
            to_value(crate::comments::list(daemon, &params.chat).await?)
        }
        ClientCall::DesignCommentAdd(params) => {
            to_value(crate::comments::add(daemon, &params).await?)
        }
        ClientCall::DesignCommentEdit(params) => {
            to_value(crate::comments::edit(daemon, &params).await?)
        }
        ClientCall::DesignCommentDelete(params) => {
            crate::comments::delete(daemon, &params).await?;
            to_value(EmptyParams {})
        }
        ClientCall::DesignCommentResolve(params) => {
            to_value(crate::comments::resolve(daemon, &params).await?)
        }
        ClientCall::DesignCommentsSend(params) => {
            to_value(crate::comments::send(daemon, &params).await?)
        }
        // As with routines, sharing futures must not enlarge the reentrant transfer handler.
        call @ (ClientCall::DesignShareStart(_)
        | ClientCall::DesignShareUpdate(_)
        | ClientCall::DesignShareStop(_)
        | ClientCall::DesignShared(_)
        | ClientCall::FilesShareStart(_)
        | ClientCall::FilesShareUpdate(_)
        | ClientCall::FilesShareStop(_)) => handle_sharing(daemon, call).await,
        ClientCall::SharesList(_) => to_value(daemon.shares.list()?),
        ClientCall::DesignShareState(params) => to_value(daemon.shares.state(&params.chat)?),
        ClientCall::DesignImport(params) => to_value(daemon.shares.import(&params).await?),
        ClientCall::MentionsList(_) => to_value(daemon.mentions.list()),
        ClientCall::MentionsSeen(params) => to_value(daemon.mentions.seen(&params.id)?),
        ClientCall::ChatRegenerateBrief(params) => {
            to_value(daemon.chats.regenerate_brief(&params.chat)?)
        }
        ClientCall::ChatDelete(params) => {
            to_value(daemon.chats.delete(&params.chat, params.force).await?)
        }
        ClientCall::ChatFork(params) => to_value(
            daemon
                .chats
                .fork(&params.chat, params.turn, params.agent)
                .await?,
        ),
        ClientCall::ChatSendConfirmed(params) => {
            if params.client_id.is_none() || params.suggestion.is_some() {
                return Err(Error::BadRequest(
                    "A confirmed send requires a client identifier and cannot send a suggestion"
                        .into(),
                ));
            }
            to_value(crate::chat_sends::send(daemon, &params).await?)
        }
        ClientCall::ChatSend(params) => match &params.suggestion {
            Some(suggestion) => to_value(daemon.workspace_shares.send(&params, suggestion).await?),
            None => to_value(crate::chat_sends::send(daemon, &params).await?),
        },
        ClientCall::WorkspaceShareState(params) => {
            to_value(daemon.workspace_shares.state(&params.workspace)?)
        }
        ClientCall::WorkspaceShareStart(params) => {
            to_value(daemon.workspace_shares.start(&params).await?)
        }
        ClientCall::WorkspaceShareUpdate(params) => {
            to_value(daemon.workspace_shares.update(&params).await?)
        }
        ClientCall::WorkspaceShareStop(params) => {
            to_value(daemon.workspace_shares.stop(&params.workspace).await?)
        }
        ClientCall::WorkspaceSharePreview(params) => {
            to_value(daemon.workspace_shares.preview(&params.url).await?)
        }
        ClientCall::ChatSuggestions(params) => {
            to_value(daemon.workspace_shares.suggestions(&params.chat)?)
        }
        ClientCall::ChatSuggestionDismiss(params) => {
            to_value(daemon.workspace_shares.dismiss(&params)?)
        }
        ClientCall::ChatShareComments(params) => {
            to_value(daemon.workspace_shares.comments(&params.chat)?)
        }
        ClientCall::ChatShareCommentAdd(params) => {
            to_value(daemon.workspace_shares.add_comment(&params).await?)
        }
        ClientCall::ChatShareCommentDelete(params) => {
            to_value(daemon.workspace_shares.delete_comment(&params).await?)
        }
        ClientCall::ChatShell(params) => {
            to_value(daemon.chats.shell(&params.chat, &params.command).await?)
        }
        ClientCall::ChatQueueSet(params) => to_value(QueueResult {
            messages: daemon.chats.queue_set(&params.chat, &params.items)?,
        }),
        ClientCall::ChatQueueSend(params) => {
            to_value(daemon.chats.queue_send(&params.chat, &params.id).await?)
        }
        ClientCall::ChatInterrupt(params) => to_value(daemon.chats.interrupt(&params.chat).await?),
        ClientCall::ChatStopTask(params) => to_value(
            daemon
                .chats
                .stop_task(&params.chat, &params.task_id)
                .await?,
        ),
        ClientCall::ChatMcpConnect(params) => to_value(
            daemon
                .chats
                .mcp_connect(&params.chat, &params.server)
                .await?,
        ),
        ClientCall::ChatContextUsage(params) => to_value(
            daemon
                .chats
                .context_usage(&params.chat, params.detail)
                .await?,
        ),
        ClientCall::ChatSubagentMessages(params) => {
            to_value(daemon.chats.subagent_messages(params).await?)
        }
        ClientCall::ChatRespond(params) => to_value(
            daemon
                .chats
                .respond(&params.chat, &params.request_id, params.response)
                .await?,
        ),
        ClientCall::ChatConfigure(params) => to_value(daemon.chats.configure(params).await?),
        ClientCall::EventsHistory(params) => {
            if params.tail && (params.chat.is_none() || params.workspace.is_some()) {
                return Err(crate::Error::BadRequest(
                    "Recent history needs exactly one chat".into(),
                ));
            }
            let mut filter = EventFilter {
                limit: params.limit,
                ..Default::default()
            };
            if let Some(chat) = &params.chat {
                filter.chats.push(daemon.chats.resolve(chat)?.id);
            }
            if let Some(workspace) = &params.workspace {
                filter.workspaces.push(daemon.workspaces.row(workspace)?.id);
            }
            if filter.chats.is_empty() && filter.workspaces.is_empty() {
                filter.all = true;
            }
            to_value(HistoryResult {
                events: daemon.store.events_page(
                    params.after_seq.unwrap_or(0),
                    params.before_seq,
                    &filter,
                    params
                        .tail
                        .then_some(chauffeur_protocol::client::Replay::Tail {
                            per_chat: params.limit.unwrap_or(200).clamp(1, 200),
                        }),
                )?,
                tail: params.tail,
            })
        }
        ClientCall::DiffGet(params) => {
            let row = daemon.workspaces.row(&params.workspace)?;
            let git = Git::new(daemon.shell_env.clone());
            let worktrees: Vec<_> = daemon
                .store
                .worktrees_of(&row.id)?
                .into_iter()
                .filter(|w| !w.removed)
                .filter(|w| params.repo.as_ref().is_none_or(|r| &w.repo == r))
                .collect();
            let mut repos = Vec::new();
            for worktree in &worktrees {
                repos.push(diff::repo_diff(&git, worktree, params.stat_only).await?);
            }
            to_value(DiffResult { repos })
        }
        ClientCall::WorkspacePin(params) => {
            to_value(daemon.workspaces.pin(&params.workspace, params.pinned)?)
        }
        ClientCall::WorkspaceMove(params) => to_value(daemon.workspaces.move_to(
            &params.workspace,
            params.before.as_deref(),
            params.pinned,
            params.section.as_deref(),
        )?),

        ClientCall::WorkspaceSetSection(params) => to_value(
            daemon
                .workspaces
                .set_section(&params.workspace, params.section.as_deref())?,
        ),
        ClientCall::SectionList(_) => to_value(SectionsResult {
            sections: sections::list(daemon)?,
        }),
        ClientCall::SectionCreate(params) => to_value(sections::create(
            daemon,
            &params.name,
            params.workspace.as_deref(),
        )?),
        ClientCall::SectionRename(params) => {
            to_value(sections::rename(daemon, &params.section, &params.name)?)
        }
        ClientCall::SectionFold(params) => {
            to_value(sections::fold(daemon, &params.section, params.folded)?)
        }
        ClientCall::SectionMove(params) => to_value(SectionsResult {
            sections: sections::move_to(daemon, &params.section, params.before.as_deref())?,
        }),
        ClientCall::SectionDelete(params) => to_value(SectionsResult {
            sections: sections::delete(daemon, &params.section)?,
        }),
        ClientCall::WorkspaceMarkUnread(params) => {
            to_value(daemon.workspaces.mark_unread(&params.workspace)?)
        }
        ClientCall::WorkspacePromote(params) => to_value(
            daemon
                .workspaces
                .promote(&params.workspace, params.discard)
                .await?,
        ),
        ClientCall::WorkspaceSetTickets(params) => to_value(
            daemon
                .workspaces
                .set_tickets(&params.workspace, &params.tickets)
                .await?,
        ),
        ClientCall::WorkspaceRename(params) => {
            to_value(daemon.workspaces.rename(&params.workspace, &params.title)?)
        }
        ClientCall::WorkspaceRetitle(params) => to_value(
            daemon
                .chats
                .regenerate_title(&params.workspace, params.last_messages)
                .await?,
        ),
        ClientCall::FilesList(params) => {
            let (_, roots) = file_roots(daemon, &params.workspace)?;
            let git = Git::new(daemon.shell_env.clone());
            to_value(FilesListResult {
                roots: files::list(&git, &roots).await?,
            })
        }
        ClientCall::FilesDownload(params) => {
            if !crate::server::deliverable_path(&params.path) {
                return Err(Error::BadRequest(
                    "choose an HTML or PDF document under .chauffeur/artifacts/".into(),
                ));
            }
            let row = daemon.workspaces.row(&params.workspace)?;
            to_value(daemon.downloads.issue(&row.id, &params.path)?)
        }
        ClientCall::FilesRead(params) => {
            let (row, roots) = file_roots(daemon, &params.workspace)?;
            let content = files::read(&row, &roots, &params.path)?;
            to_value(with_preview(daemon, &row.id, content))
        }
        ClientCall::FilesComments(params) => {
            to_value(crate::file_comments::list(daemon, &params.workspace, &params.path).await?)
        }
        ClientCall::FilesCommentAdd(params) => {
            to_value(crate::file_comments::add(daemon, &params).await?)
        }
        ClientCall::FilesCommentEdit(params) => {
            to_value(crate::file_comments::edit(daemon, &params).await?)
        }
        ClientCall::FilesCommentDelete(params) => {
            crate::file_comments::delete(daemon, &params).await?;
            to_value(EmptyParams {})
        }
        ClientCall::FilesCommentResolve(params) => {
            to_value(crate::file_comments::resolve(daemon, &params).await?)
        }
        ClientCall::FilesCommentsSend(params) => to_value(crate::file_comments::send(
            daemon,
            &params.workspace,
            &params.path,
        )?),
        ClientCall::FilesShareState(params) => {
            to_value(daemon.shares.file_state(&params.workspace, &params.path)?)
        }
        ClientCall::FilesWrite(params) => {
            let (row, roots) = file_roots(daemon, &params.workspace)?;
            let content = files::write(
                &row,
                &roots,
                &params.path,
                &params.text,
                params.expected_version.as_deref(),
            )?;
            to_value(with_preview(daemon, &row.id, content))
        }
        ClientCall::FilesOpen(params) => {
            let (_, roots) = file_roots(daemon, &params.workspace)?;
            let file = files::resolve(&roots, &params.path)?;
            if files::media_type(&file, &params.path).is_none() {
                return Err(Error::BadRequest(format!(
                    "{}: only a video, an audio file or a PDF opens in its default app",
                    params.path
                )));
            }
            open::open_default(daemon, &file).await?;
            to_value(EmptyParams {})
        }
        ClientCall::FilesReveal(params) => {
            let (_, roots) = file_roots(daemon, &params.workspace)?;
            let file =
                files::resolve_to_reveal(&roots, &daemon.paths.workspaces_dir, &params.path)?;
            open::reveal(daemon, &file).await?;
            to_value(EmptyParams {})
        }
        ClientCall::FilesSearch(params) => {
            let (_, roots) = file_roots(daemon, &params.workspace)?;
            let git = Git::new(daemon.shell_env.clone());
            to_value(files::search(&git, &roots, &params).await?)
        }
        ClientCall::TerminalOpen(params) => {
            let row = daemon.workspaces.row(&params.workspace)?;
            let (cwd, title) = match &params.repo {
                Some(repo) => {
                    let worktree = daemon
                        .store
                        .worktrees_of(&row.id)?
                        .into_iter()
                        .find(|w| !w.removed && &w.repo == repo)
                        .ok_or_else(|| Error::not_found(format!("repo {repo} in {}", row.slug)))?;
                    (PathBuf::from(worktree.path), repo.clone())
                }
                None => (PathBuf::from(&row.path), row.slug.clone()),
            };
            let mut env = daemon.shell_env.vars();
            env.extend(daemon.workspaces.agent_env(&row, false)?);
            to_value(daemon.terminals.open(OpenSpec {
                workspace_id: row.id.clone(),
                cwd,
                title,
                cols: params.cols,
                rows: params.rows,
                command: params.command,
                env,
            })?)
        }
        ClientCall::TerminalInput(params) => {
            daemon.terminals.input(&params.terminal, &params.data)?;
            to_value(EmptyParams {})
        }
        ClientCall::TerminalResize(params) => to_value(daemon.terminals.resize(
            &params.terminal,
            params.cols,
            params.rows,
        )?),
        ClientCall::TerminalClose(params) => to_value(daemon.terminals.close(&params.terminal)?),
        ClientCall::TerminalList(params) => {
            let row = daemon.workspaces.row(&params.workspace)?;
            to_value(TerminalsResult {
                terminals: daemon.terminals.list(&row.id),
            })
        }
        ClientCall::TerminalMove(params) => to_value(TerminalsResult {
            terminals: daemon
                .terminals
                .move_tab(&params.terminal, params.before.as_deref())?,
        }),
        ClientCall::TerminalSnapshot(params) => {
            let (terminal, data, end_offset) = daemon.terminals.snapshot(&params.terminal)?;
            to_value(TerminalSnapshotResult {
                terminal,
                data,
                end_offset,
            })
        }
        ClientCall::TerminalRename(params) => {
            to_value(daemon.terminals.rename(&params.terminal, &params.name)?)
        }
        ClientCall::TerminalCwd(params) => to_value(TerminalCwdResult {
            cwd: daemon
                .terminals
                .cwd(&params.terminal)?
                .to_string_lossy()
                .into_owned(),
        }),
        ClientCall::PrStatus(params) => to_value(PrStatusResult {
            prs: prs::cached_status(daemon, &params.workspace).await?,
        }),
        ClientCall::PrMerge(params) => to_value(MergePrsResult {
            outcomes: prs::merge(daemon, &params).await?,
        }),
        ClientCall::PrShipped(params) => {
            let prs = crate::shipped::shipped(daemon, &params.since, params.team).await?;
            let (projects, unassigned) = match params.team {
                true => crate::shipped::board(daemon, &prs).await,
                false => (Vec::new(), Vec::new()),
            };
            to_value(ShippedResult {
                prs,
                projects,
                unassigned,
            })
        }
        ClientCall::PrCreate(params) => {
            let row = daemon.workspaces.row(&params.workspace)?;
            crate::workspaces::read_only(&row, "it opens no pull request")?;
            let statuses = prs::status(daemon, &row.id).await?;
            let tickets: Vec<String> = row.tickets.iter().map(|t| t.identifier.clone()).collect();
            let prompt = prs::create_prompt(
                &statuses,
                &tickets,
                params.draft,
                params.instructions.as_deref(),
            );
            to_value(daemon.chats.send(&row.id, &prompt).await?)
        }
        ClientCall::NewsState(_) => to_value(crate::news::state(daemon).await?),
        ClientCall::NewsMarkRead(params) => {
            daemon.store.mark_news_read(&params.ids)?;
            to_value(EmptyParams {})
        }
        ClientCall::NewsAddFeed(params) => {
            to_value(crate::news::add_feed(daemon, &params.url).await?)
        }
        ClientCall::NewsRemoveFeed(params) => {
            to_value(crate::news::remove_feed(daemon, &params.url).await?)
        }
        ClientCall::NewsShowHackerNews(params) => {
            to_value(crate::news::show_hacker_news(daemon, params.shown).await?)
        }
        ClientCall::LinearSearch(_)
        | ClientCall::LinearIssue(_)
        | ClientCall::LinearStates(_)
        | ClientCall::LinearSetState(_)
            if !daemon.settings.get().linear_enabled =>
        {
            Err(Error::Unavailable(
                "Linear is turned off in the settings".into(),
            ))
        }
        ClientCall::LinearSearch(params) => {
            let query = params.query.as_deref();
            let (issues, milestones) = tokio::join!(
                daemon
                    .linear
                    .search(query, params.mine, params.limit.unwrap_or(20)),
                async {
                    if !params.milestones.unwrap_or(false) {
                        return Vec::new();
                    }
                    // The tickets matter more: milestones Linear cannot list are left out.
                    daemon
                        .linear
                        .milestones(query)
                        .await
                        .inspect_err(
                            |error| tracing::warn!(%error, "could not list Linear milestones"),
                        )
                        .unwrap_or_default()
                },
            );
            let mut issues = issues?;
            for issue in &mut issues {
                keep_known_repos(daemon, &mut issue.repos).await?;
            }
            to_value(LinearSearchResult { issues, milestones })
        }
        ClientCall::LinearIssue(params) => {
            let mut issue = daemon.linear.issue(&params.identifier).await?.issue;
            keep_known_repos(daemon, &mut issue.repos).await?;
            to_value(issue)
        }
        ClientCall::LinearStates(params) => {
            let identifier = crate::linear::ticket_identifier(&params.identifier)?;
            to_value(LinearStatesResult {
                states: daemon.linear.states(&identifier).await?,
            })
        }
        ClientCall::LinearSetState(params) => {
            let identifier = crate::linear::ticket_identifier(&params.identifier)?;
            let mut issue = daemon.linear.set_state(&identifier, &params.state).await?;
            keep_known_repos(daemon, &mut issue.repos).await?;
            to_value(issue)
        }
        ClientCall::UsageGet(params) => to_value(chauffeur_protocol::client::UsageResult {
            agents: daemon.usage.get(daemon, params.refresh).await,
        }),
        ClientCall::AgentLogin(params) => to_value(
            daemon
                .usage
                .login(daemon, params.agent, params.headless)
                .await?,
        ),
        ClientCall::AgentLoginCode(params) => {
            daemon.usage.login_code(daemon, params).await?;
            Ok(Value::Null)
        }
        ClientCall::AgentLoginCancel(params) => {
            daemon.usage.login_cancel(daemon, params.agent).await?;
            Ok(Value::Null)
        }
        ClientCall::ModelsList(params) => {
            // A workspace moved where this machine does not reach (not one of `[transfer]`): its
            // agents' models are not this machine's.
            if let Some(workspace) = &params.workspace
                && let Some(machine) = crate::cloud_transfer_driver::moved_to(daemon, workspace)
            {
                return Err(Error::Conflict(format!(
                    "This workspace runs on {machine}, which this machine does not reach: its models are listed there"
                )));
            }
            to_value(ModelsListResult {
                agents: daemon.models.list(daemon, params.refresh).await,
            })
        }
        ClientCall::CommandsList(params) => to_value(CommandsListResult {
            commands: daemon
                .chats
                .commands(&params.workspace, params.agent)
                .await?,
        }),
        ClientCall::CommandsForRepos(params) => to_value(CommandsListResult {
            commands: daemon
                .chats
                .commands_for_repos(&params.repos, params.agent)
                .await?,
        }),
        ClientCall::LoadoutGet(_) => to_value(LoadoutParams {
            entries: models::loadout(daemon)?,
        }),
        ClientCall::LoadoutSet(params) => {
            if params.entries.iter().any(|e| e.model.trim().is_empty()) {
                return Err(Error::BadRequest("a loadout entry needs a model".into()));
            }
            models::set_loadout(daemon, &params.entries)?;
            daemon.events.registry(RegistryEvent::LoadoutChanged {
                entries: params.entries.clone(),
            });
            to_value(params)
        }
        ClientCall::SlackStatus(_) => to_value(daemon.slack.status().await?),
        ClientCall::SlackLogin(_) => to_value(daemon.slack.login().await?),
        ClientCall::SlackPoll(_) => to_value(daemon.slack.poll().await?),
        ClientCall::SlackCancel(_) => to_value(daemon.slack.cancel().await?),
        ClientCall::SlackLogout(_) => to_value(daemon.slack.logout().await?),
        ClientCall::SlackReceive(params) => to_value(daemon.slack.receive(params).await?),
        ClientCall::SlackContext(params) => to_value(daemon.slack.context(&params.link).await?),
        ClientCall::SettingsGet(_) => to_value(crate::preferences::get(daemon).await?),
        ClientCall::SettingsSet(params) => {
            to_value(crate::preferences::set(daemon, *params).await?)
        }
        ClientCall::SettingsOpen(_) => {
            crate::preferences::write_template(daemon)?;
            open::open_file(daemon, daemon.settings.path()).await?;
            to_value(EmptyParams {})
        }
        ClientCall::PowerGet(_) => to_value(daemon.power.state()),
        ClientCall::ErrorsReport(params) => {
            let mode = daemon.settings.get().error_reports;
            to_value(daemon.error_reports.report(mode, params).await?)
        }
        ClientCall::UpdateStatus(_) => to_value(daemon.updates.status()),
        ClientCall::UpdateCheck(_) => to_value(daemon.updates.check_now(daemon).await),
        ClientCall::UpdateInstall(_) => to_value(daemon.updates.install(daemon)?),
        ClientCall::OllamaStatus(_) => to_value(daemon.ollama.status().await),
        ClientCall::OllamaInstall(_) => {
            daemon.ollama.install()?;
            to_value(EmptyParams {})
        }
        ClientCall::OllamaPull(params) => {
            daemon.ollama.pull(&params.model)?;
            to_value(EmptyParams {})
        }
        ClientCall::OllamaDelete(params) => {
            daemon.ollama.delete(&params.model).await?;
            to_value(EmptyParams {})
        }
        ClientCall::GithubStatus(_) => to_value(daemon.github.status().await),
        ClientCall::GithubConnect(_) => {
            daemon.github.connect()?;
            to_value(EmptyParams {})
        }
        ClientCall::GithubCancel(_) => {
            daemon.github.cancel();
            to_value(EmptyParams {})
        }
        ClientCall::GithubDisconnect(_) => {
            daemon.github.disconnect().await?;
            to_value(EmptyParams {})
        }
        ClientCall::GitInstall(_) => {
            crate::github::install_git()?;
            to_value(EmptyParams {})
        }
        ClientCall::EnvReload(_) => to_value(crate::envreload::reload(daemon).await?),
        ClientCall::WorkspaceRemindLater(params) => {
            let mut rows = Vec::new();
            for key in &params.workspaces {
                let row = daemon.workspaces.row(key)?;
                if crate::relay::machine_of(daemon, &row.id).is_none() {
                    return Err(Error::Conflict(format!(
                        "{} runs on this machine: there is nothing to be reminded of",
                        row.slug
                    )));
                }
                rows.push(row);
            }
            for row in rows {
                daemon.idle_reminders.snooze(daemon, &row.id);
            }
            to_value(EmptyParams {})
        }
        ClientCall::WorkspaceSeen(params) => {
            let row = daemon.workspaces.row(&params.workspace)?;
            if row.unread() {
                daemon.store.set_workspace_seen(&row.id)?;
                daemon.workspaces.publish(&row.id);
            }
            to_value(EmptyParams {})
        }
        ClientCall::OpenApps(_) => to_value(OpenAppsResult {
            apps: open::apps(daemon)?,
        }),
        ClientCall::WorkspaceOpen(params) => to_value(open::open(daemon, &params).await?),
        ClientCall::DaemonShutdown(_) => {
            daemon.shutdown.cancel();
            to_value(EmptyParams {})
        }
        ClientCall::EventsSubscribe(_) | ClientCall::EventsUnsubscribe(_) => Err(
            Error::BadRequest("subscriptions are handled by the connection".into()),
        ),
    }
}

// Construct the future in a separate frame: debug builds otherwise reserve all these
// upload/authentication futures inside each general RPC dispatch, even for transfer calls.
#[inline(never)]
fn handle_sharing(
    daemon: &Arc<Daemon>,
    call: ClientCall,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>> {
    Box::pin(async move {
        match call {
            ClientCall::DesignShareStart(params) => to_value(daemon.shares.start(&params).await?),
            ClientCall::DesignShareUpdate(params) => to_value(daemon.shares.update(&params).await?),
            ClientCall::DesignShareStop(params) => {
                to_value(daemon.shares.stop(&params.chat).await?)
            }
            ClientCall::FilesShareStart(params) => {
                to_value(daemon.shares.start_file(&params).await?)
            }
            ClientCall::FilesShareUpdate(params) => {
                to_value(daemon.shares.update_file(&params).await?)
            }
            ClientCall::FilesShareStop(params) => to_value(
                daemon
                    .shares
                    .stop_file(&params.workspace, &params.path)
                    .await?,
            ),
            ClientCall::DesignShared(_) => to_value(daemon.shares.shared().await?),
            _ => unreachable!("only sharing calls reach this handler"),
        }
    })
}

/// A ticket's `## Repo` section is free text ("web-app (one sub-issue each)"): keep the words
/// that name a repo Chauffeur knows.
#[inline(never)]
fn handle_routine(
    daemon: &Arc<Daemon>,
    call: ClientCall,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>> {
    Box::pin(async move {
        match call {
            ClientCall::RoutineList(_) => to_value(chauffeur_protocol::routines::RoutinesResult {
                routines: crate::routines::list(daemon)?,
            }),
            ClientCall::RoutineGet(params) => {
                to_value(crate::routines::get(daemon, &params.routine)?)
            }
            ClientCall::RoutineCreate(params) => {
                to_value(crate::routines::create(daemon, params).await?)
            }
            ClientCall::RoutineEdit(params) => {
                to_value(crate::routines::edit(daemon, params).await?)
            }
            ClientCall::RoutineRun(params) => {
                to_value(crate::routines::run_now(daemon, &params.routine).await?)
            }
            ClientCall::RoutineSetActive(params) => {
                to_value(crate::routines::set_active(daemon, &params.routine, params.active).await?)
            }
            ClientCall::RoutineHistory(params) => {
                to_value(crate::routines::history(daemon, &params.routine)?)
            }
            ClientCall::RoutineDelete(params) => {
                to_value(chauffeur_protocol::routines::RoutinesResult {
                    routines: crate::routines::delete(daemon, &params.routine).await?,
                })
            }
            ClientCall::RoutineSuggestions(_) => to_value(crate::find_routines::info(daemon)?),
            ClientCall::RoutineFind(params) => {
                to_value(crate::find_routines::start(daemon, params).await?)
            }
            ClientCall::RoutineSuggestionCreate(params) => {
                to_value(crate::find_routines::create(daemon, &params.suggestion).await?)
            }
            ClientCall::RoutineSuggestionSet(params) => to_value(crate::find_routines::set(
                daemon,
                &params.suggestion,
                params.state,
            )?),
            _ => unreachable!("only routine methods are routed here"),
        }
    })
}

async fn keep_known_repos(daemon: &Arc<Daemon>, repos: &mut Vec<String>) -> Result<()> {
    let known = daemon.repos.known_names_loaded(&daemon.shell_env).await?;
    if !known.is_empty() {
        repos.retain(|repo| known.contains(repo));
    }
    Ok(())
}

pub fn to_value<T: serde::Serialize>(value: T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

/// A workspace and the folders its files are browsed in.
/// An HTML file's content with the URL the Files tab renders it from (story 43). A page too large
/// for the editor still has one, as long as the design server serves it: it is read from disk, not
/// sent in the message.
fn with_preview(daemon: &Daemon, workspace: &str, mut content: FileContent) -> FileContent {
    let served = match content.kind {
        FileKind::Text => true,
        FileKind::TooLarge => content.size <= chauffeur_design::MAX_FILE_BYTES,
        _ => false,
    };
    if served && files::is_html(&content.path) {
        content.preview_url = daemon.designs.file_url(workspace, &content.path);
    }
    content
}

pub(crate) fn file_roots(
    daemon: &Arc<Daemon>,
    workspace: &str,
) -> Result<(crate::store::WorkspaceRow, Vec<files::Root>)> {
    let row = daemon.workspaces.row(workspace)?;
    let worktrees = daemon.store.worktrees_of(&row.id)?;
    let roots = files::roots(&row, &worktrees);
    Ok((row, roots))
}
