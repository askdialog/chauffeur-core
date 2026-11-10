import { ChevronDown, ChevronUp, X } from "lucide-react";
import { type MutableRefObject, type RefObject, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import type { VirtuosoHandle } from "react-virtuoso";
import type { ChatRow } from "@/lib/chat";
import { type ChatMatch, findInChat, findInText, findOwner, firstMatchFrom, matchLabel, stepMatch } from "@/lib/chatFind";
import { hasMod, hasSecondMod, inForeignEditor, kbd } from "@/lib/keys";
import { IconButton } from "../ui";

/** The CSS highlights painting the matches in view (index.css): every match, and the current one. */
const ALL = "chat-find";
const CURRENT = "chat-find-current";

/** The chats on screen, in the order they were shown, and when the user last clicked or focused each. */
type Shown = { area: RefObject<HTMLElement | null>; touched: number };
const shownChats: Shown[] = [];
let clock = 0;

/**
 * Find in the chat, as a browser's find bar: ⌘F (Ctrl+F elsewhere) opens it, ↵ / ⇧↵ (and ⌘G /
 * ⇧⌘G) go to the next / previous match, Esc closes it. The search runs over the rows' data
 * (lib/chatFind.ts); the matches of the rows in view are painted with the CSS Custom Highlight
 * API, which leaves the rendered markdown untouched. Without that API, matches are still reached,
 * not painted.
 */
export function ChatFind({
  area,
  rows,
  list,
  scroller,
  jumping,
  onClose,
}: {
  /** The whole chat, its composer included: ⌘F opens this bar when typed or last clicked in it. */
  area: RefObject<HTMLElement | null>;
  rows: readonly ChatRow[];
  list: RefObject<VirtuosoHandle | null>;
  /** The list's scroll area. */
  scroller: HTMLElement | null;
  /** True while a jump scrolls: the list must not follow new output meanwhile. */
  jumping: MutableRefObject<boolean>;
  /** Esc closed the bar: the focus goes back to the composer. */
  onClose: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  // Long chats: typing stays fluid, the search follows.
  const search = useDeferredValue(query);
  const matches = useMemo(() => (open ? findInChat(rows, search) : []), [open, rows, search]);
  const [picked, setPicked] = useState(-1);
  const current = Math.min(picked, matches.length - 1);
  const input = useRef<HTMLInputElement>(null);

  const latest = useRef({ matches, current, search });
  latest.current = { matches, current, search };

  // A jump to a match under way: cancelled by the next one, by closing or by unmounting.
  const pending = useRef<(() => void) | null>(null);
  const reveal = (match: ChatMatch) => {
    pending.current?.();
    if (!scroller) return;
    jumping.current = true;
    let frame = 0;
    let tries = 0;
    let settled = 0;
    const done = () => {
      cancelAnimationFrame(frame);
      pending.current = null;
      requestAnimationFrame(() => {
        jumping.current = false;
      });
    };
    // Rows far away are only estimated until rendered: scroll to the row, then to the match in it
    // once drawn, until it stays in view.
    const step = () => {
      const range = matchRange(scroller, match, latest.current.search);
      if (range) {
        if (inView(range, scroller)) settled++;
        else {
          settled = 0;
          const box = range.getBoundingClientRect();
          const area = scroller.getBoundingClientRect();
          scroller.scrollBy({ top: box.top + box.height / 2 - (area.top + area.height / 2) });
        }
      } else if (tries % 6 === 0) {
        list.current?.scrollToIndex({ index: match.index, align: "center" });
      }
      if (settled >= 2 || ++tries > 40) return done();
      frame = requestAnimationFrame(step);
    };
    step();
    pending.current = done;
  };
  useEffect(() => () => pending.current?.(), []);

  const go = (direction: 1 | -1) => {
    const { matches, current } = latest.current;
    const next = stepMatch(current, matches.length, direction);
    setPicked(next);
    if (next >= 0) reveal(matches[next]);
  };

  // A new search starts from the top of the view.
  useEffect(() => {
    if (!open) return;
    const start = firstMatchFrom(matches, scroller ? topRow(scroller) : 0);
    setPicked(start);
    if (start >= 0) reveal(matches[start]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, search]);

  // Two chats on screen (the Office's floating one over another): the keys go to one of them.
  const me = useRef<Shown>({ area, touched: 0 });
  useEffect(() => {
    const chat = me.current;
    shownChats.push(chat);
    const touch = () => (chat.touched = ++clock);
    const el = area.current;
    el?.addEventListener("pointerdown", touch, true);
    el?.addEventListener("focusin", touch);
    return () => {
      shownChats.splice(shownChats.indexOf(chat), 1);
      el?.removeEventListener("pointerdown", touch, true);
      el?.removeEventListener("focusin", touch);
    };
  }, [area]);

  // ⌘F opens the bar or selects its text again; ⌘G and ⇧⌘G step while it is open. The Files
  // editor and the terminal keep their own: their panes stop these keys (WorkspaceViews).
  const keys = useRef({ go, open });
  keys.current = { go, open };
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || !hasMod(event) || event.altKey || hasSecondMod(event)) return;
      if (document.querySelector("[role=dialog]")) return;
      if (inForeignEditor(event.target)) return;
      const target = event.target as Element | null;
      const owner = findOwner(
        shownChats.map((chat) => ({ typedIn: !!target && !!chat.area.current?.contains(target), touched: chat.touched })),
      );
      if (shownChats[owner] !== me.current) return;
      const key = event.key.toLowerCase();
      if (key === "f" && !event.shiftKey) {
        event.preventDefault();
        setOpen(true);
        requestAnimationFrame(() => {
          input.current?.focus();
          input.current?.select();
        });
      } else if (key === "g" && keys.current.open) {
        event.preventDefault();
        keys.current.go(event.shiftKey ? -1 : 1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Paints the matches of the rows in view, again whenever the list draws other rows or their text changes.
  useEffect(() => {
    const highlights = typeof CSS !== "undefined" && "highlights" in CSS ? CSS.highlights : null;
    if (!open || !scroller || !highlights) return;
    let frame = 0;
    const paint = () => {
      frame = 0;
      const { matches, current, search } = latest.current;
      const rowsWithMatches = new Set(matches.map((match) => match.index));
      const target = matches[current];
      const all: Range[] = [];
      let currentRange: Range | null = null;
      for (const row of scroller.querySelectorAll<HTMLElement>("[data-index]")) {
        const index = Number(row.dataset.index);
        if (!rowsWithMatches.has(index)) continue;
        const ranges = textRanges(row, search);
        if (target?.index === index) currentRange = ranges[target.occurrence] ?? null;
        all.push(...ranges.filter((range) => range !== currentRange));
      }
      highlights.set(ALL, new Highlight(...all));
      if (currentRange) highlights.set(CURRENT, new Highlight(currentRange));
      else highlights.delete(CURRENT);
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(paint);
    };
    schedule();
    const observer = new MutationObserver(schedule);
    observer.observe(scroller, { childList: true, subtree: true, characterData: true });
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
      highlights.delete(ALL);
      highlights.delete(CURRENT);
    };
  }, [open, scroller, matches, current, search]);

  const close = () => {
    pending.current?.();
    setOpen(false);
    setPicked(-1);
    onClose();
  };

  if (!open) return null;
  const label = matchLabel(current, matches.length, search);
  return (
    <div
      role="search"
      aria-label="Find in this chat"
      // Clear of the timeline rail, on the right edge.
      className="absolute right-7 top-2 z-20 flex items-center gap-0.5 rounded-lg border border-line-strong bg-raised py-1 pl-2 pr-1 shadow-lg"
      data-testid="chat-find"
    >
      <input
        ref={input}
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && !event.nativeEvent.isComposing) {
            event.preventDefault();
            go(event.shiftKey ? -1 : 1);
          } else if (event.key === "Escape") {
            // Not the window's Esc: it would interrupt the agent.
            event.preventDefault();
            event.stopPropagation();
            close();
          }
        }}
        placeholder="Find in chat"
        spellCheck={false}
        className="h-6 w-52 min-w-0 bg-transparent text-[12.5px] text-fg placeholder:text-faint focus:outline-none"
        data-testid="chat-find-input"
      />
      <span className="min-w-[60px] px-1 text-right text-[11.5px] tabular-nums text-faint" aria-live="polite" data-testid="chat-find-count">
        {label}
      </span>
      <IconButton title={`Previous match (${kbd("⇧↵")})`} disabled={matches.length === 0} onClick={() => go(-1)} data-testid="chat-find-previous">
        <ChevronUp size={14} />
      </IconButton>
      <IconButton title={`Next match (${kbd("↵")})`} disabled={matches.length === 0} onClick={() => go(1)} data-testid="chat-find-next">
        <ChevronDown size={14} />
      </IconButton>
      <IconButton title="Close (Esc)" onClick={close} data-testid="chat-find-close">
        <X size={13} />
      </IconButton>
    </div>
  );
}

