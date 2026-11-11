// App-wide state: the daemon connection, the workspaces and chats (kept live by a registry
// subscription), the selection. Unread markers live in the daemon (`WorkspaceInfo.unread`).

import type {
  Appearance,
  AgentKind,
  AgentModels,
  AgentUsage,
  ChatInfo,
  EventEnvelope,
  ExtensionInfo,
  IdentityState,
  KeyboardSettings,
  LoadoutEntry,
  PermissionMode,
  RepoInfo,
  RoutineFindInfo,
  RoutineFindParams,
  RoutineInfo,
  RoutineSuggestion,
  SectionInfo,
  SlashCommand,
  TicketRef,
  TransferInfo,
  WorkspaceInfo,
  DesignShareState,
  Mentioned,
  ShareConnection,
} from "@chauffeur/protocol";
import { create } from "zustand";
import { availableLayout, viewAvailable } from "@/lib/extensions";
import { SIGN_IN_BANNER_KEY, visibleExtensions } from "@/lib/members";
import { attentionText, chatAttention, needingYou } from "@/lib/attention";
import {
  type AwaitedServer,
  awaitedAfterReload,
  isBroken,
  loginCommand,
  mcpLoginCommand,
  mcpStatusText,
  newlyNeeded,
  reloadSummary,
  settledServers,
  signedOutSince,
} from "@/lib/connectors";
import { openChats, shownChat, withoutChat } from "@/lib/chatTabs";
import { drafts } from "@/lib/drafts";
import { basename, buildIndex, type FileIndex } from "@/lib/files";
import { workspaceName } from "@/lib/format";
import { appendDraft, type PageScreen } from "@/lib/designHandoff";
import { AGENT_NAMES, effectiveLoadout, initialChoice, type ModelChoice } from "@/lib/models";
import { notify, openExternal, resolveConnection, setAppZoom, setMenuKeys, setTrayAttention, windowFocused } from "@/lib/native";
import { stepZoom, storedZoom, ZOOM_KEY } from "@/lib/appZoom";
import { clampSidebarWidth, SIDEBAR_WIDTH_KEY, storedSidebarWidth } from "@/lib/sidebarWidth";
import { inBrowserOffice } from "@/lib/browserOffice";
import { applyFonts } from "@/lib/fonts";
import { setOverrides } from "@/lib/shortcuts";
import { applyPalette } from "@/lib/palette";
import { forgetSoundFile, playSound, unlockAudio } from "@/lib/sounds";
import { endedNotification, stillIdle, transferCopy } from "@/lib/transfer";
import { excerpt as mentionExcerpt } from "@/lib/mentions";
import { canStep, EMPTY_NAV, step, visit, type NavHistory } from "@/lib/navHistory";
import { neighbourOf, orderedWorkspaces } from "@/lib/workspaceOrder";
import type { DesignOption } from "@/lib/designs";
import type { LinkedTicket } from "@/lib/newWorkspace";
import { DEFAULT_LAYOUT, defaultMain, LAYOUTS_KEY, parseLayouts, showView, type SplitLayout, type ViewId } from "@/lib/splitLayout";

export { neighbourOf, orderedWorkspaces };
import { type ConnectionState, DaemonClient, errorMessage } from "@/lib/rpc";
import { installErrorReports, rememberFailure, setErrorReports } from "./errorReports";
import { toast } from "./toasts";

// The connection and the store are singletons: an edit reaching this module reloads the page
// rather than leaving a second, unstarted client behind (development only).
import.meta.hot?.accept(() => location.reload());

export { dismissToast, toast } from "./toasts";

export const client = new DaemonClient({
  resolve: resolveConnection,
  clientName: "chauffeur-app 0.37.0",
  onCallError: rememberFailure,
});
installErrorReports(client);

export type Tab = ViewId;

export type FileIndexState = { index: FileIndex | null; loading: boolean; error: string | null; at: number };

/** What the Files tab is asked to do next; `nonce` tells two identical requests apart. */
export type FilesRequest =
  | { kind: "open"; path: string; line: number | null; nonce: number }
  | { kind: "goto"; nonce: number }
  | { kind: "search"; nonce: number };

export type SlashCommandList = { commands: SlashCommand[]; loading: boolean; error: string | null; at: number };

export type AppState = {
  connection: ConnectionState;
  workspaces: Record<string, WorkspaceInfo>;
  workspacesLoaded: boolean;
  /** The sidebar's sections, in order (kept by the daemon). */
  sections: SectionInfo[];
  /** The routines, by id (docs/adr/routines.md). */
  routines: Record<string, RoutineInfo>;
  /** Find routines: its analysis under way and the suggestions waiting (docs/adr/find-routines.md). */
  routineFind: RoutineFindInfo | null;
  /** Find routines' dialog is open. */
  findRoutinesShown: boolean;
  chats: Record<string, ChatInfo>;
  selectedId: string | null;
  /** The workspaces shown before and after the selected one, for the back and forward arrows. */
  nav: NavHistory;
  /** Selected chat per workspace. */
  selectedChat: Record<string, string>;
  /** The main pane's view per workspace. */
  tab: Record<string, Tab>;
  /** The workspace view's split per workspace (kept across restarts). */
  split: Record<string, SplitLayout>;
  /** The split waits while the view area is too small for it. */
  splitNarrow: Record<string, boolean>;
  /** Bumped to focus a workspace's side pane. */
  splitFocus: Record<string, number>;
  createOpen: boolean;
  createWorkspaceShare: string;
  createPrefill: string;
  /** The create dialog opens on an exploration rather than a workspace. */
  createExplore: boolean;
  /** The create dialog makes a child workspace of this one. */
  createParent: string | null;
  switcherOpen: boolean;
  /** What the create dialog was filled with, by workspace still being created. */
  creations: Record<string, CreateDraft>;
  /** A creation that failed after its dialog closed: the dialog reopens with it. */
  createRetry: { draft: CreateDraft; error: string } | null;
  /** The switcher lists the archived workspaces only. */
  switcherArchived: boolean;
  searchTarget: { chat: string; seq: number; word: string } | null;
  settingsOpen: boolean;
  /** The section the settings window opens at, when asked for one. */
  settingsSection: string | null;
  /** The sidebar is folded away (⌘B), to leave the room to the workspace. */
  sidebarHidden: boolean;
  /** The sidebar's width in pixels, dragged from its right edge. */
  sidebarWidth: number;
  /** The 3D Office (docs/office.md) fills the main area instead of the selected workspace. */
  officeShown: boolean;
  /** The daemon's Office is on (`office.state` says it is configured); off, nothing shows it. */
  officeEnabled: boolean;
  /** The extensions this user is offered (`visibleExtensions`); null until read from the daemon. */
  extensions: ExtensionInfo[] | null;
  /** Chauffeur's sign-in, and whether it makes the user a member; null until read from the daemon. */
  identity: IdentityState | null;
  /** The chat open over the Office, beside the scene; null while the user only walks. */
  officeChat: OfficeChat | null;
  /** A member's quarter the user asked to go to (⌘K); `at` tells two asks apart. */
  officeTravel: { to: string; at: number } | null;
  /** A workspace whose house the user's character goes to; `at` tells two asks apart. */
  officeVisit: { workspace: string; at: number } | null;
  /** Agents waiting for their owner in the Office, by owner's first name: the go-to list shows them. */
  officeWaiting: Record<string, number>;
  /** The whole app's zoom (⌘+ / ⌘− / ⌘0), 1 for 100%. */
  zoom: number;
  /** The user's fonts (Settings › Appearance); null until read from the daemon. */
  appearance: Appearance | null;
  /** Played when a chat finishes or needs the user while the app is not in front ("": none). */
  notificationSound: string;
  /** Settings › Keyboard; Vim off and the default keys until read from the daemon. */
  keyboard: KeyboardSettings;
  repos: RepoInfo[] | null;
  reposError: string | null;
  /** When GitHub last listed the repos, and why it could not the last time it was asked. */
  reposSync: { refreshedAt: string | null; githubError: string | null };
  /** The agents' model catalogs; null until listed. */
  models: AgentModels[] | null;
  modelsLoading: boolean;
  /** The saved loadout; empty means the default one (see effectiveLoadout). */
  loadout: LoadoutEntry[] | null;
  /** What `/` offers, by `commandsKey`; absent until asked for. */
  commands: Record<string, SlashCommandList>;
  /** Each workspace's files, once listed. */
  fileIndex: Record<string, FileIndexState>;
  filesRequest: Record<string, FilesRequest>;
  /** A command for a workspace's Terminal tab to run in a new terminal (a login to redo). */
  terminalRequest: Record<string, TerminalRequest>;
  /** The agents' plan limits and logins; null until read. */
  usage: AgentUsage[] | null;
  /** Agents whose login Chauffeur started and that have not come back yet. */
  signingIn: AgentKind[];
  /** The screens selected on a design chat's canvas, by chat (their files; story 47). */
  canvasSelection: Record<string, string[]>;
  /** The screens of the canvas page a design chat shows, by chat: Open › Browser opens one. */
  canvasPage: Record<string, PageScreen[]>;
  /** Each design chat's share (story 51), by chat, as the daemon last told it. */
  designShares: Record<string, DesignShareState>;
  /** The user's mentions in shares' comments, newest first, as the daemon last told them. */
  mentions: Mentioned[];
  mentionsConnection: ShareConnection;
  /** A thread a design chat's canvas, or a shared workspace's chat, opens once it shows (a mention clicked in the sidebar). */
  threadTarget: { chat: string; thread: string } | null;
  /** A design chat's tab shows its canvas alone, the chat column hidden (⌘\, kept for the app). */
  designChatHidden: boolean;
  /** The parents whose child workspaces are folded in the sidebar (kept by the app, per machine). */
  collapsedParents: string[];
};

