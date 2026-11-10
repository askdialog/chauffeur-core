// On a throwaway app with a ready `background-transfer` workspace (verify-in-app skill):
// bun .claude/skills/verify-in-app/browser.ts <browser-dir> run - < app/scripts/check-background-transfers.js
// Startup stays pending until navigation has been verified; no real transfer or GCP call is made.
await page.keyboard.press("Escape");
await page.evaluate(async () => {
  const { client } = await import("/src/state/app.ts");
  const original = client.call.bind(client);
  window.backgroundTransferOriginalCall = original;
  client.call = (method, params) => {
    if (method === "cloud.transfer.start") {
      return new Promise((resolve, reject) => {
        window.backgroundTransferReject = reject;
        window.backgroundTransferRequest = params;
      });
    }
    return original(method, params);
  };
});
try {
  await page.locator('[data-workspace="background-transfer"]').click({ button: "right" });
  await page.getByText("Transfer…", { exact: true }).click();
  await page.locator('[data-testid="confirm-transfer"]').click();
  await page.locator('[data-testid="confirm-transfer"]').waitFor({ state: "hidden", timeout: 2000 });
  await page.locator('[data-testid="transfer-batch-pill"]').waitFor({ timeout: 2000 });
  if (await page.locator('[data-testid="transfer-batch"]').count()) throw new Error("Progress opened a blocking dialog");
  const request = await page.evaluate(() => window.backgroundTransferRequest);
  if (request.destination.kind !== "cloud") throw new Error("Cloud destination was lost");
  await page.locator('[data-workspace="background-transfer"]').click();
  await page.locator('[data-testid="composer"]').waitFor();
  await page.evaluate(() => window.backgroundTransferReject(new Error("Synthetic startup failure; retry the transfer")));
  await page.locator('[data-testid="transfer-batch-pill"]').click();
  await page.getByText("Synthetic startup failure; retry the transfer", { exact: true }).waitFor();
  await page.locator('[data-testid="transfer-batch-retry"]').waitFor();
  await page.locator('[data-testid="transfer-batch-close"]').click();
  await page.locator('[data-testid="composer"]').waitFor();
  await page.evaluate(async () => {
    const { useApp } = await import("/src/state/app.ts");
    const { client } = await import("/src/state/app.ts");
    const state = useApp.getState();
    const workspace = Object.values(state.workspaces).find((w) => w.slug === "background-transfer");
    window.backgroundTransferWorkspace = workspace;
    useApp.setState({ workspaces: { ...state.workspaces, [workspace.id]: { ...workspace, moved_to: "test-vm" } } });
    const previous = client.call.bind(client);
    client.call = async (method, params) => {
      if (method === "settings.get") {
        const settings = await window.backgroundTransferOriginalCall(method, params);
        return { ...settings, transfer: { ...settings.transfer, machines: [{ name: "test-vm", socket: "/tmp/synthetic-vm.sock" }] } };
      }
      return previous(method, params);
    };
  });
  await page.locator('[data-testid="transfer-bring-back"]').click();
  await page.getByRole("checkbox").check();
  await page.locator('[data-testid="confirm-transfer"]').click();
  await page.locator('[data-testid="confirm-transfer"]').waitFor({ state: "hidden", timeout: 2000 });
  const returning = await page.evaluate(() => window.backgroundTransferRequest);
  if (returning.source.name !== "test-vm" || returning.destination.kind !== "here" || !returning.keep_local_copy) {
    throw new Error("Return lost its source or local copy choice");
  }
  await page.evaluate(() => window.backgroundTransferReject(new Error("Synthetic return failure; retry the return")));
  await page.locator('[data-testid="transfer-batch-pill"]').click();
  await page.getByText("Synthetic return failure; retry the return", { exact: true }).waitFor();
  await page.locator('[data-testid="transfer-batch-retry"]').click();
  const retried = await page.evaluate(() => window.backgroundTransferRequest);
  if (JSON.stringify(retried) !== JSON.stringify(returning)) throw new Error("Retry changed the return request");
  await page.evaluate(() => window.backgroundTransferReject(new Error("Synthetic return failure; retry the return")));
  await page.locator('[data-testid="transfer-batch-close"]').click();
  return "PASS: pending startup closes confirmation, navigation stays usable, failures remain accessible, and return retries preserve their source and local copy choice.";
} finally {
  await page.evaluate(async () => {
    const { client } = await import("/src/state/app.ts");
    client.call = window.backgroundTransferOriginalCall;
    const { useApp } = await import("/src/state/app.ts");
    if (window.backgroundTransferWorkspace) {
      const workspace = window.backgroundTransferWorkspace;
      useApp.setState({ workspaces: { ...useApp.getState().workspaces, [workspace.id]: workspace } });
    }
  });
}
