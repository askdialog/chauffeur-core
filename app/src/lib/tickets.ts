import type { TicketRef, WorkspaceInfo } from "@chauffeur/protocol";

/** A Linear ticket identifier as typed: `ABC-3195`, any case (the daemon checks it again). */
const IDENTIFIER = /^[A-Za-z][A-Za-z0-9]*-\d+$/;

export function isTicketIdentifier(text: string): boolean {
  return IDENTIFIER.test(text.trim());
}

/** The identifiers of a workspace's tickets with one more, last (none twice). */
export function withTicket(tickets: TicketRef[], identifier: string): string[] {
  const ids = tickets.map((t) => t.identifier);
  const id = identifier.trim().toUpperCase();
  return ids.some((t) => t.toUpperCase() === id) ? ids : [...ids, id];
}

/** The identifiers of a workspace's tickets without one. */
export function withoutTicket(tickets: TicketRef[], identifier: string): string[] {
  return tickets.map((t) => t.identifier).filter((t) => t.toUpperCase() !== identifier.toUpperCase());
}

/** A ticket Linear never described: the agents have its identifier only. */
export function ticketUnread(ticket: TicketRef): boolean {
  return ticket.title == null;
}

/** Tickets are attached, detached and read again in a ready workspace only. */
export function ticketsEditable(workspace: WorkspaceInfo): boolean {
  // A moved workspace is read-only here: its tickets follow it.
  return workspace.state === "ready" && !workspace.moved_to;
}

/** What a ticket shows on hover in the workspace header. */
export function ticketTooltip(ticket: TicketRef): string {
  if (ticketUnread(ticket)) {
    return `${ticket.identifier} could not be read from Linear: the agents have its identifier only. Refresh the tickets to try again.`;
  }
  return `${ticket.identifier} ${ticket.title}`;
}

/** Past this many tickets the header folds them into a "⋯" that lists them on hover. */
const MAX_INLINE_TICKETS = 3;

export function ticketsCollapsed(tickets: TicketRef[]): boolean {
  return tickets.length > MAX_INLINE_TICKETS;
}
