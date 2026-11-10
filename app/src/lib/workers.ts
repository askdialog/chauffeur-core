// GCP workers (docs/cloud-workers.md): VMs the daemon creates with the user's gcloud, installs
// Chauffeur on and declares as transfer machines. What Settings → Transfers and the transfer dialog
// show of them. French or English, as the transfer copy is.
import type { OpenApp, WorkerInfo, WorkerMachineType, WorkersResult } from "@chauffeur/protocol";

export function workerCopy(french: boolean) {
  return french
    ? {
        section: "Workers GCP",
        intro: (project: string, zone: string) => !project.trim()
          ? "Renseignez votre projet Google Cloud ci-dessus pour préparer une destination."
          : `Chauffeur crée une VM dans ${project} (${zone}) avec votre gcloud, y installe Chauffeur, Claude Code et Codex, et en fait une machine de transfert. Elle est facturée tant qu'elle existe, même sans workspace.`,
        none: "Aucun worker.",
        name: "Nom",
        machineType: "Type de machine",
        create: "Créer un worker…",
        confirmTitle: (name: string) => `Créer le worker ${name}`,
        confirm: "Créer",
        estimate: (type: WorkerMachineType, disk: number, zone: string, project: string, cost: string) =>
          `${type.name} (${type.vcpus} vCPU, ${type.memory_gb} Go), disque de ${disk} Go, dans ${zone} (${project}) : environ ${cost} tant qu'il existe.`,
        estimateNote: (date: string) => `Prix de Paris au ${date} ; trafic réseau et usage des modèles non compris.`,
        beyondLimit: (count: number, limit: number) =>
          `${count} workers existent déjà ; au-delà de ${limit}, Chauffeur demande confirmation.`,
        creating: "Création de la VM…",
        installing: "Installation de Chauffeur…",
        ready: "Prêt",
        reconnecting: "Prêt, reconnexion…",
        failed: "Échec",
        deleting: "Suppression…",
        stopping: "Arrêt…",
        stopped: "Arrêté, disque seul facturé",
        retry: "Réessayer",
        delete: (name: string) => `Supprimer ${name}`,
        deleteTitle: (name: string) => `Supprimer le worker ${name}`,
        deleteExplain: (instance: string) =>
          `La VM ${instance} et son disque sont supprimés. Un workspace qui y tourne doit d'abord revenir ici.`,
        deleteConfirm: "Supprimer",
        cancel: "Annuler",
        details: "Détails",
        created: (name: string) => `Création du worker ${name}`,
        nameHint: "lettres minuscules, chiffres et tirets",
        worker: "worker GCP",
      }
    : {
        section: "GCP workers",
        intro: (project: string, zone: string) => !project.trim()
          ? "Enter your Google Cloud project above to prepare a destination."
          : `Chauffeur creates a VM in ${project} (${zone}) with your gcloud, installs Chauffeur, Claude Code and Codex there, and makes it a transfer machine. It is billed while it exists, even without a workspace.`,
        none: "No worker.",
        name: "Name",
        machineType: "Machine type",
        create: "Create a worker…",
        confirmTitle: (name: string) => `Create the worker ${name}`,
        confirm: "Create",
        estimate: (type: WorkerMachineType, disk: number, zone: string, project: string, cost: string) =>
          `${type.name} (${type.vcpus} vCPU, ${type.memory_gb} GB), a ${disk} GB disk, in ${zone} (${project}): about ${cost} while it exists.`,
        estimateNote: (date: string) => `Paris prices of ${date}; network traffic and model use not included.`,
        beyondLimit: (count: number, limit: number) =>
          `${count} workers already exist; beyond ${limit}, Chauffeur asks for confirmation.`,
        creating: "Creating the VM…",
        installing: "Installing Chauffeur…",
        ready: "Ready",
        reconnecting: "Ready, reconnecting…",
        failed: "Failed",
        deleting: "Deleting…",
        stopping: "Stopping…",
        stopped: "Stopped, only its disk billed",
        retry: "Retry",
        delete: (name: string) => `Delete ${name}`,
        deleteTitle: (name: string) => `Delete the worker ${name}`,
        deleteExplain: (instance: string) =>
          `The VM ${instance} and its disk are deleted. A workspace that runs there must come back here first.`,
        deleteConfirm: "Delete",
        cancel: "Cancel",
        details: "Details",
        created: (name: string) => `Creating the worker ${name}`,
        nameHint: "lowercase letters, digits and dashes",
        worker: "GCP worker",
      };
}

