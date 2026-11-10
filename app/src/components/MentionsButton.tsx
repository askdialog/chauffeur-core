// The user's mentions in shares' comments (docs/adr/mentions-in-share-comments.md): an @ in the
// sidebar's foot with the unread count, opening the list; a mention opens its thread (the design
// chat's canvas on the user's own share, else the share's page) and is read.

import type { Mentioned } from "@chauffeur/protocol";
import { AtSign } from "lucide-react";
import { useRef, useState } from "react";
import { relativeTime } from "@/lib/format";
import { excerpt, unreadCount } from "@/lib/mentions";
import { openMention, useApp } from "@/state/app";
import { cx, Popover } from "./ui";

export function MentionsButton() {
  const mentions = useApp((s) => s.mentions);
  const connection = useApp((s) => s.mentionsConnection);
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  // Nothing to show until the daemon follows the user's mentions: it answers `off` signed out.
  if (mentions.length === 0 && connection.state === "off") return null;
  const unread = unreadCount(mentions);
  return (
    <>
      <button
        ref={anchor}
        onClick={() => setOpen((on) => !on)}
        title={unread > 0 ? `${unread} unread ${unread === 1 ? "mention" : "mentions"}` : "Mentions"}
        aria-label="Mentions"
        aria-pressed={open}
        className={cx("relative inline-flex h-6 items-center gap-0.5 rounded-md px-1.5 text-[11px] hover:bg-hover", unread > 0 ? "text-accent" : "text-muted hover:text-fg")}
        data-testid="mentions-button"
      >
        <AtSign size={13} />
        {unread > 0 && <span className="font-semibold" data-testid="mentions-unread">{unread}</span>}
      </button>
      {open && (
        <Popover anchor={anchor} width={340} height={380} onClose={() => setOpen(false)} testId="mentions-list">
          <div className="flex h-8 shrink-0 items-center border-b border-line px-3 font-semibold text-fg">Mentions</div>
          {connection.state === "failed" && <div className="border-b border-line px-3 py-1.5 text-[11.5px] text-attention">Offline: {connection.message}</div>}
          <div className="max-h-[340px] overflow-y-auto">
            {mentions.length === 0 ? (
              <div className="p-5 text-center text-[12px] text-faint">Nobody mentioned you yet.</div>
            ) : (
              mentions.map((mention) => (
                <MentionRow
                  key={mention.id}
                  mention={mention}
                  onOpen={() => {
                    setOpen(false);
                    openMention(mention);
                  }}
                />
              ))
            )}
          </div>
        </Popover>
      )}
    </>
  );
}

function MentionRow({ mention, onOpen }: { mention: Mentioned; onOpen: () => void }) {
  return (
    <button type="button" onClick={onOpen} className="flex w-full items-start gap-2 border-b border-line px-3 py-2 text-left hover:bg-hover" data-testid="mention-row">
      <span className={cx("mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full", mention.seen ? "bg-transparent" : "bg-accent")} title={mention.seen ? undefined : "Unread"} />
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-1.5 text-[11px] text-muted">
          <span className="truncate">
            <span className="font-medium text-fg">{mention.author.name}</span> on {mention.share_title}
          </span>
          <span className="ml-auto shrink-0 text-faint">{relativeTime(new Date(mention.at).toISOString())}</span>
        </span>
        <span className={cx("line-clamp-2 text-[12.5px]", mention.seen ? "text-muted" : "text-fg")}>{excerpt(mention.text, 160)}</span>
      </span>
    </button>
  );
}
