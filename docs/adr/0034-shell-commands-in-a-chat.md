# 0034. Shell commands in a chat (`!`)

- **Date:** 2026-10-26
- **Status:** accepted

## Context

As in Claude Code's bash mode, the user gives the agent context from a command's output without spending a turn.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, Shell commands in a chat:

> As Claude Code's bash mode: a message starting with `!` in the app's composer (which shows a shell mode while its text starts with it; `\!` sends a message starting with `!`) runs a command instead of being sent (`chat.shell`), so that the user gives the agent context without spending a turn. The daemon runs it in the workspace folder, where agents start, with the run scripts' shell (zsh, else bash) and environment (the login shell's, and the workspace's `CHAUFFEUR_*`), stdin closed, stdout and stderr in one pipe, in its own process group: killed at 120 s, and what it left running in its group killed when it ends (a server goes in a run script). The output keeps its first and last 15,000 bytes around a "… n bytes omitted …" line. The chat shows it as an item of its own (the command, a spinner then how it ended, the output folded past 6 lines): `shell_started` is ephemeral, `shell_ended` is kept in the event log, so `chauffeur attach` shows it too. `chat.shell` answers once the command started (its `shell_id`): how it ends comes in `shell_ended`, so that an app reconnecting meanwhile loses nothing. It never starts a turn: the daemon puts the commands run since the user's last message before their next one, in Claude's and Codex's chats alike, as `<shell-command>…</shell-command>` and `<shell-output exit="0">…</shell-output>` blocks (`timed_out="120 s"`, `exit="killed"`; a `</shell-` inside a command or its output written `<\/shell-`) under a line saying they are context, not a request; the chat shows the message as written. A message sent while commands of its chat run shows at once, but reaches the agent once they ended, with their output (its `shell_ended` names the message, `message_id`, so that the next one does not hand them again); the messages sent after it wait their turn behind it (proposed 2026-10-27). Claude Code's own `<bash-input>`/`<bash-stdout>` tags were set aside: they split stdout from stderr and carry no exit code. From the app only: `chat.shell` is refused on the daemon's Unix socket, which the CLI, agents (a Codex sandbox reaches it: see Codex's sandbox and the daemon's socket above) and the mobile host use, so that neither an agent, which would get a command run outside its sandbox and its words read by another chat's agent as the user's, nor the phone runs one; only the app's connection, token- and Origin-checked, may call it; there is no CLI verb, and the mobile bridge does not forward the method either. Refused while the workspace is not ready or archived, in an exploration (its chats only read; its Terminal tab stays the way to a shell there) and in a design chat (it never touches code), where the composer offers no shell mode and `!` is plain text. A running command is not stopped by an interrupt, which stays the agent's, only by its timeout (decided with Justin 2026-10-26; a Stop on the shell item and the commands shown read-only on the phone can come later).

## Consequences

`chat.shell` is accepted only on the app's token- and Origin-checked connection: never from the CLI, an agent or the phone, never in an exploration or a design chat.
