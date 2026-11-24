# A message steering the turn can be taken back until the agent reads it

- **Date:** 2026-11-24
- **Status:** accepted

## Context

A message sent while the agent works steers the turn: marked "steering…" until the agent reads it at its next step, which may be minutes away behind a long command. Justin sent one he no longer wanted and could not take it back: interrupting with "Send now" only made the agent read it sooner.

## Decision

The "steering…" mark of a message the agent has not read yet offers Cancel: the message is taken back from the agent, which never reads it, leaves the chat, and its text returns to the composer, before what is written there. Once the agent has read it, Cancel says it is too late. Claude Code drops it from its queue (`cancel_async_message`); Codex reads a steer at once, so only a message still waiting for its turn can be taken back there; Vibe holds messages until its next prompt. Asked and decided by Justin 2026-11-24.

## Consequences

- `chat.withdraw_message` (client protocol 28) and `session.withdraw` carry it; the chat logs `user_message_withdrawn`, which the app reads to hide the message. Its `user_message` stays in the log, so a fork's handoff and a design brief leave it out (`design::conversation_text`).
- A message waiting in the daemon for the user's shell commands is never handed over once taken back, and the output of the commands it was to hand over does not reach the agent either. Left for later, if it matters.
- Its images do not return to the composer: the app says so.
