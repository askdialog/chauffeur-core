// What Settings › About Chauffeur says about the versions: the app's, the daemon's, and a newer
// published one. The app and its daemon differ after a partial install or a daemon left running
// from an older build.

import type { UpdateStatus } from "@chauffeur/protocol";

/** What to do when the daemon runs another version than the app; null when they match or the daemon is unknown. */
export function daemonMismatch(app: string, daemon: string | undefined): string | null {
  if (!daemon || daemon === app) return null;
  return `The daemon runs ${daemon}: restart it to run ${app} ("chauffeur daemon stop", then reopen Chauffeur).`;
}

/** The newer published version, as the update card at the foot of the sidebar offers it; null when there is none. */
export function updateNote(status: UpdateStatus | null): string | null {
  if (!status?.available) return null;
  return `Chauffeur ${status.available} is available: update it from the card at the foot of the sidebar.`;
}
