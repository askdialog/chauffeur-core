// Under a workspace's header: its transfer while the daemon drives it, how it ended, and, once
// the workspace runs on another machine, where, with the way to bring it back.

import type { TransferInfo, WorkspaceInfo } from "@chauffeur/protocol";
import { AlarmClock, ArrowRightLeft, CheckCircle2, ShieldAlert, ShieldCheck, Undo2, XCircle } from "lucide-react";
import { useEffect, useState } from "react";
import { clockTime } from "@/lib/format";
import { errorMessage } from "@/lib/rpc";
import {
  backupView,
  cancellable,
  failedText,
  interruptible,
  latestTransfer,
  pendingDiscard,
  keepCopyParams,
  retryParams,
  stageLabel,
  transferCopy,
  transferEnds,
  untouchedFailure,
  visibleTransfer,
} from "@/lib/transfer";
import { loginResumes, workerLoginCopy } from "@/lib/workerLogin";
import { client, openSettings, setTab, toast } from "@/state/app";
import { useWorkspaceSelector, type WorkspaceSession } from "@/state/session";
import { useWorkerLogins } from "@/state/workerLogins";
import { BackupRecoveryDialog } from "./BackupRecoveryDialog";
import { TransferDialog } from "./TransferDialog";
import { Button, cx, Spinner } from "./ui";
import { WorkerSignIn } from "./WorkerSignIn";

