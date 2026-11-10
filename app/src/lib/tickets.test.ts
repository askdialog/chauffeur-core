import { describe, expect, test } from "bun:test";
import type { TicketRef, WorkspaceInfo } from "@chauffeur/protocol";
import { isTicketIdentifier, ticketsCollapsed, ticketsEditable, ticketTooltip, ticketUnread, withoutTicket, withTicket } from "./tickets";

const read: TicketRef = { identifier: "ABC-1", title: "Facet filters", url: "https://linear.app/x/issue/ABC-1" };
const unread: TicketRef = { identifier: "ABC-3195", title: null, url: null };

describe("workspace tickets", () => {
  test("identifiers are recognised as typed", () => {
    expect(isTicketIdentifier("abc-3195")).toBe(true);
    expect(isTicketIdentifier(" ENG2-7 ")).toBe(true);
    expect(isTicketIdentifier("facet filters")).toBe(false);
    expect(isTicketIdentifier("web")).toBe(false);
    expect(isTicketIdentifier("--upload-pack=x")).toBe(false);
  });

  test("a ticket is added last and once, removed in any case", () => {
    expect(withTicket([read], "abc-2")).toEqual(["ABC-1", "ABC-2"]);
    expect(withTicket([read], "abc-1")).toEqual(["ABC-1"]);
    expect(withoutTicket([read, unread], "abc-1")).toEqual(["ABC-3195"]);
    expect(withoutTicket([read], "ABC-1")).toEqual([]);
  });

  test("a ticket Linear never described says so, and how to retry", () => {
    expect(ticketUnread(unread)).toBe(true);
    expect(ticketUnread(read)).toBe(false);
    expect(ticketTooltip(unread)).toContain("could not be read from Linear");
    expect(ticketTooltip(unread)).toContain("Refresh");
    expect(ticketTooltip(read)).toBe("ABC-1 Facet filters");
  });

  test("only a ready workspace changes its tickets", () => {
    const ws = (state: WorkspaceInfo["state"]) => ({ state }) as WorkspaceInfo;
    expect(ticketsEditable(ws("ready"))).toBe(true);
    expect(ticketsEditable({ ...ws("ready"), moved_to: "vm" })).toBe(false);
    for (const state of ["creating", "archiving", "archived", "cleanup_failed"] as const) {
      expect(ticketsEditable(ws(state))).toBe(false);
    }
  });

  test("a few tickets stay in the header, many fold into a list on hover", () => {
    const many = (n: number) => Array.from({ length: n }, (_, i) => ({ ...read, identifier: `DEC-${i}` }));
    expect(ticketsCollapsed(many(3))).toBe(false);
    expect(ticketsCollapsed(many(4))).toBe(true);
  });
});
