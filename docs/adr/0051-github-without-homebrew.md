# 0051. GitHub without Homebrew: Chauffeur's own `gh`, signed in in the browser

- **Date:** 2026-10-30
- **Status:** accepted

## Context

Chauffeur reaches GitHub through `gh` (organization repos, clones, pull requests, the agents'
open-pr and shepherd-pr skills) and expected the user to have installed it, with Homebrew, and
signed it in in a terminal. Someone without Homebrew or `gh`, a non-developer say, was stuck.
Justin asked for a GitHub connection a non-technical user finds obvious.

## Decision

Chauffeur keeps going through `gh` and makes it its own business (decided with Justin
2026-10-30, who preferred it to an OAuth application of Chauffeur's own with a token Chauffeur would
store, since the agents need `gh` anyway):

- When no `gh` is on the PATH, Chauffeur installs its own, pinned like Ollama (2.98.0, an upgrade
  is a deliberate change of the version and its checksums), from GitHub CLI's release, checked
  against its SHA-256, in the state folder; `~/.chauffeur/bin` comes last on the PATH of everything
  Chauffeur starts, so a `gh` the user installed keeps coming first and the agents find either.
- "Connect GitHub" signs `gh` in through GitHub's device flow: the app shows the one-time code
  with a button that copies it and opens GitHub's page; the user clicks Authorize. HTTPS as git's
  protocol, the `workflow` scope (a push touching CI files), then `gh auth setup-git`, so git
  pushes over HTTPS through `gh` with no SSH key. The token stays in `gh`'s keyring: Chauffeur
  never reads or stores it.
- Everything a new user lacks is covered, wherever it blocks: git on macOS (Apple's command line
  developer tools, whose installer Chauffeur opens), the account, then the organization the repos
  come from, picked among the account's. The first screen, the creation dialog and Settings ›
  GitHub show the same checklist; `chauffeur github [login | logout]` does it from a terminal.

## Consequences

- `gh`'s OAuth application is GitHub CLI's: an organization that restricts third-party
  applications must allow it, as it would for a `gh auth login` in a terminal.
- `gh auth setup-git` writes the user's `~/.gitconfig` (a credential helper for github.com that
  names `~/.chauffeur/bin/gh`, a link that outlives an upgrade). A clone whose remote is SSH keeps
  using SSH.
- Linux: git is installed by the user (the command is shown); `gh` stores its token in a file when
  no keyring answers, as it does in a terminal.
- Cloud workers keep their own `gh`, installed with apt and signed in as before.
