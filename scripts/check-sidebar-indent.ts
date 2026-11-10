#!/usr/bin/env bun
// Run against an isolated dev app with parent-indent and its child-indent workspace:
// bun scripts/check-sidebar-indent.ts http://localhost:41309 [screenshot.png]
import { strict as assert } from "node:assert";
import { chromium } from "playwright-core@1.64.1";

const origin = process.argv[2];
if (!origin) throw new Error("Pass the isolated app dev server's URL");
const browser = await chromium.launch({
  executablePath: process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  headless: true,
});
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  await page.goto(origin);
  const parent = page.locator('[data-workspace="parent-indent"]');
  const child = page.locator('[data-workspace="child-indent"]');
  await child.waitFor();
  const padding = () => child.evaluate((element) => {
    const style = getComputedStyle(element);
    return [style.paddingLeft, style.paddingRight];
  });
  assert.deepEqual(await padding(), ["24px", "8px"]);

  // Separately built extensions repeat shared utilities after Core's stylesheet. Their px-2
  // previously overrode pl-6 through padding-inline, even though the parent link was intact.
  for (const id of ["database", "office"]) {
    await page.route(`**/indent-${id}.css`, (route) => route.fulfill({
      contentType: "text/css",
      body: "@layer utilities { .px-2 { padding-inline: calc(var(--spacing) * 2); } }",
    }));
    await page.addStyleTag({ url: `${origin}/indent-${id}.css` });
    assert.deepEqual(await padding(), ["24px", "8px"], `${id} must preserve the child's indentation`);
    assert.equal(await parent.evaluate((element) => getComputedStyle(element).paddingLeft), "8px");
  }

  await child.click();
  assert.equal(await child.getAttribute("aria-current"), "page");
  assert.deepEqual(await padding(), ["24px", "8px"], "Selection must preserve indentation");
  const fold = page.locator("[data-workspace-row]").filter({ has: parent }).getByTestId("row-children-toggle");
  await fold.click();
  assert.equal(await child.count(), 0, "The parent's chevron must still hide its child");
  await fold.click();
  await child.waitFor();
  assert.deepEqual(await padding(), ["24px", "8px"], "Unfolding must preserve indentation");
  if (process.argv[3]) await page.screenshot({ path: process.argv[3] });
  console.log("Sidebar: indentation survives extension styles, selection and folding");
} finally {
  await browser.close();
}
