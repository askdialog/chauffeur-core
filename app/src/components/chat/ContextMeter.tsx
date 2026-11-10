// How full the chat's context window is, after the cost in the composer's footer: a ring that
// fills with use, a tick where the chat compacts (Jev's threshold, else Claude Code's autocompact),
// amber within 10 points of it, red past it. Hovering opens a panel above it (as the model menu
// and the sidebar's usage panel do), a click pins it until a click elsewhere or Escape: the
// breakdown by category as Claude Code's `/context` gives it, then what the categories hold (MCP
// tools by server, memory files, skills, agents), counted exactly on opening (`chat.context_usage`
// with `full`). A chat whose session is closed shows its last reading, and says so; the threshold
// marked is the compaction that applies now (Jev turned on or off since).

import type { AgentKind, ContextUsage, ContextUsageResult } from "@chauffeur/protocol";
import { ChevronRight } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  barSegments,
  compactionText,
  contextRows,
  contextTone,
  type ContextRow,
  type ContextTone,
  formatTokenCount,
  freshestReading,
  mcpServers,
  type Reading,
  readAtText,
  ringFill,
  shortPath,
  usageHeadline,
  usagePercent,
} from "@/lib/context";
import { client } from "@/state/app";
import { cx, Spinner, usePopoverPosition } from "../ui";

const OPEN_DELAY_MS = 250;
/** Long enough to move the pointer from the ring to the panel. */
const CLOSE_DELAY_MS = 200;
const PANEL_WIDTH = 340;

const TONE_TEXT: Record<ContextTone, string> = { neutral: "text-faint", warn: "text-attention", danger: "text-danger" };
const TONE_STROKE: Record<ContextTone, string> = { neutral: "var(--color-muted)", warn: "var(--color-attention)", danger: "var(--color-danger)" };

