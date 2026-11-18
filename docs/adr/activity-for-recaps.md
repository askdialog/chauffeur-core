# Agents recap the user's work from their Chauffeur activity

- **Date:** 2026-11-18
- **Status:** accepted

## Context

Reviews found a request that keeps coming back: a recap of the user's own work over a period ("récapitulatif de ce que j'ai fait depuis hier matin", "récap depuis mardi 9 h", for a daily or a 1-1). Six workspaces were created for it between Oct 28 and Nov 12. Agents had no Chauffeur command to read the user's activity across workspaces. They looked at git and GitHub only and missed the chats, so the user asked again ("Analyse les différents workspace chauffeur et les PR", Nov 12). The agent then rebuilt that activity by hand from workspace folders and daily-review files. Recaps of the current chat ("fais moi un récap" after a few days away) were answered in one turn each time and are not in question.

## Decision

Decided with Justin 2026-11-18: a read-only `activity.list` method and `chauffeur activity --since <when> [--until <when>]`. They list the workspaces and chats the user wrote in during the period, archived ones included, ordered by their first message in it. For each workspace they give its name, branch, repos and tickets. For each chat they give its title, its agent, the user's messages and the agent's last text in the period. Every chat of a workspace (not an exploration's, nor a design chat) is told in its system prompt to start a recap of the user's work from it. It then adds pull requests, commits and tickets, and says which sources it read. In-chat recaps are left as they are.

## Consequences

- What counts is what Find routines reads ([find-routines](find-routines.md)): the user's own messages, not what an agent sent (delegated chats, child workspaces, delegated answers), nor guests' suggestions, nor the routines', reviews' and internal workspaces. Pasted text stands as a mark.
- The daemon cuts a message to 800 characters and a last answer to 2,000. The text output cuts them to 300 and 600, one line each, and points to `--json`. Two days of Justin's activity (226 messages in 29 chats) come to about 45,000 characters of text.
- The daemon takes RFC 3339 instants. The CLI also takes a local date, a local date and time, or a span back from now (`36h`, `3d`), so the agent does not convert time zones itself.
- A deleted routine leaves nothing in the store that marks its workspace as a routine's, so the messages Chauffeur wrote there (the refining chat's first message, its runs' prompts) count from then on, as they do for Find routines.
- Pull requests are not in the store; their state stays with `gh`, and tickets' with Linear.
- It reads this machine's store: a workspace moved to another machine shows what was recorded here. An exploration's agent, which cannot use the Chauffeur CLI, is not told about it.
- Anyone who may call the daemon (the user's clients and the agents) may read it, as they already read any workspace's chats with `chauffeur status` and `attach`.