export type TerminalRequest = { command: string; nonce: number };

const SELECTED_KEY = "chauffeur.selected";
const SIDEBAR_KEY = "chauffeur.sidebarHidden";
const DESIGN_CHAT_HIDDEN_KEY = "chauffeur.designChatHidden";
const COLLAPSED_PARENTS_KEY = "chauffeur.collapsedParents";

function storedCollapsedParents(raw: string | null): string[] {
  try {
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed.filter((id): id is string => typeof id === "string") : [];
  } catch {
    return [];
  }
}

export const useApp = create<AppState>(() => ({
  connection: client.state,
  workspaces: {},
  workspacesLoaded: false,
  sections: [],
  routines: {},
  routineFind: null,
  findRoutinesShown: false,
  chats: {},
  selectedId: localStorage.getItem(SELECTED_KEY),
  nav: EMPTY_NAV,
  selectedChat: {},
  tab: {},
  split: parseLayouts(localStorage.getItem(LAYOUTS_KEY)),
  splitNarrow: {},
  splitFocus: {},
  createOpen: false,
  createWorkspaceShare: "",
  createPrefill: "",
  createExplore: false,
  createParent: null,
  switcherOpen: false,
  creations: {},
  createRetry: null,
  switcherArchived: false,
  searchTarget: null,
  settingsOpen: false,
  settingsSection: null,
  sidebarHidden: localStorage.getItem(SIDEBAR_KEY) === "1",
  sidebarWidth: storedSidebarWidth(localStorage.getItem(SIDEBAR_WIDTH_KEY)),
  officeShown: false,
  officeEnabled: false,
  extensions: null,
  identity: null,
  officeChat: null,
  officeTravel: null,
  officeVisit: null,
  officeWaiting: {},
  zoom: storedZoom(localStorage.getItem(ZOOM_KEY)),
  appearance: null,
  notificationSound: "",
  keyboard: { vim_mode: false, bindings: {}, binding_errors: [] },
  repos: null,
  reposError: null,
  reposSync: { refreshedAt: null, githubError: null },
  models: null,
  modelsLoading: false,
  loadout: null,
  commands: {},
  fileIndex: {},
  filesRequest: {},
  terminalRequest: {},
  usage: null,
  signingIn: [],
  canvasSelection: {},
  canvasPage: {},
  designShares: {},
  mentions: [],
  mentionsConnection: { state: "off" },
  threadTarget: null,
  designChatHidden: localStorage.getItem(DESIGN_CHAT_HIDDEN_KEY) === "1",
  collapsedParents: storedCollapsedParents(localStorage.getItem(COLLAPSED_PARENTS_KEY)),
}));

const set = useApp.setState;
const get = useApp.getState;

// --- actions ---

/**
 * The user picks a workspace (the sidebar, ⌘1–9, the switcher, a chat's link): it shows; in the
 * Office, its chat opens beside the scene and the user stays there. Only the 3D button and the
 * panel's ↗ (`leaveOfficeFor`) take them out of the Office.
 */
export function selectWorkspace(id: string | null) {
  if (id && get().officeShown) {
    visitHouse({ workspace: id });
    return;
  }
  select(id);
}

/** Leaves the Office for a workspace's full view: the chat panel's ↗. */
export function leaveOfficeFor(id: string) {
  select(id);
  set({ officeShown: false });
}

function select(id: string | null) {
  if (id) localStorage.setItem(SELECTED_KEY, id);
  else localStorage.removeItem(SELECTED_KEY);
  const { selectedId, nav } = get();
  set({ selectedId: id, nav: id ? visit(nav, selectedId, id) : nav, switcherOpen: false });
  keptUnread = null;
  if (id) markSeen(id);
}

/** Whether the user can go back (-1) or forward (1) to a workspace they were on. */
export function canGoInHistory(state: Pick<AppState, "nav" | "workspaces">, direction: 1 | -1): boolean {
  return canStep(state.nav, direction, (id) => reachable(state.workspaces, id));
}

/** Goes back (-1) to the workspace shown before this one, or forward (1) again; not in the Office. */
export function goInHistory(direction: 1 | -1) {
  const { officeShown, workspaces, nav, selectedId } = get();
  if (officeShown) return;
  const target = step(nav, selectedId, direction, (id) => reachable(workspaces, id));
  if (!target) return;
  select(target.id);
  set({ nav: target.history });
}

/** A workspace the arrows can show: one that is not archived or being archived. */
function reachable(workspaces: Record<string, WorkspaceInfo>, id: string): boolean {
  const state = workspaces[id]?.state;
  return state !== undefined && state !== "archived" && state !== "archiving";
}

/** Shows the 3D Office in the main area, or the selected workspace again. */
export function showOffice(shown = !get().officeShown) {
  if (shown && !get().officeEnabled) return;
  set({ officeShown: shown });
}

/** Said in the Office in the browser of what only the app does. */
const IN_THE_APP = "The chats and workspaces are in the Chauffeur app: this page shows the Office only.";

/** A workspace's chat to show beside the Office's scene: that one, or the one its tabs show. */
export type OfficeChat = { workspace: string; chat?: string };

/** Opens a chat beside the Office's scene, the user still in the Office; null closes it. */
export function openOfficeChat(target: OfficeChat | null) {
  if (target && inBrowserOffice) return toast(IN_THE_APP);
  if (target?.chat) selectChat(target.workspace, target.chat);
  set({ officeChat: target, switcherOpen: false });
}

/** Takes the user's character to a workspace's house in the Office, its chat open over the scene. */
export function visitHouse(target: OfficeChat) {
  openOfficeChat(target);
  set({ officeVisit: { workspace: target.workspace, at: Date.now() } });
}