export function ContextMeter({
  chatId,
  agent,
  reading,
  workspacePath,
}: {
  chatId: string;
  agent: AgentKind;
  /** The chat's last reading, from its events. */
  reading: { usage: ContextUsage; readAt: string };
  workspacePath: string | null;
}) {
  const [open, setOpen] = useState(false);
  const [pinned, setPinned] = useState(false);
  const area = useRef<HTMLDivElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const hovered = useRef(false);

  const schedule = useCallback((next: boolean, delay: number) => {
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setOpen(next), delay);
  }, []);
  useEffect(() => () => clearTimeout(timer.current), []);
  const close = useCallback(() => {
    clearTimeout(timer.current);
    setPinned(false);
    setOpen(false);
  }, []);

  const enter = () => {
    hovered.current = true;
    schedule(true, open ? 0 : OPEN_DELAY_MS);
  };
  const leave = () => {
    hovered.current = false;
    if (!pinned) schedule(false, CLOSE_DELAY_MS);
  };

  // Escape closes it, and does not reach the chat (where it would interrupt the agent); a click
  // elsewhere closes it pinned.
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      close();
    };
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (area.current?.contains(target) || panel.current?.contains(target)) return;
      close();
    };
    window.addEventListener("keydown", onKey, true);
    document.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      document.removeEventListener("mousedown", onDown);
    };
  }, [open, close]);

  // The daemon's reading after each turn (the compaction that applies now, whether the session
  // runs), and the exact breakdown on opening and after each turn while open.
  const [latest, setLatest] = useState<ContextUsageResult | null>(null);
  useEffect(() => {
    let cancelled = false;
    client
      .call("chat.context_usage", { chat: chatId, detail: "summary" })
      .then((result) => {
        if (!cancelled) setLatest(result);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [chatId, reading.readAt]);
  const [full, setFull] = useState<ContextUsageResult | null>(null);
  const [loading, setLoading] = useState(false);
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    client
      .call("chat.context_usage", { chat: chatId, detail: "full" })
      .then((result) => {
        if (!cancelled) setFull(result);
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, chatId, reading.readAt]);

  const shown = freshestReading(reading, latest, full);
  const usage = shown.usage;
  const tone = contextTone(usage);
  const label = `Context: ${usageHeadline(usage)}`;
  return (
    <div ref={area} onMouseEnter={enter} onMouseLeave={leave} className="flex">
      <button
        type="button"
        aria-label={label}
        aria-expanded={open}
        aria-haspopup="dialog"
        data-testid="context-meter"
        onClick={() => {
          clearTimeout(timer.current);
          if (pinned) close();
          else {
            setPinned(true);
            setOpen(true);
          }
        }}
        onFocus={(event) => {
          if (event.currentTarget.matches(":focus-visible")) schedule(true, 0);
        }}
        className={cx(
          "inline-flex h-6 items-center gap-1 rounded-md px-1.5 text-[11px] tabular-nums transition-colors",
          open ? "bg-hover" : "hover:bg-hover",
          TONE_TEXT[tone],
        )}
      >
        <ContextRing usage={usage} tone={tone} size={13} />
        <span>{usagePercent(usage)}</span>
      </button>
      {open && (
        <ContextPanel
          panelRef={panel}
          anchor={area}
          agent={agent}
          reading={shown}
          loading={loading}
          workspacePath={workspacePath}
          onMouseEnter={enter}
          onMouseLeave={leave}
        />
      )}
    </div>
  );
}

/** A ring filling clockwise from the top, with a tick where the chat compacts. */
export function ContextRing({ usage, tone, size }: { usage: ContextUsage; tone: ContextTone; size: number }) {
  const stroke = 2;
  const r = (size - stroke) / 2;
  const c = size / 2;
  const length = 2 * Math.PI * r;
  const fill = ringFill(usage.percent);
  const threshold = usage.compaction?.threshold_percent;
  const angle = threshold == null ? null : (Math.min(100, Math.max(0, threshold)) / 100) * 2 * Math.PI - Math.PI / 2;
  return (
    <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} aria-hidden className="shrink-0" data-testid="context-ring">
      <circle cx={c} cy={c} r={r} fill="none" stroke="var(--color-line-strong)" strokeWidth={stroke} />
      {fill > 0 && (
        <circle
          cx={c}
          cy={c}
          r={r}
          fill="none"
          stroke={TONE_STROKE[tone]}
          strokeWidth={stroke}
          strokeDasharray={`${Math.max(fill * length, 0.6)} ${length}`}
          transform={`rotate(-90 ${c} ${c})`}
          strokeLinecap="butt"
        />
      )}
      {angle != null && (
        <line
          x1={c + (r - stroke) * Math.cos(angle)}
          y1={c + (r - stroke) * Math.sin(angle)}
          x2={c + (r + stroke) * Math.cos(angle)}
          y2={c + (r + stroke) * Math.sin(angle)}
          stroke="var(--color-fg)"
          strokeWidth={1}
          data-testid="context-threshold"
        />
      )}
    </svg>
  );
}

function ContextPanel({
  panelRef,
  anchor,
  agent,
  reading,
  loading,
  workspacePath,
  onMouseEnter,
  onMouseLeave,
}: {
  panelRef: React.RefObject<HTMLDivElement | null>;
  anchor: React.RefObject<HTMLElement | null>;
  agent: AgentKind;
  reading: Reading;
  loading: boolean;
  workspacePath: string | null;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
}) {
  const position = usePopoverPosition(anchor, PANEL_WIDTH, 460);
  const { usage, readAt, live } = reading;
  const tone = contextTone(usage);
  const { window: rows, deferred } = contextRows(usage);
  const compaction = compactionText(usage);
  const details = usage.details;
  return createPortal(
    <div
      ref={panelRef}
      data-popover
      role="dialog"
      aria-label="Context"
      data-testid="context-panel"
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      className="fixed z-[60] flex flex-col overflow-hidden rounded-xl border border-line-strong bg-sidebar p-1.5 text-[12px] shadow-2xl shadow-black/70"
      style={{ ...position, width: PANEL_WIDTH }}
    >
      <div className="flex min-h-0 flex-col gap-2.5 overflow-y-auto rounded-lg border border-line bg-raised px-3 py-2.5">
        <header className="flex flex-col gap-0.5">
          <div className="flex items-center gap-2">
            <span className={cx("font-medium tabular-nums", tone === "neutral" ? "text-fg" : TONE_TEXT[tone])} data-testid="context-headline">
              {usageHeadline(usage)}
            </span>
            {loading && <Spinner size={11} className="ml-auto text-faint" />}
          </div>
          {(usage.model || compaction) && (
            <span className="truncate text-[11px] text-faint">{[usage.model, compaction].filter(Boolean).join(" · ")}</span>
          )}
          {usage.over_limit && <span className="text-[11px] text-danger">Over the window: the next turn compacts or fails</span>}
          {usage.estimated && (
            <span className="text-[11px] text-muted" data-testid="context-estimated">
              Estimated after the compaction: measured at the agent's next response
            </span>
          )}
        </header>
        <StackedBar usage={usage} />
        {rows.length > 0 ? (
          <div className="flex flex-col gap-0.5" data-testid="context-rows">
            {rows.map((row) => (
              <CategoryRow key={row.name} row={row} />
            ))}
            {deferred.length > 0 && <div className="mt-1.5 border-t border-line pt-1.5" />}
            {deferred.map((row) => (
              <CategoryRow key={row.name} row={row} />
            ))}
          </div>
        ) : (
          agent === "codex" && <span className="text-[11.5px] text-muted">Codex gives no breakdown, only the total against its window.</span>
        )}
        {details && (
          <div className="flex flex-col gap-0.5 border-t border-line pt-1.5" data-testid="context-details">
            <McpSection tools={details.mcp_tools} />
            <Section
              title="Memory files"
              count={details.memory_files.length}
              items={details.memory_files.map((file) => ({
                key: file.path,
                name: shortPath(file.path, workspacePath),
                hint: file.source,
                tokens: file.tokens,
                title: file.path,
              }))}
            />
            <Section
              title="Skills"
              count={details.skills.length}
              items={[...details.skills]
                .sort((a, b) => b.tokens - a.tokens || a.name.localeCompare(b.name))
                .map((skill) => ({ key: `${skill.source}:${skill.name}`, name: skill.name, hint: sourceName(skill.source), tokens: skill.tokens }))}
            />
            <Section
              title="Agents"
              count={details.agents.length}
              items={details.agents.map((a) => ({ key: `${a.source}:${a.name}`, name: a.name, hint: sourceName(a.source), tokens: a.tokens }))}
            />
          </div>
        )}
        <footer className="flex items-center gap-1.5 text-[11px] text-faint" data-testid="context-footer">
          <span>{readAtText(readAt) ?? ""}</span>
          {agent === "codex" ? (
            <span>· Codex gives no breakdown</span>
          ) : (
            live === false && <span className="text-muted">· last reading; the session is idle</span>
          )}
        </footer>
      </div>
    </div>,
    document.body,
  );
}

function StackedBar({ usage }: { usage: ContextUsage }) {
  const segments = barSegments(usage);
  const threshold = usage.compaction?.threshold_percent;
  const fallback = segments.length === 0 ? Math.min(100, usage.percent) : 0;
  return (
    <div className="relative h-1.5 rounded-full bg-line" data-testid="context-bar">
      <div className="absolute inset-0 flex overflow-hidden rounded-full">
        {segments.map((segment) => (
          <span key={segment.name} title={segment.name} className="h-full shrink-0" style={{ width: `${segment.width}%`, background: segment.color }} />
        ))}
        {fallback > 0 && <span className="h-full bg-muted" style={{ width: `${fallback}%` }} />}
      </div>
      {threshold != null && (
        <span
          className="absolute -top-[3px] h-[12px] w-[2px] -translate-x-1/2 rounded-full bg-fg/80 shadow-[0_0_0_1px_var(--color-raised)]"
          style={{ left: `${Math.min(100, threshold)}%` }}
          title={compactionText(usage) ?? undefined}
          data-testid="context-bar-threshold"
        />
      )}
    </div>
  );
}

function CategoryRow({ row }: { row: ContextRow }) {
  const dim = row.kind !== "used";
  return (
    <div className={cx("grid grid-cols-[10px_1fr_auto_46px] items-center gap-x-2 tabular-nums", dim ? "text-faint" : "text-muted")} data-kind={row.kind}>
      <span
        className={cx("h-2 w-2 rounded-[2px]", row.kind === "free" && "border border-line-strong")}
        style={{ background: row.kind === "free" ? "transparent" : row.color }}
      />
      <span className={cx("truncate", !dim && "text-fg/90")}>{row.name}</span>
      <span className="text-right">{formatTokenCount(row.tokens)}</span>
      <span className="text-right">{row.share ?? "—"}</span>
    </div>
  );
}

type SectionItem = { key: string; name: string; hint?: string; tokens: number; title?: string };

function Section({ title, count, items }: { title: string; count: number; items: SectionItem[] }) {
  const [open, setOpen] = useState(false);
  if (count === 0) return null;
  return (
    <div className="flex flex-col">
      <Disclosure open={open} onToggle={() => setOpen(!open)} title={title} right={String(count)} />
      {open && (
        <div className="mb-1 flex flex-col gap-0.5 pl-4">
          {items.map((item) => (
            <ItemRow key={item.key} name={item.name} hint={item.hint} tokens={item.tokens} title={item.title} />
          ))}
        </div>
      )}
    </div>
  );
}

function McpSection({ tools }: { tools: NonNullable<ContextUsage["details"]>["mcp_tools"] }) {
  const [open, setOpen] = useState(false);
  const [servers, setServers] = useState<ReadonlySet<string>>(() => new Set());
  if (tools.length === 0) return null;
  const groups = mcpServers(tools);
  return (
    <div className="flex flex-col" data-testid="context-mcp">
      <Disclosure open={open} onToggle={() => setOpen(!open)} title="MCP tools" right={String(tools.length)} />
      {open && (
        <div className="mb-1 flex flex-col gap-0.5 pl-4">
          {groups.map((group) => {
            const shown = servers.has(group.server);
            return (
              <div key={group.server} className="flex flex-col">
                <Disclosure
                  open={shown}
                  onToggle={() =>
                    setServers((current) => {
                      const next = new Set(current);
                      if (!next.delete(group.server)) next.add(group.server);
                      return next;
                    })
                  }
                  title={group.server}
                  hint={`${group.tools.length} ${group.tools.length === 1 ? "tool" : "tools"}${group.loaded === 0 ? ", deferred" : ""}`}
                  right={formatTokenCount(group.tokens)}
                />
                {shown && (
                  <div className="mb-0.5 flex flex-col gap-0.5 pl-4">
                    {group.tools.map((tool) => (
                      <ItemRow
                        key={tool.name}
                        name={tool.name}
                        // Said once on the server when all its tools are deferred.
                        hint={tool.loaded || group.loaded === 0 ? undefined : "deferred"}
                        tokens={tool.tokens}
                      />
                    ))}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function Disclosure({ open, onToggle, title, hint, right }: { open: boolean; onToggle: () => void; title: string; hint?: string; right: string }) {
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-expanded={open}
      className="-mx-1 flex items-center gap-1.5 rounded px-1 py-0.5 text-left text-muted hover:bg-hover hover:text-fg"
    >
      <ChevronRight size={11} className={cx("shrink-0 text-faint transition-transform", open && "rotate-90")} />
      <span className="truncate text-fg/90">{title}</span>
      {hint && <span className="truncate text-[11px] text-faint">{hint}</span>}
      <span className="ml-auto shrink-0 tabular-nums">{right}</span>
    </button>
  );
}

function ItemRow({ name, hint, tokens, title }: { name: string; hint?: string | undefined; tokens: number; title?: string | undefined }) {
  return (
    <div className="flex items-center gap-1.5 text-[11.5px] text-muted" title={title}>
      <span className="selectable truncate">{name}</span>
      {hint && <span className="shrink-0 text-[10.5px] text-faint">{hint}</span>}
      <span className="ml-auto shrink-0 tabular-nums text-faint">{formatTokenCount(tokens)}</span>
    </div>
  );
}

/** Claude Code's raw source names, as a person reads them. */
function sourceName(source: string): string {
  const names: Record<string, string> = {
    projectSettings: "project",
    userSettings: "user",
    localSettings: "local",
    policySettings: "managed",
    plugin: "plugin",
    syncedSkills: "synced",
  };
  return names[source] ?? source;
}
