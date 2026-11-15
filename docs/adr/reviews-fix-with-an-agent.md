# Reviews starts a fix in a new workspace and records the decision

- **Date:** 2026-11-15
- **Status:** accepted

## Context

[reviews-interface](reviews-interface.md) made Reviews a place to read patterns and experiments and to record dated follow-up by hand. Justin could not see how to iterate from there: no screen let him hand a problem to an agent, and the status had to be typed in a form after the fact. He asked for the fix to start from Reviews, with Claude or Codex.

## Decision

Asked by Justin and decided 2026-11-15: an experiment, and a pattern with no experiment, has a "Fix with Claude or Codex…" action. It opens the New workspace dialog with a prompt made of the problem, its original sessions, the experiment and its success measure. The user chooses the repos, the agent (Claude or Codex), the model and the permission mode there, as for any workspace, and can edit the prompt. Once the workspace exists, the app records a "Decided" follow-up on the experiment, naming the workspace and the agent. Closing the dialog records nothing. A pattern with several experiments starts a fix without recording a decision, since none is the one chosen.

## Consequences

- No new daemon method: the follow-up uses `reviews.follow_up`, which stays the only write, with its idempotency key and its 10-character minimum for a note. The follow-up has no source, since the new conversation has no event yet to cite; "Tried" and "Observed effect" stay the user's to record.
- If the workspace is created but the follow-up fails, the app says so in a toast; the workspace stays and the follow-up can be recorded by hand.
- Nothing is sent to an external system and no agent starts without the user pressing Create.
- The same change cleans the screens, without changing data: the Overview title no longer repeats Reviews; a pattern's related experiments sit under its summary ("What to try"), not in a side column; confidence shows its level as a badge; reports show only their exceptional state (a complete report shows no badge); excerpts lose their markdown; a partial source sample is a folded line.
