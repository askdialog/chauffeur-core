import type {
  LoadoutEntry,
  AgentKind,
  ChatInfo,
  ConfigureChatParams,
  DesignBrief,
  RequestResponse,
  SendMode,
  WorkspaceInfo,
} from "@chauffeur/protocol";
import {
  ArrowDown,
  ArrowUp,
  ChevronDown,
  CornerDownRight,
  Download,
  GitFork,
  History,
  ListPlus,
  MessageSquare,
  MessageSquarePlus,
  Pencil,
  PenTool,
  Plus,
  Repeat,
  Square,
  Trash2,
  X,
} from "lucide-react";
import { type CSSProperties, memo, type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Virtuoso, type ListRange, type VirtuosoHandle } from "react-virtuoso";
import { useShallow } from "zustand/react/shallow";
import { remountLocation, firstItemKey, type ListLocation } from "@/lib/chatList";
import { type ChatRow, type ChatView, emptyChatView, groupActivity, openRequests, type QuestionItem } from "@/lib/chat";
import { formatCost, relativeTime } from "@/lib/format";
import {
  agentBusy,
  chatLabel,
  chatTabStatus,
  chatToReopen,
  closedChats,
  forkRefusal,
  neighbourTab,
  openChats,
  placedChats,
  shownChat,
  tabActions,
} from "@/lib/chatTabs";
import { onMenuAction } from "@/lib/native";
import { tabPercent } from "@/lib/context";
import { Deliverables } from "@/components/Deliverables";
import { errorMessage } from "@/lib/rpc";
import {
  chatsOf,
  clearSearchMatch,
  client,
  closeChat,
  closedHere,
  type CommandsSource,
  deleteChat,
  forkChat,
  loadChats,
  makeRoutine,
  renameChat,
  reopenChat,
  selectChat,
  sendToChat,
  signIn,
  toast,
  upsertChats,
  useApp,
} from "@/state/app";
import { CatalogContext, useCatalog, useRemoteCatalog, useWorkspaceCatalog } from "@/state/catalog";
import { useWorkspaceSelector, type WorkspaceSession } from "@/state/session";
import { AGENT_NAMES, designChoice, effectiveLoadout, initialChoice, loadoutIn, type ModelChoice } from "@/lib/models";
import { ExploreModePicker } from "../ExploreModePicker";
import { ModelPicker } from "../ModelPicker";
import { PermissionPicker } from "../PermissionPicker";
import { Badge, Button, cx, DesignOnlyMode, EmptyState, IconButton, inputClass, Kbd, Modal, Spinner } from "../ui";
import { ContextMenu, type MenuItem } from "../ContextMenu";
import { FileLinkContext } from "../FileLinks";
import { useWorkspaceFileLinker } from "../WorkspaceFileLinker";
import { ChatHostContext } from "./chatHost";
import { useAppChatHost } from "./useAppChatHost";
import { SuggestionCards } from "./SuggestionCards";
import { forgetEditing, useWorkspaceShares } from "@/state/workspaceShares";
import { CommentSelectionButton, type Passage, PassageCommentBox, selectedPassage, ShareCommentsContext } from "./ShareComments";
import { useOwnerComments } from "./useOwnerComments";
import { pendingSuggestions } from "@/lib/workspaceShare";
import { ImageSourceContext } from "../Markdown";
import {
  ActivityContext,
  ChatIdContext,
  ForkTurnContext,
  ChatRowView,
  HighlightContext,
  PermissionChecksContext,
  SendNowContext,
  SubagentOpenContext,
  WorkspaceIdContext,
  WorkspacePathContext,
} from "./ChatItems";
import { SubagentPanel, useSubagentPanel } from "./SubagentPanel";
import { DesignChatLayout } from "../design/DesignChatLayout";
import { useTabDrag } from "../useTabDrag";
import { type PendingImage, toImageInputs } from "@/lib/images";
import { drafts } from "@/lib/drafts";
import { inShellMode, messageText, shellCommand } from "@/lib/shell";
import { type BriefBanner, briefBanner, briefFill, briefMemos } from "@/lib/designBrief";
import { AttachButton, DropOverlay, ImageStrip, useImageAttachments } from "./ImageAttachments";
import { ComposerEditor, type ComposerEditorHandle } from "./ComposerEditor";
import { ContextMeter } from "./ContextMeter";
import { McpServers } from "./McpServers";
import { ChildRequestsPanel, respondTo, useChildRequests } from "./ChildRequests";
import { QuestionPrompt } from "./QuestionPrompt";
import { QueuePanel } from "./QueuePanel";
import { TasksPanel } from "./TasksPanel";
import { useSlashMenu } from "./useSlashMenu";
import { TimelineRail } from "./TimelineRail";
import { ChatFind, textRanges } from "./ChatFind";
import { hasMod, kbd, stepTab } from "../../lib/keys";
import { chatJump, onceIn, tabKey } from "../../lib/shortcuts";
import { dismissedSuggestions, shownSuggestion, type Suggestion, suggestionKey } from "@/lib/suggestions";

type ChatPanelProps = {
  workspace: WorkspaceInfo;
  session: WorkspaceSession;
  /** Shown above the composer: the workspace's creation going on, or its warnings. */
  setup?: ReactNode;
  /** What ⌘W does instead of closing the chat shown: beside the Office, it closes the panel. */
  onCloseShortcut?: () => void;
  /** Whether its pane is the one in use: the next and previous tab shortcuts are then its own. */
  inUse?: boolean;
  /** Whether the Terminal view is the one in use: its own keys then open and close terminals. */
  terminalInUse?: boolean;
};

/** A workspace's chats; moved to another machine, they pick among that machine's models. */
export function ChatPanel(props: ChatPanelProps) {
  const catalog = useWorkspaceCatalog(props.workspace);
  return (
    <CatalogContext.Provider value={catalog}>
      <WorkspaceChats {...props} />
    </CatalogContext.Provider>
  );
}

