# A guest deletes their suggestion and comments on a passage of an agent's message

- **Date:** 2026-11-03
- **Status:** accepted

## Context

On a shared workspace's page, a guest could not take back a suggestion (a mistyped one stayed in every list, dismissed or not), and had no way to react to what the agent wrote other than suggesting a message to it. Justin asked to delete suggestions and to comment on text, for the owner to see.

## Decision

Asked by Justin 2026-11-03:

- A guest deletes their own suggestion while it waits or once dismissed (a sent one stays: it is in the chat). It leaves every guest's list and the owner's cards; nobody else deletes it.
- A guest who can suggest selects a passage of an agent's message on the page and comments on it. The comment shows under that message, with its author and the passage, to every guest and to the owner in the app, who is told of it as of a suggestion (a toast, or a notification when the window is not in front). The agent never reads it. Its author deletes it; nobody edits it, and nobody replies to it yet.

## Consequences

The share service holds the comments and is their only writer; it sends them whole to the guests and the owner's daemon, which keeps a copy so that the app shows them offline, after a restart and after the share stops. A deleted suggestion stays in both stores as `deleted` so that a daemon that has not heard of the deletion never brings it back. Daemons say a protocol revision in their `Hello`: an older one hears of neither deletions nor comments and keeps working. Not done: replies, comments by viewers, the owner deleting or answering a comment, a badge for comments, comments on a user's message or a note.
