import { expect, mock } from "bun:test";
import type { PowerState, UpdateStatus } from "@chauffeur/protocol";
import { domTest } from "@/test/dom";

// Each domTest runs in a process of its own; the mocks are declared there only, lest they reach the
// other test files that the parent process runs.
let working = 0;
let relaunched = 0;
let installs = 0;
let phase: UpdateStatus["phase"] = "idle";
const status = (): UpdateStatus => ({ current: "0.9.0", available: "0.10.0", phase, error: null, releases_url: null });
const power = (): PowerState => ({
  keep_awake: "while_working",
  holding: working > 0 ? "idle_sleep" : "none",
  working_chats: working,
  running_tasks: 0,
  creating_workspaces: 0,
  lingering_secs: null,
  low_battery: false,
  battery_percent: null,
  warning: null,
});

const inChild = process.env.CHAUFFEUR_DOM_TEST?.startsWith(`${import.meta.path}#`);
if (inChild) {
  mock.module("@/lib/native", () => ({
    isTauri: true,
    relaunchApp: async () => {
      relaunched += 1;
      return true;
    },
  }));
  mock.module("@/state/app", () => ({
    client: {
      call: async (method: string) => {
        if (method === "power.get") return power();
        if (method === "update.install") {
          installs += 1;
          phase = "downloading";
        }
        return status();
      },
    },
    toast: () => {},
    useApp: (select: (state: unknown) => unknown) => select({ connection: { status: "open" } }),
  }));
}

const { UpdateCard } = inChild ? await import("./UpdateCard") : { UpdateCard: () => null };
const text = (id: string) => document.querySelector(`[data-testid="${id}"]`)?.textContent?.trim() ?? null;
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

domTest("update now installs, then restarts at once when no agent works", import.meta.path, async ({ render, click, until }) => {
  await render(<UpdateCard />);
  await until(() => text("update-restart") === "Update now");
  await click('[data-testid="update-restart"]');
  await until(() => text("update-step") === "Downloading… Restarts once installed.");
  expect(installs).toBe(1);
  expect(relaunched).toBe(0);
  phase = "installed";
  await until(() => relaunched === 1);
});

domTest("update now while agents work warns before installing", import.meta.path, async ({ render, click, until }) => {
  working = 2;
  await render(<UpdateCard />);
  await until(() => text("update-restart") !== null);
  await click('[data-testid="update-restart"]');
  await until(() => text("update-warning")?.startsWith("2 agents working") ?? false);
  expect(installs).toBe(0);
  await click('[data-testid="update-anyway"]');
  await until(() => installs === 1);
  phase = "installed";
  await until(() => relaunched === 1);
});

domTest("restart when idle installs, then waits for the agents before restarting", import.meta.path, async ({ render, click, until }) => {
  working = 2;
  await render(<UpdateCard />);
  await until(() => text("update-when-idle") === "When idle");
  await click('[data-testid="update-when-idle"]');
  await until(() => text("update-step") === "Downloading… Restarts once installed and no agent works.");
  phase = "installed";
  await until(() => text("update-waiting") === "Restarts once no agent works: 2 agents working.");
  expect(relaunched).toBe(0);
  working = 0;
  await wait(2_500);
  await until(() => relaunched === 1);
});

domTest("a cancelled plan installs without restarting, and leaves Restart to the user", import.meta.path, async ({ render, click, until }) => {
  await render(<UpdateCard />);
  await until(() => text("update-when-idle") !== null);
  await click('[data-testid="update-when-idle"]');
  await until(() => text("update-cancel") === "Cancel");
  await click('[data-testid="update-cancel"]');
  phase = "installed";
  await until(() => text("update-restart") === "Restart");
  await wait(1_500);
  expect(relaunched).toBe(0);
});

domTest("a failed installation drops the planned restart", import.meta.path, async ({ render, click, until }) => {
  await render(<UpdateCard />);
  await until(() => text("update-restart") !== null);
  await click('[data-testid="update-restart"]');
  await until(() => installs === 1);
  phase = "failed";
  await until(() => text("update-restart") === "Update now");
  phase = "installed";
  // Coming back to the window reads the status again, as after the update installed from the CLI.
  window.dispatchEvent(new window.Event("focus"));
  await until(() => text("update-restart") === "Restart");
  expect(relaunched).toBe(0);
});
