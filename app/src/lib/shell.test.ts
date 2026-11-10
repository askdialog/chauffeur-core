import { describe, expect, test } from "bun:test";
import { foldedOutput, inShellMode, messageText, shellCommand, shellEnding } from "./shell";

describe("shell commands in the composer", () => {
  test("a text starting with ! is a command, with or without a space after it", () => {
    expect(shellCommand("! git log -3")).toBe("git log -3");
    expect(shellCommand("!ls -la\n")).toBe("ls -la");
    expect(shellCommand("  !  npm test -- --watch=false ")).toBe("npm test -- --watch=false");
    expect(shellCommand("!echo a\necho b")).toBe("echo a\necho b");
  });

  test("anything else, a lone ! included, is a message to the agent", () => {
    expect(shellCommand("fix the ! in the title")).toBeNull();
    expect(shellCommand("!")).toBeNull();
    expect(shellCommand("!   ")).toBeNull();
    expect(shellCommand("")).toBeNull();
  });

  test("a message really starting with ! is written \\! and sent with its ! alone", () => {
    expect(shellCommand("\\!important: read this")).toBeNull();
    expect(inShellMode("\\!important")).toBe(false);
    expect(messageText("\\!important: read this")).toBe("!important: read this");
    expect(messageText("  \\! spaced")).toBe("  ! spaced");
    expect(messageText("a \\! inside stays")).toBe("a \\! inside stays");
  });

  test("the composer is in shell mode as soon as its text starts with !", () => {
    expect(inShellMode("!")).toBe(true);
    expect(inShellMode("  !git")).toBe(true);
    expect(inShellMode("git!")).toBe(false);
  });

  test("an item says how its command ended once it did", () => {
    expect(shellEnding({ running: true, exitCode: null, timedOut: false })).toBeNull();
    expect(shellEnding({ running: false, exitCode: 0, timedOut: false })).toBe("exit 0");
    expect(shellEnding({ running: false, exitCode: null, timedOut: true })).toBe("timed out");
    expect(shellEnding({ running: false, exitCode: null, timedOut: false })).toBe("killed");
  });

  test("a long output folds past a few lines and says how many it hides", () => {
    expect(foldedOutput("a\nb\n")).toEqual({ text: "a\nb", hidden: 0 });
    const long = Array.from({ length: 10 }, (_, i) => `line ${i}`).join("\n");
    expect(foldedOutput(long)).toEqual({ text: "line 0\nline 1\nline 2\nline 3\nline 4\nline 5", hidden: 4 });
  });
});
