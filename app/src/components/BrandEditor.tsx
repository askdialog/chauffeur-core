// The brand editor (docs/adr/brands.md): a brand's chat beside its specimen and layouts, filling
// the main area. The chat lives in a workspace the daemon keeps for the brands and never lists:
// nothing of it shows among the user's workspaces.

import { Palette, X } from "lucide-react";
import { emptyChatView } from "@/lib/chat";
import { closeBrandEditor, useApp } from "@/state/app";
import { CatalogContext, useWorkspaceCatalog } from "@/state/catalog";
import { useWorkspaceSelector, useWorkspaceSession } from "@/state/session";
import { ChatConversation } from "./chat/ChatPanel";
import { DesignChatLayout } from "./design/DesignChatLayout";
import { EmptyState, IconButton } from "./ui";

export function BrandEditor({ editor }: { editor: { brand: string; name: string; chat: string; workspace: string } }) {
  const session = useWorkspaceSession(editor.workspace);
  const catalog = useWorkspaceCatalog({ id: editor.workspace });
  const chat = useApp((s) => s.chats[editor.chat]);
  const view = useWorkspaceSelector(session, (s) => s.chats[editor.chat]) ?? emptyChatView;
  const replayed = useWorkspaceSelector(session, (s) => s.live);
  const partial = useWorkspaceSelector(session, (s) => s.partial);
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="brand-editor">
      <div className="flex h-11 shrink-0 items-center gap-2 border-b border-line px-3">
        <Palette size={14} className="text-muted" />
        <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-fg">Brand: {editor.name}</span>
        <IconButton title="Close the brand editor" onClick={closeBrandEditor} data-testid="brand-editor-close">
          <X size={14} />
        </IconButton>
      </div>
      {chat ? (
        <CatalogContext.Provider value={catalog}>
          <div className="flex min-h-0 flex-1 flex-col">
            <DesignChatLayout chat={chat} view={view}>
              <ChatConversation
                chat={chat}
                view={view}
                readOnly={chat.status === "closed"}
                archived={false}
                loading={!replayed}
                partial={partial}
              />
            </DesignChatLayout>
          </div>
        </CatalogContext.Provider>
      ) : (
        <EmptyState title="Loading…" />
      )}
    </div>
  );
}
