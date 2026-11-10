//! Methods a client calls on the daemon, and their results.
//!
//! Wherever a parameter names a workspace it accepts its id or its slug; wherever it names a chat
//! it accepts a chat id, or a workspace id or slug meaning that workspace's first chat.

use crate::cloud_backup::{BackupRecovered, BackupRecoveryPlan, BackupStatus};
use crate::cloud_transfer::{CloudTransferMachines, CloudTransferStatus, TransferInfo};
use crate::dev_services::{DevServices, DevServicesDetected, Previews};
use crate::model::{ErrorReportOutcome, ErrorReports, PowerState, UpdateStatus};
use crate::office::{JukeboxStream, OfficeBrowserLink, OfficeState};
use crate::sidecar::{AgentLoginResult, McpConnectResult};
use crate::worker::{WorkerInfo, WorkersResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

use crate::events::{ChatEvent, ContextUsage, EventEnvelope, ProcessKind};
use crate::model::{
    AgentKind, AgentModels, ChatInfo, ChatKind, ImageInput, KeepAwake, LinearIssue,
    LinearMilestone, LinearState, LoadoutEntry, OpenApp, PermissionMode, QueuedMessage, RepoInfo,
    RequestResponse, SectionInfo, SendMode, SlashCommand, TerminalInfo, TicketRef, WorkspaceInfo,
    WorktreeInfo,
};

methods! {
    pub enum ClientCall / ClientMethod {
        /// The official extensions shipped with this version, including their activation state.
        #[serde(rename = "extensions.list")]
        #[access(User)]
        #[relay(Local)]
        ExtensionsList(EmptyParams) -> Vec<crate::extensions::ExtensionInfo>,
        #[serde(rename = "extensions.set")]
        #[access(User)]
        #[relay(Local)]
        ExtensionsSet(crate::extensions::ExtensionSetParams) -> Vec<crate::extensions::ExtensionInfo>,
        #[serde(rename = "extensions.configure")]
        #[access(User)]
        #[relay(Local)]
        ExtensionsConfigure(crate::extensions::ExtensionConfigureParams) -> Vec<crate::extensions::ExtensionInfo>,
        #[serde(rename = "extensions.manage")]
        #[access(User)]
        #[relay(Local)]
        ExtensionsManage(crate::extensions::ExtensionManageParams) -> Vec<crate::extensions::ExtensionInfo>,
        /// User clients only: an installed package cannot expand agent/browser permissions.
        #[serde(rename = "extensions.call")]
        #[access(User)]
        #[relay(Local)]
        ExtensionsCall(crate::extensions::ExtensionCallParams) -> serde_json::Value,
        /// A mounted view pins its version until the connection unsubscribes or disconnects.
        #[serde(rename = "extensions.watch")]
        #[access(User)]
        #[relay(Local)]
        ExtensionsWatch(crate::extensions::ExtensionWatchParams) -> SubscribeResult,
        #[serde(rename = "identity.state")]
        #[access(User)]
        #[relay(Local)]
        IdentityState(EmptyParams) -> crate::identity::IdentityState,
        #[serde(rename = "identity.sign_in")]
        #[access(User)]
        #[relay(Local)]
        IdentitySignIn(EmptyParams) -> crate::identity::IdentityState,
        #[serde(rename = "identity.sign_out")]
        #[access(User)]
        #[relay(Local)]
        IdentitySignOut(EmptyParams) -> crate::identity::IdentityState,
        /// PostgreSQL connections of a workspace, their credentials behind Touch ID: the user's
        /// clients only, never an agent's.
        #[serde(rename = "database.list")]
        #[access(User)]
        #[relay(Local)]
        DatabaseList(crate::database::DatabaseListParams) -> Vec<crate::database::DatabaseConnection>,
        #[serde(rename = "database.save")]
        #[access(User)]
        #[relay(Local)]
        DatabaseSave(crate::database::DatabaseSaveParams) -> crate::database::DatabaseConnection,
        #[serde(rename = "database.delete")]
        #[access(User)]
        #[relay(Local)]
        DatabaseDelete(crate::database::DatabaseParams) -> (),
        #[serde(rename = "database.test")]
        #[access(User)]
        #[relay(Local)]
        DatabaseTest(crate::database::DatabaseSaveParams) -> (),
        #[serde(rename = "database.schema")]
        #[access(User)]
        #[relay(Local)]
        DatabaseSchema(crate::database::DatabaseParams) -> Vec<crate::database::DatabaseTable>,
        #[serde(rename = "database.query")]
        #[access(User)]
        #[relay(Local)]
        DatabaseQuery(crate::database::DatabaseQueryParams) -> crate::database::DatabaseResult,
        #[serde(rename = "database.cancel")]
        #[access(User)]
        #[relay(Local)]
        DatabaseCancel(crate::database::DatabaseCancelParams) -> (),
        #[serde(rename = "database.apply")]
        #[access(User)]
        #[relay(Local)]
        DatabaseApply(crate::database::DatabaseApplyParams) -> crate::database::DatabaseResult,
        #[serde(rename = "database.history")]
        #[access(User)]
        #[relay(Local)]
        DatabaseHistory(crate::database::DatabaseParams) -> Vec<crate::database::DatabaseHistoryEntry>,
        #[serde(rename = "hello")]
        #[access(Any, office)]
        #[relay(Local)]
        Hello(HelloParams) -> HelloResult,
        /// The Office's network side (docs/office.md).
        #[serde(rename = "office.state")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeState(EmptyParams) -> OfficeState,
        /// Signs in with Google in the browser (`[office] google_*` settings); returns once done.
        #[serde(rename = "office.sign_in")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeSignIn(EmptyParams) -> OfficeState,
        #[serde(rename = "office.sign_out")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeSignOut(EmptyParams) -> OfficeState,
        #[serde(rename = "office.set_visibility")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeSetVisibility(crate::office::OfficeVisibilityParams) -> OfficeState,
        /// Where the user's character is; forwarded only while they are visible.
        #[serde(rename = "office.move")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeMove(crate::office::OfficePosition) -> EmptyParams,
        /// Microphone and camera open to the colleagues who come near.
        #[serde(rename = "office.set_media")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeSetMedia(crate::office::OfficeMediaParams) -> OfficeState,
        /// The villager outfit of the user's character, shown to colleagues.
        #[serde(rename = "office.set_skin")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeSetSkin(crate::office::OfficeSkinParams) -> OfficeState,
        /// A WebRTC message for a colleague, while both have media on.
        #[serde(rename = "office.signal")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeSignal(crate::office::OfficeSignalParams) -> EmptyParams,
        /// A speech bubble for the colleagues near the user's character, while visible.
        #[serde(rename = "office.say")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeSay(crate::office::OfficeSayParams) -> EmptyParams,
        /// A tree cut down, a rock rolled away: for everyone in the Office until it grows back.
        #[serde(rename = "office.clear")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeClear(crate::office::OfficeClearParams) -> EmptyParams,
        /// Plays, pauses or stops the plaza's jukebox for everyone in the Office.
        #[serde(rename = "office.jukebox")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeJukebox(crate::office::JukeboxCommand) -> EmptyParams,
        /// Finds the audio stream of a page the jukebox plays (YouTube…) with the user's yt-dlp.
        #[serde(rename = "office.jukebox_resolve")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeJukeboxResolve(crate::office::JukeboxResolveParams) -> JukeboxStream,
        /// Spends the points the user's levels earned on their character's stats.
        #[serde(rename = "office.spend")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeSpend(crate::office::OfficeSpendParams) -> OfficeState,
        /// Buys the next level of the user's character's weapon or armour with its gold.
        #[serde(rename = "office.upgrade")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeUpgrade(crate::office::OfficeUpgradeParams) -> OfficeState,
        /// The class of the user's character, chosen once.
        #[serde(rename = "office.choose_class")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeChooseClass(crate::office::OfficeClassParams) -> OfficeState,
        /// Another class for the user's character, for gold: its stat points come back to spend.
        #[serde(rename = "office.change_class")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeChangeClass(crate::office::OfficeClassParams) -> OfficeState,
        /// A blow of the user's weapon at a colleague within its reach, while visible.
        #[serde(rename = "office.attack")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeAttack(crate::office::OfficeAttackParams) -> EmptyParams,
        /// A blow the user's app rolled at a training dummy, for the colleagues near to see; nothing
        /// while invisible.
        #[serde(rename = "office.strike_dummy")]
        #[access(Any, office)]
        #[relay(Local)]
        OfficeStrikeDummy(crate::office::OfficeStrikeDummyParams) -> EmptyParams,
        /// Opens the Office in the user's default browser, for a web view without WebRTC: the
        /// daemon's own Office page on 127.0.0.1, through a one-time link (docs/office.md).
        #[serde(rename = "office.open_in_browser")]
        #[access(Any)]
        #[relay(Local)]
        OfficeOpenInBrowser(EmptyParams) -> OfficeBrowserLink,
        /// A one-time link to the Office's page on 127.0.0.1, without opening it: for the app,
        /// which opens it itself from the graphical session and gives it again once it expired.
        #[serde(rename = "office.browser_link")]
        #[access(Any)]
        #[relay(Local)]
        OfficeBrowserLink(EmptyParams) -> OfficeBrowserLink,
        #[serde(rename = "mobile.status")]
        #[access(Any)]
        #[relay(Local)]
        MobileStatus(EmptyParams) -> MobileHostStatus,
        #[serde(rename = "mobile.configure")]
        #[access(Any)]
        #[relay(Local)]
        MobileConfigure(MobileConfigureParams) -> MobileHostStatus,
        /// Private host-side configuration. Never forwarded to a mobile client.
        #[serde(rename = "remote.state")]
        #[access(Owner)]
        #[relay(Local)]
        RemoteState(EmptyParams) -> RemoteStateResult,
        #[serde(rename = "remote.devices")]
        #[access(Owner)]
        #[relay(Local)]
        RemoteDevices(EmptyParams) -> RemoteDevicesResult,
        #[serde(rename = "remote.approve")]
        #[access(Owner)]
        #[relay(Local)]
        RemoteApprove(RemoteApproveParams) -> RemoteDevice,
        #[serde(rename = "remote.revoke")]
        #[access(Owner)]
        #[relay(Local)]
        RemoteRevoke(RemoteDeviceParams) -> EmptyParams,
        #[serde(rename = "remote.push_set")]
        #[access(Owner)]
        #[relay(Local)]
        RemotePushSet(RemotePushParams) -> PushPreferences,
        #[serde(rename = "remote.push_get")]
        #[access(Owner)]
        #[relay(Local)]
        RemotePushGet(RemoteDeviceParams) -> PushPreferences,
        #[serde(rename = "remote.check")]
        #[access(Owner)]
        #[relay(Local)]
        RemoteCheck(RemoteDeviceParams) -> RemoteDeviceInfo,
        #[serde(rename = "cloud.transfer.cancel")]
        #[access(Any, phone)]
        #[relay(Local)]
        CloudTransferCancel(crate::cloud_transfer::CloudTransferStartParams) -> TransferInfo,
        #[serde(rename = "cloud.transfer.start")]
        #[access(Any, phone)]
        #[relay(Local)]
        CloudTransferStart(crate::cloud_transfer::CloudTransferStartParams) -> TransferInfo,
        #[serde(rename = "cloud.transfer.status")]
        #[access(Any, phone)]
        #[relay(Local)]
        CloudTransferStatus(WorkspaceParams) -> CloudTransferStatus,
        /// A transfer waiting for the end of its agents' turns interrupts them now, then goes on.
        #[serde(rename = "cloud.transfer.interrupt")]
        #[access(Any, phone)]
        #[relay(Local)]
        CloudTransferInterrupt(WorkspaceParams) -> TransferInfo,
        /// The names of the machines a workspace can be transferred to now, for a client that does
        /// not read the settings (the phone).
        #[serde(rename = "cloud.transfer.machines")]
        #[access(Any, phone)]
        #[relay(Local)]
        CloudTransferMachines(EmptyParams) -> CloudTransferMachines,
        /// Backs the workspace up now, on the machine it runs on, and returns where its backups stand.
        #[serde(rename = "cloud.backup.run")]
        #[access(Any)]
        #[relay(Act)]
        CloudBackupRun(WorkspaceParams) -> BackupStatus,
        #[serde(rename = "cloud.backup.recovery_plan")]
        #[access(Any)]
        #[relay(Local)]
        CloudBackupRecoveryPlan(WorkspaceParams) -> BackupRecoveryPlan,
        #[serde(rename = "cloud.backup.recover")]
        #[access(Any)]
        #[relay(Local)]
        CloudBackupRecover(crate::cloud_backup::BackupRecoverParams) -> BackupRecovered,
        #[serde(rename = "services.get")]
        #[access(Any)]
        #[relay(Read)]
        ServicesGet(WorkspaceParams) -> DevServices,
        #[serde(rename = "services.detect")]
        #[access(Any)]
        #[relay(Read)]
        ServicesDetect(WorkspaceParams) -> DevServicesDetected,
        #[serde(rename = "services.save")]
        #[access(Any)]
        #[relay(Act)]
        ServicesSave(crate::dev_services::DevServicesSaveParams) -> DevServices,
        /// A workspace's declared web previews and what each port does now.
        #[serde(rename = "previews.list")]
        #[access(Any, phone)]
        #[relay(Act)]
        PreviewsList(WorkspaceParams) -> Previews,
        /// GCP workers (docs/cloud-workers.md): the workers and what a new one would cost.
        #[serde(rename = "worker.list")]
        #[access(Any)]
        #[relay(Local)]
        WorkerList(EmptyParams) -> WorkersResult,
        /// Creates a worker in the background and returns it as creating; `worker.list` follows it.
        #[serde(rename = "worker.create")]
        #[access(Any)]
        #[relay(Local)]
        WorkerCreate(crate::worker::WorkerCreateParams) -> WorkerInfo,
        /// Deletes a worker and its disk in the background; refused while a workspace runs there.
        #[serde(rename = "worker.delete")]
        #[access(Any)]
        #[relay(Local)]
        WorkerDelete(crate::worker::WorkerParams) -> WorkerInfo,
        /// Starts signing an agent in on a worker, from this machine: the worker's own CLI runs a flow
        /// whose page opens here (`agent.login` headless there). The end comes as a `login_ended`
        /// event naming the worker; a transfer refused for that login resumes once none is missing.
        #[serde(rename = "worker.login")]
        #[access(Any)]
        #[relay(Local)]
        WorkerLogin(crate::worker::WorkerLoginParams) -> AgentLoginResult,
        /// Passes the code the login page showed on to the worker's CLI (Claude Code).
        #[serde(rename = "worker.login_code")]
        #[access(Any)]
        #[relay(Local)]
        WorkerLoginCode(crate::worker::WorkerLoginCodeParams) -> (),
        /// Ends a worker's login under way.
        #[serde(rename = "worker.login_cancel")]
        #[access(Any)]
        #[relay(Local)]
        WorkerLoginCancel(crate::worker::WorkerLoginParams) -> (),
        /// The worker's agents as `usage.get` reads them there: whether each is signed in.
        #[serde(rename = "worker.usage")]
        #[access(Any)]
        #[relay(Local)]
        WorkerUsage(crate::worker::WorkerParams) -> UsageResult,
        #[serde(rename = "repos.list")]
        #[access(Any)]
        #[relay(Local)]
        ReposList(ReposListParams) -> ReposListResult,
        #[serde(rename = "workspace.create")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceCreate(CreateWorkspaceParams) -> CreateWorkspaceResult,
        #[serde(rename = "workspace.list")]
        #[access(Any, phone, office)]
        #[relay(Local)]
        WorkspaceList(ListWorkspacesParams) -> WorkspacesResult,
        /// Archived conversations only, as `search.query` finds them: kept for clients older
        /// than `search.query`.
        #[serde(rename = "workspace.search_archived")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceSearchArchived(SearchArchivedParams) -> ArchivedSearchResult,
        /// Searches every workspace, active and archived: their titles, tickets and PRs, their
        /// chats' messages and their notes, by keywords and, once indexed, by meaning.
        #[serde(rename = "search.query")]
        #[access(Any)]
        #[relay(Local)]
        SearchQuery(SearchParams) -> SearchResult,
        /// Whether search by meaning is on, and how far the index is. Starts nothing.
        #[serde(rename = "search.status")]
        #[access(Any)]
        #[relay(Local)]
        SearchStatus(EmptyParams) -> SearchStatus,
        #[serde(rename = "workspace.get")]
        #[access(Any, phone)]
        #[relay(Local)]
        WorkspaceGet(WorkspaceParams) -> WorkspaceInfo,
        #[serde(rename = "workspace.add_repo")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceAddRepo(AddRepoParams) -> WorkspaceInfo,
        #[serde(rename = "workspace.archive")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceArchive(ArchiveParams) -> WorkspaceInfo,
        #[serde(rename = "workspace.restore")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceRestore(WorkspaceParams) -> WorkspaceInfo,
        /// Imports Conductor's repos, workspaces and chat history (docs/adr/conductor-import.md);
        /// with `dry_run`, what it would do, nothing changed. Releases branches in Conductor: the
        /// client asks the user first.
        #[serde(rename = "conductor.import")]
        #[access(User)]
        #[relay(Local)]
        ConductorImport(crate::conductor::ConductorImportParams) -> crate::conductor::ConductorImportResult,
        /// The routines (docs/adr/routines.md), in the order they were made.
        #[serde(rename = "routine.list")]
        #[access(Any)]
        #[relay(Local)]
        RoutineList(EmptyParams) -> crate::routines::RoutinesResult,
        #[serde(rename = "routine.get")]
        #[access(Any)]
        #[relay(Local)]
        RoutineGet(crate::routines::RoutineParams) -> crate::routines::RoutineInfo,
        /// Makes a draft routine from a chat: a workspace of its own with the chat's repos, filed
        /// in the "Routines" section, whose refining chat drafts the file from the conversation.
        #[serde(rename = "routine.create")]
        #[access(Any)]
        #[relay(Local)]
        RoutineCreate(crate::routines::RoutineCreateParams) -> crate::routines::RoutineInfo,
        /// Replaces the routine's file, validated, as a new version; repos it names that its
        /// workspace lacks are added. An unchanged file makes no version.
        #[serde(rename = "routine.edit")]
        #[access(Any)]
        #[relay(Local)]
        RoutineEdit(crate::routines::RoutineEditParams) -> crate::routines::RoutineInfo,
        /// Runs the routine now, as a new chat of its workspace; refused while the previous run
        /// works or waits for the user.
        #[serde(rename = "routine.run")]
        #[access(User)]
        #[relay(Local)]
        RoutineRun(crate::routines::RoutineParams) -> crate::routines::RoutineRun,
        /// Activates the routine (it runs on its schedule) or pauses it.
        #[serde(rename = "routine.set_active")]
        #[access(User)]
        #[relay(Local)]
        RoutineSetActive(crate::routines::RoutineSetActiveParams) -> crate::routines::RoutineInfo,
        /// The routine's versions and runs, newest first.
        #[serde(rename = "routine.history")]
        #[access(Any)]
        #[relay(Local)]
        RoutineHistory(crate::routines::RoutineParams) -> crate::routines::RoutineHistory,
        /// Deletes the routine, its versions and its runs' records; its workspace and chats stay,
        /// as an ordinary workspace.
        #[serde(rename = "routine.delete")]
        #[access(User)]
        #[relay(Local)]
        RoutineDelete(crate::routines::RoutineParams) -> crate::routines::RoutinesResult,
        /// Find routines as it stands: the analysis under way and the suggestions waiting.
        #[serde(rename = "routine.suggestions")]
        #[access(Any)]
        #[relay(Local)]
        RoutineSuggestions(EmptyParams) -> crate::routines::RoutineFindInfo,
        /// Starts Find routines: the user's own messages of the last `days` days, every workspace
        /// included, read by the agent and model given in one turn, to propose routines. Answers
        /// at once; refused while an analysis runs.
        #[serde(rename = "routine.find")]
        #[access(User)]
        #[relay(Local)]
        RoutineFind(crate::routines::RoutineFindParams) -> crate::routines::RoutineFindInfo,
        /// Makes a draft routine from a suggestion, as from its newest request's chat, with the
        /// proposal as its first version.
        #[serde(rename = "routine.suggestion_create")]
        #[access(User)]
        #[relay(Local)]
        RoutineSuggestionCreate(crate::routines::RoutineSuggestionParams) -> crate::routines::RoutineInfo,
        /// Keeps a suggestion for later, or ignores it: never proposed again.
        #[serde(rename = "routine.suggestion_set")]
        #[access(User)]
        #[relay(Local)]
        RoutineSuggestionSet(crate::routines::RoutineSuggestionSetParams) -> crate::routines::RoutineFindInfo,
        /// Changes the name shown; the folder and the branch keep theirs.
        #[serde(rename = "workspace.rename")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceRename(RenameWorkspaceParams) -> WorkspaceInfo,
        /// Names the workspace again from its conversation or last N messages, whoever named it so
        /// far; the new name is then the user's. Refused while the workspace has no conversation.
        #[serde(rename = "workspace.retitle")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceRetitle(RetitleWorkspaceParams) -> WorkspaceInfo,
        /// Turns an exploration into a workspace: a branch per repo from the commit it reads, the
        /// files to copy, ports and setup scripts; its chats can write from their next message.
        /// Refused (`CONFLICT`, with [`PromoteLeftovers`] as the error's data) while a checkout holds
        /// files its commit does not, unless `discard`.
        #[serde(rename = "workspace.promote")]
        #[access(Any)]
        #[relay(Local)]
        WorkspacePromote(PromoteParams) -> WorkspaceInfo,
        /// Replaces the Linear tickets of a workspace and reads them all from Linear again (the same
        /// list: a refresh). Rewrites the agents' briefing; idle agent sessions resume with it at their
        /// next message. A ticket newly attached is moved to In Progress as at creation.
        #[serde(rename = "workspace.set_tickets")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceSetTickets(SetTicketsParams) -> SetTicketsResult,
        /// Keeps a workspace at the top of the sidebar (`pinned`), or not.
        #[serde(rename = "workspace.pin")]
        #[access(Any)]
        #[relay(Local)]
        WorkspacePin(PinWorkspaceParams) -> WorkspaceInfo,
        /// Moves a workspace in the sidebar: above another one, or last, among the pinned or the
        /// other workspaces (which pins or unpins it).
        #[serde(rename = "workspace.move")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceMove(MoveWorkspaceParams) -> WorkspaceInfo,
        /// Moves a workspace's house in the Office before another workspace's, or last; the Office
        /// lays out the houses, and publishes them to colleagues, in that order (`house_position`).
        #[serde(rename = "workspace.move_house")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceMoveHouse(MoveHouseParams) -> WorkspaceInfo,
        /// Files a workspace in a sidebar section (`section`: its id or name), or in none; it goes
        /// on top of that section. A pinned workspace stays pinned, its section kept for when it is
        /// unpinned. Its children follow it.
        #[serde(rename = "workspace.set_section")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceSetSection(SetSectionParams) -> WorkspaceInfo,
        /// The sidebar's sections, in order.
        #[serde(rename = "section.list")]
        #[access(Any)]
        #[relay(Local)]
        SectionList(EmptyParams) -> SectionsResult,
        /// Creates a section, last of them (above the "Workspaces" list); with `workspace`, files it
        /// there.
        #[serde(rename = "section.create")]
        #[access(Any)]
        #[relay(Local)]
        SectionCreate(CreateSectionParams) -> SectionInfo,
        #[serde(rename = "section.rename")]
        #[access(Any)]
        #[relay(Local)]
        SectionRename(RenameSectionParams) -> SectionInfo,
        /// Folds or unfolds a section in the sidebar.
        #[serde(rename = "section.fold")]
        #[access(Any)]
        #[relay(Local)]
        SectionFold(FoldSectionParams) -> SectionInfo,
        /// Moves a section above another one, or last.
        #[serde(rename = "section.move")]
        #[access(Any)]
        #[relay(Local)]
        SectionMove(MoveSectionParams) -> SectionsResult,
        /// Deletes a section; its workspaces (archived ones included) go back to no section.
        #[serde(rename = "section.delete")]
        #[access(Any)]
        #[relay(Local)]
        SectionDelete(SectionParams) -> SectionsResult,
        /// Shows a workspace as unread again.
        #[serde(rename = "workspace.mark_unread")]
        #[access(Any)]
        #[relay(Act)]
        WorkspaceMarkUnread(WorkspaceParams) -> WorkspaceInfo,
        /// Reminds of these moved workspaces in an hour, rather than now, unless something happens
        /// there meanwhile.
        #[serde(rename = "workspace.remind_later")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceRemindLater(RemindLaterParams) -> EmptyParams,
        #[serde(rename = "workspace.wait")]
        #[access(Any)]
        #[relay(Read)]
        WorkspaceWait(WaitParams) -> WaitResult,
        #[serde(rename = "worktree.add")]
        #[access(Any)]
        #[relay(Act)]
        WorktreeAdd(AddWorktreeParams) -> AddWorktreeResult,
        #[serde(rename = "worktree.remove")]
        #[access(Any)]
        #[relay(Act)]
        WorktreeRemove(RemoveWorktreeParams) -> RemoveWorktreeResult,
        #[serde(rename = "process.list")]
        #[access(Any)]
        #[relay(Read)]
        ProcessList(WorkspaceParams) -> ProcessesResult,
        #[serde(rename = "process.run")]
        #[access(Any)]
        #[relay(Act)]
        ProcessRun(RunScriptParams) -> ProcessInfo,
        #[serde(rename = "process.setup")]
        #[access(Any)]
        #[relay(Act)]
        ProcessSetup(SetupScriptParams) -> ProcessInfo,
        #[serde(rename = "process.stop")]
        #[access(Any)]
        #[relay(Act)]
        ProcessStop(StopProcessParams) -> ProcessInfo,
        #[serde(rename = "chat.create")]
        #[access(Any)]
        #[relay(Act)]
        ChatCreate(CreateChatParams) -> ChatInfo,
        /// A workspace's chats; the closed ones too with `all`.
        #[serde(rename = "chat.list")]
        #[access(Any, phone, office)]
        #[relay(Read)]
        ChatList(ChatListParams) -> ChatsResult,
        #[serde(rename = "chat.send")]
        #[access(Any, phone)]
        #[relay(Act)]
        ChatSend(SendParams) -> ChatInfo,
        /// As chat.send, but requires a client identifier; older hosts refuse this method.
        #[serde(rename = "chat.send_confirmed")]
        #[access(Any, phone)]
        #[relay(Act)]
        ChatSendConfirmed(SendParams) -> ChatInfo,
        #[serde(rename = "chat.interrupt")]
        #[access(Any, phone)]
        #[relay(Act)]
        ChatInterrupt(ChatParams) -> ChatInfo,
        #[serde(rename = "chat.respond")]
        #[access(Any, phone)]
        #[relay(Act)]
        ChatRespond(RespondParams) -> ChatInfo,
        /// Stops one task of the agent (a background command, a subagent); its `task_ended` follows.
        #[serde(rename = "chat.stop_task")]
        #[access(Any)]
        #[relay(Act)]
        ChatStopTask(StopTaskParams) -> ChatInfo,
        /// Runs a command of the user's in the chat's workspace folder, shown in the chat
        /// (`shell_started`, `shell_ended`) and handed to the agent with the next message; it starts
        /// no turn. Answers once the command started. The app's only: refused on the Unix socket.
        #[serde(rename = "chat.shell")]
        #[access(App)]
        #[relay(Local)]
        ChatShell(ShellParams) -> ShellResult,
        /// Connects one of the chat's MCP servers again: its login when it needs one (open the
        /// result's `auth_url`; the chat's `mcp_servers` show it connected once done), a reconnection
        /// otherwise. Needs the chat's agent session running.
        #[serde(rename = "chat.mcp_connect")]
        #[access(Any)]
        #[relay(Local)]
        ChatMcpConnect(McpConnectParams) -> McpConnectResult,
        /// How full a chat's context window is: read from its running session (`full`: each category
        /// counted, slower), else the last reading stored.
        #[serde(rename = "chat.context_usage")]
        #[access(Any)]
        #[relay(Read)]
        ChatContextUsage(ContextUsageParams) -> ContextUsageResult,
        /// A subagent's conversation, read from its transcript (Claude Code keeps one per subagent,
        /// running or finished): its text, thinking and tool calls as chat events. Works without a
        /// running session. Codex has no such subagents.
        #[serde(rename = "chat.subagent_messages")]
        #[access(Any)]
        #[relay(Read)]
        ChatSubagentMessages(SubagentMessagesParams) -> SubagentMessagesResult,
        /// Reorders, edits or removes the messages waiting in a chat's queue.
        #[serde(rename = "chat.queue_set")]
        #[access(Any)]
        #[relay(Act)]
        ChatQueueSet(QueueSetParams) -> QueueResult,
        /// Sends one queued message now (it steers the running turn).
        #[serde(rename = "chat.queue_send")]
        #[access(Any)]
        #[relay(Act)]
        ChatQueueSend(QueueSendParams) -> ChatInfo,
        #[serde(rename = "chat.configure")]
        #[access(Any, phone)]
        #[relay(Act)]
        ChatConfigure(ConfigureChatParams) -> ChatInfo,
        /// Changes the title a chat's tab shows; an empty title gives back the automatic one.
        #[serde(rename = "chat.rename")]
        #[access(Any)]
        #[relay(Act)]
        ChatRename(RenameChatParams) -> ChatInfo,
        /// Moves a chat's tab before another chat of its workspace, or last; every client shows the
        /// tabs in that order (`ChatInfo::tab_position`).
        #[serde(rename = "chat.move")]
        #[access(Any)]
        #[relay(Act)]
        ChatMove(MoveChatParams) -> ChatInfo,
        /// Takes a chat out of the workspace's tabs, its conversation kept, and stops its agent
        /// session. Refused while its agent works unless `force` (which stops it first). Closing the
        /// last open chat of a workspace opens a new, empty one.
        #[serde(rename = "chat.close")]
        #[access(Any)]
        #[relay(Act)]
        ChatClose(CloseChatParams) -> ChatInfo,
        /// Puts a closed chat back among the tabs; its agent resumes at its next message.
        #[serde(rename = "chat.reopen")]
        #[access(Any)]
        #[relay(Act)]
        ChatReopen(ChatParams) -> ChatInfo,
        /// The user is back (the app's window took the focus): every idle chat whose last turn failed
        /// on an error that trying again could cure (no network, an overloaded API) resumes it, once
        /// per message of the user's. Returns at once.
        #[serde(rename = "chat.resume_failed")]
        #[access(Any)]
        #[relay(Local)]
        ChatResumeFailed(EmptyParams) -> EmptyParams,
        /// Deletes a chat and its conversation for good (named by its id only). Same rules as
        /// `chat.close`; the files it saved in the notes folder stay.
        #[serde(rename = "chat.delete")]
        #[access(Any)]
        #[relay(Act)]
        ChatDelete(CloseChatParams) -> ChatInfo,
        /// Forks a chat into a new tab of its workspace: the same agent and settings, the
        /// conversation so far, or as it stood at the end of one of its turns (copied from the
        /// agent's own session, which the fork's agent resumes as a session of its own), and from
        /// there each goes its own way. Into the other agent, the event log alone is copied and the
        /// fork's agent reads the conversation as text. The whole conversation is refused while the
        /// agent runs a turn or waits for the user; a design chat is never forked.
        #[serde(rename = "chat.fork")]
        #[access(Any)]
        #[relay(Act)]
        ChatFork(ChatForkParams) -> ChatInfo,
        /// Writes a design chat's design brief again from its source chat, until its first message.
        #[serde(rename = "chat.regenerate_brief")]
        #[access(Any)]
        #[relay(Act)]
        ChatRegenerateBrief(ChatParams) -> ChatInfo,
        /// A design chat's canvas: its pages, sections and screens as `canvas.json` lays them out,
        /// with the URLs the screens are served at (an origin of their own, story 47).
        #[serde(rename = "design.canvas")]
        #[access(Any, phone)]
        #[relay(Local)]
        DesignCanvas(ChatParams) -> CanvasResult,
        /// Reads one public-type resource strictly inside a local design folder (256 KiB chunks, 16 MiB per file).
        #[serde(rename = "design.resource")]
        #[access(User, phone)]
        #[relay(Local)]
        DesignResource(DesignResourceParams) -> DesignResource,
        /// Preview an independent copy of a public Claude Design canvas.
        #[serde(rename = "design.claude_preview")]
        #[access(User)]
        #[relay(Local)]
        DesignClaudePreview(ClaudeArtifactParams) -> ClaudeArtifactPreview,
        /// A design chat's screens as one PNG, in a row with their labels, rendered by a headless
        /// Chrome of the machine and kept in the design folder's `exports/`; copied to the clipboard
        /// where the machine allows (story 47).
        #[serde(rename = "design.export")]
        #[access(Any)]
        #[relay(Local)]
        DesignExport(DesignExportParams) -> DesignExportResult,
        /// A design chat's comments (story 51), read from its `comments.json`, with who the user is
        /// to them (the author of the ones they add).
        #[serde(rename = "design.comments")]
        #[access(Any)]
        #[relay(Local)]
        DesignComments(ChatParams) -> CommentsResult,
        #[serde(rename = "design.comment_add")]
        #[access(Any)]
        #[relay(Local)]
        DesignCommentAdd(CommentAddParams) -> DesignComment,
        /// A comment's text, by its author, until it is sent to the agent.
        #[serde(rename = "design.comment_edit")]
        #[access(Any)]
        #[relay(Local)]
        DesignCommentEdit(CommentEditParams) -> DesignComment,
        /// Removes a comment and its replies, by its author, until it is sent to the agent.
        #[serde(rename = "design.comment_delete")]
        #[access(Any)]
        #[relay(Local)]
        DesignCommentDelete(CommentParams) -> EmptyParams,
        /// Resolves or reopens a comment (anyone).
        #[serde(rename = "design.comment_resolve")]
        #[access(Any)]
        #[relay(Local)]
        DesignCommentResolve(CommentResolveParams) -> DesignComment,
        /// The message asking the agent to address the comments not sent yet, or one thread, which
        /// it marks sent.
        #[serde(rename = "design.comments_send")]
        #[access(Any)]
        #[relay(Local)]
        DesignCommentsSend(CommentsSendParams) -> CommentsSendResult,
        /// A design chat's share (story 51): its state, as the Share dialog shows it.
        #[serde(rename = "design.share_state")]
        #[access(Any)]
        #[relay(Local)]
        DesignShareState(ChatParams) -> DesignShareState,
        /// Shares a design chat with named reviewers: a new link each time.
        #[serde(rename = "design.share_start")]
        #[access(Any)]
        #[relay(Local)]
        DesignShareStart(ShareStartParams) -> DesignShareState,
        /// The reviewers of a live share, and whether the team sees it.
        #[serde(rename = "design.share_update")]
        #[access(Any)]
        #[relay(Local)]
        DesignShareUpdate(ShareStartParams) -> DesignShareState,
        /// A shared design taken over: its files into a new design chat of a workspace (story 51).
        #[serde(rename = "design.import")]
        #[access(Any)]
        #[relay(Local)]
        DesignImport(DesignImportParams) -> ChatInfo,
        /// Stops a share: the hosted copy is removed and its link dies; the comments stay local.
        #[serde(rename = "design.share_stop")]
        #[access(Any)]
        #[relay(Local)]
        DesignShareStop(ChatParams) -> DesignShareState,
        /// The live designs colleagues shared with the user or with the team, for the Office's
        /// Design window.
        #[serde(rename = "design.shared")]
        #[access(Any, office)]
        #[relay(Local)]
        DesignShared(EmptyParams) -> SharedDesignsResult,
        /// The user's mentions in the comments of shares open to them, newest first, as the share
        /// service last told them (docs/adr/mentions-in-share-comments.md).
        #[serde(rename = "mentions.list")]
        #[access(Any)]
        #[relay(Local)]
        MentionsList(EmptyParams) -> MentionsResult,
        /// The user opened a mention's thread: it is no longer unread, here and on the service.
        #[serde(rename = "mentions.seen")]
        #[access(Any)]
        #[relay(Local)]
        MentionsSeen(MentionSeenParams) -> MentionsResult,
        /// A workspace's share (story 53): its state, as the Share workspace dialog shows it.
        #[serde(rename = "workspace.share_state")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceShareState(WorkspaceParams) -> WorkspaceShareState,
        /// Shares a workspace's chats and notes with named guests, each a viewer or able to suggest:
        /// a new link each time.
        #[serde(rename = "workspace.share_start")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceShareStart(WorkspaceShareStartParams) -> WorkspaceShareState,
        /// The guests of a live share and their roles, and whether the team sees it.
        #[serde(rename = "workspace.share_update")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceShareUpdate(WorkspaceShareStartParams) -> WorkspaceShareState,
        /// Stops a workspace share: the hosted copy is removed and its link dies.
        #[serde(rename = "workspace.share_stop")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceShareStop(WorkspaceParams) -> WorkspaceShareState,
        /// What importing a colleague's workspace share brings (its title, owner, chats and
        /// notes), read with the user's identity as the import reads it, before creating anything.
        #[serde(rename = "workspace.share_preview")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceSharePreview(WorkspaceSharePreviewParams) -> WorkspaceSharePreview,
        /// A chat's suggestions from the guests of its workspace's share, the decided ones too.
        #[serde(rename = "chat.suggestions")]
        #[access(Any)]
        #[relay(Local)]
        ChatSuggestions(ChatParams) -> SuggestionsResult,
        /// Dismisses a pending suggestion: the guests see it dismissed, nothing goes to the agent.
        #[serde(rename = "chat.suggestion_dismiss")]
        #[access(Any)]
        #[relay(Local)]
        ChatSuggestionDismiss(SuggestionParams) -> SuggestionsResult,
        /// The comments the guests of its workspace's share made on a chat's messages.
        #[serde(rename = "chat.share_comments")]
        #[access(Any)]
        #[relay(Local)]
        ChatShareComments(ChatParams) -> ShareCommentsResult,
        /// The owner comments on a passage of an agent's message of a shared chat, or replies to a
        /// comment, for the guests to read (never the agent); the share service writes it.
        #[serde(rename = "chat.share_comment_add")]
        #[access(Any)]
        #[relay(Local)]
        ChatShareCommentAdd(ShareCommentAddParams) -> ShareCommentsResult,
        /// The owner deletes a comment of theirs, a thread's first one with its replies.
        #[serde(rename = "chat.share_comment_delete")]
        #[access(Any)]
        #[relay(Local)]
        ChatShareCommentDelete(ShareCommentParams) -> ShareCommentsResult,
        #[serde(rename = "events.subscribe")]
        #[access(Any, phone, office)]
        #[relay(Local)]
        EventsSubscribe(SubscribeParams) -> SubscribeResult,
        #[serde(rename = "events.unsubscribe")]
        #[access(Any, phone, office)]
        #[relay(Local)]
        EventsUnsubscribe(UnsubscribeParams) -> EmptyParams,
        #[serde(rename = "events.history")]
        #[access(Any, phone)]
        #[relay(Read)]
        EventsHistory(HistoryParams) -> HistoryResult,
        #[serde(rename = "diff.get")]
        #[access(Any)]
        #[relay(Read)]
        DiffGet(DiffParams) -> DiffResult,
        /// Every file of the workspace's repos (tracked, and new ones git does not ignore) and of its
        /// notes folder.
        #[serde(rename = "files.list")]
        #[access(Any)]
        #[relay(Read)]
        FilesList(WorkspaceParams) -> FilesListResult,
        #[serde(rename = "files.read")]
        #[access(Any)]
        #[relay(Read)]
        FilesRead(FileReadParams) -> FileContent,
        /// Replaces an existing file of a repo or of the notes folder; refused in an exploration.
        #[serde(rename = "files.write")]
        #[access(Any)]
        #[relay(Act)]
        FilesWrite(FileWriteParams) -> FileContent,
        /// A markdown file's comments (story 57), kept in the workspace's `.chauffeur/comments.json`,
        /// with who the user is to them and the chat they are sent to by default.
        #[serde(rename = "files.comments")]
        #[access(Any)]
        #[relay(Local)]
        FilesComments(FileCommentsParams) -> FileCommentsResult,
        /// A comment on a passage of a markdown file, or a reply to one of its threads.
        #[serde(rename = "files.comment_add")]
        #[access(Any)]
        #[relay(Local)]
        FilesCommentAdd(FileCommentAddParams) -> DesignComment,
        /// A file comment's text, by its author, until it is sent to an agent.
        #[serde(rename = "files.comment_edit")]
        #[access(Any)]
        #[relay(Local)]
        FilesCommentEdit(FileCommentEditParams) -> DesignComment,
        /// Removes a file comment and its replies, by its author, until it is sent to an agent.
        #[serde(rename = "files.comment_delete")]
        #[access(Any)]
        #[relay(Local)]
        FilesCommentDelete(FileCommentParams) -> EmptyParams,
        /// Resolves or reopens a file comment's thread (anyone).
        #[serde(rename = "files.comment_resolve")]
        #[access(Any)]
        #[relay(Local)]
        FilesCommentResolve(FileCommentResolveParams) -> DesignComment,
        /// The message asking an agent to address the file's comments not sent yet, which it marks
        /// sent; the client sends it to the chat it picks.
        #[serde(rename = "files.comments_send")]
        #[access(Any)]
        #[relay(Local)]
        FilesCommentsSend(FileCommentsParams) -> CommentsSendResult,
        /// A markdown file's share (story 57): its state, as the Share dialog shows it.
        #[serde(rename = "files.share_state")]
        #[access(Any)]
        #[relay(Local)]
        FilesShareState(FileCommentsParams) -> DesignShareState,
        /// Shares a markdown file alone with named reviewers: a new link each time.
        #[serde(rename = "files.share_start")]
        #[access(Any)]
        #[relay(Local)]
        FilesShareStart(FileShareStartParams) -> DesignShareState,
        /// The reviewers of a markdown file's live share, and whether the team sees it.
        #[serde(rename = "files.share_update")]
        #[access(Any)]
        #[relay(Local)]
        FilesShareUpdate(FileShareStartParams) -> DesignShareState,
        /// Stops a markdown file's share: the hosted copy is removed, the comments stay local.
        #[serde(rename = "files.share_stop")]
        #[access(Any)]
        #[relay(Local)]
        FilesShareStop(FileCommentsParams) -> DesignShareState,
        #[serde(rename = "files.search")]
        #[access(Any)]
        #[relay(Read)]
        FilesSearch(FilesSearchParams) -> FilesSearchResult,
        /// Opens a video, an audio file or a PDF of the workspace in the system's default app for
        /// it; any other file is refused.
        #[serde(rename = "files.open")]
        #[access(Any)]
        #[relay(Here)]
        FilesOpen(FileReadParams) -> EmptyParams,
        /// Shows a file of the workspace, of any type, selected in Finder (its folder in the file
        /// manager on Linux): nothing is opened.
        #[serde(rename = "files.reveal")]
        #[access(Any)]
        #[relay(Here)]
        FilesReveal(FileReadParams) -> EmptyParams,
        #[serde(rename = "terminal.open")]
        #[access(User)]
        #[relay(Act)]
        TerminalOpen(OpenTerminalParams) -> TerminalInfo,
        #[serde(rename = "terminal.input")]
        #[access(User)]
        #[relay(Act)]
        TerminalInput(TerminalInputParams) -> EmptyParams,
        #[serde(rename = "terminal.resize")]
        #[access(User)]
        #[relay(Read)]
        TerminalResize(ResizeTerminalParams) -> TerminalInfo,
        #[serde(rename = "terminal.close")]
        #[access(User)]
        #[relay(Act)]
        TerminalClose(TerminalParams) -> TerminalInfo,
        #[serde(rename = "terminal.list")]
        #[access(User)]
        #[relay(Read)]
        TerminalList(WorkspaceParams) -> TerminalsResult,
        #[serde(rename = "terminal.snapshot")]
        #[access(User)]
        #[relay(Read)]
        TerminalSnapshot(TerminalParams) -> TerminalSnapshotResult,
        /// Moves a terminal's tab before another terminal of its workspace, or last; returns the
        /// workspace's terminals in their new order, the order `terminal.list` gives from then on.
        #[serde(rename = "terminal.move")]
        #[access(User)]
        #[relay(Act)]
        TerminalMove(MoveTerminalParams) -> TerminalsResult,
        /// Names a terminal's tab; empty gives back its title.
        #[serde(rename = "terminal.rename")]
        #[access(User)]
        #[relay(Act)]
        TerminalRename(RenameTerminalParams) -> TerminalInfo,
        /// The folder a terminal's shell is in now (where it started, once it exited).
        #[serde(rename = "terminal.cwd")]
        #[access(User)]
        #[relay(Read)]
        TerminalCwd(TerminalParams) -> TerminalCwdResult,
        #[serde(rename = "pr.status")]
        #[access(Any, office)]
        #[relay(Read)]
        PrStatus(WorkspaceParams) -> PrStatusResult,
        #[serde(rename = "pr.create")]
        #[access(Any)]
        #[relay(Act)]
        PrCreate(CreatePrParams) -> ChatInfo,
        /// Merges the workspace's open pull requests, one repo after the other (`gh pr merge`).
        #[serde(rename = "pr.merge")]
        #[access(Any)]
        #[relay(Act)]
        PrMerge(MergePrsParams) -> MergePrsResult,
        /// The user's pull requests merged on GitHub since a moment, of every repository but
        /// Chauffeur's own, newest first, each with the tickets it was made for; with `team`, those
        /// of everyone in the GitHub organization of the settings, and their open ones.
        #[serde(rename = "pr.shipped")]
        #[access(Any, office)]
        #[relay(Local)]
        PrShipped(ShippedParams) -> ShippedResult,
        #[serde(rename = "linear.search")]
        #[access(Any, office)]
        #[relay(Local)]
        LinearSearch(LinearSearchParams) -> LinearSearchResult,
        #[serde(rename = "linear.issue")]
        #[access(Any, office)]
        #[relay(Local)]
        LinearIssue(LinearIssueParams) -> LinearIssue,
        /// The workflow states of the issue's team, in Linear's order.
        #[serde(rename = "linear.states")]
        #[access(Any, office)]
        #[relay(Local)]
        LinearStates(LinearIssueParams) -> LinearStatesResult,
        /// Moves the issue to one of its team's states; returns it as Linear then says.
        #[serde(rename = "linear.set_state")]
        #[access(Any)]
        #[relay(Local)]
        LinearSetState(LinearSetStateParams) -> LinearIssue,
        /// The Office's news kiosk: Hacker News and the user's feeds, each fetched again first when
        /// its last fetch is 15 minutes old.
        #[serde(rename = "news.state")]
        #[access(Any)]
        #[relay(Local)]
        NewsState(EmptyParams) -> NewsState,
        #[serde(rename = "news.mark_read")]
        #[access(Any)]
        #[relay(Local)]
        NewsMarkRead(NewsMarkReadParams) -> EmptyParams,
        /// Adds an RSS or Atom feed to `[news] feeds` of settings.toml; returns the state with it read.
        #[serde(rename = "news.add_feed")]
        #[access(Any)]
        #[relay(Local)]
        NewsAddFeed(NewsFeedParams) -> NewsState,
        #[serde(rename = "news.remove_feed")]
        #[access(Any)]
        #[relay(Local)]
        NewsRemoveFeed(NewsFeedParams) -> NewsState,
        /// Shows or hides Hacker News at the kiosk (`[news] hacker_news`).
        #[serde(rename = "news.show_hacker_news")]
        #[access(Any)]
        #[relay(Local)]
        NewsShowHackerNews(NewsShowParams) -> NewsState,
        /// The models each agent offers, from the agents' own catalogs (cached by the daemon).
        #[serde(rename = "models.list")]
        #[access(Any, phone)]
        #[relay(Read)]
        ModelsList(ModelsListParams) -> ModelsListResult,
        /// What is left of each agent's plan limits, read at most every few minutes.
        #[serde(rename = "usage.get")]
        #[access(Any)]
        #[relay(Local)]
        UsageGet(ModelsListParams) -> UsageResult,
        /// Starts logging an agent's CLI back in to its account: open the result's URL; a
        /// `login_ended` event follows, and `account_changed` once the login completes.
        #[serde(rename = "agent.login")]
        #[access(Any)]
        #[relay(Local)]
        AgentLogin(AgentLoginParams) -> AgentLoginResult,
        /// Passes the code a headless login's page showed on to the agent's CLI.
        #[serde(rename = "agent.login_code")]
        #[access(Any)]
        #[relay(Local)]
        AgentLoginCode(crate::sidecar::AgentLoginCodeParams) -> (),
        /// Ends the agent's login under way, reported as not completed.
        #[serde(rename = "agent.login_cancel")]
        #[access(Any)]
        #[relay(Local)]
        AgentLoginCancel(AgentLoginParams) -> (),
        /// The commands a chat of this agent runs with `/name` in this workspace: skills and commands
        /// (the user's, the plugins', each repo's) and the agent's own that work in a chat.
        #[serde(rename = "commands.list")]
        #[access(Any)]
        #[relay(Read)]
        CommandsList(CommandsListParams) -> CommandsListResult,
        /// The same, before the workspace exists: what a chat would run with `/name` in a workspace of
        /// these repos, read from their local clones (a repo not cloned yet adds nothing).
        #[serde(rename = "commands.for_repos")]
        #[access(Any)]
        #[relay(Local)]
        CommandsForRepos(CommandsForReposParams) -> CommandsListResult,
        /// The user's favorite model settings, in picker order.
        #[serde(rename = "loadout.get")]
        #[access(Any)]
        #[relay(Local)]
        LoadoutGet(EmptyParams) -> LoadoutParams,
        #[serde(rename = "loadout.set")]
        #[access(Any)]
        #[relay(Local)]
        LoadoutSet(LoadoutParams) -> LoadoutParams,
        /// The user looked at the workspace: it is no longer unread.
        #[serde(rename = "workspace.seen")]
        #[access(Any, phone)]
        #[relay(Read)]
        WorkspaceSeen(WorkspaceParams) -> EmptyParams,
        /// The apps a workspace or worktree folder can be opened in.
        #[serde(rename = "open.apps")]
        #[access(Any)]
        #[relay(Local)]
        OpenApps(EmptyParams) -> OpenAppsResult,
        /// Opens a workspace folder, or one of its worktrees, in an editor, a terminal or the file manager.
        #[serde(rename = "workspace.open")]
        #[access(Any)]
        #[relay(Local)]
        WorkspaceOpen(OpenParams) -> OpenResult,
        /// Individual Slack sign-in and imports; credentials stay with the daemon.
        #[serde(rename = "slack.status")]
        #[access(User)]
        #[relay(Local)]
        SlackStatus(EmptyParams) -> SlackStatus,
        #[serde(rename = "slack.login")]
        #[access(User)]
        #[relay(Local)]
        SlackLogin(EmptyParams) -> SlackLogin,
        #[serde(rename = "slack.poll")]
        #[access(User)]
        #[relay(Local)]
        SlackPoll(EmptyParams) -> SlackStatus,
        #[serde(rename = "slack.cancel")]
        #[access(User)]
        #[relay(Local)]
        SlackCancel(EmptyParams) -> (),
        #[serde(rename = "slack.logout")]
        #[access(User)]
        #[relay(Local)]
        SlackLogout(EmptyParams) -> (),
        #[serde(rename = "slack.context")]
        #[access(User)]
        #[relay(Local)]
        SlackContext(SlackContextParams) -> SlackContext,
        /// Receives Slack handoffs while the local desktop is open; acknowledges only after prefill.
        #[serde(rename = "slack.receive")]
        #[access(User)]
        #[relay(Local)]
        SlackReceive(SlackReceiveParams) -> SlackDelivery,
        /// The settings changed from the app (the others are in `~/.chauffeur/settings.toml`).
        #[serde(rename = "settings.get")]
        #[access(Any)]
        #[relay(Local)]
        SettingsGet(EmptyParams) -> AppSettings,
        #[serde(rename = "settings.set")]
        #[access(User)]
        #[relay(Local)]
        SettingsSet(Box<SettingsUpdate>) -> AppSettings,
        /// Opens `settings.toml` in the user's editor, written first with every key and its default
        /// when it does not exist yet.
        #[serde(rename = "settings.open")]
        #[access(User)]
        #[relay(Local)]
        SettingsOpen(EmptyParams) -> EmptyParams,
        /// Whether Chauffeur keeps the machine awake now, and why (`keep_awake`).
        #[serde(rename = "power.get")]
        #[access(Any)]
        #[relay(Local)]
        PowerGet(EmptyParams) -> PowerState,
        /// Reports an error toast of the app to Chauffeur's Sentry, with the daemon's side of the
        /// failure when the toast came from one (`error_reports`).
        #[serde(rename = "errors.report")]
        #[access(User)]
        #[relay(Local)]
        ErrorsReport(ErrorReportParams) -> ErrorReportOutcome,
        /// Whether a newer packaged Chauffeur is published, and where its installation stands.
        #[serde(rename = "update.status")]
        #[access(Any)]
        #[relay(Local)]
        UpdateStatus(EmptyParams) -> UpdateStatus,
        /// Asks GitHub for a newer version now, then answers as `update.status`. The daemon also
        /// looks on its own every 10 minutes.
        #[serde(rename = "update.check")]
        #[access(Any)]
        #[relay(Local)]
        UpdateCheck(EmptyParams) -> UpdateStatus,
        /// Downloads the newer version, checks it and puts it in place of this app; returns at once,
        /// `update.status` follows it. Agents keep working meanwhile: only the restart onto the new
        /// version interrupts them, and the app asks before that.
        #[serde(rename = "update.install")]
        #[access(User)]
        #[relay(Local)]
        UpdateInstall(EmptyParams) -> UpdateStatus,
        /// Ollama, which serves the local models: installed or not, running or not, the models on
        /// disk and the pulls in progress. Starts nothing.
        #[serde(rename = "ollama.status")]
        #[access(Any)]
        #[relay(Local)]
        OllamaStatus(EmptyParams) -> OllamaStatus,
        /// Downloads, checks and unpacks Chauffeur's own Ollama (macOS and Linux) in the background;
        /// returns at once, `ollama.status` follows it. Refused elsewhere, or while one runs.
        #[serde(rename = "ollama.install")]
        #[access(User)]
        #[relay(Local)]
        OllamaInstall(EmptyParams) -> EmptyParams,
        /// Downloads a model into Ollama in the background (starting Ollama if need be); returns at
        /// once, `ollama.status` follows it. Refused while the same model is being pulled.
        #[serde(rename = "ollama.pull")]
        #[access(Any)]
        #[relay(Local)]
        OllamaPull(OllamaModelParams) -> EmptyParams,
        /// Deletes a model from Ollama's disk.
        #[serde(rename = "ollama.delete")]
        #[access(Any)]
        #[relay(Local)]
        OllamaDelete(OllamaModelParams) -> EmptyParams,
        /// GitHub as Chauffeur reaches it: git found or not, `gh` found or not, the account it is
        /// signed in to and that account's organizations, and a connection under way.
        #[serde(rename = "github.status")]
        #[access(Any)]
        #[relay(Local)]
        GithubStatus(EmptyParams) -> GithubStatus,
        /// Signs `gh` in to GitHub in the browser, installing Chauffeur's own `gh` first when none
        /// is found, then lets git push over HTTPS through it; returns at once, `github.status`
        /// follows it (the one-time code to enter on GitHub, then the account). Refused while one runs.
        #[serde(rename = "github.connect")]
        #[access(User)]
        #[relay(Local)]
        GithubConnect(EmptyParams) -> EmptyParams,
        /// Ends the connection under way, reported as not completed.
        #[serde(rename = "github.cancel")]
        #[access(User)]
        #[relay(Local)]
        GithubCancel(EmptyParams) -> EmptyParams,
        /// Signs `gh` out of github.com.
        #[serde(rename = "github.disconnect")]
        #[access(User)]
        #[relay(Local)]
        GithubDisconnect(EmptyParams) -> EmptyParams,
        /// Opens Apple's installer of the command line developer tools, which bring git (macOS);
        /// `github.status` says once git is there. Refused elsewhere, with the command to run.
        #[serde(rename = "git.install")]
        #[access(User)]
        #[relay(Local)]
        GitInstall(EmptyParams) -> EmptyParams,
        /// Reads the login shell's environment again (a variable exported since the daemon started)
        /// and gives it to everything started afterwards: agents, terminals, scripts. Idle chats
        /// with a broken MCP server that takes a value from a variable it set or changed start again
        /// with it; busy ones get it at their next session start. Never interrupts a turn.
        #[serde(rename = "env.reload")]
        #[access(Any)]
        #[relay(Local)]
        EnvReload(EmptyParams) -> EnvReloadResult,
        #[serde(rename = "daemon.shutdown")]
        #[access(User)]
        #[relay(Local)]
        DaemonShutdown(EmptyParams) -> EmptyParams,
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RemoteApproveParams {
    pub peer_id: String,
    pub name: String,
    pub relay: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RemoteDeviceParams {
    pub device_id: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RemoteDeviceInfo {
    pub id: String,
    pub peer_id: String,
    pub name: String,
    pub relay: String,
    pub created_at: String,
}

/// Host-only secrets. Do not emit these through events or mobile RPC forwarding.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RemoteDevice {
    pub info: RemoteDeviceInfo,
    pub host_token: String,
    pub phone_token: String,
    pub secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub push: Option<PushRegistration>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RemoteStateResult {
    pub identity_seed: Vec<u8>,
    pub devices: Vec<RemoteDevice>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RemoteDevicesResult {
    pub devices: Vec<RemoteDeviceInfo>,
}

methods! {
    /// Notifications the daemon pushes to a subscribed client.
    pub enum DaemonNotification / DaemonNotificationMethod {
        #[serde(rename = "event")]
        Event(EventNotification),
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct EventNotification {
    pub subscription: String,
    pub envelope: EventEnvelope,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct EmptyParams {}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct HelloParams {
    pub protocol_version: u32,
    /// Free-form client name for logs, e.g. `chauffeur-cli 0.1.0`.
    pub client: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct HelloResult {
    /// The protocol spoken on this connection: the client's, once the daemon accepted it.
    pub protocol_version: u32,
    /// The oldest client protocol the daemon serves; 0 from a daemon older than the field.
    #[serde(default)]
    pub min_protocol_version: u32,
    pub daemon_version: String,
    pub home: String,
    pub workspaces_dir: String,
    /// The user's login shell, whose environment every spawn gets (`/bin/zsh`).
    #[serde(default)]
    pub login_shell: String,
    /// The file of that shell where a variable is best exported for Chauffeur to get it
    /// (`~/.zshenv`, `~/.bash_profile`).
    #[serde(default)]
    pub shell_profile: String,
}

/// What reading the login shell's environment again changed. Variables by name only.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default, PartialEq)]
#[ts(export)]
pub struct EnvReloadResult {
    /// The login shell read (`/bin/zsh`).
    pub shell: String,
    /// The file of that shell where a variable is best exported (`~/.zshenv`).
    pub profile: String,
    /// Variables that appeared, changed value, or went.
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    /// Variables a broken MCP server of a running chat needs that are still not set.
    pub still_missing: Vec<String>,
    /// Chats whose idle session was started again with the new environment, for their broken
    /// MCP servers to connect: those with a server that takes a value from a variable that was
    /// set or changed (`McpServer::uses_env`).
    pub restarted: Vec<String>,
    /// Chats with such a server whose agent is busy: it gets the new environment at its next
    /// session start (its next message once idle).
    pub deferred: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ReposListParams {
    /// Re-read the organization's repos from GitHub instead of the cached list.
    #[serde(default)]
    pub refresh: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ReposListResult {
    pub repos: Vec<RepoInfo>,
    /// When GitHub last listed the repos (RFC 3339); none when no GitHub owner is listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<String>,
    /// Why GitHub could not list them the last time it was asked: `repos` then holds the cached
    /// ones and the clones found on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_error: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct CreateWorkspaceParams {
    /// Copy the chats and notes of this share into independent local sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_share: Option<String>,
    /// Repo names; empty means the user's default set.
    #[serde(default)]
    pub repos: Vec<String>,
    /// Design chats only: start without repos, ignoring defaults and ticket repos.
    #[serde(default)]
    pub no_repos: bool,
    /// Linear identifiers to attach.
    #[serde(default)]
    pub tickets: Vec<String>,
    /// A Linear project milestone (id, URL or name) whose issues are attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    /// Slug override; derived from the tickets otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Branch override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Makes this a child workspace of the given workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Starts a first chat with this prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Kind of the first chat; a design chat opens even without a prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_kind: Option<ChatKind>,
    /// Images sent with the first prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ImageInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    /// Return as soon as the workspace exists, in `creating` state; worktrees, setup and the
    /// first chat follow, reported by `state_changed` (an error ends in `archived` with it).
    #[serde(default)]
    pub background: bool,
    /// An exploration: each repo checked out at its base branch (no branch of its own, no setup
    /// script, no ports), and chats that cannot change anything.
    #[serde(default)]
    pub explore: bool,
    /// A design brought into the workspace's context: copied into a design chat of its own, its
    /// folder named in the first prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design: Option<WorkspaceDesign>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ClaudeArtifactParams {
    pub url: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ClaudeArtifactPreview {
    pub title: String,
    pub pages: Vec<String>,
    pub screens: u32,
    pub warnings: Vec<String>,
}

/// The design a new workspace starts with: a colleague's share, or one of the user's design chats.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum WorkspaceDesign {
    /// An independent copy of a public Claude Design canvas.
    Claude { url: String },
    /// `https://<share service>/s/<id>`, read as a take-over reads it.
    Share { url: String },
    /// A design chat of another workspace of the user's (any id or prefix `chat.*` takes).
    Chat { chat: String },
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct CreateWorkspaceResult {
    pub workspace: WorkspaceInfo,
    pub chat: Option<ChatInfo>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ListWorkspacesParams {
    #[serde(default)]
    pub include_archived: bool,
    /// Only the children of this workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct WorkspacesResult {
    /// In the sidebar's order: pinned, then by section, then the others; archived ones last.
    pub workspaces: Vec<WorkspaceInfo>,
    /// The sidebar's sections, in order.
    #[serde(default)]
    pub sections: Vec<SectionInfo>,
}

/// Full-text search of archived conversations; whitespace-separated words must all match a message.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SearchArchivedParams {
    pub query: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ArchivedSearchHit {
    pub workspace_id: String,
    pub chat_id: String,
    pub chat_title: Option<String>,
    #[ts(type = "number")]
    pub seq: i64,
    /// SQLite's excerpt; U+0001 / U+0002 delimit matched words, never HTML.
    pub snippet: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ArchivedSearchResult {
    pub hits: Vec<ArchivedSearchHit>,
}

/// A search of every workspace (`search.query`). Words match by prefix, ignoring case and accents;
/// a "quoted phrase" matches as written.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SearchParams {
    pub query: String,
    #[serde(default)]
    pub filters: SearchFilters,
}

/// What narrows a search; anything left out narrows nothing.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default, PartialEq)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SearchFilters {
    /// Workspaces with a worktree of one of these repos.
    #[serde(default)]
    pub repos: Vec<String>,
    /// Workspaces attached to one of these tickets (`XYZ-1374`).
    #[serde(default)]
    pub tickets: Vec<String>,
    #[serde(default)]
    pub state: Option<SearchState>,
    #[serde(default)]
    pub pr: Option<SearchPr>,
    /// Only what is dated at or after this instant (RFC 3339).
    #[serde(default)]
    pub since: Option<String>,
    /// Only what is dated before this instant (RFC 3339).
    #[serde(default)]
    pub until: Option<String>,
    /// Where to look; empty: everywhere.
    #[serde(default)]
    pub sources: Vec<SearchSource>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SearchState {
    Active,
    /// Ordered by archive date first, the newest archive first (docs/adr/0041-archive-search-order.md).
    Archived,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SearchPr {
    /// Workspaces with a pull request, in any state.
    Any,
    /// Workspaces with a merged pull request.
    Merged,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SearchSource {
    /// Workspaces' titles and tickets, and pull requests' titles.
    Titles,
    /// What the user and the agents wrote in chats (not thinking, tool calls nor subagents).
    Messages,
    /// The markdown files of workspaces' notes folders.
    Notes,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SearchHitKind {
    Message,
    Note,
    Workspace,
    PullRequest,
}

/// How a hit was found.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SearchMatchedBy {
    Keywords,
    Meaning,
    Both,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct SearchHit {
    pub workspace_id: String,
    pub kind: SearchHitKind,
    /// A message's chat and its place in the event log.
    pub chat_id: Option<String>,
    pub chat_title: Option<String>,
    #[ts(type = "number | null")]
    pub seq: Option<i64>,
    /// A note's path in the workspace folder (`.chauffeur/plan.md`).
    pub path: Option<String>,
    /// A pull request's page.
    pub url: Option<String>,
    /// A pull request's or a note's title, a workspace's name.
    pub title: Option<String>,
    /// An excerpt; U+0001 / U+0002 delimit matched words, never HTML.
    pub snippet: String,
    /// When it was written (RFC 3339).
    pub at: String,
    pub matched: SearchMatchedBy,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct SearchResult {
    /// The best first, at most 50.
    pub hits: Vec<SearchHit>,
    pub status: SearchStatus,
}

/// Search by meaning (`search.status`): keywords always work.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct SearchStatus {
    pub semantic: SemanticState,
    /// The embedding model Ollama serves (`[search] embedding_model`).
    pub model: String,
    /// Pieces of text given their meaning, and in all; the history is indexed in the background.
    #[ts(type = "number")]
    pub embedded: u64,
    #[ts(type = "number")]
    pub total: u64,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SemanticState {
    /// Turned off in the settings (`[search] semantic = false`).
    Disabled,
    /// No Ollama found: install it from Settings › Local models.
    NoOllama,
    /// The embedding model is not downloaded.
    NoModel,
    /// Ollama is starting for a search: ask again in a moment.
    Starting,
    /// On battery, the history waits for the charger (what Ollama already running can do is done).
    OnBattery,
    Ready,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct WorkspaceParams {
    pub workspace: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct RetitleWorkspaceParams {
    pub workspace: String,
    /// Only the last N developer/assistant messages across all chats; omitted uses the whole conversation.
    #[serde(default)]
    pub last_messages: Option<usize>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RemindLaterParams {
    pub workspaces: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SetTicketsParams {
    pub workspace: String,
    /// Identifiers (`ABC-3195`, any case), in order; empty detaches every ticket.
    pub tickets: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SetTicketsResult {
    pub workspace: WorkspaceInfo,
    /// What could not be done: a ticket Linear did not return (attached by identifier only),
    /// Linear turned off or without a key, a move to In Progress that failed.
    pub warnings: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct PinWorkspaceParams {
    pub workspace: String,
    pub pinned: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct MoveWorkspaceParams {
    pub workspace: String,
    /// The workspace it goes above; last of its group when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// The group it goes to: the pinned workspaces or the others.
    pub pinned: bool,
    /// When not pinned, the section it goes to (id or name); absent: no section. A pinned
    /// workspace keeps its section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct MoveHouseParams {
    pub workspace: String,
    /// The workspace whose house it goes before; last when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SetSectionParams {
    pub workspace: String,
    /// Section id or name; null: no section.
    pub section: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SectionsResult {
    pub sections: Vec<SectionInfo>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct CreateSectionParams {
    pub name: String,
    /// A workspace to file in the new section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

/// Names a section by its id or its name (any case).
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SectionParams {
    pub section: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RenameSectionParams {
    pub section: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct FoldSectionParams {
    pub section: String,
    pub folded: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct MoveSectionParams {
    pub section: String,
    /// The section it goes above; last when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct AddRepoParams {
    pub workspace: String,
    pub repo: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct PromoteParams {
    pub workspace: String,
    /// The user agreed to lose what the exploration left in its checkouts: each one is reset to
    /// its commit and cleaned (`git reset --hard`, `git clean -ffdx`) before it gets its branch.
    #[serde(default)]
    pub discard: bool,
}

/// The data of a promote refused because checkouts hold files their commit does not: what the
/// exploration's commands (or the user, in its terminal) left there.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct PromoteLeftovers {
    pub repos: Vec<RepoLeftovers>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct RepoLeftovers {
    pub repo: String,
    /// Paths in the checkout, changed, new or ignored; a folder git does not track whole appears
    /// once, with a trailing `/`.
    pub files: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ArchiveParams {
    pub workspace: String,
    /// Archive even while an agent is working.
    #[serde(default)]
    pub force: bool,
    /// Return at once, the workspace `archiving`; the outcome comes as `state_changed`.
    #[serde(default)]
    pub background: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct WaitParams {
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct WaitResult {
    pub workspace: WorkspaceInfo,
    pub chats: Vec<ChatInfo>,
    /// Last assistant text of the workspace's first chat.
    pub last_text: Option<String>,
    pub timed_out: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct AddWorktreeParams {
    pub workspace: String,
    /// Repos to create a worktree for; empty means every repo of the workspace.
    #[serde(default)]
    pub repos: Vec<String>,
    /// Name of the group, e.g. the Claude subagent's worktree name or a ticket id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AddWorktreeResult {
    /// Folder holding the group's worktrees, one subfolder per repo.
    pub path: String,
    pub worktrees: Vec<WorktreeInfo>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct RemoveWorktreeParams {
    pub workspace: String,
    /// A worktree id, a worktree path, a group folder or a group name.
    pub target: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RemoveWorktreeResult {
    pub removed: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ProcessInfo {
    pub id: String,
    pub workspace_id: String,
    pub kind: ProcessKind,
    pub repo: Option<String>,
    pub name: String,
    pub command: String,
    pub running: bool,
    pub exit_code: Option<i32>,
    /// Last lines of output, oldest first.
    pub tail: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ProcessesResult {
    pub processes: Vec<ProcessInfo>,
    /// Run scripts the workspace's repos declare, as `repo:name`.
    pub scripts: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct RunScriptParams {
    pub workspace: String,
    pub repo: String,
    /// Name of the run script; the repo's default one when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SetupScriptParams {
    pub workspace: String,
    /// The previous setup, which identifies its exact worktree.
    pub process: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct StopProcessParams {
    pub process: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct CreateChatParams {
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Images sent with the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ImageInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A design chat (story 47): on Claude Code unless `agent` asks for Codex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ChatKind>,
    /// A design chat's source chat, of the same workspace: its design brief is written from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_chat: Option<String>,
}

/// An exact resource path relative to a design chat's folder; never a filesystem path.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct DesignResourceParams {
    pub chat: String,
    pub file: String,
    pub offset: u32,
    pub version: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DesignResource {
    pub mime: String,
    pub base64: String,
    pub total: u32,
    pub version: String,
}

/// A design chat's canvas, read from its folder.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct CanvasResult {
    pub chat: String,
    /// The design folder, absolute.
    pub dir: String,
    /// Laid out by `canvas.json`; empty without one, or when it is invalid.
    pub pages: Vec<CanvasPage>,
    /// HTML files of the folder that `canvas.json` does not list (all of them without a valid
    /// one), shown in a last "Unsorted" section.
    pub unsorted: Vec<CanvasScreen>,
    /// Why `canvas.json` could not be read; none when it is valid or absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct CanvasPage {
    pub name: String,
    pub sections: Vec<CanvasSection>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct CanvasSection {
    pub title: String,
    pub screens: Vec<CanvasScreen>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct CanvasScreen {
    /// Relative to the design folder, as `canvas.json` names it.
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub label: Option<String>,
    /// CSS pixels; 390 × 844 when `canvas.json` gives none.
    pub width: u32,
    pub height: u32,
    /// Where it is served, for a sandboxed iframe; none when the file is missing (or named
    /// outside the folder) or the screens are not served.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub url: Option<String>,
    /// When the file last changed (milliseconds since the epoch): a new value means reload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub modified_ms: Option<u64>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct DesignExportParams {
    pub chat: String,
    /// The screens, relative to the design folder as `canvas.json` names them, in the order they
    /// go in the row; at least one, each of them present in the folder.
    pub files: Vec<String>,
    /// What the PNG is named after (a section's title, a screen's label); the first screen's
    /// label or file when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct DesignExportResult {
    /// The PNG, absolute, under the design folder's `exports/`.
    pub path: String,
    /// Its pixels (twice the CSS size, unless that would be too large).
    pub width: u32,
    pub height: u32,
    /// Whether it was also put on the clipboard.
    pub copied: bool,
}

/// Who wrote a comment: the Office's Google sign-in, else git's user.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct CommentAuthor {
    pub name: String,
    pub email: String,
}

/// Someone a comment names with `@`: a person the share is open to (its owner, an invitee, or an
/// address of its team domain). The text keeps a readable `@Name`; this says who it is. See
/// docs/adr/mentions-in-share-comments.md.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct Mention {
    /// Lowercase.
    pub email: String,
    /// As the text writes it after `@`.
    pub name: String,
}

/// The point of a screen a comment is anchored on, in percent of its width and height, with the
/// HTML element that was under it when it was placed.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct CommentPin {
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub element: Option<PinElement>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct PinElement {
    /// A CSS selector reaching the element in its screen; a pin whose selector matches nothing
    /// after the agent rewrote the screen is stale.
    pub selector: String,
    /// The element's text, cut short.
    pub text: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct CommentResolved {
    /// An author's e-mail, or `agent`.
    pub by: String,
    #[ts(type = "number")]
    pub at: u64,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct CommentSent {
    #[ts(type = "number")]
    pub at: u64,
}

/// A comment of a design chat (story 51), or of a markdown file of a workspace (story 57): a
/// thread's first message, or a reply (`reply_to`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct DesignComment {
    pub id: String,
    /// The screen's file, relative to the design folder as `canvas.json` names it; on a markdown
    /// file's comment, the file's path under the workspace folder.
    pub screen: String,
    /// None: the whole screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pin: Option<CommentPin>,
    /// On a markdown file's thread: the passage it is on, as selected in the rendered file. None:
    /// the whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub quote: Option<String>,
    pub author: CommentAuthor,
    /// Milliseconds since the epoch.
    #[ts(type = "number")]
    pub at: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub resolved: Option<CommentResolved>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sent: Option<CommentSent>,
    /// The people its text names with `@`, each once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<Mention>>", optional)]
    pub mentions: Vec<Mention>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct CommentsResult {
    pub chat: String,
    pub comments: Vec<DesignComment>,
    /// The user, as the comments they add are signed.
    pub author: CommentAuthor,
    /// Why `comments.json` could not be read; none when it is valid or absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct CommentAddParams {
    pub chat: String,
    pub screen: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<CommentPin>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// The people the text names with `@`: only on a shared design, among those it is open to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentions: Option<Vec<Mention>>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct CommentParams {
    pub chat: String,
    pub id: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct CommentEditParams {
    pub chat: String,
    pub id: String,
    pub text: String,
    /// The people the new text names; absent keeps the comment's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentions: Option<Vec<Mention>>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct CommentResolveParams {
    pub chat: String,
    pub id: String,
    pub resolved: bool,
}

/// `design.import`: the share's link, into a design chat of `workspace`.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DesignImportParams {
    pub workspace: String,
    /// `https://<share service>/s/<id>`, as the page's address.
    pub url: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ShareStartParams {
    pub chat: String,
    /// The reviewers' e-mails.
    pub reviewers: Vec<String>,
    /// Open to every member of the team too. A new share is not unless asked; an update keeps
    /// what the share had when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<bool>,
}

/// `files.share_start`, `files.share_update`: a markdown file shared with reviewers (story 57).
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct FileShareStartParams {
    pub workspace: String,
    pub path: String,
    /// The reviewers' e-mails.
    pub reviewers: Vec<String>,
    /// Open to every member of the team too. A new share is not unless asked; an update keeps
    /// what the share had when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<bool>,
}

/// `mentions.list`, `mentions.seen`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct MentionsResult {
    /// Newest first: the unread ones and the latest read ones.
    pub mentions: Vec<crate::design_share::Mentioned>,
    /// How the daemon stands with the share service for them: `Off` when signed out or without
    /// a share service.
    pub connection: ShareConnection,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct MentionSeenParams {
    /// `Mentioned::id`.
    pub id: String,
}

/// `design.shared`: newest first.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct SharedDesignsResult {
    pub designs: Vec<crate::design_share::SharedDesign>,
}

/// How the daemon stands with the share service for one chat.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(export)]
pub enum ShareConnection {
    /// No live share.
    Off,
    Connecting,
    Online,
    Failed {
        message: String,
    },
}

/// A reviewer's last comment, for the app to tell the user.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ReviewerComment {
    pub author: String,
    pub screen: String,
    pub text: String,
    #[ts(type = "number")]
    pub at: u64,
    /// It mentions the user: the mentions' own notification tells it, not this one.
    #[serde(default)]
    pub mentioned: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct WorkspaceSharePreviewParams {
    /// The share's link, `<service>/w/<id>`.
    pub url: String,
}

/// What importing a workspace share brings.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
#[ts(optional_fields)]
pub struct WorkspaceSharePreview {
    pub title: String,
    pub owner: CommentAuthor,
    /// Conversations copied into new chats.
    pub chats: u32,
    /// Notes copied into `.chauffeur/shared-notes`.
    pub notes: u32,
    /// When its owner's daemon last pushed it, in milliseconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number")]
    pub last_push_at: Option<u64>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct WorkspaceShareStartParams {
    /// These open chats only, a chat created later staying out. Without it nor `all_chats`, a
    /// new share holds all chats and an update keeps what the share had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_ids: Option<Vec<String>>,
    /// Every open chat, those created later too; refused with `chat_ids`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_chats: Option<bool>,
    pub workspace: String,
    pub guests: Vec<crate::workspace_share::Guest>,
    /// Open to every member of the team too, as viewers. A new share is not unless asked; an
    /// update keeps what the share had when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<bool>,
}

/// A workspace's share as the app shows it (story 53).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct WorkspaceShareState {
    /// The chats the owner picked; absent when the share holds all open chats, new ones too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub chat_ids: Option<Vec<String>>,
    pub workspace: String,
    /// The share service is set (`design_share.url`).
    pub configured: bool,
    /// The user's Google e-mail (the Office's sign-in), needed to share.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub email: Option<String>,
    /// The live share, as the service last told it; none before one, or after Stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub share: Option<crate::workspace_share::WorkspaceShareStatus>,
    pub connection: ShareConnection,
    /// The e-mails named on earlier shares (designs' and workspaces'), most recent first.
    pub past_reviewers: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SuggestionParams {
    pub chat: String,
    pub id: String,
}

/// `chat.suggestions`: oldest first.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct SuggestionsResult {
    pub suggestions: Vec<crate::workspace_share::Suggestion>,
}

/// `chat.share_comment_add`: a new thread on a passage (`message`, `quote`), or a reply
/// (`reply_to`).
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ShareCommentAddParams {
    pub chat: String,
    /// The agent's message the passage is in (its `message_id`); none on a reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// The people the text names with `@`, among those the share is open to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentions: Option<Vec<Mention>>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ShareCommentParams {
    pub chat: String,
    pub id: String,
}

/// `chat.share_comments`: oldest first.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ShareCommentsResult {
    pub comments: Vec<crate::workspace_share::ShareComment>,
}

/// A design chat's share as the app shows it (story 51), or a markdown file's (story 57).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct DesignShareState {
    /// The design chat; for a markdown file, `file:<workspace id>:<path>`.
    pub chat: String,
    /// The share service is set (`design_share.url`).
    pub configured: bool,
    /// The user's Google e-mail (the Office's sign-in), needed to share.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub email: Option<String>,
    /// The live share, as the service last told it; none before one, or after Stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub share: Option<crate::design_share::ShareStatus>,
    pub connection: ShareConnection,
    /// The e-mails named on earlier shares, most recent first.
    pub past_reviewers: Vec<String>,
    /// Bumped each time a reviewer's comment reached `comments.json`: the canvas reads again.
    #[ts(type = "number")]
    pub comments_rev: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_reviewer_comment: Option<ReviewerComment>,
}

/// A markdown file of a workspace (story 57), named as `files.read` names it.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct FileCommentsParams {
    pub workspace: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct FileCommentsResult {
    pub path: String,
    /// The file's comments, threads and replies.
    pub comments: Vec<DesignComment>,
    /// The user, as the comments they add are signed.
    pub author: CommentAuthor,
    /// The open chat whose agent last wrote or edited the file, where its comments are sent by
    /// default; none when no open chat's agent touched it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub writer: Option<String>,
    /// Why the workspace's `.chauffeur/comments.json` could not be read; none when it is valid or
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct FileCommentAddParams {
    pub workspace: String,
    pub path: String,
    /// The passage commented on, as selected in the rendered file; absent: the whole file. A
    /// reply takes its thread's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct FileCommentParams {
    pub workspace: String,
    pub id: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct FileCommentEditParams {
    pub workspace: String,
    pub id: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct FileCommentResolveParams {
    pub workspace: String,
    pub id: String,
    pub resolved: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct CommentsSendParams {
    pub chat: String,
    /// One thread to send, by its first comment's id, with its replies, sent before or not; every
    /// new comment when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct CommentsSendResult {
    /// The message for the agent; empty when nothing was new.
    pub message: String,
    /// How many comments it carries (threads and replies).
    pub sent: u32,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ChatListParams {
    pub workspace: String,
    /// The closed chats too.
    #[serde(default)]
    pub all: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RenameChatParams {
    pub chat: String,
    /// Empty: the automatic title (from the first message, else none).
    pub title: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct MoveChatParams {
    pub chat: String,
    /// The chat whose tab it goes before; last when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct CloseChatParams {
    pub chat: String,
    /// Stop the agent first when it works (a turn, background tasks, queued messages, a question
    /// waiting); otherwise that is refused.
    #[serde(default)]
    pub force: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ChatsResult {
    pub chats: Vec<ChatInfo>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ChatParams {
    pub chat: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ChatForkParams {
    pub chat: String,
    /// The finished turn to fork from, counted from 1 in the order of the chat's `turn_ended`
    /// events: the fork keeps the conversation up to its end. None: the whole conversation.
    #[serde(default)]
    pub turn: Option<u32>,
    /// The fork's agent; none: the chat's own.
    #[serde(default)]
    pub agent: Option<AgentKind>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SendParams {
    pub chat: String,
    /// UUID retained across retries. Accepted sends are confirmed without executing again;
    /// an uncertain handoff is refused until the user inspects the conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    pub text: String,
    /// Images sent along with the text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ImageInput>>,
    /// While the agent works: steer the turn (default) or wait in the queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<SendMode>,
    /// A guest's pending suggestion this message sends (story 53): marked sent, or sent edited
    /// when the text differs; its author shows on the message, never to the agent. It steers a
    /// running turn whatever `mode` says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// A chat's queue as the user arranged it: kept in this order, with these texts; queued
/// messages not listed are removed.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct QueueSetParams {
    pub chat: String,
    pub items: Vec<QueueItem>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct QueueItem {
    pub id: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct QueueSendParams {
    pub chat: String,
    /// The queued message to send now, out of turn.
    pub id: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct QueueResult {
    pub messages: Vec<QueuedMessage>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ContextUsageParams {
    pub chat: String,
    #[serde(default)]
    pub detail: ContextDetail,
}

/// How closely to read a context window.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ContextDetail {
    /// From the last response's usage and estimates: instant.
    #[default]
    Summary,
    /// Each category counted with the token-count API, and the items behind them.
    Full,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ContextUsageResult {
    /// None until a turn has ended.
    pub usage: Option<ContextUsage>,
    /// RFC 3339: when it was read.
    pub read_at: Option<String>,
    /// Read from the running session just now. False: the last reading stored, the session
    /// being closed (idle) or not started, or the agent reading nothing more than after a turn
    /// (Codex).
    pub live: bool,
}

/// A subagent, by the tool call that started it, or by its agent id (its task's `task_id`); at
/// least one of the two.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SubagentMessagesParams {
    pub chat: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct SubagentMessagesResult {
    /// The subagent's id, when it was found.
    pub agent_id: Option<String>,
    /// Its transcript was found: false when the subagent is unknown, its transcript gone or not
    /// written yet (then `events` is empty).
    pub found: bool,
    /// Its conversation, oldest first, as its own: `assistant_text`, `thinking`, `tool_call` and
    /// `tool_result` events without `parent_tool_use_id` (a nested subagent's own tool calls are
    /// in its transcript, not here). The prompt it was given is the tool call's input.
    pub events: Vec<ChatEvent>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct StopTaskParams {
    pub chat: String,
    /// From the task's `task_started` event.
    pub task_id: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ShellParams {
    pub chat: String,
    /// A command line for the shell, as typed after `!`.
    pub command: String,
}

/// A chat's shell command, started: `shell_started` says so, `shell_ended` how it ended.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ShellResult {
    pub shell_id: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct McpConnectParams {
    pub chat: String,
    /// The server's name, from the chat's `mcp_servers`.
    pub server: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AgentLoginParams {
    pub agent: AgentKind,
    /// The browser is on another machine (`AgentLoginStartParams::headless`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub headless: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct RespondParams {
    pub chat: String,
    pub request_id: String,
    pub response: RequestResponse,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ConfigureChatParams {
    pub chat: String,
    /// Switches the chat to another agent. A started chat requires `confirm_agent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentKind>,
    /// The current agent whose replacement the user confirmed; refuses stale confirmations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_agent: Option<AgentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
}

/// Which events a subscription receives. Events matching any filter are delivered.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SubscribeParams {
    #[serde(default)]
    pub all: bool,
    /// Workspace and chat upserts, and every workspace's share state, suggestions and comments
    /// (story 53), ephemeral as they are.
    #[serde(default)]
    pub registry: bool,
    #[serde(default)]
    pub workspaces: Vec<String>,
    #[serde(default)]
    pub chats: Vec<String>,
    /// Replay persisted events after this sequence number before going live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub after_seq: Option<i64>,
    /// Also deliver ephemeral events (text deltas, script output).
    #[serde(default)]
    pub ephemeral: bool,
    /// Replays only part of the history (`after_seq` defaults to 0). Meant for the first
    /// subscription of a client: a reconnection resumes from the last sequence it saw, whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay: Option<Replay>,
    /// The `office` scope, delivered to no other subscription. An Office view subscribes with
    /// it: while one does, the daemon stays connected to the office server.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub office: bool,
}

/// A part of a long history, so that a client shows the end of a conversation without waiting
/// for the rest. Workspace events (the few that are not a chat's) all belong to the tail.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(tag = "part", rename_all = "snake_case")]
#[ts(export)]
pub enum Replay {
    /// Each chat's last `per_chat` events, then live as usual.
    Tail { per_chat: u32 },
    /// What `Tail` with the same `per_chat` leaves out, then `replayed`; never live.
    BeforeTail { per_chat: u32 },
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SubscribeResult {
    pub subscription: String,
    /// Sequence number of the last persisted event when the subscription started.
    #[ts(type = "number")]
    pub last_seq: i64,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct UnsubscribeParams {
    pub subscription: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct HistoryParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub after_seq: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Return the newest matching events, still ordered oldest first within the page.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub tail: bool,
    /// Exclusive upper bound when loading the history preceding an already displayed tail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub before_seq: Option<i64>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct HistoryResult {
    pub events: Vec<EventEnvelope>,
    /// Confirms support for tail pagination; absent on older daemons.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub tail: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct DiffParams {
    pub workspace: String,
    /// Only this repo; every repo of the workspace otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Leave the unified patch out (file list and counts only).
    #[serde(default)]
    pub stat_only: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DiffResult {
    pub repos: Vec<RepoDiff>,
}

/// What a repo's worktree changed against the merge-base with its base branch, uncommitted and
/// untracked files included.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RepoDiff {
    pub repo: String,
    pub worktree_id: String,
    pub path: String,
    pub branch: String,
    pub base_branch: String,
    pub merge_base: Option<String>,
    /// Commits on the branch that the base branch does not have.
    pub ahead: u32,
    pub files: Vec<FileDiff>,
    /// Unified diff; empty with `stat_only`.
    pub patch: String,
    /// The patch was cut at the size limit.
    pub truncated: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct FilesListResult {
    /// One root per repo of the workspace, then the notes folder.
    pub roots: Vec<FileRoot>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct FileRoot {
    /// Folder name under the workspace: the repo's name, or `.chauffeur` for the notes.
    pub name: String,
    /// The repo, or null for the notes folder.
    pub repo: Option<String>,
    /// Paths relative to the root, sorted.
    pub files: Vec<String>,
    /// The list stopped at its size limit.
    pub truncated: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RenameWorkspaceParams {
    pub workspace: String,
    /// The new name; empty shows the slug again.
    pub title: String,
}

/// A file is named by its path under the workspace folder: `<repo>/<path>` or `.chauffeur/<path>`.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct FileReadParams {
    pub workspace: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum FileKind {
    Text,
    Image,
    /// Neither text nor a known image format.
    Binary,
    /// Beyond the size the daemon sends.
    TooLarge,
    /// A video, an audio file or a PDF (`mime`): its bytes are not sent, the app shows it from
    /// the daemon's `/media` route.
    Media,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct FileContent {
    pub path: String,
    pub kind: FileKind,
    /// The content of a text file.
    pub text: Option<String>,
    /// The content of an image, base64-encoded, and its MIME type.
    pub base64: Option<String>,
    pub mime: Option<String>,
    #[ts(type = "number")]
    pub size: u64,
    /// A hash of the content, given back to `files.write` to detect a change made meanwhile.
    pub version: String,
    /// Edits are refused: an exploration, or a workspace that is not ready.
    pub read_only: bool,
    /// An HTML file's page, rendered in the Files tab: served on the design screens' origin,
    /// apart from the app's and the daemon's, with the files it loads by relative URLs.
    #[serde(default)]
    pub preview_url: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct FileWriteParams {
    pub workspace: String,
    pub path: String,
    pub text: String,
    /// The version read before editing; the write is refused when the file changed since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct FilesSearchParams {
    pub workspace: String,
    pub query: String,
    /// The query is a regular expression (extended), not a literal.
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    /// Only this root (a repo name, or `.chauffeur`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// At most this many matches (default 500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct FilesSearchResult {
    pub matches: Vec<SearchMatch>,
    /// More matches were found than returned.
    pub truncated: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SearchMatch {
    /// Under the workspace folder, like `files.read`'s path.
    pub path: String,
    /// 1-based.
    pub line: u32,
    /// 1-based, in bytes, of the first match on the line.
    pub column: u32,
    /// The line, cut when very long.
    pub text: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct OpenTerminalParams {
    pub workspace: String,
    /// Start in this repo's worktree; the workspace folder otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// Run this command instead of the login shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct TerminalInputParams {
    pub terminal: String,
    /// Keystrokes, as UTF-8 text.
    pub data: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ResizeTerminalParams {
    pub terminal: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct TerminalParams {
    pub terminal: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct MoveTerminalParams {
    pub terminal: String,
    /// The terminal whose tab it goes before; last when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RenameTerminalParams {
    pub terminal: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct TerminalCwdResult {
    pub cwd: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct TerminalsResult {
    pub terminals: Vec<TerminalInfo>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct TerminalSnapshotResult {
    pub terminal: TerminalInfo,
    /// The scrollback so far, base64-encoded bytes, to replay into a terminal emulator before
    /// following `terminal_output` events.
    pub data: String,
    /// Offset, in the terminal's whole output, of the byte after `data`. A live chunk whose
    /// `offset` + length is at most this was already replayed; one that starts before it
    /// overlaps by `end_offset - offset` bytes.
    #[ts(type = "number")]
    pub end_offset: u64,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct PrInfo {
    pub repo: String,
    /// The worktree group an agent created (`chauffeur worktree add`), e.g. one per sub-ticket;
    /// none for the workspace's own worktree.
    #[serde(default)]
    pub group: Option<String>,
    pub branch: String,
    pub base_branch: String,
    /// Commits on the branch that the base branch does not have.
    pub ahead: u32,
    /// Uncommitted or untracked changes in the worktree.
    pub dirty: bool,
    pub number: Option<u64>,
    /// The branch the PR merges into on GitHub: `base_branch`, or the head branch of the PR it
    /// is stacked on.
    #[serde(default)]
    pub pr_base_branch: Option<String>,
    pub url: Option<String>,
    /// `OPEN`, `MERGED` or `CLOSED`.
    pub state: Option<String>,
    pub title: Option<String>,
    pub is_draft: bool,
    /// Combined CI status: `SUCCESS`, `FAILURE`, `PENDING` or none.
    pub checks: Option<String>,
    /// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`, or none.
    #[serde(default)]
    pub review_decision: Option<String>,
    /// `MERGEABLE`, `CONFLICTING` or `UNKNOWN` (GitHub has not computed it yet).
    #[serde(default)]
    pub mergeable: Option<String>,
    /// What the repo allows, known once it has an open PR.
    #[serde(default)]
    pub merge_methods: Vec<MergeMethod>,
    /// Whether the user may merge into the repo (write access or more), known once it has an
    /// open PR; none while unknown.
    #[serde(default)]
    pub can_merge: Option<bool>,
}

/// How a pull request is merged.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum MergeMethod {
    Squash,
    Merge,
    Rebase,
}

impl MergeMethod {
    /// `gh pr merge`'s flag.
    pub fn flag(self) -> &'static str {
        match self {
            MergeMethod::Squash => "--squash",
            MergeMethod::Merge => "--merge",
            MergeMethod::Rebase => "--rebase",
        }
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct MergePrsParams {
    pub workspace: String,
    pub method: MergeMethod,
    /// The open PRs to merge, in this order (the first failure stops the rest); every open PR,
    /// APIs first, when empty.
    #[serde(default)]
    pub prs: Vec<PrRef>,
}

/// A pull request of a workspace: its repo and head branch. Every repo of a workspace is on the
/// same branch, so the branch alone does not name one.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq, Hash)]
#[ts(export)]
pub struct PrRef {
    pub repo: String,
    pub branch: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct MergePrsResult {
    pub outcomes: Vec<MergeOutcome>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct MergeOutcome {
    pub repo: String,
    pub branch: String,
    pub merged: bool,
    /// Why it was not merged: GitHub's answer, or an earlier PR that failed.
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct PrStatusResult {
    pub prs: Vec<PrInfo>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ShippedParams {
    /// RFC 3339: the oldest merge to give.
    pub since: String,
    /// Everyone's in the organization, open ones included, rather than the user's merged ones.
    #[serde(default)]
    pub team: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ShippedResult {
    pub prs: Vec<ShippedPr>,
    /// The Linear projects of the tickets of `prs`, for the team's board only: empty for the
    /// user's own, without Linear, or when it did not answer.
    #[serde(default)]
    pub projects: Vec<ShippedProject>,
    /// The tickets of `prs` Linear says nobody is assigned to: the team's board leaves them out.
    /// Empty when `projects` is.
    #[serde(default)]
    pub unassigned: Vec<String>,
}

/// A Linear project some tickets of the pull requests belong to: the EOW board shows it as one
/// card, with how far it is, rather than a card per ticket.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ShippedProject {
    pub id: String,
    pub name: String,
    pub url: String,
    /// How far it is, from 0 to 1, as Linear computes it from its issues.
    pub progress: f64,
    /// The identifiers of the pull requests' tickets in it.
    pub tickets: Vec<String>,
}

/// A pull request merged, or open (`merged_at` none).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ShippedPr {
    /// `owner/name`.
    pub repo: String,
    #[ts(type = "number")]
    pub number: u64,
    pub title: String,
    pub url: String,
    pub branch: String,
    /// Its author's GitHub login.
    pub author: String,
    /// Their name on GitHub, when they gave one.
    pub author_name: Option<String>,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub merged_at: Option<String>,
    /// The tickets of the workspace that worked on its branch, else the Linear issue its branch
    /// names; none when neither is known.
    pub tickets: Vec<TicketRef>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct CreatePrParams {
    pub workspace: String,
    #[serde(default)]
    pub draft: bool,
    /// Added to the request the agent receives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct LinearSearchParams {
    /// Free text or an identifier; empty with `mine` lists the user's open issues.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default)]
    pub mine: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Also the project milestones: named like `query`, the one a pasted link or id names, or
    /// without `query` the next ones of the projects the user is a member of. Optional, so callers
    /// written before it (an extension's UI) still type-check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestones: Option<bool>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct LinearSearchResult {
    pub issues: Vec<LinearIssue>,
    /// Asked with `milestones`: empty when Linear could not list them.
    #[serde(default)]
    pub milestones: Vec<LinearMilestone>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct LinearIssueParams {
    pub identifier: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct LinearStatesResult {
    pub states: Vec<LinearState>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct LinearSetStateParams {
    pub identifier: String,
    /// A state's `id`, from `linear.states`.
    pub state: String,
}

/// What the Office's news kiosk shows: the user's own, never sent to the office server.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
pub struct NewsState {
    /// Hacker News's top stories in its order; `None` while the user hides it.
    pub hacker_news: Option<NewsSource>,
    /// The user's feeds, in the order of settings.toml, each with its items newest first.
    pub feeds: Vec<NewsSource>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct NewsSource {
    /// The feed's URL as settings.toml keeps it (for Hacker News, its front page).
    pub url: String,
    /// The feed's own title; its URL's host until it was read.
    pub title: String,
    pub items: Vec<NewsItem>,
    /// Why its last fetch failed; its items are then those of the last one that worked.
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct NewsItem {
    /// The same at every fetch: what the read state is kept by.
    pub id: String,
    pub title: String,
    /// The article; for a story without one (Ask HN), its discussion.
    pub url: String,
    /// When it was published (RFC 3339), if the source says.
    pub published: Option<String>,
    pub read: bool,
    /// Hacker News's points, comments and discussion page; `None` for a feed's item.
    pub points: Option<u32>,
    pub comments: Option<u32>,
    pub discussion: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct NewsMarkReadParams {
    pub ids: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct NewsFeedParams {
    /// The feed's address, `https://…`.
    pub url: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct NewsShowParams {
    pub shown: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct UsageResult {
    pub agents: Vec<AgentUsage>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AgentUsage {
    pub agent: AgentKind,
    pub plan: Option<String>,
    pub windows: Vec<crate::sidecar::UsageWindow>,
    /// Why it could not be read (the agent is missing, logged out…); the last reading stays.
    pub error: Option<String>,
    /// When it was read, RFC 3339.
    pub read_at: Option<String>,
    /// When the last read failed (with `error`), RFC 3339.
    #[serde(default)]
    pub failed_at: Option<String>,
    #[serde(default)]
    pub extra_usage: Option<crate::sidecar::ExtraUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reset_credits: Option<crate::sidecar::ResetCredits>,
    /// Whether the agent's CLI is logged in: signed out when it said so, or refused its
    /// credentials in a chat, until a read or a login says otherwise.
    #[serde(default)]
    pub account: crate::model::AccountStatus,
    /// Why it is signed out.
    #[serde(default)]
    pub account_error: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ModelsListParams {
    /// Ask the agents again instead of using the daemon's cached catalogs.
    #[serde(default)]
    pub refresh: bool,
    /// The models of the machine this workspace runs on (one moved there): its agents' logins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub workspace: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ModelsListResult {
    pub agents: Vec<AgentModels>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct CommandsListParams {
    pub workspace: String,
    pub agent: AgentKind,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct CommandsForReposParams {
    pub repos: Vec<String>,
    pub agent: AgentKind,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct CommandsListResult {
    pub commands: Vec<SlashCommand>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
pub struct LoadoutParams {
    pub entries: Vec<LoadoutEntry>,
}

/// The daily session review; no model override follows the usual chat default.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(default)]
#[ts(export)]
pub struct DailyReviewSettings {
    pub enabled: bool,
    pub agent: Option<AgentKind>,
    pub model: Option<String>,
    pub wake: bool,
}

impl Default for DailyReviewSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            agent: None,
            model: None,
            wake: false,
        }
    }
}

/// What the app's settings window shows and changes; the rest is in `settings.toml`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct AppSettings {
    #[serde(default)]
    pub daily_review: DailyReviewSettings,
    #[serde(default)]
    pub daily_review_error: Option<String>,
    pub workspaces: WorkspaceDefaults,
    pub linear: LinearSettings,
    pub jev: JevSettings,
    pub appearance: Appearance,
    pub notifications: NotificationSettings,
    pub keyboard: KeyboardSettings,
    pub power: PowerSettings,
    #[serde(default)]
    pub search: SearchSettings,
    /// The agent and the model of a chat started without one (`chauffeur new`), when set: they
    /// win over the first favorite of the model picker.
    pub default_agent: Option<String>,
    pub default_model: Option<String>,
    /// Port of the daemon's WebSocket, taken at the daemon's next start.
    pub daemon_port: u16,
    /// The daemon starts at login (a LaunchAgent, or a systemd user service).
    pub start_at_login: bool,
    /// After each turn, Claude suggests the user's next message, offered in the empty composer
    /// (`prompt_suggestions`).
    pub prompt_suggestions: bool,
    /// Where workspaces can be transferred (`[transfer]`).
    pub transfer: TransferSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub workers: Option<WorkerConfiguration>,
    /// Whether error toasts are reported to Chauffeur's Sentry.
    pub error_reports: ErrorReports,
    /// This build can report errors: only packaged builds carry Sentry's address.
    pub error_reports_available: bool,
    /// `~/.chauffeur/settings.toml`, which holds every other setting.
    pub file: String,
    /// Why the file's current content is not in force (it does not parse).
    pub file_error: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct WorkspaceDefaults {
    /// The repos a new workspace starts with.
    pub default_repos: Vec<String>,
    /// Preferred display order; unlisted repos follow in automatic order.
    #[serde(default)]
    pub repo_order: Vec<String>,
    /// Branches are `<prefix>/<name>`; empty: the GitHub login.
    pub branch_prefix: String,
    /// The GitHub login, when known: the prefix when none is set.
    pub github_login: Option<String>,
    /// Folders searched for existing clones, by repo name (`~/` allowed).
    pub clone_roots: Vec<String>,
    /// The main GitHub organization: its repos are named without their owner; empty: none.
    pub github_org: String,
    /// The repos of every other owner the GitHub account reaches are listed too.
    #[serde(default)]
    pub github_sources: bool,
    /// Child workspaces one workspace may have at once.
    pub max_children: u32,
    /// First port handed out to workspaces, and ports per workspace.
    pub port_range_start: u16,
    pub port_block_size: u16,
    /// Seconds a repo's archive script may run.
    pub archive_script_timeout_secs: u32,
    /// Days an archived workspace's snapshot is kept.
    pub snapshot_retention_days: u32,
    /// Minutes without activity after which an agent's session closes (0 keeps them).
    pub idle_session_minutes: u32,
    /// A clone outside the clone folders, or another default branch, by repo.
    pub repos: Vec<RepoSetting>,
}

/// `[repos.<name>]` in `settings.toml`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct WorkerConfiguration {
    pub project: String,
    pub zone: String,
}

/// `[repos.<name>]` in `settings.toml`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct RepoSetting {
    pub name: String,
    /// Its clone (`~/` allowed).
    pub path: Option<String>,
    pub default_branch: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct LinearSettings {
    pub enabled: bool,
    /// Move attached tickets to In Progress when a workspace starts on them.
    pub start_tickets: bool,
    /// A Linear API key is saved (it is never sent back).
    pub key_saved: bool,
    /// `CHAUFFEUR_LINEAR_API_KEY` is set: it wins over the saved key.
    pub key_from_env: bool,
}

/// The fonts of the app (`[appearance]` in `settings.toml`): a font family, as CSS names it
/// (`JetBrains Mono`, or several separated by commas), and a size in pixels; empty or 0 keeps the
/// app's own. The interface's size is the app's zoom (⌘+ / ⌘−).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Default)]
#[ts(export)]
pub struct Appearance {
    pub interface_font: String,
    /// The Files editor, the Diff and the code in chats.
    pub code_font: String,
    pub code_size: u16,
    pub terminal_font: String,
    pub terminal_size: u16,
    /// A colour theme file (`~/` allowed): pywal JSON, kitty, Ghostty, foot or Alacritty (wallust
    /// writes any of them from a template).
    /// Empty: the app's own colours.
    pub terminal_theme: String,
    /// The theme colours the interface too (its background, surfaces, text and accent), unless it
    /// is light: the interface is dark only.
    #[serde(default)]
    pub theme_interface: bool,
    /// The theme's colours, read now: the terminal's, and the code's from its 16 ANSI colours.
    pub palette: Option<Palette>,
    /// Why `terminal_theme` gives no palette.
    pub palette_error: Option<String>,
}

/// `[notifications]` in `settings.toml`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Default)]
#[ts(export)]
pub struct NotificationSettings {
    /// Played when a chat finishes or needs the user while the app is not in front: empty (none),
    /// one of the app's own (`chime`, `bell`, `station`), or an audio file's path (`~/` allowed),
    /// which the daemon serves at `/sound`.
    pub sound: String,
    /// Why the sound file cannot be played.
    pub sound_error: Option<String>,
}

/// Machines a workspace can be transferred to, each a Chauffeur daemon on a Unix socket (another
/// machine's, forwarded over SSH), and the controller daemon keeping the transfers' journal.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Default)]
#[ts(export)]
pub struct TransferSettings {
    /// The controller daemon's socket; empty, no transfer can start.
    pub controller: String,
    pub machines: Vec<TransferMachine>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct TransferMachine {
    /// Shown in the app and named by `chauffeur cloud transfer --to`.
    pub name: String,
    /// The machine's daemon socket, absolute or under `~/`.
    pub socket: String,
}

/// `[keyboard]` in `settings.toml`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Default)]
#[ts(export)]
pub struct KeyboardSettings {
    /// The Files editor takes Vim's keys (normal, insert and visual modes, `:w` saves).
    pub vim_mode: bool,
    /// The keys the user gave shortcuts (`[keyboard.bindings]`), written the macOS way (`⇧⌘T`);
    /// the others keep the app's defaults.
    pub bindings: BTreeMap<ShortcutId, String>,
    /// The entries of `[keyboard.bindings]` left out, each saying why.
    pub binding_errors: Vec<String>,
}

/// A shortcut of the app whose keys `[keyboard.bindings]` can change. The app holds their default
/// keys and what they do (`app/src/lib/shortcuts.ts`).
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[ts(export)]
pub enum ShortcutId {
    #[serde(rename = "workspace.new")]
    WorkspaceNew,
    #[serde(rename = "exploration.new")]
    ExplorationNew,
    #[serde(rename = "switcher.open")]
    SwitcherOpen,
    #[serde(rename = "sidebar.toggle")]
    SidebarToggle,
    #[serde(rename = "settings.open")]
    SettingsOpen,
    #[serde(rename = "office.show")]
    OfficeShow,
    #[serde(rename = "zoom.in")]
    ZoomIn,
    #[serde(rename = "zoom.out")]
    ZoomOut,
    #[serde(rename = "zoom.reset")]
    ZoomReset,
    #[serde(rename = "files.goto")]
    FilesGoto,
    #[serde(rename = "files.search")]
    FilesSearch,
    #[serde(rename = "open.app")]
    OpenApp,
    #[serde(rename = "view.split")]
    ViewSplit,
    #[serde(rename = "chat.new")]
    ChatNew,
    #[serde(rename = "chat.close")]
    ChatClose,
    #[serde(rename = "chat.reopen")]
    ChatReopen,
    #[serde(rename = "terminal.new")]
    TerminalNew,
    #[serde(rename = "terminal.close")]
    TerminalClose,
    #[serde(rename = "tab.next")]
    TabNext,
    #[serde(rename = "tab.previous")]
    TabPrevious,
    #[serde(rename = "view.toggle-terminal")]
    ViewToggleTerminal,
    #[serde(rename = "view.chat")]
    ViewChat,
    #[serde(rename = "view.diff")]
    ViewDiff,
    #[serde(rename = "view.files")]
    ViewFiles,
    #[serde(rename = "view.terminal")]
    ViewTerminal,
    #[serde(rename = "view.processes")]
    ViewProcesses,
    #[serde(rename = "view.database")]
    ViewDatabase,
    #[serde(rename = "chat.top")]
    ChatTop,
    #[serde(rename = "chat.bottom")]
    ChatBottom,
    #[serde(rename = "model.effort")]
    ModelEffort,
    #[serde(rename = "model.fast")]
    ModelFast,
}

impl ShortcutId {
    /// The shortcut `name` (`chat.new`) names in `settings.toml`, if any.
    pub fn parse(name: &str) -> Option<ShortcutId> {
        serde_json::from_value(serde_json::Value::String(name.into())).ok()
    }

    /// Its name in `settings.toml` (`chat.new`).
    pub fn name(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|name| name.as_str().map(str::to_string))
            .expect("a shortcut id serializes to its name")
    }
}

/// Keeping the machine awake while agents work (`keep_awake`, `keep_awake_min_battery`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Default)]
#[ts(export)]
pub struct PowerSettings {
    pub keep_awake: KeepAwake,
    /// In lid-closed mode, on battery below this charge (percent), sleep is given back.
    pub min_battery_percent: u8,
    /// `lid_closed` can be chosen here (macOS).
    pub lid_closed_available: bool,
    /// The sudoers rule that lets the daemon run `pmset -a disablesleep 1|0` is installed.
    pub sudoers_rule: bool,
}

/// What changes; anything left out stays. Choosing `lid_closed` installs the sudoers rule, and
/// leaving it removes the rule, each behind one administrator password prompt; a prompt
/// cancelled or failed leaves the setting as it was.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct PowerUpdate {
    #[serde(default)]
    pub keep_awake: Option<KeepAwake>,
    #[serde(default)]
    pub min_battery_percent: Option<u8>,
}

/// What changes; anything left out stays.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct NotificationsUpdate {
    /// Empty for none, `chime`, `bell`, `station`, or a file's path.
    #[serde(default)]
    pub sound: Option<String>,
}

/// What changes; anything left out stays.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct KeyboardUpdate {
    #[serde(default)]
    pub vim_mode: Option<bool>,
    /// Every key the user gave, in place of `[keyboard.bindings]`; empty: the defaults again.
    #[serde(default)]
    pub bindings: Option<BTreeMap<ShortcutId, String>>,
}

/// A terminal palette, every colour as `#rrggbb`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Default)]
#[ts(export)]
pub struct Palette {
    pub background: String,
    pub foreground: String,
    pub cursor: Option<String>,
    pub selection: Option<String>,
    /// The 16 ANSI colours, black to bright white.
    pub ansi: Vec<String>,
}

/// What changes; anything left out stays. An empty font or a 0 size goes back to the app's own.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct AppearanceUpdate {
    #[serde(default)]
    pub interface_font: Option<String>,
    #[serde(default)]
    pub code_font: Option<String>,
    #[serde(default)]
    pub code_size: Option<u16>,
    #[serde(default)]
    pub terminal_font: Option<String>,
    #[serde(default)]
    pub terminal_size: Option<u16>,
    /// The theme file's path; empty for the app's own colours.
    #[serde(default)]
    pub terminal_theme: Option<String>,
    #[serde(default)]
    pub theme_interface: Option<bool>,
}

/// Compaction of Claude and Codex chats through Jev (TypeSafe), off until the user turns it on.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct JevSettings {
    pub enabled: bool,
    /// Share of the context in use, in percent (as Claude Code's `/context` counts it, for Codex
    /// the last response's tokens against the window), from which a chat is compacted.
    pub threshold_percent: u8,
    /// A TypeSafe API key is saved (it is never sent back).
    pub key_saved: bool,
}

/// What changes; anything left out stays.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SettingsUpdate {
    /// Replaces the daily review settings.
    #[serde(default)]
    pub daily_review: Option<DailyReviewSettings>,
    #[serde(default)]
    pub workspaces: Option<WorkspaceDefaultsUpdate>,
    #[serde(default)]
    pub linear: Option<LinearUpdate>,
    #[serde(default)]
    pub jev: Option<JevUpdate>,
    #[serde(default)]
    pub appearance: Option<AppearanceUpdate>,
    #[serde(default)]
    pub notifications: Option<NotificationsUpdate>,
    #[serde(default)]
    pub keyboard: Option<KeyboardUpdate>,
    #[serde(default)]
    pub power: Option<PowerUpdate>,
    #[serde(default)]
    pub search: Option<SearchUpdate>,
    #[serde(default)]
    pub start_at_login: Option<bool>,
    #[serde(default)]
    pub prompt_suggestions: Option<bool>,
    /// Empty removes it: the first favorite of the model picker again.
    #[serde(default)]
    pub default_agent: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub daemon_port: Option<u16>,
    /// Replaces `[transfer]` whole.
    #[serde(default)]
    pub transfer: Option<TransferSettings>,
    #[serde(default)]
    pub workers: Option<WorkerConfiguration>,
    #[serde(default)]
    pub error_reports: Option<ErrorReports>,
}

/// `[search]` in `settings.toml`: search by meaning, beside keywords.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(default)]
#[ts(export)]
pub struct SearchSettings {
    /// Index and search by meaning through Ollama; off, keywords alone.
    pub semantic: bool,
    /// The Ollama model that gives text its meaning; another one indexes everything again.
    pub embedding_model: String,
}

impl Default for SearchSettings {
    fn default() -> Self {
        SearchSettings {
            semantic: true,
            embedding_model: DEFAULT_EMBEDDING_MODEL.into(),
        }
    }
}

/// Multilingual (French and English, code), light and quick to index the history with.
pub const DEFAULT_EMBEDDING_MODEL: &str = "embeddinggemma";

/// What changes; anything left out stays.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SearchUpdate {
    #[serde(default)]
    pub semantic: Option<bool>,
    /// Empty: the default model again.
    #[serde(default)]
    pub embedding_model: Option<String>,
}

/// An error toast of the app to report.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ErrorReportParams {
    /// The toast's text.
    pub message: String,
    /// The daemon's failure the toast shows, from its error response (`RpcError.failure`).
    #[serde(default)]
    pub failure: Option<String>,
    #[serde(default)]
    pub app_version: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct WorkspaceDefaultsUpdate {
    #[serde(default)]
    pub default_repos: Option<Vec<String>>,
    /// Empty restores automatic ordering.
    #[serde(default)]
    pub repo_order: Option<Vec<String>>,
    /// Empty: the GitHub login.
    #[serde(default)]
    pub branch_prefix: Option<String>,
    #[serde(default)]
    pub clone_roots: Option<Vec<String>>,
    #[serde(default)]
    pub github_org: Option<String>,
    #[serde(default)]
    pub github_sources: Option<bool>,
    #[serde(default)]
    pub max_children: Option<u32>,
    #[serde(default)]
    pub port_range_start: Option<u16>,
    #[serde(default)]
    pub port_block_size: Option<u16>,
    #[serde(default)]
    pub archive_script_timeout_secs: Option<u32>,
    #[serde(default)]
    pub snapshot_retention_days: Option<u32>,
    #[serde(default)]
    pub idle_session_minutes: Option<u32>,
    /// Replaces every `[repos.*]` table.
    #[serde(default)]
    pub repos: Option<Vec<RepoSetting>>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct LinearUpdate {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub start_tickets: Option<bool>,
    /// A new Linear API key; empty forgets the saved one.
    #[serde(default)]
    pub api_key: Option<String>,
}

/// What changes; anything left out stays.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct JevUpdate {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub threshold_percent: Option<u8>,
    /// A new TypeSafe API key; empty forgets the saved one (and turns Jev off).
    #[serde(default)]
    pub api_key: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct OpenAppsResult {
    pub apps: Vec<OpenApp>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct OpenParams {
    pub workspace: String,
    /// One repo's worktree; the workspace folder (every repo) when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// An app id from `open.apps`; the default app when absent. Becomes the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// A design chat of the workspace, whose design folder opens instead (an editor or the file
    /// manager; never a terminal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design_chat: Option<String>,
    /// A terminal of the workspace, whose shell's current folder opens instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct OpenResult {
    pub app: OpenApp,
    pub path: String,
}

/// Local desktop configuration; never forwarded by the mobile bridge.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
pub struct MobileConfigureParams {
    pub enabled: bool,
    pub relay: String,
    // Omit to retain the saved credential. Never returned in status.
    pub operator_token: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct MobileHostStatus {
    pub enabled: bool,
    pub relay: String,
    pub key_saved: bool,
    pub phase: String,
    pub error: Option<String>,
    pub connected_devices: usize,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct PushPreferences {
    pub enabled: bool,
    pub approvals: bool,
    pub completed: bool,
    pub errors: bool,
    /// Workspaces left an hour without activity on another machine. On for a phone that opts in
    /// now; off when absent: a grant saved before, or a phone build that predates it, which has
    /// no way to turn it off.
    #[serde(default)]
    pub idle_reminders: bool,
    pub show_workspace: bool,
    pub language: String,
}
impl Default for PushPreferences {
    fn default() -> Self {
        Self {
            enabled: false,
            approvals: true,
            completed: true,
            errors: true,
            idle_reminders: true,
            show_workspace: true,
            language: "en".into(),
        }
    }
}
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PushProvider {
    #[default]
    Fcm,
    Apns,
}
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct PushRegistration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub provider: Option<PushProvider>,
    pub token: String,
    pub preferences: PushPreferences,
}
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct PushConfigureParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub provider: Option<PushProvider>,
    pub token: Option<String>,
    pub preferences: PushPreferences,
}
methods! {
    /// What the phone asks the mobile host itself, beside the daemon's methods it forwards: the
    /// phone's own push notifications, kept by the daemon under `remote.push_*`.
    pub enum MobileHostCall / MobileHostMethod {
        #[serde(rename = "push.status")]
        PushStatus(EmptyParams) -> PushPreferences,
        #[serde(rename = "push.configure")]
        PushConfigure(PushConfigureParams) -> PushPreferences,
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RemotePushParams {
    pub device_id: String,
    pub config: PushConfigureParams,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct OllamaModelParams {
    /// As Ollama names it, e.g. `qwen3.8` or `glm-4.7-flash:q8_0`.
    pub model: String,
}

/// Ollama, which serves the local models (`ollama.status`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct OllamaStatus {
    /// The `ollama` executable found; none when Ollama is not installed.
    pub executable: Option<String>,
    /// The executable is Chauffeur's own copy.
    pub own: bool,
    /// The version of Ollama Chauffeur installs.
    pub version: String,
    /// The size of its download on this system; none where Chauffeur cannot install it.
    #[ts(type = "number | null")]
    pub download_size: Option<u64>,
    /// The command that installs Ollama by hand, where Chauffeur cannot.
    pub install: String,
    /// Chauffeur's installation in progress, or the last one that failed.
    pub installing: Option<OllamaInstalling>,
    pub running: OllamaRunning,
    /// The models on disk, by name.
    pub models: Vec<OllamaModel>,
    /// The pulls in progress, and the last one that failed for each model.
    pub pulls: Vec<OllamaPull>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum OllamaRunning {
    Stopped,
    /// Started by the daemon, which stops it once unused.
    Daemon,
    /// Started by someone else (Ollama's app, a service): used as is, never stopped.
    External,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct OllamaModel {
    /// What `ollama/<name>` names in a chat's model.
    pub name: String,
    #[ts(type = "number")]
    pub size: u64,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct OllamaPull {
    pub model: String,
    /// Ollama's last word on it, e.g. `pulling manifest` or `verifying sha256 digest`.
    pub status: String,
    /// Bytes downloaded and to download of the layer being pulled; 0 when Ollama gives none.
    #[ts(type = "number")]
    pub completed: u64,
    #[ts(type = "number")]
    pub total: u64,
    /// Why it failed; the pull is over.
    pub error: Option<String>,
}

/// GitHub as Chauffeur reaches it (`github.status`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct GithubStatus {
    /// git is installed; every workspace needs it.
    pub git: bool,
    /// The way to install git here: `xcode` when `git.install` opens Apple's installer, else
    /// the command to run.
    pub git_install: String,
    /// The `gh` executable found; none until `github.connect` installs Chauffeur's own.
    pub gh: Option<String>,
    /// The executable is Chauffeur's own copy.
    pub own: bool,
    /// The version of `gh` Chauffeur installs.
    pub version: String,
    /// The size of its download on this system; none where Chauffeur cannot install it.
    #[ts(type = "number | null")]
    pub download_size: Option<u64>,
    /// The GitHub account `gh` is signed in to.
    pub account: Option<String>,
    /// The organizations that account belongs to, by login.
    pub organizations: Vec<String>,
    /// The organization (or account) whose repos a workspace picks from, the settings'
    /// `github_org`; empty when none is chosen.
    pub organization: String,
    /// The connection under way, or the last one that failed.
    pub connecting: Option<GithubConnecting>,
}

/// A connection to GitHub under way (`github.connect`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct GithubConnecting {
    /// `downloading`, `checking`, `unpacking` (Chauffeur's own `gh`), `starting`, `waiting`
    /// (for the user to enter the code on GitHub) or `configuring` (git).
    pub phase: String,
    /// Bytes downloaded and to download while `downloading`; 0 otherwise.
    #[ts(type = "number")]
    pub completed: u64,
    #[ts(type = "number")]
    pub total: u64,
    /// The one-time code to enter on GitHub, once `gh` gave it.
    pub code: Option<String>,
    /// Where to enter it.
    pub url: Option<String>,
    /// Why it failed; the connection is over.
    pub error: Option<String>,
}

/// Chauffeur installing its own Ollama (`ollama.install`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct OllamaInstalling {
    /// `downloading`, `checking` or `unpacking`.
    pub phase: String,
    /// Bytes downloaded and to download; 0 when the server gives no size.
    #[ts(type = "number")]
    pub completed: u64,
    #[ts(type = "number")]
    pub total: u64,
    /// Why it failed; the installation is over.
    pub error: Option<String>,
}

/// The connected Slack profile; credentials never cross the app RPC boundary.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SlackIdentity {
    pub user_id: String,
    pub team_id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SlackStatus {
    pub identity: Option<SlackIdentity>,
    pub pending: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SlackLogin {
    pub url: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SlackContextParams {
    pub link: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SlackContext {
    pub prompt: String,
    pub warning: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SlackReceiveParams {
    pub focused: bool,
    pub ack: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SlackDelivery {
    pub link: Option<String>,
}
