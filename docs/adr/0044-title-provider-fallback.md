# 0044. Fall back to Codex for workspace titles

- **Date:** 2026-10-29
- **Status:** accepted

## Context

Workspace naming uses Claude Haiku even for Codex chats. When Claude's quota is exhausted,
its error message can become the workspace title instead of describing the work.

## Decision

Asked by Justin on 2026-10-29: when Anthropic is unavailable, including exhausted credit,
use a small GPT model such as Luna to name the workspace. Apply this fallback to initial
naming and reconsidering a title, automatically or at the user's request. Reject provider
errors as titles.

## Consequences

Haiku remains the first choice. If it fails or times out, the sidecar tries Luna through
the user's Codex login (GPT-6.1 Luna when listed, otherwise GPT-6 Luna). Naming has a
45-second overall deadline to allow both attempts. Creation still waits at most 15 seconds
for the folder/branch name; a later answer updates only the shown title. If neither provider answers, existing
failure handling keeps the generated name or current title. Existing folders and branches
are never renamed by this fallback.
