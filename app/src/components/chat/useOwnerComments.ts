// The comments of a shared workspace's chat in its owner's app (story 53): its threads under the
// agent's messages they are on; while the chat is in a live share, the owner comments on a passage,
// replies and deletes their own, mentioning with `@` whom the share is open to or someone they
// invite at once (docs/adr/mentions-in-share-comments.md). A thread that mentions the owner is
// unread until it shows; one a mention opened is brought into view.

import type { ChatInfo, Mention, ShareComment } from "@chauffeur/protocol";
import { useCallback, useEffect, useMemo } from "react";
import type { ChatItem } from "@/lib/chat";
import { firstName } from "@/lib/mentions";
import { blockOf, placeComments } from "@/lib/shareComments";
import { markMentionSeen, openThreadDone, useApp } from "@/state/app";
import { addShareComment, deleteShareComment, inviteMentioned, loadComments, useWorkspaceShares } from "@/state/workspaceShares";
import type { MentionSetup } from "../design/Mentions";
import type { Passage, ShareComments } from "./ShareComments";

const NO_ITEMS: ChatItem[] = [];

export type OwnerComments = {
  /** What the transcript's comment cards read. */
  context: ShareComments;
  /** Comments on a passage: none while the chat is not in a live share, or read-only. */
  comment: ((passage: Passage, text: string, mentions: Mention[]) => Promise<boolean>) | null;
  mentionSetup: MentionSetup | undefined;
  /** The block of the thread a mention opened, for the list to bring into view. */
  targetBlock: string | null;
};

export function useOwnerComments(chat: ChatInfo, items: readonly ChatItem[], readOnly: boolean): OwnerComments {
  const comments = useWorkspaceShares((s) => s.comments[chat.id]);
  useEffect(() => void loadComments(chat.id), [chat.id]);
  // Without comments, the placement stays the same while the agent streams: its readers do not render again.
  const placedOn = comments?.length ? items : NO_ITEMS;
  const byItem = useMemo(() => placeComments(placedOn, comments ?? []), [placedOn, comments]);

  const state = useWorkspaceShares((s) => s.shares[chat.workspace_id]);
  const identity = useApp((s) => s.identity);
  const share = state?.share && !state.share.stopped ? state.share : null;
  const inShare = share != null && !readOnly && (state?.chat_ids == null || state.chat_ids.includes(chat.id));
  const me = share?.owner.email ?? identity?.email ?? null;
  const teamDomain = share?.team ? (identity?.team_domain ?? null) : null;
  const mentionSetup = useMemo<MentionSetup | undefined>(
    () =>
      inShare && share
        ? {
            people: share.people.length > 0 ? share.people : [share.owner, ...share.guests.map((g) => ({ email: g.email, name: firstName(g.email) }))],
            me,
            teamDomain,
            canInvite: true,
          }
        : undefined,
    [inShare, share, me, teamDomain],
  );

  // The owner's unread mentions in this chat, by thread.
  const allMentions = useApp((s) => s.mentions);
  const unreadHere = useMemo(() => allMentions.filter((m) => !m.seen && m.place === "workspace" && m.chat === chat.id), [allMentions, chat.id]);
  const unread = useMemo(() => new Set(unreadHere.map((m) => m.thread)), [unreadHere]);
  const onSeen = useCallback(
    (thread: string) => {
      for (const m of unreadHere) if (m.thread === thread) void markMentionSeen(m.id);
    },
    [unreadHere],
  );

  const threadTarget = useApp((s) => s.threadTarget);
  const target = threadTarget?.chat === chat.id ? threadTarget.thread : null;
  const targetBlock = target ? blockOf(byItem, target) : null;

  const workspace = chat.workspace_id;
  const post = useCallback(
    async (text: string, mentions: Mention[], where: { reply_to: string } | { message: string; quote: string }) => {
      if (!(await inviteMentioned(workspace, mentions, teamDomain))) return false;
      return addShareComment({ chat: chat.id, text, mentions: mentions.length > 0 ? mentions : undefined, ...where });
    },
    [workspace, teamDomain, chat.id],
  );
  const context = useMemo<ShareComments>(
    () => ({
      byItem,
      me,
      onDelete: inShare ? (comment: ShareComment) => void deleteShareComment(chat.id, comment) : null,
      onReply: inShare ? (thread: ShareComment, text: string, mentions: Mention[]) => post(text, mentions, { reply_to: thread.id }) : null,
      mentionSetup,
      unread,
      onSeen,
      target: targetBlock ? target : null,
      onTargetShown: openThreadDone,
    }),
    [byItem, me, inShare, chat.id, post, mentionSetup, unread, onSeen, target, targetBlock],
  );
  return {
    context,
    comment: inShare ? (passage, text, mentions) => post(text, mentions, { message: passage.message, quote: passage.quote }) : null,
    mentionSetup,
    targetBlock,
  };
}
