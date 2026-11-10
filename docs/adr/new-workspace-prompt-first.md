# The new-workspace dialog puts the prompt first

- **Date:** 2026-11-05
- **Status:** accepted

## Context

The creation dialog (⌘N) listed its fields in the order they were added. The import of a shared link came first, though it is rarely used. Three cards explained Workspace, Explore and Design. The tickets came before the repos, with nothing showing that a ticket adds repos. The repo list was 112 px high. Model and permission floated between the repos and the prompt. The first prompt, the main field, came sixth, on one line. Nothing showed the branch before creating. Justin reviewed the dialog in a design chat ("Je pense qu'on peut améliorer l'UX et UI de Chauffeur…") and chose a redesign, including what the daemon must send for it.

## Decision

Decided with Justin 2026-11-05, in the design chat:

- The first prompt comes first, focused at opening, shaped like the chat's composer. Its footer holds attach, the model, and the permission mode ("Read-only" in an exploration, "Design only" in a design).
- Workspace / Explore / Design is a segmented switch in the dialog's header, with one line explaining the kind chosen.
- Repos (Read from, in a design), Tickets and Branch (Checkout in an exploration) are compact rows under the prompt:
  - Repos and tickets are chips.
  - "Add repo" opens a filterable popover. It lists the chosen repos first (Selected), then Recent, the owners and "On this Mac". It says when GitHub last listed the repos. When GitHub fails, it says why, with Retry, and keeps the cached repos and local clones.
  - "Link a Linear ticket" opens a popover with your tickets and the project milestones: the next ones of your projects, those named like the search, or the one a pasted link names.
  - The repos a ticket adds are marked with its id, and go when it is unlinked.
  - "<ID> moves to In Progress when the workspace is created" is said when that will happen.
- The branch is previewed (typed, Linear's for one ticket, the ticket's id and title, the name, an imported share's title, or "named from the first prompt") and edited in place with the name. "More options" and the free-text milestone go away.
- "Import a shared workspace…" (or "Import a shared design…" in Design) is a secondary action at the bottom left.
  - A pasted workspace share is previewed before creating (title, owner, conversations and notes), read with the user's identity as the import reads it.
  - A shared design imported in Design becomes the new design's single chat, with the first prompt.
- A branch that another worktree of a chosen repo has checked out, or an invalid name, is refused before anything is created, naming the branch field, with a free `<branch>-N` suggested. A branch that only exists, here or on origin, is still taken as is (resuming a PR's branch).
- A refused creation keeps the draft below an error banner, and the primary button reads "Try again". With no repo, the Repos row turns red and creating is disabled.

## Consequences

- `repos.list` returns `refreshed_at` and `github_error`.
- `linear.search` takes `milestones` and returns `LinearMilestone`s.
- `workspace.share_preview` is new.
- `workspace.create` refuses a held or invalid branch with `data: { field: "branch", suggestion }`.
- These bump the client protocol to version 8.
- The app mirrors the daemon's slug and branch rules (`app/src/lib/newWorkspace.ts`) to preview the branch: a change to one changes the other.
