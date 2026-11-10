# Native session portability proof

Compatibility experiments toward the approved [cloud workspace handoff](cloud-workspaces.md). These are opt-in proofs, not a transfer feature or a production credential vault.

## Run

On macOS, with Bun 1.4.3, Docker running Linux, `codex-cli 0.162.1` on PATH, and sidecar dependencies installed:

```sh
bash scripts/e2e-session-portability.sh
# Run only one scenario:
bash scripts/e2e-session-portability.sh --scenario tools
bash scripts/e2e-session-portability.sh --scenario compact
# A Codex fork with a native parent-rollout dependency:
bash scripts/e2e-session-portability.sh --scenario fork
# Retain only synthetic artifacts for investigation:
KEEP=1 bash scripts/e2e-session-portability.sh
```

The initial image build downloads the pinned SDK and Codex packages. Runtime containers have no external network. No Claude/Codex login, API subscription, model spend or GCP resource is required.

### Optional real ChatGPT account proof

The separate hosted test makes real OpenAI requests against the existing native ChatGPT subscription and consumes its quota. It is never included in the default deterministic suite:

```sh
cd sidecar
bun scripts/session-portability/hosted-run.ts --use-chatgpt-account
```

It requires a file-backed native ChatGPT login with an access token valid for at least ten more minutes, the pinned Codex runtime and Docker with outbound networking. It refuses API-key authentication and never changes provider, billing mode or the personal login. The runner reads only the selected access token and account ID from `CODEX_HOME/auth.json` (default `~/.codex/auth.json`), then passes them via stdin into disposable phases. Each native app-server uses experimental `account/login/start` with `chatgptAuthTokens` and ephemeral credential storage. Tokens are held in memory, not Docker build arguments, command arguments, environment variables or transferred history. No refresh token, personal config, skills, other sessions or home directory is copied/mounted. Auth refresh requests abort the proof; renew the native login separately if needed.

The source creates one synthetic conversation, manually invokes `thread/compact/start`, and requires a persisted compaction record containing an actual encrypted `compaction` item. Each host then answers a memory question whose prompt does not contain the source's random identifier. Linux and the returning Mac resume the same native session using only its selected rollout, retain the entire source compaction record unchanged, and must not compact again. The harness rejects a changed compaction record before destination inference. No transcript rewriting or invented opaque content is used.

Only safe structured evidence is printed on success. The runner checks that no access token or `auth.json` was persisted in generated state. It removes the temporary roots, runtime container and image tag even on failure; `KEEP` is deliberately unsupported for this real-account proof. It never touches real Chauffeur state or provisions GCP.

## What it checks

For each provider and scenario (text, tools and compact for both providers, plus Codex fork, by default), start a real native session on macOS, close the source runtime, copy synthetic history and explicitly selected fixture files to Linux, resume it and close it, then repeat into a fresh macOS environment. Each phase uses a distinct HOME, agent configuration directory and workspace path. Docker mounts only the current phase, never the source workspace or personal directories.

A local deterministic HTTP fixture implements inference responses for the real agent runtime. The first prompt contains a random Unicode marker. Later prompts contain neither that marker nor earlier responses. The fixture captures the native runtime's model input; assertions require the previous user and assistant messages in order, the original marker and an unchanged native session ID. Each phase must also persist its newly completed answer before export. Neither transcript contents nor embedded historical paths are rewritten.

This checks native persistence and context reconstruction without relying on an LLM saying it remembers something. It does not establish model behavior or production-provider compatibility.

The tools scenario makes each real runtime write and read `fixture.txt`. On each destination it must first read the restored contents, then append that host's generated marker through another native write and read it back. The harness compares file bytes before and after native execution. It also compares all earlier completed tool calls/results in the model input, preserving their order, IDs, names, arguments, namespaces when present, and result bodies. By the final Mac turn, each provider must retain exactly eight completed tool results. The fixture provider returns tool requests, never fabricated tool outputs or filesystem effects.

The compact scenario seeds a conversation, then explicitly invokes native compaction on the source Mac: `/compact` for Claude, `thread/compact/start` for Codex. The local fixture answers the runtime's summarization request with a unique source-only summary. The test requires a real compact-boundary/context-compaction event and a persisted native record; it never manufactures those records. It probes active context once after compaction, then transfers and resumes on Linux and Mac. Every destination must retain the exact source summary, opaque native compaction records, post-compaction messages and session ID, without invoking compaction again.