/** The index of the first row drawn in the scroll area's window. */
function topRow(scroller: HTMLElement): number {
  const top = scroller.getBoundingClientRect().top;
  for (const row of scroller.querySelectorAll<HTMLElement>("[data-index]")) {
    if (row.getBoundingClientRect().bottom > top) return Number(row.dataset.index);
  }
  return 0;
}

/** Whether a match shows in the scroll area, below the find bar. */
function inView(range: Range, scroller: HTMLElement): boolean {
  const box = range.getBoundingClientRect();
  const area = scroller.getBoundingClientRect();
  return box.top >= area.top + 48 && box.bottom <= area.bottom - 8;
}

/** The match's range in its row, once the list draws it. */
function matchRange(scroller: HTMLElement, match: ChatMatch, query: string): Range | null {
  const row = scroller.querySelector<HTMLElement>(`[data-index="${match.index}"]`);
  if (!row) return null;
  const ranges = textRanges(row, query);
  return ranges[match.occurrence] ?? null;
}

/**
 * The matches in a row's searchable text as drawn (`data-find-text`), its buttons (a code block's
 * Copy) and diagrams (`data-find-skip`) left out, as lib/markdown.ts leaves them out of the data: the
 * two give the same matches in the same order. The text is read across its nodes, so a match may
 * span inline formatting.
 */
export function textRanges(row: HTMLElement, query: string): Range[] {
  const nodes: Text[] = [];
  const starts: number[] = [];
  let text = "";
  for (const block of row.querySelectorAll("[data-find-text]")) {
    const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT, {
      acceptNode: (node) => (node.parentElement?.closest("button, [data-find-skip]") ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
    });
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      nodes.push(node as Text);
      starts.push(text.length);
      text += node.nodeValue ?? "";
    }
  }
  // The node holding an offset: a match's end may sit at the very end of its node.
  const at = (offset: number, end: boolean) => {
    let i = nodes.length - 1;
    while (i > 0 && (end ? starts[i] >= offset : starts[i] > offset)) i--;
    return { node: nodes[i], offset: offset - starts[i] };
  };
  return findInText(text, query).map(({ start, end }) => {
    const from = at(start, false);
    const to = at(end, true);
    const range = document.createRange();
    range.setStart(from.node, from.offset);
    range.setEnd(to.node, to.offset);
    return range;
  });
}
