// A workspace's Share dialog (story 53), as a design's (story 51): the chats shared (all of them,
// new ones too, or those picked, changed while it is live), the guests named by e-mail (past ones
// recalled), each a viewer or able to suggest messages, whether anyone at the owner's team domain
// looks too (offered only to a Google Workspace account), the link once shared, who is looking,
// the connection, Stop sharing. Sharing needs Chauffeur's Google sign-in: without it, the
// dialog starts with that button.

import type { Guest, GuestRole, WorkspaceInfo, WorkspaceShareState } from "@chauffeur/protocol";
import { Check, Copy, Eye, Link as LinkIcon, X } from "lucide-react";
import { type KeyboardEvent, useEffect, useMemo, useState } from "react";
import { errorMessage } from "@/lib/rpc";
import { connectionLabel, isShared, parseEmails, suggestions as offeredReviewers } from "@/lib/share";
import {
  addGuests,
  type ChatSelection,
  guestsDiffer,
  selectionOf,
  selectionParams,
  selectionsDiffer,
  setRole,
  toggleChat,
} from "@/lib/workspaceShare";
import { workspaceName } from "@/lib/format";
import { client, toast, useApp, upsertChats } from "@/state/app";
import { signIn, useIdentity } from "@/state/identity";
import { setWorkspaceShare, useWorkspaceShares } from "@/state/workspaceShares";
import { Badge, Button, cx, IconButton, Kbd, Modal, Select, type SelectOption } from "./ui";

const ROLES: Array<SelectOption<GuestRole>> = [
  { value: "viewer", label: "Viewer", description: "Reads the chats and the notes" },
  { value: "suggest", label: "Can suggest", description: "Also suggests messages; you decide what is sent" },
];

