# One design comment thread is sent to the agent alone

- **Date:** 2026-11-09
- **Status:** accepted

## Context

A design chat's comments reach its agent only through "Send N new to the agent" (or ⌘Enter, see [comment-and-send-with-cmd-enter](comment-and-send-with-cmd-enter.md)), which sends every new comment of the canvas in one message. Justin wanted to give the agent one comment, a colleague's for instance, without the others, and to send it again once it had gone.

## Decision

A thread's popover, and its row in the Comments panel, has a "Send this thread to the agent" button: it sends that thread alone, its first comment and its replies, straight to the agent, as "Send N new to the agent" does (queued for after its turn when it works), and marks what of it was new as sent (asked by Justin 2026-11-09). The thread goes even when it was sent before, or resolved. A reviewer's page has no such button.

## Consequences

- `design.comments_send` takes an optional `thread`: the message then holds that thread alone, in the format of the grouped send ("(sent before)" on what had gone already); the other new comments stay new.
- A reply's id is refused: a reply goes with its thread.
- Nothing is written into the composer's draft: the thread goes as it is, like the grouped send.
