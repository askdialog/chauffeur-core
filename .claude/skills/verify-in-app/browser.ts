#!/usr/bin/env bun
// A headless Chrome of one's own, driven one command at a time:
//   browser.ts <dir> open | go <url> | click <sel> | type <sel> <text> | press <key>
//                        | wait <sel|ms> | text [sel] | shot <file.png> [sel] | run <js|-> | close
// `open` starts Chrome with its profile in <dir> and a DevTools port of its own; every other
// command connects to that Chrome, acts on its first tab and disconnects, leaving it running.
// Nothing is shared with another <dir>, nor with the Playwright MCP server's browser.
import { spawn } from "node:child_process";
import { existsSync, readFileSync, rmSync, mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { chromium, type Browser, type Page } from "playwright-core@1.64.1";

const [dirArg, cmd, ...args] = process.argv.slice(2);
if (!dirArg || !cmd) {
  console.error("usage: browser.ts <dir> open|go|click|type|press|wait|text|shot|run|close …");
  process.exit(2);
}
const dir = resolve(dirArg);
const profile = `${dir}/profile`;
const portFile = `${profile}/DevToolsActivePort`;
const pidFile = `${dir}/pid`;

function chromePath(): string {
  const candidates = [
    process.env.CHROME,
    chromium.executablePath(), // Playwright's Chromium, if `bunx playwright@1.64.1 install chromium` ran
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    Bun.which("google-chrome") ?? undefined,
    Bun.which("chromium") ?? undefined,
  ];
  const found = candidates.find((p) => p && existsSync(p));
  if (!found) throw new Error("no Chrome found: install Google Chrome, or run `bunx playwright@1.64.1 install chromium`, or set CHROME");
  return found;
}

function running(): boolean {
  if (!existsSync(pidFile)) return false;
  try {
    process.kill(Number(readFileSync(pidFile, "utf8")), 0);
    return existsSync(portFile);
  } catch {
    return false;
  }
}

async function open() {
  if (running()) return console.log(`already open: ${dir}`);
  rmSync(profile, { recursive: true, force: true });
  mkdirSync(profile, { recursive: true });
  const child = spawn(
    chromePath(),
    [
      "--headless=new",
      "--remote-debugging-port=0", // Chrome picks a free port and writes it to DevToolsActivePort
      `--user-data-dir=${profile}`,
      "--window-size=1440,900",
      "--no-first-run",
      "--no-default-browser-check",
      "--use-mock-keychain",
      "--password-store=basic",
      "--disable-sync",
      "--disable-background-networking",
      "--disable-component-update",
      "about:blank",
    ],
    { detached: true, stdio: "ignore" },
  );
  child.unref();
  writeFileSync(pidFile, String(child.pid));
  for (let i = 0; i < 150 && !existsSync(portFile); i++) await Bun.sleep(100);
  if (!existsSync(portFile)) throw new Error("Chrome did not start");
  console.log(`open: ${dir} (pid ${child.pid}, port ${readFileSync(portFile, "utf8").split("\n")[0]})`);
}

async function connect(): Promise<{ browser: Browser; page: Page }> {
  if (!running()) throw new Error(`no browser open in ${dir}: run \`open\` first`);
  const port = readFileSync(portFile, "utf8").split("\n")[0];
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  const context = browser.contexts()[0];
  const page = context.pages()[0] ?? (await context.newPage());
  page.setDefaultTimeout(10_000);
  return { browser, page };
}

async function close() {
  if (running()) {
    const { browser } = await connect();
    await browser.newBrowserCDPSession().then((s) => s.send("Browser.close")).catch(() => {});
    const pid = Number(readFileSync(pidFile, "utf8"));
    for (let i = 0; i < 50; i++) {
      try { process.kill(pid, 0); await Bun.sleep(100); } catch { break; }
    }
    try { process.kill(pid, "SIGKILL"); } catch {}
  }
  rmSync(profile, { recursive: true, force: true });
  rmSync(pidFile, { force: true });
  console.log(`closed: ${dir}`);
}

async function act(page: Page): Promise<unknown> {
  switch (cmd) {
    case "go":
      await page.goto(args[0]);
      return page.url();
    case "click":
      return page.locator(args[0]).first().click({ button: args[1] === "right" ? "right" : "left" });
    case "type":
      return page.locator(args[0]).first().fill(args[1] ?? "");
    case "press":
      return page.keyboard.press(args[0]);
    case "wait":
      return /^\d+$/.test(args[0]) ? page.waitForTimeout(Number(args[0])) : page.locator(args[0]).first().waitFor();
    case "text":
      return page.locator(args[0] ?? "body").first().innerText();
    case "shot": {
      const file = resolve(args[0]);
      await (args[1] ? page.locator(args[1]).first().screenshot({ path: file }) : page.screenshot({ path: file }));
      return file;
    }
    case "run": {
      // The body of an async function of `page`, e.g. `return await page.title()`.
      const code = args[0] === "-" || !args[0] ? await Bun.stdin.text() : args[0];
      return new Function("page", `return (async () => { ${code} })()`)(page);
    }
    default:
      throw new Error(`unknown command: ${cmd}`);
  }
}

try {
  if (cmd === "open") await open();
  else if (cmd === "close") await close();
  else {
    const { page } = await connect();
    const out = await act(page);
    if (out !== undefined) console.log(typeof out === "string" ? out : JSON.stringify(out, null, 2));
  }
  process.exit(0); // leave Chrome running; dropping the connection does not close it
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
}
