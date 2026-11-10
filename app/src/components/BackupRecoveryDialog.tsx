// Manual recovery here: the user chooses a backup and confirms the worker-wide stop and kept disks.
import type { BackupRecoveryPlan, WorkspaceInfo } from "@chauffeur/protocol";
import { useEffect, useState } from "react";
import { errorMessage } from "@/lib/rpc";
import { client, toast } from "@/state/app";
import { Button, Modal, Spinner } from "./ui";

export function BackupRecoveryDialog({ workspace, onClose }: { workspace: WorkspaceInfo; onClose: () => void }) {
  const french = navigator.language.startsWith("fr");
  const [plan, setPlan] = useState<BackupRecoveryPlan | null>(null);
  const [backup, setBackup] = useState("");
  const [keep, setKeep] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    setPlan(null);
    setBackup("");
    setConfirmed(false);
    setKeep(false);
    setError(null);
    client.call("cloud.backup.recovery_plan", { workspace: workspace.id })
      .then((result) => {
        if (!active) return;
        setPlan(result);
        setBackup(result.backups[0]?.id ?? "");
      })
      .catch((err) => { if (active) setError(errorMessage(err)); });
    return () => { active = false; };
  }, [workspace.id]);
  const recover = async () => {
    if (!plan || !backup || !confirmed) return;
    setBusy(true);
    setError(null);
    try {
      const result = await client.call("cloud.backup.recover", {
        workspace: plan.workspace, worker: plan.worker, epoch: plan.epoch, backup,
        affected_workspaces: plan.affected_workspaces, keep_local_copy: keep,
      });
      toast(french ? `Restauré ici. Disque conservé et toujours facturé : ${result.retained_disk}`
        : `Recovered here. Kept disk still costs storage: ${result.retained_disk}`);
      onClose();
    } catch (err) {
      setError(errorMessage(err));
    } finally { setBusy(false); }
  };
  return (
    <Modal open onClose={() => { if (!busy) onClose(); }} width={520}
      title={french ? "Restaurer une sauvegarde ici" : "Recover a backup here"}
      footer={<>
        <Button variant="ghost" disabled={busy} onClick={onClose}>{french ? "Annuler" : "Cancel"}</Button>
        <Button variant="danger" busy={busy} disabled={!plan || !backup || !confirmed} onClick={() => void recover()}>
          {french ? "Supprimer la VM et restaurer" : "Delete VM and recover"}
        </Button>
      </>}>
      <div className="space-y-3 text-[12.5px] text-muted">
        {!plan && !error && <Spinner size={14} />}
        {error && <p role="alert" className="text-danger">{error}</p>}
        {plan && <>
          <p>{french ? `L’ancienne VM de ${plan.worker} sera supprimée avant toute reprise. Tous ces workspaces seront arrêtés :`
            : `The old VM of ${plan.worker} will be deleted before anything resumes. All these workspaces will stop:`}</p>
          <p className="text-fg">{plan.affected_workspaces.join(", ")}</p>
          <p>{french ? "Ses disques seront conservés pour récupérer manuellement du travail plus récent. Ils resteront facturés."
            : "Its disks will be kept to recover newer work manually. They will still cost storage."}</p>
          <label className="block space-y-1">
            <span>{french ? "Sauvegarde" : "Backup"}</span>
            <select value={backup} disabled={busy} onChange={(e) => { setBackup(e.target.value); setConfirmed(false); }}
              className="block w-full rounded border border-line bg-bg px-2 py-1 text-fg">
              {plan.backups.map((b) => <option key={b.id} value={b.id}>{new Date(b.taken_at).toLocaleString()}</option>)}
            </select>
          </label>
          {!plan.backups.length && <p>{french ? "Aucune sauvegarde trouvée dans le stockage." : "No backup found in storage."}</p>}
          <p className="text-attention">{french ? "Le travail plus récent peut être perdu. Les bases de données ne sont pas sauvegardées ; leur présence bloque cette restauration."
            : "Newer work may be lost. Databases are not backed up; workspaces declaring services cannot be recovered this way yet."}</p>
          <label className="flex items-start gap-2"><input type="checkbox" checked={keep} disabled={busy} onChange={(e) => setKeep(e.target.checked)} />
            {french ? "Conserver aussi la copie locale à côté du workspace restauré." : "Keep the local copy beside the recovered workspace too."}</label>
          <label className="flex items-start gap-2"><input type="checkbox" checked={confirmed} disabled={busy} onChange={(e) => setConfirmed(e.target.checked)} />
            {french ? "Je confirme l’arrêt de ces workspaces et la restauration de cette sauvegarde." : "I confirm stopping these workspaces and recovering this backup."}</label>
          {busy && <p role="status">{french ? "Restauration en cours. Si la connexion est coupée, réessayer avec la même sauvegarde." : "Recovering. If disconnected, retry with the same backup."}</p>}
        </>}
      </div>
    </Modal>
  );
}
