# A delegated chat outlives its parent's turn, and its answer comes as a message

- **Date:** 2026-11-09
- **Status:** accepted

## Context

`delegate` was built as a call that holds the parent's turn until the delegated chat answers ([0047](0047-delegated-chats.md), SPEC "Delegated chats (story 49)"). Codex does not always wait: it runs its tool calls alongside its own work and may end its turn while a `delegate` call is still open. Codex then withdrew the call, the sidecar took that for an interrupt, and the delegated chats were interrupted. Nobody had asked for that, and the agent and the user were not told why. Seen on 2026-11-09: a GPT-6 Astra chat ended its turn with two delegations open, and both were cut off mid-task.

## Decision

Only an interrupt of the parent (the user's interrupt, or stopping its session: closing, archiving) interrupts its delegated chats. When the parent's turn ends in any other way before a delegated chat answered, the delegated chat goes on working. Once its turn is over, Chauffeur sends its answer to the parent as a new message, which starts a turn or steers the running one. The chat shows that message with the delegated chat it comes from. Decided with Justin 2026-11-09.

## Consequences

- A new sidecar → daemon method, `tool.delegate_detach`: the daemon waits for the delegated chat as `tool.delegate_wait` does, then sends its answer to the parent. The text reads like the tool's result, with a first line saying why it comes as a message. `user_message` gains `delegated_answer` (the delegated chat's id, agent and title), which is shown to people. The agent reads the text.
- Codex: a `delegate` call that Codex withdraws (`serverRequest/resolved`), that is still open at `turn/completed`, or that is left behind when the app-server dies is detached. Chauffeur's own interrupt and the session's stop interrupt it, as before. At the turn's end the chat shows the call as `[the delegated chat works on: its answer will come as a new message]`, not as an error.
- Claude Code waits for its MCP calls, so a Claude chat's turn never ends with one open. It goes through the same code if one ever does.
- The waiter lives in the daemon's memory: if the daemon stops before the answer, the answer is not forwarded. The delegated chat's agent stops with the daemon anyway.
- `delegate`'s description tells the agent that an answer may come later as a message.
