// What is left of Claude Code's and Codex's plan limits, at the foot of the sidebar: a bar per
// window, coloured by pace (orange ahead of the time gone by, red far ahead or past 90%). Hovering
// it (or focusing it with the keyboard) opens a panel with every window, its pace and reset, a
// projection when a window would run out before it resets, extra usage and a failed read. Read
// every two minutes; the daemon asks the agents at most every five, and again after a turn.
// An agent signed out takes a red row of its own with a Sign in button; one whose chats lost an
// MCP server shows how many, the panel naming them and the chat each one is in.

import type { AgentKind, AgentUsage, ChatInfo, UsageWindow } from "@chauffeur/protocol";
import { AlertTriangle, Plug, RefreshCw } from "lucide-react";
import { Fragment, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useShallow } from "zustand/react/shallow";
import { type BrokenConnector, brokenConnectors, isBroken, lacksEnv, mcpStatusText } from "@/lib/connectors";
import { AGENT_NAMES } from "@/lib/models";
import {
  extraUsageText,
  failureText,
  pace,
  planName,
  projectionText,
  readAtText,
  resetText,
  type Tone,
  windowName,
} from "@/lib/usage";
import { loadUsage, selectChat, selectWorkspace, setTab, signIn, useApp } from "@/state/app";
import { AgentIcon } from "./AgentIcon";
import { MissingEnvHelp } from "./MissingEnvHelp";
import { Button, cx } from "./ui";

const POLL_MS = 120_000;
/** The windows shown in the row, one column each; the others are in the panel. */
const COLUMNS = ["5h", "7d"];
const OPEN_DELAY_MS = 250;
/** Long enough to move the pointer from the row to the panel. */
const CLOSE_DELAY_MS = 200;
const PANEL_WIDTH = 360;

