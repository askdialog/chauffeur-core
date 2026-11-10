---
name: e2e
description: Run Chauffeur's end-to-end scripts (scripts/e2e-cli.sh, scripts/e2e-chat.sh with real Claude and Codex agents), find why one failed, and add a step for new behaviour. Use after changing the daemon, the CLI, chats or the sidecar, when an e2e run fails, or when a feature needs an end-to-end check.
---

# End-to-end checks

The scripts run real daemons in a throwaway `CHAUFFEUR_HOME` (a `mktemp -d` folder) with seeded repos, and never touch `~/.chauffeur`.

| Script | What | Run after | Time |
|---|---|---|---|
| `scripts/e2e-cli.sh` | the CLI against a real daemon; no agent | a daemon or CLI change | ~1 min |
| `scripts/e2e-chat.sh` | real Claude Haiku chats; `CODEX=1` adds Codex | a chat or sidecar change | 10–15 min, a few cents |
| `scripts/e2e-cloud-transfer.sh` | a whole workspace transfer between three throwaway daemons; `REAL=1` adds a Haiku chat that must resume on the destination | a change of the transfer steps or `chauffeur cloud transfer` | ~1 min |
| `scripts/e2e-cloud-workers.sh` | a GCP worker through a fake `gcloud`, with a temporary `HOME`: estimate, creation and its SSH entry, a transfer to it by name, the relay and opening it in a fake Cursor over SSH, the way back, deletion refused then done; `RECOVERY=1` also authenticates a backup, keeps the old VM's disk and resumes interrupted recovery | a change of `workers.rs`, `chauffeur worker` or how transfers find a machine | ~1 min |

`CODEX=1` needs `codex login`; `CODEX_MODEL` picks the model (the cheapest by default).

## Before running

- The chat script runs the compiled sidecar, `sidecar/dist/chauffeur-sidecar`: after a sidecar change, `cd sidecar && bun run build` first, or the run tests the old code.
- Binaries come from `$PWD/target/debug`. With a `CARGO_TARGET_DIR` elsewhere (another worktree), link it: `ln -sfn <that dir> target`.
- Long runs: start them in the background and wait for them to finish, rather than checking in a loop.

## When a run fails

1. Read the output: each step prints `== <n>. <behaviour>`; the last one printed is the one that failed. Assertions in `python3 -c` print the data they checked; `test` and `grep -q` fail silently.
2. Run again keeping the folder and tracing every command:
   ```bash
   KEEP=1 bash -x scripts/e2e-chat.sh > /tmp/e2e-x.log 2>&1
   ```
   The folder stays in `$TMPDIR` (the run prints it first: `throwaway folder: …`): logs in `home/logs/chauffeurd.log` and `sidecar.log`, the event log in `home/chauffeur.db`:
   ```bash
   sqlite3 "$T/home/chauffeur.db" "select ts, substr(payload, 1, 200) from events where workspace_id = '<id>' order by seq"
   ```
   Remove it afterwards, with the Claude Code transcripts of its chats:
   ```bash
   scripts/forget-transcripts.sh "$T" && rm -rf "$T"
   ```
   Never delete under `~/.claude/projects` with a pattern of your own: with an empty variable it erases the transcripts of every real chat, and none of them can resume. A run without `KEEP` removes its transcripts itself.
3. Tell flakiness from a regression; do not rerun until green without reading why it failed. Known flaky spots, all model behaviour:
   - **Naming (1d):** Claude Haiku names a workspace from its first message within `NAMING_WAIT` (15 s). When it is slower, only the shown title changes, not the folder and branch.
   - **Explorations (4c, 9):** a small model asked to read *and* write in one message may refuse the whole message. Ask for the read and the forbidden write in separate messages.
   - **Plan mode (4c3):** Claude Code tells the model to write nothing but its plan file, above any other instruction (and runs plan mode on Sonnet, whatever the chat's model). The model may decline a write in words: the step insists up to three messages and tells a model that never called Write ("the model did not call its Write tool") from a call Chauffeur never asked about.
   - **Auto mode's classifier (4e):** it reads the whole conversation, an exploration's read-only rules included, and has blocked a promoted exploration's commit (Self-Modification, Instruction Poisoning). The step allows its commit in the throwaway workspace's `.claude/settings.local.json`: Bash allow rules skip the classifier, Chauffeur's hooks and sandbox still apply. Do the same when a step's subject is not the classifier.
   - Agents word their answers freely: assert on files, git state, `--json` output or the event log, not on prose, except for a word the prompt asked for.
   - When the model did not do its part, say so in the failure ("the model did not …"), from the event log (no `tool_call`, a classifier denial), so that it does not read as a Chauffeur bug.

## Adding a step

- Name it after the behaviour: `echo "== 4e. an exploration continues as a workspace: …"`, numbered after the step it builds on.
- Use `c` (the CLI) and `--json` with a `python3 -c` assertion that prints what it checked:
  ```bash
  c --json show my-ws | python3 -c 'import sys,json; w=json.load(sys.stdin); assert not w["explore"], w'
  ```
- `set -e` does not stop on `! command`: write `if command; then echo "<what should not happen>"; exit 1; fi`.
- To change the daemon's environment for one step, restart it around the step and back (`c daemon stop; CHAUFFEUR_IDLE_SESSION_SECS=3 c daemon start`).
- To test what calls `gh` without real pull requests, see the fake gh in `scripts/e2e-cli.sh` (`login-shell`) or the verify-in-app skill.
- Leave nothing behind for later steps: archive the workspaces the step creates (`c archive <slug> --force`). Later steps count the clones' worktrees.
