# Reactivate an archived exploration

- **Date:** 2026-11-16
- **Status:** accepted

## Context

Archiving an exploration removes its checkouts and its temporary folder, and restoring it was refused: "the user starts a new exploration". Its chats then stayed read-only for good, though there is nothing to restore: no branch, no changes, no setup. The import from Claude Code files past sessions into history explorations so that they can be asked about later; archiving one would end that. And nobody could go back to an archived exploration's conversation.

## Decision

Proposed 2026-11-16 with the import from Claude Code, accepted by Justin 2026-11-17: any archived exploration can be reactivated, from the Restore action of its archived row and with `chauffeur restore`. It replaces "Restore is refused; the user starts a new exploration" in the SPEC's Explorations section.

Reactivating an exploration:

- keeps its folder: slugs are unique, archived ones included, so the path is always free and Claude's native sessions resume as they are, with nothing copied;
- brings its notes back from the archive;
- fetches each repo and checks it out detached at its base branch as it is now, not as it was at archive;
- recreates its temporary folder and sets its chats back to idle;
- runs no setup and takes no ports, as a new exploration.

The agent is told that the code may have changed since the conversation (in a later pull request). At archive, Chauffeur records the commit of each checkout. After a reactivation, the first message sent in each chat carries a note, for example: "exploration reactivated on 2026-11-20; `api` was at `abc123` at archive, it is now at `def456` on `main` (42 commits); `git log abc123..def456` shows what changed". Without it, the agent would answer as if the code were still the one the conversation was about. The note is sent once per chat per reactivation.

## Consequences

- `restore()` stops refusing explorations and takes its own path for them: no snapshot to apply, a fresh detached checkout instead. The rest (folder, notes, chats unarchived, configuration written again) is shared with a workspace's restore.
- The checkouts may hold newer code than the one the chats talked about; for an exploration that is what we want, and the resume note (in a later pull request) tells the agent.
- Explorations archived before this change can be reactivated too. Without a recorded commit, their resume note (in a later pull request) gives the archive date and the current commit; a history exploration made by the import gives the conversation's date and the current commit.
- It ships in steps: the reactivation first, then the resume note.
- The SPEC's Explorations section is updated and cites this ADR.
