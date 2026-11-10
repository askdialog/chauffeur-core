# Repos of every GitHub owner the account reaches, picked in a grouped picker

- **Date:** 2026-11-04
- **Status:** accepted

## Context

A workspace's repos came from one GitHub organization, the `github_org` setting. Personal repos
(a personal repo of the user's account) and those of other organizations could not be picked without switching the
organization for everything. The picker also showed "· not cloned", an implementation detail
(cloning happens on first use, story 6), in a list four rows tall. Justin asked for a better
GitHub integration. The redesign of the creation dialog's Repos field was approved from the design
chat "Design the new GitHub repo picker".

## Decision

Decided with Justin 2026-11-04:

- The picker lists the repos of every owner the GitHub account reaches: its own, its
  organizations' and the ones it collaborates on (`gh api user/repos`). It also lists the main
  organization's, through `gh repo list`, which reaches an organization the account is not a
  member of. Archived repos are left out. `github_sources = false` lists only the main
  organization's.
- `github_org` becomes the main organization: its repos are named by their short name, as before.
  A repo of any other owner is named `owner/name`, in requests (`workspace.create`,
  `workspace.add_repo`, `chauffeur new --repo`, the agent's `list_repos`/`add_repo`) and in
  `default_repos`. A short name that only one other owner has still resolves to that repo. A short
  name that several other owners have is refused, and the error names the candidates.
- A repo keeps its short name as its folder inside a workspace. Until the repo identity moves to
  `owner/name` everywhere, a workspace holds one repo of each name. Two repos of the same name are
  refused at creation.
- Chauffeur clones another owner's repo into `~/chauffeur/repos/<owner>/<name>`. A clone under the
  clone roots is reused for another owner's repo only when its `origin` is that repo. A folder name
  alone may be someone else's repo.
- The picker:
  - The owner is shown once: by its section, or by a faint `owner/` prefix in mixed lists (Recent,
    chips). The prefix appears outside the main organization, or when two owners have a repo of
    the same name.
  - Sections come in this order: Recent (the repos of the last workspaces, at most four, not
    repeated below), each owner (the main organization first, then the account, then the others),
    then "On this Mac" (clones under the clone roots that no owner lists, tagged `local`).
  - A filter per owner sits above the list. Recent is hidden while an owner is filtered or a search
    is typed.
  - Each row shows: checkbox, avatar (square for an organization, round for a person), name, a
    lock for a private repo, the description and the last push.
  - The list holds about eight rows. ↑↓ move a ring, ↵ picks, ⌘↵ still creates.
  - A pasted GitHub URL (or a typed `owner/name`) that no owner lists is offered as "Use
    github.com/owner/name". It is cloned for the workspace.
  - The footer says "Missing a repo? Paste its GitHub URL · Check access". Check access opens
    GitHub CLI's application page, where organizations (SSO ones included) grant it access.
  - No clone state shows anywhere: cloning shows in the workspace's setup lines.

## Consequences

- The repos cache is keyed by `owner/name` (`github_repos`), and the old one is dropped. A listing
  for another main organization or another `github_sources` value lists GitHub again.
- `RepoInfo` gains the request key, the owner and whether it is an organization, privacy, `local`,
  and when a workspace last took the repo in (from its worktrees' clone).
- An agent worktree of a repo the workspace has comes from that worktree's clone, whoever owns it.
- Left for later: Settings › GitHub "Sources", with a checkbox per owner, replacing the single
  organization step, and SSO organizations flagged with Authorize. Also left: streaming each
  owner's section while GitHub answers, a pasted repo's description and its no-access state, and
  the move to `owner/name` as the identity of a workspace's repos, which would allow two repos of
  the same name in one workspace.
