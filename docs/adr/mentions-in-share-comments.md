# Mentioning people in the comments of a shared design or workspace

- **Date:** 2026-11-08
- **Status:** accepted

## Context

A comment on a shared design or a shared workspace reaches the owner, but a reviewer or a guest is told of nothing: whoever is not on the page never learns that a thread needs them. Justin asked to tag people in those comments. Reviewers do not see who else a share names ([0029](0029-design-comments-and-shares.md): "never who else is named"), so there was nobody to propose.

## Decision

Asked and decided by Justin 2026-11-08:

- **Mentions.** Typing `@` in a comment, on a shared design (the app's canvas and the reviewer page) or on a shared workspace (a guest's comment on a passage), lists the people who have access to that share, filtered by name and e-mail: the owner and the invitees, and, on a share open to a team domain, any address of that domain typed in full. The comment's text keeps a readable `@Name`; the comment carries `mentions: [{email, name}]` beside it. A local design that is not shared proposes nobody.
- **Invitees see each other.** Every invitee of a share sees the names and e-mails of the owner and of the other invitees (in the mention list and in the page's "Shared with"), as in Google Docs. This supersedes "never who else is named" in [0029](0029-design-comments-and-shares.md) for the share's own page; `design.shared` still lists shares, not their invitees.
- **No access, no mention.** The owner who types an address without access is offered "Invite and mention": it adds the invitee (their invitation e-mail is sent as for any new invitee) and mentions them. A reviewer or guest cannot give access: "Bob doesn't have access. Ask Alice."
- **Notified in the app, not by e-mail.** A mention is told in Chauffeur:
  - the owner, mentioned on their own share: "Bob mentioned you" in place of the comment's usual toast or notification, opening the canvas or the chat on the thread;
  - a mentioned person who uses Chauffeur, signed in with the mentioned address: a notification on their Mac, and a Mentions list of the unread ones in the app; each opens the share's page on the thread. Their paired phone gets a generic push ("You were mentioned"), never the comment;
  - a mentioned person on the share's page only: a toast and a badge while the page is open, and on their next visit "N mentions for you", leading to the threads.
  No e-mail for a mention for now: someone without Chauffeur who does not come back to the page is not told. An e-mail as a fallback is left for later.
- **Once per person and comment.** A mention notifies each person once per comment; one added by an edit notifies, one removed before it is seen disappears from the lists. A mention is seen once its person opened the thread, on the page or from the app.
- **The owner comments on a shared workspace too** (decided with Justin 2026-11-09). On a shared workspace, the owner, in the app, comments on a passage of an agent's message as a guest who can suggest does, and the owner and those guests reply to a comment, so that a mention there can be answered. Viewers still do not comment.
- **Order.** Shared designs first, then shared workspaces with the owner's comments and the replies.

## Consequences

- The share service is the only place that sees every comment and stays online while the owner's Mac sleeps: it detects new mentions (in a reviewer's `Comment`, and in the `Comments` the daemon sends whole) and keeps them in a table of its own (share, comment, person, at, seen at), so that a Mac asleep gets them at wake. It keeps the name of each invitee who signs in with Google; until then, an invitee is shown by their e-mail.
- A daemon signed in keeps one more outbound WSS to the service, its user's own (`Hello` with a fresh ID token): the unread mentions whole at connect, each new one, `Seen` back; the app reads them from the daemon. Signed out, or without `[design_share] url`, none of it runs.
- `comments.json`, the design comment calls (`design.comment_add|edit`) and the share sockets grow an optional `mentions`; the share protocol revision in `Hello` keeps older daemons working (they ignore it). The agent reads the text with its `@Name` and nothing else changes for it.
- Links to a thread: `/s/<id>?thread=<comment>` and `/w/<id>?chat=<chat>&comment=<comment>`; the mobile review already targets a thread.
- This lifts "replies" and "the owner answering a comment" from what [share-guest-deletions-and-comments](share-guest-deletions-and-comments.md) left undone. A workspace comment grows `reply_to`; the owner's comments go from the daemon to the service as the guests' come from the page, and the agent still never reads them.
- Left for later: e-mail for mentions (an invitee without Chauffeur who does not come back to the page is not told: accepted by Justin 2026-11-09), mentioning in suggestions (they go to the agent as the owner's message), comments by viewers, mentioning the agent.