export function TransferBanner({ workspace, session }: { workspace: WorkspaceInfo; session: WorkspaceSession }) {
  const french = navigator.language.startsWith("fr"),
    copy = transferCopy(french);
  const live = useWorkspaceSelector(session, (s) => s.transfer);
  const relay = useWorkspaceSelector(session, (s) => s.relay);
  const [read, setRead] = useState<TransferInfo | null>(null);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [bringing, setBringing] = useState(false);
  const [recovering, setRecovering] = useState(false);
  const [snoozing, setSnoozing] = useState(false);
  // The daemon publishes the workspace without `idle_since` once it is snoozed.
  const remindLater = () => {
    setSnoozing(true);
    client
      .call("workspace.remind_later", { workspaces: [workspace.id] })
      .catch((error: unknown) => toast(`Could not snooze the reminder: ${errorMessage(error)}`, "error"))
      .finally(() => setSnoozing(false));
  };
  // What the daemon kept, for a transfer that started before the view opened; events follow.
  useEffect(() => {
    let disposed = false;
    client
      .call("cloud.transfer.status", { workspace: workspace.id })
      .then((status) => {
        if (!disposed) setRead(status.transfer);
      })
      .catch(() => {});
    return () => {
      disposed = true;
    };
  }, [workspace.id, workspace.moved_to]);
  const latest = latestTransfer(live, read);
  // Events are not replayed: while it runs, the daemon is asked again, so a missed end shows.
  const running = latest?.phase === "running";
  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => {
      client
        .call("cloud.transfer.status", { workspace: workspace.id })
        .then((status) => setRead(status.transfer))
        .catch(() => {});
    }, 3000);
    return () => clearInterval(timer);
  }, [running, workspace.id]);
  const transfer = visibleTransfer(latest, dismissed);
  const moved = workspace.moved_to;
  // The Mac publishes the backups' state when it changes: a silent worker turns stale on this clock.
  const [now, setNow] = useState(Date.now);
  const watching = !!moved && !!workspace.backup;
  useEffect(() => {
    if (!watching) return;
    const timer = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(timer);
  }, [watching]);
  const backup = workspace.backup ? backupView(workspace.backup, now) : null;
  if (!transfer && !moved) return null;
  return (
    <div className="mt-2 space-y-2" data-testid="transfer-banner">
      {transfer && (
        <TransferProgress
          transfer={transfer}
          copy={copy}
          onDismiss={setDismissed}
          onChanged={setRead}
        />
      )}
      {moved && transfer?.phase !== "running" && (
        <div className="flex items-center gap-2 rounded-md border border-accent/30 bg-accent/5 px-3 py-1.5 text-[12px] text-fg">
          <ArrowRightLeft size={13} className="shrink-0 text-accent" />
          <span className="min-w-0 flex-1">
            {copy.movedBanner(moved)}
            {relay?.connected === false && (
              <span className="block text-attention" data-testid="transfer-unreachable" title={relay.error ?? undefined}>
                {copy.unreachable(moved)}
              </span>
            )}
            {backup && (
              <span
                className={cx(
                  "mt-0.5 flex items-center gap-1",
                  (backup.health === "current" || backup.health === "pending") && !backup.incomplete.length
                    ? "text-muted"
                    : "text-attention",
                )}
                data-testid="transfer-backup"
                title={[
                  backup.error,
                  backup.incomplete.length ? copy.backupIncomplete(backup.incomplete) : null,
                  workspace.backup?.left_out.length ? copy.backupLeftOut(workspace.backup.left_out) : null,
                ]
                  .filter(Boolean)
                  .join("\n") || undefined}
              >
                {(backup.health === "current" || backup.health === "pending") && !backup.incomplete.length ? (
                  <ShieldCheck size={12} className="shrink-0" />
                ) : (
                  <ShieldAlert size={12} className="shrink-0" />
                )}
                {copy.backup(backup, clockTime)}
                {backup.error && <span className="truncate">{backup.error}</span>}
              </span>
            )}
            {!!workspace.backup?.left_out.length && <span className="mt-1 block text-[12px] text-attention">{copy.backupLeftOut(workspace.backup.left_out)}</span>}
            {workspace.idle_since && (
              <span className="mt-0.5 flex items-center gap-1 text-muted" data-testid="transfer-idle">
                <AlarmClock size={12} className="shrink-0 text-attention" />
                {copy.idleBanner(clockTime(workspace.idle_since))}
              </span>
            )}
          </span>
          {workspace.idle_since && (
            <Button size="sm" onClick={remindLater} disabled={snoozing} data-testid="transfer-remind-later">
              {copy.remindLater}
            </Button>
          )}
          {workspace.backup && <Button size="sm" onClick={() => setRecovering(true)} data-testid="backup-recover">
            {french ? "Restaurer une sauvegarde…" : "Recover a backup…"}
          </Button>}
          <Button size="sm" onClick={() => setBringing(true)} data-testid="transfer-bring-back">
            <Undo2 size={12} /> {copy.bringBack}
          </Button>
        </div>
      )}
      {recovering && <BackupRecoveryDialog workspace={workspace} onClose={() => setRecovering(false)} />}
      {bringing && <TransferDialog workspaces={[workspace]} onClose={() => setBringing(false)} />}
    </div>
  );
}