/** Takes the user's character to a member's quarter of the Office. */
export function goToMember(name: string) {
  set({ officeTravel: { to: name, at: Date.now() }, switcherOpen: false });
}

/** The Office tells the go-to list who has agents waiting. */
export function setOfficeWaiting(waiting: Record<string, number>) {
  set({ officeWaiting: waiting });
}

/** A workspace asked for from the keyboard (⌘1–9, the switcher): in the Office, beside the scene. */
export function goToWorkspace(id: string) {
  selectWorkspace(id);
}

/** A workspace marked unread while shown: it stays unread until selected again. */
let keptUnread: string | null = null;

/** The user looks at a workspace: the daemon clears its unread marker. */
function markSeen(id: string) {
  if (!get().workspaces[id]?.unread || id === keptUnread) return;
  client.call("workspace.seen", { workspace: id }).catch(() => {});
}

/** Shows a workspace as unread again, or as read. */
export async function setUnread(id: string, unread: boolean) {
  try {
    if (unread) {
      if (id === get().selectedId) keptUnread = id;
      upsertWorkspace(await client.call("workspace.mark_unread", { workspace: id }));
    } else {
      if (id === keptUnread) keptUnread = null;
      await client.call("workspace.seen", { workspace: id });
    }
  } catch (error) {
    toast(`Could not mark it ${unread ? "unread" : "read"}: ${errorMessage(error)}`, "error");
  }
}

/** Keeps a workspace at the top of the sidebar, or not. */
export async function pinWorkspace(id: string, pinned: boolean) {
  try {
    upsertWorkspace(await client.call("workspace.pin", { workspace: id, pinned }));
  } catch (error) {
    toast(`Could not ${pinned ? "pin" : "unpin"} it: ${errorMessage(error)}`, "error");
  }
}

// --- sections: user-named groups of the sidebar, kept by the daemon ---

/** Creates a section, last of them; `workspace`, when given, is filed in it. */
export async function createSection(name: string, workspace?: string): Promise<boolean> {
  try {
    await client.call("section.create", { name, workspace });
    return true;
  } catch (error) {
    toast(`Could not create the section: ${errorMessage(error)}`, "error");
    return false;
  }
}

export async function renameSection(section: string, name: string) {
  try {
    await client.call("section.rename", { section, name });
  } catch (error) {
    toast(`Could not rename the section: ${errorMessage(error)}`, "error");
  }
}

/** Folds or unfolds a section; shown at once, the daemon remembers it. */
export async function foldSection(section: string, folded: boolean) {
  set((s) => ({ sections: s.sections.map((x) => (x.id === section ? { ...x, folded } : x)) }));
  try {
    await client.call("section.fold", { section, folded });
  } catch (error) {
    set((s) => ({ sections: s.sections.map((x) => (x.id === section ? { ...x, folded: !folded } : x)) }));
    toast(`Could not ${folded ? "fold" : "unfold"} the section: ${errorMessage(error)}`, "error");
  }
}

/** Moves a section above `before`, or last. */
export async function moveSection(section: string, before: string | null) {
  try {
    set({ sections: (await client.call("section.move", { section, before: before ?? undefined })).sections });
  } catch (error) {
    toast(`Could not move the section: ${errorMessage(error)}`, "error");
  }
}

/** Deletes a section: its workspaces go back to "Workspaces". */
export async function deleteSection(section: SectionInfo) {
  try {
    set({ sections: (await client.call("section.delete", { section: section.id })).sections });
    toast(`Section ${section.name} deleted; its workspaces are back in Workspaces`);
  } catch (error) {
    toast(`Could not delete the section: ${errorMessage(error)}`, "error");
  }
}

/** Files a workspace in a section, or in none. */
export async function setWorkspaceSection(workspace: string, section: string | null) {
  try {
    upsertWorkspace(await client.call("workspace.set_section", { workspace, section }));
  } catch (error) {
    toast(`Could not move it: ${errorMessage(error)}`, "error");
  }
}

/** Whether the user is looking at this workspace right now. */
function viewing(id: string): boolean {
  return windowFocused() && get().selectedId === id;
}

/** Opens an archived conversation at the message found, without changing its closed state. */
export function openSearchMatch(workspace: string, chat: string, seq: number, word: string) {
  selectWorkspace(workspace);
  selectChat(workspace, chat);
  setTab(workspace, "chat");
  set({ searchTarget: { chat, seq, word } });
}

export function clearSearchMatch() {
  set({ searchTarget: null });
}

export function selectChat(workspaceId: string, chatId: string) {
  set((s) => ({ selectedChat: { ...s.selectedChat, [workspaceId]: chatId } }));
}

export function setTab(workspaceId: string, tab: Tab) {
  if (!viewAvailable(get().extensions, tab)) return;
  set((s) => viewShown(s, workspaceId, tab));
}

/** The split of a workspace's view: its side pane, where, how big. */
export function splitOf(s: AppState, workspaceId: string): SplitLayout {
  return s.split[workspaceId] ?? DEFAULT_LAYOUT;
}

/** The view a workspace's main pane shows. */
export function mainView(s: AppState, workspaceId: string): Tab {
  const tab = s.tab[workspaceId];
  return tab && viewAvailable(s.extensions, tab) ? tab : defaultMain(splitOf(s, workspaceId));
}

/** Shows a view: in the main pane, or focused in the side pane when that one shows it. */
function viewShown(s: AppState, workspaceId: string, view: Tab): Partial<AppState> {
  const shown = showView(splitOf(s, workspaceId), mainView(s, workspaceId), view, s.splitNarrow[workspaceId] ?? false);
  if (shown.focusSide) return { splitFocus: { ...s.splitFocus, [workspaceId]: (s.splitFocus[workspaceId] ?? 0) + 1 } };
  return { tab: { ...s.tab, [workspaceId]: shown.main } };
}

const NO_FILES: FileIndexState = { index: null, loading: false, error: null, at: 0 };

const fileIndexLoads = new Map<string, Promise<void>>();

/** Lists a workspace's files, unless a list fresher than `maxAgeMs` is there or on its way (then awaited). */
export function loadFileIndex(workspaceId: string, maxAgeMs = 60_000): Promise<void> {
  const loading = fileIndexLoads.get(workspaceId);
  if (loading) return loading;
  const current = get().fileIndex[workspaceId];
  if (current?.index && Date.now() - current.at < maxAgeMs) return Promise.resolve();
  const load = listFiles(workspaceId).finally(() => fileIndexLoads.delete(workspaceId));
  fileIndexLoads.set(workspaceId, load);
  return load;
}

/** Drops a workspace's list of files (up to 100,000 paths) once no view of it is kept: it is listed again when shown. */
export function forgetFileIndex(workspaceId: string) {
  if (!get().fileIndex[workspaceId]) return;
  set((s) => {
    const { [workspaceId]: _, ...rest } = s.fileIndex;
    return { fileIndex: rest };
  });
}

async function listFiles(workspaceId: string): Promise<void> {
  const patch = (state: Partial<FileIndexState>) =>
    set((s) => ({
      fileIndex: {
        ...s.fileIndex,
        [workspaceId]: { ...NO_FILES, ...s.fileIndex[workspaceId], ...state },
      },
    }));
  patch({ loading: true });
  try {
    const result = await client.call("files.list", { workspace: workspaceId });
    patch({ index: buildIndex(result.roots), loading: false, error: null, at: Date.now() });
  } catch (error) {
    patch({ loading: false, error: errorMessage(error), at: Date.now() });
  }
}

let filesNonce = 0;
type FilesRequestInput =
  | { kind: "open"; path: string; line: number | null }
  | { kind: "goto" }
  | { kind: "search" };

