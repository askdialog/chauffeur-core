import { describe, expect, test } from "bun:test";
import { codeOf, linkUsable, SESSION_ENDED, sessionFor } from "./browserOffice";

function storage() {
  const items = new Map<string, string>();
  return { getItem: (key: string) => items.get(key) ?? null, setItem: (key: string, value: string) => void items.set(key, value) };
}

describe("the Office in the browser", () => {
  test("a link is handed out again only well within its code's minute", () => {
    expect(linkUsable(0, 30_000)).toBe(true);
    expect(linkUsable(0, 45_000)).toBe(false);
    expect(linkUsable(0, 61_000)).toBe(false);
  });

  test("only a link's fragment carries a code", () => {
    expect(codeOf("#office=0a1b2c")).toBe("0a1b2c");
    expect(codeOf("#office")).toBeNull();
    expect(codeOf("#office=0a1b&token=x")).toBeNull();
    expect(codeOf("#chat-fixture")).toBeNull();
  });

  test("a link's code is exchanged once, and the session kept for a reload", async () => {
    const tab = storage();
    const exchanged: string[] = [];
    const exchange = async (code: string) => {
      exchanged.push(code);
      return `session-of-${code}`;
    };
    expect(await sessionFor("#office=c0de", tab, exchange)).toBe("session-of-c0de");
    expect(await sessionFor("#office", tab, exchange)).toBe("session-of-c0de");
    expect(exchanged).toEqual(["c0de"]);
  });

  test("a page with neither a code nor a session says to open it again", async () => {
    await expect(sessionFor("#office", storage(), async () => "never")).rejects.toThrow(SESSION_ENDED);
  });
});