export function UsageMeter() {
  const connected = useApp((s) => s.connection.status === "open");
  const agents = useApp((s) => s.usage);
  // Only the chats with a broken server: the meter stays still while other chats change.
  const broken = useApp(useShallow((s) => Object.values(s.chats).filter((c) => !c.closed && c.mcp_servers.some(isBroken))));
  const signingIn = useApp((s) => s.signingIn);
  const [now, setNow] = useState(Date.now());
  const [reading, setReading] = useState(false);
  const connectors = useMemo(() => ({ claude: brokenConnectors(broken, "claude"), codex: brokenConnectors(broken, "codex") }), [broken]);
  useEffect(() => setNow(Date.now()), [agents]);

  const load = useCallback((refresh = false) => {
    if (refresh) setReading(true);
    loadUsage(refresh)
      .catch(() => {})
      .finally(() => {
        if (refresh) setReading(false);
      });
  }, []);

  useEffect(() => {
    if (!connected) return;
    load();
    const timer = setInterval(() => load(), POLL_MS);
    const onFocus = () => load();
    window.addEventListener("focus", onFocus);
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [connected, load]);

  return (
    <UsageMeterView
      agents={agents ?? []}
      connectors={connectors}
      signingIn={signingIn}
      now={now}
      reading={reading}
      onRefresh={() => load(true)}
      onSignIn={(agent) => void signIn(agent)}
      onOpenChat={openChat}
    />
  );
}

function openChat(chat: ChatInfo) {
  selectWorkspace(chat.workspace_id);
  selectChat(chat.workspace_id, chat.id);
  setTab(chat.workspace_id, "chat");
}

const NO_CONNECTORS: Record<AgentKind, BrokenConnector[]> = { claude: [], codex: [] };

/** The row and its panel, fed by `UsageMeter` (or a dev fixture). */
export function UsageMeterView({
  agents,
  connectors = NO_CONNECTORS,
  signingIn = [],
  now,
  reading,
  onRefresh,
  onSignIn = () => {},
  onOpenChat = () => {},
}: {
  agents: AgentUsage[];
  /** Each agent's MCP servers that its chats running now lost. */
  connectors?: Record<AgentKind, BrokenConnector[]>;
  /** Agents whose login was started and has not come back yet. */
  signingIn?: AgentKind[];
  now: number;
  reading: boolean;
  onRefresh: () => void;
  onSignIn?: (agent: AgentKind) => void;
  onOpenChat?: (chat: ChatInfo) => void;
}) {
  const [open, setOpen] = useState(false);
  const area = useRef<HTMLDivElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const schedule = useCallback((next: boolean, delay: number) => {
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setOpen(next), delay);
  }, []);
  // Entering the row or the panel keeps it open; leaving both closes it.
  const hovered = useRef(false);
  const enter = () => {
    hovered.current = true;
    schedule(true, open ? 0 : OPEN_DELAY_MS);
  };
  const leave = () => {
    hovered.current = false;
    schedule(false, CLOSE_DELAY_MS);
  };
  useEffect(() => () => clearTimeout(timer.current), []);

  // Escape closes it, and does not reach the chat (where it would interrupt the agent).
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      clearTimeout(timer.current);
      setOpen(false);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open]);

  // The focus leaving both the row and the panel closes it, unless the pointer is over them.
  const blur = (event: React.FocusEvent) => {
    const to = event.relatedTarget as Node | null;
    if (!hovered.current && !area.current?.contains(to) && !panel.current?.contains(to)) schedule(false, 0);
  };

  const signedOut = agents.filter((a) => a.account === "signed_out");
  const shown = agents.filter((a) => a.windows.length > 0 && a.account !== "signed_out");
  const inPanel = agents.filter((a) => a.windows.length > 0 || a.account === "signed_out" || connectors[a.agent].length > 0);
  if (inPanel.length === 0) return null;
  return (
    <div ref={area} onMouseEnter={enter} onMouseLeave={leave} className="border-t border-line">
      {signedOut.map((usage) => (
        <SignedOutRow key={usage.agent} usage={usage} waiting={signingIn.includes(usage.agent)} onSignIn={() => onSignIn(usage.agent)} />
      ))}
      {shown.length > 0 && (
      <button
        type="button"
        onClick={onRefresh}
        // The keyboard opens it too; a click's focus does not.
        onFocus={(event) => {
          if (event.currentTarget.matches(":focus-visible")) schedule(true, 0);
        }}
        onBlur={blur}
        aria-label="Plan usage of Claude Code and Codex: click to read it again"
        aria-expanded={open}
        aria-haspopup="dialog"
        className="flex w-full flex-col gap-1 px-3 py-2 text-left hover:bg-hover/40"
        data-testid="usage-meter"
      >
        {shown.map((usage) => (
          <AgentRow key={usage.agent} usage={usage} now={now} broken={connectors[usage.agent].length} />
        ))}
      </button>
      )}
      {open && (
        <UsagePanel
          panelRef={panel}
          anchor={area}
          agents={inPanel}
          connectors={connectors}
          signingIn={signingIn}
          onSignIn={onSignIn}
          onOpenChat={onOpenChat}
          reading={reading}
          onRefresh={onRefresh}
          onMouseEnter={enter}
          onMouseLeave={leave}
          onBlur={blur}
        />
      )}
    </div>
  );
}

/** An agent signed out: every chat of it fails until the user signs in again. */
function SignedOutRow({ usage, waiting, onSignIn }: { usage: AgentUsage; waiting: boolean; onSignIn: () => void }) {
  return (
    <div
      className="flex items-center gap-2 bg-danger/10 px-3 py-1.5"
      title={usage.account_error ?? undefined}
      data-testid="usage-signed-out"
      data-agent={usage.agent}
    >
      <AgentIcon agent={usage.agent} size={12} className="shrink-0" />
      <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-danger" />
      <span className="min-w-0 flex-1 truncate text-[11.5px] text-danger">{AGENT_NAMES[usage.agent]} signed out</span>
      {/* Not disabled while waiting: a browser tab closed too soon needs a new login. */}
      <Button size="xs" variant="primary" onClick={onSignIn} title={waiting ? "Waiting for the browser: click to start again" : undefined}>
        {waiting ? "Waiting…" : "Sign in"}
      </Button>
    </div>
  );
}

