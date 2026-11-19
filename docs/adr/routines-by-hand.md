# Routines made by hand, from the sidebar's "Routines" section

- **Date:** 2026-11-19
- **Status:** proposed

## Context

A routine is born from a chat ([routines](routines.md)) or from a Find routines suggestion ([find-routines](find-routines.md)). Some work worth a routine was never done in a chat first: the user knows what they want run each morning, and has to start a throwaway chat just to make the routine from it. Asked by Louis 2026-11-19; awaiting Justin's approval, since it widens "born from a chat".

## Decision

Asked by Louis 2026-11-19:

- **A "+" in the "Routines" heading**, beside Find routines, opens a "New routine" modal.
- **A short modal.** The routine's name, its repos (the new-workspace dialog's repo picker), what each run should do in the user's words, and the agent and model (the composer's picker, its first favorite by default). No schedule, no prompt editor.
- **The refining chat does the rest.** Create makes a draft as "Make a routine" does: a workspace of its own with those repos, filed in "Routines", whose refining chat starts at once. Its first version's prompt is the description and its schedule a placeholder (weekdays at 08:00). The refining chat reads the file and the repos, says what a run will do and what it would question, asks when it should run unless the description says so, and rewrites the prompt for an agent starting from nothing.

## Consequences

- `routine.create_by_hand` (name, repos, description, agent, model, effort), callable by the user and by agents as `routine.create` is; `chauffeur routine new --name … --repo … --describe … [--agent …] [--model …]`. `routine.create` keeps making a routine from a chat.
- Such a routine has no source chat. Its workspace's `.chauffeur/routine-source.md` holds the description instead of a conversation.
- The file names the repos by their folder in the workspace, as `routine edit` reads them, even when one was chosen as `owner/name`.
- A draft runs only by hand: nothing runs on the placeholder schedule until the user activates the routine.
