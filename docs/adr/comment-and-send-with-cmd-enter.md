# ⌘Enter writes a design comment and sends it to the agent

- **Date:** 2026-11-04
- **Status:** accepted

## Context

A comment on a design's screen is written with Enter (or the bubble's Comment button) and stays new until "Send N new to the agent", in the Comments panel, sends it. Giving one remark to the agent then takes a comment, opening the panel and a click. Justin asked for a shortcut that writes the comment and sends it at once.

## Decision

In a design chat's canvas, full screen and Present, ⌘Enter in a new comment's bubble or in a reply writes the comment, then sends the new comments to the agent as "Send N new to the agent" does (asked by Justin 2026-11-04). Enter still only writes it. A reviewer's page has no such shortcut, since nothing goes to the agent from there.

## Consequences

- The send is the panel's: every new comment goes in the one message, not only the one just written, and it is queued for after the agent's turn when it works.
- The bubble's hint names the shortcut (`⌘⏎ send`) beside Enter and Esc. Ctrl+Enter does the same, for keyboards without ⌘.
- Editing a comment ("Save") keeps Enter only: an edit is of a comment not sent yet, sent with the others.
