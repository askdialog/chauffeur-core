import { describe, expect, test } from "bun:test";
import type { Palette } from "@chauffeur/protocol";
import { applyPalette, contrast, DEFAULT_TERMINAL_THEME, interfaceHint, interfaceVariables, isLight, paletteVariables, terminalTheme } from "./palette";

const DRACULA: Palette = {
  background: "#282a36",
  foreground: "#f8f8f2",
  cursor: null,
  selection: "#44475a",
  ansi: ["#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2", "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff"],
};

// As wallust writes ~/.cache/wal/colors.json from a wallpaper: a warm dark background, a pale blue.
const WALLUST: Palette = {
  background: "#1F1D20",
  foreground: "#FFEFD0",
  cursor: "#F8B491",
  selection: null,
  ansi: ["#454347", "#FA3C86", "#F17A52", "#14C2DE", "#C8B9D5", "#A7D4B0", "#FFD07A", "#F6DFB4", "#AD9C7E", "#FA3C86", "#F17A52", "#14C2DE", "#C8B9D5", "#A7D4B0", "#FFD07A", "#F6DFB4"],
};

const SOLARIZED_LIGHT: Palette = { ...DRACULA, background: "#fdf6e3", foreground: "#657b83" };

describe("terminal theme", () => {
  test("the app's own colours without a theme", () => {
    expect(terminalTheme(null)).toBe(DEFAULT_TERMINAL_THEME);
  });

  test("a theme colours the terminal, its 16 ANSI colours by name", () => {
    const theme = terminalTheme(DRACULA);
    expect(theme.background).toBe("#282a36");
    expect(theme.cursor).toBe("#f8f8f2");
    expect(theme.selectionBackground).toBe("#44475a");
    expect(theme.red).toBe("#ff5555");
    expect(theme.brightWhite).toBe("#ffffff");
    expect(terminalTheme({ ...DRACULA, selection: null }).selectionBackground).toBe("#f8f8f240");
  });

  test("the code takes the theme's background and its ANSI colours", () => {
    const vars = paletteVariables(DRACULA);
    expect(vars["--code-bg"]).toBe("#282a36");
    expect(vars["--hl-keyword"]).toBe("#ff79c6");
    expect(vars["--hl-string"]).toBe("#50fa7b");
    expect(vars["--hl-comment"]).toBe("#6272a4");
  });
});

describe("interface colours from the theme", () => {
  test("contrast follows WCAG", () => {
    expect(contrast("#000000", "#ffffff")).toBeCloseTo(21);
    expect(contrast("#ffffff", "#ffffff")).toBe(1);
    expect(isLight("#fdf6e3")).toBe(true);
    expect(isLight(WALLUST.background)).toBe(false);
  });

  test("a wallust palette colours the background, surfaces, text and accent, all readable", () => {
    const vars = interfaceVariables(WALLUST);
    if (!vars) throw new Error("a dark theme colours the interface");
    expect(vars["--color-bg"]).toBe("#1F1D20");
    expect(vars["--color-fg"]).toBe("#ffefd0");
    for (const surface of ["--color-sidebar", "--color-panel", "--color-raised", "--color-hover", "--color-line"]) {
      expect(contrast(vars[surface], "#000000")).toBeGreaterThan(contrast(vars["--color-bg"], "#000000"));
    }
    for (const on of [vars["--color-bg"], vars["--color-hover"]]) {
      expect(contrast(vars["--color-fg"], on)).toBeGreaterThanOrEqual(4.5);
      expect(contrast(vars["--color-muted"], on)).toBeGreaterThanOrEqual(3);
      expect(contrast(vars["--color-faint"], on)).toBeGreaterThanOrEqual(3);
      expect(contrast(vars["--color-accent"], on)).toBeGreaterThanOrEqual(3);
    }
    // Its pale color4 is darkened so that a button's white label reads on it.
    expect(contrast("#ffffff", vars["--color-accent"])).toBeGreaterThanOrEqual(3);
    expect(contrast("#ffffff", vars["--color-accent-strong"])).toBeGreaterThanOrEqual(4.5);
  });

  test("the colours that carry a meaning stay the app's own", () => {
    const names = Object.keys(interfaceVariables(WALLUST) ?? {});
    for (const semantic of ["--color-danger", "--color-success", "--color-attention", "--color-working", "--hl-deletion"]) {
      expect(names).not.toContain(semantic);
    }
  });

  test("text too dim on the theme's background is lightened until it reads", () => {
    const vars = interfaceVariables({ ...DRACULA, background: "#202020", foreground: "#505050" });
    if (!vars) throw new Error("a dark theme colours the interface");
    expect(contrast("#505050", "#202020")).toBeLessThan(4.5);
    expect(contrast(vars["--color-fg"], vars["--color-hover"])).toBeGreaterThanOrEqual(4.5);
    expect(contrast(vars["--color-muted"], vars["--color-hover"])).toBeGreaterThanOrEqual(3);
  });

  test("the switch's hint says what it needs first: a theme file, a dark one", () => {
    expect(interfaceHint("", null)).toContain("Choose a terminal theme file above first");
    expect(interfaceHint("~/.cache/wal/colors.json", SOLARIZED_LIGHT)).toContain("this theme is light");
    expect(interfaceHint("~/.cache/wal/colors.json", WALLUST)).toContain("accent");
  });

  test("a light theme leaves the interface its own colours", () => {
    expect(interfaceVariables(SOLARIZED_LIGHT)).toBeNull();
  });

  test("the interface follows the theme only while asked, and goes back to its own colours", () => {
    const style = new Map<string, string>();
    // Only the two methods applyPalette calls: bun's tests have no DOM.
    const root = {
      style: { setProperty: (name: string, value: string) => style.set(name, value), removeProperty: (name: string) => style.delete(name) },
    } as unknown as HTMLElement;
    applyPalette(WALLUST, false, root);
    expect(style.get("--code-bg")).toBe("#1F1D20");
    expect(style.has("--color-bg")).toBe(false);
    applyPalette(WALLUST, true, root);
    expect(style.get("--color-bg")).toBe("#1F1D20");
    applyPalette({ ...WALLUST, background: "#101418" }, true, root);
    expect(style.get("--color-bg")).toBe("#101418");
    applyPalette(SOLARIZED_LIGHT, true, root);
    expect(style.has("--color-bg")).toBe(false);
    expect(style.get("--code-bg")).toBe("#fdf6e3");
    applyPalette(WALLUST, true, root);
    applyPalette(WALLUST, false, root);
    expect(style.has("--color-bg")).toBe(false);
    applyPalette(null, false, root);
    expect(style.size).toBe(0);
  });
});
