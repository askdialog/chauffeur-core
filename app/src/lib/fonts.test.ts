import { describe, expect, test } from "bun:test";
import { DEFAULT_MONO, DEFAULT_SANS, fontStack, terminalFont } from "./fonts";

describe("fonts", () => {
  test("the user's font comes first, the app's own after it", () => {
    expect(fontStack("", DEFAULT_MONO)).toBe(DEFAULT_MONO);
    expect(fontStack("Menlo", "monospace")).toBe("Menlo, monospace");
    expect(fontStack(" JetBrains Mono ", "monospace")).toBe('"JetBrains Mono", monospace');
    expect(fontStack('"Fira Code", Menlo', "monospace")).toBe('"Fira Code", Menlo, monospace');
  });

  test("without Apple's fonts, the desktop's generic families decide, a picked font missing too", () => {
    const apple = ["-apple-system", "BlinkMacSystemFont", '"SF Pro Text"', '"SF Mono"', "ui-monospace", "Menlo", "Monaco"];
    const left = (stack: string) => stack.split(",").map((name) => name.trim()).filter((name) => !apple.includes(name));
    expect(left(DEFAULT_SANS)).toEqual(["system-ui", "sans-serif"]);
    expect(left(DEFAULT_MONO)).toEqual(["monospace"]);
    expect(left(fontStack("Fira Code", DEFAULT_MONO))).toEqual(['"Fira Code"', "monospace"]);
  });

  test("the terminal keeps its own until the user picks", () => {
    expect(terminalFont(null)).toEqual({ fontFamily: DEFAULT_MONO, fontSize: 12 });
    const picked = { interface_font: "", code_font: "", code_size: 0, terminal_font: "Fira Code", terminal_size: 14, terminal_theme: "", theme_interface: false, palette: null, palette_error: null };
    expect(terminalFont(picked)).toEqual({ fontFamily: `"Fira Code", ${DEFAULT_MONO}`, fontSize: 14 });
  });
});
