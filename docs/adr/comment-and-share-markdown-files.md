# A markdown file is commented on, shared alone, and its comments are sent to the chat that wrote it

- **Date:** 2026-11-09
- **Status:** accepted

## Context

An agent's plan, or any document it writes, is a markdown file: a note under `.chauffeur/` (plans in `.chauffeur/plans/`) or a `.md` of a repo. To challenge it, the user had to paraphrase it in the chat, and a colleague had nothing to point at: comments existed on a design's screens ([0029](0029-design-comments-and-shares.md)) and on a passage of an agent's message in a workspace share ([share-guest-deletions-and-comments](share-guest-deletions-and-comments.md)), whose comments the agent never reads and which left out comments on a note. Louis asked to comment on the plans an agent writes (2026-11-09); Justin asked to share such a file with him, let him comment and challenge it, and send his comments to the chat to adjust it.

## Decision

Asked by Justin 2026-11-09, from Louis's request, decided with Justin the same day:

- Any markdown file of a workspace can be commented on: a note (plans included) or a `.md` of one of its repos. In the Files tab, the user selects a passage of the rendered file and comments on it, whether the file is shared or not.
- A markdown file can be shared alone: a Share button on the file in the Files tab (and the CLI). The link shows that file only, rendered, updated as it changes on the Mac; no chat, no other note, no other file. It is shared with named people or the team as a design is, through the same service, sign-in and invitations. Its reviewers comment on a passage the same way and reply to comments.
- The user sends the new comments to a chat when they decide, as a design's ("Send N new"): by default to the chat whose agent last wrote or edited the file, another chat picked from a menu. The message names the file and gives each passage quoted with its comments and their authors, for the agent to adjust the file. Nothing is sent to an agent without the user.
- A comment whose passage is no longer in the file stays, marked as outdated.

## Consequences

- The daemon is the only writer of a file's comments on the Mac. They follow a design's rules and format, kept with every file's of the workspace in the notes' `comments.json` (kept on archive), each with the file's path and the passage's text: a passage is found again by its text, so an edited passage makes its thread outdated rather than moving it.
- The chat that last wrote the file is read from the workspace's tool calls naming it (Claude's Write, Edit and MultiEdit, Codex's patches). A write through a shell command is not seen; with no such open chat, the user picks one.
- A file's share is a design share holding that one file: the same service, sign-in, invitations, reviewers, mentions and socket, the reviewer page rendering the markdown with the app's Comments column instead of the canvas. The daemon keys its link by the workspace and the file's path, pushes that file alone and syncs that file's comments with the notes' `comments.json`.
- Not done: comments on a code file or any other file, comments on a note through a workspace share, a guest's comments in a workspace share sent to an agent, the agent resolving a file's comments.
