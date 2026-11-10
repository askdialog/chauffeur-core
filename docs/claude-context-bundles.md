# Selected Claude native context

`sidecar/src/agents/claude/context-bundle.ts` captures a selected stopped session's
native files. It does not use transcript mirroring as final evidence: the pinned
SDK's mirror is best-effort, and its local import helper skips malformed records.
This adapter rejects malformed/truncated JSONL instead of omitting any line.
It has no production RPC or ordinary-chat caller.

## Selection and preservation

The caller explicitly supplies a canonical Claude `projects` root, the selected
SDK project key and native session UUID. Capture reads only `<id>.jsonl` and the
selected `<id>/subagents/` transcripts/metadata, including nested scopes/journals,
and `<id>/tool-results/*.txt` payloads persisted for large tool outputs.
It does not scan other conversations or copy a personal home, credentials,
configuration, skills, global indexes or other project files.

Native bytes remain unchanged, including compaction boundaries, tool records,
subagent metadata, UUIDs, field order and historical paths. Each JSONL line must
be a JSON object with a type; the main transcript must contain a user/assistant
message belonging to the selected native session. Empty/system-only or obviously
mismatched main transcripts are refused. This check is not proof of a complete
message chain or an authenticated source-stop receipt.

The bundle has a version, native session UUID, source project key, relative file
paths, base64 bytes and SHA-256 digests. Metadata without its corresponding
subagent/journal transcript is refused. Tool-result payloads retain their exact
bytes and are checked for integrity and file safety, without JSONL parsing. They
use the same file, size and bundle limits as transcripts. Unsupported files/directories
inside the selected session tree fail instead of silently disappearing. Historical
absolute paths inside messages remain unchanged; carried tool results live under
the destination's native project/session folder. Other external dependencies referenced by records still
require preflight: attachments, file-history checkpoints, MCP/tool environment and
other required data are not reconstructed by this adapter.

## Relocation and durability

`claudeProjectKey(cwd)` obtains the installed SDK's own key mapping through its
public session-store read interface with an empty in-memory store. It does not
read personal history, invoke inference, mutate environment variables or guess a
path sanitizer/hash. The requested cwd must be canonical and already exist.
A changed/unsupported SDK mapping fails rather than inventing a key.

Restoration validates every component and selected session before writing. It
creates only a **new** private project namespace under the caller's explicit
projects root, using 0700 directories and 0600 files with file/directory fsync.
Only directory placement changes to the destination project key; native record
contents are never rewritten. Existing namespaces are never overwritten/merged
or removed on error. A failure removes only the incomplete newly created root.
`restoreClaudeContexts` restores several explicitly selected conversations into
that new namespace together, enforcing shared limits and refusing duplicates.

`agents/context-files.ts` shares the existing Codex file boundary: no-follow
opens, canonical roots/ancestors, regular single-link files, stable file identity,
bounded reads and repeated content checks. Limits are 64 MiB per file, 256 MiB
and 512 files per bundle/batch, with bounded selected-tree traversal. Symlinks,
hardlinked selected files, traversal, invalid UTF-8, damaged hashes and partial
records fail. This requires a trusted stopped/fenced source; rechecks are not
containment against arbitrary concurrent writers.

The artifact is sensitive **plaintext**. Digests detect corruption, not forgery.
Authenticated operation binding, encryption before cloud storage/transport, full
source writer/service quiescence, target ownership and runtime readiness remain
caller prerequisites. No snapshot/worker execution permission is issued here.

## Evidence and limits

Isolated tests cover exact main/nested-subagent metadata and persisted tool-result bytes, strict JSONL,
unsupported files, orphan metadata, wrong-session messages, corrupted/duplicate/
unsafe components, existing destinations, symlinks, native SDK key mapping and
multi-conversation restoration. Existing Codex tests run against the shared file
helpers unchanged.

The portability harness uses native local files rather than its former synthetic
Claude session store. Each query's owned `ClaudeProcess` must confirm root exit
before capture; iterator completion alone is not used as exit evidence. On the
next host the native files are restored into a fresh config/project namespace
before `query({ resume: sameId, cwd: newCwd })`. The pinned runtime, not a summary
adapter, reconstructs the conversation. This root-stop observation still does
not prove detached descendants or external writers stopped.

The Mac–network-isolated Linux–Mac proof checks text, native file tools and native
compaction for Claude and Codex, plus Codex's parent-dependent fork. It compares
ordered model inputs, native session IDs, exact source compaction records and
selected file/tool results. Missing compaction is rejected before inference.
The inference fixture is local/deterministic; this is not real-provider account
validation, a live subagent-resume proof, an attachment/checkpoint portability
proof or complete workspace handoff. Production export/routing, authenticated
transport, selected access transfer, GCP execution and conflict-aware return
remain integration work.