/** Shows the Files tab (focused, if in the side pane) and asks it to open a file, focus quick open, or search. */
export function requestFiles(workspaceId: string, request: FilesRequestInput) {
  const next = { ...request, nonce: ++filesNonce } as FilesRequest;
  set((s) => ({
    filesRequest: { ...s.filesRequest, [workspaceId]: next },
    ...viewShown(s, workspaceId, "files"),
  }));
}

let terminalNonce = 0;

/** Shows a workspace's Terminal tab and runs `command` in a new terminal there. */
export function requestTerminal(workspaceId: string, command: string) {
  set((s) => ({
    terminalRequest: { ...s.terminalRequest, [workspaceId]: { command, nonce: ++terminalNonce } },
    ...viewShown(s, workspaceId, "terminal"),
  }));
}

// --- agents' logins and MCP servers ---

/**
 * Reads the agents' plan limits and logins; an agent that just got signed out is notified (once:
 * the next readings find it signed out already).
 */
export async function loadUsage(refresh = false): Promise<void> {
  const result = await client.call("usage.get", { refresh });
  const lost = signedOutSince(get().usage, result.agents);
  const back = result.agents.filter((usage) => usage.account === "signed_in").map((usage) => usage.agent);
  set((s) => ({ usage: result.agents, signingIn: s.signingIn.filter((agent) => !back.includes(agent)) }));
  for (const usage of lost) {
    const what = `${AGENT_NAMES[usage.agent]} is signed out`;
    if (windowFocused()) toast(`${what}: sign in again from the bottom of the sidebar`, "error");
    else {
      void notify("Chauffeur", `${what}: its chats cannot run until you sign in again`);
      playNotificationSound().catch(() => {});
    }
  }
}

/**
 * Logs an agent back in: in the browser when the agent can start its login for Chauffeur,
 * else with its own command in a terminal of the workspace on screen.
 */
export async function signIn(agent: AgentKind): Promise<void> {
  const name = AGENT_NAMES[agent];
  try {
    const { url } = await client.call("agent.login", { agent });
    set((s) => ({ signingIn: [...s.signingIn.filter((a) => a !== agent), agent] }));
    await openExternal(url);
    toast(`Finish signing in to ${name} in your browser`);
  } catch (error) {
    const workspace = get().selectedId;
    const command = loginCommand(agent);
    if (workspace && get().workspaces[workspace]?.state === "ready") {
      requestTerminal(workspace, command);
      toast(`${name} could not start its login here (${errorMessage(error)}): finish it in the terminal`, "error");
    } else {
      toast(`${name} could not start its login (${errorMessage(error)}): run \`${command}\` in a terminal`, "error");
    }
  }
}

/**
 * Connects one of a chat's MCP servers again: its login page in the browser when it needs one,
 * a reconnection otherwise; its own command in a terminal of the chat's workspace when the agent
 * cannot do either.
 */
export async function connectMcp(chat: ChatInfo, server: string): Promise<void> {
  try {
    const { auth_url } = await client.call("chat.mcp_connect", { chat: chat.id, server });
    if (auth_url) {
      await openExternal(auth_url);
      toast(`Finish logging in to ${server} in your browser`);
      return;
    }
    // Reconnected: the session reported the server again before answering.
    const now = get().chats[chat.id]?.mcp_servers.find((s) => s.name === server);
    if (now && isBroken(now)) toast(`${server} still does not connect: ${mcpStatusText(now)}`, "error");
    else toast(`${server} is connected again`);
  } catch (error) {
    requestTerminal(chat.workspace_id, mcpLoginCommand(chat.agent, server));
    toast(`Could not connect ${server} (${errorMessage(error)}): finish it in the terminal`, "error");
  }
}

/** Servers whose chat a reload restarted, until each comes up or fails again (said once). */
let awaitedServers: AwaitedServer[] = [];

/**
 * Reads the login shell's environment again (a variable the user just exported): the daemon
 * restarts idle chats whose MCP servers failed, and each server is announced once it connects.
 */
export async function reloadEnvironment(): Promise<void> {
  const before = get().chats;
  try {
    const result = await client.call("env.reload", {});
    const { message, level } = reloadSummary(result);
    toast(message, level);
    awaitedServers = [...awaitedServers, ...awaitedAfterReload(result, before)];
    // A restarted session may have reported its servers before the answer came.
    for (const chatId of result.restarted) {
      const chat = get().chats[chatId];
      if (chat) announceSettled(chat);
    }
    setTimeout(() => {
      awaitedServers = [];
    }, 90_000);
  } catch (error) {
    toast(`Could not reload the environment: ${errorMessage(error)}`, "error");
  }
}

function announceSettled(chat: ChatInfo) {
  if (awaitedServers.length === 0) return;
  const { connected, broken, rest } = settledServers(awaitedServers, chat);
  awaitedServers = rest;
  for (const name of connected) toast(`${name} connected`);
  for (const server of broken) toast(`${server.name} still does not connect: ${mcpStatusText(server)}`, "error");
}

export function openFile(workspaceId: string, path: string, line: number | null = null) {
  requestFiles(workspaceId, { kind: "open", path, line });
}

/** Shows a file of the workspace selected in Finder (its folder on Linux). */
export async function revealFile(workspaceId: string, path: string) {
  try {
    await client.call("files.reveal", { workspace: workspaceId, path });
  } catch (error) {
    toast(`Could not show ${basename(path)}: ${errorMessage(error)}`, "error");
  }
}

/** Opens the create dialog, for an exploration, or for a child workspace of `parent`. */
export function openCreate(open = true, explore = false, parent: string | null = null, workspaceShare = "") {
  if (open && inBrowserOffice) return toast(IN_THE_APP);
  set({ createWorkspaceShare: workspaceShare, createOpen: open, createExplore: explore, createParent: parent, switcherOpen: false, createPrefill: "" });
}

/** Prefills a new workspace, appending context when the user already has a form open. */
export function prefillCreate(prompt: string) {
  if (inBrowserOffice) return toast(IN_THE_APP);
  set((s) => ({
    createOpen: true,
    createWorkspaceShare: s.createOpen ? s.createWorkspaceShare : "",
    createPrefill: s.createPrefill ? `${s.createPrefill}\n\n${prompt}` : prompt,
    createExplore: s.createOpen ? s.createExplore : false,
    createParent: s.createOpen ? s.createParent : null,
    switcherOpen: false,
    settingsOpen: false,
    officeShown: false,
  }));
}

/** What the create dialog was filled with. */
export type CreateDraft = {
  designChat?: boolean;
  workspaceShare?: string;
  explore: boolean;
  repos: string[];
  tickets: TicketRef[];
  name: string;
  branch: string;
  milestone: string;
  choice: ModelChoice;
  mode: PermissionMode;
  prompt: string;
  parent: string | null;
  /** The design picked from the 3D view. */
  design: DesignOption | null;
  /** What the linked tickets brought: Linear's branch, their state and repos. */
  linked?: Record<string, LinkedTicket>;
  /** Which ticket added each repo it added. */
  repoSources?: Record<string, string>;
  /** The milestone's name, shown in its chip (`milestone` is its id). */
  milestoneName?: string;
  /** A shared design to start from, for a design (`<service>/s/<id>`). */
  designShare?: string;
};

export function rememberCreation(workspaceId: string, draft: CreateDraft) {
  set((s) => ({ creations: { ...s.creations, [workspaceId]: draft } }));
}

function forgetCreation(workspaceId: string): CreateDraft | undefined {
  const draft = get().creations[workspaceId];
  if (draft) {
    set((s) => {
      const creations = { ...s.creations };
      delete creations[workspaceId];
      return { creations };
    });
  }
  return draft;
}

/** The failed creation the create dialog opens with, once. */
export function takeCreateRetry(): { draft: CreateDraft; error: string } | null {
  const retry = get().createRetry;
  if (retry) set({ createRetry: null });
  return retry;
}

export function openSwitcher(open = true, archived = false) {
  set({ switcherOpen: open, switcherArchived: open && archived });
}

