// The chat's queue: messages sent with ⌘⏎ while the agent works, held by the daemon until the
// turn ends, then sent one by one. Drag to reorder, edit, delete, or send one now (it steers).

import type { QueuedMessage } from "@chauffeur/protocol";
import { ArrowUp, GripVertical, Paperclip, Pencil, X } from "lucide-react";
import { useRef, useState } from "react";
import { moveEntry } from "@/lib/models";
import { errorMessage } from "@/lib/rpc";
import { client, toast } from "@/state/app";
import { Button, cx } from "../ui";

export function QueuePanel({ chatId, queue, working }: { chatId: string; queue: QueuedMessage[]; working: boolean }) {
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(null);
  const [drag, setDrag] = useState<{ from: number; over: number } | null>(null);
  const rows = useRef<Array<HTMLDivElement | null>>([]);

  const save = async (messages: Array<{ id: string; text: string }>) => {
    try {
      await client.call("chat.queue_set", { chat: chatId, items: messages });
    } catch (error) {
      toast(`Could not change the queue: ${errorMessage(error)}`, "error");
    }
  };
  const sendNow = async (id: string) => {
    try {
      await client.call("chat.queue_send", { chat: chatId, id });
    } catch (error) {
      toast(`Could not send: ${errorMessage(error)}`, "error");
    }
  };
  const items = (list: QueuedMessage[]) => list.map((m) => ({ id: m.id, text: m.text }));

  const shown = drag ? moveEntry(queue, drag.from, drag.over) : queue;
  const moveDrag = (event: React.PointerEvent) => {
    if (!drag) return;
    let over = 0;
    queue.forEach((_, i) => {
      const rect = rows.current[i]?.getBoundingClientRect();
      if (rect && event.clientY > rect.top + rect.height / 2) over = i;
    });
    if (over !== drag.over) setDrag({ ...drag, over });
  };
  const endDrag = () => {
    if (drag && drag.from !== drag.over) void save(items(moveEntry(queue, drag.from, drag.over)));
    setDrag(null);
  };

  return (
    <div className="mx-auto w-full max-w-[860px] px-5 pb-1.5" data-testid="queue">
      <div className="rounded-lg border border-line bg-raised" onPointerMove={moveDrag} onPointerUp={endDrag}>
        <div className="flex h-7 items-center gap-2 border-b border-line px-2.5 text-[11.5px] text-faint">
          {working ? (
            <span>
              Queued · sent one by one when the agent finishes
            </span>
          ) : (
            <>
              <span className="text-attention">Queue paused: the last turn did not finish on its own</span>
              <Button size="sm" variant="ghost" className="ml-auto" onClick={() => void sendNow(queue[0]?.id ?? "")}>
                Send next
              </Button>
            </>
          )}
        </div>
        {shown.map((message, index) => (
          <div
            key={message.id}
            ref={(el) => {
              rows.current[index] = el;
            }}
            className={cx(
              "group flex min-h-8 items-center gap-2 px-1.5 py-1 text-[12.5px]",
              drag?.over === index && "bg-hover",
            )}
          >
            <span
              onPointerDown={(event) => {
                event.preventDefault();
                (event.target as Element).setPointerCapture(event.pointerId);
                setDrag({ from: index, over: index });
              }}
              title="Drag to reorder"
              className="flex h-6 w-4 cursor-grab items-center justify-center text-faint/60 hover:text-muted"
            >
              <GripVertical size={12} />
            </span>
            {editing?.id === message.id ? (
              <input
                autoFocus
                value={editing.text}
                onChange={(e) => setEditing({ id: message.id, text: e.target.value })}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && editing.text.trim()) {
                    e.preventDefault();
                    void save(items(queue).map((m) => (m.id === message.id ? { ...m, text: editing.text.trim() } : m)));
                    setEditing(null);
                  } else if (e.key === "Escape") {
                    e.preventDefault();
                    e.stopPropagation();
                    setEditing(null);
                  }
                }}
                onBlur={() => setEditing(null)}
                className="h-6 min-w-0 flex-1 rounded border border-accent/60 bg-bg px-1.5 text-fg focus:outline-none"
              />
            ) : (
              <span className="min-w-0 flex-1 truncate text-fg" title={message.text}>
                {message.text || <span className="text-faint">(attachments only)</span>}
              </span>
            )}
            {message.attachments.length > 0 && (
              <span className="flex shrink-0 items-center gap-0.5 text-[11px] text-faint" title={message.attachments.map((a) => a.name).join(", ")}>
                <Paperclip size={11} />
                {message.attachments.length}
              </span>
            )}
            <span className="flex shrink-0 items-center gap-0.5 opacity-0 group-hover:opacity-100">
              <RowAction title="Send now (steers the running turn)" onClick={() => void sendNow(message.id)}>
                <ArrowUp size={12} />
              </RowAction>
              <RowAction title="Edit" onClick={() => setEditing({ id: message.id, text: message.text })}>
                <Pencil size={11} />
              </RowAction>
              <RowAction title="Remove from the queue" onClick={() => void save(items(queue.filter((m) => m.id !== message.id)))}>
                <X size={12} />
              </RowAction>
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

function RowAction({ title, onClick, children }: { title: string; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      className="inline-flex h-5 w-5 items-center justify-center rounded text-muted hover:bg-hover hover:text-fg"
    >
      {children}
    </button>
  );
}