The reference is the source's already-compacted state, not every original message: native compaction itself replaces context. The fixture checks transport and reconstruction fidelity, not summarization quality. It also damages a separate copy by removing compaction records and requires the harness to reject it before any inference request. These negative controls do not modify the successful round trip's source or destination.

## Provider mechanisms

- Claude Agent SDK **0.3.302**: native local JSONL capture/restore through the [selected context adapter](claude-context-bundles.md), followed by `query` with the same `resume` ID and a new `cwd`. Capture follows confirmed exit of its owned native root and rejects malformed records instead of trusting best-effort mirror/import behavior. The installed SDK supplies each host's project-key mapping; only native directory placement changes, with historical record bytes preserved. The tools scenario exposes native Read/Write in accept-edits mode.
- Codex app-server **0.162.1**: the existing typed stdio client, `thread/start`, `thread/resume` by ID and `turn/start`. The [selected context adapter](codex-context-bundles.md) carries the native rollout plus the exact referenced prefixes of any parent rollouts to the next fresh `CODEX_HOME`; SQLite databases, credentials, config, skills and shell snapshots are not copied. The destination recreates its own configuration and uses an explicit new `cwd`. The pinned model's native tool surface is Code Mode: the local provider emits `functions.exec` custom calls invoking the real `exec_command` for generated `printf` and `cat` commands. Text uses read-only mode; tools use full access only inside this deterministic synthetic fixture, not a real model or production workspace. This does not validate sandbox or permission portability.

References: [Claude session storage](https://code.claude.com/docs/en/agent-sdk/session-storage), [Codex app-server](https://learn.chatgpt.com/docs/app-server). These describe provider interfaces; the executable proof pins and checks the locally used runtime versions rather than assuming current documentation is the exact pinned schema.

The fork scenario asks the pinned Codex runtime to fork a completed thread and
requires an actual `history_base` dependency; a flat copied transcript cannot
satisfy it. The child and required parent prefixes resume across all three hosts.
Removing its parent from a separate copy must fail before inference.

## Observed result

On 2026-10-21, all scenarios passed using strict selected native-file adapters for both Claude and Codex,
including the real parent-dependent fork and missing-parent refusal. Text, tools,
compaction and their existing negative controls passed for both providers.


On 2026-10-19, Darwin arm64 → Linux arm64 in Docker → Darwin arm64 passed for both pinned runtimes, for text, native tools and manually compacted context. Each kept one session ID across three checked conversation turns; compaction also uses an initial seed turn and the native compaction operation. The tools round trip retained all eight tool calls/results and cumulative file contents. The compact round trip preserved the full selected native compaction records and summary without resummarization at destinations. A standalone selected Codex rollout was sufficient for these threads; the fixture does not copy the host's SQLite stores.

Also on 2026-10-19, the separate hosted Codex proof passed with `codex-cli 0.162.1`, `gpt-6-luna` and an existing ChatGPT login. The source performed one real native compaction containing an encrypted remote compaction item. Linux and Mac return retained the exact record and session ID, successfully continued with the real service and answered the memory probe; both reported zero new compactions. No token was persisted in generated state. This establishes acceptance of one real compacted session across these hosts, not that the model recovered the identifier exclusively from the encrypted item (native compaction may retain other context), nor general model recall quality.

## Limits and remaining gates

- Text, a narrow completed file-tool sequence and manual local/custom-provider compaction only. No arbitrary commands, unfinished/background processes, subagents, images/attachments, encrypted thinking, queued messages, pending approvals, MCP state, checkpoints or large histories.
- Automatic context-limit triggers, partial/preserved-segment compaction, Jev/plugin compaction and compaction with tool history remain unvalidated. The deterministic fixture does not evaluate real model summary fidelity; the hosted proof covers only one short manually compacted Codex thread.
- The hosted proof uses a still-valid ChatGPT access token with native external auth. It does not validate durable account persistence, refresh, expiration, revocation, Claude cloud authentication, provider policy or authorization of a real GCP worker. The default deterministic fixture inherits no credentials.
- No production daemon session routing, ownership transfer/fencing, Git worktree reconstruction, database migration, cloud provisioning, E2E archive encryption or fault recovery.
- Linux arm64 only in the recorded run; validate the intended worker CPU architecture separately.
- The fixture writes private plaintext context bundles to disk; it is not encrypted cloud storage or a backup implementation.
- The transferred file is an explicit synthetic fixture, not a Git worktree snapshot or a filesystem metadata/permissions/symlink portability test. KEEP=1 also retains the last synthetic provider request per phase for diagnosis.

Do not expose handoff to real workspaces or treat this test as a complete-context guarantee until the remaining gates are validated.