export function openSettings(open = true, section: string | null = null) {
  set({ settingsOpen: open, settingsSection: open ? section : null, switcherOpen: false });
}

/** The notification sound in force; a file picked again is fetched again. */
export function setNotificationSound(sound: string) {
  if (sound !== get().notificationSound) forgetSoundFile();
  set({ notificationSound: sound });
}

/** Plays the notification sound the user picked, if any. */
export function playNotificationSound(sound = get().notificationSound): Promise<void> {
  return playSound(sound, () => client.httpUrl("/sound"));
}

export function setKeyboard(keyboard: KeyboardSettings) {
  set({ keyboard });
  // A daemon older than the app sends no bindings: the defaults then.
  setOverrides(keyboard.bindings ?? {});
  setMenuKeys({ bindings: keyboard.bindings ?? {} });
}

/** The fonts in force, applied to the page and the terminals. */
export function setAppearance(appearance: Appearance) {
  set({ appearance });
  applyFonts(appearance);
  applyPalette(appearance.palette, appearance.theme_interface);
}

/** Reads the fonts, the sound, the keys and the error reports from the daemon: at connection, and when the app is back
 * in front (the user may have changed `settings.toml` by hand meanwhile). */
async function loadAppearance() {
  try {
    const settings = await client.call("settings.get", {});
    setErrorReports(settings);
    setAppearance(settings.appearance);
    setNotificationSound(settings.notifications.sound);
    setKeyboard(settings.keyboard);
  } catch {
    // The app's own fonts stay.
  }
}

/** The daemon's extensions as last read, before `visibleExtensions` leaves out what this user is not offered. */
let catalogue: ExtensionInfo[] | null = null;

/** Applies authoritative activation to every open workspace and discards disabled pane references. */
export function applyExtensions(all: ExtensionInfo[]) {
  catalogue = all;
  const extensions = visibleExtensions(all, get().identity?.member ?? false);
  set((state) => {
    const split = Object.fromEntries(Object.entries(state.split).map(([id, layout]) => [id, availableLayout(extensions, layout)]));
    const tab = Object.fromEntries(Object.entries(state.tab).map(([id, view]) => [id, viewAvailable(extensions, view) ? view : defaultMain(split[id] ?? DEFAULT_LAYOUT)]));
    const officeEnabled = viewAvailable(extensions, "office") && state.officeEnabled;
    return { extensions, split, tab, officeEnabled, ...(officeEnabled ? {} : { officeShown: false, officeChat: null }) };
  });
  try { localStorage.setItem(LAYOUTS_KEY, JSON.stringify(get().split)); }
  catch { /* The in-memory layout still holds when storage is unavailable. */ }
  void loadOffice();
}

export async function loadExtensions() {
  applyExtensions(await client.call("extensions.list", {}));
}

/** Chauffeur's sign-in as the daemon says it: what a member sees shows or goes at once (the Office, the menu's items). */
export function setIdentity(identity: IdentityState) {
  const wasMember = get().identity?.member ?? false;
  set({ identity });
  setMenuKeys({ member: identity.member });
  if (identity.email) {
    try { localStorage.setItem(SIGN_IN_BANNER_KEY, "1"); }
    catch { /* The banner shows only while signed out anyway. */ }
  }
  if (catalogue && identity.member !== wasMember) applyExtensions(catalogue);
}

async function loadIdentity() {
  try { setIdentity(await client.call("identity.state", {})); }
  catch (error) { toast(`Could not read your Chauffeur account: ${errorMessage(error)}`, "error"); }
}

async function loadOffice() {
  const configured = await client.call("office.state", {}).then((state) => state.configured, () => false);
  const officeEnabled = configured && viewAvailable(get().extensions, "office");
  set({ officeEnabled, ...(officeEnabled ? {} : { officeShown: false, officeChat: null }) });
}

/** Folds or unfolds the child workspaces shown under a parent in the sidebar. */
export function toggleChildren(parent: string, collapsed = !get().collapsedParents.includes(parent)) {
  const rest = get().collapsedParents.filter((id) => id !== parent);
  const collapsedParents = collapsed ? [...rest, parent] : rest;
  try {
    localStorage.setItem(COLLAPSED_PARENTS_KEY, JSON.stringify(collapsedParents));
  } catch {
    // Storage full or unavailable: it holds until the app restarts.
  }
  set({ collapsedParents });
}

export function toggleSidebar(hidden = !get().sidebarHidden) {
  localStorage.setItem(SIDEBAR_KEY, hidden ? "1" : "0");
  set({ sidebarHidden: hidden });
}

/** The sidebar's width while its edge is dragged; kept once `done` (the edge let go). */
export function resizeSidebar(width: number, done: boolean) {
  const sidebarWidth = clampSidebarWidth(width);
  set({ sidebarWidth });
  if (!done) return;
  try {
    localStorage.setItem(SIDEBAR_WIDTH_KEY, String(sidebarWidth));
  } catch {
    // Storage full or unavailable: the width holds until the app restarts.
  }
}

/** Zooms the whole app a step in (1), out (-1) or back to 100% (0). */
export function zoomApp(direction: 1 | -1 | 0) {
  const zoom = stepZoom(get().zoom, direction);
  localStorage.setItem(ZOOM_KEY, String(zoom));
  set({ zoom });
  void setAppZoom(zoom);
  toast(`Zoom ${Math.round(zoom * 100)}%`);
}

/** The screens selected on a design chat's canvas (stage 9c sends them to the source chat). */
export function setCanvasSelection(chatId: string, files: string[]) {
  const current = get().canvasSelection[chatId] ?? [];
  if (current.length === files.length && current.every((file, i) => file === files[i])) return;
  set((s) => ({ canvasSelection: { ...s.canvasSelection, [chatId]: files } }));
}

/** A design chat's share as the daemon tells it (a call's answer, or its event). */
export function setDesignShare(state: DesignShareState) {
  set((s) => ({ designShares: { ...s.designShares, [state.chat]: state } }));
}

/**
 * A reviewer's comment reached the design: the user is told unless they are looking at the
 * workspace already (the canvas shows the pin). One that mentions them is told by the mentions.
 */
export function reviewerCommented(workspaceId: string, state: DesignShareState) {
  const before = get().designShares[state.chat]?.last_reviewer_comment;
  const last = state.last_reviewer_comment;
  setDesignShare(state);
  if (!last || (before && before.at === last.at && before.author === last.author)) return;
  if (last.mentioned) return;
  // A markdown file's share (story 57) is keyed `file:<workspace>:<path>`, its comments on the file's name.
  const on = state.chat.startsWith("file:") ? last.screen : (get().chats[state.chat]?.title ?? "a design");
  const what = `${last.author} commented on ${on}: ${last.text}`;
  if (viewing(workspaceId)) toast(what);
  else void notify(get().workspaces[workspaceId]?.slug ?? "Chauffeur", what);
}

// --- mentions (docs/adr/mentions-in-share-comments.md) ---

/** The user's mentions, read from the daemon (at each connection: its events say what changes). */
async function loadMentions() {
  try {
    const result = await client.call("mentions.list", {});
    set({ mentions: result.mentions, mentionsConnection: result.connection });
  } catch (error) {
    // A daemon without mentions, or none yet: the list stays empty.
    console.warn("the mentions", errorMessage(error));
  }
}

/** The mentions changed: the new ones are told, as a toast, or a notification when the window is not in front. */
function mentionsChanged(mentions: Mentioned[], fresh: readonly string[]) {
  set({ mentions });
  for (const id of fresh) {
    const mention = mentions.find((m) => m.id === id);
    if (!mention || mention.seen) continue;
    const what = `${mention.author.name} mentioned you on ${mention.share_title}: ${mentionExcerpt(mention.text)}`;
    if (windowFocused()) toast(what);
    else {
      void notify("Chauffeur", what);
      playNotificationSound().catch(() => {});
    }
  }
}

