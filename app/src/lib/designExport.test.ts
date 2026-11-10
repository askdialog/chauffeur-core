import { describe, expect, test } from "bun:test";
import type { CanvasScreen } from "@chauffeur/protocol";
import type { ShownSection } from "./canvas";
import { exportedMessage, sectionExport, selectionExport } from "./designExport";

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
  { title: "Checkout", unsorted: false, screens: [screen("pay.html"), screen("gone.html", { url: undefined, modified_ms: undefined })] },
  { title: "Unsorted", unsorted: true, screens: [screen("stray.html")] },
];

describe("selectionExport", () => {
  test("nothing selected is nothing to export", () => {
    expect(selectionExport(sections, [])).toBeNull();
    expect(selectionExport(sections, ["gone.html"])).toBeNull();
  });

  test("one screen is named after its label, or its file", () => {
    expect(selectionExport(sections, ["cart-empty.html"])).toEqual({ files: ["cart-empty.html"], name: "Empty cart" });
    expect(selectionExport(sections, ["pay.html"])).toEqual({ files: ["pay.html"], name: "pay.html" });
  });

  test("screens of one section are named after it, in canvas order", () => {
    expect(selectionExport(sections, ["cart-empty.html", "cart.html"])).toEqual({ files: ["cart.html", "cart-empty.html"], name: "Cart" });
  });

  test("screens across sections are counted, missing ones left out", () => {
    expect(selectionExport(sections, ["stray.html", "cart.html", "gone.html", "pay.html"])).toEqual({
      files: ["cart.html", "pay.html", "stray.html"],
      name: "3 screens",
    });
  });
});

describe("sectionExport", () => {
  test("takes the section's present screens", () => {
    expect(sectionExport(sections[1]!)).toEqual({ files: ["pay.html"], name: "Checkout" });
    expect(sectionExport(sections[2]!)).toEqual({ files: ["stray.html"], name: "Unsorted" });
    expect(sectionExport({ title: "", unsorted: false, screens: [screen("gone.html", { url: undefined, modified_ms: undefined })] })).toBeNull();
  });

  test("a section without a title is named by its count", () => {
    expect(sectionExport({ title: " ", unsorted: false, screens: [screen("a.html"), screen("b.html")] })?.name).toBe("2 screens");
  });
});

describe("exportedMessage", () => {
  test("names the file and whether it is on the clipboard", () => {
    expect(exportedMessage({ path: "/ws/.chauffeur/designs/d/exports/2026-10-21-120000-cart.png", copied: true })).toBe(
      "Exported 2026-10-21-120000-cart.png, copied to the clipboard",
    );
    expect(exportedMessage({ path: "/x/cart.png", copied: false })).toBe("Exported cart.png into the design folder's exports/");
  });
});
