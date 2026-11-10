// Isolated process so browser globals and store mocks cannot affect other app tests.
import assert from "node:assert/strict";
import { mock } from "bun:test";
import { Window } from "happy-dom";
import { act, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { create } from "zustand";
import type { WorkspaceInfo } from "@chauffeur/protocol";
import type { WorkspaceSession } from "../src/state/session";

const browser = new Window({ url: "http://localhost" });
Object.assign(globalThis, {
  window: browser, document: browser.document, navigator: browser.navigator,
  IS_REACT_ACT_ENVIRONMENT: true,
});
const useApp = create<{ terminalRequest: Record<string, { nonce: number; command: string }> }>(() => ({ terminalRequest: {} }));
const opened: Array<{ command?: string }> = [];
let listFails = false;
mock.module("../src/state/app", () => ({
  useApp, toast: () => {},
  client: { call: async (method: string, params: { command?: string }) => {
    if (method === "terminal.list") {
      if (listFails) throw new Error("List unavailable");
      return { terminals: [] };
    }
    if (method === "terminal.open") {
      opened.push(params);
      // Keep creation pending to reproduce list arriving before the login terminal is ready.
      return new Promise(() => {});
    }
    throw new Error(`Unexpected RPC: ${method}`);
  } },
}));
mock.module("../src/state/session", () => ({ useWorkspaceSelector: () => ({}) }));
const { TerminalPanel } = await import("../src/components/TerminalPanel");
const session = { viewTerminal: () => {}, listTerminals: () => {} } as unknown as WorkspaceSession;
const workspace = { id: "fixture", state: "ready", worktrees: [] } as unknown as WorkspaceInfo;
const host = document.createElement("div");
document.body.append(host);

async function scenario(login: boolean, fail: boolean) {
  opened.length = 0;
  listFails = fail;
  useApp.setState({ terminalRequest: login ? { fixture: { nonce: 1, command: "claude auth login" } } : {} });
  const root = createRoot(host);
  try {
    await act(async () => root.render(<StrictMode><TerminalPanel workspace={workspace} session={session} visible inUse={false} /></StrictMode>));
    assert.equal(opened.length, fail ? 0 : 1, "one opening at most, including a pending login request");
    if (login) assert.equal(opened[0].command, "claude auth login");
    await act(async () => root.render(<StrictMode><TerminalPanel workspace={workspace} session={session} visible={false} inUse={false} /></StrictMode>));
    await act(async () => root.render(<StrictMode><TerminalPanel workspace={workspace} session={session} visible inUse={false} /></StrictMode>));
    assert.equal(opened.length, fail ? 0 : 1, "returning to the tab never opens a second terminal");
  } finally {
    await act(() => root.unmount());
  }
}
try {
  await scenario(true, false);
  await scenario(false, false);
  await scenario(false, true);
  console.log("Initial terminal opening is coordinated with login requests and list failures.");
} finally {
  await browser.happyDOM.close();
}

// React keeps its scheduler channel alive in this isolated browser process.
process.exit(0);