function TransferProgress({
  transfer,
  copy,
  onDismiss,
  onChanged,
}: {
  transfer: TransferInfo;
  copy: ReturnType<typeof transferCopy>;
  /** Hides the transfer of that id. */
  onDismiss: (id: string) => void;
  /** The transfer the daemon answered with, resumed (running) or cancelled. */
  onChanged: (transfer: TransferInfo) => void;
}) {
  const running = transfer.phase === "running";
  const failed = transfer.phase === "failed";
  const preparation = transfer.preparation;
  const attention = preparation?.status === "needs_attention";
  const french = navigator.language.startsWith("fr");
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running]);
  const elapsed = Math.max(0, Math.floor((now - Date.parse(transfer.progress?.stage_started_at ?? transfer.started_at)) / 1000));
  const step = stageLabel(transfer, copy);
  const retry = retryParams(transfer);
  // Nothing stopped: Retry and Dismiss, which ends it on the daemon too.
  const untouched = untouchedFailure(transfer);
  const keepCopy = keepCopyParams(transfer);
  // Not stopped yet: cancelling leaves the workspace as it was.
  const stoppable = cancellable(transfer) ? transferEnds(transfer) : null;
  const discard = pendingDiscard(transfer);
  const [acting, setActing] = useState(false);
  const [refused, setRefused] = useState<string | null>(null);
  const run = (call: Promise<TransferInfo>) => {
    setActing(true);
    setRefused(null);
    call
      .then(onChanged)
      .catch((error: unknown) => setRefused(errorMessage(error)))
      .finally(() => setActing(false));
  };
  const act = (method: "cloud.transfer.start" | "cloud.transfer.cancel") => {
    if (retry) run(client.call(method, retry));
  };
  const dismissUntouched = () => {
    if (!retry) return;
    setActing(true);
    setRefused(null);
    client
      .call("cloud.transfer.cancel", retry)
      .then((cancelled) => {
        onChanged(cancelled);
        onDismiss(cancelled.id);
      })
      .catch((error: unknown) => setRefused(errorMessage(error)))
      .finally(() => setActing(false));
  };
  return (
    <div
      className={cx(
        "rounded-md border px-3 py-1.5 text-[12px]",
        failed ? "border-danger/30 bg-danger/10 text-danger" : "border-line bg-raised/60 text-fg",
      )}
    >
      <div className="flex items-start gap-2">
        {running ? (
          <Spinner size={12} />
        ) : failed || attention ? (
          <XCircle size={13} className="mt-0.5 shrink-0" />
        ) : (
          <CheckCircle2 size={13} className="mt-0.5 shrink-0 text-success" />
        )}
        <div className="min-w-0 flex-1 selectable">
          {running && (
            <p data-testid="transfer-stage" data-stage={transfer.stage ?? undefined}>
              {transfer.archive ? copy.archivingBack(transfer.source) : copy.running(transfer.destination)}
              {step && <span className="text-muted"> · {step}</span>}
            </p>
          )}
          {running && <p className="text-muted" aria-live="off">
            {Math.floor(elapsed / 60)}:{String(elapsed % 60).padStart(2, "0")}
            {transfer.stage === "sending" && transfer.progress && transfer.progress.total_bytes > 0 && (
              <> · {(transfer.progress.confirmed_bytes / 1048576).toFixed(1)} / {(transfer.progress.total_bytes / 1048576).toFixed(1)} MiB {french ? "reçus" : "received"}</>
            )}
          </p>}
          {attention && <div role="alert" className="space-y-1 text-attention" data-testid="transfer-preparation-failed">
            <p>{french ? "Workspace transféré · préparation à vérifier" : "Workspace transferred · preparation needs attention"}</p>
            <p>{preparation.message}</p>
            <Button size="sm" onClick={() => setTab(transfer.workspace_id, "processes")}>{french ? "Voir les processus et les logs" : "Open processes and logs"}</Button>
          </div>}
          {interruptible(transfer) && <p className="text-muted">{copy.waitingHint}</p>}
          {failed && <p className="whitespace-pre-wrap">{failedText(transfer, copy)}</p>}
          {failed && transfer.needs_login && <TransferSignIn transfer={transfer} onResumed={onChanged} />}
          {keepCopy && <p className="text-muted">{copy.keepLocalCopyHint(transfer.destination)}</p>}
          {failed && retry && (
            <p className="text-muted">
              {untouched ? copy.untouchedHint(transfer.source) : copy.failedHint(transfer.source)}
            </p>
          )}
          {refused && <p className="whitespace-pre-wrap">{refused}</p>}
          {transfer.phase === "cancelled" && <p>{copy.cancelled(transfer.slug, transfer.source)}</p>}
          {transfer.phase === "done" && transfer.returned && (
            <p>{transfer.archive ? copy.archivedBack(transfer.source) : copy.returned(transfer.source)}</p>
          )}
          {discard && (
            <p className="text-attention" data-testid="transfer-discard-pending">
              {copy.discardPending(transfer.source, discard)}
            </p>
          )}
          {transfer.phase === "done" && transfer.kept_copy && (
            <p className="text-attention">{copy.kept(transfer.kept_copy)}</p>
          )}
        </div>
        {interruptible(transfer) && (
          <Button
            size="sm"
            disabled={acting}
            title={copy.interruptHint}
            onClick={() => run(client.call("cloud.transfer.interrupt", { workspace: transfer.workspace_id }))}
            data-testid="transfer-interrupt"
          >
            {copy.interrupt}
          </Button>
        )}
        {stoppable && (
          <Button
            size="sm"
            variant="ghost"
            disabled={acting}
            onClick={() => run(client.call("cloud.transfer.cancel", stoppable))}
            data-testid="transfer-cancel-waiting"
          >
            {copy.cancel}
          </Button>
        )}
        {keepCopy && (
          <Button
            size="sm"
            disabled={acting}
            onClick={() => run(client.call("cloud.transfer.start", keepCopy))}
            data-testid="transfer-keep-local-copy"
          >
            {copy.keepLocalCopy}
          </Button>
        )}
        {failed && retry && (
          <>
            <Button size="sm" disabled={acting} onClick={() => act("cloud.transfer.start")} data-testid="transfer-resume">
              {untouched ? copy.retry : copy.resume}
            </Button>
            {untouched ? (
              <Button
                size="sm"
                variant="ghost"
                disabled={acting}
                onClick={dismissUntouched}
                data-testid="transfer-dismiss-untouched"
              >
                {copy.dismiss}
              </Button>
            ) : (
              <Button
                size="sm"
                variant="ghost"
                disabled={acting}
                onClick={() => act("cloud.transfer.cancel")}
                data-testid="transfer-cancel"
              >
                {copy.cancelTransfer}
              </Button>
            )}
          </>
        )}
        {!running && !(failed && retry) && (
          <Button size="sm" variant="ghost" onClick={() => onDismiss(transfer.id)}>
            {copy.dismiss}
          </Button>
        )}
      </div>
      {transfer.log.length > 0 && (
        <details className="mt-1">
          <summary className="cursor-pointer text-[11px] text-faint">{copy.details}</summary>
          <pre className="mt-1 max-h-40 overflow-auto whitespace-pre-wrap font-mono text-[11px] text-muted selectable">
            {transfer.log.join("\n")}
          </pre>
        </details>
      )}
    </div>
  );
}

