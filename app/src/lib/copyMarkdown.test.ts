import { afterEach, expect, test } from "bun:test";
import { copyMarkdown, headingsAsBold, markdownHtml } from "./copyMarkdown";

const navigatorDescriptor = Object.getOwnPropertyDescriptor(globalThis, "navigator");
const itemDescriptor = Object.getOwnPropertyDescriptor(globalThis, "ClipboardItem");
afterEach(() => {
  for (const [key, descriptor] of [["navigator", navigatorDescriptor], ["ClipboardItem", itemDescriptor]] as const) {
    if (descriptor) Object.defineProperty(globalThis, key, descriptor);
    else Reflect.deleteProperty(globalThis, key);
  }
});
function clipboard(write: (items: ClipboardItem[]) => Promise<void>, writeText: (text: string) => Promise<void>) {
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { clipboard: { write, writeText } } });
  Object.defineProperty(globalThis, "ClipboardItem", { configurable: true, value: class {
    constructor(public data: Record<string, Blob>) {}
    getType(type: string) { return Promise.resolve(this.data[type]); }
  } });
}
const source = "**bold** and *italic* [link](https://example.com)\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```ts\nconst x = '<b>';\n```";
const headed = "## Summary\n\nDone.\n\n```md\n# Not a heading\n```";
const headedPlain = "**Summary**\n\nDone.\n\n```md\n# Not a heading\n```";
test("copy offers intact Markdown and semantic HTML in the same clipboard item", async () => {
  let written = false;
  clipboard(async (items) => {
    written = true;
    expect(items).toHaveLength(1);
    const item = items[0];
    if (!item) throw new Error("no clipboard item");
    expect(await (await item.getType("text/plain")).text()).toBe(source);
    const html = await (await item.getType("text/html")).text();
    for (const fragment of ["<strong>bold</strong>", "<em>italic</em>", '<a href="https://example.com">link</a>', "<table>", "<td>1</td>", "&lt;b&gt;"]) expect(html).toContain(fragment);
  }, async () => { throw new Error("unexpected fallback"); });
  await copyMarkdown(source);
  expect(written).toBe(true);
});
test("the plain text has its headings as bold lines while the HTML keeps them as headings", async () => {
  clipboard(async (items) => {
    const item = items[0];
    if (!item) throw new Error("no clipboard item");
    expect(await (await item.getType("text/plain")).text()).toBe(headedPlain);
    expect(await (await item.getType("text/html")).text()).toContain("<h2>Summary</h2>");
  }, async () => { throw new Error("unexpected fallback"); });
  await copyMarkdown(headed);
});
// The fallback's text goes to the same places, Slack included, so it gets the same headings.
test("rich copying refused by a webview falls back to the same plain text", async () => {
  let plain = "";
  clipboard(async () => { throw new Error("not supported"); }, async (text) => { plain = text; });
  await copyMarkdown(headed);
  expect(plain).toBe(headedPlain);
});
test("a browser without rich copying still copies the same plain text", async () => {
  let plain = "";
  clipboard(async () => { throw new Error("unexpected rich copy"); }, async (text) => { plain = text; });
  Reflect.deleteProperty(globalThis, "ClipboardItem");
  await copyMarkdown(headed);
  expect(plain).toBe(headedPlain);
});
test("a failed plain fallback reaches the button's error handler", async () => {
  clipboard(async () => { throw new Error("rich denied"); }, async () => { throw new Error("plain denied"); });
  await expect(copyMarkdown(source)).rejects.toThrow("plain denied");
});
test("HTML copying escapes raw markup and removes unsafe link targets", () => {
  const html = markdownHtml('<script>alert(1)</script>\n\n[x](javascript:alert%281%29)');
  expect(html).not.toContain("<script>");
  expect(html).not.toContain('href="javascript:');
});
test("headings of every level become bold lines and keep the blank line under them", () => {
  const levels = [1, 2, 3, 4, 5, 6].map((level) => `${"#".repeat(level)} Level ${level}\n\nText ${level}.`).join("\n\n");
  const bold = [1, 2, 3, 4, 5, 6].map((level) => `**Level ${level}**\n\nText ${level}.`).join("\n\n");
  expect(headingsAsBold(levels)).toBe(bold);
});
test("a setext heading becomes a bold line without its underline", () => {
  expect(headingsAsBold("Title\n=====\n\nText")).toBe("**Title**\n\nText");
});
test("a heading keeps its text as written, without its closing hashes or doubled bold", () => {
  expect(headingsAsBold("## Run `cargo test` and [docs](https://x.dev) ##\n\nx")).toBe("**Run `cargo test` and [docs](https://x.dev)**\n\nx");
  expect(headingsAsBold("### **Already bold**\n\nx")).toBe("**Already bold**\n\nx");
  expect(headingsAsBold("### **Partly** bold\n\nx")).toBe("**Partly bold**\n\nx");
});
test("hash lines that are not headings are left alone", () => {
  const text = "```md\n# In a fence\n```\n\n    # Indented code\n\n#hashtag\n\n- item\n  \\# escaped";
  expect(headingsAsBold(text)).toBe(text);
});
test("everything but the headings is copied byte for byte", () => {
  const rest = "Intro with **bold**, *italic* and a  double space.\n\n- one\n- two\n\n> quote\n>\n> # Quoted heading\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n";
  expect(headingsAsBold(`${rest}## Next\n\n1. first\n2. second\n`)).toBe(`${rest.replace("# Quoted heading", "**Quoted heading**")}**Next**\n\n1. first\n2. second\n`);
  expect(headingsAsBold(source)).toBe(source);
});
