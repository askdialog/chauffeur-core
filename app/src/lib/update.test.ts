import { describe, expect, test } from "bun:test";
import type { PowerState, UpdateStatus } from "@chauffeur/protocol";
import { idleForRestart, plannedRestart, restartWarning, updateCard, updating, waitingForIdle } from "./update";

const status = (over: Partial<UpdateStatus>): UpdateStatus => ({
  current: "0.9.0",
  available: "0.10.0",
  phase: "idle",
  error: null,
  releases_url: null,
  ...over,
});

describe("updateCard", () => {
  test("nothing shows when no newer version is published", () => {
    expect(updateCard(null, null)).toBeNull();
    expect(updateCard(status({ available: null }), null)).toBeNull();
  });

  test("a version the user hid stays hidden, a newer one shows again", () => {
    expect(updateCard(status({}), "0.10.0")).toBeNull();
    expect(updateCard(status({ available: "0.11.0" }), "0.10.0")).toEqual({ kind: "available", version: "0.11.0" });
  });

  test("a failed installation can be hidden too", () => {
    expect(updateCard(status({ phase: "failed", error: "no access" }), "0.10.0")).toBeNull();
  });

  test("an installation under way shows even for a hidden version", () => {
    expect(updateCard(status({ phase: "downloading" }), "0.10.0")).toEqual({
      kind: "installing",
      version: "0.10.0",
      step: "Downloading…",
    });
    expect(updateCard(status({ phase: "installed" }), "0.10.0")).toEqual({ kind: "installed", version: "0.10.0" });
    expect(updateCard(status({ phase: "failed", error: "gh release download failed: HTTP 404" }), null)).toEqual({
      kind: "failed",
      version: "0.10.0",
      error: "gh release download failed: HTTP 404",
    });
  });

  test("only downloading and installing are followed closely", () => {
    expect(updating(status({ phase: "downloading" }))).toBe(true);
    expect(updating(status({ phase: "installing" }))).toBe(true);
    expect(updating(status({ phase: "installed" }))).toBe(false);
    expect(updating(null)).toBe(false);
  });
});

const power = (over: Partial<PowerState>): PowerState => ({
  keep_awake: "while_working",
  holding: "idle_sleep",
  working_chats: 0,
  running_tasks: 0,
  creating_workspaces: 0,
  lingering_secs: null,
  low_battery: false,
  battery_percent: null,
  warning: null,
  ...over,
});

describe("restartWarning", () => {
  test("restarting is free when no agent works", () => {
    expect(restartWarning(power({}))).toBeNull();
    expect(restartWarning(power({ running_tasks: 2, lingering_secs: 60 }))).toBeNull();
  });

  test("working agents and workspaces being created are named", () => {
    expect(restartWarning(power({ working_chats: 2 }))).toBe(
      "2 agents working: restarting interrupts their turns, which resume once Chauffeur is back.",
    );
    expect(restartWarning(power({ creating_workspaces: 1 }))).toStartWith("1 workspace being created:");
  });
});

describe("restart when idle", () => {
  test("it restarts once no agent works and no workspace is being created", () => {
    expect(idleForRestart(power({}))).toBe(true);
    expect(idleForRestart(power({ running_tasks: 1, lingering_secs: 60 }))).toBe(true);
    expect(idleForRestart(power({ working_chats: 1 }))).toBe(false);
    expect(idleForRestart(power({ creating_workspaces: 1 }))).toBe(false);
  });

  test("while it waits, the card says what it waits for", () => {
    expect(waitingForIdle(null)).toBe("Restarts once no agent works.");
    expect(waitingForIdle(power({ working_chats: 2 }))).toBe("Restarts once no agent works: 2 agents working.");
  });

  test("while the update installs, the card says which restart follows", () => {
    expect(plannedRestart("now")).toBe("Restarts once installed.");
    expect(plannedRestart("idle")).toBe("Restarts once installed and no agent works.");
  });
});
