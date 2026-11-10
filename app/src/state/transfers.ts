// The sidebar's multiple selection and the batch of transfers started from it. The Mac's daemon
// drives each transfer; the app follows the batch while it runs, so that its view, reopened from
// the sidebar, shows each workspace's outcome.

import type { TransferEndpoint, TransferInfo, WorkspaceInfo } from "@chauffeur/protocol";
import { create } from "zustand";
import { workspaceName } from "@/lib/format";
import { errorMessage } from "@/lib/rpc";
import { pickRange, togglePicked } from "@/lib/selection";
import { type BatchItem, batchCounts, latestTransfer } from "@/lib/transfer";
import { client, useApp } from "./app";

export type TransferBatch = {
  /** Where the latest workspaces added went. */
  machine: string;
  items: BatchItem[];
};

type TransfersState = {
  /** The workspaces picked in the sidebar, in the order picked. */
  picked: string[];
  /** Where a ⇧-click range starts: the last workspace picked. */
  anchor: string | null;
  /** The latest batch, until dismissed. */
  batch: TransferBatch | null;
  batchOpen: boolean;
};

export const useTransfers = create<TransfersState>(() => ({ picked: [], anchor: null, batch: null, batchOpen: false }));

const set = useTransfers.setState;
const get = useTransfers.getState;

/** ⌘-click (`range` false) or ⇧-click on a sidebar row; `order` is the rows shown, top to bottom. */
export function pick(id: string, range: boolean, order: string[]) {
  const { picked, anchor } = get();
  const open = useApp.getState().selectedId;
  set(
    range
      ? { picked: pickRange(picked, order, anchor ?? open, id), anchor: id }
      : { picked: togglePicked(picked, id, open), anchor: id },
  );
}

export function clearPicked() {
  if (get().picked.length) set({ picked: [], anchor: null });
}

/** Tracks outgoing transfers or returns without opening a modal; each refusal stays in progress. */
export async function startBatch(
  workspaces: WorkspaceInfo[],
  machine: string,
  interrupt: boolean,
  destination: TransferEndpoint = { kind: "machine", name: machine },
  source: TransferEndpoint = { kind: "here" },
  keepLocalCopy = false,
) {
  const items: BatchItem[] = workspaces.map((w) => ({
    workspace: w.id,
    name: workspaceName(w),
    machine,
    destination,
    source,
    keep_local_copy: keepLocalCopy,
    interrupt,
    transfer: null,
    error: null,
  }));
  // An earlier batch's transfers still running stay in view, above the new ones.
  const running = (get().batch?.items ?? []).filter(
    (item) => (!item.transfer && !item.error) || item.transfer?.phase === "running",
  );
  const kept = running.filter((item) => !items.some((i) => i.workspace === item.workspace));
  set({ batch: { machine, items: [...kept, ...items] }, batchOpen: false, picked: [], anchor: null });
  await Promise.all(
    workspaces.map((w) =>
      client
        .call("cloud.transfer.start", {
          workspace: w.id,
          source,
          destination,
          keep_local_copy: keepLocalCopy,
          ...(interrupt && { interrupt: true }),
        })
        .then(
          (transfer) => updateItem(w.id, { transfer, error: null }),
          (error: unknown) => updateItem(w.id, { error: errorMessage(error) }),
        ),
    ),
  );
  followBatch();
}

/** What the daemon answered about one workspace of the batch: a transfer started, resumed or
 * cancelled, or a refusal. */
export function updateItem(workspace: string, change: Partial<Pick<BatchItem, "transfer" | "error">>) {
  const batch = get().batch;
  if (!batch) return;
  set({
    batch: {
      ...batch,
      items: batch.items.map((item) => {
        if (item.workspace !== workspace) return item;
        // A stale answer never hides a newer state of the same transfer.
        const transfer = change.transfer === undefined ? item.transfer : latestTransfer(change.transfer, item.transfer);
        return { ...item, ...change, transfer };
      }),
    },
  });
}

let following: ReturnType<typeof setInterval> | null = null;

/** Reads each running transfer of the batch again every 2 s until none runs. */
export function followBatch() {
  if (following) return;
  const read = () => {
    const batch = get().batch;
    if (!batch || batchCounts(batch.items).running === 0) {
      if (following) clearInterval(following);
      following = null;
      return;
    }
    for (const item of batch.items) {
      if (item.transfer?.phase !== "running") continue;
      client
        .call("cloud.transfer.status", { workspace: item.workspace })
        .then((status) => {
          if (status.transfer?.workspace_id === item.workspace) updateItem(item.workspace, { transfer: status.transfer });
        })
        .catch(() => {});
    }
  };
  following = setInterval(read, 2000);
  read();
}

export function openBatch(open: boolean) {
  set({ batchOpen: open });
}

/** Forgets an ended batch: its workspaces' headers keep what each needs to say. */
export function dismissBatch() {
  set({ batch: null, batchOpen: false });
}

/** A transfer of the batch the user acted on (resumed, cancelled, interrupted): followed again. */
export function batchTransferChanged(transfer: TransferInfo) {
  updateItem(transfer.workspace_id, { transfer, error: null });
  followBatch();
}
