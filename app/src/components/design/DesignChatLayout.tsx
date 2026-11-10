// A design chat's tab (story 47): the chat on the left (about 380 px, resizable, the width kept for
// the app), the canvas on the right. Too narrow for both, the tab shows one at a time, with a
// switch in its header. ⌘\ (or the canvas header's button) hides the chat, the canvas taking the
// whole width; kept for the app, as the width is. Narrow, it switches between the two.

import type { ChatInfo } from "@chauffeur/protocol";
import { LayoutGrid, MessageSquare, PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { CHAT_WIDTH_KEY, designPanes, DIVIDER, storedChatWidth } from "@/lib/canvas";
import type { ChatView } from "@/lib/chat";
import { kbd } from "@/lib/keys";
import { toggleDesignChat, useApp } from "@/state/app";
import { cx, IconButton } from "../ui";
import { DesignCanvas } from "./DesignCanvas";

type Pane = "chat" | "canvas";

export function DesignChatLayout({ chat, view, children }: { chat: ChatInfo; view: ChatView; children: ReactNode }) {
  const box = useRef<HTMLDivElement>(null);
  const [total, setTotal] = useState<number | null>(null);
  const [wanted, setWanted] = useState(() => storedChatWidth(readWidth()));
  const [resizing, setResizing] = useState(false);
  const hidden = useApp((s) => s.designChatHidden);
  const [shown, setShown] = useState<Pane>(hidden ? "canvas" : "chat");
  // Narrow, ⌘\ switches to the canvas or back to the chat.
  const wasHidden = useRef(hidden);
  useEffect(() => {
    if (wasHidden.current === hidden) return;
    wasHidden.current = hidden;
    setShown(hidden ? "canvas" : "chat");
  }, [hidden]);

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setTotal(el.clientWidth));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const panes = total == null ? { narrow: false, chat: wanted } : designPanes(total, wanted);
  const narrow = panes.narrow;
  const widthAt = (clientX: number) => clientX - (box.current?.getBoundingClientRect().left ?? 0) - DIVIDER / 2;
  const switcher = narrow ? (
    <PaneSwitch shown={shown} onChange={setShown} />
  ) : (
    <IconButton
      title={`${hidden ? "Show" : "Hide"} the chat (${kbd("⌘\\")})`}
      onClick={() => toggleDesignChat()}
      className="mr-1"
      data-testid="design-toggle-chat"
    >
      {hidden ? <PanelLeftOpen size={13} /> : <PanelLeftClose size={13} />}
    </IconButton>
  );
  const chatHidden = !narrow && hidden;

  return (
    <div ref={box} className="relative flex min-h-0 flex-1" data-testid="design-layout" data-narrow={narrow || undefined}>
      <div
        className={cx(
          "flex min-h-0 min-w-0 flex-col",
          narrow ? (shown === "chat" ? "flex-1" : "hidden") : chatHidden ? "hidden" : "shrink-0",
        )}
        style={narrow ? undefined : { width: panes.chat }}
        data-testid="design-chat-column"
      >
        {narrow && <div className="flex h-8 shrink-0 items-center border-b border-line px-2">{switcher}</div>}
        {children}
      </div>
      {!narrow && !chatHidden && (
        <div
          role="separator"
          aria-orientation="vertical"
          title="Drag to resize"
          className="group relative shrink-0 cursor-col-resize touch-none"
          style={{ width: DIVIDER }}
          onPointerDown={(event) => {
            if (event.button !== 0) return;
            event.preventDefault();
            event.currentTarget.setPointerCapture(event.pointerId);
            setResizing(true);
          }}
          onPointerMove={(event) => {
            if (resizing) setWanted(designPanes(total ?? 0, widthAt(event.clientX)).chat);
          }}
          onPointerUp={(event) => {
            if (!resizing) return;
            setResizing(false);
            const width = designPanes(total ?? 0, widthAt(event.clientX)).chat;
            setWanted(width);
            saveWidth(width);
          }}
          onPointerCancel={() => setResizing(false)}
          onDoubleClick={() => {
            setWanted(storedChatWidth(null));
            saveWidth(storedChatWidth(null));
          }}
          data-testid="design-divider"
        >
          <div
            className={cx(
              "absolute inset-y-0 left-1/2 w-px -translate-x-1/2 transition-colors",
              resizing ? "bg-accent" : "bg-line group-hover:bg-accent/60",
            )}
          />
        </div>
      )}
      <div className={cx("min-h-0 min-w-0 flex-1", narrow && shown === "chat" && "hidden")}>
        <DesignCanvas chat={chat} view={view} headerStart={switcher} />
      </div>
      {/* While resizing, the screens' iframes must not take the pointer. */}
      {resizing && <div className="fixed inset-0 z-[65] cursor-col-resize" />}
    </div>
  );
}

function PaneSwitch({ shown, onChange }: { shown: Pane; onChange: (pane: Pane) => void }) {
  const option = (pane: Pane, label: string, icon: ReactNode) => (
    <button
      role="tab"
      aria-selected={shown === pane}
      onClick={() => onChange(pane)}
      className={cx(
        "flex h-5 items-center gap-1 rounded px-2 text-[12px]",
        shown === pane ? "bg-hover text-fg" : "text-muted hover:text-fg",
      )}
      data-testid={`design-show-${pane}`}
    >
      {icon}
      {label}
    </button>
  );
  return (
    <div role="tablist" aria-label="Show" className="mr-1 flex shrink-0 items-center gap-0.5 rounded-md border border-line bg-panel p-0.5">
      {option("chat", "Chat", <MessageSquare size={11} />)}
      {option("canvas", "Canvas", <LayoutGrid size={11} />)}
    </div>
  );
}

function readWidth(): string | null {
  try {
    return localStorage.getItem(CHAT_WIDTH_KEY);
  } catch {
    return null;
  }
}

function saveWidth(width: number) {
  try {
    localStorage.setItem(CHAT_WIDTH_KEY, String(width));
  } catch {
    // Storage full or unavailable: the width holds until the app restarts.
  }
}
