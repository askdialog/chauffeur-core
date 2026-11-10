# 0015. Attachments and the images agents look at

- **Date:** 2026-10-16
- **Status:** accepted

## Context

Screenshots and files are part of a conversation, as in Conductor, and the user should see the images the agent looks at (requested by a user, 2026-10-15).

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 39:

> As a developer, I want to paste, drop or pick images (PNG, JPEG, GIF, WebP) into a message, see them in the chat, and have the agent see them (Claude as image blocks, Codex as local images), kept in the notes folder, so that I share screenshots as in Conductor. Any other file (a zip, an HTML page, a log; up to 20 MB) is attached the same way: kept in the notes folder, shown in the chat by its name and size, and named to the agent with its path for it to open; the daemon never serves it over HTTP (requested by Justin, 2026-10-16; a design chat's screens, and the workspace's files the Files tab previews (story 43), are the only HTML the daemon serves, on an origin of their own, story 47). The images the agent looks at show in the chat too: a tool result's (a screenshot, an image file it read, Codex's image viewer) as thumbnails under the tool, kept in the notes folder the same way; and an image its answer names in markdown (`![](shot.png)`, absolute or under the workspace folder), which the daemon serves only if its bytes are an image (requested by a user, 2026-10-15).

## Consequences

Attachments are kept in the notes folder. The daemon never serves an attachment over HTTP and serves an image only if its bytes are one; design screens and the Files tab's HTML previews are the only HTML it serves, on an origin of their own ([0018](0018-design-chats.md)).
