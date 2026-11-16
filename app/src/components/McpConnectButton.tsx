import type { ChatInfo, McpServer } from "@chauffeur/protocol";
import { isBroken, lacksEnv } from "@/lib/connectors";
import { connectMcp, useApp } from "@/state/app";
import { Button } from "./ui";

/** Both places that report a disconnected server act on the same session and pending attempt. */
export function McpConnectButton({ chat, server }: { chat: ChatInfo; server: McpServer }) {
  const busy = useApp((s) => s.connectingMcp[chat.id]?.includes(server.name) ?? false);
  if (lacksEnv(server) || (!isBroken(server) && !busy)) return null;
  return (
    <Button size="xs" variant={server.status === "needs_auth" ? "primary" : "secondary"}
      busy={busy} onClick={() => void connectMcp(chat, server.name)}
      aria-label={`${server.status === "needs_auth" ? "Log in to" : "Reconnect"} ${server.name}`}>
      {busy ? "Connecting…" : server.status === "needs_auth" ? "Log in" : "Reconnect"}
    </Button>
  );
}
