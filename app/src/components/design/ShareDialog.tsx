// A design chat's Share dialog (story 51), or a document's (story 57): the reviewers named by e-mail (past ones recalled),
// whether anyone at the owner's team domain may open it too (offered only to a Google Workspace
// account), the link once shared, who is looking, the connection, Stop sharing. Sharing needs Chauffeur's Google
// sign-in: without it, the dialog starts with that button. A markdown file's dialog offers to carry the files it links
// to on the same link, listed, checked by default when there are any (docs/adr/share-linked-files.md).

import type { DesignShareState, FilesLinkedResult } from "@chauffeur/protocol";
import { Check, Copy, Eye, Link as LinkIcon, X } from "lucide-react";
import { type KeyboardEvent, useEffect, useMemo, useState } from "react";
import { errorMessage } from "@/lib/rpc";
import { connectionLabel, isShared, parseEmails, suggestions } from "@/lib/share";
import { client, setDesignShare, toast, useApp } from "@/state/app";
import { signIn, useIdentity } from "@/state/identity";
import { Badge, Button, cx, IconButton, Kbd, Modal } from "../ui";

/** What is shared: a design chat, or one document of a workspace. */
export type ShareTarget = { kind: "design"; chat: string } | { kind: "file"; workspace: string; path: string };

export function ShareDialog({ target, state, onClose }: { target: ShareTarget; state: DesignShareState | undefined; onClose: () => void }) {
  const comments = target.kind === "design" || /\.(md|mdx|markdown)$/i.test(target.path);
  const what = target.kind === "design" ? "design" : "file";
  const calls = shareCalls(target);
  const shared = isShared(state);
  const [named, setNamed] = useState<string[]>(() => state?.share?.reviewers ?? []);
  const [team, setTeam] = useState(() => state?.share?.team ?? false);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const signingIn = useIdentity((s) => s.signingIn);
  const teamDomain = useApp((s) => s.identity?.team_domain ?? null);
  const markdownFile = target.kind === "file" && comments;
  const [linkedFiles, setLinkedFiles] = useState<FilesLinkedResult | null>(null);
  const [linked, setLinked] = useState(() => (shared ? (state?.linked ?? false) : true));
  const fileWorkspace = target.kind === "file" ? target.workspace : null;
  const filePath = target.kind === "file" ? target.path : null;
  useEffect(() => {
    if (!markdownFile || !fileWorkspace || !filePath) return;
    let active = true;
    client.call("files.linked", { workspace: fileWorkspace, path: filePath }).then(
      (result) => active && setLinkedFiles(result),
      () => active && setLinkedFiles({ files: [], truncated: false }),
    );
    return () => { active = false; };
  }, [markdownFile, fileWorkspace, filePath]);
  const sharedLinked = shared ? (state?.linked ?? false) : undefined;
  useEffect(() => {
    if (sharedLinked !== undefined) setLinked(sharedLinked);
  }, [sharedLinked]);
  // Offered when the file links to any, or when its share carries them already (to turn it off).
  const offerLinked = markdownFile && ((linkedFiles?.files.length ?? 0) > 0 || sharedLinked === true);
  const carry = offerLinked && linked;
  const signInToShare = async () => {
    if (!await signIn()) return;
    try { setDesignShare(await calls.state()); }
    catch (error) { toast(`Could not read the share: ${errorMessage(error)}`, "error"); }
  };
  // The reviewers follow the share's once it answers (the dialog opened before the state came).
  const reviewers = state?.share?.reviewers;
  useEffect(() => {
    if (reviewers) setNamed(reviewers);
  }, [reviewers]);
  const sharedTeam = state?.share?.team;
  useEffect(() => {
    if (sharedTeam !== undefined) setTeam(sharedTeam);
  }, [sharedTeam]);

  const parsed = useMemo(() => parseEmails(typed), [typed]);
  const all = useMemo(() => [...named, ...parsed.emails.filter((e) => !named.includes(e))], [named, parsed.emails]);
  const offered = suggestions(state, all);
  const changed = shared && (team !== sharedTeam || (offerLinked && linked !== sharedLinked) || all.length !== reviewers?.length || all.some((e) => !reviewers?.includes(e)));
  const connection = connectionLabel(state);

  const takeTyped = () => {
    if (parsed.emails.length === 0) return;
    setNamed(all);
    setTyped(parsed.invalid.join(" "));
  };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter" || event.key === "," || event.key === " ") {
      if (parsed.emails.length > 0) {
        event.preventDefault();
        takeTyped();
      }
    } else if (event.key === "Backspace" && typed === "" && named.length > 0) {
      setNamed(named.slice(0, -1));
    }
  };

  const act = async (what: string, call: () => Promise<DesignShareState>) => {
    setBusy(true);
    try {
      setDesignShare(await call());
    } catch (error) {
      toast(`Could not ${what}: ${errorMessage(error)}`, "error");
    } finally {
      setBusy(false);
    }
  };
  const share = () => act(`share the ${what}`, () => calls.start(all, team, carry));
  const update = () => act("change the share", () => calls.update(all, team, offerLinked ? linked : undefined));
  const stop = () => act("stop sharing", () => calls.stop());
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

  const body = !state?.configured ? (
    <p className="text-[12.5px] text-muted">Sharing is off on this computer: set design_share.url in ~/.chauffeur/settings.toml.</p>
  ) : !state.email ? (
    <div className="flex flex-col gap-3">
      <p className="text-[12.5px] text-muted">Share this {what} live with anyone by e-mail: sign in with your Google account first (your Chauffeur sign-in).</p>
      <Button variant="primary" onClick={() => void signInToShare()} disabled={signingIn} data-testid="share-sign-in">
        {signingIn ? "Finish in your browser…" : "Sign in with Google"}
      </Button>
    </div>
  ) : (
    <div className="flex flex-col gap-3">
      <div>
        <div className="mb-1 text-[11.5px] font-medium text-muted">Reviewers</div>
        <div
          className="flex min-h-9 flex-wrap items-center gap-1 rounded-md border border-line-strong bg-bg px-2 py-1 focus-within:border-accent/60"
          onClick={(event) => (event.currentTarget.querySelector("input") as HTMLInputElement | null)?.focus()}
        >
          {named.map((email) => (
            <span key={email} className="inline-flex items-center gap-1 rounded bg-raised px-1.5 text-[12px] text-fg" data-testid="share-reviewer">
              {email}
              <button type="button" className="text-faint hover:text-fg" onClick={() => setNamed(named.filter((e) => e !== email))} aria-label={`Remove ${email}`}>
                <X size={11} />
              </button>
            </span>
          ))}
          <input
            value={typed}
            onChange={(event) => setTyped(event.target.value)}
            onKeyDown={onKeyDown}
            onBlur={takeTyped}
            placeholder={named.length === 0 ? "alice@example.com, bob@example.com" : ""}
            className="min-w-[160px] flex-1 bg-transparent py-1 text-[12.5px] text-fg outline-none placeholder:text-faint"
            data-testid="share-emails"
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
                onClick={() => setNamed([...named, email])}
                data-testid="share-suggestion"
              >
                {email}
              </button>
            ))}
          </div>
        )}
      </div>
      {teamDomain && (
        <label className="flex items-start gap-2 text-[12.5px] text-fg">
          <input type="checkbox" className="mt-0.5 accent-accent" checked={team} onChange={(event) => setTeam(event.target.checked)} data-testid="share-team" />
          <span>
            Anyone at {teamDomain}
            <span className="block text-[11.5px] text-faint">Anyone signed in with a Google account at {teamDomain} can open it.</span>
          </span>
        </label>
      )}
      {offerLinked && linkedFiles && (
        <div>
          <label className="flex items-start gap-2 text-[12.5px] text-fg">
            <input type="checkbox" className="mt-0.5 accent-accent" checked={linked} onChange={(event) => setLinked(event.target.checked)} data-testid="share-linked" />
            <span>
              {linkedFiles.files.length === 1 ? "Also share the linked file" : `Also share the ${linkedFiles.files.length}${linkedFiles.truncated ? "+" : ""} linked files`}
              <span className="block text-[11.5px] text-faint">On the same link: reviewers open them from this file, and comment on them too. Files linked later join it.</span>
            </span>
          </label>
          {linkedFiles.files.length > 0 && (
            <ul className={cx("ml-6 mt-1.5 max-h-28 overflow-auto rounded-md border border-line bg-bg px-2 py-1", !linked && "opacity-50")} data-testid="share-linked-files">
              {linkedFiles.files.map((path) => (
                <li key={path} className="truncate font-mono text-[11.5px] text-muted" title={path}>
                  {path}
                </li>
              ))}
            </ul>
          )}
          {linkedFiles.truncated && <div className="ml-6 mt-1 text-[11.5px] text-faint">Only the first {linkedFiles.files.length} travel.</div>}
        </div>
      )}
      {shared && state.share && (
        <div className="flex flex-col gap-2 rounded-md border border-line bg-bg p-2.5" data-testid="share-live">
          <div className="flex items-center gap-2 text-[12px]">
            <LinkIcon size={12} className="shrink-0 text-faint" />
            <span className="min-w-0 flex-1 truncate font-mono text-[11.5px] text-fg selectable" title={state.share.url}>
              {state.share.url}
            </span>
            <IconButton title={copied ? "Copied" : "Copy the link"} onClick={() => void copy()} data-testid="share-copy">
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
        Reviewers sign in as the address you named, see the {target.kind === "design" ? "canvas" : "file"} live{comments ? " and comment; their comments land here, in Comments" : ""}. <Kbd>⏎</Kbd> adds an address.
      </p>
    </div>
  );

  return (
    <Modal
      open
      onClose={onClose}
      title={shared ? `Shared ${what}` : `Share this ${what}`}
      width={520}
      footer={
        state?.configured && state.email ? (
          <div className="flex w-full items-center gap-2">
            {shared && (
              <Button variant="danger" onClick={() => void stop()} busy={busy} data-testid="share-stop">
                Stop sharing
              </Button>
            )}
            <span className="flex-1" />
            <Button variant="ghost" onClick={onClose}>
              Close
            </Button>
            {shared ? (
              <Button variant="primary" onClick={() => void update()} disabled={!changed} busy={busy} data-testid="share-update">
                Update
              </Button>
            ) : (
              <Button variant="primary" onClick={() => void share()} disabled={all.length === 0 && !team} busy={busy} data-testid="share-start">
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

/** The daemon's calls for a target's share. */
function shareCalls(target: ShareTarget) {
  if (target.kind === "design") {
    const { chat } = target;
    return {
      state: () => client.call("design.share_state", { chat }),
      start: (reviewers: string[], team: boolean, _linked?: boolean) => client.call("design.share_start", { chat, reviewers, team }),
      update: (reviewers: string[], team: boolean, _linked?: boolean) => client.call("design.share_update", { chat, reviewers, team }),
      stop: () => client.call("design.share_stop", { chat }),
    };
  }
  const file = { workspace: target.workspace, path: target.path };
  return {
    state: () => client.call("files.share_state", file),
    start: (reviewers: string[], team: boolean, linked?: boolean) => client.call("files.share_start", { ...file, reviewers, team, linked }),
    update: (reviewers: string[], team: boolean, linked?: boolean) => client.call("files.share_update", { ...file, reviewers, team, linked }),
    stop: () => client.call("files.share_stop", file),
  };
}