function AgentRow({ usage, now, broken }: { usage: AgentUsage; now: number; broken: number }) {
  return (
    <div className="flex items-center gap-2">
      <AgentIcon agent={usage.agent} size={12} className={cx("shrink-0", usage.error && "opacity-50")} />
      {COLUMNS.map((label) => {
        const w = usage.windows.find((candidate) => candidate.label === label);
        return w ? <Window key={label} window={w} now={now} /> : <span key={label} className="flex-1" />;
      })}
      {broken > 0 && (
        <span
          className="flex shrink-0 items-center gap-0.5 text-[10.5px] tabular-nums text-attention"
          title={`${broken} MCP server${broken === 1 ? "" : "s"} disconnected`}
          data-testid="usage-connectors"
        >
          <Plug size={10} />
          {broken}
        </span>
      )}
    </div>
  );
}

const TONE: Record<Tone, string> = { ok: "bg-success/80", warn: "bg-attention", danger: "bg-danger" };
const TONE_TEXT: Record<Tone, string> = { ok: "text-muted", warn: "text-attention", danger: "text-danger" };

function Window({ window, now }: { window: UsageWindow; now: number }) {
  const { tone } = pace(window, now);
  const used = Math.max(0, Math.min(100, window.used_percent));
  return (
    <span className="flex min-w-0 flex-1 items-center gap-1.5 text-[10.5px] tabular-nums">
      <span className="shrink-0 text-faint">{window.label.split(" ")[0]}</span>
      <span className="h-1 min-w-6 flex-1 overflow-hidden rounded-full bg-line">
        <span className={cx("block h-full rounded-full", TONE[tone])} style={{ width: `${used}%` }} />
      </span>
      <span className={cx("shrink-0", TONE_TEXT[tone])}>{Math.round(used)}%</span>
    </span>
  );
}

function UsagePanel({
  panelRef,
  anchor,
  agents,
  connectors,
  signingIn,
  onSignIn,
  onOpenChat,
  reading,
  onRefresh,
  onMouseEnter,
  onMouseLeave,
  onBlur,
}: {
  panelRef: React.RefObject<HTMLDivElement | null>;
  anchor: React.RefObject<HTMLElement | null>;
  agents: AgentUsage[];
  connectors: Record<AgentKind, BrokenConnector[]>;
  signingIn: AgentKind[];
  onSignIn: (agent: AgentKind) => void;
  onOpenChat: (chat: ChatInfo) => void;
  reading: boolean;
  onRefresh: () => void;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
  onBlur: (event: React.FocusEvent) => void;
}) {
  const position = usePosition(anchor);
  // The times in words move while it stays open.
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(timer);
  }, []);
  useEffect(() => setNow(Date.now()), [agents]);

  return createPortal(
    <div
      ref={panelRef}
      data-popover
      role="dialog"
      aria-label="Plan usage"
      data-testid="usage-panel"
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      onBlur={onBlur}
      className="fixed z-[60] flex flex-col overflow-y-auto rounded-xl border border-line-strong bg-sidebar p-1.5 text-[12px] shadow-2xl shadow-black/70"
      style={{ ...position, width: PANEL_WIDTH }}
    >
      <div className="flex flex-col divide-y divide-line rounded-lg border border-line bg-raised">
        {agents.map((usage) => (
          <AgentSection
            key={usage.agent}
            usage={usage}
            now={now}
            reading={reading}
            onRefresh={onRefresh}
            connectors={connectors[usage.agent]}
            waiting={signingIn.includes(usage.agent)}
            onSignIn={() => onSignIn(usage.agent)}
            onOpenChat={onOpenChat}
          />
        ))}
      </div>
    </div>,
    document.body,
  );
}

