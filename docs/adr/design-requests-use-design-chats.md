# Interface mockup requests use Chauffeur design chats

- **Date:** 2026-11-06
- **Status:** accepted

## Context

A request to see a proposed SaaS interface made Codex invoke its bundled `visualize` skill. It wrote an HTML fragment into the notes and returned a content reference that Chauffeur displayed verbatim. The user expected a Chauffeur design chat and its canvas; an explicit follow-up made the agent use the existing CLI successfully.

## Decision

Decided with Justin 2026-11-06: instruct agents to use a Chauffeur design chat when asked for an interface mockup. An ordinary workspace chat creates it from the current conversation, then sends the design request to start the designer without asking for confirmation. The request carries the relevant context and asks for screens consistent with the existing product and verified renders. The parent names the design chat without claiming unfinished screens are ready.

Every chat is told that Chauffeur cannot render the `visualize` skill's content references. Mockups belong in a design chat's canvas as self-contained HTML screens. An exploration's agent retains its restrictions and directs the user to “Design from this chat”.

## Consequences

Use the existing creation, brief and send commands; no new rendering protocol or agent tool is introduced. Both Claude Code and Codex receive the guidance when their session starts. Existing sessions receive it on their next session start. Design chats keep their own screen instructions and do not create another design chat. Ordinary explanations, Mermaid diagrams, implementation requests and explicit file exports are unaffected.