/** A mention's thread was opened: it is read, here and on the share service. */
export async function markMentionSeen(id: string) {
  const mention = get().mentions.find((m) => m.id === id);
  if (!mention || mention.seen) return;
  set((s) => ({ mentions: s.mentions.map((m) => (m.id === id ? { ...m, seen: true } : m)) }));
  try {
    const result = await client.call("mentions.seen", { id });
    set({ mentions: result.mentions, mentionsConnection: result.connection });
  } catch (error) {
    console.warn("marking a mention read", errorMessage(error));
  }
}

/**
 * Opens a mention's thread: on the user's own share, its design chat's canvas, or the shared
 * workspace's chat, when the app knows which chat it is; else the share's page in the browser.
 */
export function openMention(mention: Mentioned) {
  void markMentionSeen(mention.id);
  const state = get();
  // Only the owner's daemon holds a design chat's share state, and the chats of a shared
  // workspace: one that has them is the user's own.
  const shared = mention.place === "design" ? Object.values(state.designShares).find((d) => d.share?.id === mention.share_id) : undefined;
  const chat = shared ? state.chats[shared.chat] : mention.place === "workspace" && mention.chat ? state.chats[mention.chat] : undefined;
  if (chat) {
    selectWorkspace(chat.workspace_id);
    selectChat(chat.workspace_id, chat.id);
    setTab(chat.workspace_id, "chat");
    set({ threadTarget: { chat: chat.id, thread: mention.thread } });
    return;
  }
  void openExternal(mention.url);
}

/** The canvas, or the chat, showed the thread asked for. */
export function openThreadDone() {
  set({ threadTarget: null });
}

/** The screens of the page a design chat's canvas shows, for the header's Open › Browser. */
export function setCanvasPage(chatId: string, screens: PageScreen[]) {
  const current = get().canvasPage[chatId];
  if (current && current.length === screens.length && current.every((s, i) => s.file === screens[i].file && s.url === screens[i].url)) {
    return;
  }
  set((s) => ({ canvasPage: { ...s.canvasPage, [chatId]: screens } }));
}

/** ⌘\ on a design chat: hides or shows its chat column, the canvas taking the whole width. */
export function toggleDesignChat(hidden = !get().designChatHidden) {
  try {
    localStorage.setItem(DESIGN_CHAT_HIDDEN_KEY, hidden ? "1" : "0");
  } catch {
    // Storage full or unavailable: it holds until the app restarts.
  }
  set({ designChatHidden: hidden });
}

/**
 * "Send to source chat" (story 47): the message goes below the chat's draft, then the chat shows
 * (reopened if closed). Nothing is sent. `target` null starts a new chat for it.
 */
export async function sendToChat(workspaceId: string, target: ChatInfo | null, message: string): Promise<void> {
  let chat = target;
  if (!chat) {
    try {
      const { loadout, models } = get();
      const choice = initialChoice(effectiveLoadout(loadout, models));
      chat = await client.call("chat.create", {
        workspace: workspaceId,
        agent: choice.agent,
        model: choice.model ?? undefined,
        effort: choice.effort ?? undefined,
        fast: choice.fast || undefined,
      });
      upsertChats([chat]);
    } catch (error) {
      toast(`Could not start a chat: ${errorMessage(error)}`, "error");
      return;
    }
  }
  // Before the chat shows: its composer reads its draft when it mounts.
  drafts.setText(chat.id, appendDraft(drafts.text(chat.id), message));
  if (chat.closed) await reopenChat(chat.id);
  else selectChat(workspaceId, chat.id);
  set((s) => viewShown(s, workspaceId, "chat"));
}

/**
 * A design chat's own buttons ("Send N new to the agent", "Keep these"; story 51): the message is
 * sent to the chat, queued for after the agent's turn when it works; the draft is left alone.
 */
export async function sendNow(chat: ChatInfo, message: string): Promise<boolean> {
  try {
    await client.call("chat.send", { chat: chat.id, text: message, mode: "queue" });
    return true;
  } catch (error) {
    toast(`Could not send: ${errorMessage(error)}`, "error");
    return false;
  }
}

export function upsertWorkspace(workspace: WorkspaceInfo) {
  const before = get().workspaces;
  set((s) => ({ workspaces: { ...s.workspaces, [workspace.id]: workspace } }));
  leaveWhenArchiving(before, workspace);
}

/**
 * A workspace leaves the sidebar as soon as it starts archiving (that goes on in the background):
 * the one the user was looking at gives way to its neighbour in the list.
 */
function leaveWhenArchiving(before: Record<string, WorkspaceInfo>, workspace: WorkspaceInfo) {
  if (workspace.state !== "archiving" || get().selectedId !== workspace.id) return;
  const next = neighbourOf(before, workspace.id, get().sections);
  // Archiving goes on in the background: it does not take the user out of the Office.
  if (next !== undefined) select(next);
}

export function upsertChats(chats: ChatInfo[]) {
  if (chats.length === 0) return;
  set((s) => {
    const next = { ...s.chats };
    for (const chat of chats) next[chat.id] = chat;
    return { chats: next };
  });
}

/** Every chat of a workspace, closed ones too, into the store: its views count and show them. */
export function loadChats(workspaceId: string) {
  client
    .call("chat.list", { workspace: workspaceId, all: true })
    .then((result) => upsertChats(result.chats))
    .catch(() => {});
}

/** A chat deleted (here or elsewhere): out of the store, its draft forgotten. */
export function removeChat(chatId: string) {
  set((s) => withoutChat(s, chatId));
  drafts.setText(chatId, "");
  drafts.setFiles(chatId, []);
}

/** Chats closed from this window, the last one last: ⇧⌘T reopens from the end. */
export const closedHere: string[] = [];

/** Changes a chat's title; empty gives back the automatic one. */
export async function renameChat(chatId: string, title: string): Promise<void> {
  try {
    upsertChats([await client.call("chat.rename", { chat: chatId, title })]);
  } catch (error) {
    toast(`Could not rename the chat: ${errorMessage(error)}`, "error");
  }
}

/** Closes a chat (its conversation kept); `force` stops its agent first. Whether it closed. */
export async function closeChat(chatId: string, force: boolean): Promise<boolean> {
  try {
    upsertChats([await client.call("chat.close", { chat: chatId, force })]);
    closedHere.push(chatId);
    return true;
  } catch (error) {
    toast(`Could not close the chat: ${errorMessage(error)}`, "error");
    return false;
  }
}

/** Puts a closed chat back among the tabs and shows it. */
export async function reopenChat(chatId: string): Promise<void> {
  try {
    const chat = await client.call("chat.reopen", { chat: chatId });
    upsertChats([chat]);
    selectChat(chat.workspace_id, chat.id);
    const index = closedHere.lastIndexOf(chatId);
    if (index >= 0) closedHere.splice(index, 1);
  } catch (error) {
    toast(`Could not reopen the chat: ${errorMessage(error)}`, "error");
  }
}

/**
 * Forks a chat into a new tab, with its conversation so far or up to the end of its turn `turn`,
 * and shows the fork; into `agent` when given, the other one reading the conversation as text.
 */
export async function forkChat(chatId: string, turn?: number, agent?: AgentKind): Promise<void> {
  try {
    const chat = await client.call("chat.fork", { chat: chatId, ...(turn === undefined ? {} : { turn }), ...(agent ? { agent } : {}) });
    upsertChats([chat]);
    selectChat(chat.workspace_id, chat.id);
  } catch (error) {
    toast(`Could not fork the chat: ${errorMessage(error)}`, "error");
  }
}

