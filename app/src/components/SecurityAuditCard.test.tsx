import { expect, spyOn } from "bun:test";
import type { SecurityAudit } from "@chauffeur/protocol";
import { domTest } from "@/test/dom";

domTest("an active audit requires confirmation and displays the request until Codex reports a result", import.meta.path, async ({ render, click, until }) => {
  const { client } = await import("@/state/app");
  const { SecurityAuditCard } = await import("./SecurityAuditCard");
  const now = new Date().toISOString();
  const scan: SecurityAudit = {
    id: "scan", repo: "shop", mode: "deep", phase: "discovery", status: "running",
    progress: { completed: 7, total: 8, unit: "passes" }, last_activity_at: now, heartbeat_at: now,
    owner_chat: "codex-chat", has_report: false,
  };
  const rpc = spyOn(client, "call").mockImplementation(async (method) => {
    if (method === "security_audit.list") return [scan];
    return {};
  });
  try {
    await render(<SecurityAuditCard workspace="workspace" />);
    await until(() => document.body.textContent?.includes("7/8 passes") === true);
    await click('[data-testid="security-audit-card"] li button');
    await until(() => document.body.textContent?.includes("Cancel security audit?") === true);
    expect(rpc.mock.calls.some(([method]) => method === "security_audit.cancel")).toBe(false);
    const confirm = [...document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')]
      .find((button) => button.textContent === "Request cancellation");
    expect(confirm).toBeDefined();
    confirm?.click();
    await until(() => document.body.textContent?.includes("Cancellation requested") === true);
    expect(rpc).toHaveBeenCalledWith("security_audit.cancel", { workspace: "workspace", scan: "scan" });
    expect(document.querySelector('[data-testid="security-audit-card"]')?.textContent).toContain("Running");
  } finally {
    rpc.mockRestore();
  }
});