function WorkspaceChats({ workspace, session, setup = null, onCloseShortcut, inUse = true, terminalInUse = false }: ChatPanelProps) {
  // The same list while chats of other workspaces change.
  const all = useApp(useShallow((s) => chatsOf(s.chats, workspace.id)));
  const open = useMemo(() => workspace.state === "archived" ? all : openChats(all), [all, workspace.state]);
  const closed = useMemo(() => workspace.state === "archived" ? [] : closedChats(all), [all, workspace.state]);
  const selected = useApp((s) => s.selectedChat[workspace.id]);
  const chatViews = useWorkspaceSelector(session, (s) => s.chats);
  const replayed = useWorkspaceSelector(session, (s) => s.live);
  const partial = useWorkspaceSelector(session, (s) => s.partial);
  const [creating, setCreating] = useState(false);
  const [menu, setMenu] = useState<{ chat: ChatInfo; x: number; y: number } | null>(null);
  const [closedMenu, setClosedMenu] = useState<{ x: number; y: number } | null>(null);
  const [newMenu, setNewMenu] = useState<{ x: number; y: number } | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const strip = useRef<HTMLDivElement>(null);
  const tabDrag = useTabDrag({
    strip,
    ids: open.map((c) => c.id),
    onMove: async (id, before) => {
      // Shown at once; the daemon's answer and events confirm it, a refusal puts it back.
      upsertChats(placedChats(all, id, before));
      try {
        await client.call("chat.move", { chat: id, before });
      } catch (error) {
        upsertChats(all);
        toast(`Could not move the tab: ${errorMessage(error)}`, "error");
      }
    },
  });

  useEffect(() => loadChats(workspace.id), [workspace.id]);

  const chat = shownChat(open, selected);
  const shownId = chat?.id;
  // The tab shown stays in view when the strip scrolls: a chat opened or chosen elsewhere may sit past its edge.
  useEffect(() => {
    if (shownId) strip.current?.querySelector(`[data-chat="${shownId}"]`)?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [shownId, open.length]);
  // A workspace being created takes messages already: they wait for its agent.
  // Moved to another machine, its chats run there, piloted through this daemon while it answers.
  const relay = useWorkspaceSelector(session, (s) => s.relay);
  const reachable = relay?.connected !== false;
  const readOnly = (workspace.state !== "ready" && workspace.state !== "creating") || (!!workspace.moved_to && !reachable);
  const actions = tabActions(workspace.state, workspace.moved_to ?? null, reachable);

  // New chats start on the first loadout entry, or on what the empty composer picked.
  const catalog = useCatalog();
  // On another machine, only the favorites its agents have.
  const remote = useRemoteCatalog();
  const loadoutHere = (saved: LoadoutEntry[] | null) => (remote ? loadoutIn(saved, catalog) : effectiveLoadout(saved, catalog));
  const startChoice = () => initialChoice(loadoutHere(useApp.getState().loadout));
  const newChat = async (prompt?: string, choice: ModelChoice = startChoice(), images: PendingImage[] = []) => {
    setCreating(true);
    try {
      const created = await client.call("chat.create", {
        workspace: workspace.id,
        prompt,
        images: toImageInputs(images),
        agent: choice.agent,
        model: choice.model ?? undefined,
        effort: choice.effort ?? undefined,
        fast: choice.fast || undefined,
      });
      upsertChats([created]);
      selectChat(workspace.id, created.id);
      return created;
    } catch (error) {
      toast(`Could not start a chat: ${errorMessage(error)}`, "error");
      return null;
    } finally {
      setCreating(false);
    }
  };

  /** A design chat (story 47): blank, or from `source` with a design brief the daemon writes. */
  const [importing, setImporting] = useState(false);
  const newDesignChat = async (source?: ChatInfo) => {
    setCreating(true);
    try {
      // The user's first Claude favorite, with its effort and speed: a design chat starts on Claude
      // Code, and its composer's model picker moves it to Codex.
      const { loadout, models } = useApp.getState();
      const choice = designChoice(effectiveLoadout(loadout, models));
      const created = await client.call("chat.create", {
        workspace: workspace.id,
        kind: "design",
        source_chat: source?.id,
        model: choice.model ?? undefined,
        effort: choice.effort ?? undefined,
        fast: choice.fast || undefined,
      });
      upsertChats([created]);
      selectChat(workspace.id, created.id);
    } catch (error) {
      toast(`Could not start a design chat: ${errorMessage(error)}`, "error");
    } finally {
      setCreating(false);
    }
  };

  const busy = (c: ChatInfo) => agentBusy(c, chatViews[c.id]?.queue.length ?? 0);
  /** The tab shown gives way to its neighbour; the last one, to the new chat the daemon opens. */
  const leave = (c: ChatInfo) => {
    const next = neighbourTab(open, c.id);
    if (c.id === chat?.id && next) selectChat(workspace.id, next);
  };
  const close = async (c: ChatInfo, force: boolean) => {
    const next = neighbourTab(open, c.id);
    if ((await closeChat(c.id, force)) && c.id === chat?.id && next) selectChat(workspace.id, next);
  };
  const requestClose = (c: ChatInfo) => {
    if (!actions.close) return;
    if (busy(c)) setConfirm({ kind: "close", chat: c, busy: true });
    else void close(c, false);
  };
  const requestDelete = (c: ChatInfo) => {
    if (actions.delete) setConfirm({ kind: "delete", chat: c, busy: busy(c) });
  };
  const reopenLast = () => {
    const target = chatToReopen(closed, closedHere);
    if (target && actions.close) void reopenChat(target.id);
  };

  // ⌘T opens a chat, ⌘W closes the chat shown, ⇧⌘T reopens the last one closed: from the keyboard,
  // or from the app menu (macOS), whichever comes first. Ctrl+Tab and the like show another tab.
  const shortcuts = useRef({ new: () => {}, close: () => {}, reopen: () => {}, step: (_step: 1 | -1) => {}, inUse, terminalInUse });
  shortcuts.current = {
    new: () => {
      if (!readOnly && !creating) void newChat();
    },
    close: onCloseShortcut ?? (() => chat && requestClose(chat)),
    reopen: reopenLast,
    step: (step) => {
      const target = stepTab(open.map((c) => c.id), chat?.id, step);
      if (target) selectChat(workspace.id, target);
    },
    inUse,
    terminalInUse,
  };
  useEffect(() => {
    const run = (action: "new-chat" | "close-chat" | "reopen-chat") => {
      // A dialog in front (settings, a confirmation) keeps the keys for itself.
      if (document.querySelector("[role=dialog]")) return;
      const { current } = shortcuts;
      onceIn(action, action === "new-chat" ? current.new : action === "close-chat" ? current.close : current.reopen);
    };
    const onKey = (event: KeyboardEvent) => {
      const key = tabKey(event);
      if (key === "next" || key === "previous") {
        if (!shortcuts.current.inUse || document.querySelector("[role=dialog]")) return;
        event.preventDefault();
        shortcuts.current.step(key === "next" ? 1 : -1);
      } else if (key && !shortcuts.current.terminalInUse) {
        event.preventDefault();
        // A held key acts once, not once per repeat.
        if (!event.repeat) run(key === "new" ? "new-chat" : key === "close" ? "close-chat" : "reopen-chat");
      }
    };
    window.addEventListener("keydown", onKey);
    const unlisten = onMenuAction((action) => {
      if (action === "new-chat" || action === "close-chat" || action === "reopen-chat") run(action);
    });
    return () => {
      window.removeEventListener("keydown", onKey);
      unlisten();
    };
  }, []);

  const tabMenu = (c: ChatInfo): MenuItem[] => {
    const why = actions.reason ?? undefined;
    const other: AgentKind = c.agent === "claude" ? "codex" : "claude";
    return [
      {
        label: "Rename",
        icon: <Pencil size={13} />,
        shortcut: "r",
        disabled: !actions.rename,
        hint: actions.rename ? "Or double-click the tab" : why,
        onSelect: () => setRenaming(c.id),
      },
      {
        label: "Close",
        icon: <X size={13} />,
        shortcut: "c",
        disabled: !actions.close,
        hint: actions.close ? `Keeps its conversation: reopen it from the closed chats (${kbd("⇧⌘T")}). ${kbd("⌘W")}` : why,
        onSelect: () => requestClose(c),
      },
      ...(c.kind === "design"
        ? []
        : [
            {
              label: "Fork into a new tab",
              icon: <GitFork size={13} />,
              shortcut: "f",
              disabled: !actions.fork || forkRefusal(c) !== null,
              hint: !actions.fork ? why : (forkRefusal(c) ?? "A new chat with this conversation so far: from there, each goes its own way"),
              onSelect: () => void forkChat(c.id),
            },
            {
              label: `Fork into ${AGENT_NAMES[other]}`,
              icon: <GitFork size={13} />,
              shortcut: "o",
              disabled: !actions.fork || forkRefusal(c) !== null,
              hint: !actions.fork
                ? why
                : (forkRefusal(c) ??
                  `A new ${AGENT_NAMES[other]} chat that reads this conversation as text with its first message`),
              onSelect: () => void forkChat(c.id, undefined, other),
            },
            {
              label: "Design from this chat",
              icon: <PenTool size={13} />,
              disabled: !actions.design,
              hint: actions.design ? "A design chat whose brief summarizes this one" : why,
              onSelect: () => void newDesignChat(c),
            },
            {
              label: "Make a routine",
              icon: <Repeat size={13} />,
              disabled: !actions.design,
              hint: actions.design
                ? "A prompt Chauffeur runs by itself at set times, in a workspace of its own; refine it before activating it"
                : why,
              onSelect: () => void makeRoutine(c),
            },
          ]),
      "separator",
      {
        label: "Delete…",
        icon: <Trash2 size={13} />,
        shortcut: "d",
        danger: true,
        disabled: !actions.delete,
        hint: actions.delete ? "Erases its conversation for good" : why,
        onSelect: () => requestDelete(c),
      },
    ];
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-0.5 border-b border-line px-3">
        <div
          ref={strip}
          className="flex min-w-0 select-none items-center gap-0.5 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
          data-testid="chat-tabs"
          onWheel={(event) => {
            // A mouse wheel only turns vertically: on the strip it scrolls sideways.
            if (event.deltaX === 0) event.currentTarget.scrollLeft += event.deltaY;
          }}
        >
          {open.map((c) => (
            <ChatTab
              key={c.id}
              chat={c}
              label={chatLabel(c, all)}
              parentLabel={subagentParentLabel(c, all)}
              active={c.id === chat?.id}
              view={chatViews[c.id]}
              renaming={renaming === c.id}
              dragStyle={tabDrag.tabStyle(c.id)}
              onPointerDown={actions.rename ? (event) => tabDrag.onPointerDown(c.id, event) : undefined}
              onClick={() => {
                if (!tabDrag.clickIsDrop()) selectChat(workspace.id, c.id);
              }}
              onRename={() => {
                if (actions.rename) setRenaming(c.id);
              }}
              onRenamed={() => setRenaming(null)}
              onMenu={(x, y) => setMenu({ chat: c, x, y })}
              onMiddleClick={() => requestClose(c)}
            />
          ))}
        </div>
        <div className="flex shrink-0 items-center gap-0.5">
          {!readOnly && (
            <>
              <IconButton title="New chat" onClick={() => void newChat()} disabled={creating}>
                {creating ? <Spinner size={12} /> : <Plus size={13} />}
              </IconButton>
              <IconButton
                title="New chat or design chat"
                className="-ml-0.5 w-4"
                data-testid="new-chat-menu"
                disabled={creating}
                onClick={(event) => {
                  const rect = event.currentTarget.getBoundingClientRect();
                  setNewMenu({ x: rect.left, y: rect.bottom + 4 });
                }}
              >
                <ChevronDown size={12} />
              </IconButton>
            </>
          )}
          {closed.length > 0 && actions.close && (
            <IconButton
              title={`Closed chats (${closed.length}): reopen one (${kbd("⇧⌘T")} for the last)`}
              data-testid="closed-chats"
              onClick={(event) => {
                const rect = event.currentTarget.getBoundingClientRect();
                setClosedMenu({ x: rect.left, y: rect.bottom + 4 });
              }}
            >
              <History size={13} />
            </IconButton>
          )}
        </div>
      </div>
      {menu && <ContextMenu x={menu.x} y={menu.y} items={tabMenu(menu.chat)} onClose={() => setMenu(null)} />}
      {newMenu && (
        <ContextMenu
          x={newMenu.x}
          y={newMenu.y}
          items={[
            {
              label: "New chat",
              icon: <MessageSquarePlus size={13} />,
              shortcut: "n",
              hint: "An agent that codes in this workspace",
              onSelect: () => void newChat(),
            },
            {
              label: "New design chat",
              icon: <PenTool size={13} />,
              shortcut: "d",
              disabled: !actions.design,
              hint: actions.design ? "An agent that designs screens; it never touches the code" : (actions.reason ?? ""),
              onSelect: () => void newDesignChat(),
            },
            {
              label: "Import a shared design…",
              icon: <Download size={13} />,
              shortcut: "i",
              disabled: !actions.design,
              hint: actions.design ? "A design a colleague shared with you, into a design chat of your own" : (actions.reason ?? ""),
              onSelect: () => setImporting(true),
            },
          ]}
          onClose={() => setNewMenu(null)}
        />
      )}
      {importing && (
        <ImportDesignDialog
          onClose={() => setImporting(false)}
          onImport={async (url) => {
            const created = await client.call("design.import", { workspace: workspace.id, url });
            upsertChats([created]);
            selectChat(workspace.id, created.id);
            await sendToChat(workspace.id, created, importBrief(url));
            setImporting(false);
          }}
        />
      )}
      {closedMenu && (
        <ContextMenu
          x={closedMenu.x}
          y={closedMenu.y}
          items={closed.map((c) => ({
            label: `${chatLabel(c, all)} · ${relativeTime(c.last_activity_at ?? c.created_at)}`,
            icon: c.kind === "design" ? <PenTool size={13} /> : <MessageSquare size={13} />,
            hint: `Reopen it; last active ${relativeTime(c.last_activity_at ?? c.created_at)}`,
            onSelect: () => void reopenChat(c.id),
          }))}
          onClose={() => setClosedMenu(null)}
        />
      )}
      {confirm && (
        <ConfirmChatDialog
          confirm={confirm}
          label={chatLabel(confirm.chat, all)}
          onCancel={() => setConfirm(null)}
          onConfirm={async () => {
            const { kind, chat: target, busy: force } = confirm;
            setConfirm(null);
            if (kind === "close") await close(target, true);
            else {
              leave(target);
              await deleteChat(target.id, force);
            }
          }}
        />
      )}
      {chat ? (
        <DesignOrPlain key={chat.id} chat={chat} view={chatViews[chat.id] ?? emptyChatView}>
          <ChatConversation
            key={chat.id}
            chat={chat}
            view={chatViews[chat.id] ?? emptyChatView}
            readOnly={readOnly || chat.status === "closed"}
            archived={workspace.state === "archived"}
            explore={workspace.explore}
            setup={setup}
            settingUp={workspace.state === "creating"}
            loading={!replayed}
            partial={partial}
          />
        </DesignOrPlain>
      ) : (
        <NoChat
          workspaceId={workspace.id}
          readOnly={readOnly}
          explore={workspace.explore}
          onSend={async (text, choice, images) => {
            if (!(await newChat(text, choice, images))) throw new Error("not sent");
          }}
          busy={creating}
          setup={setup}
        />
      )}
    </div>
  );
}

/** A design chat's conversation sits beside its canvas (story 47); any other chat takes the tab. */
function DesignOrPlain({ chat, view, children }: { chat: ChatInfo; view: ChatView; children: ReactNode }) {
  if (chat.kind !== "design") return <>{children}</>;
  return (
    <DesignChatLayout chat={chat} view={view}>
      {children}
    </DesignChatLayout>
  );
}

type Confirm = { kind: "close" | "delete"; chat: ChatInfo; busy: boolean };

/** Asks before stopping a working agent to close its chat, and before any delete. */
function ConfirmChatDialog({
  confirm,
  label,
  onCancel,
  onConfirm,
}: {
  confirm: Confirm;
  label: string;
  onCancel: () => void;
  onConfirm: () => Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const deleting = confirm.kind === "delete";
  const action = deleting ? (confirm.busy ? "Stop and delete" : "Delete") : "Stop and close";
  return (
    <Modal
      open
      onClose={onCancel}
      width={440}
      title={deleting ? `Delete '${label}'?` : `Close '${label}'?`}
      footer={
        <>
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button
            variant="danger"
            autoFocus
            busy={busy}
            data-testid="confirm-chat-action"
            onClick={() => {
              setBusy(true);
              void onConfirm();
            }}
          >
            {action}
          </Button>
        </>
      }
    >
      <div className="space-y-2 text-[12.5px] text-muted">
        {deleting ? (
          <>
            {confirm.busy && <p className="text-attention">The agent is working: it is stopped first.</p>}
            <p>Its conversation is erased; this cannot be undone.</p>
            <p className="text-faint">Files it saved in the notes folder stay there.</p>
          </>
        ) : (
          <>
            <p className="text-attention">The agent is working. Stop it and close?</p>
            <p className="text-faint">Its conversation is kept: reopen it from the closed chats ({kbd("⇧⌘T")}).</p>
          </>
        )}
      </div>
    </Modal>
  );
}

/** A delegated chat's parent, as its tab names it; undefined for any other chat. */
function subagentParentLabel(chat: ChatInfo, all: ChatInfo[]): string | undefined {
  if (!chat.parent_chat) return undefined;
  const parent = all.find((c) => c.id === chat.parent_chat);
  return parent ? chatLabel(parent, all) : "a chat";
}

function ChatTab({
  chat,
  label,
  parentLabel,
  active,
  view,
  renaming,
  dragStyle,
  onPointerDown,
  onClick,
  onRename,
  onRenamed,
  onMenu,
  onMiddleClick,
}: {
  chat: ChatInfo;
  label: string;
  /** A delegated chat's parent (story 49). */
  parentLabel?: string | undefined;
  active: boolean;
  view: ChatView | undefined;
  renaming: boolean;
  /** Where a drag of the strip's tabs draws it (`useTabDrag`). */
  dragStyle: CSSProperties | undefined;
  /** Starts a drag that reorders the tabs; absent where they cannot move (an archived workspace). */
  onPointerDown: ((event: React.PointerEvent) => void) | undefined;
  onClick: () => void;
  onRename: () => void;
  onRenamed: () => void;
  onMenu: (x: number, y: number) => void;
  onMiddleClick: () => void;
}) {
  // Past half the window, the tab says how full the context is.
  const context = tabPercent(view?.context?.usage);
  const suggested = pendingSuggestions(useWorkspaceShares((s) => s.suggestions[chat.id])).length;
  const className = cx(
    "flex h-6 max-w-[220px] shrink-0 items-center gap-1.5 rounded-md px-2 text-[12px]",
    active ? "bg-hover text-fg" : "text-muted hover:bg-hover/60 hover:text-fg",
  );
  const body = (
    <>
      <ChatStatusIcon chat={chat} view={view} />
      {chat.kind === "design" && (
        <PenTool size={11} className="shrink-0 text-faint" aria-label="Design chat" data-testid="design-chat-icon" />
      )}
      {parentLabel !== undefined && (
        <CornerDownRight size={11} className="shrink-0 text-faint" aria-label="Delegated chat" data-testid="subagent-chat-icon" />
      )}
      {renaming ? <TabRenameField chat={chat} label={label} onDone={onRenamed} /> : <span className="truncate">{label}</span>}
      <span className="text-[10.5px] text-faint">{chat.agent === "codex" ? "codex" : ""}</span>
      {suggested > 0 && (
        <Badge tone="attention" title={`${suggested} ${suggested === 1 ? "suggestion waits" : "suggestions wait"} for you`}>
          {suggested}
        </Badge>
      )}
      {context && (
        <span className="shrink-0 text-[10.5px] tabular-nums text-faint" title="Share of the context window in use" data-testid="chat-tab-context">
          {context}
        </span>
      )}
    </>
  );
  // A div while renaming: in a button, the field's spaces would press it.
  if (renaming) {
    return (
      <div className={className} data-chat={chat.id} data-tab={chat.id}>
        {body}
      </div>
    );
  }
  return (
    <button
      onClick={onClick}
      onPointerDown={onPointerDown}
      onDoubleClick={onRename}
      // The middle button closes a tab, as in a browser; on Linux it would paste or start autoscroll.
      onMouseDown={(event) => event.button === 1 && event.preventDefault()}
      onAuxClick={(event) => {
        if (event.button !== 1) return;
        event.preventDefault();
        onMiddleClick();
      }}
      onContextMenu={(event) => {
        event.preventDefault();
        onMenu(event.clientX, event.clientY);
      }}
      title={parentLabel !== undefined ? `${label} · delegated by “${parentLabel}”` : label}
      data-chat={chat.id}
      data-tab={chat.id}
      style={dragStyle}
      className={className}
    >
      {body}
    </button>
  );
}

/** A tab's title, edited in place: Enter saves (empty: the automatic title), Escape or leaving cancels. */
function TabRenameField({ chat, label, onDone }: { chat: ChatInfo; label: string; onDone: () => void }) {
  const [value, setValue] = useState(label);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);
  const save = () => {
    onDone();
    if (value.trim() !== label) void renameChat(chat.id, value);
  };
  return (
    <input
      ref={input}
      value={value}
      onChange={(event) => setValue(event.target.value)}
      onKeyDown={(event) => {
        event.stopPropagation();
        if (event.key === "Enter") save();
        if (event.key === "Escape") onDone();
      }}
      onBlur={onDone}
      placeholder="Automatic title"
      className="w-[160px] min-w-0 rounded border border-line-strong bg-bg px-1 text-[12px] text-fg outline-none"
      data-testid="chat-rename-field"
    />
  );
}

function ChatStatusIcon({ chat }: { chat: ChatInfo; view: ChatView | undefined }) {
  const status = chatTabStatus(chat);
  if (status === "working") return <Spinner size={10} className="text-working" />;
  if (status === "needs_input") return <span className="h-1.5 w-1.5 rounded-full bg-attention" />;
  if (status === "error") return <span className="h-1.5 w-1.5 rounded-full bg-danger" />;
  return <span className="h-1.5 w-1.5 rounded-full bg-faint/50" />;
}

function NoChat({
  workspaceId,
  readOnly,
  explore,
  onSend,
  busy,
  setup,
}: {
  workspaceId: string;
  readOnly: boolean;
  explore: boolean;
  onSend: (text: string, choice: ModelChoice, images: PendingImage[]) => Promise<void>;
  busy: boolean;
  setup: ReactNode;
}) {
  const models = useCatalog();
  const remote = useRemoteCatalog();
  const loadout = useApp((s) => s.loadout);
  const [picked, setPicked] = useState<ModelChoice | null>(null);
  const choice = picked ?? initialChoice(remote ? loadoutIn(loadout, models) : effectiveLoadout(loadout, models));
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1">
        <EmptyState icon={<MessageSquarePlus size={22} />} title="No chat yet">
          {readOnly
            ? "This workspace is not ready."
            : explore
              ? "Ask about the code: the agent reads each repo at its base branch and explains. It changes nothing."
              : "Send a first message to start an agent in this workspace."}
        </EmptyState>
      </div>
      {setup}
      {!readOnly && (
        <ComposerBox
          key={`new:${workspaceId}`}
          draftKey={`new:${workspaceId}`}
          disabled={busy}
          working={false}
          placeholder={explore ? "How does… work? Where is… handled?" : "Ask the agent to do something…"}
          commandsFor={{ workspaceId, agent: choice.agent }}
          onSend={(text, images) => onSend(text, choice, images)}
          onInterrupt={() => {}}
          controls={
            <>
              <ModelPicker value={choice} onChange={setPicked} shortcuts />
              {explore && <ExploreModePicker workspaceId={workspaceId} />}
            </>
          }
        />
      )}
    </div>
  );
}

