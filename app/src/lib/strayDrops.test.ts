import { describe, expect, test } from "bun:test";
import { refuseStrayDrop } from "./strayDrops";

function dragEvent(types: string[], defaultPrevented = false) {
  const event = {
    defaultPrevented,
    preventDefault: () => { event.defaultPrevented = true; },
    dataTransfer: { types, dropEffect: "copy" as DataTransfer["dropEffect"] },
  };
  return event;
}

describe("refuseStrayDrop", () => {
  test("a file dropped outside every drop target is refused instead of opened", () => {
    const event = dragEvent(["Files"]);
    refuseStrayDrop(event, false);
    expect(event.defaultPrevented).toBe(true);
    expect(event.dataTransfer.dropEffect).toBe("none");
  });

  test("on Linux a file dragged as a URI list is refused too", () => {
    const event = dragEvent(["text/uri-list", "text/html"]);
    refuseStrayDrop(event, true);
    expect(event.defaultPrevented).toBe(true);
    expect(event.dataTransfer.dropEffect).toBe("none");
  });

  test("a drop a target accepted keeps its effect", () => {
    const event = dragEvent(["Files"], true);
    refuseStrayDrop(event, false);
    expect(event.dataTransfer.dropEffect).toBe("copy");
  });

  test("dragged text is left to the text field under it", () => {
    for (const native of [false, true]) {
      const event = dragEvent(["text/plain", "text/html"]);
      refuseStrayDrop(event, native);
      expect(event.defaultPrevented).toBe(false);
      expect(event.dataTransfer.dropEffect).toBe("copy");
    }
  });

  test("a link dragged from a browser is left alone outside Linux", () => {
    const event = dragEvent(["text/uri-list", "text/plain"]);
    refuseStrayDrop(event, false);
    expect(event.defaultPrevented).toBe(false);
    expect(event.dataTransfer.dropEffect).toBe("copy");
  });

  test("the page's own drags are left alone", () => {
    const event = dragEvent(["application/x-chauffeur-workspace"]);
    refuseStrayDrop(event, true);
    expect(event.defaultPrevented).toBe(false);
  });
});
