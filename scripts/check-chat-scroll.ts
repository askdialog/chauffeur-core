#!/usr/bin/env bun
// Run against a dev server with an empty CHAUFFEUR_HOME (no daemon or agent needed):
// bun scripts/check-chat-scroll.ts http://localhost:41509 [screenshot.png]
// BROWSER=webkit bun scripts/check-chat-scroll.ts http://localhost:41509 [screenshot.png]
// Install WebKit first with bunx playwright@1.64.1 install webkit.
import { strict as assert } from "node:assert";
import { chromium, webkit } from "playwright-core@1.64.1";

const origin = process.argv[2];
if (!origin) throw new Error("Pass the isolated app dev server's URL");
const browser = process.env.BROWSER === "webkit"
  ? await webkit.launch({ headless: true })
  : await chromium.launch({
    executablePath: process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    headless: true,
  });
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  await page.goto(`${origin}/?long&stream#chat-fixture`);
  const scroller = page.locator("[data-virtuoso-scroller]");
  await scroller.waitFor();
  // Virtuoso retries the initial jump for up to 1200 ms while new rows are measured.
  await page.waitForTimeout(1500);
  const last = await page.locator("[data-index]").last().getAttribute("data-index");
  assert(Number(last) > 1600, "The fixture must exercise a long conversation");
  const initialGap = await scroller.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight);
  assert(initialGap <= 2, `Opening a long chat must reach the bottom: ${initialGap}px remaining`);

  // Leave live following through a native user scroll before placing the reader in history.
  await scroller.hover();
  await page.mouse.wheel(0, -600);
  await page.waitForFunction(() => {
    const el = document.querySelector("[data-virtuoso-scroller]");
    return el !== null && el.scrollHeight - el.scrollTop - el.clientHeight > 100;
  });
  await scroller.evaluate((el) => { el.scrollTop = el.scrollHeight / 2; });
  await page.waitForTimeout(700);
  const rows = page.locator("[data-index]");
  const measurements = await rows.evaluateAll((elements) => elements.map((el) => ({
    known: Number(el.getAttribute("data-known-size")),
    actual: el.getBoundingClientRect().height,
  })));
  assert(measurements.some(({ actual }) => actual !== Math.round(actual)), "Exercise fractional row heights");
  for (const { known, actual } of measurements) {
    assert(Math.abs(known - actual) < 0.01, `Row measurement drifts: ${known} versus ${actual}`);
  }

  const before = await scroller.evaluate((el) => el.scrollTop);
  await page.waitForTimeout(1500);
  const after = await scroller.evaluate((el) => el.scrollTop);
  assert(Math.abs(after - before) <= 1, "Streaming must not move a reader in the history");

  // Repeatedly cross virtualisation boundaries in both directions.
  for (const direction of [-1, 1]) {
    for (let i = 0; i < 12; i++) {
      await scroller.evaluate((el, delta) => { el.scrollTop += delta; }, direction * 300);
      await page.waitForTimeout(80);
      assert(await rows.count() > 0, "Scrolling must keep messages rendered");
    }
  }
  // The timeline's ticks sit over the list and forward wheel movement into its scroller.
  const ticks = page.getByTestId("timeline-tick");
  await ticks.last().hover();
  const beforeWheel = await scroller.evaluate((el) => el.scrollTop);
  await page.mouse.wheel(0, 300);
  await page.waitForFunction((before) => {
    const el = document.querySelector("[data-virtuoso-scroller]");
    return el !== null && el.scrollTop > before;
  }, beforeWheel);

  await page.getByRole("button", { name: "Latest", exact: true }).click();
  await page.waitForTimeout(700);
  const bottomGap = await scroller.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight);
  assert(bottomGap <= 2, `Latest must reach the bottom: ${bottomGap}px remaining`);
  await page.waitForTimeout(1500);
  const streamingGap = await scroller.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight);
  assert(streamingGap <= 2, `Streaming must stay at the bottom: ${streamingGap}px remaining`);
  if (process.argv[3]) await page.screenshot({ path: process.argv[3] });
  console.log("Long chat: exact row heights, stable history, scrolling and live following passed");
} finally {
  await browser.close();
}
