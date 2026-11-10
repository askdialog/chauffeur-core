# Archiving a workspace that runs on another machine brings it back first

- **Date:** 2026-11-02
- **Status:** accepted

## Context

A workspace moved to a GCP worker could not be archived from the Mac: the daemon refused it ("moved to another machine") and the app hid Archive, since archiving works on this machine's files while the work (commits, uncommitted changes, conversations) is on the worker. The user had to bring it back, then archive it, and an unused worker kept running, and billed, meanwhile. Justin asked that archiving such a workspace be possible, and that its VM stop.

## Decision

Decided with Justin 2026-11-02: archiving a workspace that runs on another machine this Mac sent it to brings it back first, its agents interrupted at once (archiving stops them anyway), then archives it here as a local workspace. Its machine stops as after any return: once its last workspace is back. It is not archived on the machine: its snapshot would live on a disk that stops with it, and deleting the worker would take it along.

## Consequences

- `workspace.archive` on such a workspace starts the return (`interrupt` and `archive` set on the transfer) and answers at once, the workspace still marked moved; the transfer archives it here once it is back, and says so in its log. The CLI's `chauffeur archive` and the app's Archive (row menu, header) do the same.
- The return runs no setup script here: they would start what archiving stops.
- A return that fails or is cancelled archives nothing: the workspace stays where it runs. Resume asks the archive again; so does starting again after the daemon stopped mid-way (the transfer then shows failed, as any).
- A machine that does not answer refuses the archive, with the return's error. The stale copy on this Mac is never archived in its place.
- A workspace moved by another daemon is still refused: it is brought back from where it was sent.
- The app keeps the workspace in the sidebar while it comes back (a toast says so, the banner shows "Archiving: coming back from <machine> first…"), then it leaves it as any archived one.
