// The reviewer page of a design share (story 51): the share's canvas as the app shows it, read
// from the share service and kept live over its reviewer socket; Comment mode and the threads,
// who is looking, Live or when the design was last pushed. No chat, no export, nothing to the
// agent: a reviewer's comments reach the owner's daemon through the service.

import type { CanvasResult, CommentAuthor, DesignComment, FromReviewer, Mention, ShareSnapshot, ToReviewer } from "@chauffeur/protocol";
import { AtSign, Copy, Download, Lock, LogOut, Mail, MessageSquarePlus, MessageSquareText, Play, Scan, UserRound, Users, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { layoutCanvas, noScreens, pageSections, percent, presentOrder, ZOOM_STEPS } from "@/lib/canvas";
import { counts as commentCounts, type Draft, threadsOf } from "@/lib/comments";
import { mentions as mentionsMe, threadParam, unreadThreads } from "@/lib/mentions";
import { flashFor, initials, liveLabel, noAccessText, refusalText, sharedWithYou, shareIdOf, signInTokenOf, socketUrl } from "@/lib/sharePage";
import { parseEmails } from "@/lib/share";
import { FullScreenSection, PresentScreens } from "@/components/design/CanvasOverlays";
import { CanvasViewport } from "@/components/design/CanvasViewport";
import { CommentComposer, CommentsPanel, type Filter, ThreadView } from "@/components/design/CommentsPanel";
import type { MentionSetup } from "@/components/design/Mentions";
import { useCanvasView } from "@/components/design/useCanvasView";
import type { ScreenCommenting } from "@/components/design/ScreenComments";
import { usePins } from "@/components/design/usePins";
import { type CommentActions, MarkdownComments } from "@/components/MarkdownComments";
import { Button, cx, Select, type SelectOption, Spinner } from "@/components/ui";
import { api, type ServiceConfig, ServiceError, type ShareKind } from "./api";
import { renderSignIn } from "./google";
import icon from "./icon.png";
import { ReturnToReview } from "./mobileAuth";
import { MobileReview } from "./MobileReview";
import "./mobileReview.css";

/** How long a screen that changed keeps its outline. */
const FLASH_MS = 1000;
const RECONNECT_MIN_MS = 1000;
const RECONNECT_MAX_MS = 15_000;
const NO_PUBLISHED: Record<string, string> = {};

type Phase<T> =
  | { kind: "loading" }
  | { kind: "offline" }
  | { kind: "sign-in"; config: ServiceConfig | null; owner: string | null; error: string | null }
  | { kind: "no-access"; email: string }
  | { kind: "refused"; text: string }
  | { kind: "open"; me: CommentAuthor; snapshot: T };

/** What a share's gate says of its kind of share: the brand's word, and which share it is. */
export type GateWords = { brand: string; kind: ShareKind };

const DESIGN_WORDS: GateWords = { brand: "Design", kind: "design" };

const failure = (error: unknown) => (error instanceof Error ? error.message : String(error));
const statusOf = (error: unknown) => (error instanceof ServiceError ? error.status : 0);

/**
 * A share's page before it opens: who the reader is (the session cookie, a sign-in link of the
 * address, else Google or a sign-in link to ask for), the share read (`load`), or why it is
 * refused. `children` renders the open share.
 */
export function ShareGate<T>({
  id,
  load,
  words,
  children,
}: {
  id: string | null;
  load: (id: string) => Promise<T>;
  words: GateWords;
  children: (open: { id: string; me: CommentAuthor; first: T; onRefused: (text: string) => void; onSignedOut: () => void }) => React.ReactNode;
}) {
  const [phase, setPhase] = useState<Phase<T>>({ kind: "loading" });

  const open = useCallback(
    async (linkError: string | null = null) => {
      if (!id) {
        setPhase({ kind: "refused", text: "There is no such share." });
        return;
      }
      const signIn = async (error: string | null) => {
        let owner: string | null = null;
        try {
          owner = (await api.about(words.kind, id)).owner;
        } catch (e) {
          if ([404, 410].includes(statusOf(e))) {
            setPhase({ kind: "refused", text: refusalText(statusOf(e), failure(e)) });
            return;
          }
        }
        setPhase({ kind: "sign-in", config: await api.config().catch(() => null), owner, error });
      };
      let error = linkError;
      const token = signInTokenOf(location.hash);
      if (token) {
        // The fragment goes at once: a reload or a copied address never carries the token.
        history.replaceState(null, "", location.pathname + location.search);
        await api.sessionFromLink(token).catch((e: unknown) => (error = failure(e)));
      }
      let me: CommentAuthor;
      try {
        me = await api.me();
      } catch (e) {
        if (statusOf(e) === 401) await signIn(error);
        else if (statusOf(e) === 0) setPhase({ kind: "offline" });
        else setPhase({ kind: "refused", text: refusalText(statusOf(e), failure(e)) });
        return;
      }
      try {
        setPhase({ kind: "open", me, snapshot: await load(id) });
      } catch (e) {
        const status = statusOf(e);
        if (status === 401) await signIn(null);
        else if (status === 403) setPhase({ kind: "no-access", email: me.email });
        else if (status === 0) setPhase({ kind: "offline" });
        else setPhase({ kind: "refused", text: refusalText(status, failure(e)) });
      }
    },
    [id, load, words.kind],
  );
  useEffect(() => void open(), [open]);
  const onRefused = useCallback((text: string) => setPhase({ kind: "refused", text }), []);
  const onSignedOut = useCallback(() => void open(), [open]);
  const onToken = useCallback(
    async (token: string) => {
      try {
        await api.session(token);
        await open();
      } catch (error) {
        setPhase((p) => (p.kind === "sign-in" ? { ...p, error: failure(error) } : p));
      }
    },
    [open],
  );

  if (phase.kind === "loading") {
    return (
      <Shell brand={words.brand}>
        <Backdrop>
          <Card icon={<Spinner size={18} />} title="Opening shared design" testId="share-opening"><p className="text-[13px] text-muted">Checking your access{new URLSearchParams(location.search).get("screen") ? ` to ${new URLSearchParams(location.search).get("screen")}` : " to the linked design"}. Your screen and comment destination will be kept.</p></Card>
        </Backdrop>
      </Shell>
    );
  }
  if (phase.kind === "offline") return <Shell brand={words.brand}><Backdrop><Card icon={<Lock size={18} />} title="You’re offline" testId="share-offline"><p className="text-[13px] text-muted">Connect to the internet to open this shared design. The link’s destination is kept for your return.</p><Button onClick={() => { setPhase({ kind: "loading" }); void open(); }}>Try again</Button></Card></Backdrop></Shell>;
  if (phase.kind === "sign-in") {
    return (
      <Shell brand={words.brand}>
        <Backdrop>
          <SignIn id={id!} kind={words.kind} config={phase.config} owner={phase.owner} error={phase.error} onToken={onToken} />
        </Backdrop>
      </Shell>
    );
  }
  if (phase.kind === "no-access") {
    return (
      <Shell brand={words.brand}>
        <Backdrop>
          <Card icon={<Lock size={18} />} title="This account doesn’t have access" testId="share-no-access"><p className="text-[13px] text-muted">{noAccessText(phase.email)} Ask the owner to invite this address, or use another account.</p>
            <Button size="sm" variant="secondary" className="self-center" onClick={() => void api.signOut().then(() => open(), () => open())} data-testid="share-switch-account">
              <UserRound size={12} /> Use another account
            </Button>
          </Card>
        </Backdrop>
      </Shell>
    );
  }
  if (phase.kind === "refused") {
    return (
      <Shell brand={words.brand}>
        <Backdrop>
          <Card icon={<Lock size={18} />} title={phase.text === "This share was stopped." ? "This design is no longer shared" : phase.text} testId="share-refused">{phase.text === "This share was stopped." && <p className="text-[13px] text-muted">The owner stopped sharing it. Existing comments remain with the design owner.</p>}<Button onClick={() => void open()}>Try again</Button></Card>
        </Backdrop>
      </Shell>
    );
  }
  return <>{children({ id: id!, me: phase.me, first: phase.snapshot, onRefused, onSignedOut })}</>;
}

export function SharePage() {
  const id = useMemo(() => shareIdOf(location.pathname), []);
  return (
    <ShareGate id={id} load={api.snapshot} words={DESIGN_WORDS}>
      {(open) => new URLSearchParams(location.search).get("mobile-auth") === "1" ? <ReturnToReview id={open.id} title={open.first.title} me={open.me} /> : <OpenShare {...open} />}
    </ShareGate>
  );
}

function Shell({ brand, children }: { brand: string; children: React.ReactNode }) {
  return (
    <div className="share-gate flex h-dvh w-screen flex-col bg-bg text-fg">
      <Brand word={brand} />
      {children}
    </div>
  );
}

/** The page's bar before a share shows: the app's icon and name, as its title bar. */
function Brand({ word }: { word: string }) {
  return (
    <div className="flex h-10 shrink-0 items-center gap-2 border-b border-line px-3">
      <img src={icon} alt="" width={18} height={18} className="rounded-[4px]" />
      <span className="text-[13px] font-semibold">Chauffeur</span>
      <span className="text-[13px] text-faint">{word}</span>
    </div>
  );
}

/** The canvas's dot grid behind a card, so that the page looks like the app before the design shows. */
function Backdrop({ children }: { children: React.ReactNode }) {
  return (
    <div
      className="share-gate-backdrop flex flex-1 items-center justify-center p-6"
      style={{ backgroundImage: "radial-gradient(circle, var(--color-line-strong) 1px, transparent 1.3px)", backgroundSize: "24px 24px" }}
    >
      {children}
    </div>
  );
}

/** A panel as the app's (the Office's presence, the Share dialog): an icon, a title, its body. */
function Card({ icon, title, children, testId }: { icon: React.ReactNode; title: string; children?: React.ReactNode; testId?: string }) {
  return (
    <div className="flex w-full max-w-[360px] flex-col items-center gap-3 rounded-lg border border-line bg-raised/95 p-5 text-center shadow-xl" data-testid={testId}>
      <div className="flex h-10 w-10 items-center justify-center rounded-full bg-panel text-accent ring-1 ring-line-strong">{icon}</div>
      <div className="text-[14px] font-semibold text-fg">{title}</div>
      {children}
    </div>
  );
}

/** Google's button's width, the card's inner width. */
const SIGN_IN_WIDTH = 280;

/**
 * The card a signed-out reader sees: who shared it, "Continue with Google", then "Email me a
 * sign-in link" for an address without a Google account; "Shared with Chauffeur" at its foot.
 */
function SignIn({
  id,
  kind,
  config,
  owner,
  error,
  onToken,
}: {
  id: string;
  kind: ShareKind;
  config: ServiceConfig | null;
  owner: string | null;
  error: string | null;
  onToken: (token: string) => void;
}) {
  const button = useRef<HTMLDivElement>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [browserContinue, setBrowserContinue] = useState(() => kind !== "design" || new URLSearchParams(location.search).get("mobile-auth") === "1" || !matchMedia("(max-width: 700px), (pointer: coarse)").matches);
  const [devEmail, setDevEmail] = useState("");
  const [link, setLink] = useState<{ step: "closed" | "asking" | "sending" } | { step: "sent"; message: string }>({ step: "closed" });
  const [email, setEmail] = useState("");
  const [linkError, setLinkError] = useState<string | null>(null);
  const webClientId = config?.web_client_id ?? null;
  useEffect(() => {
    if (!browserContinue || new URLSearchParams(location.search).get("native") === "1" || !webClientId || !button.current) return;
    renderSignIn(button.current, webClientId, SIGN_IN_WIDTH, onToken).catch((e: unknown) => setLoadError(failure(e)));
  }, [webClientId, onToken, link.step, browserContinue]);

  const askLink = async () => {
    setLink({ step: "sending" });
    setLinkError(null);
    try {
      const { message } = await api.askSignInLink(kind, id, email.trim());
      setLink({ step: "sent", message });
    } catch (e) {
      setLinkError(failure(e));
      setLink({ step: "asking" });
    }
  };

  const shownError = linkError || error || loadError;
  const openInApp = `chauffeur://review?url=${encodeURIComponent(location.href)}`;
  if (new URLSearchParams(location.search).get("native") === "1") return <Card icon={<Lock size={22} />} title="Sign in to review" testId="share-sign-in"><p className="text-[14px] text-muted">Use the address invited to this design. Pairing a Mac is not required.</p><a className="review-primary" href={`/mobile-sign-in${location.search}`}>Continue to sign in</a></Card>;
  if (!browserContinue) return <Card icon={<img src={icon} alt="" width={22} height={22} className="rounded-[5px]" />} title={sharedWithYou(owner, kind)} testId="share-link"><p className="text-[14px] text-muted">Review the screens and leave comments where they belong. Use the address invited to this design.</p><a className="review-primary" href={openInApp}>Open in Chauffeur</a><Button variant="secondary" onClick={() => setBrowserContinue(true)}>Continue in browser</Button></Card>;
  return (
    <Card icon={<img src={icon} alt="" width={22} height={22} className="rounded-[5px]" />} title={sharedWithYou(owner, kind)} testId="share-sign-in">
      {kind === "design" && matchMedia("(max-width: 700px), (pointer: coarse)").matches && new URLSearchParams(location.search).get("mobile-auth") !== "1" && <a className="review-primary" href={openInApp}>Open in Chauffeur</a>}
      {link.step === "sent" ? (
        <div className="flex flex-col gap-1" data-testid="share-link-sent">
          <div className="text-[13px] font-semibold text-fg">Check your inbox</div>
          <p className="text-[12.5px] text-muted">{link.message}</p>
        </div>
      ) : (
        <div className="flex w-full max-w-[280px] flex-col items-stretch gap-2">
          {webClientId ? (
            // Google's button is an iframe of a light page: under the app's dark color-scheme the
            // browser paints it an opaque white backdrop, so its container stays light.
            <div ref={button} className="flex min-h-10 justify-center [color-scheme:light]" />
          ) : (
            // A development server (--insecure-dev) takes an e-mail in place of Google.
            <form
              className="flex w-full items-center gap-2"
              onSubmit={(event) => {
                event.preventDefault();
                if (devEmail.trim()) onToken(`dev:${devEmail.trim()}`);
              }}
            >
              <input
                className="h-8 min-w-0 flex-1 rounded-md border border-line-strong bg-bg px-2 text-[12.5px] outline-none focus:border-accent"
                placeholder="you@example.com (development)"
                value={devEmail}
                onChange={(event) => setDevEmail(event.target.value)}
                data-testid="share-dev-email"
              />
              <Button size="sm" variant="primary" type="submit" data-testid="share-dev-sign-in">
                Sign in
              </Button>
            </form>
          )}
          {link.step === "closed" ? (
            <Button size="sm" variant="secondary" onClick={() => setLink({ step: "asking" })} data-testid="share-ask-link">
              <Mail size={12} /> Email me a sign-in link
            </Button>
          ) : (
            <form
              className="flex w-full items-center gap-2"
              onSubmit={(event) => {
                event.preventDefault();
                if (email.trim()) void askLink();
              }}
            >
              <input
                type="email"
                autoFocus
                className="h-8 min-w-0 flex-1 rounded-md border border-line-strong bg-bg px-2 text-[12.5px] outline-none focus:border-accent"
                placeholder="Your e-mail address"
                value={email}
                onChange={(event) => setEmail(event.target.value)}
                data-testid="share-link-email"
              />
              <Button size="sm" variant="secondary" type="submit" disabled={link.step === "sending" || !email.trim()} data-testid="share-send-link">
                Send link
              </Button>
            </form>
          )}
        </div>
      )}
      {shownError && <div className="text-[12px] text-danger selectable">{shownError}</div>}
      <p className="text-[11.5px] text-faint">
        {config?.site_url ? (
          <a href={config.site_url} target="_blank" rel="noreferrer" className="hover:text-muted">
            Shared with Chauffeur
          </a>
        ) : (
          "Shared with Chauffeur"
        )}
      </p>
    </Card>
  );
}

/** How a reviewer takes the design over, in their own Chauffeur: the app's menu, or the command. */
function TakeOverPanel({ url, onClose }: { url: string; onClose: () => void }) {
  const command = `chauffeur import-design <workspace> ${url}`;
  const [copied, setCopied] = useState<"link" | "command" | null>(null);
  const copy = (what: "link" | "command", text: string) => {
    navigator.clipboard.writeText(text).then(
      () => {
        setCopied(what);
        setTimeout(() => setCopied(null), 1500);
      },
      () => setCopied(null),
    );
  };
  return (
    <div
      role="dialog"
      aria-label="Take over"
      data-canvas-popover
      className="absolute right-0 top-full z-30 mt-1 flex w-[380px] max-w-[calc(100vw-24px)] flex-col gap-2 rounded-lg border border-line bg-raised/95 p-3 shadow-xl"
      data-testid="share-take-over"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          onClose();
        }
      }}
    >
      <div className="flex items-center gap-2">
        <span className="font-semibold text-fg">Take over in Chauffeur</span>
        <IconButtonClose onClick={onClose} />
      </div>
      <p className="text-[12.5px] text-muted">Its screens and comments land in a design chat of your own, with your own agent; this share stays as it is.</p>
      <div className="text-[12px] text-fg">
        In Chauffeur: a workspace's <span className="rounded bg-bg px-1">+</span> menu → <span className="font-medium">Import a shared design…</span>, and paste this link.
      </div>
      <div className="flex items-center gap-2">
        <code className="min-w-0 flex-1 truncate rounded-md bg-bg px-2 py-1 text-[11.5px] text-muted" title={url}>
          {url}
        </code>
        <Button size="sm" variant="secondary" onClick={() => copy("link", url)} title="Copy the link" data-testid="share-copy-link">
          <Copy size={12} /> {copied === "link" ? "Copied" : "Copy"}
        </Button>
      </div>
      <div className="text-[12px] text-fg">Or in a terminal:</div>
      <div className="flex items-center gap-2">
        <code className="min-w-0 flex-1 truncate rounded-md bg-bg px-2 py-1 text-[11.5px] text-muted" title={command}>
          {command}
        </code>
        <Button size="sm" variant="secondary" onClick={() => copy("command", command)} title="Copy the command" data-testid="share-copy-command">
          <Copy size={12} /> {copied === "command" ? "Copied" : "Copy"}
        </Button>
      </div>
    </div>
  );
}

