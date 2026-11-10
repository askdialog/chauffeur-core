# A worker gets the Mac's Claude Code configuration on each send

- **Date:** 2026-11-02
- **Status:** accepted

## Context

An agent on a GCP worker ran without the user's Claude Code configuration: no global `CLAUDE.md`, no skills, no plugins (the team's skills among them), no hooks, no user MCP servers. The first real trial (2026-10-31) found an agent there unable to follow a pull request with the skill it uses on the Mac. [worker-github-and-aws-access](worker-github-and-aws-access.md) gave the worker the Mac's GitHub and AWS access; Justin asked for the Claude Code configuration too.

## Decision

Decided with Justin 2026-11-02: on each transfer to a GCP worker, before anything stops, the Mac's daemon copies its Claude Code configuration there: `CLAUDE.md`, `settings.json`, `skills/`, `agents/`, `commands/`, `hooks/`, the installed plugins (`plugins/installed_plugins.json`, `known_marketplaces.json`, `cache/`, `marketplaces/`), and the user MCP servers of its state file (`.claude.json`'s `mcpServers` only). Never the login, the transcripts (`projects/`), the history, sessions, caches or backups. The agents' logins stay one per worker.

## Consequences

- The configuration goes as one archive (tar, zstd), in chunks through private peer methods (`worker.claude_config_status`, `_write`, `_install`); the worker's daemon checks its digest and puts each item in place itself, a folder whole and a file by a rename. The worker keeps the digest it installed: an unchanged configuration is not sent again.
- The Mac's configuration folder and home in `settings.json`, the plugin lists and the MCP servers become the worker's; plugin and skill contents are copied as they are.
- An MCP server served on the Mac's own address (`localhost`, `127.0.0.1`, `::1`), or whose command the worker does not have, is left out with a warning in the transfer's log. The headers written in an MCP server's configuration travel with it; one signed in by OAuth needs its sign-in on the worker, whose tokens are not copied.
- `settings.json` and `.claude.json` are owner-only there (`0600`), the configuration folder `0700`. The worker's other `.claude.json` keys (its own login) and its own MCP servers are kept.
- A link leaving its folder (a plugin's absolute one) is not copied, and the worker refuses an archive with one, an absolute path, a `..` or an item outside the list.
- A failure is a warning in the transfer's log, never a refusal. Machines declared by hand in Settings → Transfer and the way back get nothing.
