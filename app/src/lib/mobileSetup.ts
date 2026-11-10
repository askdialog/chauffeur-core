import type { MobileHostStatus } from "@chauffeur/protocol";

type Saved = Pick<MobileHostStatus, "enabled" | "relay" | "key_saved">;

const QR = "The QR code then shows in the window Connect phone opens.";

/** Whether mobile access cannot be turned on yet: the daemon has no relay or no credential for it. */
export function relaySetupMissing(status: Saved): boolean {
  return !status.relay || !status.key_saved;
}

/** What the user still has to do before a phone can scan the QR code; null once mobile access is on. */
export function mobileSetupHint(status: Saved): string | null {
  if (relaySetupMissing(status)) {
    const what = !status.relay && !status.key_saved
      ? "the relay address and the operator credential"
      : !status.relay ? "the relay address" : "the operator credential";
    return `To turn mobile access on, enter ${what} your relay's operator gave you under Relay configuration, then choose Save and enable. ${QR}`;
  }
  if (!status.enabled) return `Turn mobile access on. ${QR}`;
  return null;
}