export type WorkerCopy = ReturnType<typeof workerCopy>;

/** An hourly cost in US dollars, rounded to the cent: "$0.17/h", "0,17 $/h". */
export function hourly(usd: number, french: boolean): string {
  return french ? `${usd.toFixed(2).replace(".", ",")} $/h` : `$${usd.toFixed(2)}/h`;
}

/** What a worker is doing, and how it reads. */
export function workerStatus(
  worker: WorkerInfo,
  copy: WorkerCopy,
): { label: string; tone: "success" | "working" | "danger" | "attention" | "neutral" } {
  switch (worker.state) {
    case "ready":
      return worker.connected ? { label: copy.ready, tone: "success" } : { label: copy.reconnecting, tone: "attention" };
    case "creating":
      return { label: copy.creating, tone: "working" };
    case "installing":
      return { label: copy.installing, tone: "working" };
    case "deleting":
      return { label: copy.deleting, tone: "working" };
    case "stopping":
      return { label: copy.stopping, tone: "working" };
    case "stopped":
      return { label: copy.stopped, tone: "neutral" };
    case "failed":
      return { label: copy.failed, tone: "danger" };
  }
}

/** A worker the daemon is creating, stopping or deleting: the list is read again soon. */
export function changing(workers: WorkerInfo[]): boolean {
  return workers.some(inProgress);
}

/** The daemon is changing it: nothing else can be asked of it meanwhile. */
export function inProgress(worker: Pick<WorkerInfo, "state">): boolean {
  return ["creating", "installing", "deleting", "stopping"].includes(worker.state);
}

/** The first free name of the form `worker-<n>`. */
export function nextWorkerName(workers: Pick<WorkerInfo, "name">[]): string {
  for (let n = 1; ; n++) {
    const name = `worker-${n}`;
    if (!workers.some((w) => w.name === name)) return name;
  }
}

/** The name the daemon takes: up to 20 lowercase letters, digits and dashes, from a letter. */
export function validWorkerName(name: string): boolean {
  return /^[a-z]([a-z0-9-]{0,18}[a-z0-9])?$/.test(name);
}

/** Creating one more worker goes past the limit the daemon creates without asking again. */
export function beyondLimit(result: Pick<WorkersResult, "workers" | "limit">): boolean {
  return result.workers.length >= result.limit;
}

/** How this machine's editors open a workspace moved to `machine`: its worker's ssh host once the
 * entry is written and included, else the daemon's reason (with the `Include` line to add by hand);
 * null for a machine declared by hand, which opens nowhere from here. */
export type RemoteOpen = { machine: string; host: string | null; issue: string | null };

export function remoteOpen(machine: string, workers: WorkerInfo[]): RemoteOpen | null {
  const worker = workers.find((w) => w.name === machine);
  if (!worker) return null;
  const host = worker.state === "ready" ? worker.ssh_host : null;
  return { machine, host, issue: host ? null : (worker.ssh_issue ?? `${machine} does not open in an editor here yet`) };
}

/** The editors that open a folder on a worker over SSH (Remote-SSH). */
export function remoteEditors(apps: OpenApp[]): OpenApp[] {
  return apps.filter((a) => a.id === "cursor" || a.id === "vscode");
}

/** Asks whether a workspace moved to `machine` opens in an editor here, telling `onRemote` how, and
 * asks again each time `onFocus` fires (the window comes back: the user may have added the Include
 * line by hand). A failure is reported once until a read succeeds; an answer coming after a newer
 * question, or after the returned cancel (the workspace came back, or moved elsewhere), is dropped. */
export function watchRemote(
  machine: string,
  list: () => Promise<Pick<WorkersResult, "workers">>,
  onRemote: (remote: RemoteOpen | null) => void,
  onError: (error: unknown) => void,
  onFocus: (listener: () => void) => () => void,
): () => void {
  let current = true;
  let asked = 0;
  let failed = false;
  const ask = () => {
    const question = ++asked;
    list()
      .then((result) => {
        if (!current || question !== asked) return;
        failed = false;
        onRemote(remoteOpen(machine, result.workers));
      })
      .catch((error) => {
        if (!current || question !== asked || failed) return;
        failed = true;
        onError(error);
      });
  };
  ask();
  const unsubscribe = onFocus(ask);
  return () => {
    current = false;
    unsubscribe();
  };
}
