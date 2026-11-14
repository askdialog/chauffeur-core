# A child workspace's requests reach its parent, where the user or the parent's agent answers them

- **Date:** 2026-11-14
- **Status:** accepted

## Context

Story 22 brings a child workspace's questions and permission requests to the user: an amber dot on the child's row, the card in the child's chat. Its parent's agent saw none of it. On 2026-11-13 a child asked three questions in three minutes about its ticket's scope. Its parent was settling the same subject with the user in another chat, so it had the context the child lacked, but `chauffeur wait` and `status` gave it only `needs_input`. It ended up reading the child's raw transcript with grep, and its relay reached the user two seconds after the user had answered in the child's tab. On 2026-11-14 Justin, piloting from a parent, again had to answer a child's question in the child's tab.

## Decision

1. `chauffeur wait` and `chauffeur status` (`workspace.wait`) give the requests a workspace's chats wait on (`open_requests`: the chat, the request's id, its question and options, its tool, or its MCP message). The text output prints them with the `chauffeur respond` line that answers them. The parent no longer reads transcripts. *(Done 2026-11-13.)*
2. A child's requests show in its parent's chats too, with "Open in <child>": its questions, as the same card in place of the composer, and its permission requests and MCP confirmations, as the same cards above the composer. They can be answered from either place; the first answer wins, as between the app, the CLI and the phone, and the card then leaves both. Decided with Justin 2026-11-14.
3. All of a child's requests go to the parent, permissions and MCP confirmations included, not only its questions (decided with Justin 2026-11-14; the proposal kept permissions in the child).
4. The parent's agent may answer its child's requests itself with `chauffeur respond`, when what it knows settles them; otherwise it passes them on to the user with what it knows that bears on them (decided with Justin 2026-11-14; the proposal forbade it).

## Consequences

- Point 1 adds `open_requests` to `WaitResult` and changes nothing else: `wait` already returned when a child needed input.
- The app asks `workspace.wait` with a zero timeout for each child whose activity is `needs_input`, and asks again on its request events (`question`, `permission_request`, `mcp_elicitation`, `request_resolved`, `status_changed`). No new method or event.
- In the parent, Esc on a child's question sets it aside to write to the parent; "Answer" brings it back. It does not decline it, as Esc does in the child.
- The workspace instructions tell an orchestrating agent that `wait` returns on a request, that the user sees it too, and that it may answer it (point 4).
- The child keeps its amber dot and notification: the user may still answer there.
