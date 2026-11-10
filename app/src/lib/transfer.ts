// Moving a workspace to another machine and back (docs/cloud-workspace-transfer.md): what the app
// shows and when it offers it. The Mac's daemon drives each transfer; the app starts it and follows
// its progress. French or English, as the Cloud settings are.
import type {
  BackupHealth,
  BackupStatus,
  CloudTransferStartParams,
  TransferEndpoint,
  TransferInfo,
  TransferMachine,
  TransferSettings,
  TransferStage,
  WorkerInfo,
  WorkspaceInfo,
} from "@chauffeur/protocol";

export function transferCopy(french: boolean) {
  return french
    ? {
        section: "Transferts",
        intro:
          "Envoyez vos workspaces vers le cloud à la demande ou vers une machine déclarée ci-dessous. Chauffeur prépare automatiquement le service qui coordonne les transferts.",
        controller: "Contrôleur personnalisé (avancé)",
        controllerHint: "Laissez vide pour que Chauffeur gère le contrôleur automatiquement. Renseignez un socket uniquement pour utiliser un autre contrôleur.",
        machines: "Machines",
        machineName: "nom",
        machineSocket: "socket, par ex. /tmp/vm.sock",
        addMachine: "Ajouter une machine",
        remove: (name: string) => `Retirer ${name || "cette ligne"}`,
        transferTo: "Transférer vers",
        transferItem: "Transférer…",
        bringBack: "Ramener ici",
        noMachine: "Aucune machine : ajoutez-en ou créez un worker dans Réglages → Transferts",
        movedBadge: (machine: string) => `sur ${machine}`,
        movedBadgeTitle: (machine: string) => `Tourne sur ${machine} ; il s'y pilote d'ici`,
        idleTitle: (machine: string, since: string) =>
          `Rien ne s'est passé sur ${machine} depuis ${since} : ramenez-le ou arrêtez ce qui tourne si vous n'en avez plus besoin.`,
        idleBanner: (since: string) =>
          `Rien ne s'est passé ici depuis ${since}. Sa machine tourne, et se paie, tant qu'il y est.`,
        remindLater: "Me le rappeler dans une heure",
        idleNotification: (names: string[]) =>
          names.length === 1
            ? `${names[0]} : rien ne s'est passé depuis une heure sur sa machine. Ramenez-le, ou demandez à ce qu'on vous le rappelle dans une heure.`
            : `${names.join(", ")} : rien ne s'est passé depuis une heure sur leur machine. Ramenez-les, ou demandez à ce qu'on vous les rappelle dans une heure.`,
        movedNotification: (name: string, machine: string) => `${name} est transféré : il tourne maintenant sur ${machine}.`,
        backNotification: (name: string) => `${name} est revenu sur cet ordinateur.`,
        archivedNotification: (name: string) => `${name} est revenu sur cet ordinateur et s'y archive.`,
        failedNotification: (name: string, error: string) => `Le transfert de ${name} a échoué : ${error}`,
        needsLogin: (worker: string) =>
          `Aucun agent n'est connecté sur ${worker}. Connectez Claude Code ou Codex dans Réglages → Transferts : le transfert reprend tout seul ensuite.`,
        needsLoginNotification: (name: string, worker: string) =>
          `Pour transférer ${name}, connectez Claude Code ou Codex sur ${worker} dans Réglages → Transferts. Le transfert reprend tout seul ensuite.`,
        dialogTitle: (name: string) => `Transférer ${name}`,
        backTitle: (name: string) => `Ramener ${name}`,
        explain:
          "Les agents et services du workspace s'arrêtent ici, puis il est capturé, chiffré et restauré sur l'autre machine avec ses conversations. Il se pilote toujours d'ici, à travers l'autre machine, jusqu'à son retour.",
        explainBack: (machine: string) =>
          `Le workspace s'arrête sur ${machine} et revient ici avec ce qui y a été fait. Sa copie sur ${machine} est ensuite supprimée.`,
        keepLocal: "Garder la copie de cet ordinateur si elle a été modifiée depuis le départ",
        keepLocalHint: "Elle reste à côté du workspace revenu ; sinon, le retour est refusé et nomme ce qui a changé.",
        login:
          "Sur la destination, les agents utilisent la connexion de cette machine ; celle d'un worker GCP se fait d'ici si le transfert en a besoin.",
        noMachines:
          "Aucune destination disponible : vérifiez les réglages de transfert.",
        openSettings: "Ouvrir les réglages",
        start: (machine: string) => `Transférer vers ${machine}`,
        startBack: "Ramener",
        whereNow: "Sur quelle machine tourne-t-il maintenant ?",
        cancel: "Annuler",
        started: (name: string, machine: string) => `${name} part vers ${machine}`,
        startedBack: (name: string) => `${name} revient`,
        running: (machine: string) => `Transfert vers ${machine}…`,
        archivingBack: (machine: string) => `Archivage : retour depuis ${machine} d'abord…`,
        archivedBack: (machine: string) => `Revenu de ${machine} ; archivé ici.`,
        archivingStarted: (name: string, machine: string) =>
          `${name} revient de ${machine}, ses agents interrompus, puis s'archive ici.`,
        failed: "Le transfert a échoué",
        resume: "Reprendre",
        cancelTransfer: "Annuler le transfert",
        keepLocalCopy: "Ramener en gardant cette copie",
        keepLocalCopyHint: (machine: string) =>
          `La copie laissée sur ${machine} a été modifiée depuis le départ : « Ramener en gardant cette copie » la met de côté, intacte, à côté de celle qui revient.`,
        failedHint: (source: string) =>
          `Reprendre repart de l'étape où il s'est arrêté ; Annuler le transfert rend le workspace à ${source}.`,
        untouchedHint: (source: string) =>
          `Rien n'a été arrêté : le workspace tourne toujours sur ${source}. Réessayez une fois le problème réglé, ou masquez ce message.`,
        cancelled: (name: string, source: string) => `Transfert annulé ; ${name} tourne de nouveau sur ${source}.`,
        movedBanner: (machine: string) =>
          `Ce workspace tourne sur ${machine} : il s'y pilote d'ici (chats, terminaux, scripts, fichiers), ses ports y mènent.`,
        unreachable: (machine: string) => `${machine} ne répond pas : le workspace revient dès qu'il répond.`,
        returned: (machine: string) => `Revenu de ${machine}.`,
        kept: (path: string) => `La copie qui était ici est gardée dans ${path} ; supprimez-la une fois comparée.`,
        discardPending: (machine: string, reason: string) =>
          `Sa copie sur ${machine} n'est pas encore supprimée (${reason}) ; ${machine} réessaie toutes les 10 minutes.`,
        dismiss: "Masquer",
        details: "Détails",
        stage: (stage: TransferStage, machine: string, working: number) =>
          ({
            preparing_worker: "Préparation du worker à la demande",
            preparing: `Vérification de ${machine}`,
            waiting_for_turns: `En attente de la fin des tours (${plural(working, "agent", "agents")})`,
            queued: "En file : un autre transfert se termine d'abord",
            stopping: "Arrêt et capture du workspace",
            sending: `Envoi vers ${machine}`,
            restoring: `Restauration sur ${machine}`,
            activating: `Démarrage sur ${machine}`,
          })[stage],
        interrupting: "Interruption des agents…",
        waitingHint: "Rien n'est arrêté avant la fin de leur tour.",
        interrupt: "Interrompre et transférer",
        interruptHint: "Interrompt les agents au milieu de leur tour, puis transfère",
        select: "Sélectionner",
        deselect: "Désélectionner",
        picked: (count: number) => `${count} sélectionné${count > 1 ? "s" : ""}`,
        clearPicked: "Effacer la sélection (Échap)",
        transferMany: (count: number) => `Transférer ${count} workspaces…`,
        batchTitle: (count: number) => `Transférer ${count} workspaces`,
        batchExplain:
          "Chacun part de son côté : ses agents finissent leur tour, puis il s'arrête ici et repart sur l'autre machine avec ses conversations. Un échec n'annule pas les autres.",
        agentsWorking: "agents au travail : attend la fin de leur tour",
        agentWaits: "un agent attend votre réponse : attend la fin de son tour",
        alreadyOn: (machine: string) => `déjà sur ${machine}`,
        startMany: (count: number, machine: string) => `Transférer ${count} vers ${machine}`,
        batchRunning: (machine: string) => `Transferts vers ${machine}`,
        summary: (counts: BatchCounts) =>
          [
            counts.moved && `${counts.moved} transféré${counts.moved > 1 ? "s" : ""}`,
            counts.running && `${counts.running} en cours`,
            counts.failed && `${counts.failed} échec${counts.failed > 1 ? "s" : ""}`,
            counts.cancelled && `${counts.cancelled} annulé${counts.cancelled > 1 ? "s" : ""}`,
          ]
            .filter(Boolean)
            .join(" · "),
        movedRow: (machine: string) => `Transféré sur ${machine}`,
        cancelledRow: "Annulé : tourne toujours ici",
        retry: "Réessayer",
        close: "Fermer",
        closeHint: "Fermer ne les arrête pas : le démon du Mac les pilote.",
        batchPill: (done: number, count: number) => `Transferts ${done}/${count}`,
        batchPillTitle: "Ouvrir les transferts en cours",
        backup: (view: BackupView, time: (iso: string) => string) => {
          const at = view.at ? time(view.at) : null;
          const live = view.live ? " (prise pendant un tour)" : "";
          switch (view.health) {
            case "pending":
              return "Première sauvegarde chiffrée en cours.";
            case "current":
              return `Sauvegardé, chiffré, à ${at}${live}${view.checked ? ` ; inchangé à ${time(view.checked)}` : ""}.`;
            case "stale":
              return at
                ? `Dernière sauvegarde à ${at}${live} : aucune n'a réussi depuis 15 minutes.`
                : "Aucune sauvegarde n'a réussi depuis 15 minutes.";
            case "failed":
              return at
                ? `La dernière sauvegarde a échoué ; la précédente, à ${at}${live}, est gardée.`
                : "La sauvegarde a échoué ; aucune n'est encore gardée.";
          }
        },
        backupLeftOut: (files: string[]) => `Hors sauvegarde (dans aucun dépôt) : ${files.join(", ")}`,
        backupIncomplete: (chats: string[]) =>
          `${chats.length === 1 ? "Une conversation n'a pas pu être sauvegardée" : `${chats.length} conversations n'ont pas pu être sauvegardées`}, réessayé au prochain changement, ou dans l'heure : ${chats.join(" ; ")}`,
      }
    : {
        section: "Transfers",
        intro:
          "Send workspaces to on-demand Cloud or to a machine listed below. Chauffeur automatically prepares the service that coordinates transfers.",
        controller: "Custom controller (advanced)",
        controllerHint: "Leave empty for Chauffeur to manage the controller automatically. Enter a socket only to use another controller.",
        machines: "Machines",
        machineName: "name",
        machineSocket: "socket, e.g. /tmp/vm.sock",
        addMachine: "Add a machine",
        remove: (name: string) => `Remove ${name || "this row"}`,
        transferTo: "Transfer to",
        transferItem: "Transfer…",
        bringBack: "Bring back here",
        noMachine: "No machine: add one or create a worker in Settings → Transfers",
        movedBadge: (machine: string) => `on ${machine}`,
        movedBadgeTitle: (machine: string) => `Runs on ${machine}; piloted from here`,
        idleTitle: (machine: string, since: string) =>
          `Nothing has happened on ${machine} since ${since}: bring it back, or stop what runs there if you no longer need it.`,
        idleBanner: (since: string) =>
          `Nothing has happened here since ${since}. Its machine runs, and is paid for, while it is there.`,
        remindLater: "Remind me in an hour",
        idleNotification: (names: string[]) =>
          names.length === 1
            ? `${names[0]}: nothing has happened for an hour on its machine. Bring it back, or ask to be reminded in an hour.`
            : `${names.join(", ")}: nothing has happened for an hour on their machine. Bring them back, or ask to be reminded in an hour.`,
        movedNotification: (name: string, machine: string) => `${name} is transferred: it now runs on ${machine}.`,
        backNotification: (name: string) => `${name} is back on this computer.`,
        archivedNotification: (name: string) => `${name} is back on this computer and is being archived.`,
        failedNotification: (name: string, error: string) => `Could not transfer ${name}: ${error}`,
        needsLogin: (worker: string) =>
          `No agent is signed in on ${worker}. Sign Claude Code or Codex in from Settings → Transfers: the transfer then resumes by itself.`,
        needsLoginNotification: (name: string, worker: string) =>
          `To transfer ${name}, sign Claude Code or Codex in on ${worker} from Settings → Transfers. The transfer then resumes by itself.`,
        dialogTitle: (name: string) => `Transfer ${name}`,
        backTitle: (name: string) => `Bring back ${name}`,
        explain:
          "The workspace's agents and services stop here, then it is captured, encrypted and restored on the other machine with its conversations. It is still piloted from here, through the other machine, until it comes back.",
        explainBack: (machine: string) =>
          `The workspace stops on ${machine} and comes back here with what was done there. Its copy on ${machine} is then deleted.`,
        keepLocal: "Keep this computer's copy if it was edited since it left",
        keepLocalHint: "It stays beside the returning workspace; otherwise the return is refused, naming what changed.",
        login: "On the destination, agents use that machine's own login; a GCP worker's is signed in from here when the transfer needs it.",
        noMachines: "No destination available: check the transfer settings.",
        openSettings: "Open Settings",
        start: (machine: string) => `Transfer to ${machine}`,
        startBack: "Bring back",
        whereNow: "Which machine does it run on now?",
        cancel: "Cancel",
        started: (name: string, machine: string) => `${name} is moving to ${machine}`,
        startedBack: (name: string) => `${name} is coming back`,
        running: (machine: string) => `Transferring to ${machine}…`,
        archivingBack: (machine: string) => `Archiving: coming back from ${machine} first…`,
        archivedBack: (machine: string) => `Back from ${machine}; archived here.`,
        archivingStarted: (name: string, machine: string) =>
          `${name} comes back from ${machine}, its agents interrupted, then is archived here.`,
        failed: "The transfer failed",
        resume: "Resume",
        cancelTransfer: "Cancel the transfer",
        keepLocalCopy: "Bring back keeping this copy",
        keepLocalCopyHint: (machine: string) =>
          `The copy left on ${machine} was edited since it left: Bring back keeping this copy sets it aside, untouched, beside the returning one.`,
        failedHint: (source: string) =>
          `Resume goes on from the step where it stopped; Cancel the transfer gives the workspace back to ${source}.`,
        untouchedHint: (source: string) =>
          `Nothing was stopped: the workspace still runs on ${source}. Retry once the problem is fixed, or dismiss this.`,
        cancelled: (name: string, source: string) => `Transfer cancelled; ${name} runs on ${source} again.`,
        movedBanner: (machine: string) =>
          `This workspace runs on ${machine}: it is piloted from here (chats, terminals, scripts, files), its ports lead there.`,
        unreachable: (machine: string) => `${machine} does not answer: the workspace is back once it does.`,
        returned: (machine: string) => `Back from ${machine}.`,
        kept: (path: string) => `The copy that was here is kept at ${path}; remove it once compared.`,
        discardPending: (machine: string, reason: string) =>
          `Its copy on ${machine} is not deleted yet (${reason}); ${machine} tries again every 10 minutes.`,
        dismiss: "Dismiss",
        details: "Details",
        stage: (stage: TransferStage, machine: string, working: number) =>
          ({
            preparing_worker: "Preparing an on-demand worker",
            preparing: `Checking ${machine}`,
            waiting_for_turns: `Waiting for the end of turns (${plural(working, "agent", "agents")})`,
            queued: "Queued: another transfer ends first",
            stopping: "Stopping and capturing the workspace",
            sending: `Sending it to ${machine}`,
            restoring: `Restoring it on ${machine}`,
            activating: `Starting it on ${machine}`,
          })[stage],
        interrupting: "Interrupting the agents…",
        waitingHint: "Nothing stops before their turn ends.",
        interrupt: "Interrupt and transfer",
        interruptHint: "Interrupts the agents in the middle of their turn, then transfers",
        select: "Select",
        deselect: "Deselect",
        picked: (count: number) => `${count} selected`,
        clearPicked: "Clear the selection (Esc)",
        transferMany: (count: number) => `Transfer ${count} workspaces…`,
        batchTitle: (count: number) => `Transfer ${count} workspaces`,
        batchExplain:
          "Each goes on its own: its agents finish their turn, then it stops here and starts again on the other machine with its conversations. One failing does not undo the others.",
        agentsWorking: "agents working: waits for the end of their turn",
        agentWaits: "an agent waits for your answer: waits for the end of its turn",
        alreadyOn: (machine: string) => `already on ${machine}`,
        startMany: (count: number, machine: string) => `Transfer ${count} to ${machine}`,
        batchRunning: (machine: string) => `Transfers to ${machine}`,
        summary: (counts: BatchCounts) =>
          [
            counts.moved && `${counts.moved} moved`,
            counts.running && `${counts.running} running`,
            counts.failed && `${counts.failed} failed`,
            counts.cancelled && `${counts.cancelled} cancelled`,
          ]
            .filter(Boolean)
            .join(" · "),
        movedRow: (machine: string) => `Moved to ${machine}`,
        cancelledRow: "Cancelled: still runs here",
        retry: "Retry",
        close: "Close",
        closeHint: "Closing does not stop them: the Mac's daemon drives them.",
        batchPill: (done: number, count: number) => `Transfers ${done}/${count}`,
        batchPillTitle: "Open the transfers under way",
        backup: (view: BackupView, time: (iso: string) => string) => {
          const at = view.at ? time(view.at) : null;
          const live = view.live ? " (taken during a turn)" : "";
          switch (view.health) {
            case "pending":
              return "First encrypted backup on its way.";
            case "current":
              return `Backed up, encrypted, at ${at}${live}${view.checked ? `; unchanged at ${time(view.checked)}` : ""}.`;
            case "stale":
              return at
                ? `Last backup at ${at}${live}: none succeeded for 15 minutes.`
                : "No backup succeeded for 15 minutes.";
            case "failed":
              return at
                ? `The last backup failed; the one before, at ${at}${live}, is kept.`
                : "The backup failed; none is kept yet.";
          }
        },
        backupLeftOut: (files: string[]) => `Not backed up (in no repo): ${files.join(", ")}`,
        backupIncomplete: (chats: string[]) =>
          `${chats.length === 1 ? "A conversation could not be backed up" : `${chats.length} conversations could not be backed up`}; tried again at the next change, or within the hour: ${chats.join("; ")}`,
      };
}

export type TransferCopy = ReturnType<typeof transferCopy>;

function plural(count: number, one: string, many: string) {
  return `${count} ${count === 1 ? one : many}`;
}

/** What a running transfer is doing, as said to the user; its last step when the daemon predates
 * stages. */
export function stageLabel(transfer: TransferInfo, copy: TransferCopy): string | null {
  if (transfer.phase !== "running") return null;
  if (!transfer.stage) return currentStep(transfer);
  if (transfer.stage === "waiting_for_turns" && transfer.interrupt) return copy.interrupting;
  return copy.stage(transfer.stage, transfer.destination, transfer.working_agents);
}

/** Whether a running transfer has not stopped its source yet: cancelling it leaves the workspace
 * running there, nothing of it stopped (agents it interrupted stay interrupted). */
export function cancellable(transfer: TransferInfo): boolean {
  return (
    transfer.phase === "running" &&
    (transfer.stage === "preparing_worker" ||
      transfer.stage === "preparing" ||
      transfer.stage === "waiting_for_turns" ||
      transfer.stage === "queued")
  );
}

/** Whether "Interrupt and transfer" applies: it waits for agents it was not asked to interrupt. */
export function interruptible(transfer: TransferInfo): boolean {
  return transfer.phase === "running" && transfer.stage === "waiting_for_turns" && !transfer.interrupt;
}

/** The same ends asked again, to resume or cancel it, keeping its choice about an edited local
 * copy; null when an end is a socket only the CLI names. */
export function transferEnds(transfer: TransferInfo): CloudTransferStartParams | null {
  if (!transfer.from || !transfer.to) return null;
  return {
    workspace: transfer.workspace_id,
    source: transfer.from,
    destination: transfer.to,
    keep_local_copy: transfer.keep_local_copy,
    ...(transfer.interrupt && { interrupt: true }),
    ...(transfer.archive && { archive: true }),
  };
}

/** One workspace of a batch: its transfer once started, or why it did not start. */
export type BatchItem = {
  workspace: string;
  name: string;
  /** How it was asked, to ask it again when it was refused before it started. */
  machine: string;
  destination?: TransferEndpoint;
  source?: TransferEndpoint;
  keep_local_copy?: boolean;
  interrupt: boolean;
  transfer: TransferInfo | null;
  error: string | null;
};

export type BatchCounts = { moved: number; running: number; failed: number; cancelled: number };

/** Where each workspace of a batch is: one the daemon has not answered about yet counts as running. */
export function batchCounts(items: BatchItem[]): BatchCounts {
  const counts: BatchCounts = { moved: 0, running: 0, failed: 0, cancelled: 0 };
  for (const item of items) {
    // Refused before it started, it failed; an action refused since leaves its transfer's phase.
    const phase = item.transfer?.phase ?? (item.error ? "failed" : "running");
    if (phase === "running") counts.running++;
    else if (phase === "done") counts.moved++;
    else if (phase === "failed") counts.failed++;
    else counts.cancelled++;
  }
  return counts;
}

/** Why a selected workspace does not go with the batch, or null: it moved already, or is not ready. */
export function batchUnavailable(workspace: WorkspaceInfo, copy: TransferCopy, french: boolean): string | null {
  if (workspace.moved_to) return copy.alreadyOn(workspace.moved_to);
  return transferUnavailable(workspace, french);
}

/** What a selected workspace's agents make the transfer do, or null when none works. */
export function batchNote(workspace: WorkspaceInfo, copy: TransferCopy): string | null {
  if (workspace.activity === "working") return copy.agentsWorking;
  if (workspace.activity === "needs_input") return copy.agentWaits;
  return null;
}

/** Why the workspace cannot be transferred away now, or null: only a ready workspace that runs here. */
export function transferUnavailable(
  workspace: Pick<WorkspaceInfo, "state" | "moved_to">,
  french: boolean,
): string | null {
  if (workspace.moved_to) return null;
  if (workspace.state !== "ready")
    return french ? "Seul un workspace prêt peut être transféré" : "Only a ready workspace can be transferred";
  return null;
}

/** A machine a workspace can go to: declared in the settings, or a GCP worker. */
export type TransferTarget = TransferMachine & { worker?: WorkerInfo };

/** Declared machines, then ready workers; an empty controller uses Chauffeur's managed one. */
export function transferTargets(settings: TransferSettings | null, workers: WorkerInfo[] = []): TransferTarget[] {
  if (!settings) return [];
  const machines: TransferTarget[] = settings.machines.filter((m) => m.name.trim() && m.socket.trim());
  return machines.concat(
    workers.filter((w) => w.state === "ready").map((worker) => ({ name: worker.name, socket: worker.socket, worker })),
  );
}

/** The transfer to show for a workspace: running, or ended with something to say and not dismissed. A
 * move that went through says nothing more than the workspace's own "runs on" notice. */
export function visibleTransfer(transfer: TransferInfo | null, dismissed: string | null): TransferInfo | null {
  if (!transfer) return null;
  if (transfer.phase === "running") return transfer;
  if (transfer.phase === "done" && !transfer.returned && !transfer.kept_copy && transfer.preparation?.status !== "needs_attention") return null;
  return transfer.id === dismissed ? null : transfer;
}

/** Why the copy a return left on the machine it came from is not deleted yet, or null: none was
 * left, or that machine deleted it since (the daemon asks it again). */
export function pendingDiscard(transfer: TransferInfo): string | null {
  return transfer.phase === "done" && transfer.returned ? (transfer.discard_pending ?? null) : null;
}

/** The step a running transfer is at: its last top-level line (warnings and outputs are indented). */
export function currentStep(transfer: TransferInfo): string | null {
  for (let i = transfer.log.length - 1; i >= 0; i--) {
    const line = transfer.log[i]!;
    if (!line.startsWith(" ")) return line;
  }
  return null;
}

/** The newer of what the live events and the daemon's status said: a later transfer, or for the
 * same one, an ended state over a running one, then the longer log. */
export function latestTransfer(live: TransferInfo | null, read: TransferInfo | null): TransferInfo | null {
  if (!live || !read) return live ?? read;
  if (live.id !== read.id) return live.started_at >= read.started_at ? live : read;
  if ((live.phase === "running") !== (read.phase === "running")) return live.phase === "running" ? read : live;
  if (live.revision != null && read.revision != null && live.revision !== read.revision) return live.revision > read.revision ? live : read;
  return live.log.length >= read.log.length ? live : read;
}

/** The same transfer asked again, to resume or cancel a failed one, keeping its choice about an
 * edited local copy; null when it did not fail or an end is a socket only the CLI names. */
export function retryParams(transfer: TransferInfo): CloudTransferStartParams | null {
  return transfer.phase === "failed" ? transferEnds(transfer) : null;
}

/** A transfer that failed before the controller prepared it: nothing of the workspace stopped, so
 * it is retried rather than resumed, and dismissing it ends it on the daemon too. */
export function untouchedFailure(transfer: TransferInfo): boolean {
  return transfer.phase === "failed" && !!transfer.untouched;
}

/** The workspaces a reminder names that still show idle: one active again meanwhile, or forgotten
 * (a reminder replayed after a reconnection), is left out. */
/** A return refused because the copy it would replace was edited since the workspace left: the
 * same transfer asked again keeping that copy beside the returning one; null otherwise. */
export function keepCopyParams(transfer: TransferInfo): CloudTransferStartParams | null {
  const retry = retryParams(transfer);
  return retry && transfer.local_copy_changed && !retry.keep_local_copy ? { ...retry, keep_local_copy: true } : null;
}

/** Why a transfer failed: for want of an agent signed in on its worker, what to do about it. */
export function failedText(transfer: TransferInfo, copy: TransferCopy): string {
  return transfer.needs_login ? copy.needsLogin(transfer.needs_login.worker) : (transfer.error ?? copy.failed);
}

/** What the user is told once a transfer this computer drove ended: moved, back, waiting for an agent
 * signed in on its worker, or failed with why; null for a cancelled one, which the user asked for. */
export function endedNotification(
  transfer: TransferInfo,
  name: string,
  copy: TransferCopy,
): { text: string; failed: boolean } | null {
  switch (transfer.phase) {
    case "done":
      if (transfer.preparation?.status === "needs_attention") return { text: `${name} · ${transfer.destination}: ${transfer.preparation.message}`, failed: true };
      return {
        text: !transfer.returned
          ? copy.movedNotification(name, transfer.destination)
          : transfer.archive
            ? copy.archivedNotification(name)
            : copy.backNotification(name),
        failed: false,
      };
    case "failed":
      return {
        text: transfer.needs_login
          ? copy.needsLoginNotification(name, transfer.needs_login.worker)
          : copy.failedNotification(name, transfer.error ?? "no reason given"),
        failed: true,
      };
    default:
      return null;
  }
}

export function stillIdle(ids: string[], workspaces: Record<string, WorkspaceInfo>): WorkspaceInfo[] {
  return ids.flatMap((id) => {
    const workspace = workspaces[id];
    return workspace?.moved_to && workspace.idle_since ? [workspace] : [];
  });
}

/** Past this without a success, backups are stale: three missed in a row, as the daemon counts. */
const STALE_MS = 15 * 60 * 1000;

/** What a moved workspace's backups show: the daemon's health, turned stale on this device's clock
 * when no news came since (the Mac publishes it only when something changes), whether a backup was
 * kept or a first one is still awaited since the setup, the newest kept backup, and the last
 * attempt's error. */
export type BackupView = {
  health: BackupHealth;
  at: string | null;
  live: boolean;
  error: string | null;
  /** When the newest backup was last found unchanged, if well after it was taken: it is current. */
  checked: string | null;
  /** Conversations the newest backup could not carry, with why. */
  incomplete: string[];
};

export function backupView(backup: BackupStatus, now: number): BackupView {
  const checked = backup.checked_at ? Date.parse(backup.checked_at) : Number.NaN;
  // A first backup awaited too long, as when the worker went silent right after its setup.
  const awaited = backup.configured_at ? now - Date.parse(backup.configured_at) > STALE_MS : false;
  const late = (backup.health === "current" && !(now - checked <= STALE_MS)) || (backup.health === "pending" && awaited);
  const health = late ? "stale" : backup.health;
  const newest = backup.backups[0];
  const at = newest?.taken_at ?? backup.last_success_at;
  const later = at !== null && backup.checked_at !== null && checked - Date.parse(at) > 60_000;
  return {
    health,
    at,
    live: newest?.live ?? false,
    error: backup.error,
    checked: later ? backup.checked_at : null,
    incomplete: backup.incomplete,
  };
}
