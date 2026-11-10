# Security

## Reporting a vulnerability

Report it privately through GitHub's private vulnerability reporting: the repository's
**Security** tab → **Report a vulnerability**. Do not open a public issue, pull request or
discussion for it. Say what is affected, how to reproduce it, and what an attacker gains. The
maintainers answer there and agree with you on a fix and a disclosure date.

## Scope

What matters most in Chauffeur:

- **The daemon's local API**: JSON-RPC over a WebSocket on 127.0.0.1, guarded by a token in a
  0600 file and an Origin check, and over a Unix socket only the user can reach. Any way for
  another local user, a web page or an agent to call methods it should not (see `#[access]` in
  `crates/chauffeur-protocol`) is in scope.
- **Agent guards and sandboxes**: an exploration or a design chat writing outside what it is
  allowed, or an agent reaching the daemon's user-only methods.
- **The mobile relay** (`relay/`): it must only forward ciphertext between paired devices; a way
  to read or inject traffic, or to pair without the Mac's approval, is in scope.
- **The share service** (`crates/chauffeur-design-share`): a design or workspace share readable
  by someone it does not name, or a sandboxed screen escaping its sandbox.
- **Credentials copied to workers**: a workspace sent to a GCP worker carries the Mac's agent,
  GitHub and cloud logins; their exposure beyond that worker and its user is in scope.
- **The Slack service** (`slack/`) and its sign-in.

Out of scope: hosted instances run by third parties, and issues in Claude Code, Codex, `gh` or
other tools Chauffeur runs (report those to their own projects).
