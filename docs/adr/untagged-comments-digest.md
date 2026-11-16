# Telling the owner of comments that do not mention them, after a quiet half hour

- **Date:** 2026-11-16
- **Status:** accepted

## Context

A comment on a shared design or workspace that mentions the owner is told at once ([mentions-in-share-comments](mentions-in-share-comments.md)). One that does not is told only as a toast or a notification while the owner's Mac is awake and the app runs, and a Mac asleep tells of the last one at wake. Justin asked to be told of untagged comments too, without a notification for each message of a review session.

## Decision

Asked by Justin, decided with Justin 2026-11-16:

- **A tag is told at once, as before.** Nothing changes for a comment that mentions the owner.
- **The rest is told after 30 minutes without a new comment on the share.** The delay is fixed. The notification says who commented and how many comments wait ("Alice and Bob left 3 comments on CTA"), and opens the share's page. It adds to the toast or notification a comment already makes while the Mac is awake; it does not replace it.
- **Which comments.** Those by someone other than the owner, that do not mention the owner (they may mention someone else), that the owner has not answered in their thread since, and that were less than a day old when the service first saw them. A comment deleted before the notification leaves it.
- **Where.** In the app only (a toast, or a notification when the window is not in front); no e-mail and no push to the phone.

## Consequences

- The share service decides, since it sees every comment and stays online while the owner's Mac sleeps. It keeps the waiting comments in a table of its own and looks every minute for shares quiet for 30 minutes. It tells the owner's daemon over the user WebSocket (`Digest`); with no daemon connected, the comments wait and are told at the next connection, which is also when a Mac asleep wakes.
- Each comment is told at most once. A share's comments are told together, once per quiet period.
- Left for later: an e-mail for an owner without Chauffeur, a delay the user sets, a cap on the wait for a thread that never goes quiet, and telling a comment of a thread the owner opened.