function AgentSection({
  usage,
  now,
  reading,
  onRefresh,
  connectors,
  waiting,
  onSignIn,
  onOpenChat,
}: {
  usage: AgentUsage;
  now: number;
  reading: boolean;
  onRefresh: () => void;
  connectors: BrokenConnector[];
  waiting: boolean;
  onSignIn: () => void;
  onOpenChat: (chat: ChatInfo) => void;
}) {
  const signedOut = usage.account === "signed_out";
  const labels = usage.windows.map((w) => w.label);
  const failure = failureText(usage, now);
  const readAt = usage.error ? null : readAtText(usage.read_at, now);
  const extra = extraUsageText(usage.extra_usage);
  const projections = usage.windows.flatMap((w) => {
    const text = projectionText(w, windowName(w.label, labels), now);
    return text ? [{ label: w.label, text, tone: pace(w, now).tone }] : [];
  });
  return (
    <section className="flex flex-col gap-2 px-3 py-2.5" data-agent={usage.agent}>
      <header className="flex items-center gap-2">
        <AgentIcon agent={usage.agent} size={13} className="shrink-0" />
        <span className="truncate">
          <span className="font-medium text-fg">{AGENT_NAMES[usage.agent]}</span>
          {usage.plan && <span className="text-muted"> · {planName(usage.plan)}</span>}
        </span>
        {readAt && <span className="ml-auto shrink-0 text-[11px] text-faint">{readAt}</span>}
        <button
          type="button"
          title="Read the usage again"
          aria-label="Read the usage again"
          onClick={onRefresh}
          className={cx("-mr-1 rounded p-1 text-faint hover:bg-hover hover:text-fg", !readAt && "ml-auto")}
        >
          <RefreshCw size={11} className={cx(reading && "animate-spin")} />
        </button>
      </header>
      {signedOut && (
        <div className="flex flex-col gap-1.5 rounded-md bg-danger/10 px-2 py-1.5" data-testid="usage-account">
          <span className="flex items-start gap-1.5 text-[11.5px] text-danger">
            <AlertTriangle size={12} className="mt-px shrink-0" />
            Signed out: its chats cannot run until you sign in again.
          </span>
          {usage.account_error && (
            <span className="line-clamp-2 break-words pl-[18px] text-[10.5px] text-faint" title={usage.account_error}>
              {usage.account_error}
            </span>
          )}
          <span className="flex items-center gap-2 pl-[18px]">
            <Button size="sm" variant="primary" onClick={onSignIn}>
              {waiting ? "Start again" : `Sign in to ${AGENT_NAMES[usage.agent]}`}
            </Button>
            {waiting && <span className="text-[11px] text-faint">Waiting for the browser…</span>}
          </span>
        </div>
      )}
      {connectors.length > 0 && (
        <div className="flex flex-col gap-1 rounded-md bg-attention/10 px-2 py-1.5" data-testid="usage-mcp">
          <span className="flex items-center gap-1.5 text-[11.5px] text-attention">
            <Plug size={12} className="shrink-0" />
            {connectors.length === 1 ? "An MCP server is disconnected" : `${connectors.length} MCP servers are disconnected`}
          </span>
          {connectors.map(({ server, chat }) => (
            <Fragment key={server.name}>
              <span className="flex min-w-0 items-baseline gap-1.5 pl-[18px] text-[11px]">
                <span className="shrink-0 font-medium text-fg">{server.name}</span>
                <span className="min-w-0 flex-1 truncate text-faint" title={mcpStatusText(server)}>
                  {mcpStatusText(server)}
                </span>
                {/* A chat title can be a whole sentence: it is cut so the row stays inside the panel. */}
                <button
                  type="button"
                  className="min-w-0 max-w-[60%] truncate text-accent hover:underline"
                  title={chat.title ? `Open “${chat.title}” to connect it again` : "Open the chat to connect it again"}
                  onClick={() => onOpenChat(chat)}
                >
                  {chat.title ? `In ${chat.title}` : "Open the chat"}
                </button>
              </span>
              {lacksEnv(server) && (
                <div className="pb-1 pl-[18px]">
                  <MissingEnvHelp server={server} />
                </div>
              )}
            </Fragment>
          ))}
        </div>
      )}
      {failure && !signedOut && (
        <div className="flex flex-col gap-0.5 rounded-md bg-attention/10 px-2 py-1.5" data-testid="usage-error">
          <span className="flex items-start gap-1.5 text-[11.5px] text-attention">
            <AlertTriangle size={12} className="mt-px shrink-0" />
            {failure}
          </span>
          <span className="line-clamp-3 break-words pl-[18px] text-[10.5px] text-faint" title={usage.error ?? undefined}>
            {usage.error}
          </span>
        </div>
      )}
      <div className="grid grid-cols-[56px_1fr_32px_140px] items-center gap-x-2.5 gap-y-1.5 tabular-nums">
        {usage.windows.map((w) => (
          <WindowLine key={w.label} window={w} name={windowName(w.label, labels)} now={now} />
        ))}
      </div>
      {projections.map((p) => (
        <span key={p.label} className={cx("text-[11.5px]", TONE_TEXT[p.tone])} data-testid="usage-projection">
          {p.text}
        </span>
      ))}
      {!signedOut && usage.reset_credits && (
        <div className="flex flex-col gap-1.5 border-t border-line pt-2" data-testid="usage-reset-credits">
          <span className="font-medium text-fg">
            Usage resets · {usage.reset_credits.available_count} available
          </span>
          {usage.reset_credits.credits?.map((credit, index) => (
            <div key={index} className="flex flex-col gap-0.5 text-[11.5px]">
              <span className="text-muted">{credit.title}</span>
              {credit.description && <span className="text-faint">{credit.description}</span>}
              <span className="text-faint">
                {credit.expires_at
                  ? `Expires ${new Date(credit.expires_at).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}`
                  : "Does not expire"}
              </span>
            </div>
          ))}
          {usage.reset_credits.available_count > 0 && usage.reset_credits.credits == null && (
            <span className="text-[11px] text-faint">Reset details unavailable</span>
          )}
        </div>
      )}
      {extra && <span className="text-[11.5px] text-muted">{extra}</span>}
    </section>
  );
}

