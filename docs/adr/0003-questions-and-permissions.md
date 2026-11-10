# 0003. Questions, permission requests and Always allow

- **Date:** 2026-10-14 (also 2026-10-17, 2026-10-18, 2026-10-19, 2026-10-21)
- **Status:** accepted

## Context

A blocked agent must never wait on a prompt the user cannot see. Chauffeur answers Claude's and Codex's questions, permission requests and MCP confirmations itself, from the app, the CLI and the phone.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 14:

> As a developer, I want to answer the agent's questions and permission requests from the app or the CLI, so that a blocked agent never waits on a hidden prompt. Questions (Claude's AskUserQuestion; Codex's request_user_input, which Codex offers only in its experimental Plan collaboration mode, not used yet) are answered in place of the composer the way Claude Code's terminal does: a tab per question, numbered options, a free answer, a review before sending, Esc to answer in the chat instead (decided with Justin on 2026-10-14). An option's preview (Claude asked for markdown, never HTML) shows beside the options, below them in a narrow chat, for the option focused or hovered (decided with Justin 2026-10-17). Always allow on a permission request means this tool is not asked again in this chat (for Bash, the command prefix Claude suggests, else that exact command), in every permission mode, plan mode included where Claude Code itself asks again whatever its rules: the chat keeps the rules and answers the calls they cover; the open requests they cover are allowed with it, as Claude Code's terminal does. The chat matches a whole tool or MCP server, a Bash command, and reads of an absolute folder (`Read(//abs/**)`, Claude's suggestion for a file outside the working directories, covering Read, Glob and Grep under it), so that one answer allows the parallel reads of the same folder (asked by Justin 2026-10-19). Claude's own suggested rules are kept when it makes some (it may write them to the workspace's `.claude/settings.local.json`), else the rule is the chat's only, never written to a settings file; the card says "always allowed" and which rule. Claude's other suggestions are dropped: Always allow never switches the chat to accept edits (`setMode`) nor makes a folder outside the workspace a working directory (`addDirectories`, which would let every tool read it unasked); the chat answers the calls its rules cover itself (asked by Justin 2026-10-18). The daemon hands the chat's rules to its next agent sessions (an idle session closed, a restart) (decided with Justin 2026-10-18). A Codex chat's Always allow is Codex's session approval (`acceptForSession`), and claims no more than it: a command in its folder, the files a change touches; a change asked again after the sandbox refused it, input to a running command, a subcommand and network access get no rule (Codex asks again, or its key is not shown). The card names those rules, the open requests they cover are allowed with it, and the chat answers Codex when it asks for them again, its next sessions included (asked by Justin 2026-10-18; the next sessions part pending his confirmation).

From `SPEC.md`, MCP confirmations (2026-10-21):

> MCP confirmations in Codex chats are put to the user rather than automatically declined: show the server and message, JSON Schema form or explicit HTTP(S) confirmation link, and Accept/Continue, Decline and Cancel. Acceptance is validated against the original schema in the sidecar; invalid answers leave the request open. Unsupported schemas/links are visible and cannot be accepted. A confirmation with nothing to fill in (no field, no link, as Computer Use's "Allow Computer Use to use "Google Chrome"?") also offers Always allow: the chat keeps its server and message as a rule (`cua_repl: Allow Computer Use to use "Google Chrome"?`), accepts the open confirmations it covers and the same confirmation each time Codex asks again, its next sessions included, as for a Codex approval; another app or a form with a field is still asked (asked by Justin 2026-10-21). The desktop, CLI and phone carry the response; interruption, turn end and server withdrawal cancel the pending request. Response content is not copied into the chat event log. This adds MCP confirmation transport; it does not grant macOS privileges or establish a successful real Computer Use session.

## Consequences

Always allow rules live in the chat (and in Claude's own suggested rules when it makes some) and the daemon hands them to the chat's next sessions; Always allow never changes the permission mode nor adds a working directory. For Codex, the next-sessions part was left pending Justin's confirmation.
