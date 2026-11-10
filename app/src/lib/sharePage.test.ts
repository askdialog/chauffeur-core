import { describe, expect, test } from "bun:test";
import { flashFor, initials, liveLabel, noAccessText, refusalText, sharedWithYou, shareIdOf, signInTokenOf, socketUrl } from "./sharePage";

describe("the share named in the address", () => {
  test("is the hex id of /s/<id>, and nothing else", () => {
    expect(shareIdOf("/s/0123456789abcdef0123456789abcdef")).toBe("0123456789abcdef0123456789abcdef");
    expect(shareIdOf("/s/ABCDEF0123/")).toBe("abcdef0123");
    expect(shareIdOf("/")).toBeNull();
    expect(shareIdOf("/s/")).toBeNull();
    expect(shareIdOf("/s/not-hex")).toBeNull();
    expect(shareIdOf("/v1/shares/abcdef01")).toBeNull();
  });

  test("its socket is on the page's host, secure when the page is", () => {
    expect(socketUrl({ protocol: "https:", host: "design.askdialog.ai" }, "abcdef01")).toBe("wss://design.askdialog.ai/v1/shares/abcdef01/reviewer");
    expect(socketUrl({ protocol: "http:", host: "127.0.0.1:1421" }, "abcdef01")).toBe("ws://127.0.0.1:1421/v1/shares/abcdef01/reviewer");
  });
});

describe("the banner", () => {
  test("says Live with the daemon, else when the design was last pushed", () => {
    const now = Date.parse("2026-10-21T10:00:00Z");
    expect(liveLabel(true, now - 1000, now)).toEqual({ text: "Live", live: true });
    expect(liveLabel(false, now - 2 * 3600_000, now).live).toBe(false);
    expect(liveLabel(false, now - 2 * 3600_000, now).text).toMatch(/^Last update .*2h/);
    expect(liveLabel(false, undefined, now)).toEqual({ text: "Waiting for the first screens", live: false });
  });

  test("a person's initials", () => {
    expect(initials("Bob Dupont")).toBe("BD");
    expect(initials("bob")).toBe("BO");
    expect(initials("jean-pierre martin")).toBe("JM");
    expect(initials("  ")).toBe("?");
  });
});

describe("a refusal", () => {
  test("is said for what it is", () => {
    expect(refusalText(410, "This share was stopped")).toBe("This share was stopped.");
    expect(refusalText(403, "This design was not shared with you. Ask Alice.")).toBe("This design was not shared with you. Ask Alice.");
    expect(refusalText(403, null)).toBe("This design was not shared with you.");
    expect(refusalText(404, null)).toBe("There is no such share.");
    expect(refusalText(500, "Something failed on the share service; try again")).toBe("Something failed on the share service; try again");
  });
});

test("only screens flash when files change", () => {
  expect(flashFor(["a.html", "styles.css", "b/c.HTM", "canvas.json"], 7)).toEqual({ "a.html": 7, "b/c.HTM": 7 });
});

describe("before signing in", () => {
  test("a sign-in link's token comes from the fragment, and nothing else does", () => {
    expect(signInTokenOf("#sign-in=0a1B2c3d4e5f60718293a4b5c6d7e8f9")).toBe("0a1B2c3d4e5f60718293a4b5c6d7e8f9");
    expect(signInTokenOf("")).toBeNull();
    expect(signInTokenOf("#sign-in=")).toBeNull();
    expect(signInTokenOf("#sign-in=not-hex-at-all-xxxxxxxx")).toBeNull();
    expect(signInTokenOf("#other=0a1b2c3d4e5f6071")).toBeNull();
  });

  test("the card names the owner and the kind of share, or says it plainly", () => {
    expect(sharedWithYou("Alice", "design")).toBe("Alice shared a design with you");
    expect(sharedWithYou("Alice", "workspace")).toBe("Alice shared a workspace with you");
    expect(sharedWithYou(null, "design")).toBe("A design was shared with you");
  });

  test("a reader without access is told which account they used", () => {
    expect(noAccessText("bob@x.com")).toBe("You're signed in as bob@x.com, which doesn't have access.");
  });
});
