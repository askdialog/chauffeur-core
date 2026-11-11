// A markdown file's comments in the Files tab (story 57), against the daemon: read and changed
// through `files.comment*`, "Send N new" to the chat that wrote the file or another, and the
// file's share (its reviewers' comments come back through the daemon). The view is
// MarkdownComments; the rules live in the daemon (file_comments.rs).

import type { CommentAuthor, DesignComment, DesignShareState } from "@chauffeur/protocol";
import { Send } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { chatLabel, openChats } from "@/lib/chatTabs";
import { counts } from "@/lib/comments";
import { errorMessage } from "@/lib/rpc";
import { chatsOf, client, sendNow, setDesignShare, toast, useApp } from "@/state/app";
import { type CommentActions, MarkdownComments } from "./MarkdownComments";
import { Button, Select } from "./ui";

export type FileComments = {
  comments: DesignComment[];
  me: CommentAuthor | null;
  /** The open chat whose agent last wrote the file. */
  writer: string | null;
  error: string | null;
  reload: () => Promise<void>;
};

/** What a document's share is keyed by, as the daemon names it. */
export function fileShareKey(workspaceId: string, path: string): string {
  return `file:${workspaceId}:${path}`;
}

/** A document's share, read once shown, then kept by the daemon's events. */
export function useFileShare(workspaceId: string, path: string | null): DesignShareState | undefined {
  const state = useApp((s) => (path ? s.designShares[fileShareKey(workspaceId, path)] : undefined));
  const status = useApp((s) => s.connection.status);
  useEffect(() => {
    if (!path || status !== "open") return;
    client
      .call("files.share_state", { workspace: workspaceId, path })
      .then(setDesignShare)
      .catch((error: unknown) => console.warn("the file's share", errorMessage(error)));
  }, [workspaceId, path, status]);
  return state;
}

/** A markdown file's comments, read again when `revision` changes (the file, a reviewer's comment) and after each change of the user's. */
export function useFileComments(workspaceId: string, path: string | null, revision: string | undefined): FileComments {
  const [state, setState] = useState<Omit<FileComments, "reload">>({ comments: [], me: null, writer: null, error: null });
  const load = useCallback(async () => {
    if (!path) return;
    try {
      const result = await client.call("files.comments", { workspace: workspaceId, path });
      setState({
        comments: result.comments,
        me: result.author,
        writer: result.writer ?? null,
        error: result.error ? `.chauffeur/comments.json is invalid: ${result.error}` : null,
      });
    } catch (error) {
      setState((s) => ({ ...s, error: errorMessage(error) }));
    }
  }, [workspaceId, path]);
  useEffect(() => {
    setState({ comments: [], me: null, writer: null, error: null });
    void load();
  }, [load]);
  useEffect(() => {
    if (revision) void load();
  }, [revision, load]);
  return { ...state, reload: load };
}

export function CommentedMarkdown({
  workspaceId,
  path,
  text,
  state,
  panel,
  onPanel,
}: {
  workspaceId: string;
  path: string;
  text: string;
  state: FileComments;
  panel: boolean;
  onPanel: (open: boolean) => void;
}) {
  const [busy, setBusy] = useState(false);
  const act = async (what: string, run: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await run();
    } catch (error) {
      toast(`Could not ${what}: ${errorMessage(error)}`, "error");
      throw error;
    } finally {
      setBusy(false);
      await state.reload();
    }
  };

  const chats = useApp((s) => s.chats);
  const selectedChat = useApp((s) => s.selectedChat[workspaceId]);
  const open = useMemo(() => openChats(chatsOf(chats, workspaceId)), [chats, workspaceId]);
  const [picked, setPicked] = useState<string | null>(null);
  const target =
    open.find((c) => c.id === picked) ?? open.find((c) => c.id === state.writer) ?? open.find((c) => c.id === selectedChat) ?? open[0] ?? null;

  /** Sends the file's new comments to the chat picked as one message, and marks them sent. */
  const sendNew = async () => {
    if (!target) {
      toast("Open a chat to send the comments to", "error");
      return;
    }
    const result = await client.call("files.comments_send", { workspace: workspaceId, path });
    if (result.sent === 0) {
      toast("Every comment was sent already");
      return;
    }
    if (await sendNow(target, result.message)) {
      toast(`${result.sent === 1 ? "1 comment" : `${result.sent} comments`} sent to ${chatLabel(target, open)}`);
    }
  };
  const quiet = (run: Promise<void>) => void run.catch(() => {});
  // Kept first, then sent on its own: a failed send leaves the comment kept, and nothing to retry twice.
  const actions: CommentActions = {
    add: async (quote, text, _mentions, send) => {
      await act("add the comment", () => client.call("files.comment_add", { workspace: workspaceId, path, quote, text }));
      if (send) quiet(act("send the comments", sendNew));
    },
    reply: async (threadId, text, _mentions, send) => {
      await act("reply", () => client.call("files.comment_add", { workspace: workspaceId, path, text, reply_to: threadId }));
      if (send) quiet(act("send the comments", sendNew));
    },
    edit: (id, text) => quiet(act("edit the comment", () => client.call("files.comment_edit", { workspace: workspaceId, id, text }))),
    remove: (id) => quiet(act("delete the comment", () => client.call("files.comment_delete", { workspace: workspaceId, id }))),
    resolve: (id, resolved) =>
      quiet(act(resolved ? "resolve the comment" : "reopen the comment", () => client.call("files.comment_resolve", { workspace: workspaceId, id, resolved }))),
  };

  const unsent = counts(state.comments).unsent;
  const sendBar = (
    <div className="flex shrink-0 items-center gap-1.5 border-b border-line px-3 py-1.5 text-[12px]">
      <Button
        size="sm"
        variant="primary"
        disabled={unsent === 0 || !target}
        busy={busy}
        onClick={() => quiet(act("send the comments", sendNew))}
        title={
          !target
            ? "Open a chat to send the comments to"
            : unsent === 0
              ? "Every comment was sent"
              : "Send the new comments to this chat's agent, asking it to address them (after its turn, when it works)"
        }
        data-testid="file-comments-send"
      >
        {!busy && <Send size={12} />} Send {unsent} new to
      </Button>
      {target ? (
        <Select
          value={target.id}
          options={open.map((chat) => ({
            value: chat.id,
            label: chatLabel(chat, open),
            description: chat.id === state.writer ? "Its agent last wrote this file" : undefined,
          }))}
          onChange={setPicked}
          title="The chat the comments go to"
          className="min-w-0"
          testId="file-comments-chat"
        />
      ) : (
        <span className="text-faint">no open chat</span>
      )}
    </div>
  );

  return (
    <MarkdownComments
      text={text}
      comments={state.comments}
      me={state.me}
      error={state.error}
      busy={busy}
      panel={panel}
      onPanel={onPanel}
      actions={actions}
      canSend
      sendBar={sendBar}
    />
  );
}
