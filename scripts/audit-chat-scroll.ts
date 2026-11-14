#!/usr/bin/env bun
// BROWSER=webkit bun scripts/audit-chat-scroll.ts http://localhost:41419 /tmp/scroll-audit
// Synthetic data only; start Vite with an empty CHAUFFEUR_HOME. No agent/daemon is needed.
import { strict as assert } from "node:assert";
import { mkdir } from "node:fs/promises";
import { chromium, webkit, type Page } from "playwright-core@1.64.1";
const origin = process.argv[2];
if (!origin) throw new Error("Pass the isolated app dev server's URL");
const output = process.argv[3] ?? "/tmp/chauffeur-scroll-audit";
await mkdir(output, { recursive: true });
const engine = process.env.BROWSER === "webkit" ? "webkit" : "chromium";
const browser = engine === "webkit" ? await webkit.launch({ headless: true }) : await chromium.launch({
  executablePath: process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true,
});
const failures: string[] = [];
const results: { scenario: string; check: string; error?: string }[] = [];
const scenarios = [
  { name: "short", query: "count=1", count: 1 },
  { name: "single-tall", query: "count=1&tall=last&paragraphs=2000", count: 1 },
  { name: "first-tall", query: "count=100&tall=first&paragraphs=2000", count: 100 },
  { name: "last-tall", query: "count=100&tall=last&paragraphs=2000", count: 100 },
  { name: "all-tall", query: "count=200&tall=all&paragraphs=40", count: 200 },
  { name: "tall-user", query: "count=100&user-tall&paragraphs=2000", count: 100 },
  { name: "many", query: "count=10000", count: 10000 },
  { name: "late-layout", query: "count=100&late", count: 100 },
  { name: "partial-search", query: "count=200&partial", count: 200 },
  { name: "partial-end", query: "count=200&partial", count: 200 },
  { name: "partial-tall", query: "count=100&partial&tall=last&paragraphs=2000", count: 100 },
  { name: "partial", query: "count=200&partial", count: 200 },
];
if (process.env.STRESS) scenarios.push({ name: "hundred-thousand", query: "count=50000", count: 50000 });
const selector = "[data-virtuoso-scroller]";
async function bottom(page: Page) {
  await page.waitForFunction((selector) => {
    const el = document.querySelector(selector);
    return el && el.scrollHeight - el.scrollTop - el.clientHeight <= 2;
  }, selector, { timeout: 10000 });
}
async function latest(page: Page) {
  // The button can disappear between measuring a streaming row and clicking it. End expresses
  // the same intent even when already at the bottom; the Top/Latest check covers the buttons.
  await page.keyboard.press("End");
  await bottom(page);
}

