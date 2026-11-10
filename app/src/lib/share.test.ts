import { describe, expect, test } from "bun:test";
import type { DesignShareState, SharedDesign } from "@chauffeur/protocol";
import { connectionLabel, isShared, parseEmails, sharedChangedAt, sharedSections, shareTitle, suggestions } from "./share";

const state = (more: Partial<DesignShareState> = {}): DesignShareState => ({
  chat: "c1",
  configured: true,
  email: "alice@example.com",
  connection: { state: "online" },
  past_reviewers: ["bob@example.com", "carol@example.com"],
  comments_rev: 0,
  share: {
    id: "s1",
    url: "https://design.askdialog.ai/s/s1",
    title: "CTA",
    owner: { name: "Alice", email: "alice@example.com" },
    reviewers: ["bob@example.com"],
    people: [],
    team: false,
    viewers: [],
    online: true,
    stopped: false,
  },
  ...more,
});

describe("parseEmails", () => {
  test("takes e-mails apart by commas, spaces and lines, lowercase, once", () => {
    expect(parseEmails("Bob@example.com, carol@example.com\nbob@example.com dave@example.com;")).toEqual({
      emails: ["bob@example.com", "carol@example.com", "dave@example.com"],
      invalid: [],
    });
  });

  test("names what is not an e-mail", () => {
    expect(parseEmails("bob, carol@example.com, @x")).toEqual({ emails: ["carol@example.com"], invalid: ["bob", "@x"] });
    expect(parseEmails("  ")).toEqual({ emails: [], invalid: [] });
  });
});

describe("the share's state in words", () => {
  test("shared or not, and who is looking", () => {
    expect(isShared(undefined)).toBe(false);
    expect(isShared(state({ share: undefined }))).toBe(false);
    expect(isShared(state())).toBe(true);
    expect(shareTitle(state())).toBe("Shared with 1: nobody looking now");
    expect(shareTitle(state({ share: { ...state().share!, viewers: ["bob@example.com"] } }))).toBe("Shared with 1: bob@example.com is looking");
    expect(shareTitle(state({ share: { ...state().share!, viewers: ["a@x.com", "b@x.com"] } }))).toBe("Shared with 1: 2 looking");
    expect(shareTitle(undefined)).toContain("Share this design");
    expect(shareTitle(state({ share: { ...state().share!, team: true } }))).toBe("Shared with the team and 1: nobody looking now");
    expect(shareTitle(state({ share: { ...state().share!, team: true, reviewers: [] } }))).toBe("Shared with the team: nobody looking now");
  });

  test("the connection", () => {
    expect(connectionLabel(state())).toEqual({ text: "Live", tone: "online" });
    expect(connectionLabel(state({ connection: { state: "connecting" } }))).toEqual({ text: "Connecting…", tone: "busy" });
    expect(connectionLabel(state({ connection: { state: "failed", message: "no network" } })).tone).toBe("failed");
    expect(connectionLabel(state({ share: undefined }))).toEqual({ text: "Not shared", tone: "off" });
  });

  test("past reviewers not named yet are offered", () => {
    expect(suggestions(state(), ["bob@example.com"])).toEqual(["carol@example.com"]);
    expect(suggestions(undefined, [])).toEqual([]);
  });
});

describe("the Office's colleagues' designs", () => {
  const design = (id: string, named: boolean, team: boolean, more: Partial<SharedDesign> = {}): SharedDesign => ({
    id,
    url: `https://design.askdialog.ai/s/${id}`,
    title: id,
    owner: { name: "Bob", email: "bob@example.com" },
    named,
    team,
    online: true,
    created_at: Date.parse("2026-10-27T10:00:00Z"),
    ...more,
  });

  test("those naming the user first, the team's apart, each once, in the service's order", () => {
    const sections = sharedSections([design("a", false, true), design("b", true, true), design("c", true, false), design("d", false, true)]);
    expect(sections.named.map((d) => d.id)).toEqual(["b", "c"]);
    expect(sections.team.map((d) => d.id)).toEqual(["a", "d"]);
  });

  test("changed when last pushed, else when shared", () => {
    expect(sharedChangedAt(design("a", true, false))).toBe("2026-10-27T10:00:00.000Z");
    expect(sharedChangedAt(design("a", true, false, { last_push_at: Date.parse("2026-10-27T11:30:00Z") }))).toBe("2026-10-27T11:30:00.000Z");
  });
});