function WindowLine({ window, name, now }: { window: UsageWindow; name: string; now: number }) {
  const { tone, target } = pace(window, now);
  const used = Math.max(0, Math.min(100, window.used_percent));
  return (
    <>
      <span className="truncate text-muted">{name}</span>
      <span
        className="relative h-1.5 rounded-full bg-line"
        title={target != null ? `${Math.round(target)}% of the window has gone by` : undefined}
      >
        <span className={cx("absolute inset-y-0 left-0 rounded-full", TONE[tone])} style={{ width: `${used}%` }} />
        {/* The pace: the share of the window gone by. Used beyond it is ahead of pace. */}
        {target != null && (
          <span
            className="absolute -top-[3px] h-[12px] w-[2px] -translate-x-1/2 rounded-full bg-fg/80 shadow-[0_0_0_1px_var(--color-raised)]"
            style={{ left: `${target}%` }}
            data-testid="usage-pace"
          />
        )}
      </span>
      <span className={cx("text-right", TONE_TEXT[tone])}>{Math.round(window.used_percent)}%</span>
      <span className="text-right text-[11px] text-faint">{resetText(window.resets_at, now) ?? ""}</span>
    </>
  );
}

/** To the right of the sidebar's foot, its bottom on the row's; above the row when there is no room. */
function usePosition(anchor: React.RefObject<HTMLElement | null>): React.CSSProperties {
  const [style, setStyle] = useState<React.CSSProperties>({ visibility: "hidden" });
  useLayoutEffect(() => {
    const place = () => {
      const rect = anchor.current?.getBoundingClientRect();
      if (!rect) return;
      const right = rect.right + 8;
      setStyle(
        right + PANEL_WIDTH <= window.innerWidth - 8
          ? { left: right, bottom: Math.max(8, window.innerHeight - rect.bottom), maxHeight: window.innerHeight - 16 }
          : {
              left: Math.max(8, Math.min(rect.left, window.innerWidth - PANEL_WIDTH - 8)),
              bottom: window.innerHeight - rect.top + 6,
              maxHeight: rect.top - 14,
            },
      );
    };
    place();
    window.addEventListener("resize", place);
    return () => window.removeEventListener("resize", place);
  }, [anchor]);
  return style;
}
