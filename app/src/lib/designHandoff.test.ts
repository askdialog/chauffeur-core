import { describe, expect, test } from "bun:test";
import type { CanvasScreen, ChatInfo, JsonValue, OpenApp } from "@chauffeur/protocol";
import type { ShownSection } from "./canvas";
import type { ToolItem } from "./chat";
import {
  appendDraft,
  browserUrl,
  chatsToSendTo,
  designApp,
  designFile,
  handoffMessage,
  pageScreens,
  publishedScreens,
  publishedUrl,
  screenPath,
  screensToSend,
  sourceChatOf,
} from "./designHandoff";

const screen = (file: string, more: Partial<CanvasScreen> = {}): CanvasScreen => ({
  file,
  width: 390,
  height: 844,
  url: `http://127.0.0.1:5000/s/w/d/${file}`,
  modified_ms: 1,
  ...more,
});

const sections: ShownSection[] = [
  { title: "Cart", unsorted: false, screens: [screen("cart.html", { label: "Full cart" }), screen("cart-empty.html", { label: "Empty cart" })] },
  { title: "Payment", unsorted: false, screens: [screen("pay.html", { label: "Card" })] },
  { title: "Unsorted", unsorted: true, screens: [screen("draft.html")] },
];

const chat = (id: string, more: Partial<ChatInfo> = {}): ChatInfo => ({
  id,
  kind: "chat",
  workspace_id: "w",
  agent: "claude",
  mcp_servers: [],
  model: null,
  effort: null,
  fast: false,
  permission_mode: "accept_edits",
  title: null,
  status: "idle",
  agent_session_id: null,
  created_at: "2026-10-18T10:00:00Z",
  running_tasks: 0,
  closed: false,
  last_activity_at: null,
  ...more,
});

describe("the screens sent", () => {
  test("every screen of the page when none is selected", () => {
    expect(screensToSend(sections, []).map((s) => [s.title, s.screens.map((x) => x.file)])).toEqual([
      ["Cart", ["cart.html", "cart-empty.html"]],
      ["Payment", ["pay.html"]],
      ["Unsorted", ["draft.html"]],
    ]);
  });

  test("only the selected ones, by section, the sections left empty out", () => {
    expect(screensToSend(sections, ["pay.html", "cart-empty.html"]).map((s) => [s.title, s.screens.map((x) => x.file)])).toEqual([
      ["Cart", ["cart-empty.html"]],
      ["Payment", ["pay.html"]],
    ]);
  });

  test("a selection of another page counts as none", () => {
    expect(screensToSend(sections, ["profile.html"]).flatMap((s) => s.screens)).toHaveLength(4);
  });

  test("missing files are left out", () => {
    const withMissing: ShownSection[] = [
      { title: "Cart", unsorted: false, screens: [screen("cart.html"), screen("gone.html", { url: undefined, modified_ms: undefined })] },
      { title: "Gone", unsorted: false, screens: [screen("lost.html", { url: undefined, modified_ms: undefined })] },
    ];
    expect(screensToSend(withMissing, []).map((s) => [s.title, s.screens.map((x) => x.file)])).toEqual([["Cart", ["cart.html"]]]);
    // A file the daemon does not serve is still there.
    expect(screensToSend([{ title: "A", unsorted: false, screens: [screen("a.html", { url: undefined })] }], [])).toHaveLength(1);
  });
});

describe("the message", () => {
  test("lists the sections, the labels (else the file) and the paths, and asks to implement them", () => {
    expect(handoffMessage("Checkout", "d1", screensToSend(sections, []))).toBe(
      [
        "Implement these screens from the design chat “Checkout”:",
        "",
        "Cart:",
        "- Full cart: `.chauffeur/designs/d1/cart.html`",
        "- Empty cart: `.chauffeur/designs/d1/cart-empty.html`",
        "",
        "Payment:",
        "- Card: `.chauffeur/designs/d1/pay.html`",
        "",
        "Unsorted:",
        "- draft.html: `.chauffeur/designs/d1/draft.html`",
        "",
        "Open each file to see the screen, then build them with the repo's own components, matching the design as closely as they allow.",
      ].join("\n"),
    );
  });

  test("speaks of one screen in the singular", () => {
    const one = handoffMessage("Checkout", "d1", screensToSend(sections, ["pay.html"]));
    expect(one.startsWith("Implement this screen from the design chat “Checkout”:")).toBe(true);
    expect(one).toContain("then build it with");
  });

  test("paths are under the workspace folder", () => {
    expect(screenPath("d1", "./screens/a.html")).toBe(".chauffeur/designs/d1/screens/a.html");
  });

  test("goes below a draft already written, or alone", () => {
    expect(appendDraft("", "M")).toBe("M");
    expect(appendDraft("  \n", "M")).toBe("M");
    expect(appendDraft("Also fix the header.\n", "M")).toBe("Also fix the header.\n\nM");
  });
});

describe("the chat it goes to", () => {
  const source = chat("s", { title: "Ticket" });
  const design = chat("d", { kind: "design", source_chat: "s" });

  test("the source chat, even closed", () => {
    expect(sourceChatOf(design, [source, design])?.id).toBe("s");
    expect(sourceChatOf(design, [{ ...source, closed: true }, design])?.id).toBe("s");
  });

  test("none for a blank design chat, or once the source is deleted", () => {
    expect(sourceChatOf(chat("d", { kind: "design" }), [source])).toBeNull();
    expect(sourceChatOf(design, [design])).toBeNull();
  });

  test("Send to a chat… lists the other open chats that write code", () => {
    const all = [
      source,
      design,
      chat("other-design", { kind: "design" }),
      chat("closed", { closed: true }),
      chat("elsewhere", { workspace_id: "w2" }),
      chat("t"),
    ];
    expect(chatsToSendTo(design, all).map((c) => c.id)).toEqual(["s", "t"]);
  });
});