async function anchor(page: Page) {
  return page.locator(selector).evaluate(el => {
    const top = el.getBoundingClientRect().top;
    const row = [...el.querySelectorAll<HTMLElement>("[data-index]")].find(row => row.getBoundingClientRect().bottom > top);
    if (!row) throw new Error("No visible row");
    return { text: row.textContent?.slice(0, 100), index: row.dataset.index, top: row.getBoundingClientRect().top - top };
  });
}
try {
  for (const scenario of scenarios.filter(s => !process.env.SCENARIOS || process.env.SCENARIOS.split(",").includes(s.name))) {
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    page.setDefaultTimeout(10000);
    const check = async (name: string, action: () => Promise<void>) => {
      try { await action(); results.push({ scenario: scenario.name, check: name }); }
      catch (error) {
        const message = error instanceof Error ? error.message.split("\n")[0] : String(error);
        results.push({ scenario: scenario.name, check: name, error: message });
        failures.push(`${scenario.name}: ${name}: ${message}`);
        console.error(failures.at(-1));
        console.error(await page.locator(selector).evaluate(el => ({ top: el.scrollTop, gap: el.scrollHeight - el.scrollTop - el.clientHeight, rows: [...el.querySelectorAll<HTMLElement>("[data-index]")].map(r => ({ index: r.dataset.index, top: r.getBoundingClientRect().top - el.getBoundingClientRect().top })) })));
        await page.screenshot({ path: `${output}/${engine}-${scenario.name}-failure.png` });
      } finally {
        if ((await page.getByTestId("audit-stream").textContent())?.includes("on")) await page.getByTestId("audit-stream").click();
      }
    };
    await page.route("https://scroll-fixture.invalid/image.svg", async route => {
      await new Promise(resolve => setTimeout(resolve, 800));
      await route.fulfill({ contentType: "image/svg+xml", body: '<svg xmlns="http://www.w3.org/2000/svg" width="600" height="1200"><rect width="600" height="1200" fill="#345"/></svg>' });
    });
    await page.goto(`${origin}/?${scenario.query}#scroll-fixture`);
    await page.locator(selector).waitFor({ timeout: 60000 });
    await check("opens at the end", async () => { await bottom(page); await page.waitForTimeout(400); await bottom(page); });
    if (scenario.name === "late-layout") await check("delayed images and diagrams keep the end visible", async () => {
      await page.waitForFunction(() => {
        const img = document.querySelector<HTMLImageElement>('img[alt="Delayed audit image"]');
        return img?.complete && img.naturalHeight > 0 && document.querySelector("[data-mermaid] svg");
      });
      await bottom(page);
    });
    await check("Top and Latest reach both ends", async () => {
      const top = page.getByRole("button", { name: "Top", exact: true });
      if (await top.count()) {
        await top.click();
        await page.waitForFunction(selector => document.querySelector(selector)?.scrollTop === 0, selector);
        await page.getByRole("button", { name: "Latest", exact: true }).click();
        await bottom(page);
      }
    });
    if (scenario.name === "partial-search") await check("a global search result survives the history remount", async () => {
      await page.getByTestId("audit-global-find").click();
      await page.getByTestId("audit-history").click();
      await page.waitForFunction(({ selector, query }) => {
        const s = document.querySelector(selector);
        if (!s) return false;
        const viewport = s.getBoundingClientRect();
        return [...s.querySelectorAll("p")].some(p => p.textContent === query && p.getBoundingClientRect().top >= viewport.top && p.getBoundingClientRect().bottom <= viewport.bottom);
      }, { selector, query: `END-OF-ANSWER-${Math.floor(scenario.count / 2)}.` });
      await latest(page);
    });
    if (scenario.name === "partial-end") await check("loading earlier messages keeps a reader at the end", async () => {
      await page.getByTestId("audit-history").click();
      await bottom(page);
      await page.waitForTimeout(500);
      await bottom(page);
    });
    if (scenario.name === "short") { await page.close(); continue; }
    await check("native scrolling and keyboard navigation cross virtual boundaries", async () => {
      const s = page.locator(selector);
      await s.hover(); await page.mouse.wheel(0, -600);
      await page.waitForTimeout(250);
      for (const fraction of [0, 0.1, 0.5, 0.9, 0.4, 0]) {
        await s.evaluate((el, fraction) => { el.scrollTop = (el.scrollHeight - el.clientHeight) * fraction; }, fraction);
        await page.waitForTimeout(120);
        const state = await anchor(page);
        assert(state.text, "The viewport became blank");
        assert(await s.locator("[data-index]").count() < 100, "Messages lost virtualisation");
      }
      await s.focus();
      await page.keyboard.press("Meta+ArrowDown"); await bottom(page);
      await page.keyboard.press("Meta+ArrowUp");
      await page.waitForFunction(selector => document.querySelector(selector)?.scrollTop === 0, selector);
      await page.keyboard.press("Meta+ArrowDown"); await bottom(page);
    });
    await check("user scroll stays in history during streaming", async () => {
      const s = page.locator(selector);
      await s.hover(); await page.mouse.wheel(0, -600);
      await page.waitForTimeout(250);
      await s.evaluate(el => { el.scrollTop = el.scrollHeight / 2; });
      await page.waitForTimeout(700);
      const before = await anchor(page);
      await page.getByTestId("audit-stream").click();
      await page.waitForTimeout(1200);
      const after = await anchor(page);
      assert.equal(after.index, before.index, "Streaming changed the visible row");
      assert(Math.abs(after.top - before.top) <= 2, `The visible message moved ${after.top - before.top}px`);
      await page.getByTestId("audit-stream").click();
    });
    await check("clicking content at the end does not stop following", async () => {
      await latest(page);
      await page.locator(selector).click({ position: { x: 30, y: 30 } });
      await page.getByTestId("audit-stream").click();
      await page.waitForTimeout(500); await bottom(page);
      await page.getByTestId("audit-stream").click();
    });
    await check("horizontal, pinch and consumed wheel gestures keep following", async () => {
      await latest(page);
      for (const wheel of [{ deltaX: 100, deltaY: -1 }, { deltaY: -100, ctrlKey: true }, { deltaY: -100 }]) {
        // WebKit performs the default action even for dispatched WheelEvents. Model an inner
        // surface consuming the gesture explicitly, while still delivering it to our listener.
        await page.locator(selector).evaluate(el => el.addEventListener("wheel", event => event.preventDefault(), { once: true }));
        await page.locator(selector).dispatchEvent("wheel", { ...wheel, cancelable: true });
        await page.waitForTimeout(100);
        await page.getByTestId("audit-stream").click();
        await page.waitForTimeout(300); await bottom(page);
        await page.getByTestId("audit-stream").click();
      }
    });
    await check("scrolling without a wheel stops following", async () => {
      await latest(page);
      // A scrollbar drag has pointer input, but no wheel. Give it the same initial pointerdown.
      const box = await page.locator(selector).boundingBox();
      assert(box);
      await page.locator(selector).dispatchEvent("pointerdown", { clientX: box.x + box.width - 5, clientY: box.y + 30 });
      await page.locator(selector).evaluate(el => { el.scrollTop -= 600; });
      await page.waitForTimeout(400);
      const before = await anchor(page);
      await page.getByTestId("audit-stream").click();
      await page.waitForTimeout(600);
      const after = await anchor(page);
      assert.equal(after.index, before.index);
      assert(Math.abs(after.top - before.top) <= 2, "A non-wheel scroll was overridden");
      await page.getByTestId("audit-stream").click();
    });
    await check("Latest follows a growing last message", async () => {
      await latest(page);
      await page.getByTestId("audit-stream").click();
      await page.waitForTimeout(1200); await bottom(page);
      await page.getByTestId("audit-stream").click();
      await bottom(page);
    });
    await check("resize and new rows keep the end reachable", async () => {
      await page.getByTestId("audit-resize").click(); await bottom(page);
      await page.getByTestId("audit-append").click(); await bottom(page);
      await page.getByTestId("audit-resize").click(); await bottom(page);
    });
    if (scenario.count > 1) {
      await check("a middle timeline mark reaches an unmeasured message", async () => {
        const rail = page.getByTestId("timeline-rail");
        await rail.evaluate(el => el.addEventListener("click", event => {
          const button = event.target instanceof Element ? event.target.closest("button") : null;
          if (button) el.setAttribute("data-audit-clicked", button.getAttribute("aria-label") ?? "");
        }, { once: true, capture: true }));
        const ticks = page.getByTestId("timeline-tick");
        await ticks.nth(Math.floor(await ticks.count() / 2)).click();
        const label = await rail.getAttribute("data-audit-clicked");
        const prompt = label?.match(/Prompt (\d+)\./)?.[1];
        assert(prompt !== undefined, "The pointer must hit a message mark");
        await page.waitForFunction(({ selector, prompt }) => {
          const s = document.querySelector(selector);
          if (!s) return false;
          const row = [...s.querySelectorAll('[data-kind="user"]')].find(row => row.textContent?.startsWith(`Prompt ${prompt}.`));
          if (!row) return false;
          const viewport = s.getBoundingClientRect();
          const box = row.getBoundingClientRect();
          return box.top >= viewport.top - 3 && box.top < viewport.bottom;
        }, { selector, prompt });
      });
      await check("a timeline tick reveals its user message", async () => {
        await page.getByTestId("timeline-tick").last().click();
        await page.waitForFunction(({ selector, prompt }) => {
          const s = document.querySelector(selector);
          const row = [...(s?.querySelectorAll('[data-kind="user"]') ?? [])].find(row => row.textContent?.startsWith(prompt));
          if (!s || !row) return false;
          const viewport = s.getBoundingClientRect();
          const box = row.getBoundingClientRect();
          return box.top >= viewport.top - 3 && box.top < viewport.bottom && box.bottom > viewport.top;
        }, { selector, prompt: `Prompt ${scenario.count - 1}.` });
      });
      await check("the wheel over the rail stops following live output", async () => {
        await latest(page);
        await page.getByTestId("audit-stream").click();
        await page.getByTestId("timeline-tick").last().hover();
        await page.mouse.wheel(0, -600);
        await page.waitForTimeout(400);
        const before = await anchor(page);
        await page.waitForTimeout(700);
        const after = await anchor(page);
        assert.equal(after.index, before.index);
        assert(Math.abs(after.top - before.top) <= 2, `Live output overrode the wheel on the rail: ${JSON.stringify({before, after})}`);
        await page.getByTestId("audit-stream").click();
      });
      await check("a timeline jump is superseded by Latest", async () => {
        const ticks = page.getByTestId("timeline-tick");
        const box = await ticks.first().boundingBox();
        assert(box);
        await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
        await page.getByRole("button", { name: "Latest", exact: true }).click();
        await page.waitForTimeout(800); await bottom(page);
      });
      await check("find reaches a match deep inside a message", async () => {
        await page.keyboard.press("Meta+f");
        const query = `END-OF-ANSWER-${(scenario.name === "partial" ? scenario.count - 2 : scenario.name === "partial-tall" ? scenario.count - 1 : scenario.name === "last-tall" ? scenario.count - 1 : scenario.name === "first-tall" ? 0 : Math.floor(scenario.count / 2))}.`;
        await page.getByTestId("chat-find-input").fill(query);
        await page.waitForFunction(({ selector, query }) => {
          const s = document.querySelector(selector);
          if (!s) return false;
          const viewport = s.getBoundingClientRect();
          for (const p of s.querySelectorAll("p")) {
            if (p.textContent?.includes(query)) {
              const box = p.getBoundingClientRect();
              return box.top >= viewport.top + 40 && box.bottom <= viewport.bottom;
            }
          }
          return false;
        }, { selector, query });
        await page.getByTestId("chat-find-close").click();
      });
    }
    if (["partial", "partial-tall"].includes(scenario.name)) {
      await check("loading earlier messages preserves the reader's message and offset", async () => {
        await page.locator(selector).hover(); await page.mouse.wheel(0, -500);
        await page.waitForTimeout(700);
        const before = await anchor(page);
        await page.getByTestId("audit-history").click();
        await page.waitForTimeout(1200);
        const after = await anchor(page);
        assert.equal(after.text, before.text);
        assert(Math.abs(after.top - before.top) <= 2, `History moved the reader by ${after.top - before.top}px`);
      });
    }
    await check("Latest cancels a global search jump through later streamed updates", async () => {
      await page.getByTestId("audit-global-find").click();
      await page.waitForTimeout(20);
      await latest(page);
      await page.getByTestId("audit-stream").click();
      await page.waitForTimeout(500); await bottom(page);
      await page.getByTestId("audit-stream").click();
    });
    await check("shrinking a message leaves the end reachable", async () => {
      await latest(page);
      await page.getByTestId("audit-shrink").click();
      await bottom(page);
    });
    if (["last-tall", "many", "partial"].includes(scenario.name)) await page.screenshot({ path: `${output}/${engine}-${scenario.name}.png` });
    console.log(`${engine}: ${scenario.name}: ${results.filter(r => r.scenario === scenario.name && !r.error).length} passed; ${results.filter(r => r.scenario === scenario.name && r.error).length} failed`);
    await page.close();
  }
} finally {
  await browser.close();
  await Bun.write(`${output}/${engine}.json`, JSON.stringify(results, null, 2));
}
if (failures.length) { console.error(failures.join("\n")); process.exit(1); }
console.log(`${results.length} scrolling checks passed in ${engine}`);
