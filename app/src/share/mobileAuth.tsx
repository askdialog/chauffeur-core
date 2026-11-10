import type { CommentAuthor } from "@chauffeur/protocol";
import { useState } from "react";
import { api } from "./api";
import { browserReviewUrl, mobileAuthContext } from "./mobileAuthContext";
import { Button } from "@/components/ui";

/** Consent is required even for a mailed continuation: its challenge may come from anyone. */
export function ReturnToReview({ id, title, me }: { id: string; title: string; me: CommentAuthor }) {
  const [error, setError] = useState<string | null>(null);
  const [callback, setCallback] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const context = mobileAuthContext();
  const authorize = async () => {
    if (!context || busy) return;
    setBusy(true); setError(null);
    try {
      const result = await api.mobileAuthorize(id, context);
      setCallback(result.callback);
      location.assign(result.callback);
    } catch (e) { setError(e instanceof Error ? e.message : "Could not return to Chauffeur; sign in again from the app."); }
    finally { setBusy(false); }
  };
  return <div className="mobile-review mobile-return" data-testid="review-consent">
    <h1>Sign in to Chauffeur?</h1>
    <p>Review <strong>{title}</strong> in the Chauffeur app as <strong>{me.email}</strong>.</p>
    <p className="review-sub">Only continue if you just requested sign-in from Chauffeur on this device. The app will be signed in with this account.</p>
    {!context && <p role="status">This sign-in request expired. Return to Chauffeur and sign in again, or continue in this browser.</p>}
    {error && <p role="status">{error}</p>}
    {context && <Button variant="primary" busy={busy} disabled={busy} onClick={() => void authorize()}>Allow and open Chauffeur</Button>}
    {callback && <a className="review-primary" href={callback}>Return to Chauffeur</a>}
    <a className="review-browser-link" href={browserReviewUrl(location.href)}>View in this browser</a>
  </div>;
}
