import { describe, expect, test } from "bun:test";
import type { OpenApp, TerminalInfo } from "@chauffeur/protocol";
import { terminalApp, terminalLabel, terminalMenu } from "./terminalTabs";

const terminal = (id: string, over: Partial<TerminalInfo> = {}): TerminalInfo => ({
  id,
  workspace_id: "w",
  cwd: "/ws",
  title: "web",
  name: null,
  cols: 80,
  rows: 24,
  running: true,
  exit_code: null,
  command_running: false,
  command_finished_at: null,
  ...over,
});
const app = (id: string, kind: OpenApp["kind"], is_default = false): OpenApp => ({ id, name: id, kind, is_default });

describe("a terminal tab", () => {
  test("shows the name it was given, else its title, else its place", () => {
    expect(terminalLabel(terminal("a", { name: "dev server" }), 0)).toBe("dev server");
    expect(terminalLabel(terminal("a"), 0)).toBe("web");
    expect(terminalLabel(terminal("a", { title: "" }), 2)).toBe("Terminal 3");
  });

  test("opens in the default app when it is a terminal, else in the first terminal found", () => {
    expect(terminalApp([app("cursor", "editor", true), app("kitty", "terminal"), app("ghostty", "terminal")])?.id).toBe("kitty");
    expect(terminalApp([app("kitty", "terminal"), app("ghostty", "terminal", true)])?.id).toBe("ghostty");
    expect(terminalApp([app("cursor", "editor", true), app("files", "files")])).toBeNull();
  });
});

describe("a terminal tab's menu", () => {
  const items = (over: Partial<Parameters<typeof terminalMenu>[0]> = {}) =>
    terminalMenu({
      terminal: terminal("a"),
      terminals: [terminal("a"), terminal("b"), terminal("c")],
      apps: [app("cursor", "editor", true), app("kitty", "terminal")],
      cwd: "/ws/web/src",
      ...over,
    }).filter((item) => item !== "separator");

  test("renames, closes, closes the others, opens in the terminal app, copies the path", () => {
    expect(items().map((item) => item.label)).toEqual(["Rename", "Close", "Close the others", "Open in kitty", "Copy the path"]);
    expect(items().every((item) => !item.disabled)).toBe(true);
    expect(items().find((item) => item.action === "close-others")?.hint).toContain("the 2 others");
    expect(items().find((item) => item.action === "copy-path")?.hint).toBe("/ws/web/src");
  });

  test("a lone terminal has no others to close", () => {
    const lone = items({ terminals: [terminal("a")] }).find((item) => item.action === "close-others");
    expect(lone?.disabled).toBe(true);
    expect(lone?.hint).toBe("It is the only terminal");
  });

  test("without a terminal app, or before the apps are found, it cannot open one", () => {
    const none = items({ apps: [app("cursor", "editor", true)] }).find((item) => item.action === "open");
    expect(none).toMatchObject({ label: "Open in a terminal app", disabled: true, hint: "No terminal app found on this machine" });
    expect(items({ apps: null }).find((item) => item.action === "open")).toMatchObject({ disabled: true, hint: "Looking for apps…" });
    expect(items({ cwd: null }).find((item) => item.action === "copy-path")?.hint).toBe("Reading its folder…");
  });
});
