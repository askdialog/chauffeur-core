// The chat's MCP servers, in the composer beside the context meter, only while one of them is
// broken (its login expired, it does not answer): clicking it lists every server of the chat's
// agent session with where it stands, and logs in or reconnects a broken one.

import type { ChatInfo, McpServer } from "@chauffeur/protocol";
import { Plug } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { isBroken, lacksEnv, mcpStatusText, sortedServers } from "@/lib/connectors";
import { connectMcp } from "@/state/app";
import { MissingEnvHelp } from "../MissingEnvHelp";
import { Button, cx, usePopoverPosition } from "../ui";

const PANEL_WIDTH = 340;

export function McpServers({ chat }: { chat: ChatInfo }) {
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const broken = chat.mcp_servers.filter(isBroken);

  // A click elsewhere or Escape closes it; Escape does not reach the chat (it would interrupt).
  useEffect(() => {
    if (!open) return;
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (!panel.current?.contains(target) && !anchor.current?.contains(target)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  if (broken.length === 0) return null;
  const label = broken.length === 1 ? broken[0]!.name : `${broken.length} MCP servers`;
  return (
    <>
      <button
        ref={anchor}
        type="button"
        onClick={() => setOpen(!open)}
        aria-expanded={open}
        aria-haspopup="dialog"
        title={broken.length === 1 ? `${broken[0]!.name}: ${mcpStatusText(broken[0]!)}` : `${broken.length} MCP servers are disconnected`}
        className={cx(
          "inline-flex h-6 max-w-[180px] items-center gap-1 rounded-md px-1.5 text-[11px] text-attention transition-colors",
          open ? "bg-hover" : "hover:bg-hover",
        )}
        data-testid="mcp-servers"
      >
        <Plug size={12} className="shrink-0" />
        <span className="truncate">{label}</span>
      </button>
      {open && <McpPanel panelRef={panel} anchor={anchor} chat={chat} />}
    </>
  );
}

function McpPanel({
  panelRef,
  anchor,
  chat,
}: {
  panelRef: React.RefObject<HTMLDivElement | null>;
  anchor: React.RefObject<HTMLElement | null>;
  chat: ChatInfo;
}) {
  const position = usePopoverPosition(anchor, PANEL_WIDTH, 320);
  return createPortal(
    <div
      ref={panelRef}
      data-popover
      role="dialog"
      aria-label="MCP servers"
      data-testid="mcp-panel"
      className="fixed z-[60] flex flex-col overflow-y-auto rounded-xl border border-line-strong bg-sidebar p-1.5 text-[12px] shadow-2xl shadow-black/70"
      style={{ ...position, width: PANEL_WIDTH }}
    >
      <div className="px-2 pb-1.5 pt-1 text-[11px] font-medium text-faint">MCP servers of this chat</div>
      <div className="flex flex-col divide-y divide-line rounded-lg border border-line bg-raised">
        {sortedServers(chat.mcp_servers).map((server) => (
          <ServerRow key={server.name} chat={chat} server={server} />
        ))}
      </div>
    </div>,
    document.body,
  );
}

const DOT: Record<McpServer["status"], string> = {
  connected: "bg-success",
  pending: "bg-faint",
  needs_auth: "bg-attention",
  failed: "bg-danger",
  disabled: "bg-line-strong",
};

function ServerRow({ chat, server }: { chat: ChatInfo; server: McpServer }) {
  const [busy, setBusy] = useState(false);
  const broken = isBroken(server);
  // Reconnecting cannot give it a variable: the guide below does.
  const needsEnv = lacksEnv(server);
  const connect = async () => {
    setBusy(true);
    try {
      await connectMcp(chat, server.name);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="flex flex-col gap-1.5 px-2.5 py-1.5" data-server={server.name} data-status={server.status}>
      <div className="flex items-center gap-2">
        <span className={cx("h-1.5 w-1.5 shrink-0 rounded-full", DOT[server.status])} />
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-fg">{server.name}</span>
          <span className={cx("truncate text-[11px]", broken ? "text-attention" : "text-faint")} title={mcpStatusText(server)}>
            {mcpStatusText(server)}
            {server.failed_call && " · the agent needed it"}
          </span>
        </span>
        {broken && !needsEnv && (
          <Button size="xs" variant={server.status === "needs_auth" ? "primary" : "secondary"} busy={busy} onClick={() => void connect()}>
            {server.status === "needs_auth" ? "Log in" : "Reconnect"}
          </Button>
        )}
      </div>
      {needsEnv && (
        <div className="pl-3.5">
          <MissingEnvHelp server={server} />
        </div>
      )}
    </div>
  );
}
