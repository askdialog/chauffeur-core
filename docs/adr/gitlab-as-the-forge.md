# GitHub by default, a self-hosted GitLab when the installation is set to it

- **Date:** 2026-11-18
- **Status:** accepted

## Context

Chauffeur reaches one forge, GitHub, through `gh` ([0051](0051-github-without-homebrew.md)): the
account, the repos of its owners, clones, pull requests and their merge, the branches deleted on
archive, the search's pull requests, cloud workers' access and the agents' own skills. A team whose
code lives on a GitLab it hosts itself cannot use any of that; only what goes through `git` works.

## Decision

Decided with Justin 2026-11-18:

- GitHub stays the default: an installation that sets nothing behaves as today, and installs nothing
  more.
- The forge is a setting of the installation, not of a repo: set to GitLab with its host, the whole
  installation goes through that GitLab instead of GitHub, as it goes through one `github_org` today.
  A workspace does not mix GitHub and GitLab repos.
- Chauffeur reaches GitLab through `glab`, GitLab's CLI, as it reaches GitHub through `gh`: the
  user's own when on the PATH, else one Chauffeur installs. `glab` is installed only when the
  installation is set to GitLab, never for a GitHub one. The token stays in `glab`; Chauffeur never
  stores it.
- The words follow the forge connected: "pull request" and "PR" with GitHub, "merge request" and
  "MR" with GitLab, wherever Chauffeur names them (the app, the CLI, what the agents are told).

## Consequences

- What reaches GitHub today goes through one forge chosen by the setting, GitHub's being the
  current code: signing in (`glab auth login --hostname <host>`, then git over HTTPS through it),
  listing the repos (projects of the groups the account reaches, in place of owners), telling a
  clone's `origin` (a GitLab path may hold subgroups: `group/subgroup/project`, not `owner/name`),
  merge requests in the header (list, state, pipelines as checks, approvals as the review decision,
  merge, stacked requests), the branches deleted on archive, the search's merge requests.
- Signing in takes a personal access token the user pastes (scopes `api` and `write_repository`):
  GitLab's sign-in in the browser needs an application registered on each self-hosted instance,
  which Chauffeur cannot count on. The daemon hands the token to `glab auth login --stdin`, which
  keeps it in the system's keyring and lets git push over HTTPS through it; the token is never
  written, logged or sent back by Chauffeur.
- `glab` is pinned and checked like `gh` and Ollama: a version and its SHA-256 written in the daemon,
  unpacked into the state folder, last on the PATH of everything Chauffeur starts.
- Explorations and design chats allow the GitLab host on the network in place of GitHub's, and their
  git and forge write guard refuses `glab mr create`, `glab api -X POST` and posts to the GitLab
  API as it refuses `gh`'s.
- Cloud workers get the Mac's `glab` access as they get its `gh` access; a GitLab reachable only
  from the user's network (a VPN) is out of a GCP worker's reach, which says so.
- The agents' instructions and the team skills (open a pull request, shepherd it) name merge
  requests and `glab` on a GitLab installation.
- Left for later: a GitLab on gitlab.com rather than self-hosted (the same code, its host being
  gitlab.com), and other forges.