/** The owner's list, as the app's Share dialog: chips with a cross, a box to add, who is looking. */
function ReviewersPanel({ reviewers, viewers, onChange, onClose }: { reviewers: string[]; viewers: string[]; onChange: (reviewers: string[]) => void; onClose: () => void }) {
  const [typed, setTyped] = useState("");
  const [invalid, setInvalid] = useState<string[]>([]);
  const add = () => {
    const parsed = parseEmails(typed);
    setInvalid(parsed.invalid);
    if (parsed.emails.length === 0) return;
    onChange([...reviewers, ...parsed.emails.filter((e) => !reviewers.includes(e))]);
    setTyped("");
  };
  return (
    <div
      role="dialog"
      aria-label="Shared with"
      data-canvas-popover
      className="absolute right-0 top-full z-30 mt-1 flex w-[340px] max-w-[calc(100vw-24px)] flex-col gap-2 rounded-lg border border-line bg-raised/95 p-3 shadow-xl"
      data-testid="share-reviewers"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          onClose();
        }
      }}
    >
      <div className="flex items-center gap-2">
        <span className="font-semibold text-fg">Shared with</span>
        <span className="text-[11.5px] text-faint">work Google accounts</span>
        <IconButtonClose onClick={onClose} />
      </div>
      <div className="flex flex-col gap-1">
        {reviewers.length === 0 && <div className="text-[12px] text-faint">Nobody yet: only you can open this link.</div>}
        {reviewers.map((email) => (
          <div key={email} className="flex items-center gap-2 rounded-md bg-bg px-2 py-1 text-[12.5px]" data-testid="share-reviewer">
            <span className={cx("h-1.5 w-1.5 shrink-0 rounded-full", viewers.includes(email) ? "bg-success" : "bg-faint")} title={viewers.includes(email) ? "Looking now" : "Not looking now"} />
            <span className="min-w-0 flex-1 truncate">{email}</span>
            <button type="button" className="text-faint hover:text-danger" onClick={() => onChange(reviewers.filter((e) => e !== email))} aria-label={`Remove ${email}`} title="Remove: they lose access at once">
              <X size={12} />
            </button>
          </div>
        ))}
      </div>
      <form
        className="flex items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          add();
        }}
      >
        <input
          className="h-8 min-w-0 flex-1 rounded-md border border-line-strong bg-bg px-2 text-[12.5px] outline-none focus:border-accent"
          placeholder="alice@example.com, bob@example.com"
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          autoFocus
          data-testid="share-reviewer-input"
        />
        <Button size="sm" variant="primary" type="submit" disabled={!typed.trim()} data-testid="share-reviewer-add">
          Add
        </Button>
      </form>
      {invalid.length > 0 && <div className="text-[12px] text-danger">Not an e-mail: {invalid.join(", ")}</div>}
      <div className="text-[11.5px] text-faint">Someone added opens the link at once; someone removed loses it at once. The app's Share dialog shows the same list.</div>
    </div>
  );
}

