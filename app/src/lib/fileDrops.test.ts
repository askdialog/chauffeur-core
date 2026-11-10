import { describe, expect, test } from "bun:test";
import { bringsFiles, deliverFileDrop, FILE_DROP_ATTRIBUTE, type FileDrop, registerDropTarget } from "./fileDrops";

/** An element that is, or sits inside, `target` (null: outside every drop target). */
function element(target: Element | null): Element {
  return { closest: (selector: string) => (selector === `[${FILE_DROP_ATTRIBUTE}]` ? target : null) } as unknown as Element;
}

function drop(x: number, y: number): FileDrop {
  return { drop: 1, x, y, files: [{ name: "Screenshot.png", mediaType: "image/png", size: 5 }] };
}

describe("bringsFiles", () => {
  test("a file dragged from a file manager is taken, on Linux by its URI list", () => {
    expect(bringsFiles(["Files"], false)).toBe(true);
    expect(bringsFiles(["text/uri-list", "text/html"], true)).toBe(true);
  });

  test("a URI list without files is left alone outside Linux, and the page's own drags everywhere", () => {
    expect(bringsFiles(["text/uri-list", "text/html"], false)).toBe(false);
    expect(bringsFiles(["application/x-chauffeur-workspace"], true)).toBe(false);
    expect(bringsFiles(["text/plain"], true)).toBe(false);
  });
});

describe("deliverFileDrop", () => {
  test("a drop goes to the drop target under it, its position taken back from the zoom", () => {
    const composer = {} as Element;
    const taken: FileDrop[] = [];
    const unregister = registerDropTarget(composer, (d) => taken.push(d));
    const points: [number, number][] = [];
    const root = {
      elementFromPoint: (x: number, y: number) => {
        points.push([x, y]);
        return element(composer);
      },
    };
    expect(deliverFileDrop(root, drop(300, 150), 1.5)).toBe(true);
    expect(points).toEqual([[200, 100]]);
    expect(taken.map((d) => d.files[0].name)).toEqual(["Screenshot.png"]);
    unregister();
    expect(deliverFileDrop(root, drop(300, 150), 1.5)).toBe(false);
    expect(taken).toHaveLength(1);
  });

  test("a drop next to every drop target attaches nothing", () => {
    expect(deliverFileDrop({ elementFromPoint: () => element(null) }, drop(10, 10), 1)).toBe(false);
    expect(deliverFileDrop({ elementFromPoint: () => null }, drop(-5, 10), 1)).toBe(false);
  });

  test("a target taken off the page keeps its successor's registration", () => {
    const target = {} as Element;
    const first: FileDrop[] = [];
    const second: FileDrop[] = [];
    const undoFirst = registerDropTarget(target, (d) => first.push(d));
    registerDropTarget(target, (d) => second.push(d));
    undoFirst();
    expect(deliverFileDrop({ elementFromPoint: () => element(target) }, drop(1, 1), 1)).toBe(true);
    expect([first.length, second.length]).toEqual([0, 1]);
  });
});
