import { expect, spyOn } from "bun:test";
import type { EventEnvelope } from "@chauffeur/protocol";
import { domTest } from "@/test/dom";

domTest("a failed local sign-in clears the waiting state and shows the reason", import.meta.path, async () => {
  const { client, signIn, startApp, useApp } = await import("./app");
  const { useToasts } = await import("./toasts");
  const start = spyOn(client, "start").mockImplementation(() => {});
  const subscribe = spyOn(client, "subscribe");
  const rpc = spyOn(client, "call").mockImplementation(async () => ({ url: "https://example.com/sign-in" }));
  const open = spyOn(window, "open").mockImplementation(() => null);
  try {
    startApp();
    const onRegistry = subscribe.mock.calls[0][1];
    await signIn("vibe");
    expect(useApp.getState().signingIn).toEqual(["vibe"]);
    function announce(worker?: string) {
      const envelope: EventEnvelope = {
        seq: null,
        ts: "2026-11-20T12:00:00Z",
        event: {
          scope: "registry",
          event: { type: "login_ended", agent: "vibe", worker, success: false, error: "Vibe could not save its credentials" },
        },
      };
      onRegistry(envelope);
    }
    announce("worker-1");
    expect(useApp.getState().signingIn).toEqual(["vibe"]);
    announce();
    expect(useApp.getState().signingIn).toEqual([]);
    expect(useToasts.getState().toasts.at(-1)?.message).toBe("Mistral Vibe could not sign in: Vibe could not save its credentials");
  } finally {
    open.mockRestore();
    rpc.mockRestore();
    subscribe.mockRestore();
    start.mockRestore();
  }
});
