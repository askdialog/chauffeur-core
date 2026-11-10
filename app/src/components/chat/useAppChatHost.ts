// The app's chat host: the daemon serves the attachments, the store knows the chats.

import { useMemo } from "react";
import { useShallow } from "zustand/react/shallow";
import { client, reopenChat, selectChat, setTab, useApp } from "@/state/app";
import type { ChatHost } from "./chatHost";

export function useAppChatHost(): ChatHost {
  const chats = useApp((s) => s.chats);
  // Only which workspaces are explorations: the host, and every row reading it, stays the same
  // while a workspace's activity changes.
  const explorations = useApp(useShallow((s) => Object.values(s.workspaces).filter((w) => w.explore).map((w) => w.id)));
  // A new host at each reconnection: an attachment's URL follows it (the port may change).
  const status = useApp((s) => s.connection.status);
  return useMemo(
    () => ({
      attachmentUrl: (workspaceId, attachment) =>
        client.httpUrl(`/attachments/${encodeURIComponent(workspaceId)}/${encodeURIComponent(attachment.file)}`),
      chats,
      openChat: (chat) => {
        if (chat.closed) void reopenChat(chat.id);
        else selectChat(chat.workspace_id, chat.id);
        setTab(chat.workspace_id, "chat");
      },
      isExplore: (workspaceId) => explorations.includes(workspaceId),
      waitingFor: null,
    }),
    [chats, explorations, status],
  );
}
