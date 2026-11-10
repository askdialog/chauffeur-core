// The share service as the reviewer and guest pages call it (stories 51, 52): JSON over the
// page's own origin, the session cookie carried by the browser. A refusal keeps its status, for
// what the page says.

import { mobileAuthContext } from "./mobileAuthContext";
import type { CanvasResult, CommentAuthor, EventEnvelope, ShareAbout, ShareSnapshot, ShareStatus, SignInLinkParams, SignInLinkSessionParams, WorkspaceShareSnapshot } from "@chauffeur/protocol";

export class ServiceError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

async function call<T>(url: string, init?: RequestInit): Promise<T> {
  const response = await fetch(url, { credentials: "same-origin", cache: "no-store", ...init });
  if (!response.ok) {
    let reason: string | null = null;
    try {
      reason = ((await response.json()) as { error?: string }).error ?? null;
    } catch {
      // Not the service's JSON: the status says enough.
    }
    throw new ServiceError(response.status, reason ?? `The share service answered ${response.status}`);
  }
  if (response.status === 204) return undefined as T;
  return (await response.json()) as T;
}

/** `site_url`: where "Shared with Chauffeur" links; none on a service that names no site. */
export type ServiceConfig = { web_client_id: string | null; site_url: string | null; version: number };

/** A design share is under `/v1/shares`, a workspace share under `/v1/workspace-shares`. */
export type ShareKind = "design" | "workspace";

const base = (kind: ShareKind, id: string) => `/v1/${kind === "design" ? "shares" : "workspace-shares"}/${id}`;
const json = (body: unknown): RequestInit => ({ method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });

export const api = {
  config: () => call<ServiceConfig>("/v1/config"),
  /** Who the session cookie says; 401 without one. */
  me: () => call<CommentAuthor>("/v1/me"),
  /** A Google ID token (or `dev:<email>` on a development server) for a session cookie. */
  session: (idToken: string) => call<CommentAuthor>("/v1/session", json({ id_token: idToken })),
  /** A sign-in link's token, once, for a session cookie of its address. */
  sessionFromLink: (token: string) => call<CommentAuthor>("/v1/session/link", json({ token } satisfies SignInLinkSessionParams)),
  /** Who shared it, for a reader not signed in yet. */
  about: (kind: ShareKind, id: string) => call<ShareAbout>(`${base(kind, id)}/about`),
  /** Asks for a sign-in link to `email`; the answer is the same whether it was invited or not. */
  askSignInLink: (kind: ShareKind, id: string, email: string) => {
    const target = new URLSearchParams(location.search);
    const context = kind === "design" ? mobileAuthContext() : null;
    return call<{ message: string }>(`${base(kind, id)}/sign-in-link`, json({ email, ...(kind === "design" ? context ? { mobile_auth: context } : { screen: target.get("screen") || undefined, thread: target.get("thread") || undefined } : {}) } satisfies SignInLinkParams & { screen?: string; thread?: string; mobile_auth?: ReturnType<typeof mobileAuthContext> }));
  },
  mobileAuthorize: (id: string, context: NonNullable<ReturnType<typeof mobileAuthContext>>) => call<{ callback: string }>(`/v1/shares/${id}/mobile-authorize`, { ...json(context), headers: { "content-type": "application/json", "X-Chauffeur-Mobile": "1" } }),
  signOut: () => call<void>("/v1/session", { method: "DELETE" }),
  snapshot: (id: string) => call<ShareSnapshot>(`/v1/shares/${id}/snapshot`),
  canvas: (id: string) => call<CanvasResult>(`/v1/shares/${id}/canvas`),
  /** A markdown file's share (story 57): the file's text. */
  document: async (id: string, name: string): Promise<string> => {
    const response = await fetch(`/v1/shares/${id}/files/${encodeURIComponent(name)}`, { credentials: "same-origin", cache: "no-store" });
    if (!response.ok) throw new ServiceError(response.status, `The share service answered ${response.status}`);
    return response.text();
  },
  workspaceSnapshot: (id: string) => call<WorkspaceShareSnapshot>(`/v1/workspace-shares/${id}/snapshot`),
  /** A shared chat's events, oldest first. */
  chatEvents: (id: string, chat: string) => call<EventEnvelope[]>(`/v1/workspace-shares/${id}/chats/${encodeURIComponent(chat)}/events`),
  /** The owner, from their own page: the whole list of reviewers.*/
  setReviewers: (id: string, reviewers: string[]) =>
    call<ShareStatus>(`/v1/shares/${id}`, { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify({ reviewers }) }),
};
