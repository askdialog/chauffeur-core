import { describe, expect, test } from "bun:test";
import type { BackupStatus, TransferInfo, WorkerInfo, WorkspaceInfo } from "@chauffeur/protocol";
import {
  backupView,
  batchCounts,
  batchNote,
  batchUnavailable,
  cancellable,
  currentStep,
  endedNotification,
  interruptible,
  keepCopyParams,
  latestTransfer,
  pendingDiscard,
  retryParams,
  untouchedFailure,
  stageLabel,
  stillIdle,
  transferCopy,
  transferEnds,
  transferTargets,
  transferUnavailable,
  visibleTransfer,
} from "./transfer";

const transfer = (change: Partial<TransferInfo> = {}): TransferInfo => ({
  id: "t1",
  workspace_id: "w",
  slug: "moving",
  source: "mac",
  destination: "vm",
  phase: "running",
  log: [],
  error: null,
  workspace_path: null,
  kept_copy: null,
  returned: false,
  keep_local_copy: false,
  started_at: "2026-10-26T10:00:00Z",
  finished_at: null,
  working_agents: 0,
  interrupt: false,
  archive: false,
  ...change,
});

describe("transfer", () => {
  test("the end of a transfer reads moved, back, waiting for a sign-in or failed with its reason, and nothing once cancelled", () => {
    const copy = transferCopy(false);
    expect(endedNotification(transfer({ phase: "done", destination: "cloud-1" }), "billing", copy)).toEqual({
      text: "billing is transferred: it now runs on cloud-1.",
      failed: false,
    });
    expect(endedNotification(transfer({ phase: "done", returned: true }), "billing", copy)?.text).toBe(
      "billing is back on this computer.",
    );
    // Brought back to be archived, it says so.
    expect(endedNotification(transfer({ phase: "done", returned: true, archive: true }), "billing", copy)?.text).toBe(
      "billing is back on this computer and is being archived.",
    );
    expect(endedNotification(transfer({ phase: "failed", error: "vm does not answer" }), "billing", copy)).toEqual({
      text: "Could not transfer billing: vm does not answer",
      failed: true,
    });
    expect(
      endedNotification(
        transfer({ phase: "failed", error: "w1 cannot take billing yet", needs_login: { worker: "w1", agents: ["claude", "codex"] } }),
        "billing",
        copy,
      ),
    ).toEqual({
      text: "To transfer billing, sign Claude Code or Codex in on w1 from Settings → Transfers. The transfer then resumes by itself.",
      failed: true,
    });
    expect(endedNotification(transfer({ phase: "cancelled" }), "billing", copy)).toBeNull();
    expect(endedNotification(transfer(), "billing", copy)).toBeNull();
  });

  test("a failed transfer between named ends can be resumed or cancelled, as it was asked", () => {
    const failed = transfer({ phase: "failed", from: { kind: "here" }, to: { kind: "machine", name: "vm" } });
    expect(retryParams(failed)).toEqual({
      workspace: failed.workspace_id,
      source: { kind: "here" },
      destination: { kind: "machine", name: "vm" },
      keep_local_copy: false,
    });
    // Running or done: nothing to retry. A socket is the CLI's to name.
    // A return asked to keep the edited copy here asks it again.
    expect(retryParams({ ...failed, keep_local_copy: true })?.keep_local_copy).toBe(true);
    // A return asked to archive the workspace once back, its agents interrupted, asks both again.
    const archiving = retryParams({ ...failed, interrupt: true, archive: true });
    expect(archiving?.archive).toBe(true);
    expect(archiving?.interrupt).toBe(true);
    expect(retryParams({ ...failed, phase: "running" })).toBeNull();
    expect(retryParams({ ...failed, to: undefined })).toBeNull();
  });

  test("a transfer that failed before anything stopped is retried, not resumed", () => {
    const failed = transfer({ phase: "failed", from: { kind: "here" }, to: { kind: "machine", name: "vm" } });
    expect(untouchedFailure({ ...failed, untouched: true })).toBe(true);
    expect(untouchedFailure(failed)).toBe(false);
    expect(untouchedFailure({ ...failed, phase: "cancelled", untouched: true })).toBe(false);
  });

  test("a return refused for an edited copy here is offered again keeping that copy, and only then", () => {
    const refused = transfer({
      phase: "failed",
      from: { kind: "machine", name: "vm" },
      to: { kind: "here" },
      local_copy_changed: true,
    });
    expect(keepCopyParams(refused)).toEqual({
      workspace: refused.workspace_id,
      source: { kind: "machine", name: "vm" },
      destination: { kind: "here" },
      keep_local_copy: true,
    });
    // Another failure, a copy already kept, or nothing to retry: no such offer.
    expect(keepCopyParams({ ...refused, local_copy_changed: undefined })).toBeNull();
    expect(keepCopyParams({ ...refused, keep_local_copy: true })).toBeNull();
    expect(keepCopyParams({ ...refused, phase: "running" })).toBeNull();
    expect(transferCopy(true).keepLocalCopyHint("mac")).toContain("mac");
  });

  test("a return whose copy on the other machine is not deleted yet says why until it is", () => {
    const back = transfer({ phase: "done", returned: true, source: "vm", destination: "mac" });
    expect(pendingDiscard(back)).toBeNull();
    const pending = { ...back, discard_pending: "could not remove the copy's folder" };
    expect(pendingDiscard(pending)).toBe("could not remove the copy's folder");
    expect(visibleTransfer(pending, null)).toBe(pending);
    expect(transferCopy(false).discardPending("vm", "busy")).toContain("vm");
    // Only once the return is done: a transfer still running reports its own steps.
    expect(pendingDiscard({ ...pending, phase: "running" })).toBeNull();
  });

  test("both languages have the same keys", () => {
    expect(Object.keys(transferCopy(true)).sort()).toEqual(Object.keys(transferCopy(false)).sort());
  });

  test("only a ready workspace that runs here can go; a moved one can come back", () => {
    for (const french of [true, false]) {
      expect(transferUnavailable({ state: "ready" }, french)).toBeNull();
      expect(transferUnavailable({ state: "creating" }, french)).not.toBeNull();
      expect(transferUnavailable({ state: "ready", moved_to: "vm" }, french)).toBeNull();
    }
  });

  test("machines need a name and socket; a default controller needs no configuration", () => {
    expect(transferTargets(null)).toEqual([]);
    expect(transferTargets({ controller: " ", machines: [{ name: "vm", socket: "/s" }] })).toEqual([{ name: "vm", socket: "/s" }]);
    expect(
      transferTargets({
        controller: "/c",
        machines: [
          { name: "vm", socket: "/s" },
          { name: "", socket: "/t" },
        ],
      }),
    ).toEqual([{ name: "vm", socket: "/s" }]);
  });

  test("a ready worker is a machine too, after the declared ones", () => {
    const worker = (name: string, state: WorkerInfo["state"]): WorkerInfo => ({
      name,
      instance: `chauffeur-worker-${name}`,
      project: "p",
      zone: "z",
      machine_type: "e2-standard-4",
      hourly_usd: 0.17,
      state,
      connected: true,
      socket: `/w/${name}.sock`,
      ssh_host: null,
      ssh_issue: null,
      log: [],
      error: null,
      created_at: "2026-10-26T10:00:00Z",
    });
    const ready = worker("w1", "ready");
    const settings = { controller: "/c", machines: [{ name: "vm", socket: "/s" }] };
    expect(transferTargets(settings, [ready, worker("w2", "installing")])).toEqual([
      { name: "vm", socket: "/s" },
      { name: "w1", socket: "/w/w1.sock", worker: ready },
    ]);
    expect(transferTargets({ controller: "", machines: [] }, [ready])).toEqual([
      { name: "w1", socket: "/w/w1.sock", worker: ready },
    ]);
  });

  test("a running transfer always shows; an ended one until dismissed", () => {
    expect(visibleTransfer(null, null)).toBeNull();
    expect(visibleTransfer(transfer(), "t1")).not.toBeNull();
    const back = transfer({ phase: "done", returned: true });
    expect(visibleTransfer(back, null)).toBe(back);
    expect(visibleTransfer(back, "t1")).toBeNull();
    expect(visibleTransfer(transfer({ phase: "done" }), null)).toBeNull();
    expect(visibleTransfer(transfer({ phase: "failed", error: "no" }), null)).not.toBeNull();
  });

  test("the current step skips indented warnings and outputs", () => {
    expect(currentStep(transfer())).toBeNull();
    expect(currentStep(transfer({ log: ["checking the destination", "  gh is not logged in"] }))).toBe(
      "checking the destination",
    );
  });

  test("the ended state of a transfer wins over a stale running event, a later transfer over both", () => {
    const running = transfer({ log: ["checking the destination"] });
    const ended = transfer({ phase: "done", log: ["checking the destination", "restoring it on the destination"] });
    expect(latestTransfer(running, ended)).toBe(ended);
    expect(latestTransfer(ended, running)).toBe(ended);
    expect(latestTransfer(null, running)).toBe(running);
    const next = transfer({ id: "t2", started_at: "2026-10-26T11:00:00Z" });
    expect(latestTransfer(next, ended)).toBe(next);
  });

  test("a running transfer says its stage, the agents it waits for, and that it interrupts them", () => {
    const copy = transferCopy(false);
    const waiting = transfer({ stage: "waiting_for_turns", working_agents: 2 });
    expect(stageLabel(waiting, copy)).toBe("Waiting for the end of turns (2 agents)");
    expect(stageLabel({ ...waiting, working_agents: 1 }, transferCopy(true))).toBe("En attente de la fin des tours (1 agent)");
    expect(stageLabel({ ...waiting, interrupt: true }, copy)).toBe(copy.interrupting);
    expect(stageLabel(transfer({ stage: "restoring" }), copy)).toBe("Restoring it on vm");
    // A daemon that predates stages: its last step.
    expect(stageLabel(transfer({ log: ["checking the destination"] }), copy)).toBe("checking the destination");
    expect(stageLabel(transfer({ phase: "done", stage: "activating" }), copy)).toBeNull();
  });

  test("until it stops the source, a transfer can be cancelled; while it waits, its agents interrupted", () => {
    for (const stage of ["preparing", "waiting_for_turns", "queued"] as const) expect(cancellable(transfer({ stage }))).toBe(true);
    for (const stage of ["stopping", "sending", "restoring", "activating"] as const) expect(cancellable(transfer({ stage }))).toBe(false);
    expect(cancellable(transfer({ phase: "failed", stage: "queued" }))).toBe(false);
    expect(interruptible(transfer({ stage: "waiting_for_turns" }))).toBe(true);
    expect(interruptible(transfer({ stage: "waiting_for_turns", interrupt: true }))).toBe(false);
    expect(interruptible(transfer({ stage: "queued" }))).toBe(false);
    // Cancelled with the ends it was started with, an app naming machines only.
    const ends = { from: { kind: "here" }, to: { kind: "machine", name: "vm" } } as const;
    expect(transferEnds(transfer({ ...ends, interrupt: true }))?.interrupt).toBe(true);
    expect(transferEnds(transfer())).toBeNull();
  });

  test("a batch counts each workspace once: refused, running, moved, failed or cancelled", () => {
    const item = (change: Partial<TransferInfo> | null, error: string | null = null) => ({
      workspace: "w",
      name: "w",
      machine: "vm",
      interrupt: false,
      transfer: change ? transfer(change) : null,
      error,
    });
    expect(
      batchCounts([
        item(null),
        item(null, "already being transferred"),
        item({}),
        item({ phase: "done" }),
        item({ phase: "failed" }),
        item({ phase: "cancelled" }),
        // An action refused since leaves its transfer as it was.
        item({ phase: "done" }, "could not cancel"),
      ]),
    ).toEqual({ moved: 2, running: 2, failed: 2, cancelled: 1 });
    expect(transferCopy(false).summary({ moved: 1, running: 0, failed: 1, cancelled: 0 })).toBe("1 moved · 1 failed");
  });

  test("a selected workspace that moved or is not ready stays out of the batch; one whose agents work waits", () => {
    const copy = transferCopy(false);
    const workspace = (change: Partial<WorkspaceInfo>) => ({ state: "ready", activity: "idle", ...change }) as WorkspaceInfo;
    expect(batchUnavailable(workspace({}), copy, false)).toBeNull();
    expect(batchUnavailable(workspace({ moved_to: "vm" }), copy, false)).toBe("already on vm");
    expect(batchUnavailable(workspace({ state: "creating" }), copy, false)).not.toBeNull();
    expect(batchNote(workspace({}), copy)).toBeNull();
    expect(batchNote(workspace({ activity: "working" }), copy)).toBe(copy.agentsWorking);
    expect(batchNote(workspace({ activity: "needs_input" }), copy)).toBe(copy.agentWaits);
  });
});

