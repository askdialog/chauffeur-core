// The reviewer page of a share (story 51), the parts with no React in them: the share named in
// the page's address, its socket's address, what the banner says of the owner's daemon, and what
// a refusal of the service means to the reviewer.

import { relativeTime } from "@/lib/format";

/** The share id of `/s/<id>`; null for any other address. */
export function shareIdOf(pathname: string): string | null {
  const match = /^\/s\/([0-9a-f]{8,64})\/?$/i.exec(pathname);
  return match ? match[1].toLowerCase() : null;
}

/** The reviewer socket of a share, on the page's own host. */
export function socketUrl(location: { protocol: string; host: string }, id: string): string {
  return `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/v1/shares/${id}/reviewer`;
}

/** What the banner says of the design: live from the owner's Mac, or as it was last pushed. */
export function liveLabel(online: boolean, lastPushAt: number | undefined, now = Date.now()): { text: string; live: boolean } {
  if (online) return { text: "Live", live: true };
  if (lastPushAt == null) return { text: "Waiting for the first screens", live: false };
  return { text: `Last update ${relativeTime(new Date(lastPushAt).toISOString(), now)}`, live: false };
}

/** Up to two letters for a person's avatar. */
export function initials(name: string): string {
  const parts = name.trim().split(/[\s._-]+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

/** The token of a sign-in link, which the service puts in the page's fragment (`#sign-in=<hex>`);
 *  null for any other fragment. */
export function signInTokenOf(hash: string): string | null {
  const match = /^#sign-in=([0-9a-f]{16,128})$/i.exec(hash);
  return match ? match[1] : null;
}

/** The sign-in card's title before anyone signs in: who shared what. */
export function sharedWithYou(owner: string | null, kind: "design" | "workspace"): string {
  return owner ? `${owner} shared a ${kind} with you` : `A ${kind} was shared with you`;
}

/** What a signed-in reader who may not open the share reads. */
export function noAccessText(email: string): string {
  return `You're signed in as ${email}, which doesn't have access.`;
}

/** What a refusal of the service means to the reviewer, in a sentence. */
export function refusalText(status: number, reason: string | null): string {
  if (status === 410) return "This share was stopped.";
  if (status === 403) return reason || "This design was not shared with you.";
  if (status === 404) return "There is no such share.";
  return reason || `The share service answered ${status}.`;
}

/** The files changed since the page last read the canvas, as the canvas names its screens. */
export function flashFor(changed: readonly string[], stamp: number): Record<string, number> {
  return Object.fromEntries(changed.filter((file) => /\.html?$/i.test(file)).map((file) => [file, stamp]));
}