/** Deletes a chat and its conversation for good; `force` stops its agent first. Whether it went. */
export async function deleteChat(chatId: string, force: boolean): Promise<boolean> {
  try {
    await client.call("chat.delete", { chat: chatId, force });
    removeChat(chatId);
    return true;
  } catch (error) {
    toast(`Could not delete the chat: ${errorMessage(error)}`, "error");
    return false;
  }
}

export async function loadRepos(refresh = false) {
  try {
    const result = await client.call("repos.list", { refresh });
    set({
      repos: result.repos,
      reposError: null,
      reposSync: { refreshedAt: result.refreshed_at ?? null, githubError: result.github_error ?? null },
    });
  } catch (error) {
    set({ reposError: errorMessage(error) });
  }
}

export async function loadModels(refresh = false) {
  if (get().modelsLoading) return;
  set({ modelsLoading: true });
  try {
    const result = await client.call("models.list", { refresh });
    set({ models: result.agents });
  } catch (error) {
    toast(`Could not list the models: ${errorMessage(error)}`, "error");
  } finally {
    set({ modelsLoading: false });
  }
}

/** Whose commands `/` lists: a workspace's chats, or those of a workspace not created yet, by its repos. */
export type CommandsSource = { agent: AgentKind } & ({ workspaceId: string } | { repos: string[] });

export function commandsKey(source: CommandsSource): string {
  return "workspaceId" in source
    ? `${source.workspaceId}/${source.agent}`
    : `repos:${source.repos.join(",")}/${source.agent}`;
}

/** A workspace's `/` commands are listed again when older than this; skills rarely change. */
const COMMANDS_TTL_MS = 60_000;

/** Lists what `/` offers in chats of `source`, unless a fresh list is at hand. */
export async function loadCommands(source: CommandsSource) {
  const key = commandsKey(source);
  const current = get().commands[key];
  if (current && (current.loading || Date.now() - current.at < COMMANDS_TTL_MS)) return;
  const store = (list: Partial<SlashCommandList>) =>
    set((s) => ({
      commands: {
        ...s.commands,
        [key]: { ...(s.commands[key] ?? { commands: [], loading: false, error: null, at: 0 }), ...list },
      },
    }));
  store({ loading: true });
  try {
    const result =
      "workspaceId" in source
        ? await client.call("commands.list", { workspace: source.workspaceId, agent: source.agent })
        : await client.call("commands.for_repos", { repos: source.repos, agent: source.agent });
    store({ commands: result.commands, loading: false, error: null, at: Date.now() });
  } catch (error) {
    store({ loading: false, error: errorMessage(error), at: Date.now() });
  }
}

export async function loadLoadout() {
  try {
    const result = await client.call("loadout.get", {});
    set({ loadout: result.entries });
  } catch (error) {
    toast(`Could not read the model loadout: ${errorMessage(error)}`, "error");
  }
}

export async function saveLoadout(entries: LoadoutEntry[]) {
  const previous = get().loadout;
  set({ loadout: entries });
  try {
    await client.call("loadout.set", { entries });
  } catch (error) {
    set({ loadout: previous });
    toast(`Could not save the model loadout: ${errorMessage(error)}`, "error");
  }
}

export async function refreshWorkspaces() {
  try {
    const result = await client.call("workspace.list", { include_archived: true });
    const workspaces: Record<string, WorkspaceInfo> = {};
    for (const workspace of result.workspaces) workspaces[workspace.id] = workspace;
    set({ workspaces, sections: result.sections ?? [], workspacesLoaded: true });
    const selected = get().selectedId;
    if (selected && viewing(selected)) markSeen(selected);
  } catch (error) {
    toast(`Could not list workspaces: ${errorMessage(error)}`, "error");
  }
}

export async function loadRoutines() {
  try {
    const result = await client.call("routine.list", {});
    set({ routines: Object.fromEntries(result.routines.map((routine) => [routine.id, routine])) });
  } catch (error) {
    toast(`Could not list the routines: ${errorMessage(error)}`, "error");
  }
}

export function upsertRoutine(routine: RoutineInfo) {
  set((s) => ({ routines: { ...s.routines, [routine.id]: routine } }));
}

/**
 * Makes a draft routine from a chat (docs/adr/routines.md): a workspace of its own, created with
 * its worktrees, then shown on its Routine view while its refining chat drafts the file.
 */
export async function makeRoutine(chat: ChatInfo) {
  toast("Making a routine from this chat: creating its workspace…");
  try {
    const routine = await client.call("routine.create", { chat: chat.id });
    upsertRoutine(routine);
    selectWorkspace(routine.workspace_id);
    setTab(routine.workspace_id, "routine");
  } catch (error) {
    toast(`Could not make a routine from this chat: ${errorMessage(error)}`, "error");
  }
}

export async function loadRoutineFind() {
  try {
    set({ routineFind: await client.call("routine.suggestions", {}) });
  } catch (error) {
    toast(`Could not read the routine suggestions: ${errorMessage(error)}`, "error");
  }
}

export function showFindRoutines(shown: boolean) {
  set({ findRoutinesShown: shown });
}

/** Starts Find routines; its suggestions come with `routine_suggestions_changed`. */
export async function findRoutines(params: RoutineFindParams): Promise<boolean> {
  try {
    set({ routineFind: await client.call("routine.find", params) });
    return true;
  } catch (error) {
    toast(`Could not start Find routines: ${errorMessage(error)}`, "error");
    return false;
  }
}

/** Makes a suggestion a draft routine, then shows it on its Routine view. */
export async function createSuggestedRoutine(suggestion: RoutineSuggestion) {
  toast(`Making the routine ${suggestion.spec.name}: creating its workspace…`);
  try {
    const routine = await client.call("routine.suggestion_create", { suggestion: suggestion.id });
    upsertRoutine(routine);
    set({ findRoutinesShown: false });
    selectWorkspace(routine.workspace_id);
    setTab(routine.workspace_id, "routine");
  } catch (error) {
    toast(`Could not make the routine ${suggestion.spec.name}: ${errorMessage(error)}`, "error");
  }
}

/** Keeps a suggestion for later, or ignores it for good. */
export async function setSuggestion(suggestion: RoutineSuggestion, state: "later" | "ignored") {
  try {
    set({ routineFind: await client.call("routine.suggestion_set", { suggestion: suggestion.id, state }) });
  } catch (error) {
    toast(`Could not ${state === "later" ? "keep" : "ignore"} ${suggestion.spec.name}: ${errorMessage(error)}`, "error");
  }
}

// --- registry: workspaces and chats, unread markers, notifications ---

function onChatUpserted(chat: ChatInfo) {
  const state = get();
  const workspace = state.workspaces[chat.workspace_id];
  const attention = chatAttention(state.chats[chat.id]?.status, chat.status, workspace?.activity);
  set((s) => ({ chats: { ...s.chats, [chat.id]: chat } }));
  announceSettled(chat);
  // A server the agent needed while it was broken: said once, while the app is not in front
  // (in front, the chat's composer shows it).
  const needed = newlyNeeded(state.chats[chat.id]?.mcp_servers, chat.mcp_servers);
  if (needed.length > 0 && !windowFocused()) {
    const names = needed.map((server) => server.name).join(", ");
    const what = `${names} ${needed.length === 1 ? "is" : "are"} disconnected`;
    void notify(workspace?.slug ?? "Chauffeur", chat.title ? `${what}: ${chat.title} needs it` : what);
    playNotificationSound().catch(() => {});
  }
  if (!attention) return;
  const focused = windowFocused();
  if (!focused) {
    const what = attentionText(attention, chat.agent === "codex" ? "Codex" : "Claude");
    void notify(workspace?.slug ?? "Chauffeur", chat.title ? `${what}: ${chat.title}` : what);
    playNotificationSound().catch(() => {});
  }
}

