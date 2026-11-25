# Esc gives the interrupted message back to the composer, and Esc twice empties it

- **Date:** 2026-11-25
- **Status:** accepted

## Context

Esc interrupts a working agent, but the message the turn answered stayed only in the chat: to correct and resend it, the user had to copy it or recall it with ↑. Claude Code's terminal puts it back in the prompt on Esc, and empties the prompt on a double Esc. Romain asked for the same; Justin asked for it 2026-11-25.

## Decision

Interrupting the agent with Esc or Stop puts the message the turn answered back into the composer when the composer is empty: the latest message the agent read since the last turn ended, not one still waiting to be read, taken back, suggested by a guest or sent by Chauffeur. The message stays in the chat. "Send now" does not, its message running next. In the composer, two Esc within 800 ms empty it, its text and its images, while the agent is idle (while it works, Esc interrupts). Asked by Justin 2026-11-25, after Romain.

## Consequences

- The app does it alone (`interruptedMessage` in `app/src/lib/chat.ts`, `isDoubleEsc` in `app/src/lib/promptHistory.ts`); no protocol change.
- A draft already in the composer is never replaced: the message is not given back then.
- A turn started by no message of the user's (a background subagent handing back) gives nothing back.
- The first Esc of a double Esc still reaches what else Esc closes (a subagent's panel); only the second is taken by the composer.
