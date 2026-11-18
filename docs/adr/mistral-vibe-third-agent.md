# Mistral Vibe is a third agent, driven through ACP, on the user's Mistral plan

- **Date:** 2026-11-18
- **Status:** accepted

## Context

Justin asked for Mistral Vibe beside Claude Code and Codex, used through his Vibe subscription (Le Chat Pro) rather than API billing. A spike on 2026-11-18 (Vibe 3.1.3, notes in the workspace `ajouter-vibe-mistral-aux-modeles`, `.chauffeur/spike-vibe/`) drove `vibe-acp` by hand. Sessions, models, permissions, MCP servers and resuming all worked. A prompt moved the plan's budget (`_account/read`), so the plan pays, not API billing.

## Decision

Decided with Justin 2026-11-18:

- **A third agent, not a model of another one.** Vibe is an agent of its own (`AgentKind::Vibe`), with its own models, sign-in and plan usage, beside Claude Code and Codex.
- **Driven through ACP, by a generic agent.** The sidecar talks to Vibe over the Agent Client Protocol (`vibe-acp`, JSON-RPC over stdio), through an ACP agent written for any ACP agent, Vibe being its first. Vibe's undocumented `vibe-app-server` is not used.
- **On the user's Mistral plan.** The user signs in with their Mistral account through Vibe's own sign-in. The key it makes is billed to their plan (Le Chat Pro, Team), and pay-as-you-go applies only if they enabled it. Chauffeur never reads or stores the key.
- **Full parity in the first version.** Everything a Codex chat does, a Vibe chat does too: workspace chats, explorations, delegated chats, design chats, routines, titles and briefs, forks, compaction, changing a chat's provider, plan usage and sign-in in the sidebar, the phone, transfers to a worker.
- **Vibe stays out of reviews.** A review (`delegate` with `review: true`) still goes from Claude to Codex and from Codex to Claude. A review asked from a Vibe chat goes to Claude, or to Codex when Claude cannot work. Vibe runs a delegated chat only when it is the parent's agent or when the user names it.

## Consequences

- **Installation.** As for Codex, the sidecar runs the user's own Vibe: `CHAUFFEUR_VIBE_PATH`, else `vibe-acp` on the agent's PATH (`uv tool install mistral-vibe`, or the standalone `vibe-acp` Mistral publishes on each GitHub release). When Vibe is missing, the chat says how to install it. The sidecar records the version it was tested with (3.1.3), and a session logs a warning when `initialize` reports another one.
- **Sign-in.** Sign in uses ACP's `authenticate` with the delegated browser sign-in (`browser-auth-delegated`). The app opens the `signInUrl` Vibe returns, as for the other agents' logins. Vibe 3.1.3's own browser sign-in gives up after about 40 s: `console.mistral.ai` answers `429` to its polling and Vibe stops after three failures. Chauffeur must not depend on that polling, and the bug is to be reported to Mistral.
- **Plan usage.** Usage is read through Vibe's ACP extension `_account/read`: plan, budget percentage, reset date, pay-as-you-go. Vibe has one monthly budget, not 5h and 7d windows: its row in the sidebar shows that budget. For `delegate`, Vibe "cannot work" when it is signed out or its budget is spent without pay-as-you-go.
- **Guards.** Vibe has no OS sandbox and no hooks. Chauffeur's guards (the git and GitHub write guard, an exploration's plan folder, a design chat's folder, `chauffeur` commands refused from inside) therefore run in the sidecar's answer to `session/request_permission`. ACP carries no reason with a refusal, so the sidecar cancels Vibe's step and prompts it again at once with why, in the same turn of the chat. Vibe runs in its `ask` mode, and the sidecar allows by itself what the chat's permission mode allows and the guards accept. This is weaker than an OS sandbox: a shell command the sidecar allows is not confined. Explorations and design chats on Vibe must ask for every shell command that is not known to be read-only.
- **Vibe's own extras.** Vibe loads the user's skills and has its own `vibe:worktree` command. Worktrees go through Chauffeur: the guard refuses the `git worktree add` it runs, as for the other agents. Vibe reads a folder's AGENTS.md only once the folder is trusted: the workspace folder is marked trusted for Vibe (`_trust/decision`, `trust_cwd`), and the session opened again so that the first one reads it.
- **Chauffeur's tools.** `list_repos`, `add_repo` and `delegate` reach Vibe as an MCP server of the session, served by the sidecar over streamable HTTP on 127.0.0.1 with a token of the session's: Vibe 3.1.3 refuses ACP's own MCP transport. Vibe cancels a tool its code mode calls after 60 s, whatever the server's timeout, so `delegate` and `add_repo` answer within 45 s; a delegated chat then works on and an answer of the user's comes later, each as a new message. Vibe keeps its plan natively (ACP `plan`), as Codex does.
- **Models and effort.** Models come from the session's `configOptions`, which depend on the account (Mistral Large 4, Mistral Medium and GLM models on 2026-11-18), and effort from its `thinking` option (low, medium, high). `delegate`'s tiers gain a Vibe column in `models::tier_model`. A model the account does not offer is said in the chat, which runs on Vibe's default.
- **Changing a chat's provider.** The handoff of [0039](0039-changing-a-chats-provider.md) applies between any two of the three agents. Design chats, until now Claude and Codex only ([codex-design-chats](codex-design-chats.md)), take Vibe too.
- **Cost.** `AgentKind::Codex` appears about 70 times in 13 Rust files, and `"codex"` in 29 TypeScript files of the app and the sidecar. Each such place must handle a third agent. The end-to-end scripts gain Vibe chats, and they skip them where Vibe is not signed in.
- **SPEC.** `SPEC.md` names Claude Code and Codex as Chauffeur's agents. It is updated with the change that brings Vibe, citing this ADR.
