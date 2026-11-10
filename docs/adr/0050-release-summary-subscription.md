# 0050. Release summaries through a Claude subscription

- **Date:** 2026-10-29
- **Status:** accepted

## Context

The automated release draft had no French summary because its Anthropic API key
was not configured. Justin asked to use a Codex or Claude subscription instead,
then configured the proposed Claude Code OAuth token in GitHub Actions.

## Decision

Use Claude Code with `CLAUDE_CODE_OAUTH_TOKEN` to write French release summaries
from the Claude subscription, without separate API-key billing (decided with
Justin 2026-10-29).

## Consequences

The release runner installs Claude Code and passes the subscription token only to
the summary process. It runs without tools or repository customizations. Usage
counts toward the subscription limits. Missing or expired credentials and failed
summaries retain the complete change list for human review. Publication stays
manual.
