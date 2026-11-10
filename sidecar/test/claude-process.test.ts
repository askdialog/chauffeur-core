import { expect, test } from "bun:test";
import { ClaudeProcess } from "../src/agents/claude/process.ts";

const options = (code: string) => ({
  command: process.execPath, args: ["-e", code], cwd: process.cwd(), env: process.env, signal: new AbortController().signal,
});

test("Claude shutdown seals deferred launches before any child exists", async () => {
  const child = new ClaudeProcess(() => {});
  await child.stop(10);
  expect(() => child.spawn(options("process.exit(0)"))).toThrow("launch is closed");
});

test("Claude root exit is confirmed independently of an SDK iterator, with escalation for a lingering child", async () => {
  const owned = new ClaudeProcess(() => {});
  const child = owned.spawn(options('process.on("SIGTERM", () => {}); console.log("ready"); setInterval(() => {}, 1000);'));
  try {
    await new Promise<void>((resolve, reject) => { child.stdout.once("data", () => resolve()); child.once("error", reject); });
    expect(owned.pid).toBe(child.pid);
    expect(owned.pid).toBeGreaterThan(0);
    await owned.stop(20);
    expect(child.signalCode).toBe("SIGKILL");
    await owned.stop(0);
    expect(() => owned.spawn(options("process.exit(0)"))).toThrow();
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
  }
});

test("an SDK abort signal with a native PID is followed by actual root exit", async () => {
  const abort = new AbortController();
  const owned = new ClaudeProcess(() => {});
  const child = owned.spawn({ ...options('console.log("ready"); setInterval(() => {}, 1000);'), signal: abort.signal });
  try {
    await new Promise<void>((resolve, reject) => { child.stdout.once("data", () => resolve()); child.once("error", reject); });
    abort.abort();
    await owned.stop(100);
    expect(child.signalCode).not.toBeNull();
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
  }
});
