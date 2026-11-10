# Routines: a prompt Chauffeur runs by itself at set times, born from a chat

- **Date:** 2026-11-08
- **Status:** accepted

## Context

Justin repeats some prompts every day or every week: a morning triage, a weekly dependency check. He wants Chauffeur to run them by itself. A prompt written in one go rarely works unattended. So a routine starts from a chat where the work was done once. Justin runs it by hand and fixes it until it is good enough, and only then lets it run alone. Asked by Justin 2026-11-08.

## Decision

Decided with Justin 2026-11-08:

- **Born from a chat.** "Make a routine" in a chat tab's menu creates a routine from that chat. It starts imperfect: Justin runs it, refines it, and only once it is viable does it run by itself.
- **A file.** A routine is a TOML file: its name, prompt, repos, agent and model, permission mode and schedule. The daemon keeps every version of it. Sharing a routine with colleagues is left out for now.
- **Draft, then Active.** A draft runs only by hand (Run now). After a run, Justin tells the routine's refining chat what was wrong, and its agent edits the file: a new version. Activate makes it run on its schedule; Pause stops that.
- **Local.** The daemon runs it on the Mac. There is no cloud worker.
- **Triggered by the time.** A schedule (days of the week and times of day, local time, e.g. every weekday at 8:00). No other trigger for now (no Linear or GitHub events).
- **Missed runs are caught up once.** If the Mac was off or asleep at 8:00 and the daemon is back at 9:00, the routine runs at 9:00. Several missed occurrences give a single run.
- **One dedicated workspace per routine.** Making the routine creates a workspace of its own with the routine's repos, filed in a "Routines" sidebar section. Its first chat is the refining chat; each run is a new chat (a new tab) in that workspace. The workspace the routine came from stays independent: it can be archived without affecting the routine.
- **Worktrees refreshed when clean.** Before a run, each worktree with no uncommitted change and no unpushed commit is put back on the freshly fetched `origin/<base>`. Any other worktree is left as it is, and the run's prompt tells the agent so. A run never loses work.
- **No overlap.** An occurrence that falls while the previous run still works or waits for Justin is not started beside it. It counts as missed and is caught up once, as soon as that run is done.
- **Finished runs' tabs are closed.** When a run starts, the tabs of earlier runs that are finished and not waiting for Justin are closed (Close: the conversation is kept, listed among the closed chats, reopenable). The refining chat, a running run and the runs waiting for Justin stay open.
- **Auto permission mode by default.** A routine runs in Auto mode unless its file says otherwise. A question or a permission request waits for Justin, with the usual desktop notification and mobile push.
- **Sidebar and routine view, and a CLI.** The "Routines" section lists each routine's workspace with its state (Draft, Active, Paused) and next run. The routine's view shows its file, Run now, Activate/Pause, its versions and its runs. The CLI: `chauffeur routine new|ls|show|edit|run|activate|pause|rm|history`.

## Consequences

- The daemon owns routines, their versions and runs, and the schedule. It works out what is due when it starts and every minute after; a Mac waking up is caught up within a minute.
- The tests drive the schedule with a given clock (`run_due(d, now)`), never the real time.
- The agent edits a routine through `chauffeur routine edit`, which the daemon validates and records as a new version. A file it refuses changes nothing. The current version is also written to the routine workspace's notes (`.chauffeur/routine.toml`) for the agent to read and edit.
- Only the user activates, pauses, runs or deletes a routine; an agent may read it, create a draft and edit it.
- Not done yet: sharing routines, waking the Mac for a run (the daily review's `pmset` wake), and triggers other than the time.
