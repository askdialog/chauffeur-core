# Find routines: proposing routines from the user's own recent requests

- **Date:** 2026-11-09
- **Status:** accepted

## Context

Routines ([routines](routines.md)) are made from a chat, by hand, once the user thinks of it. The work worth a routine shows in what the user keeps asking: the same request worded differently from one week to the next, or a request that carries its own cadence ("analyse all the installs of the last 3 days"). Justin asked Chauffeur to spot those in his recent conversations and propose routines written properly. Asked by Justin 2026-11-09.

## Decision

Decided with Justin 2026-11-09:

- **On demand only.** A "Find routines" button in the sidebar's "Routines" section; no automatic analysis.
- **A modal to start it.** The number of days to read (14 by default, editable) and the agent and model that run the analysis, with the composer's model picker.
- **What is read.** The user's own messages of the last N days (not tool outputs or the agents' answers), across all workspaces, archived ones included, each with its workspace, chat and date.
- **Two signals.** A request that comes back, the same intent worded differently, grouped by meaning with the local search index's vectors when it has them ([local-hybrid-search](local-hybrid-search.md)); and a request naming a time window ("the last 3 days", "this week", "since yesterday", "every morning"): one occurrence is enough, the window gives the cadence ("the last 3 days" gives every 3 days).
- **Cards.** Each candidate shows the original request(s) with a link to their chat, and the routine proposed, written properly: name, a generalised prompt whose dates stay relative to the run, repos, cadence and time. Actions: Create the routine (a draft from that chat, as "Make a routine" does, with the proposal as its first version), Ignore (never proposed again), Later.
- **Every N days.** A routine's schedule also takes every N days at set times, from a first day, besides days of the week.

Proposed to Justin the same day and accepted:

- **The cards stay** after the modal closes: a "Suggestions (n)" entry under the "Routines" heading, kept until each card is created or ignored, reopens them. Later moves a card below the new ones.
- **One hidden call.** The daemon picks and orders what the model reads, then asks the chosen agent and model once, without tools (as titles and design briefs are asked), for the routines as JSON, which it checks. No workspace is created for the analysis. For 14 days of Justin's history (3,682 messages of his own, 294 chats), that is about 50,000 to 80,000 tokens read and 5,000 written: one to two minutes.

## Consequences

- Left out of what is read, as not typed by the user or not a request: delegated and subagent chats, child workspaces (an orchestrating agent wrote their prompts), design chats, the routines' and daily reviews' workspaces, answers delegated back, guests' suggestions, pasted text, and messages shorter than 20 characters without a time window. Each message is cut to 800 characters; within about 200,000 characters, messages naming a window or alike another chat's go first, then each chat's first message, then the others, the newest first.
- The vectors group two messages of different chats whose cosine similarity reaches 0.55 with embeddinggemma (same-intent pairs measured 0.57 to 0.80, French against English included; unrelated requests 0.45 at most). Without vectors, the model groups alone. Both signals are hints to the model, which decides.
- A message a suggestion came from is never read again, so an ignored suggestion is not proposed again; existing routines and every suggestion made are also listed to the model as not to propose again.
- A suggestion's routine takes the agent, model and effort of its newest request's chat; its repos are among its requests' workspaces'. A name another routine has gets a number. Its refining chat starts by saying what a run will do and what it would question.
- `every_days` needs `from` and excludes `days`; a proposed every-N-days schedule starts the day after the analysis. Find routines never runs on a cloud worker, and one analysis runs at a time.
- `agent.ask` (sidecar) and `routine.find`, `routine.suggestions`, `routine.suggestion_create`, `routine.suggestion_set` (client), with `chauffeur routine find|suggestions|suggestion`.
