import type { Attachment, JsonValue, RequestResponse } from "@chauffeur/protocol";
import {
  AlertTriangle,
  Brain,
  Check,
  ChevronDown,
  ChevronRight,
  CircleHelp,
  Download,
  Gauge,
  FileText,
  FolderPlus,
  GitFork,
  Globe,
  Info,
  Layers,
  PanelRightOpen,
  Pencil,
  Search,
  ShieldQuestion,
  SquareTerminal,
  Users,
  Wrench,
  X,
} from "lucide-react";
import { createContext, memo, type ReactNode, useContext, useState } from "react";
import { ElicitationCard } from "./ElicitationCard";
import { ShareCommentCards } from "./ShareComments";
import { ChatHostContext } from "./chatHost";
import type {
  ActivityGroup,
  AssistantItem,
  ChatItem,
  ChatRow,
  ElicitationItem,
  NoticeItem,
  PermissionItem,
  QuestionItem,
  ShellItem,
  ThinkingItem,
  ToolItem,
  TurnItem,
  UserItem,
} from "@/lib/chat";
import {
  activitySummary,
  ASK_USER_QUESTION,
  delegatedAgent,
  delegatedChat,
  isDelegateCall,
  parseAnswers,
  permissionTitle,
  type RepoProposal,
  repoProposal,
  repoProposalOutcome,
  ruleLabel,
} from "@/lib/chat";
import { isAgentCall, type SubagentTarget } from "@/lib/subagent";
import { foldedOutput, shellEnding } from "@/lib/shell";
import { formatCost, formatDuration, formatTokens } from "@/lib/format";
import { usageNotice } from "@/lib/usage-notice";
import { withoutPrivate } from "@/lib/files";
import { useFileLinker, useFileMenu } from "../FileLinks";
import { Lightbox } from "../Lightbox";
import { FileIcon } from "./FileIcon";
import { formatBytes } from "@/lib/images";
import { Markdown } from "../Markdown";
import { CopyMessageButton } from "../CopyMessageButton";
import { ContextMenu } from "../ContextMenu";
import { saveFiles } from "../saveFiles";
import { canDownload, type Download as DownloadFile } from "@/lib/downloads";
import { Badge, Button, cx, IconButton, Spinner } from "../ui";
import { AgentIcon } from "../AgentIcon";

export type Respond = (requestId: string, response: RequestResponse) => Promise<void>;

/** The workspace folder: paths under it are shown relative to it. */
export const WorkspacePathContext = createContext<string | null>(null);
/** The workspace a chat belongs to: its attachments are served under its id. */
export const WorkspaceIdContext = createContext<string | null>(null);
/** The chat shown: a `delegate` call finds the delegated chat it started among this chat's. */
export const ChatIdContext = createContext<string | null>(null);

function useRelative(): (text: string) => string {
  const base = useContext(WorkspacePathContext);
  if (!base) return (text) => text;
  const prefix = `${withoutPrivate(base.replace(/\/$/, ""))}/`;
  // Agents may report /tmp/… as /private/tmp/… on macOS.
  return (text) => text.split(`/private${prefix}`).join("").split(prefix).join("");
}

/** Which folded groups of steps are open, kept by the chat: the list unmounts rows scrolled away. */
export const ActivityContext = createContext<{ open: ReadonlySet<string>; toggle: (key: string) => void }>({
  open: new Set(),
  toggle: () => {},
});

/** The message a jump from the timeline rail just reached: it flashes for a moment. */
export const HighlightContext = createContext<string | null>(null);

/**
 * Opens a subagent's own view (SubagentPanel) from its Agent call; null where there is none
 * (Codex chats). Inside the panel, a nested subagent's call opens it there.
 */
export const SubagentOpenContext = createContext<((target: SubagentTarget) => void) | null>(null);

/**
 * Forks the chat as it stood at the end of one of its turns (by number); null where the chat
 * cannot be forked (read-only, a design chat, a subagent's view).
 */
export const ForkTurnContext = createContext<((turn: number) => void) | null>(null);

/**
 * Interrupts the turn, so that the messages steering it, not read yet, run as the next one
 * instead of waiting for the end of the step under way; null where the chat cannot be interrupted.
 */
export const SendNowContext = createContext<(() => void) | null>(null);

/** Tool calls auto mode is checking before they run (`ChatView.checking`), by tool use id. */
export const PermissionChecksContext = createContext<ReadonlySet<string>>(new Set());

export const ChatRowView = memo(function ChatRowView({
  row,
  last,
  working,
  readOnly,
  respond,
}: {
  row: ChatRow;
  /** The last row of the chat: a group there shows the step the agent is on. */
  last: boolean;
  working: boolean;
  readOnly: boolean;
  respond: Respond;
}) {
  if (row.kind === "activity") {
    return (
      <div className="mx-auto w-full max-w-[860px] px-5 py-1" data-kind="activity">
        <ActivityGroupView group={row} live={last && working} working={working} readOnly={readOnly} respond={respond} />
      </div>
    );
  }
  return (
    <div className="mx-auto w-full max-w-[860px] px-5 py-1" data-kind={row.kind}>
      <ItemBody item={row} working={working} readOnly={readOnly} respond={respond} />
    </div>
  );
});