describe("Open on claude.ai", () => {
  const dir = "/ws/.chauffeur/designs/d";
  let n = 0;
  const artifact = (input: JsonValue, output: string | null, isError = false): ToolItem => ({
    kind: "tool",
    key: `a${++n}`,
    ts: "",
    depth: 0,
    toolUseId: `t${n}`,
    name: "Artifact",
    input,
    parentToolUseId: null,
    summary: "",
    result: output == null ? null : { output, isError, attachments: [] },
  });
  const published = (url: string) => JSON.stringify({ url, path: "/artifact/x", title: "Cart" });

  test("maps each published file to its last URL", () => {
    const items = [
      artifact({ file_path: `${dir}/cart.html` }, published("https://claude.ai/artifact/1")),
      artifact({ action: "publish", file_path: "pay.html" }, published("https://claude.ai/artifact/2")),
      artifact({ file_path: `${dir}/cart.html` }, published("https://claude.ai/artifact/1b")),
    ];
    expect(publishedScreens(items, dir)).toEqual({
      "cart.html": "https://claude.ai/artifact/1b",
      "pay.html": "https://claude.ai/artifact/2",
    });
  });

  test("ignores other actions, errors, calls still running, other tools and files outside the folder", () => {
    const items: ToolItem[] = [
      artifact({ action: "list" }, JSON.stringify({ artifacts: [{ title: "x", url: "https://claude.ai/artifact/9" }] })),
      artifact({ action: "read", url: "https://claude.ai/artifact/1" }, JSON.stringify({ read: { url: "https://claude.ai/artifact/1" } })),
      artifact({ action: "pin", url: "https://claude.ai/artifact/1", file_path: "cart.html" }, published("https://claude.ai/artifact/1")),
      artifact({ file_path: "cart.html" }, "Permission denied", true),
      artifact({ file_path: "pay.html" }, published("https://claude.ai/artifact/2"), true),
      artifact({ file_path: "draft.html" }, null),
      artifact({ file_path: "/elsewhere/a.html" }, published("https://claude.ai/artifact/3")),
      artifact({ file_path: "../a.html" }, published("https://claude.ai/artifact/4")),
      { ...artifact({ file_path: "a.html" }, published("https://claude.ai/artifact/5")), name: "Write" },
    ];
    expect(publishedScreens(items, dir)).toEqual({});
  });

  test("a URL only from a publish's JSON, over https", () => {
    expect(publishedUrl(published("https://claude.ai/artifact/1"))).toBe("https://claude.ai/artifact/1");
    expect(publishedUrl("Published at https://claude.ai/artifact/1")).toBeNull();
    expect(publishedUrl(JSON.stringify({ url: "javascript:alert(1)" }))).toBeNull();
    expect(publishedUrl(JSON.stringify(["https://claude.ai/artifact/1"]))).toBeNull();
  });

  test("a file's path relative to the design folder", () => {
    expect(designFile(`${dir}/screens/a.html`, dir)).toBe("screens/a.html");
    expect(designFile(`${dir}/a.html`, `${dir}/`)).toBe("a.html");
    expect(designFile("./a.html", dir)).toBe("a.html");
    expect(designFile("a.html", null)).toBe("a.html");
    expect(designFile(`${dir}/a.html`, null)).toBeNull();
    expect(designFile(`${dir}-other/a.html`, dir)).toBeNull();
  });
});

describe("Open › Browser", () => {
  const screens = pageScreens([
    { title: "A", unsorted: false, screens: [screen("gone.html", { url: undefined }), screen("a.html"), screen("b.html")] },
  ]);

  test("the first selected screen with a URL", () => {
    expect(browserUrl(screens, ["b.html"])).toBe("http://127.0.0.1:5000/s/w/d/b.html");
  });

  test("else the page's first screen with a URL", () => {
    expect(browserUrl(screens, [])).toBe("http://127.0.0.1:5000/s/w/d/a.html");
    expect(browserUrl(screens, ["gone.html"])).toBe("http://127.0.0.1:5000/s/w/d/a.html");
  });

  test("none without a screen served", () => {
    expect(browserUrl([], [])).toBeNull();
    expect(browserUrl([{ file: "a.html", url: null }], ["a.html"])).toBeNull();
  });
});

describe("the app the Open button names for a design folder", () => {
  const app = (id: string, kind: OpenApp["kind"], is_default = false): OpenApp => ({ id, name: id, kind, is_default });

  test("the default, unless it is a terminal", () => {
    expect(designApp([app("cursor", "editor"), app("zed", "editor", true), app("finder", "files")])?.id).toBe("zed");
    expect(designApp([app("ghostty", "terminal", true), app("cursor", "editor"), app("finder", "files")])?.id).toBe("cursor");
    expect(designApp([app("ghostty", "terminal", true), app("finder", "files")])?.id).toBe("finder");
    expect(designApp([app("ghostty", "terminal", true)])).toBeNull();
  });
});
