// The sidebar's one-time sign-in banner: offered to a user signed out, until they sign in or say
// Not now; never again after that (`SIGN_IN_BANNER_KEY`).

import { LogIn } from "lucide-react";
import { useState } from "react";
import { SIGN_IN_BANNER_KEY, signInBannerShown } from "@/lib/members";
import { useApp } from "@/state/app";
import { signIn, useIdentity } from "@/state/identity";
import { Button } from "./ui";

function storedDone(): boolean {
  try { return localStorage.getItem(SIGN_IN_BANNER_KEY) === "1"; }
  catch { return false; }
}

export function SignInBanner() {
  const identity = useApp((s) => s.identity);
  const signingIn = useIdentity((s) => s.signingIn);
  const [done, setDone] = useState(storedDone);
  if (!signInBannerShown(identity, done)) return null;
  const dismiss = () => {
    try { localStorage.setItem(SIGN_IN_BANNER_KEY, "1"); }
    catch { /* Storage unavailable: it comes back at the next launch. */ }
    setDone(true);
  };
  return (
    <div className="mx-2 mb-2 shrink-0 rounded-lg border border-line bg-raised p-2.5 text-[12px] shadow-sm" data-testid="sign-in-banner">
      <div className="flex items-start gap-2">
        <LogIn size={14} className="mt-px shrink-0 text-accent" />
        <div className="min-w-0 flex-1">
          <div className="font-medium text-fg">Sign in with Google to share designs and workspaces</div>
          <div className="mt-2 flex gap-1.5">
            <Button size="sm" variant="primary" disabled={signingIn} onClick={() => void signIn()} data-testid="sign-in-banner-sign-in">
              {signingIn ? "Finish in your browser…" : "Sign in"}
            </Button>
            <Button size="sm" variant="ghost" onClick={dismiss} data-testid="sign-in-banner-dismiss">
              Not now
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
