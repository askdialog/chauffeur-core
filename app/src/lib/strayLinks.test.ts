import { describe, expect, test } from "bun:test";
import { openStrayLink, strayLink } from "./strayLinks";

/** A click inside a link to `href` (null: outside any link). */
const click = (href: string | null, more: { button?: number; defaultPrevented?: boolean } = {}) => {
  let prevented = more.defaultPrevented ?? false;
  return {
    button: more.button ?? 0,
    get defaultPrevented() {
      return prevented;
    },
    preventDefault: () => {
      prevented = true;
    },
    target: { closest: (selector: string) => (selector === "a[href]" && href !== null ? { getAttribute: () => href } : null) },
  };
};

describe("stray links", () => {
  test("a web link no component opened goes to the browser, a middle click too", () => {
    expect(strayLink(click("https://github.com/acme/web/pull/320"))).toBe(
      "https://github.com/acme/web/pull/320",
    );
    expect(strayLink(click("http://example.com", { button: 1 }))).toBe("http://example.com");
    const opened: string[] = [];
    const event = click("https://linear.app/acme/issue/ABC-1");
    openStrayLink(event, (url) => opened.push(url));
    expect(opened).toEqual(["https://linear.app/acme/issue/ABC-1"]);
    expect(event.defaultPrevented).toBe(true);
  });

  test("a link a component opened, a right click, a file or an anchor of the page stay as they are", () => {
    expect(strayLink(click("https://github.com", { defaultPrevented: true }))).toBeNull();
    expect(strayLink(click("https://github.com", { button: 2 }))).toBeNull();
    expect(strayLink(click("src/lib/officeWorld.ts"))).toBeNull();
    expect(strayLink(click("#top"))).toBeNull();
    expect(strayLink(click(null))).toBeNull();
    const event = click("src/main.tsx");
    openStrayLink(event, () => {
      throw new Error("opened");
    });
    expect(event.defaultPrevented).toBe(false);
  });
});
