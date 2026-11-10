// A design chat's share (story 51), kept pure so that it is tested in share.test.ts: the
// reviewers typed into the dialog, what the Share button and the dialog say of the share's state.
// A workspace's share (story 53) is said the same way: its connection, the e-mails to offer.

import type { DesignShareState, SharedDesign, WorkspaceShareState } from "@chauffeur/protocol";

/** A design's share or a workspace's, for what both dialogs say alike. */
type AnyShareState = DesignShareState | WorkspaceShareState;

/** The e-mails typed (commas, spaces or lines apart), lowercase, once; and what is not one. */
export function parseEmails(input: string): { emails: string[]; invalid: string[] } {
  const emails: string[] = [];
  const invalid: string[] = [];
  for (const raw of input.split(/[\s,;]+/)) {
    const email = raw.trim().toLowerCase();
    if (!email) continue;
    if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) {
      invalid.push(raw.trim());
      continue;
    }
    if (!emails.includes(email)) emails.push(email);
  }
  return { emails, invalid };
}

/** Whether a design is shared now: a share that is not stopped. */
export function isShared(state: AnyShareState | undefined): boolean {
  return state?.share != null && !state.share.stopped;
}

/** What the Share button's title says. */
export function shareTitle(state: DesignShareState | undefined): string {
  if (!isShared(state)) return "Share this design live with named colleagues (Google sign-in)";
  const viewers = state?.share?.viewers.length ?? 0;
  const who = viewers === 0 ? "nobody looking now" : viewers === 1 ? `${state?.share?.viewers[0]} is looking` : `${viewers} looking`;
  const named = state?.share?.reviewers.length ?? 0;
  return `Shared with ${state?.share?.team ? (named > 0 ? `the team and ${named}` : "the team") : named}: ${who}`;
}

/** The connection, in a word for the dialog. */
export function connectionLabel(state: AnyShareState | undefined): { text: string; tone: "online" | "off" | "failed" | "busy" } {
  if (!isShared(state)) return { text: "Not shared", tone: "off" };
  const c = state?.connection;
  if (!c || c.state === "off") return { text: "Not connected", tone: "off" };
  if (c.state === "connecting") return { text: "Connecting…", tone: "busy" };
  if (c.state === "failed") return { text: `Offline: ${c.message}. The share shows its last state.`, tone: "failed" };
  return { text: "Live", tone: "online" };
}

/** The e-mails to offer under the box: past reviewers not named yet. */
export function suggestions(state: AnyShareState | undefined, named: readonly string[]): string[] {
  return (state?.past_reviewers ?? []).filter((email) => !named.includes(email));
}

/** The Office's Design window: colleagues' designs naming the user, then those open to the team. */
export function sharedSections(designs: readonly SharedDesign[]): { named: SharedDesign[]; team: SharedDesign[] } {
  return { named: designs.filter((d) => d.named), team: designs.filter((d) => !d.named && d.team) };
}

/** When a shared design last changed, for `relativeTime`. */
export function sharedChangedAt(design: SharedDesign): string {
  return new Date(design.last_push_at ?? design.created_at).toISOString();
}