export function WorkspaceShareDialog({ workspace, onClose }: { workspace: WorkspaceInfo; onClose: () => void }) {
  const state = useWorkspaceShares((s) => s.shares[workspace.id]);
  useEffect(() => {
    client
      .call("workspace.share_state", { workspace: workspace.id })
      .then(setWorkspaceShare)
      .catch((error: unknown) => toast(`Could not read the workspace's share: ${errorMessage(error)}`, "error"));
  }, [workspace.id]);
  const shared = isShared(state);
  const chatMap = useApp((s) => s.chats);
  const chats = Object.values(chatMap).filter((chat) => chat.workspace_id === workspace.id && !chat.closed);
  const [chatsLoaded, setChatsLoaded] = useState(false);
  useEffect(() => {
    client.call("chat.list", { workspace: workspace.id, all: false }).then((result) => { upsertChats(result.chats); setChatsLoaded(true); })
      .catch((error: unknown) => toast(`Could not read chats: ${errorMessage(error)}`, "error"));
  }, [workspace.id]);
  const openIds = chats.map((chat) => chat.id);
  // What the share holds now (all chats unless it picked some), and what the dialog holds.
  const sharedSelection = selectionOf(state?.chat_ids, openIds);
  const sharedIds = state?.chat_ids ?? null;
  const [picked, setPicked] = useState<ChatSelection>(sharedIds);
  // The dialog follows the share's selection when it changes (read late, or updated), by value.
  const sharedKey = sharedIds?.join(" ") ?? null;
  useEffect(() => {
    setPicked(sharedIds);
  }, [sharedKey]);
  const selection = selectionOf(picked, openIds);
  const [guests, setGuests] = useState<Guest[]>(() => state?.share?.guests ?? []);
  const [team, setTeam] = useState(() => state?.share?.team ?? false);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const signingIn = useIdentity((s) => s.signingIn);
  const signInError = useIdentity((s) => s.error);
  const teamDomain = useApp((s) => s.identity?.team_domain ?? null);
  const signInToShare = async () => {
    if (!await signIn()) return;
    try { setWorkspaceShare(await client.call("workspace.share_state", { workspace: workspace.id })); }
    catch (error) { toast(`Could not read the share: ${errorMessage(error)}`, "error"); }
  };
  // The guests follow the share's once it answers (the dialog opened before the state came).
  const sharedGuests = state?.share?.guests;
  useEffect(() => {
    if (sharedGuests) setGuests(sharedGuests);
  }, [sharedGuests]);
  const sharedTeam = state?.share?.team;
  useEffect(() => {
    if (sharedTeam !== undefined) setTeam(sharedTeam);
  }, [sharedTeam]);

  const parsed = useMemo(() => parseEmails(typed), [typed]);
  const all = useMemo(() => addGuests(guests, parsed.emails), [guests, parsed.emails]);
  const offered = offeredReviewers(
    state,
    all.map((g) => g.email),
  );
  const reselected = selectionsDiffer(selection, sharedSelection);
  const changed = shared && (team !== sharedTeam || guestsDiffer(all, sharedGuests ?? []) || reselected);
  const connection = connectionLabel(state);

  const takeTyped = () => {
    if (parsed.emails.length === 0) return;
    setGuests(all);
    setTyped(parsed.invalid.join(" "));
  };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter" || event.key === "," || event.key === " ") {
      if (parsed.emails.length > 0) {
        event.preventDefault();
        takeTyped();
      }
    } else if (event.key === "Backspace" && typed === "" && guests.length > 0) {
      setGuests(guests.slice(0, -1));
    }
  };

  const act = async (what: string, call: () => Promise<WorkspaceShareState>) => {
    setBusy(true);
    try {
      setWorkspaceShare(await call());
    } catch (error) {
      toast(`Could not ${what}: ${errorMessage(error)}`, "error");
    } finally {
      setBusy(false);
    }
  };
  const share = () => act("share the workspace", () => client.call("workspace.share_start", { workspace: workspace.id, guests: all, team, ...selectionParams(selection) }));
  // The selection goes only when it changed: unchanged, the share keeps its own.
  const update = () =>
    act("change the share", () =>
      client.call("workspace.share_update", { workspace: workspace.id, guests: all, team, ...(reselected ? selectionParams(selection) : {}) }),
    );
  const stop = () => act("stop sharing", () => client.call("workspace.share_stop", { workspace: workspace.id }));
  const copy = async () => {
    const url = state?.share?.url;
    if (!url) return;
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (error) {
      toast(`Could not copy the link: ${errorMessage(error)}`, "error");
    }
  };

  const body = !state ? (
    <p className="text-[12.5px] text-muted">Reading the workspace's share…</p>
  ) : !state.configured ? (
    <p className="text-[12.5px] text-muted">Sharing is off on this computer: set design_share.url in ~/.chauffeur/settings.toml.</p>
  ) : !state.email ? (
    <div className="flex flex-col gap-3">
      <p className="text-[12.5px] text-muted">
        Share this workspace's chats live with anyone by e-mail: sign in with your Google account first (your Chauffeur sign-in).
      </p>
      {signInError && <p className="text-[12px] text-danger">{signInError}</p>}
      <Button variant="primary" onClick={() => void signInToShare()} disabled={signingIn} data-testid="workspace-share-sign-in">
        {signingIn ? "Finish in your browser…" : "Sign in with Google"}
      </Button>
    </div>
  ) : (
    <div className="flex flex-col gap-3">
      <fieldset disabled={busy} className="flex flex-col gap-1.5" data-testid="workspace-share-chats">
        <legend className="mb-1 text-[11.5px] font-medium text-muted">Chats</legend>
        <label className="flex items-start gap-2 text-[12.5px] text-fg">
          <input type="radio" name="workspace-share-chats" className="mt-0.5 accent-accent" checked={selection === null} onChange={() => setPicked(null)} data-testid="workspace-share-all-chats" />
          <span>
            All chats
            <span className="block text-[11.5px] text-faint">Chats you open later are shared too.</span>
          </span>
        </label>
        <label className="flex items-start gap-2 text-[12.5px] text-fg">
          <input
            type="radio"
            name="workspace-share-chats"
            className="mt-0.5 accent-accent"
            checked={selection !== null}
            onChange={() => setPicked(sharedSelection ?? openIds)}
            data-testid="workspace-share-picked-chats"
          />
          <span>
            Selected chats
            <span className="block text-[11.5px] text-faint">Only these; chats you open later stay private.</span>
          </span>
        </label>
        {selection !== null && (
          <div className="ml-6 flex max-h-40 flex-col gap-1 overflow-auto">
            {chats.map((chat) => (
              <label key={chat.id} className="flex items-center gap-2 text-[12.5px] text-fg">
                <input
                  type="checkbox"
                  className="accent-accent"
                  checked={selection.includes(chat.id)}
                  onChange={(event) => setPicked(toggleChat(selection, chat.id, event.target.checked))}
                  data-testid="workspace-share-chat"
                />
                <span className="min-w-0 truncate">{chat.title || "Untitled chat"}</span>
              </label>
            ))}
            {selection.length === 0 && <span className="text-[11.5px] text-danger">Pick at least one chat.</span>}
          </div>
        )}
        {shared && reselected && (
          <p className="text-[11.5px] text-faint">At Update, guests see the new chats at once; a chat taken out leaves their page with its suggestions and comments.</p>
        )}
      </fieldset>
      <div>
        <div className="mb-1 text-[11.5px] font-medium text-muted">Guests</div>
        {guests.length > 0 && (
          <div className="mb-1.5 flex flex-col gap-1">
            {guests.map((guest) => (
              <div key={guest.email} className="flex items-center gap-2 rounded-md bg-bg px-2 py-0.5 text-[12.5px]" data-testid="workspace-share-guest">
                <span
                  className={cx("h-1.5 w-1.5 shrink-0 rounded-full", state.share?.viewers.includes(guest.email) ? "bg-success" : "bg-faint")}
                  title={state.share?.viewers.includes(guest.email) ? "Looking now" : "Not looking now"}
                />
                <span className="min-w-0 flex-1 truncate text-fg">{guest.email}</span>
                <Select
                  value={guest.role}
                  options={ROLES}
                  onChange={(role) => setGuests(setRole(guests, guest.email, role))}
                  title="What they may do"
                  testId="workspace-share-role"
                  width={260}
                />
                <button
                  type="button"
                  className="text-faint hover:text-danger"
                  onClick={() => setGuests(guests.filter((g) => g.email !== guest.email))}
                  aria-label={`Remove ${guest.email}`}
                  title="Remove: they lose access at the next Update"
                >
                  <X size={12} />
                </button>
              </div>
            ))}
          </div>
        )}
        <div className="flex min-h-9 items-center rounded-md border border-line-strong bg-bg px-2 py-1 focus-within:border-accent/60">
          <input
            value={typed}
            onChange={(event) => setTyped(event.target.value)}
            onKeyDown={onKeyDown}
            onBlur={takeTyped}
            placeholder="alice@example.com, bob@example.com"
            className="min-w-[160px] flex-1 bg-transparent py-1 text-[12.5px] text-fg outline-none placeholder:text-faint"
            data-testid="workspace-share-emails"
            autoFocus
          />
        </div>
        {parsed.invalid.length > 0 && <div className="mt-1 text-[11.5px] text-danger">Not an e-mail: {parsed.invalid.join(", ")}</div>}
        {offered.length > 0 && (
          <div className="mt-1.5 flex flex-wrap items-center gap-1 text-[11.5px] text-faint">
            <span>Before:</span>
            {offered.map((email) => (
              <button
                key={email}
                type="button"
                className="rounded border border-line px-1.5 text-muted hover:border-accent hover:text-accent"
                onClick={() => setGuests(addGuests(guests, [email]))}
                data-testid="workspace-share-offered"
              >
                {email}
              </button>
            ))}
          </div>
        )}
      </div>
      {teamDomain && (
        <label className="flex items-start gap-2 text-[12.5px] text-fg">
          <input type="checkbox" className="mt-0.5 accent-accent" checked={team} onChange={(event) => setTeam(event.target.checked)} data-testid="workspace-share-team" />
          <span>
            Anyone at {teamDomain}
            <span className="block text-[11.5px] text-faint">Anyone signed in with a Google account at {teamDomain} can open it, as a viewer unless named above.</span>
          </span>
        </label>
      )}
      {shared && state.share && (
        <div className="flex flex-col gap-2 rounded-md border border-line bg-bg p-2.5" data-testid="workspace-share-live">
          <div className="flex items-center gap-2 text-[12px]">
            <LinkIcon size={12} className="shrink-0 text-faint" />
            <span className="min-w-0 flex-1 truncate font-mono text-[11.5px] text-fg selectable" title={state.share.url}>
              {state.share.url}
            </span>
            <IconButton title={copied ? "Copied" : "Copy the link"} onClick={() => void copy()} data-testid="workspace-share-copy">
              {copied ? <Check size={12} /> : <Copy size={12} />}
            </IconButton>
          </div>
          <div className="flex items-center gap-2 text-[11.5px]">
            <span
              className={cx(
                "h-2 w-2 shrink-0 rounded-full",
                connection.tone === "online" ? "bg-success" : connection.tone === "failed" ? "bg-danger" : connection.tone === "busy" ? "bg-working" : "bg-faint",
              )}
            />
            <span className="min-w-0 flex-1 truncate text-muted" title={connection.text}>
              {connection.text}
            </span>
            {state.share.viewers.length > 0 && (
              <Badge tone="accent" title={state.share.viewers.join(", ")}>
                <Eye size={10} /> {state.share.viewers.length === 1 ? state.share.viewers[0] : `${state.share.viewers.length} looking`}
              </Badge>
            )}
          </div>
        </div>
      )}
      <p className="text-[11.5px] text-faint">
        Guests sign in as the address you named and follow the shared chats live, with the workspace’s notes; never its terminals, diff or files. A
        recipient can import an independent copy of the conversations and notes into Chauffeur. A suggestion comes to you above the chat's composer: you send it, edit it or dismiss it. <Kbd>⏎</Kbd> adds an address.
      </p>
    </div>
  );

  return (
    <Modal
      open
      onClose={onClose}
      title={shared ? `${workspaceName(workspace)}, shared` : `Share ${workspaceName(workspace)}`}
      width={560}
      footer={
        state?.configured && state.email ? (
          <div className="flex w-full items-center gap-2">
            {shared && (
              <Button variant="danger" onClick={() => void stop()} busy={busy} data-testid="workspace-share-stop">
                Stop sharing
              </Button>
            )}
            <span className="flex-1" />
            <Button variant="ghost" onClick={onClose}>
              Close
            </Button>
            {shared ? (
              <Button variant="primary" onClick={() => void update()} disabled={!changed || selection?.length === 0} busy={busy} data-testid="workspace-share-update">
                Update
              </Button>
            ) : (
              <Button variant="primary" onClick={() => void share()} disabled={!chatsLoaded || selection?.length === 0 || (all.length === 0 && !team)} busy={busy} data-testid="workspace-share-start">
                Share
              </Button>
            )}
          </div>
        ) : undefined
      }
    >
      {body}
    </Modal>
  );
}
