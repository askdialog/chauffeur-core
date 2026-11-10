# A delegated chat runs on its parent's agent, a review on the other

- **Date:** 2026-11-09
- **Status:** accepted

## Context

The `delegate` tool (story 49) made the calling agent name `agent`, codex or claude, on every call, with nothing to go on but "a review or a second opinion by a different model". The agent did not know either plan's usage, whether the other agent was signed in, nor which model would run: a delegated chat always took the agent's default model. Justin asked who chooses, then settled how.

## Decision

Decided with Justin 2026-11-09:

- **Delegate only when it is needed.** `delegate` is for a review or a second opinion, or a long or parallel piece of work the user may want to follow in its own tab. A quick search or a small sub-task stays with the agent's own subagents when it has them.
- **The parent's agent by default.** A delegated chat runs on the agent of the chat that started it: the load stays on the plan the user chose (a Codex chat keeps everything on Codex).
- **A review on the other agent.** `review: true` is adversarial: it goes to Codex from a Claude chat, to Claude from a Codex chat. The agent decides that a task is a review.
- **Another agent only when the first cannot work.** When the agent chosen is signed out, or one of its plan's own windows (`5h`, `7d`, not a model's) is used to 95% or more and not reset, the task goes to the other agent, and the answer says so. When both are out, it stays where it was asked.
- **The agent picks a tier, Chauffeur the model.** `tier` is `fast`, `default` or `strongest`; `effort` sets the reasoning effort when the model takes one. `default` is the parent's model and effort on the parent's agent, the user's default model on the other. `fast` is Haiku on Claude, GPT-6 Luna on Codex; `strongest` is Opus on Claude (not Fable), GPT-6 Astra on Codex.
- **Fable, or any model, only when the user names it.** `model` takes a model id of either catalog, only when the user asked for that model; it picks the agent too. `agent` likewise only when the user asked for one.

## Consequences

`agent` is no longer required, and `review`, `tier`, `model` and `effort` join the tool's input; `tool.delegate` returns the model, the effort and a `notice` when Chauffeur chose another agent than asked. An unknown model, or an effort the model does not take, is refused with the models or what to do.

The tier table is kept by hand in the daemon (`models::tier_model`). A new Claude model comes with a new SDK, so with a release; a new Codex model comes with the user's `codex`, without one. The `release` skill checks the table against `chauffeur models`. Until the table knows a new model, its tiers keep the older one.

A parent on a local model delegates to the hosted agent's default model: a local model is too slow for another chat's task.

Codex threads keep the tools they started with: a Codex chat started before this change keeps the old `delegate` until its session starts again.
