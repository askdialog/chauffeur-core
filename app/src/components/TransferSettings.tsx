import { useEffect, useState } from "react";
import type { AppSettings, SettingsUpdate, TransferMachine, WorkerConfiguration } from "@chauffeur/protocol";
import { Plus, X } from "lucide-react";
import { transferCopy } from "@/lib/transfer";
import { Button, cx, Field, IconButton, inputClass } from "./ui";

// In-memory drafts survive switching settings sections; each daemon's file owns its draft.
const drafts = new Map<string, { controller: string; rows: TransferMachine[] }>();
const cloudDrafts = new Map<string, WorkerConfiguration>();

/** Settings → Transfers: `[transfer]`, the controller's socket and the machines, saved whole. */
export function TransferSettings({
  settings,
  update,
  busy,
}: {
  settings: AppSettings;
  update: (change: SettingsUpdate) => Promise<boolean>;
  busy: boolean;
}) {
  const french = navigator.language.startsWith("fr"),
    copy = transferCopy(french);
  const saved = settings.transfer;
  const [controller, setController] = useState(() => drafts.get(settings.file)?.controller ?? saved.controller);
  const [rows, setRows] = useState<TransferMachine[]>(() => drafts.get(settings.file)?.rows ?? saved.machines);
  const [problem, setProblem] = useState<string | null>(null);
  const dirty = controller !== saved.controller || JSON.stringify(rows) !== JSON.stringify(saved.machines);
  useEffect(() => {
    if (dirty) drafts.set(settings.file, { controller, rows });
    else drafts.delete(settings.file);
  }, [controller, rows, dirty, settings.file]);
  const save = (next: { controller?: string; machines?: TransferMachine[] }) => {
    const machines = (next.machines ?? rows)
      .map((m) => ({ name: m.name.trim(), socket: m.socket.trim() }))
      .filter((m) => m.name || m.socket);
    // A row being filled in, or emptied of one field, stays here until it has both: saving
    // without it would drop the machine; the cross removes one.
    if (machines.some((m) => !m.name || !m.socket)) {
      setProblem(french ? "Complétez le nom et la connexion de chaque machine avant d’enregistrer." : "Complete each machine’s name and connection before saving.");
      return;
    }
    setProblem(null);
    const transfer = { controller: (next.controller ?? controller).trim(), machines };
    if (JSON.stringify(transfer) !== JSON.stringify(saved)) void update({ transfer }).then((saved) => {
      if (saved) { setController(transfer.controller); setRows(machines); drafts.delete(settings.file); }
    });
  };
  const edit = (i: number, change: Partial<TransferMachine>) =>
    setRows((current) => current.map((m, j) => (j === i ? { ...m, ...change } : m)));
  const onKey = (event: React.KeyboardEvent) => {
    if (event.key === "Enter") save({});
  };
  return (
    <section className="space-y-4" data-testid="transfer-settings">
      <CloudConfiguration settings={settings} update={update} busy={busy} french={french} />
      <div>
        <h3 className="text-sm font-medium text-fg">{copy.section}</h3>
        <p className="mt-2 text-[12.5px] leading-relaxed text-muted">{copy.intro}</p>
      </div>
      <details open={saved.controller.trim().length > 0}>
        <summary className="cursor-pointer text-[12px] text-muted">{copy.controller}</summary>
        <Field label={copy.controller}>
          <p className="mb-1.5 text-[11.5px] text-faint">{copy.controllerHint}</p>
          <input
            value={controller}
            onChange={(event) => setController(event.target.value)}
            onKeyDown={onKey}
            disabled={busy}
            spellCheck={false}
            className={cx(inputClass, "font-mono text-[12px]")}
            data-testid="transfer-controller"
          />
        </Field>
      </details>
      <Field label={copy.machines}>
        <div className="space-y-1.5" data-testid="transfer-machines">
          {rows.map((machine, i) => (
            <div key={i} className="flex items-center gap-1.5">
              <span className="w-32 shrink-0">
                <input
                  value={machine.name}
                  placeholder={copy.machineName}
                  onChange={(event) => edit(i, { name: event.target.value })}
                  onKeyDown={onKey}
                  disabled={busy}
                  spellCheck={false}
                  className={cx(inputClass, "text-[12px]")}
                />
              </span>
              <span className="min-w-0 flex-1">
                <input
                  value={machine.socket}
                  placeholder={copy.machineSocket}
                  onChange={(event) => edit(i, { socket: event.target.value })}
                  onKeyDown={onKey}
                  disabled={busy}
                  spellCheck={false}
                  className={cx(inputClass, "font-mono text-[12px]")}
                />
              </span>
              <IconButton
                title={copy.remove(machine.name)}
                onClick={() => {
                  const next = rows.filter((_, j) => j !== i);
                  setRows(next);
                  save({ machines: next });
                }}
                disabled={busy}
              >
                <X size={12} />
              </IconButton>
            </div>
          ))}
          <Button
            size="sm"
            variant="ghost"
            onClick={() => setRows((current) => [...current, { name: "", socket: "" }])}
            disabled={busy}
            data-testid="transfer-add-machine"
          >
            <Plus size={12} /> {copy.addMachine}
          </Button>
        </div>
      </Field>
      {problem && <p role="alert" className="text-[12px] text-danger">{problem}</p>}
      {dirty && <div className="flex items-center gap-2 text-[12px]">
        <span className="text-attention">{french ? "Non enregistré · brouillon conservé" : "Unsaved · draft retained"}</span>
        <Button size="sm" disabled={busy} onClick={() => save({})}>{french ? "Enregistrer" : "Save"}</Button>
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => { setController(saved.controller); setRows(saved.machines); setProblem(null); drafts.delete(settings.file); }}>{french ? "Annuler les modifications" : "Discard changes"}</Button>
      </div>}
    </section>
  );
}

