import type { LoginSessionInfo } from "@chauffeur/protocol";

/** What a session's state reads as, beside its label. */
export function sessionStateText(session: LoginSessionInfo, now = Date.now()): string {
  if (session.logging_in) return "Login running…";
  switch (session.state) {
    case "holds": {
      if (!session.expires_at) return "Signed in";
      const minutes = Math.round((Date.parse(session.expires_at) - now) / 60_000);
      if (minutes < 1) return "Ends now";
      return minutes < 60 ? `Ends in ${minutes} min` : `Ends in ${Math.round(minutes / 60)} h`;
    }
    case "expired":
      return "Expired";
    case "unchecked":
      return "Not checked yet";
  }
}