/** A transfer refused for want of an agent signed in on its worker: the way to Settings →
 * Transfers, where its agents sign in, and a sign-in per agent here. The Mac's daemon resumes it
 * once one is in; the new attempt goes to `onResumed`. */
export function TransferSignIn({ transfer, onResumed }: { transfer: TransferInfo; onResumed: (transfer: TransferInfo) => void }) {
  const french = navigator.language.startsWith("fr");
  const needs = transfer.needs_login;
  const logins = useWorkerLogins((s) => s.logins);
  const resuming = loginResumes(transfer, logins);
  useEffect(() => {
    if (!resuming) return;
    let tries = 0;
    const timer = setInterval(() => {
      if (++tries > 20) clearInterval(timer);
      client
        .call("cloud.transfer.status", { workspace: transfer.workspace_id })
        .then((status) => {
          if (status.transfer && status.transfer.id !== transfer.id) {
            clearInterval(timer);
            onResumed(status.transfer);
          }
        })
        .catch(() => {});
    }, 1000);
    return () => clearInterval(timer);
  }, [resuming, transfer.id, transfer.workspace_id, onResumed]);
  if (!needs) return null;
  return (
    <div className="mt-1.5 space-y-1.5 text-[12px]" data-testid="transfer-sign-in">
      <Button size="sm" variant="primary" onClick={() => openSettings(true, "transfer")} data-testid="transfer-open-settings">
        {transferCopy(french).openSettings}
      </Button>
      {needs.agents.map((agent) => (
        <WorkerSignIn key={agent} worker={needs.worker} agent={agent} french={french} />
      ))}
      <p className="text-muted">{workerLoginCopy(french).resumes}</p>
    </div>
  );
}
