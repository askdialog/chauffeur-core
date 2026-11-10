// A batch of transfers started together: each workspace's progress and outcome, with what can be
// done about it (interrupt its agents, cancel, resume, retry). Closing it stops nothing: the Mac's
// daemon drives each transfer, and the sidebar reopens the view while the batch is kept.

import type { CloudTransferStartParams, TransferInfo } from "@chauffeur/protocol";
import { CheckCircle2, RotateCcw, XCircle } from "lucide-react";
import { useState } from "react";
import { errorMessage } from "@/lib/rpc";
import {
  type BatchItem,
  batchCounts,
  cancellable,
  failedText,
  interruptible,
  keepCopyParams,
  retryParams,
  stageLabel,
  transferCopy,
  transferEnds,
  untouchedFailure,
} from "@/lib/transfer";
import { client, selectWorkspace } from "@/state/app";
import { batchTransferChanged, dismissBatch, openBatch, updateItem, useTransfers } from "@/state/transfers";
import { TransferSignIn } from "./TransferBanner";
import { Button, cx, Modal, Spinner } from "./ui";

export function TransferBatch() {
  const batch = useTransfers((s) => s.batch);
  const open = useTransfers((s) => s.batchOpen);
  if (!batch || !open) return null;
  const copy = transferCopy(navigator.language.startsWith("fr"));
  const counts = batchCounts(batch.items);
  return (
    <Modal
      open
      onClose={() => openBatch(false)}
      title={copy.batchRunning(batch.machine)}
      width={560}
      footer={
        <>
          <span className="mr-auto text-[11.5px] text-faint">{counts.running > 0 ? copy.closeHint : ""}</span>
          {counts.running === 0 && (
            <Button variant="ghost" onClick={dismissBatch}>
              {copy.dismiss}
            </Button>
          )}
          <Button variant="primary" onClick={() => openBatch(false)} data-testid="transfer-batch-close">
            {copy.close}
          </Button>
        </>
      }
    >
      <p className="mb-2 text-[12.5px] text-fg" aria-live="polite" data-testid="transfer-batch-summary">
        {copy.summary(counts)}
      </p>
      <ul className="space-y-1.5" data-testid="transfer-batch">
        {batch.items.map((item) => (
          <BatchRow key={item.workspace} item={item} copy={copy} />
        ))}
      </ul>
    </Modal>
  );
}

function BatchRow({ item, copy }: { item: BatchItem; copy: ReturnType<typeof transferCopy> }) {
  const machine = item.transfer?.destination ?? item.machine;
  const [acting, setActing] = useState(false);
  const transfer = item.transfer;
  const phase = transfer?.phase ?? (item.error ? "failed" : "running");
  const failed = phase === "failed";
  const act = (call: Promise<TransferInfo>) => {
    setActing(true);
    call
      .then(batchTransferChanged)
      .catch((error: unknown) => updateItem(item.workspace, { error: errorMessage(error) }))
      .finally(() => setActing(false));
  };
  const call = (method: "cloud.transfer.start" | "cloud.transfer.cancel", params: CloudTransferStartParams) =>
    act(client.call(method, params));
  // Refused before it started: asked again as it was.
  const again: CloudTransferStartParams = {
    workspace: item.workspace,
    source: item.source ?? { kind: "here" },
    destination: item.destination ?? { kind: "machine", name: item.machine },
    keep_local_copy: item.keep_local_copy ?? false,
    ...(item.interrupt && { interrupt: true }),
  };
  const ends = transfer ? transferEnds(transfer) : null;
  const retry = transfer ? retryParams(transfer) : again;
  const keepCopy = transfer ? keepCopyParams(transfer) : null;
  return (
    <li
      className={cx(
        "rounded-md border px-3 py-2 text-[12px]",
        failed ? "border-danger/30 bg-danger/10" : "border-line bg-raised/60",
      )}
      data-testid="transfer-batch-row"
      data-phase={phase}
    >
      <div className="flex items-start gap-2">
        <span className="mt-0.5 shrink-0">
          {phase === "running" ? (
            <Spinner size={12} />
          ) : phase === "done" ? (
            <CheckCircle2 size={13} className="text-success" />
          ) : phase === "cancelled" ? (
            <RotateCcw size={13} className="text-faint" />
          ) : (
            <XCircle size={13} className="text-danger" />
          )}
        </span>
        <div className="min-w-0 flex-1 selectable">
          <button type="button" className="truncate font-medium text-fg hover:underline" onClick={() => selectWorkspace(item.workspace)}>
            {item.name}
          </button>
          <p className={cx("whitespace-pre-wrap", failed ? "text-danger" : "text-muted")}>
            {phase === "running" && (transfer ? stageLabel(transfer, copy) : copy.stage("preparing", machine, 0))}
            {phase === "done" && (transfer?.returned ? copy.returned(transfer.source) : copy.movedRow(machine))}
            {phase === "cancelled" && copy.cancelledRow}
            {failed && (transfer ? failedText(transfer, copy) : (item.error ?? copy.failed))}
          </p>
          {transfer?.stage === "waiting_for_turns" && !transfer.interrupt && <p className="text-faint">{copy.waitingHint}</p>}
          {transfer && item.error && <p className="whitespace-pre-wrap text-danger">{item.error}</p>}
          {failed && transfer?.needs_login && <TransferSignIn transfer={transfer} onResumed={batchTransferChanged} />}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {transfer && interruptible(transfer) && (
            <Button
              size="sm"
              disabled={acting}
              title={copy.interruptHint}
              onClick={() => act(client.call("cloud.transfer.interrupt", { workspace: item.workspace }))}
              data-testid="transfer-interrupt"
            >
              {copy.interrupt}
            </Button>
          )}
          {transfer && ends && cancellable(transfer) && (
            <Button size="sm" variant="ghost" disabled={acting} onClick={() => call("cloud.transfer.cancel", ends)}>
              {copy.cancel}
            </Button>
          )}
          {keepCopy && (
            <Button size="sm" disabled={acting} onClick={() => call("cloud.transfer.start", keepCopy)} data-testid="transfer-keep-local-copy">
              {copy.keepLocalCopy}
            </Button>
          )}
          {failed && retry && (
            <Button size="sm" disabled={acting} onClick={() => call("cloud.transfer.start", retry)} data-testid="transfer-batch-retry">
              {transfer && !untouchedFailure(transfer) ? copy.resume : copy.retry}
            </Button>
          )}
          {failed && transfer && retry && (
            <Button size="sm" variant="ghost" disabled={acting} onClick={() => call("cloud.transfer.cancel", retry)}>
              {copy.cancelTransfer}
            </Button>
          )}
        </div>
      </div>
    </li>
  );
}
