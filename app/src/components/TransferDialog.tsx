// Transferring workspaces to another machine, or bringing one back, from the sidebar or a header.
// The Mac's daemon drives each transfer in the background: the dialog confirms what goes where and
// starts them; a workspace's header follows its transfer, the batch view a batch's.

import type { TransferEndpoint, TransferSettings, WorkersResult, WorkspaceInfo } from "@chauffeur/protocol";
import { XCircle } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { workspaceName } from "@/lib/format";
import { errorMessage } from "@/lib/rpc";
import { batchNote, batchUnavailable, type TransferTarget, transferCopy, transferTargets } from "@/lib/transfer";
import { hourly, workerCopy } from "@/lib/workers";
import { client, openSettings, selectWorkspace } from "@/state/app";
import { startBatch } from "@/state/transfers";
import { Button, cx, Modal, Spinner, StatusDot } from "./ui";

export function TransferDialog({ workspaces, onClose }: { workspaces: WorkspaceInfo[]; onClose: () => void }) {
  const french = navigator.language.startsWith("fr"),
    copy = transferCopy(french);
  const single = workspaces.length === 1 ? workspaces[0]! : null;
  const back = single?.moved_to ?? null;
  // What goes, and why the others do not.
  const going = back ? [] : workspaces.filter((w) => !batchUnavailable(w, copy, french));
  const busyAgents = going.some((w) => batchNote(w, copy));
  const [machines, setMachines] = useState<TransferTarget[] | null>(null);
  // Going: where to. Coming back: where it runs, the machine it moved to when that is declared.
  const choiceMade = useRef(false);
  const [machine, setMachine] = useState<string | null>(null);
  const [offer, setOffer] = useState<WorkersResult | null>(null);
  const [automatic, setAutomatic] = useState(false);
  const [beyondLimit, setBeyondLimit] = useState(false);
  const [keep, setKeep] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cloudError, setCloudError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    let settings: TransferSettings | null = null;
    let workers: WorkersResult | null = null;
    const refresh = () => {
      if (disposed) return;
      const targets = transferTargets(settings ?? { controller: "", machines: [] }, workers?.workers ?? []);
      setMachines(targets);
      setMachine((chosen) => chosen && targets.some((target) => target.name === chosen)
        ? chosen : (back ? targets.find((target) => target.name === back)?.name : targets[0]?.name) ?? null);
    };
    void client.call("settings.get", {}).then((result) => {
      settings = result.transfer;
      refresh();
    }).catch((error) => { if (!disposed) { setError(errorMessage(error)); refresh(); } });
    void client.call("worker.list", {}).then((result) => {
      if (disposed) return;
      workers = result;
      setOffer(result);
      // A configured machine can be selected while the Cloud request is still in flight.
      if (!back && !choiceMade.current && !settings?.machines.length) setAutomatic(!!result.project.trim());
      refresh();
    }).catch((error) => { if (!disposed) { setCloudError(errorMessage(error)); refresh(); } });
    return () => { disposed = true; };
  }, [back]);
  // Moved by another daemon, or to a machine since renamed: the user says where it runs.
  const choose = !back || (machines !== null && !machines.some((m) => m.name === back));

  const cloud = !back && automatic && offer !== null;
  const destination: TransferEndpoint = cloud
    ? { kind: "cloud", beyond_limit: beyondLimit }
    : { kind: "machine", name: machine ?? "" };
  const target = cloud ? "Cloud" : machine;
  const limit = cloud && offer.workers.length >= offer.limit;
  const cloudConfigured = !!offer?.project.trim();
  const canStart = !!target && (!cloud || cloudConfigured) && (!limit || beyondLimit);
  const estimate = offer?.machine_types.find((m) => m.name === offer.default_machine_type);

  const start = (interrupt: boolean) => {
    if (!canStart) return;
    void startBatch(
      single ? [single] : going,
      back ? (french ? "cet ordinateur" : "this computer") : target!,
      interrupt,
      back ? { kind: "here" } : destination,
      back ? { kind: "machine", name: machine! } : { kind: "here" },
      !!back && keep,
    );
    if (single) selectWorkspace(single.id);
    onClose();
  };

  const none = machines !== null && machines.length === 0 && !cloudConfigured;
  const title = single ? (back ? copy.backTitle(workspaceName(single)) : copy.dialogTitle(workspaceName(single))) : copy.batchTitle(workspaces.length);
  const primary = back ? copy.startBack : single ? copy.start(target ?? "…") : copy.startMany(going.length, target ?? "…");
  return (
    <Modal
      open
      onClose={onClose}
      title={title}
      width={single ? 480 : 520}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {copy.cancel}
          </Button>
          {none ? (
            <Button
              variant="primary"
              onClick={() => {
                onClose();
                openSettings(true, "transfer");
              }}
            >
              {copy.openSettings}
            </Button>
          ) : (
            <>
              {busyAgents && (
                <Button
                  onClick={() => void start(true)}
                  disabled={!canStart}
                  title={copy.interruptHint}
                  data-testid="confirm-transfer-interrupt"
                >
                  {copy.interrupt}
                </Button>
              )}
              <Button
                variant="primary"
                onClick={() => void start(false)}
                disabled={!canStart || (!back && going.length === 0)}
                data-testid="confirm-transfer"
              >
                {primary}
              </Button>
            </>
          )}
        </>
      }
    >
      <div className="space-y-3 text-[12.5px] text-muted">
        {!single && (
          <ul className="max-h-56 space-y-0.5 overflow-y-auto rounded-md border border-line p-1" data-testid="transfer-batch-list">
            {workspaces.map((w) => {
              const unavailable = batchUnavailable(w, copy, french);
              const note = unavailable ?? batchNote(w, copy);
              return (
                <li key={w.id} className={cx("flex items-center gap-2 rounded px-2 py-1", unavailable && "opacity-60")}>
                  <StatusDot workspace={w} />
                  <span className={cx("min-w-0 flex-1 truncate", unavailable ? "text-faint line-through" : "text-fg")}>
                    {workspaceName(w)}
                  </span>
                  {note && <span className={cx("shrink-0 text-[11.5px]", unavailable ? "text-faint" : "text-attention")}>{note}</span>}
                </li>
              );
            })}
          </ul>
        )}
        <p>{back ? copy.explainBack(back) : single ? copy.explain : copy.batchExplain}</p>
        <p>{french ? "Vous pouvez fermer Chauffeur. Gardez cet ordinateur éveillé jusqu’à la fin du transfert ; le pilotage mobile continue de passer par lui ensuite." : "You can close Chauffeur. Keep this computer awake until the transfer finishes; mobile control still connects through it afterwards."}</p>
        {cloudError && <p role="alert" className="text-attention">{french ? "Cloud indisponible : " : "Cloud unavailable: "}{cloudError}</p>}
        {!back && offer && !cloudConfigured && <Button onClick={() => { onClose(); openSettings(true, "transfer"); }}>{french ? "Configurer Cloud" : "Configure Cloud"}</Button>}
        {single && !back && batchNote(single, copy) && <p className="text-attention first-letter:uppercase">{batchNote(single, copy)}</p>}
        {machines === null && !error && <Spinner />}
        {!back && offer && estimate && cloudConfigured && (
          <div className="space-y-2 rounded-md border border-line px-3 py-2">
            <label className="flex items-start gap-2">
              <input type="checkbox" checked={automatic} onChange={(e) => { choiceMade.current = true; setAutomatic(e.target.checked); }} data-testid="automatic-worker" />
              <span>{french ? "Cloud à la demande" : "On-demand Cloud"}
                <span className="block text-[12.5px] leading-relaxed text-muted">{french
                  ? `Réutilise un worker automatique, arrêté ou non, ou crée un ${estimate.name} dans ${offer.zone} (${offer.project}), environ ${hourly(estimate.hourly_usd, french)} avec disque et adresse. Arrête la VM au retour du dernier workspace sur cet ordinateur : seul son disque reste facturé, et ses agents restent connectés pour la fois suivante. Réseau et modèles en supplément ; prix du ${offer.prices_as_of}.`
                  : `Reuses an automatic worker, stopped or not, or creates ${estimate.name} in ${offer.zone} (${offer.project}), about ${hourly(estimate.hourly_usd, french)} including disk and address. Stops the VM after its last workspace returns to this computer: only its disk stays billed, and its agents stay logged in for next time. Network and models extra; prices as of ${offer.prices_as_of}.`}</span>
              </span>
            </label>
            {limit && <label className="flex items-start gap-2 text-attention">
              <input type="checkbox" checked={beyondLimit} onChange={(e) => setBeyondLimit(e.target.checked)} />
              {french ? `J'accepte la création au-delà de la limite de ${offer.limit} workers si nécessaire.` : `I accept creating beyond the limit of ${offer.limit} workers if needed.`}
            </label>}
          </div>
        )}
        {none && <p className="text-attention">{copy.noMachines}</p>}
        {!back && (cloud || machines?.find((m) => m.name === machine)?.worker) && (
          <p className="text-attention">{french
            ? "Si ce worker doit être mis à jour, les agents de tous ses workspaces seront interrompus avant le transfert. Vérifiez les autres travaux en cours avant de continuer."
            : "If this worker needs an update, agents in all its workspaces will be interrupted before the transfer. Check its other ongoing work before continuing."}</p>
        )}
        {back && choose && machines && machines.length > 0 && <p>{copy.whereNow}</p>}
        {choose && !cloud && machines && machines.length > 0 && (
          <div role="radiogroup" aria-label={copy.transferTo} className="space-y-1" data-testid="transfer-machines-choice">
            {machines.map((m) => (
              <label
                key={m.name}
                className={cx(
                  "flex cursor-pointer items-center gap-2 rounded-md border px-3 py-2",
                  machine === m.name ? "border-accent/50 bg-accent/5 text-fg" : "border-line hover:bg-hover/60",
                )}
              >
                <input type="radio" name="machine" checked={machine === m.name} onChange={() => { choiceMade.current = true; setMachine(m.name); }} />
                <span className="font-medium">{m.name}</span>
                {m.worker ? (
                  <span className="truncate text-[12px] text-muted">
                    {workerCopy(french).worker} · {m.worker.machine_type} · {hourly(m.worker.hourly_usd, french)}
                  </span>
                ) : (
                  <span className="truncate font-mono text-[11px] text-faint">{m.socket}</span>
                )}
              </label>
            ))}
          </div>
        )}
        {back && (
          <label className="flex items-start gap-2">
            <input type="checkbox" className="mt-0.5" checked={keep} onChange={(event) => setKeep(event.target.checked)} />
            <span>
              {copy.keepLocal}
              <span className="block text-[11.5px] text-faint">{copy.keepLocalHint}</span>
            </span>
          </label>
        )}
        {!back && <p className="text-faint">{copy.login}</p>}
        {error && (
          <div className="flex items-start gap-2 whitespace-pre-wrap rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-[12.5px] text-danger selectable">
            <XCircle size={14} className="mt-0.5 shrink-0" />
            <span>{error}</span>
          </div>
        )}
      </div>
    </Modal>
  );
}