/** A permission request or an MCP confirmation, as the chat shows it: also shown in a parent's chat. */
export function RequestCard({ item, readOnly, respond }: { item: PermissionItem | ElicitationItem; readOnly: boolean; respond: Respond }) {
  if (item.kind === "elicitation") return <ElicitationCard item={item} readOnly={readOnly} respond={respond} />;
  const proposal = repoProposal(item);
  return proposal ? (
    <AddRepoCard item={item} proposal={proposal} readOnly={readOnly} respond={respond} />
  ) : (
    <PermissionCard item={item} readOnly={readOnly} respond={respond} />
  );
}

function ItemBody({
  item,
  working,
  readOnly,
  respond,
}: {
  item: ChatItem;
  working: boolean;
  readOnly: boolean;
  respond: Respond;
}) {
  const body = (() => {
    switch (item.kind) {
      case "user":
        return <UserMessage item={item} />;
      case "assistant":
        return <AssistantMessage item={item} />;
      case "thinking":
        return <Thinking item={item} />;
      case "tool":
        if (isDelegateCall(item)) return <DelegateCall item={item} />;
        return item.name === ASK_USER_QUESTION ? (
          <AskUserQuestionCall item={item} />
        ) : (
          <ToolCall item={item} working={working} />
        );
      case "permission":
      case "elicitation":
        return <RequestCard item={item} readOnly={readOnly} respond={respond} />;
      case "question":
        return <QuestionRecord item={item} />;
      case "notice":
        return <Notice item={item} />;
      case "shell":
        return <ShellCommand item={item} />;
      case "turn":
        return <TurnSummary item={item} />;
    }
  })();
  return item.depth > 0 ? (
    <div className="border-l border-line-strong pl-3" style={{ marginLeft: (item.depth - 1) * 14 + 6 }}>
      {body}
    </div>
  ) : (
    body
  );
}

/**
 * The agent's steps between two of its messages, folded into one line: what it ran, by tool, and
 * whether any failed. While it works, the step it is on shows under the line. Images a step
 * returned stay visible.
 */
function ActivityGroupView({
  group,
  live,
  working,
  readOnly,
  respond,
}: {
  group: ActivityGroup;
  live: boolean;
  working: boolean;
  readOnly: boolean;
  respond: Respond;
}) {
  const { open: openGroups, toggle } = useContext(ActivityContext);
  const open = openGroups.has(group.key);
  const { tools, names, failed, pending } = activitySummary(group);
  const title = tools > 0 ? `${tools} ${tools === 1 ? "tool call" : "tool calls"}` : `${group.items.length} steps`;
  const shown = names.slice(0, 4);
  const current = live && !open ? group.items[group.items.length - 1] : null;
  const attachments = open
    ? []
    : group.items.flatMap((item) => (item.kind === "tool" && item.result ? item.result.attachments : []));
  return (
    <div className="py-0.5">
      <button
        onClick={() => toggle(group.key)}
        className="group flex w-full min-w-0 items-center gap-1.5 rounded px-1 py-0.5 text-left text-[12px] hover:bg-hover/60"
        aria-expanded={open}
      >
        <span className="text-faint">{open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}</span>
        <span className="text-muted">
          <Layers size={12} />
        </span>
        <span className="shrink-0 font-medium text-muted">{title}</span>
        <span className="min-w-0 truncate text-faint">
          {shown.map(({ name, count }) => (count > 1 ? `${name} ×${count}` : name)).join(" · ")}
          {names.length > shown.length && ` · +${names.length - shown.length}`}
        </span>
        <span className="ml-auto flex shrink-0 items-center gap-1.5 pl-2">
          {failed > 0 && (
            <span className="flex items-center gap-0.5 text-[11px] text-danger">
              <X size={12} /> {failed} failed
            </span>
          )}
          {pending && working ? (
            <Spinner size={11} className="text-working" />
          ) : failed === 0 && tools > 0 ? (
            <Check size={12} className="text-success/80" />
          ) : null}
        </span>
      </button>
      {open && (
        <div className="ml-[11px] mt-0.5 border-l border-line pl-2">
          {group.items.map((item) => (
            <ItemBody key={item.key} item={item} working={working} readOnly={readOnly} respond={respond} />
          ))}
        </div>
      )}
      {current && (
        <div className="ml-[11px] mt-0.5 border-l border-line pl-2">
          <ItemBody item={current} working={working} readOnly={readOnly} respond={respond} />
        </div>
      )}
      {attachments.length > 0 && !current && (
        <div className="ml-5 mt-1">
          <AttachedImages attachments={attachments} />
        </div>
      )}
    </div>
  );
}

