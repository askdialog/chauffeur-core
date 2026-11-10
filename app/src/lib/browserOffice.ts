// The Office in the user's browser (docs/office.md): the page the daemon serves on 127.0.0.1 for
// an app whose web view cannot make calls. Its link carries a one-time code in the fragment, which
// the page exchanges once for a session of the Office alone; the daemon's token never reaches it.

import type { ExtensionUi } from "@chauffeur/protocol";
import type { ConnectionInfo } from "./rpc";

/** This page is the Office in the browser (`#office`), not the app. */
export const inBrowserOffice = typeof location !== "undefined" && location.hash.startsWith("#office");

const SESSION_KEY = "chauffeur.office.session";

/**
 * How long the app hands out a link it got: the daemon takes its code for a minute, less the time
 * for a browser to start or for the user to paste it.
 */
const LINK_USABLE_MS = 45_000;

/** Whether a link the daemon gave at `given` (ms) may still be opened or copied at `now`. */
export function linkUsable(given: number, now: number): boolean {
  return now - given < LINK_USABLE_MS;
}

export const SESSION_ENDED = "This page's session ended: open the Office in your browser again from Chauffeur.";

/** The one-time code of the link (`#office=<code>`), else null. */
export function codeOf(hash: string): string | null {
  const code = hash.match(/^#office=([0-9a-f]+)$/)?.[1];
  return code ?? null;
}

/**
 * The page's session: the link's code exchanged, once, then kept by the tab so that a reload
 * keeps it; the tab's own session without a code. Throws when there is neither.
 */
export async function sessionFor(
  hash: string,
  storage: Pick<Storage, "getItem" | "setItem">,
  exchange: (code: string) => Promise<string>,
): Promise<string> {
  const code = codeOf(hash);
  if (code) {
    const session = await exchange(code);
    storage.setItem(SESSION_KEY, session);
    return session;
  }
  const kept = storage.getItem(SESSION_KEY);
  if (!kept) throw new Error(SESSION_ENDED);
  return kept;
}

async function exchange(code: string): Promise<string> {
  const response = await fetch("/session", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ code }),
  });
  if (!response.ok) throw new Error((await response.text()) || `HTTP ${response.status}`);
  return ((await response.json()) as { token: string }).token;
}

let session: Promise<string> | null = null;

/** Where the Office's page reaches the daemon: its own listener, with its session. */
export async function browserConnection(): Promise<ConnectionInfo> {
  session ??= sessionFor(location.hash, sessionStorage, exchange).finally(() => {
    // The code is spent: out of the address bar, the history and a bookmark.
    history.replaceState(null, "", "#office");
  });
  try {
    return { url: `ws://${location.host}/ws`, token: await session };
  } catch (error) {
    session = null;
    throw error;
  }
}

/** The page's session is over: only a new link from the app opens the Office again. */
export class SessionEnded extends Error {}

/** The installed Office revision, as the page's listener describes it; null while not ready. */
export async function fetchBrowserUi(token: string, fetcher: typeof fetch = fetch): Promise<ExtensionUi | null> {
  const response = await fetcher("/extension-ui/office", {
    headers: { authorization: `Bearer ${token}` },
    cache: "no-store",
  });
  if (response.status === 401) throw new SessionEnded(SESSION_ENDED);
  if (response.status === 503) return null;
  if (!response.ok) throw new Error((await response.text()) || `HTTP ${response.status}`);
  return (await response.json()) as ExtensionUi;
}
