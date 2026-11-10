# 0031. Piloting a moved workspace from the Mac

- **Date:** 2026-10-26 (also 2026-10-27)
- **Status:** accepted

## Context

A workspace that moved still belongs to the user's Mac: they keep driving it from there, are reminded it costs money while idle, and can open its web previews on the phone.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, Cloud workspace follow-up:

> Piloting a moved workspace (decided with Justin 2026-10-26): the Mac's app keeps using a
> workspace that runs on another machine; the Mac's daemon relays its calls to the
> machine named by `moved_to` (the socket declared in Settings → Transfer) and that
> daemon's events back, so the app pilots it as a local one, marked as running there. A
> first version covers its chats (history, send, interrupt, approvals and confirmations,
> new chat); then its terminals, scripts, diff, files, pull requests and worktrees, and the
> model picker shows that machine's agents. Its whole port block is reachable here while it
> runs there: this daemon listens on loopback on the workspace's ports and passes each
> connection to the same port of its block on that machine, from the move to its return,
> nothing to declare (decided with Justin 2026-10-26). It is built in stages: the calls first (usable
> from the CLI), then the events, then the app.
>
> Idle reminders (decided with Justin 2026-10-26): a moved workspace keeps its machine running,
> and paid for. After an hour without an agent's turn there or the user acting on it from here,
> the Mac's daemon sends one reminder grouping every workspace that reached the hour at the same
> check (each minute): a system notification of the desktop app (a toast while it is in front)
> and a push to the phone, a category of its own (on for a phone that opts in to push from
> then on; off for a grant saved before, until turned on). An ignored reminder is not repeated
> (what the daemon knows is in memory: a daemon that starts again counts the hour from its start): the workspace shows idle (a mark in the sidebar, a line in its banner with "Remind
> me in an hour" and "Bring back here") until something happens there; "Remind me in an hour"
> hides it and reminds again an hour later if nothing happened. A dev server left running is not
> activity. Nothing is ever stopped, deleted or brought back for it
> ([workspace relay](../workspace-relay.md#idle-reminders)).
>
> Private previews on the phone (decided with Justin 2026-10-27, [relay](../workspace-relay.md#previews-on-the-phone)):
> a workspace's web previews are declared as its Docker services are, by detection then
> confirmation: the "Development services" dialog and `chauffeur services <workspace> detect`
> list the ports of its block that answer HTTP (named after the run script listening there, else
> `port-<port>`), and the user checks those to declare (`save --preview <name>=<port>`). The
> profile keeps each by its place in the block, so it names the same service on every machine. The
> phone lists them, with their ready/starting/stopped/error states, only for a workspace that runs
> on another machine; for one on the Mac it shows nothing. Opening one shows it in the app's web
> view: while that screen is open the phone listens on its own loopback on the Mac's port numbers
> (another free one when the phone uses it), and passes each connection through the encrypted
> session to the Mac's mobile host, which passes it to the daemon's Unix socket only, which passes
> it to the machine; a port not declared there is refused. Nothing listens beyond loopback, no
> public link. Once the workspace leaves the machine its open connections are cut within 5 s and the
> screen closes.

## Consequences

Nothing is ever stopped, deleted or brought back because of an idle reminder; previews never listen beyond loopback. Details: [`docs/workspace-relay.md`](../workspace-relay.md).
