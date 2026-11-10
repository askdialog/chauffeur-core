import { useEffect, useState } from "react";
import type { MobileHostStatus } from "@chauffeur/protocol";
import { Smartphone } from "lucide-react";
import { client, useApp } from "@/state/app";
import { mobileSetupHint, relaySetupMissing } from "@/lib/mobileSetup";
import { openMobileConnection } from "@/lib/native";
import { errorMessage } from "@/lib/rpc";
import { secretStore } from "@/lib/secretStore";
import { Button, Field, inputClass, Toggle } from "./ui";

/** Settings › Mobile: phone access is a member's only; others are told it is coming. */
export function MobileSettings() {
  const member = useApp((s) => s.identity?.member ?? false);
  return member ? <MobileAccess /> : <p className="text-[12.5px] text-muted" data-testid="mobile-coming-soon">Phone access is coming soon.</p>;
}

function MobileAccess() {
  const [status, setStatus] = useState<MobileHostStatus | null>(null);
  const [relay, setRelay] = useState("");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let disposed = false;
    let initialized = false;
    const refresh = async () => {
      try {
        const value = await client.call("mobile.status", {});
        if (disposed) return;
        setStatus(value);
        if (!initialized) { setRelay(value.relay); initialized = true; }
      } catch (err) { if (!disposed) setError(errorMessage(err)); }
    };
    void refresh();
    const timer = setInterval(() => void refresh(), 2000);
    return () => { disposed = true; clearInterval(timer); };
  }, []);
  async function save(enabled: boolean) {
    setBusy(true); setError(null);
    try {
      const value = await client.call("mobile.configure", { enabled, relay: enabled ? relay.trim() : status!.relay, operator_token: enabled && token ? token.trim() : null });
      setToken(""); setStatus(value); setRelay(value.relay);
    } catch (err) { setError(errorMessage(err)); }
    finally { setBusy(false); }
  }
  const hint = status && mobileSetupHint(status);
  return <div className="space-y-4" data-testid="mobile-settings">
    <h3 className="text-sm font-medium text-fg">Your agents, wherever you are</h3>
    <p className="text-[12.5px] leading-relaxed text-muted">Connect your phone with a QR code and approve it on this computer. Conversations and commands stay encrypted from end to end.</p>
    {error && <p role="alert" className="text-xs text-red">{error}</p>}
    {status && <>
      {hint && <p data-testid="mobile-setup-hint" className="text-xs text-fg">{hint}</p>}
      <button type="button" role="switch" aria-checked={status.enabled} aria-label="Mobile access" disabled={busy || (!status.enabled && (!status.key_saved || relay.trim() !== status.relay) && !token.trim())} onClick={() => void save(!status.enabled)} className="flex w-full items-center justify-between gap-4 text-left disabled:opacity-50">
        <span><span className="text-sm text-fg">Mobile access</span><span className="mt-1 block text-xs text-muted">Starts automatically with Chauffeur. Agents remain reachable when you close the Chauffeur window.</span></span><Toggle on={status.enabled} />
      </button>
      <div role="status" data-testid="mobile-host-status" className="text-xs text-muted">
        {status.phase === "ready" ? status.connected_devices ? `${status.connected_devices} phone${status.connected_devices > 1 ? "s" : ""} connected` : "Ready to connect your phone" : status.phase === "disabled" ? "Mobile access is off" : status.phase === "error" ? "Mobile connection unavailable" : "Starting the mobile connection…"}
        {status.error && <p className="mt-1">{status.error}</p>}
      </div>
      <Button disabled={busy || status.phase !== "ready"} onClick={() => void openMobileConnection().catch(err => setError(errorMessage(err)))}><Smartphone size={14} /> Connect phone</Button>
      <p className="text-xs text-muted">The connection window lists paired phones and lets you revoke access. Turning mobile access off preserves existing pairings.</p>
      <details open={relaySetupMissing(status)} className="text-xs text-muted"><summary className="cursor-pointer">Relay configuration</summary><div className="mt-3 space-y-3">
        <Field label="Relay address"><input aria-label="Relay address" className={inputClass} value={relay} onChange={e => setRelay(e.target.value)} spellCheck={false} disabled={busy} /></Field>
        <Field label={status.key_saved ? "Replace relay credential" : "Relay credential"}><input aria-label="Relay credential" type="password" autoComplete="off" className={inputClass} value={token} onChange={e => setToken(e.target.value)} disabled={busy} placeholder={status.key_saved ? `Saved in ${secretStore(false)}` : "Enter the relay operator credential"} /></Field>
        <p>The credential is kept by Chauffeur in {secretStore(false)}. It is never sent to your phone. Changing relay requires its own credential.</p>
        <Button disabled={busy || !token.trim()} onClick={() => void save(true)}>Save and enable</Button>
      </div></details>
    </>}
  </div>;
}
