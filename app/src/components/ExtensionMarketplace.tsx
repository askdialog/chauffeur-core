import { BadgeCheck, Database, Download, Package, RefreshCw, RotateCcw, Trash2, Users } from "lucide-react";
import { useEffect, useState } from "react";
import type { ExtensionCommand, ExtensionId, ExtensionInfo } from "@chauffeur/protocol";
import { applyExtensions, client, loadExtensions, useApp } from "@/state/app";
import { packageStatus } from "@/lib/extensions";
import { errorMessage } from "@/lib/rpc";
import { Badge, Button, cx, Spinner, Toggle } from "./ui";

/** What a click started, per extension, until the daemon answered. */
type Pending = ExtensionCommand | "enable" | "disable" | "auto-update";

export function ExtensionMarketplace() {
  const extensions = useApp((state) => state.extensions);
  const [pending, setPending] = useState<Partial<Record<ExtensionId, Pending>>>({});
  const [errors, setErrors] = useState<Partial<Record<ExtensionId, string>>>({});
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const refresh = async () => {
    setLoading(true);
    setError(null);
    try { await loadExtensions(); }
    catch (error) { setError(errorMessage(error)); }
    finally { setLoading(false); }
  };
  useEffect(() => { void refresh(); }, []);

  const run = async (id: ExtensionId, what: Pending, call: () => Promise<ExtensionInfo[]>) => {
    setPending((current) => ({ ...current, [id]: what }));
    setErrors((current) => ({ ...current, [id]: undefined }));
    try { applyExtensions(await call()); }
    catch (error) { setErrors((current) => ({ ...current, [id]: errorMessage(error) })); }
    finally { setPending((current) => ({ ...current, [id]: undefined })); }
  };

  return <div className="space-y-4" data-testid="extension-marketplace">
    <div>
      <div className="flex items-center justify-between gap-3">
        <h2 className="flex items-center gap-2 text-sm font-medium"><Package size={16} />Extension marketplace</h2>
        <Button size="sm" onClick={() => void refresh()} disabled={loading} aria-label="Refresh extensions"><RefreshCw size={12} /></Button>
      </div>
      <p className="mt-2 text-[12.5px] leading-relaxed text-muted">Make Chauffeur yours with official extensions. Each one installs and updates on its own, without a new version of Chauffeur.</p>
    </div>
    {extensions === null && loading && <Spinner />}
    {error && <p role="alert" className="selectable rounded-md border border-danger/30 bg-danger/5 px-3 py-2 text-[12px] text-danger">{error}</p>}
    {extensions?.length === 0 && <p className="text-[12.5px] text-muted">This Chauffeur offers no extensions.</p>}
    {extensions?.map((extension) => <ExtensionCard
      key={extension.id}
      extension={extension}
      pending={pending[extension.id] ?? null}
      error={errors[extension.id] ?? null}
      run={(what, call) => void run(extension.id, what, call)}
    />)}
    <p className="text-[11.5px] leading-relaxed text-muted">Disabling or removing an extension removes its views and stops its background activity. Connections and preferences stay on this computer. An update waits until the extension's views are closed, so nothing you are editing is lost.</p>
  </div>;
}

