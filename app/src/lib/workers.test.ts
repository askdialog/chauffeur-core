import { describe, expect, test } from "bun:test";
import type { WorkerInfo } from "@chauffeur/protocol";
import {
  beyondLimit,
  changing,
  hourly,
  nextWorkerName,
  remoteEditors,
  remoteOpen,
  validWorkerName,
  watchRemote,
  workerCopy,
  workerStatus,
} from "./workers";

const never = () => () => {};

const worker = (change: Partial<WorkerInfo> = {}): WorkerInfo => ({
  name: "w1",
  instance: "chauffeur-worker-w1",
  project: "acme-chauffeur",
  zone: "europe-west9-a",
  machine_type: "e2-standard-4",
  hourly_usd: 0.1684,
  state: "ready",
  connected: true,
  socket: "/s",
  ssh_host: "chauffeur-worker-w1",
  ssh_issue: null,
  log: [],
  error: null,
  created_at: "2026-10-26T10:00:00Z",
  ...change,
});

describe("workers", () => {
  test("both languages have the same keys", () => {
    expect(Object.keys(workerCopy(true)).sort()).toEqual(Object.keys(workerCopy(false)).sort());
  });

  test("an hourly cost reads to the cent, in each language's way", () => {
    expect(hourly(0.1684, false)).toBe("$0.17/h");
    expect(hourly(0.0906, true)).toBe("0,09 $/h");
  });

  test("a ready worker whose forward dropped reads as reconnecting", () => {
    const copy = workerCopy(false);
    expect(workerStatus(worker(), copy).label).toBe("Ready");
    expect(workerStatus(worker({ connected: false }), copy).tone).toBe("attention");
    expect(workerStatus(worker({ state: "failed" }), copy).tone).toBe("danger");
    expect(workerStatus(worker({ state: "stopped" }), copy).label).toBe("Stopped, only its disk billed");
  });

  test("the list is read again while a worker is created or deleted", () => {
    expect(changing([worker()])).toBe(false);
    expect(changing([worker(), worker({ state: "installing" })])).toBe(true);
    expect(changing([worker({ state: "failed" })])).toBe(false);
    expect(changing([worker({ state: "stopping" })])).toBe(true);
    expect(changing([worker({ state: "stopped" })])).toBe(false);
  });

  test("a new worker's name is the first free one, and only what the daemon takes is valid", () => {
    expect(nextWorkerName([])).toBe("worker-1");
    expect(nextWorkerName([{ name: "worker-1" }, { name: "worker-3" }])).toBe("worker-2");
    for (const name of ["w", "worker-1", "a1-b2"]) expect(validWorkerName(name)).toBe(true);
    for (const name of ["", "W1", "1w", "w_1", "w-", "a-name-longer-than-twenty"]) expect(validWorkerName(name)).toBe(false);
  });

  test("a third worker goes past the limit", () => {
    expect(beyondLimit({ workers: [worker()], limit: 2 })).toBe(false);
    expect(beyondLimit({ workers: [worker(), worker({ name: "w2" })], limit: 2 })).toBe(true);
  });

  test("a workspace on a worker opens over SSH once its entry is included, and says why not before", () => {
    expect(remoteOpen("w1", [worker()])).toEqual({ machine: "w1", host: "chauffeur-worker-w1", issue: null });
    // A machine declared by hand opens nowhere from here: no button at all.
    expect(remoteOpen("vm", [worker()])).toBeNull();
    const missing = 'Add `Include "/Users/me/.chauffeur/ssh_config"` at the top of /Users/me/.ssh/config, then open it again';
    expect(remoteOpen("w1", [worker({ ssh_host: null, ssh_issue: missing })])).toEqual({ machine: "w1", host: null, issue: missing });
    expect(remoteOpen("w1", [worker({ state: "deleting", ssh_host: null, ssh_issue: "The worker w1 is not ready" })])).toEqual({
      machine: "w1",
      host: null,
      issue: "The worker w1 is not ready",
    });
  });

  test("only Cursor and VS Code open a folder on a worker", () => {
    const app = (id: string, kind: "editor" | "terminal" | "files" = "editor") => ({ id, name: id, kind, is_default: false });
    expect(remoteEditors([app("cursor"), app("zed"), app("vscode"), app("iterm", "terminal"), app("finder", "files")]).map((a) => a.id)).toEqual([
      "cursor",
      "vscode",
    ]);
  });

  test("an answer about a workspace's worker that comes after it came back is dropped", async () => {
    let answer: (result: { workers: WorkerInfo[] }) => void = () => {};
    const seen: Array<string | null> = [];
    const cancel = watchRemote("w1", () => new Promise((resolve) => (answer = resolve)), (r) => seen.push(r?.machine ?? null), () => {}, never);
    cancel();
    answer({ workers: [worker()] });
    await Promise.resolve();
    await Promise.resolve();
    expect(seen).toEqual([]);
    watchRemote("w1", async () => ({ workers: [worker()] }), (r) => seen.push(r?.machine ?? null), () => {}, never);
    await Promise.resolve();
    await Promise.resolve();
    expect(seen).toEqual(["w1"]);
  });

  test("a failure to list the workers is reported, unless the question was dropped", async () => {
    const errors: unknown[] = [];
    watchRemote("w1", async () => Promise.reject(new Error("daemon down")), () => {}, (e) => errors.push(e), never);
    const cancel = watchRemote("w1", async () => Promise.reject(new Error("late")), () => {}, (e) => errors.push(e), never);
    cancel();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(errors.map((e) => (e as Error).message)).toEqual(["daemon down"]);
  });
  test("the workers are read again when the window comes back, so an Include added by hand shows", async () => {
    let focus = () => {};
    let unsubscribed = false;
    const answers = [
      { workers: [worker({ ssh_host: null, ssh_issue: "Add `Include …`" })] },
      { workers: [worker()] },
    ];
    const seen: Array<{ host: string | null; issue: string | null } | null> = [];
    const cancel = watchRemote(
      "w1",
      async () => answers.shift() ?? { workers: [] },
      (r) => seen.push(r && { host: r.host, issue: r.issue }),
      () => {},
      (listener) => {
        focus = listener;
        return () => (unsubscribed = true);
      },
    );
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(seen).toEqual([{ host: null, issue: "Add `Include …`" }]);
    focus();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(seen.at(-1)).toEqual({ host: "chauffeur-worker-w1", issue: null });
    cancel();
    expect(unsubscribed).toBe(true);
  });

  test("a failure to list the workers is reported once until a read succeeds again", async () => {
    let focus = () => {};
    const errors: unknown[] = [];
    let fail = true;
    watchRemote(
      "w1",
      async () => (fail ? Promise.reject(new Error("daemon down")) : { workers: [worker()] }),
      () => {},
      (e) => errors.push(e),
      (listener) => ((focus = listener), () => {}),
    );
    await new Promise((resolve) => setTimeout(resolve, 0));
    focus();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(errors.length).toBe(1);
    fail = false;
    focus();
    await new Promise((resolve) => setTimeout(resolve, 0));
    fail = true;
    focus();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(errors.length).toBe(2);
  });
});