export function ChatConversation({
  chat,
  view,
  readOnly,
  archived,
  explore = false,
  setup = null,
  settingUp = false,
  loading = false,
  partial = false,
}: {
  chat: ChatInfo;
  view: ChatView;
  readOnly: boolean;
  archived: boolean;
  /** An exploration's chat: its mode is read-only and cannot change. */
  explore?: boolean;
  /** Shown above the composer: the workspace's creation going on, or its warnings. */
  setup?: ReactNode;
  /** The workspace is being created: the agent has not started; messages wait. */
  settingUp?: boolean;
  /** The daemon is still sending the conversation: the list waits for all of it rather than move under the reader. */
  loading?: boolean;
  /** Only the end of the conversation is here yet: what counts or looks through all of it waits. */
  partial?: boolean;
}) {
  const list = useRef<VirtuosoHandle>(null);
  const [atBottom, setAtBottom] = useState(true);
  const [atTop, setAtTop] = useState(false);
  // The timeline rail: the list's scroll area, its measure, and a jump under way (which the list
  // must not undo by following new output to the bottom).
  const [scroller, setScroller] = useState<HTMLElement | null>(null);
  const scrollerRef = useCallback((el: HTMLElement | Window | null) => setScroller(el instanceof HTMLElement ? el : null), []);
  const railRefresh = useRef<() => void>(() => {});
  const jumping = useRef(false);
  const root = useRef<HTMLDivElement>(null);
  const [highlight, setHighlight] = useState<string | null>(null);
  useEffect(() => {
    if (!highlight) return;
    const timer = setTimeout(() => setHighlight(null), 1400);
    return () => clearTimeout(timer);
  }, [highlight]);
  const toTop = () => list.current?.scrollTo({ top: 0 });
  const toBottom = () => list.current?.scrollToIndex({ index: "LAST", align: "end", behavior: "auto" });
  const working = view.working || chat.status === "working" || chat.status === "starting" || chat.status === "needs_input";
  const signedOut = useApp((s) => s.usage?.find((u) => u.agent === chat.agent)?.account === "signed_out");
  // An idle chat has no session to hear an answer: a request still open in its log was cut by a
  // restart before the daemon closed it (older chats), and answering it would only fail.
  const requests = chat.status === "idle" ? [] : openRequests(view);
  // The oldest open question is answered in place of the composer; permissions stay in the list.
  const question = requests.find((r): r is QuestionItem => r.kind === "question") ?? null;
  // A child workspace's requests show here too; its oldest question takes the composer's place
  // when the chat has none of its own, until Esc sets it aside.
  const childRequests = useChildRequests(chat.workspace_id);
  const [asideQuestions, setAsideQuestions] = useState<ReadonlySet<string>>(() => new Set());
  const childQuestion = readOnly || question
    ? null
    : (childRequests.find((r) => r.item.kind === "question" && !asideQuestions.has(r.item.requestId)) ?? null);
  const permissions = requests.filter((r) => r.kind === "permission" || r.kind === "elicitation");
  // An AskUserQuestion call answered through its question is shown there only.
  const items = useMemo(() => view.items.filter((item) => !(item.kind === "tool" && item.asked)), [view.items]);
  // Tool calls and thinking between two messages fold into one line, opened on demand.
  const rows = useMemo(() => groupActivity(items), [items]);
  // The comments of a shared workspace, under the agent's messages they are on: the guests' and
  // the owner's, who comments on a passage selected and replies while the chat is shared.
  const owner = useOwnerComments(chat, items, readOnly);
  const commentOn = owner.comment;
  const [selected, setSelected] = useState<Passage | null>(null);
  const [commenting, setCommenting] = useState<Passage | null>(null);
  // A thread a mention opened: its message brought into view, then the thread itself.
  useEffect(() => {
    if (!owner.targetBlock) return;
    const index = rows.findIndex((row) => row.key === owner.targetBlock);
    if (index >= 0) list.current?.scrollToIndex({ index, align: "center" });
  }, [owner.targetBlock, rows]);
  const mount = useListMount(rows, partial, atBottom);
  const searchTarget = useApp((s) => s.searchTarget);
  useEffect(() => {
    if (searchTarget?.chat !== chat.id || partial || loading || !scroller) return;
    const index = rows.findIndex((row) => row.key === `s${searchTarget.seq}`);
    if (index < 0) return;
    jumping.current = true;
    let frame = 0;
    let tries = 0;
    let settled = 0;
    const step = () => {
      const row = scroller.querySelector<HTMLElement>(`[data-index="${index}"]`);
      const range = row ? textRanges(row, searchTarget.word)[0] : null;
      const box = range?.getBoundingClientRect() ?? row?.getBoundingClientRect();
      const viewport = scroller.getBoundingClientRect();
      const offset = box ? box.top - viewport.top : null;
      const atEnd = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2;
      if (offset !== null && (range && box ? offset >= 12 && box.bottom <= viewport.bottom - 8 : Math.abs(offset) < 4 || atEnd)) settled++;
      else {
        settled = 0;
        if (range && box) scroller.scrollBy({ top: box.top - viewport.top - 48 });
        else list.current?.scrollToIndex({ index, align: "start" });
      }
      if (settled >= 3 || ++tries >= 60) {
        setHighlight(rows[index].key);
        clearSearchMatch();
        return;
      }
      frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => {
      cancelAnimationFrame(frame);
      jumping.current = false;
    };
  }, [searchTarget, chat.id, partial, loading, scroller, rows]);

  const [openGroups, setOpenGroups] = useState<ReadonlySet<string>>(() => new Set());
  const activity = useMemo(
    () => ({
      open: openGroups,
      toggle: (key: string) =>
        setOpenGroups((current) => {
          const next = new Set(current);
          if (!next.delete(key)) next.add(key);
          return next;
        }),
    }),
    [openGroups],
  );

  const respond = useCallback(
    async (requestId: string, response: RequestResponse) => {
      try {
        await client.call("chat.respond", { chat: chat.id, request_id: requestId, response });
      } catch (error) {
        toast(`Could not answer: ${errorMessage(error)}`, "error");
      }
    },
    [chat.id],
  );

  // Not in an exploration, whose chats only read, nor in a design chat, which never touches code.
  const shellOffered = !explore && chat.kind !== "design";
  const send = useCallback(async (text: string, images: PendingImage[], mode: SendMode = "steer") => {
    const command = shellOffered ? shellCommand(text) : null;
    if (command !== null) {
      if (images.length > 0) {
        toast("A shell command takes no attachments: remove them, or start the message without “!”", "error");
        throw new Error("shell command with attachments");
      }
      // Not awaited: a command may run for two minutes, and the composer stays free meanwhile.
      // Refused, its text comes back to an empty composer.
      client.call("chat.shell", { chat: chat.id, command }).catch((error) => {
        toast(`Could not run “${command}”: ${errorMessage(error)}`, "error");
        if (!drafts.text(chat.id)) drafts.setText(chat.id, text);
      });
      list.current?.scrollToIndex({ index: "LAST", behavior: "auto" });
      return;
    }
    try {
      const message = shellOffered ? messageText(text) : text;
      // After Edit on a guest's suggestion, the next message sends it (story 53).
      const suggestion = useWorkspaceShares.getState().editing[chat.id];
      await client.call("chat.send", { chat: chat.id, text: message, images: toImageInputs(images), mode, suggestion });
      if (suggestion) forgetEditing(chat.id);
      list.current?.scrollToIndex({ index: "LAST", behavior: "auto" });
    } catch (error) {
      toast(`Could not send: ${errorMessage(error)}`, "error");
      throw error;
    }
  }, [shellOffered, chat.id]);

  const interrupt = useCallback(async () => {
    try {
      await client.call("chat.interrupt", { chat: chat.id });
    } catch (error) {
      toast(`Could not interrupt: ${errorMessage(error)}`, "error");
    }
  }, [chat.id]);
  const sendNow = useMemo(() => (readOnly || settingUp ? null : () => void interrupt()), [readOnly, settingUp, interrupt]);

  // Esc interrupts the working agent when no dialog is open.
  useEffect(() => {
    if (!working || readOnly || settingUp) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      if (document.querySelector("[role=dialog]")) return;
      event.preventDefault();
      void interrupt();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [working, readOnly, settingUp, chat.id]);

  // ⌘↑ and ⌘↓ (Ctrl elsewhere) jump to the first and the latest message, as Home and End do
  // outside a text field. In a composer with text they still move the caret.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || document.querySelector("[role=dialog]")) return;
      const jump = chatJump(event);
      if (!jump) return;
      event.preventDefault();
      if (jump === "top") toTop();
      else toBottom();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // A design chat (story 47): Claude Code only, its permission mode fixed, its brief in the composer.
  const design = chat.kind === "design";
  const workspaceChats = useApp(useShallow((s) => (design ? chatsOf(s.chats, chat.workspace_id) : NO_CHATS)));
  const banner = useMemo(() => (design ? briefBanner(chat, workspaceChats) : null), [design, chat, workspaceChats]);
  /** Writes the brief again. Asked on a written brief, the new one replaces the draft; on a failed one, it fills an empty draft. */
  const regenerateBrief = async () => {
    const previous = chat.brief;
    if (previous?.state === "ready") briefMemos.regenerating(chat.id);
    try {
      const updated = await client.call("chat.regenerate_brief", { chat: chat.id });
      upsertChats([updated]);
    } catch (error) {
      if (previous?.state === "ready" && previous.text) briefMemos.seen(chat.id, previous.text);
      toast(`Could not write the brief again: ${errorMessage(error)}`, "error");
    }
  };

  const [providerChoice, setProviderChoice] = useState<ModelChoice | null>(null);
  const [changingProvider, setChangingProvider] = useState(false);
  useEffect(() => setProviderChoice(null), [chat.id, chat.agent]);
  const providerBusy = working || chat.running_tasks > 0 || view.queue.length > 0 || permissions.length > 0;

  const configure = useCallback(async (patch: Omit<ConfigureChatParams, "chat">) => {
    try {
      const updated = await client.call("chat.configure", { chat: chat.id, ...patch });
      upsertChats([updated]);
      return true;
    } catch (error) {
      toast(`Could not change the chat settings: ${errorMessage(error)}`, "error");
      return false;
    }
  }, [chat.id]);

  const changeProvider = async () => {
    if (!providerChoice || changingProvider || providerBusy) return;
    setChangingProvider(true);
    try {
      if (await configure({ ...changes(chat, providerChoice), confirm_agent: chat.agent })) setProviderChoice(null);
    } finally {
      setChangingProvider(false);
    }
  };

  // A subagent's conversation, in a panel over the right of the list (Claude chats).
  const subagents = useSubagentPanel();
  const openSubagent = chat.agent === "claude" ? subagents.open : null;

  const checking = useMemo(() => new Set(view.checking), [view.checking]);
  // A finished turn forks even while the agent runs a later one.
  const forkTurn = useMemo(
    // Turns are counted from the first: not before it is all here.
    () => (readOnly || settingUp || partial || chat.kind === "design" ? null : (turn: number) => void forkChat(chat.id, turn)),
    [readOnly, settingUp, partial, chat.kind, chat.id],
  );

  const firstRequestIndex = permissions.length ? rows.indexOf(permissions[0]) : -1;
  const chatHost = useAppChatHost();
  const workspacePath = useApp((s) => s.workspaces[chat.workspace_id]?.path ?? null);
  const fileLinker = useWorkspaceFileLinker(chat.workspace_id, workspacePath);
  // An image an answer names, served by the daemon; again on reconnection (the port may change).
  const connection = useApp((s) => s.connection.status);
  const imageSource = useMemo(
    () => (src: string) => {
      const base = client.httpUrl(`/images/${encodeURIComponent(chat.workspace_id)}`);
      return base && `${base}&path=${encodeURIComponent(src)}`;
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [chat.workspace_id, connection],
  );

  // The composer is memoised: its controls change with the chat, not with each streamed token.
  const commandsFor = useMemo(() => ({ workspaceId: chat.workspace_id, agent: chat.agent }), [chat.workspace_id, chat.agent]);
  const controls = useMemo(
    () => (
      <>
        <ModelPicker
          value={{ agent: chat.agent, model: chat.model, effort: chat.effort, fast: chat.fast }}
          lockedAgent={providerBusy ? chat.agent : null}
          lockReason={providerBusy ? "Finish or stop the chat's work before changing provider" : undefined}
          disabled={changingProvider || providerChoice !== null}
          onChange={(next) => next.agent !== chat.agent ? setProviderChoice(next) : void configure(changes(chat, next))}
          shortcuts
        />
        {design ? (
          <DesignOnlyMode agent={chat.agent} />
        ) : explore ? (
          <ExploreModePicker workspaceId={chat.workspace_id} />
        ) : (
          <PermissionPicker
            value={chat.permission_mode}
            effective={chat.permission_mode_effective}
            model={chat.model}
            onChange={(permission_mode) => void configure({ permission_mode })}
          />
        )}
        {formatCost(view.costUsd) && !partial && (
          <span className="text-[11px] text-faint" title={view.costEstimated ? "Estimated token cost at API rates, including cache and Fast mode; excludes tool fees and may differ from your ChatGPT subscription charges." : undefined}>
            {formatCost(view.costUsd, view.costEstimated)}
          </span>
        )}
        <McpServers chat={chat} />
        {view.context && (
          <ContextMeter chatId={chat.id} agent={chat.agent} reading={view.context} workspacePath={workspacePath} />
        )}
      </>
    ),
    [chat, providerBusy, changingProvider, providerChoice, design, explore, configure, view.costUsd, view.costEstimated, view.context, partial, workspacePath],
  );

  return (
    <ChatHostContext.Provider value={chatHost}>
    <WorkspaceIdContext.Provider value={chat.workspace_id}>
    <ChatIdContext.Provider value={chat.id}>
    <WorkspacePathContext.Provider value={workspacePath}>
    <FileLinkContext.Provider value={fileLinker}>
    <ImageSourceContext.Provider value={imageSource}>
    <ActivityContext.Provider value={activity}>
    <HighlightContext.Provider value={highlight}>
    <SubagentOpenContext.Provider value={openSubagent}>
    <PermissionChecksContext.Provider value={checking}>
    <ForkTurnContext.Provider value={forkTurn}>
    <SendNowContext.Provider value={sendNow}>
    <ShareCommentsContext.Provider value={owner.context}>
    <Deliverables key={chat.id} workspaceId={chat.workspace_id} revision={view.turns}>
    <div ref={root} className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div
        className="group/list relative min-h-0 flex-1"
        onMouseDown={() => setSelected(null)}
        // Read once the browser has settled the selection.
        onMouseUp={() => {
          if (commentOn) requestAnimationFrame(() => setSelected(selectedPassage(root.current)));
        }}
        onScrollCapture={() => setSelected(null)}
      >
        {loading ? (
          <div className="flex h-full min-h-[160px] items-center justify-center gap-2 text-[12.5px] text-muted" role="status">
            <Spinner size={14} className="text-working" /> Loading the conversation…
          </div>
        ) : items.length === 0 ? (
          <EmptyState title={working ? "Starting the agent…" : "Nothing here yet"}>
            {readOnly ? "This chat is read-only." : "Messages, tool calls and answers will appear here."}
          </EmptyState>
        ) : (
          <Virtuoso
            key={mount.epoch}
            ref={list}
            className="h-full"
            data={rows}
            // Rounded heights accumulate drift in long chats; delayed measurements expose a
            // frame with stale offsets when rows enter the viewport or streamed text wraps.
            itemSize={measureChatRow}
            // Unmeasured rows count as this height. Left to itself, Virtuoso measures the first row
            // and gives its height to every row it has not rendered yet: a long first message, such as a
            // child workspace's brief, then makes the list much taller than it is. Scrolling up from
            // the bottom, it shrinks under the reader, and WebKit puts them back at the bottom.
            defaultItemHeight={ROW_ESTIMATE}
            skipAnimationFrameInResizeObserver
            computeItemKey={(_, row) => row.key}
            initialTopMostItemIndex={mount.initial}
            rangeChanged={mount.rangeChanged}
            followOutput={(bottom) => (bottom && !jumping.current ? "auto" : false)}
            atBottomStateChange={setAtBottom}
            atBottomThreshold={4}
            atTopStateChange={setAtTop}
            increaseViewportBy={{ top: 400, bottom: 400 }}
            scrollerRef={scrollerRef}
            totalListHeightChanged={() => railRefresh.current()}
            itemContent={(index, row) => (
              <ChatRowView row={row} last={index === rows.length - 1} working={working} readOnly={readOnly} respond={respond} />
            )}
            context={{ working, status: chat.status, since: view.turnStartedAt, settingUp, spared: view.tasks.some((t) => t.background) }}
            components={listComponents}
          />
        )}
        {partial && !loading && items.length > 0 && (
          <div
            role="status"
            className="pointer-events-none absolute left-1/2 top-2 z-10 flex -translate-x-1/2 items-center gap-1.5 rounded-full border border-line-strong bg-raised px-2.5 py-1 text-[11.5px] text-muted shadow-lg"
          >
            <Spinner size={11} className="text-working" /> Loading earlier messages…
          </div>
        )}
        {items.length > 0 && (
          <TimelineRail
            rows={rows}
            list={list}
            scroller={scroller}
            header={LIST_HEADER}
            refresh={railRefresh}
            jumping={jumping}
            onArrive={setHighlight}
          />
        )}
        {items.length > 0 && (
          <ChatFind
            area={root}
            rows={rows}
            list={list}
            scroller={scroller}
            jumping={jumping}
            onClose={() => root.current?.querySelector<HTMLElement>('[data-testid="composer"]')?.focus()}
          />
        )}
        {items.length > 0 && (!atTop || !atBottom) && (
          <div className="pointer-events-none absolute bottom-3 left-1/2 flex -translate-x-1/2 items-center gap-1.5">
            {!atTop && (
              <JumpButton
                onClick={toTop}
                title={`The first message (${kbd("⌘↑")})`}
                // At the bottom, where one reads the latest answer, it shows only while the pointer is over the chat.
                className={atBottom ? "opacity-0 group-hover/list:opacity-100" : undefined}
              >
                <ArrowUp size={12} /> Top
              </JumpButton>
            )}
            {!atBottom && (
              <JumpButton onClick={toBottom} title={`The latest message (${kbd("⌘↓")})`}>
                <ArrowDown size={12} /> Latest
              </JumpButton>
            )}
          </div>
        )}
        {subagents.stack.length > 0 && (
          <SubagentPanel
            chat={chat}
            view={view}
            stack={subagents.stack}
            readOnly={readOnly}
            onPush={subagents.push}
            onBack={subagents.back}
            onClose={subagents.close}
          />
        )}
      </div>
      {permissions.length > 0 && !readOnly && (
        <div className="mx-auto flex w-full max-w-[860px] items-center gap-2 px-5 pb-1 text-[12px] text-attention">
          <span className="h-1.5 w-1.5 rounded-full bg-attention" />
          {permissions.length === 1 ? "The agent is waiting for your permission" : `${permissions.length} permission requests are waiting for you`}
          <button
            className="text-accent hover:underline"
            onClick={() => list.current?.scrollToIndex({ index: firstRequestIndex, align: "center", behavior: "smooth" })}
          >
            Show
          </button>
        </div>
      )}
      {!readOnly && (
        <ChildRequestsPanel
          requests={childRequests}
          shownQuestion={childQuestion?.item.requestId ?? null}
          onAnswer={(requestId) =>
            setAsideQuestions((current) => {
              const next = new Set(current);
              next.delete(requestId);
              // The questions before it go aside, so that this one is the oldest shown.
              for (const r of childRequests) {
                if (r.item.requestId === requestId) break;
                if (r.item.kind === "question") next.add(r.item.requestId);
              }
              return next;
            })
          }
        />
      )}
      {signedOut && !readOnly && (
        <div className="mx-auto flex w-full max-w-[860px] items-center gap-2 px-5 pb-1 text-[12px] text-danger" data-testid="chat-signed-out">
          <span className="h-1.5 w-1.5 rounded-full bg-danger" />
          {AGENT_NAMES[chat.agent]} is signed out: its turns fail until you sign in again.
          <button className="text-accent hover:underline" onClick={() => void signIn(chat.agent)}>
            Sign in
          </button>
        </div>
      )}
      {banner && !readOnly && <BriefBar banner={banner} onRegenerate={() => void regenerateBrief()} />}
      {setup}
      <TasksPanel
        chatId={chat.id}
        tasks={view.tasks}
        readOnly={readOnly}
        onOpen={openSubagent ? (task) => openSubagent({ toolUseId: task.toolUseId, agentId: task.taskId }) : undefined}
      />
      {!readOnly && view.queue.length > 0 && <QueuePanel chatId={chat.id} queue={view.queue} working={working} />}
      {selected && commentOn && (
        <CommentSelectionButton
          passage={selected}
          onComment={(passage) => {
            setCommenting(passage);
            setSelected(null);
          }}
        />
      )}
      {commenting && commentOn && (
        <PassageCommentBox
          key={`${commenting.message}:${commenting.quote}`}
          passage={commenting}
          mentionSetup={owner.mentionSetup}
          readers="the guests"
          onComment={(text, mentions) => commentOn(commenting, text, mentions)}
          onClose={() => setCommenting(null)}
        />
      )}
      {!readOnly && <SuggestionCards chat={chat} />}
      {readOnly ? (
        <div className="border-t border-line px-5 py-2.5 text-center text-[12px] text-faint">
          {archived ? "This workspace is archived: its chats are read-only." : "This chat is read-only."}
        </div>
      ) : question ? (
        <QuestionPrompt key={question.requestId} item={question} respond={respond} />
      ) : childQuestion?.item.kind === "question" ? (
        <QuestionPrompt
          key={childQuestion.item.requestId}
          item={childQuestion.item}
          respond={respondTo(childQuestion)}
          onEscape={() => setAsideQuestions((current) => new Set(current).add(childQuestion.item.requestId))}
        />
      ) : (
        <ComposerBox
          key={chat.id}
          draftKey={chat.id}
          working={working && !settingUp}
          placeholder={
            settingUp
              ? "Write already: the message waits for the agent to start…"
              : working
                ? `Steer the agent (↵), or queue for after its turn (${kbd("⌘↵")})…`
                : banner?.state === "writing"
                  ? "The brief fills this box once written…"
                  : design
                    ? "Describe the screens to design…"
                    : "Message the agent…"
          }
          commandsFor={commandsFor}
          shell={shellOffered}
          brief={design ? chat.brief : undefined}
          suggestion={settingUp ? null : view.suggestion}
          onSend={send}
          onInterrupt={interrupt}
          controls={controls}
        />
      )}
    </div>
    </Deliverables>
    {providerChoice && (
      <Modal
        open
        title={`Switch to ${AGENT_NAMES[providerChoice.agent]}?`}
        width={460}
        dismissable={!changingProvider}
        onClose={() => setProviderChoice(null)}
        footer={<>
          <Button disabled={changingProvider} onClick={() => setProviderChoice(null)}>Cancel</Button>
          <Button variant="primary" disabled={changingProvider || providerBusy} onClick={() => void changeProvider()}>
            {changingProvider ? <Spinner size={12} /> : null} Switch to {AGENT_NAMES[providerChoice.agent]}
          </Button>
        </>}
      >
        <div className="space-y-3 text-[12.5px] text-muted">
          <p>Continue this conversation with {AGENT_NAMES[providerChoice.agent]} in the same tab. Your files and draft stay here.</p>
          <p>The conversation is handed over as text with your next message. The new provider cannot resume {AGENT_NAMES[chat.agent]}'s internal session. Previous tool outputs and images are not included; long conversations may be shortened.</p>
          {providerBusy && <p className="text-attention">Finish or stop the chat's work before switching.</p>}
        </div>
      </Modal>
    )}
    </ShareCommentsContext.Provider>
    </SendNowContext.Provider>
    </ForkTurnContext.Provider>
    </PermissionChecksContext.Provider>
    </SubagentOpenContext.Provider>
    </HighlightContext.Provider>
    </ActivityContext.Provider>
    </ImageSourceContext.Provider>
    </FileLinkContext.Provider>
    </WorkspacePathContext.Provider>
    </ChatIdContext.Provider>
    </WorkspaceIdContext.Provider>
    </ChatHostContext.Provider>
  );
}

/** What `chat.configure` needs to move a chat to another choice. */
function changes(chat: ChatInfo, next: ModelChoice): Omit<ConfigureChatParams, "chat"> {
  const patch: Omit<ConfigureChatParams, "chat"> = {};
  if (next.agent !== chat.agent) patch.agent = next.agent;
  if (next.model && (next.model !== chat.model || patch.agent)) patch.model = next.model;
  if (next.effort && (next.effort !== chat.effort || patch.agent)) patch.effort = next.effort;
  if (next.fast !== chat.fast || patch.agent) patch.fast = next.fast;
  return patch;
}

/** `spared`: background tasks run, which an interrupt leaves running. */
type ListContext = { working: boolean; status: ChatInfo["status"]; since: string | null; settingUp: boolean; spared: boolean };

/** A design chat's brief, above its composer: being written, written (Regenerate), or failed (Retry). */
function BriefBar({ banner, onRegenerate }: { banner: NonNullable<BriefBanner>; onRegenerate: () => void }) {
  return (
    <div
      className={cx(
        "mx-auto flex w-full max-w-[860px] items-center gap-2 px-5 pb-1 text-[12px]",
        banner.state === "failed" ? "text-danger" : "text-faint",
      )}
      data-testid="design-brief"
      data-state={banner.state}
    >
      {banner.state === "writing" ? (
        <Spinner size={12} className="text-working" />
      ) : banner.state === "failed" ? (
        <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-danger" />
      ) : (
        <PenTool size={11} className="shrink-0" />
      )}
      <span className="min-w-0 truncate" title={banner.message}>
        {banner.message}
      </span>
      {banner.state !== "writing" && (
        <button
          className="shrink-0 text-accent hover:underline"
          onClick={onRegenerate}
          title={
            banner.state === "ready"
              ? "Write the brief again from the source chat: it replaces the composer's text"
              : "Try writing the brief again"
          }
          data-testid="regenerate-brief"
        >
          {banner.state === "ready" ? "Regenerate" : "Retry"}
        </button>
      )}
    </div>
  );
}

function JumpButton({
  onClick,
  title,
  className,
  children,
}: {
  onClick: () => void;
  title: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      className={cx(
        "pointer-events-auto flex items-center gap-1 rounded-full border border-line-strong bg-raised px-2.5 py-1 text-[11.5px] text-muted shadow-lg transition-opacity hover:text-fg",
        className,
      )}
    >
      {children}
    </button>
  );
}

/** The first item a row holds: an activity group stands for several. */
/**
 * Where the list (re)mounts. While the start of a long conversation is still coming, the list holds
 * its end; when the rest arrives the list mounts again, at the bottom if the reader was there,
 * else on the row it was showing at the top, so nothing moves under them.
 */
function useListMount(rows: ChatRow[], partial: boolean, atBottom: boolean) {
  const top = useRef<string | null>(null);
  const [state, setState] = useState<{ epoch: number; initial: ListLocation; partial: boolean }>({
    epoch: 0,
    initial: { index: "LAST", align: "end" },
    partial,
  });
  if (state.partial !== partial) {
    if (partial) setState({ ...state, partial });
    else {
      const initial = remountLocation(rows, top.current, atBottom);
      setState({ epoch: state.epoch + 1, initial, partial });
    }
  }
  const rangeChanged = useCallback(
    (range: ListRange) => {
      const row = rows[range.startIndex];
      if (row) top.current = firstItemKey(row);
    },
    [rows],
  );
  return { epoch: state.epoch, initial: state.initial, rangeChanged };
}

/** The space above the first row of a chat, in pixels: the list's Header. */
const LIST_HEADER = 12;
const NO_CHATS: ChatInfo[] = [];

/** The height of a row not measured yet, in pixels: about a chat's average, folded activity lines and messages together. */
const ROW_ESTIMATE = 60;

function measureChatRow(element: HTMLElement) {
  return element.getBoundingClientRect().height;
}

const listComponents = {
  Header: () => <div style={{ height: LIST_HEADER }} />,
  Footer: ({ context }: { context?: ListContext }) =>
    context ? (
      <WorkingFooter
        working={context.working}
        status={context.status}
        since={context.since}
        settingUp={context.settingUp}
        spared={context.spared}
      />
    ) : null,
};

function WorkingFooter({
  working,
  status,
  since,
  settingUp,
  spared,
}: {
  working: boolean;
  status: ChatInfo["status"];
  since: string | null;
  settingUp: boolean;
  spared: boolean;
}) {
  const [, tick] = useState(0);
  useEffect(() => {
    if (!working) return;
    const timer = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(timer);
  }, [working]);
  if (!working) return <div className="h-4" />;
  if (settingUp) {
    return (
      <div className="mx-auto flex w-full max-w-[860px] items-center gap-2 px-5 py-3 text-[12px] text-faint">
        <Spinner size={12} className="text-working" /> Waiting for the workspace: the agent starts once it is ready
      </div>
    );
  }
  const started = since ? Date.parse(since) : NaN;
  const seconds = Number.isNaN(started) ? null : Math.max(0, Math.round((Date.now() - started) / 1000));
  return (
    <div className="mx-auto flex w-full max-w-[860px] items-center gap-2 px-5 py-3 text-[12px] text-faint">
      <Spinner size={12} className="text-working" />
      {status === "needs_input" ? "Waiting for you" : status === "starting" ? "Starting the agent" : "Working"}
      {seconds != null && <span>· {seconds}s</span>}
      <span className="ml-1">
        <Kbd>esc</Kbd> to interrupt{spared && " (background tasks keep running)"}
      </span>
    </div>
  );
}

const ComposerBox = memo(function ComposerBox({
  draftKey,
  working,
  disabled,
  placeholder,
  commandsFor,
  suggestion = null,
  shell: shellOffered = false,
  brief,
  onSend,
  onInterrupt,
  controls,
}: {
  /** What the user writes is kept under it while they look elsewhere: the chat, or the workspace's first chat. */
  draftKey: string;
  working: boolean;
  disabled?: boolean;
  placeholder: string;
  /** Whose commands `/` lists. */
  commandsFor: CommandsSource;
  /** The agent's suggested next message, offered while the composer is empty and the chat idle. */
  suggestion?: Suggestion | null;
  /** A text starting with "!" runs as a shell command: the composer shows its shell mode. */
  shell?: boolean;
  /** A design chat's brief: once written, it fills the draft (`draftKey` is the chat). */
  brief?: DesignBrief;
  onSend: (text: string, images: PendingImage[], mode: SendMode) => Promise<void>;
  onInterrupt: () => void;
  controls: React.ReactNode;
}) {
  const [text, setText] = useState(() => drafts.text(draftKey));
  const [sending, setSending] = useState(false);
  const area = useRef<ComposerEditorHandle>(null);
  const attachments = useImageAttachments(
    () => area.current?.focus(),
    () => drafts.files(draftKey),
  );
  const { images, reading } = attachments;
  // Kept for when the user comes back to this chat; taken when written from elsewhere meanwhile.
  useEffect(() => drafts.setText(draftKey, text), [draftKey, text]);
  useEffect(() => drafts.subscribe(draftKey, setText), [draftKey]);
  useEffect(() => drafts.setFiles(draftKey, images), [draftKey, images]);
  const slash = useSlashMenu(text, commandsFor, (command) => {
    setText(`/${command.name} `);
    area.current?.focus();
  });
  // Set aside (Esc, or taken with Tab), for this chat, until the next one arrives. Typing only hides
  // it: it shows again once the composer is empty.
  const [dismissed, setDismissed] = useState(() => dismissedSuggestions.get(draftKey));
  const ghost = shownSuggestion(suggestion, { text, images: images.length, working, dismissed });
  const dismiss = (key: string) => {
    setDismissed(key);
    dismissedSuggestions.set(draftKey, key);
  };
  useEffect(() => {
    area.current?.focus();
  }, []);

  // The brief goes into an empty draft once written, and replaces the draft after a Regenerate;
  // also when it is written while the composer is shown.
  useEffect(() => {
    const action = briefFill(brief, text, briefMemos.get(draftKey));
    if (!action || !brief?.text) return;
    briefMemos.seen(draftKey, brief.text);
    if (action === "fill") setText(brief.text);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draftKey, brief?.state, brief?.text]);

  const canSend = (text.trim() !== "" || images.length > 0) && reading === 0;
  const shell = shellOffered && inShellMode(text);
  const submit = async (mode: SendMode = "steer") => {
    const value = text.trim();
    if (!canSend || sending) return;
    setSending(true);
    // The composer empties at once: the message shows in the chat before the daemon answers, and
    // must not show in both places meanwhile.
    const sent = attachments.detach();
    setText("");
    try {
      await onSend(value, sent, mode);
      attachments.release(sent);
    } catch {
      // The caller showed the error: the text and the images come back, before anything typed since.
      setText((current) => (current ? `${value}\n${current}` : value));
      attachments.restore(sent);
    } finally {
      setSending(false);
      area.current?.focus();
    }
  };

  return (
    <div className="shrink-0 px-5 pb-3 pt-1">
      <div
        {...attachments.dropTarget}
        className={cx(
          "relative mx-auto max-w-[860px] rounded-lg border bg-panel",
          shell ? "border-attention/60" : "border-line-strong focus-within:border-accent/50",
        )}
      >
        {attachments.dropping && <DropOverlay />}
        {shell && (
          <div className="px-3 pt-2 font-mono text-[10.5px] text-attention" data-testid="shell-mode">
            ! shell: runs in the workspace folder; the agent reads it with your next message ({"\\!"} sends a message)
          </div>
        )}
        {slash.menu}
        <ImageStrip images={images} reading={reading} onRemove={attachments.remove} className="px-2.5 pt-2.5" />
        <ComposerEditor
          ref={area}
          value={text}
          disabled={disabled}
          onChange={setText}
          onPaste={attachments.onPaste}
          onKeyDown={(e) => {
            const offer = suggestionKey(ghost, {
              key: e.key,
              shiftKey: e.shiftKey,
              metaKey: e.metaKey,
              ctrlKey: e.ctrlKey,
              altKey: e.altKey,
              isComposing: e.isComposing,
            });
            if (offer && ghost) {
              e.preventDefault();
              dismiss(ghost.key);
              // The editor puts the caret after it.
              if (offer === "accept") setText(ghost.text);
              return;
            }
            if (slash.onKeyDown(e)) return;
            if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
              e.preventDefault();
              // ⌘⏎ while the agent works: wait for the end of its turn instead of steering it.
              void submit(hasMod(e) && working ? "queue" : "steer");
            } else if (e.key === "Escape" && working) {
              e.preventDefault();
              onInterrupt();
            }
          }}
          placeholder={ghost ? ghost.text : placeholder}
        />
        <div className="flex flex-wrap items-center gap-1.5 px-2 pb-2">
          <AttachButton onFiles={(files) => void attachments.addFiles(files)} disabled={disabled} />
          {controls}
          <div className="ml-auto flex items-center gap-1.5">
            {ghost && (
              <span className="mr-1 text-[11px] text-faint" data-testid="prompt-suggestion" title="The agent's guess at your next message">
                <Kbd>tab</Kbd> to use the suggestion · <Kbd>esc</Kbd> to dismiss
              </span>
            )}
            {working && (
              <Button size="sm" variant="ghost" onClick={onInterrupt} title="Interrupt (Esc)">
                <Square size={10} fill="currentColor" /> Stop
              </Button>
            )}
            {working && (
              <Button
                size="sm"
                disabled={!canSend || sending}
                onClick={() => void submit("queue")}
                title={`Queue: sent when the agent finishes its turn (${kbd("⌘↵")})`}
                data-testid="queue-button"
              >
                <ListPlus size={12} /> Queue
              </Button>
            )}
            <Button
              size="sm"
              variant="primary"
              disabled={!canSend}
              busy={sending}
              onClick={() => void submit()}
              title={working ? "Steer: the agent reads it at its next step (↵)" : "Send (↵)"}
            >
              <ArrowUp size={12} /> {working ? "Steer" : "Send"}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
});

/** What the composer says once a shared design was imported: the user presses Send, or not. */
function importBrief(url: string): string {
  return `This design was imported from a colleague's share (${url}): read canvas.json and comments.json in the design folder, then wait for what to change.`;
}

/** The link of a shared design, for `design.import` (story 51). */
function ImportDesignDialog({ onClose, onImport }: { onClose: () => void; onImport: (url: string) => Promise<void> }) {
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    if (!url.trim() || busy) return;
    setBusy(true);
    try {
      await onImport(url.trim());
    } catch (error) {
      toast(`Could not import the design: ${errorMessage(error)}`, "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      open
      onClose={onClose}
      title="Import a shared design"
      width={480}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => void submit()} disabled={!url.trim()} busy={busy} data-testid="import-design-go">
            Import
          </Button>
        </>
      }
    >
      <form
        className="flex flex-col gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <p className="text-[12.5px] text-muted">
          The link a colleague shared with you (<span className="text-fg">…/s/…</span>). Its screens and comments land in a new design chat of this workspace, for your own agent; the share itself is left as it is.
        </p>
        <input
          className={inputClass}
          placeholder="https://share.example.com/s/…"
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          autoFocus
          data-testid="import-design-url"
        />
      </form>
    </Modal>
  );
}
