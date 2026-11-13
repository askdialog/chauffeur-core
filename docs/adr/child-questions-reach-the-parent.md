# A child workspace's questions reach its parent, and the user still answers them

- **Date:** 2026-11-13
- **Status:** proposed

## Context

Story 22 brings a child workspace's questions and permission requests to the user: an amber dot on the child's row, the card in the child's chat. Its parent's agent saw none of it. On 2026-11-13 a child asked three questions in three minutes about its ticket's scope. Its parent was settling the same subject with the user in another chat, so it had the context the child lacked, but `chauffeur wait` and `status` gave it only `needs_input`. It ended up reading the child's raw transcript with grep, and its relay reached the user two seconds after the user had answered in the child's tab.

## Decision

1. `chauffeur wait` and `chauffeur status` (`workspace.wait`) give the requests a workspace's chats wait on (`open_requests`: the chat, the request's id, its question and options, its tool, or its MCP message). The text output prints them with the `chauffeur respond` line that answers them. The parent no longer reads transcripts. *(Done with this ADR.)*
2. A child's question shows in its parent's chat too, as the same card, with "Open in <child>". It can be answered from either place. The first answer wins, as between the app, the CLI and the phone today, and the other card reads "answered: …".
3. Only questions go to the parent. A child's permission requests and MCP confirmations stay in the child, with the amber dot and the notification: they concern one command of the child and need nothing the parent knows.
4. The parent's agent does not answer its child's questions. It may annotate one for the user ("we settled this earlier, I would pick B"), but it does not call `chauffeur respond` itself. Letting it answer routine questions would be a separate decision.

Proposed to Justin 2026-11-13; points 2 to 4 await his decision.

## Consequences

- Point 1 adds `open_requests` to `WaitResult` and changes nothing else: `wait` already returned when a child needed input.
- The workspace instructions tell an orchestrating agent that `wait` returns on a question, and that it relays the question rather than answers it (point 4).
- Point 2 needs the app to show another workspace's question card in a chat and keep both cards in step through `request_resolved`. The daemon already broadcasts that event.