/** Who the share is open to, for a reviewer or a guest: the owner and the invitees, names and e-mails. */
export function PeoplePanel({ people, owner, viewers, onClose }: { people: CommentAuthor[]; owner: string; viewers: string[]; onClose: () => void }) {
  return (
    <div
      role="dialog"
      aria-label="Shared with"
      data-canvas-popover
      className="absolute right-0 top-full z-30 mt-1 flex w-[320px] max-w-[calc(100vw-24px)] flex-col gap-2 rounded-lg border border-line bg-raised/95 p-3 shadow-xl"
      data-testid="share-people"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          onClose();
        }
      }}
    >
      <div className="flex items-center gap-2">
        <span className="font-semibold text-fg">Shared with</span>
        <IconButtonClose onClick={onClose} />
      </div>
      <div className="flex flex-col gap-1">
        {people.map((person) => (
          <div key={person.email} className="flex items-center gap-2 rounded-md bg-bg px-2 py-1 text-[12.5px]" data-testid="share-person">
            <span className={cx("h-1.5 w-1.5 shrink-0 rounded-full", viewers.includes(person.email) ? "bg-success" : "bg-faint")} title={viewers.includes(person.email) ? "Looking now" : "Not looking now"} />
            <span className="font-medium text-fg">{person.name}</span>
            <span className="min-w-0 flex-1 truncate text-faint">{person.email}</span>
            {person.email === owner && <span className="shrink-0 text-[10.5px] text-faint">owner</span>}
          </div>
        ))}
      </div>
      <div className="text-[11.5px] text-faint">Type @ in a comment to mention one of them: they are told in Chauffeur.</div>
    </div>
  );
}

