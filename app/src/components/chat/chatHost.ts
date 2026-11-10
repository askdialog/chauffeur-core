// What a chat's rows ask of the place that shows them: in the app, the daemon (attachments, the
// workspace's chats, its settings); on a workspace share's page, nothing of it. The rows read it
// from a context, so that the share page's build carries no daemon connection.

import type { Attachment, ChatInfo } from "@chauffeur/protocol";
import { createContext } from "react";

export type ChatHost = {
  /** Where an attachment of a workspace is served; null where none is (a share page). */
  attachmentUrl: (workspaceId: string, attachment: Attachment) => string | null;
  /** Every chat known, to find a delegate call's delegated chat. */
  chats: Record<string, ChatInfo>;
  /** Shows a chat (a delegated one), reopened if closed. */
  openChat: (chat: ChatInfo) => void;
  isExplore: (workspaceId: string) => boolean;
  /** Who answers an open request when the reader cannot (a share page: the owner's name). */
  waitingFor: string | null;
};

export const NO_HOST: ChatHost = {
  attachmentUrl: () => null,
  chats: {},
  openChat: () => {},
  isExplore: () => false,
  waitingFor: null,
};

export const ChatHostContext = createContext<ChatHost>(NO_HOST);
