# A design chat started from a chat runs on that chat's agent

- **Date:** 2026-11-24
- **Status:** accepted

## Context

“Design from this chat” always started the design chat on Claude Code ([codex-design-chats](codex-design-chats.md)), even from a Codex or Mistral Vibe chat. A user working with Mistral got an error and thought design chats needed Claude. Design chats run on all three agents ([mistral-vibe-third-agent](mistral-vibe-third-agent.md)), and an agent that creates one with `chauffeur chat --design --from` already passes its own agent.

## Decision

Asked by a user, decided with Justin 2026-11-24: “Design from this chat” starts the design chat on the source chat's agent, with the user's first favorite of that agent (its default model when there is none). A blank design chat (the new-chat menu, ⌘N, `chauffeur chat --design` without `--agent`) still starts on Claude Code.

A chat created with a model and no agent (`chauffeur chat --model mistral-large-4-0`) runs on the agent whose catalog lists that model, as a delegated chat already does; a model of another agent than the one asked is refused. A Vibe chat's agent had created a design chat with Mistral's model and no `--agent`: it started on Claude Code, which failed on the model.

## Consequences

- The app passes `agent` to `chat.create`.
- `chat.create` lists the agents' catalogs when a model is named, which may start an agent.
- The composer's model picker still moves a design chat to another agent.
- The design brief is still written by the user's default Claude model.
