// On a throwaway app with a ready `sidebar-parent` and three child workspaces:
// .claude/skills/verify-in-app/browser.ts <browser-dir> run - < app/scripts/check-sidebar-activity.js
const parentRow = page.locator('[data-workspace="sidebar-parent"]');
await parentRow.waitFor();
const children = await page.evaluate(async () => {
  const { useApp } = await import("/src/state/app.ts");
  const { workspaces } = useApp.getState();
  const parent = Object.values(workspaces).find((workspace) => workspace.slug === "sidebar-parent");
  const children = Object.values(workspaces).filter((workspace) => workspace.parent_id === parent.id);
  if (children.length !== 3) throw new Error("Expected three child workspaces");
  useApp.setState({
    workspaces: {
      ...workspaces,
      [parent.id]: { ...parent, activity: "working", running_tasks: 1, pinned: true },
      [children[0].id]: { ...children[0], activity: "working" },
      [children[1].id]: { ...children[1], activity: "needs_input" },
    },
  });
  return children.map((child) => child.slug);
});

const toggle = parentRow.locator("..").getByTestId("row-children-toggle");
if (await toggle.getAttribute("aria-expanded") === "true") await toggle.click();
const parentSpinner = parentRow.locator('[title="Working"] .animate-spin');
const childSpinner = toggle.getByLabel("A sub-workspace is working");
const childAttention = toggle.getByLabel("A sub-workspace needs input");
for (const width of [200, 248]) {
  await page.evaluate(async (sidebarWidth) => {
    const { useApp } = await import("/src/state/app.ts");
    useApp.setState({ sidebarWidth });
  }, width);
  await page.waitForFunction((width) => document.querySelector("[data-sidebar]").getBoundingClientRect().width === width, width);
  const [parent, button] = await Promise.all([parentSpinner.boundingBox(), toggle.boundingBox()]);
  if (!parent || !button || parent.x + parent.width > button.x) {
    throw new Error(`The parent status overlaps the child toggle at ${width}px`);
  }
  await childSpinner.waitFor();
  await childAttention.waitFor();
}

const childRow = page.locator(`[data-workspace="${children[0]}"]`);
await toggle.click();
await childRow.waitFor();
await childSpinner.waitFor({ state: "hidden" });
await toggle.click();
await childRow.waitFor({ state: "hidden" });
await childSpinner.waitFor();
console.log("PASS: parent and child activity have distinct space, and the toggle opens and closes the children.");