/** The user's own messages: a bubble on the right, apart from the agent's work on the left. */
function UserMessage({ item }: { item: UserItem }) {
  const highlighted = useContext(HighlightContext) === item.key;
  const sendNow = useContext(SendNowContext);
  return (
    <div className="mt-4 flex justify-end">
      <div
        className={cx(
          "max-w-[85%] rounded-2xl rounded-br-md border border-accent/30 bg-accent/[0.14] px-3.5 py-2 transition-shadow duration-700",
          highlighted && "ring-2 ring-accent/70",
        )}
      >
        {item.attachments.length > 0 && <AttachedImages attachments={item.attachments} />}
        {item.text && (
          <div className="whitespace-pre-wrap break-words text-fg selectable" data-find-text>
            {item.text}
          </div>
        )}
        {item.queued && (
          <div className="mt-1 flex items-center gap-1.5">
            <Badge tone="working" title="Sent while the agent works: it reads it at its next step, once the step under way ends">
              steering…
            </Badge>
            {sendNow && (
              <Button
                size="xs"
                variant="ghost"
                onClick={sendNow}
                title="Interrupt the agent so that it reads this now (as Esc does)"
                data-testid="send-now"
              >
                Send now
              </Button>
            )}
          </div>
        )}
        {item.suggestedBy && (
          <div className="mt-1 text-right text-[11px] text-muted" title={item.suggestedBy.email} data-testid="suggested-by">
            Suggested by {item.suggestedBy.name}
          </div>
        )}
        {item.delegatedAnswer && (
          <div
            className="mt-1 text-right text-[11px] text-muted"
            title="Your agent's turn had ended before this delegated chat answered: Chauffeur sent its answer"
            data-testid="delegated-answer"
          >
            Answer of the delegated chat {item.delegatedAnswer.title ? `“${item.delegatedAnswer.title}”` : item.delegatedAnswer.chat}
          </div>
        )}
      </div>
    </div>
  );
}

/** A command the user ran with `!`: theirs, on the right; its output folded past a few lines. */
function ShellCommand({ item }: { item: ShellItem }) {
  const [all, setAll] = useState(false);
  const folded = foldedOutput(item.output);
  const ending = shellEnding(item);
  return (
    <div className="mt-4 flex justify-end" data-testid="shell-command">
      <div className="w-[85%] overflow-hidden rounded-md border border-line bg-[#121215]">
        <div className="flex items-center gap-2 border-b border-line px-2 py-1">
          <SquareTerminal size={12} className="shrink-0 text-faint" />
          <code className="min-w-0 flex-1 truncate font-mono text-[11.5px] text-fg selectable" title={item.command}>
            {item.command}
          </code>
          {ending ? (
            <Badge tone={item.exitCode === 0 ? "neutral" : "danger"}>{ending}</Badge>
          ) : (
            <Spinner />
          )}
        </div>
        {!item.running && (
          <pre className="max-h-[320px] overflow-auto whitespace-pre-wrap break-words px-2 py-1.5 font-mono text-[11.5px] leading-snug text-muted selectable">
            {(all ? item.output.replace(/\n+$/, "") : folded.text) || "(no output)"}
          </pre>
        )}
        {!all && folded.hidden > 0 && (
          <button className="px-2 pb-1 text-[11px] text-accent hover:underline" onClick={() => setAll(true)}>
            Show {folded.hidden} more lines
          </button>
        )}
      </div>
    </div>
  );
}

/** An attachment's URL where the chat is shown; null where none is served. */
function useAttachmentUrl(): (attachment: Attachment) => string | null {
  const workspaceId = useContext(WorkspaceIdContext);
  const host = useContext(ChatHostContext);
  return (attachment) => (workspaceId ? host.attachmentUrl(workspaceId, attachment) : null);
}

const IMAGE_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);

/** A message's attachments: images as thumbnails that open full screen, other files by name. */
function AttachedImages({ attachments }: { attachments: Attachment[] }) {
  const url = useAttachmentUrl();
  /** The open image's place among the images, which the Lightbox steps through. */
  const [open, setOpen] = useState<number | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; download: DownloadFile } | null>(null);
  const images = attachments.filter((attachment) => IMAGE_TYPES.has(attachment.media_type));
  /** What can be saved: the images the daemon serves. */
  const downloads = images.flatMap((image) => {
    const src = url(image);
    return canDownload && src ? [{ url: src, name: image.name }] : [];
  });
  return (
    <div className="mb-1.5 flex flex-wrap items-start gap-1.5">
      {attachments.map((attachment) => {
        if (!IMAGE_TYPES.has(attachment.media_type)) return <AttachedFile key={attachment.file} attachment={attachment} />;
        const src = url(attachment);
        return (
          <div key={attachment.file} className="group relative">
            <button
              type="button"
              title={attachment.name}
              onClick={() => setOpen(images.indexOf(attachment))}
              onContextMenu={(event) => {
                if (!src || !canDownload) return;
                event.preventDefault();
                setMenu({ x: event.clientX, y: event.clientY, download: { url: src, name: attachment.name } });
              }}
              className="h-20 max-w-[220px] overflow-hidden rounded-md border border-line bg-bg hover:border-line-strong"
            >
              {src ? (
                <img src={src} alt={attachment.name} className="h-full w-auto object-cover" draggable={false} />
              ) : (
                <span className="px-2 text-[11px] text-faint">{attachment.name}</span>
              )}
            </button>
            {src && canDownload && (
              <IconButton
                title={`Download ${attachment.name}`}
                onClick={() => void saveFiles([{ url: src, name: attachment.name }])}
                className="absolute right-1 bottom-1 bg-black/70 text-white opacity-0 hover:bg-black/80 hover:text-white focus-visible:opacity-100 group-hover:opacity-100"
              >
                <Download size={12} />
              </IconButton>
            )}
          </div>
        );
      })}
      {downloads.length > 1 && (
        <button
          type="button"
          title={`Download the ${downloads.length} images`}
          onClick={() => void saveFiles(downloads)}
          className="flex h-6 items-center gap-1 self-end rounded-md px-1.5 text-[11.5px] text-muted hover:bg-hover hover:text-fg"
        >
          <Download size={12} /> Download all
        </button>
      )}
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={[{ label: "Download", icon: <Download size={13} />, shortcut: "d", onSelect: () => void saveFiles([menu.download]) }]}
          onClose={() => setMenu(null)}
        />
      )}
      {open != null && (
        <Lightbox
          images={images.map((image) => ({ src: url(image), name: image.name, downloadable: true }))}
          start={open}
          onClose={() => setOpen(null)}
        />
      )}
    </div>
  );
}

