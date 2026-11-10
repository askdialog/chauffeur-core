import { describe, expect, test } from "bun:test";
import { fenceLanguage } from "./codeLanguages";

describe("fenceLanguage", () => {
  test("a fence's info string names its language by name or usual alias", () => {
    expect(fenceLanguage("ts")?.name).toBe("TypeScript");
    expect(fenceLanguage("typescript")?.name).toBe("TypeScript");
    expect(fenceLanguage("tsx")?.name).toBe("TSX");
    expect(fenceLanguage("Python")?.name).toBe("Python");
    expect(fenceLanguage("bash")?.name).toBe("Shell");
    expect(fenceLanguage("yml")?.name).toBe("YAML");
    expect(fenceLanguage("rs")?.name).toBe("Rust");
  });

  test("only the info string's first word counts", () => {
    expect(fenceLanguage('py title="x.py"')?.name).toBe("Python");
  });

  test("a fence without a language, or with one the app does not bundle, stays plain", () => {
    expect(fenceLanguage("")).toBeNull();
    expect(fenceLanguage("cobol")).toBeNull();
    expect(fenceLanguage("text")).toBeNull();
  });

  test("a language loaded on demand gives its parser", async () => {
    const support = await fenceLanguage("py")?.load();
    expect(support?.language.name).toBe("python");
  });
});
