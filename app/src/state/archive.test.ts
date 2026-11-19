import { expect, spyOn } from "bun:test";
import type { EventEnvelope, WorkspaceInfo } from "@chauffeur/protocol";
import { domTest } from "@/test/dom";

domTest("an archive failure offers to retry the same workspace without forcing it", import.meta.path, async () => {
  const { client, startApp, useApp } = await import("./app");
  const { useToasts } = await import("./toasts");
  const archiving = { id: "workspace-1", slug: "my-workspace", state: "archiving" } as WorkspaceInfo;
  const failed: WorkspaceInfo = { ...archiving, state: "cleanup_failed", last_error: "Git index is locked" };
  const start = spyOn(client, "start").mockImplementation(() => {});
  const subscribe = spyOn(client, "subscribe");
  const rpc = spyOn(client, "call").mockImplementation(async () => archiving);

  try {
    startApp();
    const onRegistry = subscribe.mock.calls[0][1];
    function announce(workspace: WorkspaceInfo) {
      const envelope: EventEnvelope = {
        seq: null,
        ts: "2026-11-19T12:00:00Z",
        event: { scope: "registry", event: { type: "workspace_upserted", workspace } },
      };
      onRegistry(envelope);
    }

    announce(archiving);
    announce(failed);
    const [failure] = useToasts.getState().toasts;
    expect(failure.message).toContain("Git index is locked");
    expect(failure.action?.label).toBe("Retry archive");
    await failure.action?.run();
    expect(rpc.mock.calls).toEqual([["workspace.archive", { workspace: failed.id, force: false, background: true }]]);
    expect(useApp.getState().workspaces[failed.id].state).toBe("archiving");
  } finally {
    rpc.mockRestore();
    subscribe.mockRestore();
    start.mockRestore();
  }
});