function CloudConfiguration({ settings, update, busy, french }: {
  settings: AppSettings; update: (change: SettingsUpdate) => Promise<boolean>; busy: boolean; french: boolean;
}) {
  const saved = settings.workers;
  const [draft, setDraft] = useState<WorkerConfiguration>(() => cloudDrafts.get(settings.file) ?? saved ?? { project: "", zone: "europe-west9-a" });
  const dirty = JSON.stringify(draft) !== JSON.stringify(saved);
  useEffect(() => {
    if (dirty) cloudDrafts.set(settings.file, draft);
    else cloudDrafts.delete(settings.file);
  }, [draft, dirty, settings.file]);
  if (!saved) return <p className="text-attention">{french ? "Mettez Chauffeur à jour pour configurer Cloud ici." : "Update Chauffeur to configure Cloud here."}</p>;
  return <div className="space-y-3 border-b border-line pb-4" data-testid="cloud-configuration">
    <h3 className="text-sm font-medium text-fg">{french ? "Cloud géré" : "Managed Cloud"}</h3>
    <p className="text-[12.5px] leading-relaxed text-muted">{french ? "Choisissez le projet de votre compte Google Cloud. L’enregistrement ne crée aucune VM ; le coût est présenté avant chaque transfert." : "Choose the project for your Google Cloud account. Saving creates no VM; the estimated cost is shown before each transfer."}</p>
    <Field label={french ? "Identifiant du projet Google Cloud" : "Google Cloud project ID"}>
      <input className={inputClass} value={draft.project} placeholder="my-project" disabled={busy} spellCheck={false} onChange={(event) => setDraft({ ...draft, project: event.target.value })} data-testid="worker-project" />
    </Field>
    <Field label={french ? "Zone (Paris)" : "Zone (Paris)"}>
      <select className={inputClass} value={draft.zone} disabled={busy} onChange={(event) => setDraft({ ...draft, zone: event.target.value })}>
        {[...new Set([saved.zone, "europe-west9-a", "europe-west9-b", "europe-west9-c"])].map((zone) => <option key={zone}>{zone}</option>)}
      </select>
    </Field>
    <div className="flex items-center gap-2">
      <Button size="sm" disabled={busy || !dirty} onClick={() => void update({ workers: { project: draft.project.trim(), zone: draft.zone } }).then((ok) => { if (ok) { setDraft({ ...draft, project: draft.project.trim() }); cloudDrafts.delete(settings.file); } })}>{french ? "Enregistrer Cloud" : "Save Cloud settings"}</Button>
      <span role="status" className="text-[12px] text-muted">{dirty ? (french ? "Non enregistré" : "Unsaved") : saved.project ? (french ? "Projet configuré" : "Project configured") : (french ? "À configurer" : "Needs configuration")}</span>
    </div>
  </div>;
}