/** A file that is not an image: its name and size; it opens in the Files tab when listed there. */
function AttachedFile({ attachment }: { attachment: Attachment }) {
  const linker = useFileLinker();
  const { onContextMenu, menu } = useFileMenu(linker);
  const ref = linker?.resolve(attachment.path) ?? null;
  const body = (
    <>
      <FileIcon name={attachment.name} size={18} className="shrink-0 text-muted" />
      <span className="min-w-0 text-left">
        <span className="block truncate text-[12px] text-fg">{attachment.name}</span>
        <span className="block text-[11px] text-faint">{formatBytes(attachment.size)}</span>
      </span>
    </>
  );
  const className = "flex h-12 max-w-[260px] items-center gap-2 rounded-md border border-line bg-bg px-2.5";
  return ref && linker ? (
    <>
      <button
        type="button"
        title={`Open ${attachment.name}`}
        onClick={() => linker.open(ref)}
        onContextMenu={onContextMenu(ref)}
        className={cx(className, "hover:border-line-strong")}
        data-attachment={attachment.file}
      >
        {body}
      </button>
      {menu}
    </>
  ) : (
    <div className={className} title={attachment.name} data-attachment={attachment.file}>
      {body}
    </div>
  );
}

function AssistantMessage({ item }: { item: AssistantItem }) {
  const highlighted = useContext(HighlightContext) === item.key;
  return (
    <div className={cx("rounded py-1 text-fg transition-shadow duration-700", highlighted && "ring-2 ring-accent/70")}>
      {/* A guest's selection in it is commented on by its message id (the share's guest page). */}
      <div data-find-text data-message-id={item.messageId}><Markdown text={item.text} streaming={item.streaming} /></div>
      {!item.streaming && item.text.trim() && <CopyMessageButton text={item.text} />}
      <ShareCommentCards itemKey={item.key} />
    </div>
  );
}

function Thinking({ item }: { item: ThinkingItem }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="py-0.5">
      <button
        onClick={() => setOpen((v) => !v)}
        className="flex items-center gap-1.5 text-[12px] text-faint hover:text-muted"
      >
        {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        <Brain size={12} />
        {item.streaming ? "Thinking…" : "Thought"}
        {!open && item.text && (
          <span className="max-w-[520px] truncate font-normal italic">{item.text.split("\n")[0]}</span>
        )}
      </button>
      {open && (
        <div className="mt-1 whitespace-pre-wrap border-l border-line pl-3 text-[12px] italic text-muted selectable">
          {item.text}
        </div>
      )}
    </div>
  );
}

function toolIcon(name: string) {
  const n = name.toLowerCase();
  if (n === "bash" || n.includes("shell") || n === "exec_command") return <SquareTerminal size={12} />;
  if (n === "read" || n === "notebookread") return <FileText size={12} />;
  if (n === "edit" || n === "write" || n === "multiedit" || n === "notebookedit" || n === "apply_patch")
    return <Pencil size={12} />;
  if (n === "grep" || n === "glob" || n.includes("search")) return <Search size={12} />;
  if (n === "webfetch" || n === "websearch") return <Globe size={12} />;
  if (n === "task" || n === "agent") return <Users size={12} />;
  return <Wrench size={12} />;
}

/** The file a tool read or wrote opens in the Files tab; other summaries are plain text. */
function ToolSummary({ item, text }: { item: ToolItem; text: string }) {
  const linker = useFileLinker();
  const { onContextMenu, menu } = useFileMenu(linker);
  const input = item.input && typeof item.input === "object" && !Array.isArray(item.input) ? item.input : null;
  const path = input && (input.file_path ?? input.notebook_path);
  const ref = linker && typeof path === "string" ? linker.resolve(path) : null;
  if (!ref || !linker) {
    return (
      <span className="min-w-0 truncate font-mono text-[11.5px] text-faint" title={item.summary}>
        {text}
      </span>
    );
  }
  return (
    <>
      <span
        role="link"
        tabIndex={0}
        title={`Open ${ref.path}`}
        className="min-w-0 truncate font-mono text-[11.5px] text-faint underline decoration-dotted decoration-faint/60 underline-offset-2 hover:text-accent"
        onClick={(event) => {
          event.stopPropagation();
          linker.open(ref);
        }}
        onContextMenu={onContextMenu(ref)}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.stopPropagation();
            linker.open(ref);
          }
        }}
        data-file-link={ref.path}
      >
        {text}
      </span>
      {menu}
    </>
  );
}

