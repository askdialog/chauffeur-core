import { useApp } from "@/state/app";
import { signIn, signOut, useIdentity } from "@/state/identity";
import { Button, Spinner } from "./ui";

/** Settings › Account: Chauffeur's Google sign-in, which shares go through. */
export function AccountSettings() {
  const identity = useApp((s) => s.identity);
  const signingIn = useIdentity((s) => s.signingIn);
  if (!identity) return <Spinner />;
  return (
    <div className="space-y-3" data-testid="account-settings">
      <h3 className="text-sm font-medium text-fg">Account</h3>
      {identity.email ? (
        <div className="flex items-center justify-between gap-3">
          <p className="min-w-0 truncate text-[12.5px] text-fg selectable" data-testid="account-email">
            Signed in as {identity.email}
          </p>
          <Button size="sm" onClick={() => void signOut()} data-testid="account-sign-out">
            Sign out
          </Button>
        </div>
      ) : identity.can_sign_in ? (
        <>
          <p className="text-[12.5px] leading-relaxed text-muted">Sign in to share designs and workspaces.</p>
          <Button variant="primary" onClick={() => void signIn()} disabled={signingIn} data-testid="account-sign-in">
            {signingIn ? "Finish in your browser…" : "Sign in with Google"}
          </Button>
        </>
      ) : (
        <p className="text-[12.5px] leading-relaxed text-muted">
          Sign-in is off on this computer: set office.google_client_id and office.google_client_secret in ~/.chauffeur/settings.toml.
        </p>
      )}
    </div>
  );
}
