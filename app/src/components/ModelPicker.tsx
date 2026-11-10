// The model picker: a loadout of favorite settings (⌃⌘1–9), a search over every agent's models,
// effort (⌘⇧/ cycles) and fast mode (⌘⇧E). Modelled on Conductor's.

import type { AgentKind, AgentModel, LoadoutEntry } from "@chauffeur/protocol";
import { Check, ChevronDown, ChevronLeft, ChevronRight, GripVertical, Plus, RefreshCw, Search, Settings2, Star, X, Zap } from "lucide-react";
import { type ReactNode, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  AGENT_NAMES,
  cycleEffort,
  effectiveLoadout,
  loadoutIn,
  effortLabel,
  findModel,
  fromEntry,
  isEntry,
  type ModelChoice,
  modelName,
  moveEntry,
  searchModels,
  shownEffort,
  toEntry,
  withModel,
} from "@/lib/models";
import { saveLoadout, useApp } from "@/state/app";
import { useCatalog, useCatalogLoader, useRemoteCatalog } from "@/state/catalog";
import { AgentIcon } from "./AgentIcon";
import { cx, Toggle, usePopoverPosition } from "./ui";
import { digitOf, kbd } from "../lib/keys";
import { pressed } from "../lib/shortcuts";

const WIDTH = 330;