describe("idle reminders", () => {
  const workspace = (id: string, change: Partial<WorkspaceInfo> = {}) =>
    ({ id, slug: id, moved_to: "vm", idle_since: "2026-10-26T10:00:00.000Z", ...change }) as WorkspaceInfo;

  test("a reminder names only the workspaces that still show idle", () => {
    const workspaces = {
      a: workspace("a"),
      b: workspace("b", { idle_since: undefined }),
      c: workspace("c", { moved_to: undefined }),
    };
    expect(stillIdle(["a", "b", "c", "gone"], workspaces).map((w) => w.id)).toEqual(["a"]);
  });

  test("the notification names every workspace of the reminder, in French and in English", () => {
    expect(transferCopy(false).idleNotification(["alpha", "beta"])).toStartWith("alpha, beta: nothing has happened for an hour");
    expect(transferCopy(true).idleNotification(["alpha"])).toStartWith("alpha : rien ne s'est passé depuis une heure");
  });
});

describe("a moved workspace's backups", () => {
  const backup = (change: Partial<BackupStatus> = {}): BackupStatus => ({
    health: "current",
    last_success_at: "2026-10-27T10:00:00Z",
    checked_at: "2026-10-27T10:05:00Z",
    last_attempt_at: "2026-10-27T10:05:00Z",
    configured_at: "2026-10-27T09:59:00Z",
    error: null,
    backups: [
      {
        id: "b2",
        object: "gs://bucket/vm/w/b2.bin",
        taken_at: "2026-10-27T10:00:00Z",
        live: true,
        digest: "d",
        bytes: 10,
        manifest_digest: "m",
        worker_peer_id: "p",
        epoch: 2,
      },
    ],
    left_out: [],
    incomplete: [],
    ...change,
  });
  const at = (minutes: number) => Date.parse("2026-10-27T10:05:00Z") + minutes * 60_000;
  const time = (iso: string) => iso.slice(11, 16);

  test("are current with the newest kept backup, marked when taken during a turn", () => {
    const view = backupView(backup(), at(1));
    expect(view).toEqual({
      health: "current",
      at: "2026-10-27T10:00:00Z",
      live: true,
      error: null,
      checked: "2026-10-27T10:05:00Z",
      incomplete: [],
    });
    expect(transferCopy(false).backup(view, time)).toBe(
      "Backed up, encrypted, at 10:00 (taken during a turn); unchanged at 10:05.",
    );
    expect(transferCopy(true).backup(view, time)).toBe(
      "Sauvegardé, chiffré, à 10:00 (prise pendant un tour) ; inchangé à 10:05.",
    );
    // Checked as it was taken: nothing more to say.
    const fresh = backupView(backup({ checked_at: "2026-10-27T10:00:30Z" }), at(1));
    expect(transferCopy(false).backup(fresh, time)).toBe("Backed up, encrypted, at 10:00 (taken during a turn).");
  });

  test("turn stale on this device's clock fifteen minutes after the last news, as when the worker went silent", () => {
    expect(backupView(backup(), at(15)).health).toBe("current");
    const silent = backupView(backup(), at(16));
    expect(silent.health).toBe("stale");
    expect(transferCopy(false).backup(silent, time)).toBe(
      "Last backup at 10:00 (taken during a turn): none succeeded for 15 minutes.",
    );
  });

  test("a first backup awaited fifteen minutes after the setup turns stale, as when the worker went silent at once", () => {
    const pending = backup({ health: "pending", backups: [], last_success_at: null, checked_at: null, last_attempt_at: null });
    expect(backupView(pending, Date.parse("2026-10-27T10:13:00Z")).health).toBe("pending");
    const silent = backupView(pending, Date.parse("2026-10-27T10:15:00Z"));
    expect(silent.health).toBe("stale");
    expect(transferCopy(false).backup(silent, time)).toBe("No backup succeeded for 15 minutes.");
    expect(transferCopy(true).backup(silent, time)).toBe("Aucune sauvegarde n'a réussi depuis 15 minutes.");
  });

  test("a conversation the newest backup could not carry is named", () => {
    const view = backupView(backup({ incomplete: ["Fix the build: no complete record yet"] }), at(1));
    expect(transferCopy(false).backupIncomplete(view.incomplete)).toBe(
      "A conversation could not be backed up; tried again at the next change, or within the hour: Fix the build: no complete record yet",
    );
  });

  test("a failure says the last good one is kept, or that none is yet", () => {
    const failed = backupView(backup({ health: "failed", error: "storage is down" }), at(1));
    expect(failed.error).toBe("storage is down");
    expect(transferCopy(false).backup(failed, time)).toBe(
      "The last backup failed; the one before, at 10:00 (taken during a turn), is kept.",
    );
    const none = backupView(backup({ health: "failed", backups: [], last_success_at: null }), at(1));
    expect(transferCopy(false).backup(none, time)).toBe("The backup failed; none is kept yet.");
    expect(transferCopy(false).backup(backupView(backup({ health: "pending", backups: [] }), at(1)), time)).toBe(
      "First encrypted backup on its way.",
    );
  });
});

test("worker preparation is visible and cancellable in both languages and retry preserves Cloud", () => {
  const preparing = transfer({
    stage: "preparing_worker",
    from: { kind: "here" },
    to: { kind: "cloud", beyond_limit: true },
  });
  for (const french of [true, false]) expect(stageLabel(preparing, transferCopy(french))).toBeTruthy();
  expect(cancellable(preparing)).toBe(true);
  expect(retryParams({ ...preparing, phase: "failed" })?.destination).toEqual({ kind: "cloud", beyond_limit: true });
});


test("progress revisions win without a new log line and preparation failures remain visible", () => {
  const before = transfer({ revision: 1 });
  const after = transfer({ revision: 2 });
  expect(latestTransfer(before, after)).toBe(after);
  const attention = transfer({ phase: "done", preparation: { status: "needs_attention", scripts: ["setup api"], message: "Setup failed" } });
  expect(visibleTransfer(attention, null)).toBe(attention);
  expect(endedNotification(attention, "billing", transferCopy(false))?.failed).toBe(true);
  expect(visibleTransfer(attention, attention.id)).toBeNull();
});