function onRegistry(envelope: EventEnvelope) {
  const event = envelope.event;
  if (event.scope !== "registry") return;
  if (event.event.type === "workspace_upserted") {
    const workspace = event.event.workspace;
    const all = get().workspaces;
    const before = all[workspace.id];
    set((s) => ({ workspaces: { ...s.workspaces, [workspace.id]: workspace } }));
    leaveWhenArchiving(all, workspace);
    // Archiving runs in the background: say how it ended.
    if (before?.state === "archiving" && workspace.state === "archived") toast(`${workspaceName(workspace)} archived`);
    if (before?.state === "archiving" && workspace.state === "cleanup_failed") {
      toast(`Could not archive ${workspaceName(workspace)}: ${workspace.last_error ?? "a cleanup step failed"}`, "error");
    }
    if (workspace.state !== "creating") forgetCreation(workspace.id);
    // News in the workspace the user is looking at is read at once.
    if (workspace.unread && viewing(workspace.id)) markSeen(workspace.id);
  } else if (event.event.type === "chat_upserted") {
    onChatUpserted(event.event.chat);
  } else if (event.event.type === "extensions_changed") {
    applyExtensions(event.event.extensions);
  } else if (event.event.type === "loadout_changed") {
    set({ loadout: event.event.entries });
  } else if (event.event.type === "sections_changed") {
    set({ sections: event.event.sections });
  } else if (event.event.type === "routine_upserted") {
    upsertRoutine(event.event.routine);
  } else if (event.event.type === "routine_suggestions_changed") {
    const before = get().routineFind;
    const find = event.event.find;
    set({ routineFind: find });
    // The analysis just ended: say so when its dialog is not there to show it.
    if (before?.running && !find.running && !get().findRoutinesShown) {
      if (find.last_error) toast(`Find routines failed: ${find.last_error}`, "error");
      else toast(find.last_found ? `Find routines proposes ${find.last_found} routine${find.last_found === 1 ? "" : "s"}` : "Find routines found nothing worth a routine");
    }
  } else if (event.event.type === "routine_removed") {
    const id = event.event.routine_id;
    set((s) => {
      const routines = { ...s.routines };
      delete routines[id];
      return { routines };
    });
  } else if (event.event.type === "identity_changed") {
    setIdentity(event.event.identity);
    void loadMentions();
  } else if (event.event.type === "mentions_changed") {
    mentionsChanged(event.event.mentions, event.event.new ?? []);
  } else if (event.event.type === "account_changed") {
    // The reading says whether it is news (a replay after a reconnection is not).
    void loadUsage().catch(() => {});
  } else if (event.event.type === "login_ended") {
    // A worker's login is followed by state/workerLogins; this machine's by `account_changed`.
  } else if (event.event.type === "chat_removed") {
    removeChat(event.event.chat_id);
  } else if (event.event.type === "idle_reminder") {
    remindIdle(event.event.workspace_ids);
  } else if (event.event.type === "transfer_ended") {
    announceTransferEnded(event.event.transfer);
  } else {
    // A creation that failed: nothing of it is kept.
    const { workspace_id: id, slug, error } = event.event;
    set((s) => {
      const workspaces = { ...s.workspaces };
      delete workspaces[id];
      return { workspaces, selectedId: s.selectedId === id ? null : s.selectedId };
    });
    // The dialog it came from reopens, filled in, with the error; else a toast says it.
    const draft = forgetCreation(id);
    if (!error) return;
    if (draft && !get().createOpen) {
      set({ createRetry: { draft, error }, createOpen: true, createExplore: draft.explore, createParent: draft.parent, switcherOpen: false });
    } else {
      toast(`Could not create ${slug}: ${error}`, "error");
    }
  }
}

/** Moved workspaces left an hour without activity: one notification for all of them, while the
 * sidebar and each one's banner show it until something happens there. */
function remindIdle(ids: string[]) {
  const idle = stillIdle(ids, get().workspaces);
  if (idle.length === 0) return;
  const text = transferCopy(navigator.language.startsWith("fr")).idleNotification(idle.map(workspaceName));
  if (windowFocused()) toast(text);
  else void notify("Chauffeur", text);
}

/** A transfer ended: a notification while the app is not in front, else a toast unless the user
 * looks at the workspace, whose header says it already. */
function announceTransferEnded(transfer: TransferInfo) {
  const workspace = get().workspaces[transfer.workspace_id];
  const name = workspace ? workspaceName(workspace) : transfer.slug;
  const ended = endedNotification(transfer, name, transferCopy(navigator.language.startsWith("fr")));
  if (!ended) return;
  if (!windowFocused()) {
    void notify("Chauffeur", ended.text);
    playNotificationSound().catch(() => {});
  } else if (!viewing(transfer.workspace_id)) {
    toast(ended.text, ended.failed ? "error" : "info");
  }
}

let started = false;

/** The user is back: the chats whose turn failed on a network error resume it, once. */
function resumeFailedTurns() {
  if (get().connection.status !== "open") return;
  // A failure changes nothing for the user: the chat still waits for a message.
  client.call("chat.resume_failed", {}).catch(() => {});
}

/** Connects to the daemon and follows the registry. Idempotent. */
export function startApp() {
  if (started) return;
  started = true;
  if (get().zoom !== 1) void setAppZoom(get().zoom);
  unlockAudio();
  window.addEventListener("focus", () => {
    if (get().connection.status === "open") {
      void loadAppearance();
      void loadExtensions().catch((error) => toast(`Could not load extensions: ${errorMessage(error)}`, "error"));
    }
    resumeFailedTurns();
  });
  // A theme file the user's tools rewrite (a new wallpaper): read again every few seconds while
  // the app is in front.
  setInterval(() => {
    if (get().appearance?.terminal_theme && windowFocused() && get().connection.status === "open") void loadAppearance();
  }, 5000);
  client.onStateChange((connection) => {
    set({ connection });
    if (connection.status === "open") {
      void refreshWorkspaces();
      void loadRoutines();
      void loadRoutineFind();
      void loadLoadout();
      void loadAppearance();
      void loadIdentity();
      void loadMentions();
      void loadExtensions().catch((error) => toast(`Could not load extensions: ${errorMessage(error)}`, "error"));
      resumeFailedTurns();
      if (get().repos == null) void loadRepos();
      if (get().models == null) void loadModels();
    }
  });
  client.subscribe(
    { all: false, registry: true, workspaces: [], chats: [], ephemeral: false },
    onRegistry,
  );
  client.start();
  // The tray is told only when the count changes, and once after a reload of the page.
  let needing: number | null = null;
  useApp.subscribe((s) => {
    const count = needingYou(s.workspaces);
    if (count === needing) return;
    needing = count;
    void setTrayAttention(count);
  });
  window.addEventListener("focus", () => {
    const id = get().selectedId;
    if (id) markSeen(id);
  });
}

// --- derived ---

/** The chat tab a workspace shows (whether or not its Chat view is the one on screen). */
export function shownChatOf(s: AppState, workspaceId: string): ChatInfo | null {
  return shownChat(openChats(chatsOf(s.chats, workspaceId)), s.selectedChat[workspaceId]);
}

/** The design chat a workspace's main pane shows (its Chat view, on a design chat's tab), if any. */
export function designChatShown(s: AppState, workspaceId: string): string | null {
  if (mainView(s, workspaceId) !== "chat") return null;
  const chat = shownChatOf(s, workspaceId);
  return chat?.kind === "design" ? chat.id : null;
}

/** Workspaces in sidebar order: parents newest first, each followed by its children. */
export function chatsOf(chats: Record<string, ChatInfo>, workspaceId: string): ChatInfo[] {
  return Object.values(chats)
    .filter((c) => c.workspace_id === workspaceId)
    .sort((a, b) => a.created_at.localeCompare(b.created_at));
}