export function ModelPicker({
  value,
  onChange,
  lockedAgent = null,
  lockReason,
  shortcuts = false,
  disabled = false,
  field = false,
}: {
  value: ModelChoice;
  onChange: (choice: ModelChoice) => void;
  /** Shown as a form field (the settings) rather than the composer's compact button. */
  field?: boolean;
  /** Only this agent's models can be picked (a busy chat). */
  lockedAgent?: AgentKind | null;
  /** Why the other agents' models cannot be picked; by default, a new chat would take them. */
  lockReason?: string;
  /** Answers ⌃⌘1–9, ⌘⇧/ and ⌘⇧E unless a dialog it is not part of is open. */
  shortcuts?: boolean;
  disabled?: boolean;
}) {
  const catalog = useCatalog();
  const loader = useCatalogLoader();
  const saved = useApp((s) => s.loadout);
  // On another machine, only the favorites its agents have.
  const remote = useRemoteCatalog();
  const loadout = useMemo(
    () => (remote ? loadoutIn(saved, catalog) : effectiveLoadout(saved, catalog)),
    [remote, saved, catalog],
  );
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const model = findModel(catalog, value.agent, value.model);
  const effort = shownEffort(catalog, value);

  const allowed = (agent: AgentKind) => lockedAgent == null || lockedAgent === agent;
  const pickEntry = (entry: LoadoutEntry) => {
    if (allowed(entry.agent)) onChange(fromEntry(entry));
  };
  const toggleFast = () => {
    if (model?.supports_fast) onChange({ ...value, fast: !value.fast });
  };

  // Keyboard shortcuts, as long as no other dialog covers this picker.
  const latest = useRef({ loadout, value, catalog, pickEntry, toggleFast, onChange });
  latest.current = { loadout, value, catalog, pickEntry, toggleFast, onChange };
  useEffect(() => {
    if (!shortcuts || disabled) return;
    const onKey = (event: KeyboardEvent) => {
      const pick = pressed(event, "model.pick");
      if (!pick && !pressed(event, "model.effort") && !pressed(event, "model.fast")) return;
      const dialog = document.querySelector("[role=dialog]");
      if (dialog && trigger.current && !dialog.contains(trigger.current)) return;
      const { loadout, value, catalog, pickEntry, toggleFast, onChange } = latest.current;
      if (pick) {
        const entry = loadout[(digitOf(event) ?? 0) - 1];
        if (!entry) return;
        event.preventDefault();
        pickEntry(entry);
      } else if (pressed(event, "model.effort")) {
        event.preventDefault();
        onChange(cycleEffort(catalog, value));
      } else {
        event.preventDefault();
        toggleFast();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [shortcuts, disabled]);

  return (
    <>
      <button
        ref={trigger}
        type="button"
        disabled={disabled}
        onClick={() => {
          setOpen((o) => !o);
          if (catalog == null) loader.load(false);
        }}
        title="Model, effort and speed"
        data-testid="model-picker"
        className={cx(
          "inline-flex items-center gap-1.5 rounded-md transition-colors disabled:opacity-50",
          field
            ? "h-8 w-full border border-line bg-bg px-2.5 text-[12.5px] hover:border-line-strong"
            : "h-6 max-w-[280px] px-1.5 text-[12px]",
          open ? "bg-hover text-fg" : "text-muted hover:bg-hover hover:text-fg",
        )}
      >
        <AgentIcon agent={value.agent} size={13} />
        <span className="truncate text-fg">{modelName(catalog, value)}</span>
        {effort && <span className="shrink-0 text-faint">{effortLabel(effort)}</span>}
        {value.fast && <Zap size={11} className="shrink-0 text-accent" fill="currentColor" />}
        <ChevronDown size={12} className={cx("shrink-0 text-faint", field && "ml-auto")} />
      </button>
      {open && (
        <PickerPopover
          anchor={trigger}
          value={value}
          model={model}
          loadout={loadout}
          allowed={allowed}
          lockReason={lockReason}
          onPickEntry={(entry) => {
            pickEntry(entry);
            setOpen(false);
          }}
          onPickModel={(agent, picked) => {
            onChange(withModel(value, agent, picked));
            setOpen(false);
          }}
          onChange={onChange}
          onToggleFast={toggleFast}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}

type Row =
  | { kind: "entry"; index: number; entry: LoadoutEntry }
  | { kind: "browse" }
  | { kind: "add" }
  | { kind: "hit"; agent: AgentKind; model: AgentModel };

function PickerPopover({
  anchor,
  value,
  model,
  loadout,
  allowed,
  lockReason,
  onPickEntry,
  onPickModel,
  onChange,
  onToggleFast,
  onClose,
}: {
  anchor: React.RefObject<HTMLButtonElement | null>;
  value: ModelChoice;
  model: AgentModel | null;
  loadout: LoadoutEntry[];
  allowed: (agent: AgentKind) => boolean;
  lockReason?: string;
  onPickEntry: (entry: LoadoutEntry) => void;
  onPickModel: (agent: AgentKind, model: AgentModel) => void;
  onChange: (choice: ModelChoice) => void;
  onToggleFast: () => void;
  onClose: () => void;
}) {
  const catalog = useCatalog();
  const loader = useCatalogLoader();
  const modelsLoading = loader.loading;
  // Inside a moved workspace the loadout shown is the favorites that machine runs: saving it
  // would drop the others, so the favorites are edited outside it.
  const editable = !useRemoteCatalog();
  const persist = (next: LoadoutEntry[]) => {
    if (editable) void saveLoadout(next);
  };
  const [query, setQuery] = useState("");
  const [browsing, setBrowsing] = useState(false);
  const [editing, setEditing] = useState(false);
  const [effortOpen, setEffortOpen] = useState(false);
  // Opens on the loadout entry in use.
  const [highlight, setHighlight] = useState(() => Math.max(0, loadout.findIndex((entry) => isEntry(catalog, entry, value))));
  const [drag, setDrag] = useState<{ from: number; over: number } | null>(null);
  const panel = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const rowRefs = useRef<Array<HTMLDivElement | null>>([]);
  const effortRow = useRef<HTMLDivElement>(null);
  const position = usePopoverPosition(anchor, WIDTH, 380);

  const searching = browsing || query.trim() !== "";
  const hits = useMemo(() => (searching ? searchModels(catalog, query) : []), [searching, catalog, query]);
  const current = toEntry(catalog, value);
  const inLoadout = loadout.some((entry) => isEntry(catalog, entry, value));
  const shown = drag ? moveEntry(loadout, drag.from, drag.over) : loadout;

  const rows: Row[] = searching
    ? hits.map((hit) => ({ kind: "hit", ...hit }))
    : [
        ...shown.map((entry, index) => ({ kind: "entry" as const, index, entry })),
        ...(editing && current && !inLoadout ? [{ kind: "add" as const }] : []),
        { kind: "browse" as const },
      ];
  const selectable = (row: Row) =>
    row.kind === "entry" ? allowed(row.entry.agent) : row.kind === "hit" ? allowed(row.agent) : true;

  const firstRender = useRef(true);
  useEffect(() => {
    if (firstRender.current) firstRender.current = false;
    else setHighlight(0);
  }, [query, browsing]);
  useEffect(() => setEffortOpen(false), [searching]);
  // Once placed: an element still hidden cannot take the focus.
  useEffect(() => {
    const frame = requestAnimationFrame(() => search.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, []);

  // Clicks outside close the picker (the trigger toggles it itself).
  useEffect(() => {
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (panel.current?.contains(target) || anchor.current?.contains(target)) return;
      if ((target as Element).closest?.("[data-popover]")) return;
      onClose();
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [anchor, onClose]);

  const activate = (row: Row | undefined) => {
    if (!row || !selectable(row)) return;
    if (row.kind === "entry") onPickEntry(row.entry);
    else if (row.kind === "hit") onPickModel(row.agent, row.model);
    else if (row.kind === "browse") setBrowsing(true);
    else if (row.kind === "add" && current) persist([...loadout, current]);
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (rows.length === 0) return;
      const step = event.key === "ArrowDown" ? 1 : -1;
      setHighlight((h) => (h + step + rows.length) % rows.length);
    } else if (event.key === "Enter") {
      event.preventDefault();
      activate(rows[highlight]);
    }
  };

  // Escape steps back (query, then all models, then closed) wherever the focus is; the Modal
  // around the picker leaves Escape to it, and a chat does not take it as an interrupt.
  const escape = useRef(() => {});
  escape.current = () => {
    if (effortOpen) setEffortOpen(false);
    else if (query) setQuery("");
    else if (browsing) setBrowsing(false);
    else onClose();
  };
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      escape.current();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  const removeEntry = (index: number) => persist(loadout.filter((_, i) => i !== index));
  const toggleStar = (agent: AgentKind, hit: AgentModel) => {
    const has = loadout.some((e) => e.agent === agent && findModel(catalog, agent, e.model)?.id === hit.id);
    if (has) {
      persist(loadout.filter((e) => !(e.agent === agent && findModel(catalog, agent, e.model)?.id === hit.id)));
    } else {
      const entry = toEntry(catalog, withModel(value, agent, hit));
      if (entry) persist([...loadout, { ...entry, fast: false }]);
    }
  };

  // Drag a loadout row by its grip; the list previews the new order until the drop.
  const startDrag = (index: number, event: React.PointerEvent) => {
    event.preventDefault();
    (event.target as Element).setPointerCapture(event.pointerId);
    setDrag({ from: index, over: index });
  };
  const moveDrag = (event: React.PointerEvent) => {
    if (!drag) return;
    const tops = loadout.map((_, i) => rowRefs.current[i]?.getBoundingClientRect());
    let over = 0;
    tops.forEach((rect, i) => {
      if (rect && event.clientY > rect.top + rect.height / 2) over = i;
    });
    if (tops[0] && event.clientY < tops[0].top + tops[0].height / 2) over = 0;
    if (over !== drag.over) setDrag({ ...drag, over });
  };
  const endDrag = () => {
    if (drag && drag.from !== drag.over) persist(moveEntry(loadout, drag.from, drag.over));
    setDrag(null);
  };

  const efforts = model?.efforts ?? [];
  const effort = shownEffort(catalog, value);

  return createPortal(
    <div
      ref={panel}
      data-popover
      aria-label="Choose a model"
      onKeyDown={onKeyDown}
      className="fixed z-[60] flex flex-col overflow-hidden rounded-xl border border-line-strong bg-sidebar p-1.5 text-[13px] shadow-2xl shadow-black/70"
      style={{ ...position, width: WIDTH }}
    >
      <div className="flex min-h-0 flex-col overflow-hidden rounded-lg border border-line bg-raised">
        <div className="flex h-9 shrink-0 items-center gap-2 border-b border-line px-2.5">
          {searching && !query ? (
            <button
              type="button"
              title="Back to the loadout"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => setBrowsing(false)}
              className="text-faint hover:text-fg"
            >
              <ChevronLeft size={14} />
            </button>
          ) : (
            <Search size={13} className="text-faint" />
          )}
          <input
            ref={search}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search models…"
            className="h-full min-w-0 flex-1 bg-transparent text-[12.5px] text-fg placeholder:text-faint focus:outline-none"
            data-testid="model-search"
          />
          {searching && (
            <button
              type="button"
              title="Ask the agents for their models again"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => loader.load(true)}
              className="text-faint hover:text-fg"
            >
              <RefreshCw size={12} className={cx(modelsLoading && "animate-spin")} />
            </button>
          )}
        </div>

        <div
          className="min-h-0 flex-1 overflow-y-auto py-1"
          onPointerMove={moveDrag}
          onPointerUp={endDrag}
          onMouseEnter={() => setEffortOpen(false)}
        >
          {searching ? (
            <SearchResults
              rows={rows}
              highlight={highlight}
              setHighlight={setHighlight}
              selectable={selectable}
              lockReason={lockReason}
              activate={activate}
              value={value}
              loadout={loadout}
              toggleStar={editable ? toggleStar : undefined}
              loading={catalog == null || modelsLoading}
            />
          ) : (
            rows.map((row, i) => {
              if (row.kind === "entry") {
                const entryModel = findModel(catalog, row.entry.agent, row.entry.model);
                const active = isEntry(catalog, row.entry, value);
                const enabled = allowed(row.entry.agent);
                return (
                  <div
                    key={`${row.entry.agent}:${row.entry.model}:${row.entry.effort ?? ""}:${row.entry.fast}:${i}`}
                    ref={(el) => {
                      rowRefs.current[row.index] = el;
                    }}
                    role="option"
                    aria-selected={active}
                    aria-disabled={!enabled}
                    title={enabled ? undefined : (lockReason ?? `Start a new chat to use ${AGENT_NAMES[row.entry.agent]}`)}
                    onMouseDown={(e) => e.preventDefault()}
                    onMouseEnter={() => setHighlight(i)}
                    onClick={() => !drag && activate(row)}
                    className={cx(
                      "group mx-1 flex h-9 items-center gap-2 rounded-md pr-2.5 pl-1",
                      highlight === i && enabled && "bg-hover",
                      drag?.over === row.index && "bg-hover ring-1 ring-line-strong",
                      !enabled && "opacity-40",
                    )}
                  >
                    <span
                      onPointerDown={(e) => startDrag(row.index, e)}
                      onClick={(e) => e.stopPropagation()}
                      className="flex h-6 w-4 cursor-grab items-center justify-center text-faint/60 hover:text-muted"
                      title="Drag to reorder"
                    >
                      <GripVertical size={12} />
                    </span>
                    <AgentIcon agent={row.entry.agent} size={15} />
                    <span className="truncate text-fg">{entryModel?.name ?? row.entry.model}</span>
                    {row.entry.effort && <span className="shrink-0 text-[12px] text-faint">{effortLabel(row.entry.effort)}</span>}
                    {row.entry.fast && <Zap size={11} className="shrink-0 text-faint" fill="currentColor" />}
                    <span className="ml-auto flex shrink-0 items-center">
                      {editing ? (
                        <button
                          type="button"
                          title="Remove from the loadout"
                          onClick={(e) => {
                            e.stopPropagation();
                            removeEntry(row.index);
                          }}
                          className="rounded p-0.5 text-faint hover:bg-line hover:text-fg"
                        >
                          <X size={12} />
                        </button>
                      ) : active ? (
                        <Check size={14} className="text-fg" />
                      ) : row.index < 9 ? (
                        <Shortcut keys={kbd(`⌃⌘${row.index + 1}`)} />
                      ) : null}
                    </span>
                  </div>
                );
              }
              if (row.kind === "add") {
                return (
                  <MenuRow key="add" active={highlight === i} onHover={() => setHighlight(i)} onClick={() => activate(row)}>
                    <Plus size={13} className="ml-5 text-faint" />
                    <span className="text-muted">Add {modelName(catalog, value)}{effort ? ` ${effortLabel(effort)}` : ""}</span>
                  </MenuRow>
                );
              }
              return (
                <MenuRow key="browse" active={highlight === i} onHover={() => setHighlight(i)} onClick={() => activate(row)}>
                  <span className="ml-5 text-muted">All models</span>
                  <ChevronRight size={13} className="ml-auto text-faint" />
                </MenuRow>
              );
            })
          )}
        </div>

        {!searching && (
          <div className="shrink-0 border-t border-line py-1">
            <div ref={effortRow}>
              <MenuRow
                disabled={efforts.length === 0}
                active={effortOpen}
                onHover={() => efforts.length > 0 && setEffortOpen(true)}
                onClick={() => setEffortOpen(true)}
                title={efforts.length === 0 ? `${model?.name ?? "This model"} has no effort levels` : undefined}
              >
                <span className="text-fg">Effort</span>
                <span className="ml-auto text-muted">{effort ? effortLabel(effort) : "Default"}</span>
                <ChevronRight size={13} className="text-faint" />
              </MenuRow>
            </div>
            {effortOpen && efforts.length > 0 && (
              <EffortMenu
                panel={panel}
                row={effortRow}
                efforts={efforts}
                current={effort}
                onPick={(level) => {
                  onChange({ ...value, effort: level });
                  setEffortOpen(false);
                }}
              />
            )}
            <MenuRow
              onHover={() => setEffortOpen(false)}
              disabled={!model?.supports_fast}
              onClick={onToggleFast}
              title={model?.supports_fast ? "Faster responses, more usage" : `${model?.name ?? "This model"} has no fast mode`}
            >
              <span className="text-fg">Fast</span>
              <span className="ml-auto">
                <Shortcut keys={kbd("⌘⇧E")} />
              </span>
              <Toggle on={value.fast && !!model?.supports_fast} />
            </MenuRow>
          </div>
        )}
      </div>

      <div className="flex h-8 shrink-0 items-center px-2 text-[12px] text-muted">
        <button
          type="button"
          hidden={!editable}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            setBrowsing(false);
            setQuery("");
            setEditing((e) => !e);
          }}
          className="inline-flex items-center gap-1.5 rounded px-1 py-0.5 hover:text-fg"
        >
          {editing ? <Check size={13} /> : <Settings2 size={13} />}
          {editing ? "Done" : "Edit"}
        </button>
        <span className="ml-auto flex items-center gap-1.5">
          <span className="rounded border border-line-strong px-1 font-sans text-[11px] text-faint">{kbd("⌘⇧/")}</span>
          Cycle effort
        </span>
      </div>
    </div>,
    document.body,
  );
}

function SearchResults({
  rows,
  highlight,
  setHighlight,
  selectable,
  lockReason,
  activate,
  value,
  loadout,
  toggleStar,
  loading,
}: {
  rows: Row[];
  highlight: number;
  setHighlight: (index: number) => void;
  selectable: (row: Row) => boolean;
  lockReason?: string;
  activate: (row: Row) => void;
  value: ModelChoice;
  loadout: LoadoutEntry[];
  /** None: the favorites are not edited here. */
  toggleStar?: (agent: AgentKind, model: AgentModel) => void;
  loading: boolean;
}) {
  const catalog = useCatalog();
  const errors = (catalog ?? []).filter((c) => c.error);
  if (rows.length === 0) {
    return (
      <div className="px-3 py-4 text-center text-[12px] text-faint">
        {loading ? "Asking the agents for their models…" : "No model matches."}
        {errors.map((c) => (
          <div key={c.agent} className="mt-1 text-danger/80">
            {AGENT_NAMES[c.agent]}: {c.error}
          </div>
        ))}
      </div>
    );
  }
  const current = findModel(catalog, value.agent, value.model);
  let lastAgent: AgentKind | null = null;
  return (
    <>
      {rows.map((row, i) => {
        if (row.kind !== "hit") return null;
        const header =
          row.agent !== lastAgent ? (
            <div key={`h-${row.agent}`} className="flex items-center gap-1.5 px-3 pt-2 pb-1 text-[11px] font-medium text-faint">
              <AgentIcon agent={row.agent} size={11} />
              {AGENT_NAMES[row.agent]}
            </div>
          ) : null;
        lastAgent = row.agent;
        const enabled = selectable(row);
        const starred = loadout.some((e) => e.agent === row.agent && findModel(catalog, row.agent, e.model)?.id === row.model.id);
        const active = value.agent === row.agent && current?.id === row.model.id;
        return (
          <div key={`${row.agent}:${row.model.id}`}>
            {header}
            <div
              role="option"
              aria-selected={active}
              aria-disabled={!enabled}
              title={enabled ? row.model.description : (lockReason ?? `Start a new chat to use ${AGENT_NAMES[row.agent]}`)}
              onMouseDown={(e) => e.preventDefault()}
              onMouseEnter={() => setHighlight(i)}
              onClick={() => activate(row)}
              className={cx(
                "group mx-1 flex items-center gap-2 rounded-md px-2.5 py-1.5",
                highlight === i && enabled && "bg-hover",
                !enabled && "opacity-40",
              )}
            >
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-1.5">
                  <span className="truncate text-fg">{row.model.name}</span>
                  {row.model.is_default && <span className="text-[10.5px] text-faint">default</span>}
                  {row.model.supports_fast && <Zap size={10} className="text-faint" />}
                </div>
                <div className="truncate text-[11.5px] text-faint">{row.model.description}</div>
              </div>
              {active && <Check size={13} className="shrink-0 text-fg" />}
              {toggleStar && (
                <button
                  type="button"
                  title={starred ? "Remove from the loadout" : "Add to the loadout"}
                  onClick={(e) => {
                    e.stopPropagation();
                    toggleStar(row.agent, row.model);
                  }}
                  className={cx(
                    "shrink-0 rounded p-0.5 hover:bg-line",
                    starred ? "text-attention" : "text-faint opacity-0 group-hover:opacity-100",
                  )}
                >
                  <Star size={12} fill={starred ? "currentColor" : "none"} />
                </button>
              )}
            </div>
          </div>
        );
      })}
    </>
  );
}

/** The effort levels, in a menu beside the picker's Effort row. */
function EffortMenu({
  panel,
  row,
  efforts,
  current,
  onPick,
}: {
  panel: React.RefObject<HTMLDivElement | null>;
  row: React.RefObject<HTMLDivElement | null>;
  efforts: string[];
  current: string | null;
  onPick: (effort: string) => void;
}) {
  const [style, setStyle] = useState<React.CSSProperties>({ visibility: "hidden" });
  useLayoutEffect(() => {
    const box = panel.current?.getBoundingClientRect();
    const line = row.current?.getBoundingClientRect();
    if (!box || !line) return;
    const width = 170;
    const height = efforts.length * 32 + 12;
    const left = box.right + 4 + width <= window.innerWidth - 8 ? box.right + 4 : box.left - 4 - width;
    const top = Math.max(8, Math.min(line.top - 6, window.innerHeight - height - 8));
    setStyle({ left, top, width });
  }, [panel, row, efforts.length]);
  return createPortal(
    <div
      data-popover
      className="fixed z-[61] rounded-xl border border-line-strong bg-raised p-1.5 text-[13px] shadow-2xl shadow-black/70"
      style={style}
    >
      {efforts.map((level) => (
        <MenuRow key={level} onClick={() => onPick(level)}>
          <span className={level === current ? "text-fg" : "text-muted"}>{effortLabel(level)}</span>
          {level === current && <Check size={13} className="ml-auto text-fg" />}
        </MenuRow>
      ))}
    </div>,
    document.body,
  );
}

function MenuRow({
  children,
  onClick,
  onHover,
  active,
  disabled,
  title,
}: {
  children: ReactNode;
  onClick: () => void;
  onHover?: () => void;
  active?: boolean;
  disabled?: boolean;
  title?: string | undefined;
}) {
  return (
    <div
      role="button"
      aria-disabled={disabled}
      title={title}
      onMouseDown={(e) => e.preventDefault()}
      onMouseEnter={onHover}
      onClick={() => !disabled && onClick()}
      className={cx(
        "mx-1 flex h-8 items-center gap-2 rounded-md px-2.5",
        disabled ? "opacity-40" : "hover:bg-hover",
        active && !disabled && "bg-hover",
      )}
    >
      {children}
    </div>
  );
}

function Shortcut({ keys }: { keys: string }) {
  return <span className="font-sans text-[11.5px] tracking-wide text-faint">{keys}</span>;
}