function ExtensionCard({ extension, pending, error, run }: {
  extension: ExtensionInfo;
  pending: Pending | null;
  error: string | null;
  run: (what: Pending, call: () => Promise<ExtensionInfo[]>) => void;
}) {
  const { id, name } = extension;
  const pkg = extension.package;
  const status = packageStatus(extension);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const blocked = pending !== null || status.busy;
  const manage = (command: ExtensionCommand) => run(command, () => client.call("extensions.manage", { id, command }));
  const installed = pkg ? pkg.desired_installed : true;

  return <article data-testid={`extension-${id}`} className="rounded-lg border border-line bg-raised/40 p-4">
    <div className="flex items-start gap-3">
      <span className="rounded-lg border border-line bg-bg p-2 text-accent">{id === "database" ? <Database size={21} /> : <Users size={21} />}</span>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <h3 className="text-[13px] font-medium">{name}</h3>
          {installed && <Badge>{extension.enabled ? "Enabled" : "Disabled"}</Badge>}
          {pkg && <Badge tone={status.tone} className={cx(status.busy && "gap-1.5")}>{status.busy && <Spinner size={10} />}{status.label}</Badge>}
        </div>
        <p className="mt-1 flex items-center gap-1 text-[11.5px] text-muted"><BadgeCheck size={12} />By {extension.publisher}<span className="mx-1">·</span>v{extension.version}</p>
      </div>
    </div>
    <p className="mt-3 text-[12.5px] leading-relaxed text-muted">{extension.description}</p>
    <details className="mt-3 text-[11.5px] text-muted"><summary className="cursor-pointer">What this extension uses</summary><ul className="mt-2 space-y-1 pl-4 list-disc">{extension.capabilities.map((capability) => <li key={capability}>{capability}</li>)}</ul></details>
    <div aria-live="polite">
      {status.detail && <p data-testid={`extension-${id}-status`} className={cx("selectable mt-3 text-[12px]", status.tone === "danger" ? "text-danger" : status.tone === "attention" ? "text-attention" : "text-muted")}>{status.detail}</p>}
      {error && <p role="alert" data-testid={`extension-${id}-error`} className="selectable mt-3 rounded-md border border-danger/30 bg-danger/5 px-3 py-2 text-[12px] text-danger">{error}</p>}
    </div>

    {pkg && installed && <button
      type="button"
      role="switch"
      aria-checked={pkg.auto_update}
      data-testid={`extension-${id}-auto-update`}
      disabled={pending !== null}
      onClick={() => run("auto-update", () => client.call("extensions.configure", { id, auto_update: !pkg.auto_update }))}
      className="mt-4 flex w-full items-center justify-between gap-3 rounded-md border border-line bg-bg/40 px-3 py-2 text-left disabled:opacity-60"
    >
      <span><span className="block text-[12.5px] text-fg">Update automatically</span><span className="mt-0.5 block text-[11.5px] text-muted">{pkg.auto_update ? "New versions download and apply on their own." : "New versions wait for you to update."}</span></span>
      <Toggle on={pkg.auto_update} />
    </button>}

    {confirmRemove ? <div className="mt-4 rounded-md border border-danger/30 bg-danger/5 p-3" data-testid={`extension-${id}-confirm-remove`}>
      <p className="text-[12px] text-fg">Remove {name}? Its views close. Its saved data stays on this computer for a reinstall.</p>
      <div className="mt-2 flex justify-end gap-2">
        <Button size="sm" variant="ghost" onClick={() => setConfirmRemove(false)}>Cancel</Button>
        <Button size="sm" variant="danger" busy={pending === "remove"} disabled={pending !== null} onClick={() => { setConfirmRemove(false); manage("remove"); }} data-testid={`extension-${id}-remove-confirm`}>Remove</Button>
      </div>
    </div> : <div className="mt-4 flex flex-wrap items-center justify-end gap-2">
      {pkg && !installed && <Button size="sm" variant="primary" busy={pending === "install"} disabled={blocked} onClick={() => manage("install")} data-testid={`extension-${id}-install`}><Download size={12} />Install</Button>}
      {pkg && installed && <>
        {pkg.can_restore && <Button size="sm" variant="ghost" busy={pending === "restore"} disabled={blocked} onClick={() => manage("restore")} data-testid={`extension-${id}-restore`} title="Go back to the version installed before"><RotateCcw size={12} />Restore previous</Button>}
        <Button size="sm" variant="ghost" busy={pending === "remove"} disabled={blocked} onClick={() => setConfirmRemove(true)} data-testid={`extension-${id}-remove`}><Trash2 size={12} />Remove</Button>
        {pkg.phase === "missing" || pkg.phase === "failed"
          ? <Button size="sm" busy={pending === "install"} disabled={blocked} onClick={() => manage("install")} data-testid={`extension-${id}-install`}><Download size={12} />{pkg.phase === "failed" ? "Reinstall" : "Install"}</Button>
          : pkg.available && pkg.phase === "installed"
            ? <Button size="sm" variant="primary" busy={pending === "update"} disabled={blocked} onClick={() => manage("update")} data-testid={`extension-${id}-update`}><Download size={12} />Update to {pkg.available}</Button>
            : <Button size="sm" busy={pending === "check"} disabled={blocked} onClick={() => manage("check")} data-testid={`extension-${id}-check`}><RefreshCw size={12} />Check for updates</Button>}
      </>}
      {installed && <Button size="sm" variant={extension.enabled ? "secondary" : "primary"} busy={pending === "enable" || pending === "disable"} disabled={pending !== null} onClick={() => run(extension.enabled ? "disable" : "enable", () => client.call("extensions.set", { id, enabled: !extension.enabled }))} data-testid={`extension-${id}-toggle`} aria-label={`${extension.enabled ? "Disable" : "Enable"} ${name}`}>
        {extension.enabled ? "Disable" : "Enable"}
      </Button>}
    </div>}
  </article>;
}