function ToolCall({ item, working }: { item: ToolItem; working: boolean }) {
  const [open, setOpen] = useState(false);
  const relative = useRelative();
  const pending = item.result == null;
  const checking = useContext(PermissionChecksContext).has(item.toolUseId) && pending;
  // A subagent's call opens its conversation beside the chat rather than unfolding here.
  const openSubagent = useContext(SubagentOpenContext);
  const subagent = openSubagent && isAgentCall(item.name) ? openSubagent : null;
  return (
    <div className="py-0.5">
      <button
        onClick={() => (subagent ? subagent({ toolUseId: item.toolUseId, agentId: null }) : setOpen((v) => !v))}
        className="group flex w-full min-w-0 items-center gap-1.5 rounded px-1 py-0.5 text-left text-[12px] hover:bg-hover/60"
        data-tool={item.name}
        data-subagent={subagent ? item.toolUseId : undefined}
        title={subagent ? "Open its conversation" : undefined}
      >
        <span className="text-faint">
          {subagent ? <PanelRightOpen size={12} /> : open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        </span>
        <span className="text-muted">{toolIcon(item.name)}</span>
        <span className="shrink-0 font-medium text-muted">{item.name}</span>
        {item.summary && <ToolSummary item={item} text={relative(item.summary)} />}
        <span className="ml-auto flex shrink-0 items-center gap-1.5 pl-2">
          {checking && (
            <span className="text-[11px] text-faint" data-testid="permission-check">
              Auto mode is checking this call…
            </span>
          )}
          {pending ? (
            working || checking ? (
              <Spinner size={11} className="text-working" />
            ) : null
          ) : item.result!.isError ? (
            <X size={12} className="text-danger" />
          ) : (
            <Check size={12} className="text-success/80" />
          )}
        </span>
      </button>
      {open && (
        <div className="ml-5 mt-1 space-y-1.5">
          <ToolInput name={item.name} input={item.input} />
          {item.result && <ToolOutput output={item.result.output} isError={item.result.isError} />}
        </div>
      )}
      {!!item.result?.attachments.length && (
        <div className="ml-5 mt-1">
          <AttachedImages attachments={item.result.attachments} />
        </div>
      )}
    </div>
  );
}

/**
 * The agent gave a task to a delegated chat (story 49): the other agent, the task, where it stands,
 * and its chat one click away; its answer unfolds once given.
 */
function DelegateCall({ item }: { item: ToolItem }) {
  const [open, setOpen] = useState(false);
  const parentId = useContext(ChatIdContext);
  const host = useContext(ChatHostContext);
  const child = parentId ? delegatedChat(item, parentId, Object.values(host.chats)) : null;
  const agent = delegatedAgent(item, child);
  const name = agent === "codex" ? "Codex" : "Claude";
  const input = asObject(item.input);
  const prompt = typeof input?.prompt === "string" ? input.prompt.trim() : "";
  // The chat's title once it differs from the task it was named after (renamed, or titled by a model).
  const title = child?.title && !prompt.startsWith(child.title.replace(/…$/, "").trim()) ? child.title : null;
  const status = (() => {
    if (item.result) return item.result.isError ? { tone: "danger", label: "stopped" } : { tone: "success", label: "answered" };
    if (child?.status === "needs_input") return { tone: "attention", label: "waiting for you" };
    return { tone: "working", label: "working" };
  })() as { tone: "danger" | "success" | "attention" | "working"; label: string };
  const show = () => {
    if (child) host.openChat(child);
  };
  return (
    <div className="my-2 rounded-lg border border-line bg-panel px-3 py-2.5" data-testid="delegate-call" data-subagent-chat={child?.id}>
      <div className="flex min-w-0 items-center gap-2">
        <AgentIcon agent={agent} size={14} className="shrink-0" />
        <span className="shrink-0 font-medium text-fg">{name}</span>
        {title && (
          <span className="min-w-0 truncate text-[12px] text-muted" title={title}>
            {title}
          </span>
        )}
        <Badge tone={status.tone} className="ml-auto shrink-0">
          {status.label === "working" && <Spinner size={10} />}
          {status.label}
        </Badge>
        {child && (
          <Button size="sm" onClick={show} title={`Show ${name}'s chat`}>
            Open chat
          </Button>
        )}
      </div>
      {prompt && (
        <div className="mt-1.5 line-clamp-3 whitespace-pre-wrap text-[12px] text-muted selectable" title={prompt}>
          {prompt}
        </div>
      )}
      {item.result && (
        <div className="mt-1.5">
          <button
            className="flex items-center gap-1 text-[11.5px] text-faint hover:text-fg"
            onClick={() => setOpen((v) => !v)}
            data-testid="delegate-answer"
          >
            {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
            {item.result.isError ? "What happened" : "Its answer"}
          </button>
          {open && (
            <div className="mt-1">
              <ToolOutput output={item.result.output} isError={item.result.isError} />
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** An Edit's replacement as lines: the lines both sides share at the start and end as context. */
function editLines(before: string, after: string): Array<{ type: "ctx" | "del" | "add"; text: string }> {
  const a = before.split("\n");
  const b = after.split("\n");
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let end = 0;
  while (end < a.length - start && end < b.length - start && a[a.length - 1 - end] === b[b.length - 1 - end]) end++;
  return [
    ...a.slice(0, start).map((text) => ({ type: "ctx" as const, text })),
    ...a.slice(start, a.length - end).map((text) => ({ type: "del" as const, text })),
    ...b.slice(start, b.length - end).map((text) => ({ type: "add" as const, text })),
    ...a.slice(a.length - end).map((text) => ({ type: "ctx" as const, text })),
  ];
}

function asObject(input: JsonValue): Record<string, JsonValue> | null {
  return input && typeof input === "object" && !Array.isArray(input) ? input : null;
}

function ToolInput({ name, input }: { name: string; input: JsonValue }) {
  const obj = asObject(input);
  const relative = useRelative();
  const n = name.toLowerCase();
  if (obj && n === "bash" && typeof obj.command === "string") {
    return <Pre label="command">{obj.command}</Pre>;
  }
  if (obj && n === "edit" && typeof obj.old_string === "string" && typeof obj.new_string === "string") {
    return (
      <div className="overflow-hidden rounded-md border border-line font-mono text-[11.5px]">
        {typeof obj.file_path === "string" && (
          <div className="border-b border-line bg-raised px-2 py-1 text-faint">{relative(obj.file_path)}</div>
        )}
        <div className="max-h-[320px] overflow-auto selectable">
          {editLines(obj.old_string, obj.new_string).map((line, i) => (
            <div
              key={i}
              className={cx(
                "whitespace-pre px-2",
                line.type === "del" && "bg-danger/10 text-[#f5a3a3]",
                line.type === "add" && "bg-success/10 text-[#9ee0bd]",
                line.type === "ctx" && "text-muted",
              )}
            >
              {line.type === "del" ? "-" : line.type === "add" ? "+" : " "}
              {line.text}
            </div>
          ))}
        </div>
      </div>
    );
  }
  if (obj && n === "write" && typeof obj.content === "string") {
    return <Pre label={typeof obj.file_path === "string" ? relative(obj.file_path) : "content"}>{obj.content}</Pre>;
  }
  if (input == null) return null;
  return <Pre label="input">{JSON.stringify(input, null, 2)}</Pre>;
}

const MAX_LINES = 60;

function ToolOutput({ output, isError }: { output: string; isError: boolean }) {
  const [all, setAll] = useState(false);
  const lines = output.split("\n");
  const shown = all ? output : lines.slice(0, MAX_LINES).join("\n");
  return (
    <div>
      <Pre label={isError ? "error" : "output"} tone={isError ? "danger" : undefined}>
        {shown || "(no output)"}
      </Pre>
      {!all && lines.length > MAX_LINES && (
        <button className="mt-0.5 text-[11px] text-accent hover:underline" onClick={() => setAll(true)}>
          Show all {lines.length} lines
        </button>
      )}
    </div>
  );
}

function Pre({ label, children, tone }: { label: string; children: ReactNode; tone?: "danger" }) {
  return (
    <div className="overflow-hidden rounded-md border border-line bg-[#121215]">
      <div className="border-b border-line px-2 py-0.5 text-[10.5px] uppercase tracking-wide text-faint">{label}</div>
      <pre
        className={cx(
          "max-h-[320px] overflow-auto whitespace-pre-wrap break-words px-2 py-1.5 font-mono text-[11.5px] leading-snug",
          tone === "danger" ? "text-danger" : "text-muted",
        )}
      >
        {children}
      </pre>
    </div>
  );
}

function RequestFrame({
  icon,
  title,
  state,
  outcome,
  outcomeTone = "success",
  children,
}: {
  icon: ReactNode;
  title: ReactNode;
  state: PermissionItem["state"];
  outcome: string | null;
  /** The badge's tone once answered (expired is always neutral). */
  outcomeTone?: "success" | "neutral";
  children: ReactNode;
}) {
  const waitingFor = useContext(ChatHostContext).waitingFor;
  return (
    <div
      className={cx(
        "my-2 rounded-lg border px-3 py-2.5",
        state === "open" ? "border-attention/40 bg-attention/[0.06]" : "border-line bg-panel",
      )}
      data-request-state={state}
    >
      <div className="mb-1.5 flex items-center gap-2">
        <span className={state === "open" ? "text-attention" : "text-faint"}>{icon}</span>
        <span className="font-medium text-fg">{title}</span>
        {state !== "open" && (
          <Badge tone={state === "expired" ? "neutral" : outcomeTone} className="ml-auto">
            {state === "expired" ? "expired" : (outcome ?? "answered")}
          </Badge>
        )}
        {state === "open" && waitingFor && (
          <Badge tone="attention" className="ml-auto" data-testid="request-waiting">
            Waiting for {waitingFor}
          </Badge>
        )}
      </div>
      {children}
    </div>
  );
}

function PermissionCard({ item, readOnly, respond }: { item: PermissionItem; readOnly: boolean; respond: Respond }) {
  const [busy, setBusy] = useState<string | null>(null);
  const relative = useRelative();
  const workspace = useContext(WorkspacePathContext);
  const answer = async (key: string, response: RequestResponse) => {
    setBusy(key);
    try {
      await respond(item.requestId, response);
    } finally {
      setBusy(null);
    }
  };
  const open = item.state === "open" && !readOnly;
  return (
    <RequestFrame
      icon={<ShieldQuestion size={14} />}
      title={permissionTitle(item)}
      state={item.state}
      outcome={item.rules ? "always allowed" : item.outcome}
    >
      {item.agent?.description && (
        <div className="mb-1 flex min-w-0 items-center gap-1.5 text-[11.5px] text-faint" data-testid="request-agent">
          <Users size={11} className="shrink-0" />
          <span className="truncate" title={item.agent.description}>
            {item.agent.description}
          </span>
        </div>
      )}
      {item.summary && <div className="mb-2 font-mono text-[11.5px] text-muted selectable">{relative(item.summary)}</div>}
      <ToolInputDisclosure name={item.toolName} input={item.input} />
      {item.rules && (
        <div className="mt-1.5 text-[11.5px] text-faint" data-testid="request-always">
          Not asked again in this chat:{" "}
          <span className="font-mono text-muted selectable">{item.rules.map((rule) => relative(ruleLabel(rule, workspace))).join(", ")}</span>
        </div>
      )}
      {open && (
        <div className="mt-2 flex gap-1.5">
          <Button variant="primary" size="sm" busy={busy === "allow"} onClick={() => answer("allow", { type: "allow", always: false })}>
            Allow
          </Button>
          <Button size="sm" busy={busy === "always"} onClick={() => answer("always", { type: "allow", always: true })}>
            Always allow
          </Button>
          <Button variant="danger" size="sm" busy={busy === "deny"} onClick={() => answer("deny", { type: "deny", message: null })}>
            Deny
          </Button>
        </div>
      )}
    </RequestFrame>
  );
}

/**
 * The agent proposes a repo for the workspace (story 48): the repo, what it is, why the agent needs
 * it, and Add or Not now. Never Always allow: each proposal is the user's call.
 */
function AddRepoCard({
  item,
  proposal,
  readOnly,
  respond,
}: {
  item: PermissionItem;
  proposal: RepoProposal;
  readOnly: boolean;
  respond: Respond;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const answer = async (key: string, response: RequestResponse) => {
    setBusy(key);
    try {
      await respond(item.requestId, response);
    } finally {
      setBusy(null);
    }
  };
  const open = item.state === "open" && !readOnly;
  const outcome = repoProposalOutcome(item);
  const workspaceId = useContext(WorkspaceIdContext);
  const explore = useContext(ChatHostContext).isExplore(workspaceId ?? "");
  return (
    <RequestFrame
      icon={<FolderPlus size={14} />}
      title={item.agent ? permissionTitle(item) : (item.title ?? `Add ${proposal.repo} to this workspace?`)}
      state={item.state}
      outcome={outcome}
      outcomeTone={outcome === "accepted" ? "success" : "neutral"}
    >
      <div data-testid="add-repo-card">
        {item.agent?.description && (
          <div className="mb-1 flex min-w-0 items-center gap-1.5 text-[11.5px] text-faint" data-testid="request-agent">
            <Users size={11} className="shrink-0" />
            <span className="truncate" title={item.agent.description}>
              {item.agent.description}
            </span>
          </div>
        )}
        <div className="flex min-w-0 items-baseline gap-2 selectable">
          <span className="font-mono text-[12px] text-fg">{proposal.repo}</span>
          {proposal.description && (
            <span className="truncate text-[11.5px] text-faint" title={proposal.description}>
              {proposal.description}
            </span>
          )}
        </div>
        {proposal.reason && (
          <div className="mt-1.5 border-l-2 border-line-strong pl-2 text-[12.5px] text-muted selectable" data-testid="add-repo-reason">
            {proposal.reason}
          </div>
        )}
        {open && (
          <div className="mt-2.5 flex items-center gap-1.5">
            <Button variant="primary" size="sm" busy={busy === "add"} onClick={() => answer("add", { type: "allow", always: false })}>
              Add {proposal.repo}
            </Button>
            <Button size="sm" busy={busy === "deny"} onClick={() => answer("deny", { type: "deny", message: null })}>
              Not now
            </Button>
            <span className="ml-1 text-[11px] text-faint">
              {explore ? "A read-only checkout of its base branch" : "On this workspace's branch, with its setup script"}
            </span>
          </div>
        )}
      </div>
    </RequestFrame>
  );
}

function ToolInputDisclosure({ name, input }: { name: string; input: JsonValue }) {
  const [open, setOpen] = useState(false);
  if (input == null) return null;
  return (
    <div>
      <button onClick={() => setOpen((v) => !v)} className="flex items-center gap-1 text-[11.5px] text-faint hover:text-muted">
        {open ? <ChevronDown size={11} /> : <ChevronRight size={11} />} details
      </button>
      {open && (
        <div className="mt-1">
          <ToolInput name={name} input={input} />
        </div>
      )}
    </div>
  );
}

function QuestionRecord({ item }: { item: QuestionItem }) {
  const open = item.state === "open";
  const waitingFor = useContext(ChatHostContext).waitingFor;
  return (
    <div className="my-1 rounded px-1 py-0.5 text-[12.5px]" data-request-state={item.state}>
      <div className={cx("flex items-center gap-1.5", open ? "text-attention" : "text-muted")}>
        <CircleHelp size={12} />
        <span className="font-medium">
          {open
            ? waitingFor
              ? `Waiting for ${waitingFor}'s answer`
              : "Waiting for your answer below"
            : item.answers
              ? item.questions.length === 1
                ? "You answered"
                : "You answered its questions"
              : item.state === "expired"
                ? "Not answered"
                : item.outcome === "denied"
                  ? "You chose to answer in the chat"
                  : "Answered"}
        </span>
      </div>
      <AskedList questions={item.questions.map((q) => q.question)} answers={item.answers} />
    </div>
  );
}

/** Claude's questions, each with the answer it got, the way Claude Code lists them. */
function AskedList({ questions, answers }: { questions: string[]; answers: Record<string, string> | null }) {
  return (
    <div className="ml-[18px] mt-0.5 space-y-0.5 selectable">
      {questions.map((question) => (
        <div key={question} className="text-[12px]">
          <span className="text-faint">· {question}</span>
          {answers?.[question] && <span className="text-fg"> → {answers[question]}</span>}
        </div>
      ))}
    </div>
  );
}

/** An AskUserQuestion call that never reached the user (Claude Code answered it itself). */
function AskUserQuestionCall({ item }: { item: ToolItem }) {
  const input = item.input && typeof item.input === "object" && !Array.isArray(item.input) ? item.input : {};
  const questions = (Array.isArray(input.questions) ? input.questions : [])
    .map((q) => (q && typeof q === "object" && !Array.isArray(q) && typeof q.question === "string" ? q.question : null))
    .filter((q): q is string => q !== null);
  const answers = item.result ? parseAnswers(item.result.output) : null;
  const answered = answers && Object.keys(answers).length > 0;
  return (
    <div className="my-1 rounded px-1 py-0.5 text-[12.5px]">
      <div className="flex items-center gap-1.5 text-muted">
        <CircleHelp size={12} />
        <span className="font-medium">{item.result == null ? "Asking you" : answered ? "Answered" : "Not answered"}</span>
      </div>
      <AskedList questions={questions} answers={answered ? answers : null} />
      {item.result && !answered && (
        <div className="ml-[18px] mt-0.5 text-[11.5px] text-faint selectable">{item.result.output.slice(0, 300)}</div>
      )}
    </div>
  );
}
function Notice({ item }: { item: NoticeItem }) {
  const usage = usageNotice(item.message);
  if (usage) return (
    <div
      className={cx(
        "my-3 flex items-start gap-3 rounded-xl border px-4 py-3",
        item.level === "error" ? "border-danger/20 bg-danger/5 text-danger" : "border-attention/20 bg-attention/5 text-attention",
      )}
      data-level={item.level}
      data-testid="usage-notice"
    >
      <div className={cx("mt-0.5 rounded-lg p-2", item.level === "error" ? "bg-danger/10" : "bg-attention/10")}>
        <Gauge size={16} aria-hidden="true" />
      </div>
      <div className="min-w-0 py-0.5 selectable">
        <div className="text-[13px] font-medium leading-5">{usage.title}</div>
        {usage.reset && <div className="mt-0.5 text-[12px] leading-5 text-muted">{usage.reset}</div>}
      </div>
    </div>
  );
  const Icon = item.level === "info" ? Info : AlertTriangle;
  return (
    <div
      className={cx(
        "my-1 flex items-start gap-2 rounded-md px-2.5 py-1.5 text-[12px]",
        item.level === "info" && "bg-raised text-muted",
        item.level === "warning" && "bg-attention/10 text-attention",
        item.level === "error" && "bg-danger/10 text-danger",
      )}
      data-level={item.level}
    >
      <Icon size={13} className="mt-0.5 shrink-0" />
      <span className="selectable">{item.message}</span>
      {item.count && (
        <Badge tone={item.level === "error" ? "danger" : item.level === "warning" ? "attention" : "neutral"} title={`${item.count} identical notices in a row`}>
          ×{item.count}
        </Badge>
      )}
      {item.badge && (
        <Badge tone={item.level === "error" ? "danger" : item.level === "warning" ? "attention" : "neutral"} className="selectable">
          {item.badge}
        </Badge>
      )}
    </div>
  );
}

function TurnSummary({ item }: { item: TurnItem }) {
  const forkTurn = useContext(ForkTurnContext);
  const parts = [
    item.outcome === "success" ? "Done" : item.outcome === "interrupted" ? "Interrupted" : "Failed",
    formatDuration(item.durationMs),
    formatCost(item.costUsd, item.costEstimated),
    formatTokens(item.tokens),
  ].filter(Boolean);
  const context = formatTokens(item.contextTokens);
  return (
    <div className="group/turn my-1.5">
      <div className="relative flex items-center gap-2 text-[11px] text-faint">
        <span className="h-px flex-1 bg-line" />
        <span
          className={cx(item.outcome === "error" && "text-danger", item.outcome === "interrupted" && "text-attention")}
          title={context ? `${item.tokens != null ? "Tokens used by the turn's model calls, cache included. " : ""}Context after the turn: ${context}` : undefined}
        >
          {parts.join(" · ")}
        </span>
        <span className="h-px flex-1 bg-line" />
        {forkTurn && item.forkable && (
          <button
            type="button"
            onClick={() => forkTurn(item.turn)}
            title="A new chat with the conversation up to here: from there, each goes its own way"
            className="absolute right-0 flex items-center gap-1 rounded bg-bg px-1.5 py-0.5 text-faint opacity-0 hover:text-fg focus-visible:opacity-100 group-hover/turn:opacity-100"
          >
            <GitFork size={11} />
            Fork from here
          </button>
        )}
      </div>
      {item.error && <div className="mt-1 text-center text-[12px] text-danger selectable">{item.error}</div>}
    </div>
  );
}
