import { describe, expect, test } from "bun:test";
import type { Element } from "hast";
import { codeBlockText } from "./codeBlock";

const pre = (code: Element["children"]): Element => ({
  type: "element",
  tagName: "pre",
  properties: {},
  children: [{ type: "element", tagName: "code", properties: { className: ["language-sql"] }, children: code }],
});

describe("codeBlockText", () => {
  test("is the block's source without its closing newline", () => {
    expect(codeBlockText(pre([{ type: "text", value: "SELECT 1\nFROM t;\n" }]))).toBe("SELECT 1\nFROM t;");
  });

  test("keeps indentation and blank lines", () => {
    expect(codeBlockText(pre([{ type: "text", value: "WITH a AS (\n  SELECT 1\n)\n\nSELECT * FROM a\n" }]))).toBe(
      "WITH a AS (\n  SELECT 1\n)\n\nSELECT * FROM a",
    );
  });

  test("joins text nested in elements", () => {
    const nested = pre([
      { type: "element", tagName: "span", properties: {}, children: [{ type: "text", value: "a" }] },
      { type: "text", value: "b\n" },
    ]);
    expect(codeBlockText(nested)).toBe("ab");
  });

  test("is empty without a block", () => {
    expect(codeBlockText(undefined)).toBe("");
  });
});