function IconButtonClose({ onClick }: { onClick: () => void }) {
  return (
    <button type="button" className="ml-auto rounded p-0.5 text-faint hover:bg-hover hover:text-fg" onClick={onClick} title="Close (Esc)" aria-label="Close">
      <X size={14} />
    </button>
  );
}

type Live = { canvas: CanvasResult; comments: DesignComment[]; viewers: string[]; online: boolean; lastPushAt: number | undefined; reviewers: string[]; people: CommentAuthor[]; teamDomain: string | null; unread: string[] };

/** What a snapshot sets of the page's live state. */
function liveOf(snapshot: ShareSnapshot): Live {
  return {
    canvas: snapshot.canvas,
    comments: snapshot.comments,
    viewers: snapshot.viewers,
    online: snapshot.online,
    lastPushAt: snapshot.last_push_at,
    reviewers: snapshot.reviewers,
    people: snapshot.people,
    teamDomain: snapshot.team_domain ?? null,
    unread: snapshot.unread_mentions,
  };
}

function OpenShare({
  id,
  me,
  first,
  onRefused,
  onSignedOut,
}: {
  id: string;
  me: CommentAuthor;
  first: ShareSnapshot;
  onRefused: (text: string) => void;
  onSignedOut: () => void;
}) {
  const [live, setLive] = useState<Live>(() => liveOf(first));
  // A markdown file's share (story 57): the file rendered with its passages' comments, no canvas.
  const fileName = first.document ?? null;
  const [documentText, setDocumentText] = useState<string | null>(null);
  const [focusThread, setFocusThread] = useState<{ id: string; at: number } | null>(null);
  // Its Comments column shows until the reviewer closes it (a passage clicked opens it again);
  // closed at first on a phone, where it would leave the file no room.
  const [documentPanel, setDocumentPanel] = useState(() => !matchMedia("(max-width: 700px)").matches);
  const liveRef = useRef(live);
  liveRef.current = live;
  const owner = me.email === first.owner.email;
  const [reviewersOpen, setReviewersOpen] = useState(false);
  const [peopleOpen, setPeopleOpen] = useState(false);
  const [takeOverOpen, setTakeOverOpen] = useState(false);
  const [connected, setConnected] = useState(false);
  const [snapshotEpoch, setSnapshotEpoch] = useState(0);
  const [flash, setFlash] = useState<Record<string, number>>({});
  const [toast, setToast] = useState<string | null>(null);
  const socket = useRef<WebSocket | null>(null);
  const [, tick] = useState(0);
  // "Last update 2h ago" moves with the clock.
  useEffect(() => {
    const timer = setInterval(() => tick((n) => n + 1), 30_000);
    return () => clearInterval(timer);
  }, []);
  // Renew the screens' 15-minute read leases through the authenticated page. Their URLs stay
  // stable while renewed; returning after sleep replaces an expired lease and reloads the frames.
  const readDocument = useCallback(() => {
    if (!fileName) return;
    api.document(id, fileName).then(setDocumentText, (error: unknown) => {
      if (error instanceof ServiceError && error.status === 401) onSignedOut();
      else setToast("Could not read the file; check your connection or reload the page.");
    });
  }, [id, fileName, onSignedOut]);
  useEffect(readDocument, [readDocument]);
  useEffect(() => {
    if (fileName) return;
    let closed = false;
    const renew = () => {
      if (window.document.visibilityState === "hidden") return;
      api.canvas(id).then((canvas) => {
        if (!closed) setLive((current) => ({ ...current, canvas }));
      }).catch((error: unknown) => {
        if (closed) return;
        if (error instanceof ServiceError && error.status === 401) onSignedOut();
        else if (error instanceof ServiceError && [403, 404, 410].includes(error.status)) onRefused(error.message);
        else setToast("Could not refresh the screens; check your connection or reload the shared design.");
      });
    };
    const timer = setInterval(renew, 5 * 60_000);
    window.document.addEventListener("visibilitychange", renew);
    return () => {
      closed = true;
      clearInterval(timer);
      window.document.removeEventListener("visibilitychange", renew);
    };
  }, [id, fileName, onRefused, onSignedOut]);
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), 4000);
    return () => clearTimeout(timer);
  }, [toast]);

  const flashScreens = useCallback((changed: string[]) => {
    const stamp = Date.now();
    const files = flashFor(changed, stamp);
    if (Object.keys(files).length === 0) return;
    setFlash((current) => ({ ...current, ...files }));
    setTimeout(() => setFlash((current) => Object.fromEntries(Object.entries(current).filter(([, at]) => at !== stamp))), FLASH_MS);
  }, []);

  // The reviewer socket: the snapshot again at each (re)connection, then what changes; reconnects
  // with backoff until the share refuses the reviewer.
  useEffect(() => {
    let closed = false;
    let delay = RECONNECT_MIN_MS;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let active: WebSocket | null = null;
    const connect = () => {
      const ws = new WebSocket(socketUrl(location, id));
      socket.current = ws;
      active = ws;
      ws.onopen = () => {
        if (closed || socket.current !== ws) return;
        setConnected(true);
        delay = RECONNECT_MIN_MS;
      };
      ws.onmessage = (event) => {
        if (closed || socket.current !== ws) return;
        let message: ToReviewer;
        try {
          message = JSON.parse(String(event.data)) as ToReviewer;
        } catch {
          return;
        }
        switch (message.type) {
          case "snapshot":
            setSnapshotEpoch(epoch => epoch + 1);
            setLive(liveOf(message.snapshot));
            // Again at each (re)connection: the file may have changed while the page was away.
            readDocument();
            break;
          case "files":
            if (fileName) {
              readDocument();
              break;
            }
            api
              .canvas(id)
              .then((canvas) => {
                setLive((l) => ({ ...l, canvas }));
                flashScreens(message.changed);
              })
              .catch(() => {});
            break;
          case "comment": {
            const comment = message.comment;
            // A comment that mentions the reader for the first time: told, and unread until opened.
            const known = liveRef.current.comments.some((c) => c.id === comment.id && mentionsMe(c, me.email));
            const fresh = !known && comment.author.email !== me.email && mentionsMe(comment, me.email);
            if (fresh) setToast(`${comment.author.name} mentioned you`);
            setLive((l) => ({
              ...l,
              comments: [...l.comments.filter((c) => c.id !== comment.id), comment],
              unread: fresh && !l.unread.includes(comment.id) ? [...l.unread, comment.id] : l.unread,
            }));
            break;
          }
          case "comment_removed":
            setLive((l) => ({ ...l, comments: l.comments.filter((c) => c.id !== message.id) }));
            break;
          case "viewers":
            setLive((l) => ({ ...l, viewers: message.viewers }));
            break;
          case "live":
            setLive((l) => ({ ...l, online: message.online, lastPushAt: message.last_push_at ?? l.lastPushAt }));
            break;
          case "reviewers":
            setLive((l) => ({ ...l, reviewers: message.reviewers }));
            break;
          case "people":
            setLive((l) => ({ ...l, people: message.people }));
            break;
          case "error":
            setToast(message.reason);
            break;
          case "refused":
            closed = true;
            onRefused(message.reason);
            break;
        }
      };
      ws.onclose = () => {
        if (socket.current !== ws) return;
        setConnected(false);
        socket.current = null;
        if (closed) return;
        timer = setTimeout(connect, delay);
        delay = Math.min(delay * 2, RECONNECT_MAX_MS);
      };
    };
    connect();
    return () => {
      closed = true;
      if (timer) clearTimeout(timer);
      active?.close();
    };
  }, [id, me.email, onRefused, flashScreens, fileName, readDocument]);

  /** `quiet`: no toast when not connected (a thread read is said again once the snapshot comes). */
  const say = (message: FromReviewer, quiet = false) => {
    const ws = socket.current;
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      if (!quiet) setToast("Not connected to the share: your comment was not sent. It reconnects on its own.");
      return false;
    }
    ws.send(JSON.stringify(message));
    return true;
  };

  // --- the canvas ---
  const [page, setPage] = useState(0);
  const pages = live.canvas.pages;
  const pageIndex = Math.min(page, Math.max(0, pages.length - 1));
  const sections = useMemo(() => pageSections(live.canvas, pageIndex), [live.canvas, pageIndex]);
  const layout = useMemo(() => layoutCanvas(sections), [sections]);
  const steps = useMemo(() => presentOrder(sections), [sections]);
  const [selection, setSelection] = useState<string[]>([]);
  const { zoom, area, setArea, moveView, fit, zoomTo } = useCanvasView(layout, pageIndex);
  const [fullScreen, setFullScreen] = useState<number | null>(null);
  const [presenting, setPresenting] = useState<number | null>(null);
  const empty = noScreens(live.canvas);

  // --- comments ---
  const threads = useMemo(() => threadsOf(live.comments), [live.comments]);
  const counts = useMemo(() => commentCounts(live.comments), [live.comments]);
  const { onFrame, onOverlayFrame, pins, stale, elementUnder } = usePins(threads);
  const [commentMode, setCommentMode] = useState(false);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [openThread, setOpenThread] = useState<string | null>(null);
  const [panel, setPanel] = useState(false);
  const [filter, setFilter] = useState<Filter>("open");
  const shownThread = openThread ? (threads.find((t) => t.comment.id === openThread) ?? null) : null;
  const labelOf = useCallback(
    (file: string) => {
      for (const section of sections) for (const screen of section.screens) if (screen.file === file) return screen.label || file;
      return file;
    },
    [sections],
  );
  const startComment = (file: string, x: number, y: number, whole: boolean, frame?: HTMLIFrameElement | null) => {
    setOpenThread(null);
    setDraft({ file, x, y, whole, element: null });
    if (!whole) void elementUnder(file, x, y, frame).then((element) => setDraft((d) => (d && d.file === file && d.x === x && d.y === y ? { ...d, element } : d)));
  };
  // Mentions (docs/adr/mentions-in-share-comments.md): among the people the share is open to; the
  // owner may invite someone else in the same stroke.
  const mentionSetup = useMemo<MentionSetup>(() => ({ people: live.people, me: me.email, teamDomain: live.teamDomain, canInvite: owner }), [live.people, live.teamDomain, me.email, owner]);
  /** The owner's mentions of addresses the share is not open to: invited first. */
  const inviteMentioned = async (mentions: Mention[]): Promise<boolean> => {
    const newcomers = mentions
      .map((m) => m.email)
      .filter((email) => !live.people.some((p) => p.email === email) && !(live.teamDomain && email.endsWith(`@${live.teamDomain}`)));
    if (!owner || newcomers.length === 0) return true;
    try {
      const status = await api.setReviewers(id, [...live.reviewers, ...newcomers]);
      setLive((l) => ({ ...l, reviewers: status.reviewers, people: status.people.length > 0 ? status.people : l.people }));
      if (status.notice) setToast(status.notice);
      return true;
    } catch (error) {
      setToast(`Could not invite ${newcomers.join(", ")}: ${error instanceof Error ? error.message : String(error)}`);
      return false;
    }
  };
  const withMentions = (mentions: Mention[]) => (mentions.length > 0 ? mentions : undefined);
  const addComment = (d: Draft, text: string, mentions: Mention[]) => {
    void inviteMentioned(mentions).then((ok) => {
      if (!ok) return;
      say({ type: "add", screen: d.file, pin: d.whole ? undefined : { x: d.x, y: d.y, element: d.element ?? undefined }, text, mentions: withMentions(mentions) });
    });
    setDraft(null);
  };
  // The threads mentioning the reader that they have not opened; opening one reads it.
  const unread = useMemo(() => unreadThreads(live.comments, live.unread), [live.comments, live.unread]);
  const unreadSet = useMemo(() => new Set(unread), [unread]);
  const markSeen = (threadId: string) => {
    if (!unreadSet.has(threadId) || !say({ type: "seen", thread: threadId }, true)) return;
    setLive((l) => ({ ...l, unread: l.unread.filter((commentId) => !l.comments.some((c) => c.id === commentId && (c.reply_to ?? c.id) === threadId)) }));
  };
  const markSeenRef = useRef(markSeen);
  markSeenRef.current = markSeen;
  useEffect(() => {
    if (openThread) markSeenRef.current(openThread);
  }, [openThread, unreadSet]);
  /** Shows a thread: its screen's page, the screen selected, the thread at its pin. */
  const showThread = useCallback(
    (threadId: string) => {
      const comment = live.comments.find((c) => c.id === threadId);
      if (!comment) return;
      const index = live.canvas.pages.findIndex((p) => p.sections.some((section) => section.screens.some((screen) => screen.file === comment.screen)));
      if (index >= 0) setPage(index);
      setSelection([comment.screen]);
      setDraft(null);
      setOpenThread(threadId);
    },
    [live.comments, live.canvas.pages],
  );
  // `?thread=<id>`: the link of a mention, or one copied from a thread, opens it.
  const linked = useRef(threadParam(location.search));
  useEffect(() => {
    if (!linked.current || !live.comments.some((c) => c.id === linked.current)) return;
    if (fileName) setFocusThread({ id: linked.current, at: Date.now() });
    else showThread(linked.current);
    linked.current = null;
  }, [live.comments, showThread, fileName]);
  const escape = () => {
    if (takeOverOpen) setTakeOverOpen(false);
    else if (reviewersOpen) setReviewersOpen(false);
    else if (peopleOpen) setPeopleOpen(false);
    else if (draft) setDraft(null);
    else if (openThread) setOpenThread(null);
    else if (panel) setPanel(false);
    else if (commentMode) setCommentMode(false);
    else setSelection([]);
  };
  const escapeRef = useRef(escape);
  escapeRef.current = escape;
  const somethingOpen = takeOverOpen || reviewersOpen || peopleOpen || draft != null || openThread != null || panel || commentMode;
  useEffect(() => {
    if (!somethingOpen) return;
    const listener = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      escapeRef.current();
    };
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, [somethingOpen]);

  const popover =
    draft ? (
      <div className="w-[min(280px,calc(100vw-32px))] rounded-lg border border-line-strong bg-bg p-2 shadow-2xl" data-testid="comment-bubble">
        <div className="mb-1 truncate text-[11px] text-muted">
          {draft.whole ? "Whole screen" : draft.element?.text ? `On “${draft.element.text}”` : `At ${Math.round(draft.x)}%, ${Math.round(draft.y)}%`}
        </div>
        <CommentComposer placeholder="Your comment" submitLabel="Comment" mentionSetup={mentionSetup} onSubmit={(text, mentions) => addComment(draft, text, mentions)} onCancel={() => setDraft(null)} />
      </div>
    ) : shownThread ? (
      <div className="w-[min(320px,calc(100vw-32px))] rounded-lg border border-line-strong bg-bg p-2.5 shadow-2xl" data-testid="comment-popover">
        <ThreadView
          thread={shownThread}
          me={me}
          busy={false}
          stale={stale.has(shownThread.comment.id)}
          mentionSetup={mentionSetup}
          onReply={(threadId, text, mentions) =>
            void inviteMentioned(mentions).then((ok) => ok && say({ type: "add", screen: "", text, reply_to: threadId, mentions: withMentions(mentions) }))
          }
          onResolve={(threadId, resolved) => say({ type: "resolve", id: threadId, resolved })}
          onEdit={(commentId, text, mentions) => void inviteMentioned(mentions).then((ok) => ok && say({ type: "edit", id: commentId, text, mentions }))}
          onDelete={(commentId) => {
            say({ type: "delete", id: commentId });
            setOpenThread((current) => (current === commentId ? null : current));
          }}
          onClose={() => setOpenThread(null)}
        />
      </div>
    ) : null;
  // Full screen and Present comment as the canvas does; the bubble shows only in front.
  const overlay = fullScreen != null || presenting != null;
  const commenting: ScreenCommenting = {
    commentMode,
    onToggleCommentMode: () => {
      setCommentMode((on) => !on);
      setDraft(null);
    },
    pins,
    draft,
    openThread: shownThread,
    onFrame: onOverlayFrame,
    onCommentAt: startComment,
    onOpenThread: (threadId) => {
      setDraft(null);
      setOpenThread(threadId);
    },
    onEscape: () => {
      if (draft) setDraft(null);
      else if (openThread) setOpenThread(null);
      else setCommentMode(false);
    },
    popover,
  };

  const zoomOptions: Array<SelectOption<string>> = [
    { value: "fit", label: "Fit", name: "Fit the page", description: "⇧1" },
    ...ZOOM_STEPS.map((step) => ({ value: String(step), label: percent(step), description: step === 1 ? "⇧0" : undefined })),
  ];
  const zoomValue = ZOOM_STEPS.find((step) => Math.abs(step - zoom.scale) < 1e-6);
  const status = liveLabel(live.online, live.lastPushAt);
  const [mobile, setMobile] = useState(() => matchMedia("(max-width: 700px), (max-height: 500px) and (pointer: coarse)").matches || new URLSearchParams(location.search).get("mobile") === "1");
  useEffect(() => {
    const query = matchMedia("(max-width: 700px), (max-height: 500px) and (pointer: coarse)");
    const change = () => setMobile(query.matches || new URLSearchParams(location.search).get("mobile") === "1");
    query.addEventListener("change", change);
    return () => query.removeEventListener("change", change);
  }, []);
  const documentActions: CommentActions = {
    // A comment not sent (no socket, an invitation refused) rejects: its draft stays.
    add: async (quote, text, mentions) => {
      if (!fileName || !(await inviteMentioned(mentions)) || !say({ type: "add", screen: fileName, quote, text, mentions: withMentions(mentions) })) {
        throw new Error("not sent");
      }
    },
    reply: async (threadId, text, mentions) => {
      if (!(await inviteMentioned(mentions)) || !say({ type: "add", screen: "", text, reply_to: threadId, mentions: withMentions(mentions) })) {
        throw new Error("not sent");
      }
    },
    edit: (commentId, text, mentions) => void inviteMentioned(mentions).then((ok) => ok && say({ type: "edit", id: commentId, text, mentions })),
    remove: (commentId) => say({ type: "delete", id: commentId }),
    resolve: (threadId, resolved) => say({ type: "resolve", id: threadId, resolved }),
    opened: markSeen,
  };
  if (mobile && !fileName) return <MobileReview key={`${id}:${me.email}`} snapshotEpoch={snapshotEpoch} onReconnect={() => socket.current?.close()} id={id} title={first.title} owner={first.owner.name} me={me} canvas={live.canvas} comments={live.comments} online={live.online} lastPushAt={live.lastPushAt} connected={connected} say={say} serviceError={toast} pins={pins} stale={stale} onFrame={onFrame} elementUnder={elementUnder} mentionSetup={{ ...mentionSetup, canInvite: false }} unreadThreads={unread} onSeen={markSeen} />;


  return (
    <div className="flex h-dvh w-screen flex-col bg-bg text-fg" data-testid="share-page">
      <div className="flex h-10 shrink-0 items-center gap-2 border-b border-line px-3">
        <img src={icon} alt="Chauffeur" width={18} height={18} className="shrink-0 rounded-[4px]" />
        <span className="min-w-0 truncate text-[13px] font-semibold" title={first.title} data-testid="share-title">
          {first.title}
        </span>
        <span className="hidden text-[11.5px] text-faint sm:inline">by {first.owner.name}</span>
        <span className="flex items-center gap-1.5 text-[11.5px]" title={connected ? status.text : "The page lost the share service: it reconnects on its own"} data-testid="share-live">
          <span className={cx("h-1.5 w-1.5 rounded-full", !connected ? "bg-danger" : status.live ? "bg-success" : "bg-faint")} />
          <span className={status.live ? "text-success" : "text-muted"}>{connected ? status.text : "Reconnecting…"}</span>
        </span>
        {unread.length > 0 && (
          <Button
            size="sm"
            variant="secondary"
            className="shrink-0 text-accent"
            onClick={() => {
              const next = unread.find((t) => t !== openThread) ?? unread[0];
              if (fileName) setFocusThread({ id: next, at: Date.now() });
              else showThread(next);
            }}
            title="Open the next thread that mentions you"
            data-testid="share-mentions"
          >
            <AtSign size={12} /> {unread.length === 1 ? "1 mention for you" : `${unread.length} mentions for you`}
          </Button>
        )}
        {!owner && (
          <div className="relative ml-auto">
            <Button size="sm" variant={peopleOpen ? "secondary" : "ghost"} onClick={() => setPeopleOpen((on) => !on)} title="Who this design is shared with" aria-pressed={peopleOpen} data-testid="share-people-button">
              <Users size={12} /> <span className="hidden sm:inline">Shared with</span>
              <span className="rounded-full bg-accent/15 px-1 text-[10.5px] text-accent">{live.people.length}</span>
            </Button>
            {peopleOpen && <PeoplePanel people={live.people} owner={first.owner.email} viewers={live.viewers} onClose={() => setPeopleOpen(false)} />}
          </div>
        )}
        {!owner && !fileName && (
          <div className="relative">
            <Button size="sm" variant={takeOverOpen ? "secondary" : "ghost"} onClick={() => setTakeOverOpen((on) => !on)} title="Take this design over in your own Chauffeur: its screens and comments, for your own agent" aria-pressed={takeOverOpen} data-testid="share-take-over-button">
              <Download size={12} /> <span className="hidden sm:inline">Take over</span>
            </Button>
            {takeOverOpen && <TakeOverPanel url={location.href.split(/[?#]/)[0]} onClose={() => setTakeOverOpen(false)} />}
          </div>
        )}
        {owner && (
          <div className="relative ml-auto">
            <Button size="sm" variant={reviewersOpen ? "secondary" : "ghost"} onClick={() => setReviewersOpen((on) => !on)} title="Who this design is shared with" aria-pressed={reviewersOpen} data-testid="share-reviewers-button">
              <Users size={12} /> <span className="hidden sm:inline">Shared with</span>
              <span className="rounded-full bg-accent/15 px-1 text-[10.5px] text-accent">{live.reviewers.length}</span>
            </Button>
            {reviewersOpen && (
              <ReviewersPanel
                reviewers={live.reviewers}
                viewers={live.viewers}
                onChange={(reviewers) =>
                  api
                    .setReviewers(id, reviewers)
                    .then((status) => {
                      setLive((l) => ({ ...l, reviewers: status.reviewers, people: status.people.length > 0 ? status.people : l.people }));
                      // Saved, but some invitations were held back: the owner hears why.
                      if (status.notice) setToast(status.notice);
                    })
                    .catch((error: unknown) => setToast(`Could not change the reviewers: ${error instanceof Error ? error.message : String(error)}`))
                }
                onClose={() => setReviewersOpen(false)}
              />
            )}
          </div>
        )}
        <div className={cx("flex items-center gap-1", !owner && "ml-auto")} data-testid="share-viewers" title={live.viewers.length === 0 ? "Nobody else is looking" : live.viewers.join(", ")}>
          {live.viewers
            .filter((email) => email !== me.email)
            .slice(0, 6)
            .map((email) => (
              <span key={email} className="inline-flex h-6 w-6 items-center justify-center rounded-full bg-accent/20 text-[10px] font-semibold text-accent" title={email}>
                {initials(email.split("@")[0])}
              </span>
            ))}
          <span className="ml-1 inline-flex h-6 w-6 items-center justify-center rounded-full bg-raised text-[10px] font-semibold text-fg ring-1 ring-line-strong" title={`${me.name} (you)`}>
            {initials(me.name)}
          </span>
          <Button
            size="sm"
            variant="ghost"
            title={`Sign out (${me.email})`}
            onClick={() => void api.signOut().then(onSignedOut, () => setToast("Could not sign out; try again"))}
            data-testid="share-sign-out"
          >
            <LogOut size={12} />
          </Button>
        </div>
      </div>
      {fileName ? (
        <div className="relative min-h-0 flex-1" data-testid="share-document">
          {documentText == null ? (
            <div className="flex h-full items-center justify-center text-faint">
              <Spinner />
            </div>
          ) : (
            <MarkdownComments
              text={documentText}
              comments={live.comments}
              me={me}
              busy={false}
              panel={documentPanel}
              onPanel={setDocumentPanel}
              actions={documentActions}
              canSend={false}
              mentionSetup={mentionSetup}
              unread={unreadSet}
              focus={focusThread}
            />
          )}
          {toast && (
            <div className="pointer-events-none absolute inset-x-0 bottom-4 flex justify-center" data-testid="share-toast">
              <div className="rounded-md border border-line-strong bg-raised px-3 py-1.5 text-[12px] text-fg shadow-xl selectable">{toast}</div>
            </div>
          )}
        </div>
      ) : (
      <>
      <div className="@container flex h-8 shrink-0 items-center gap-1 border-b border-line px-2">
        {pages.length > 1 && (
          <Select
            value={String(pageIndex)}
            options={pages.map((p, i) => ({ value: String(i), label: p.name || `Page ${i + 1}` }))}
            onChange={(value) => {
              setPage(Number(value));
              setSelection([]);
            }}
            title="Canvas page"
            testId="canvas-page"
            width={220}
          />
        )}
        <div className="ml-auto flex items-center gap-0.5">
          <Select
            value={zoomValue != null ? String(zoomValue) : "custom"}
            options={zoomOptions}
            shown={{ label: percent(zoom.scale) }}
            onChange={(value) => (value === "fit" ? fit() : zoomTo(Number(value)))}
            title="Zoom"
            testId="canvas-zoom"
            width={170}
            disabled={layout.sections.length === 0}
          />
          <Button size="sm" variant="ghost" onClick={fit} disabled={layout.sections.length === 0} title="Fit the page (⇧1)" data-testid="canvas-fit">
            <Scan size={12} /> <span className="hidden @[640px]:inline">Fit</span>
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => steps.length > 0 && setPresenting(Math.max(0, steps.findIndex((step) => selection.includes(step.screen.file))))}
            disabled={steps.length === 0}
            title="Present: one screen at a time, interactive (← → between screens, Esc leaves)"
            data-testid="canvas-present"
          >
            <Play size={12} /> <span className="hidden @[640px]:inline">Present</span>
          </Button>
          <Button
            size="sm"
            variant={commentMode ? "secondary" : "ghost"}
            onClick={() => {
              setCommentMode((on) => !on);
              setDraft(null);
            }}
            disabled={layout.sections.length === 0}
            title={commentMode ? "Leave Comment mode (C or Esc)" : "Comment mode (C): click a screen to comment on it, its label for the whole screen"}
            aria-pressed={commentMode}
            data-testid="canvas-comment-mode"
          >
            <MessageSquarePlus size={12} /> <span className="hidden @[640px]:inline">Comment</span>
          </Button>
          <Button size="sm" variant={panel ? "secondary" : "ghost"} onClick={() => setPanel((on) => !on)} title="The comments, by screen" aria-pressed={panel} data-testid="canvas-comments">
            <MessageSquareText size={12} /> <span className="hidden @[640px]:inline">Comments</span>
            {counts.open > 0 && <span className="rounded-full bg-accent/15 px-1 text-[10.5px] text-accent">{counts.open}</span>}
          </Button>
        </div>
      </div>
      {live.canvas.error && (
        <div className="flex shrink-0 items-center gap-2 border-b border-line bg-attention/10 px-3 py-1.5 text-[12px] text-attention">
          <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-attention" />
          <span className="min-w-0 selectable">canvas.json is invalid: {live.canvas.error}. The screens are sorted automatically.</span>
        </div>
      )}
      <div className="relative min-h-0 flex-1">
        {empty ? (
          <div className="flex h-full items-center justify-center p-6 text-center text-[12.5px] text-faint" data-testid="canvas-empty">
            No screens yet.
          </div>
        ) : (
          <CanvasViewport
            layout={layout}
            zoom={zoom}
            area={area}
            setZoom={moveView}
            onArea={setArea}
            selection={selection}
            onSelect={setSelection}
            flash={flash}
            commentMode={commentMode}
            pins={pins}
            draft={draft}
            openThread={shownThread}
            onFrame={onFrame}
            onCommentAt={startComment}
            onOpenThread={commenting.onOpenThread}
            onEscape={escape}
            onToggleCommentMode={commenting.onToggleCommentMode}
            popover={overlay ? null : popover}
            onFullScreen={setFullScreen}
            onFit={fit}
            onActualSize={() => zoomTo(1)}
            published={NO_PUBLISHED}
          />
        )}
        {panel && (
          <CommentsPanel
            me={me.email}
            unread={unreadSet}
            threads={threads}
            counts={counts}
            stale={stale}
            busy={false}
            error={null}
            filter={filter}
            labelOf={labelOf}
            onFilter={setFilter}
            onOpen={(thread) => {
              setDraft(null);
              setOpenThread(thread.comment.id);
            }}
            onResolve={(threadId, resolved) => say({ type: "resolve", id: threadId, resolved })}
            onClose={() => setPanel(false)}
          />
        )}
        {toast && (
          <div className="pointer-events-none absolute inset-x-0 bottom-4 flex justify-center" data-testid="share-toast">
            <div className="rounded-md border border-line-strong bg-raised px-3 py-1.5 text-[12px] text-fg shadow-xl selectable">{toast}</div>
          </div>
        )}
      </div>
      </>
      )}
      {fullScreen != null && sections[fullScreen] && <FullScreenSection section={sections[fullScreen]} onClose={() => setFullScreen(null)} commenting={commenting} />}
      {presenting != null && steps.length > 0 && <PresentScreens steps={steps} start={presenting} published={NO_PUBLISHED} onClose={() => setPresenting(null)} commenting={commenting} />}
    </div>
  );
}
