# Focused mobile navigation and separate preferences

- **Date:** 2026-11-08
- **Status:** accepted

## Context

Justin requested implementation of the approved “UX mobile — inspiration Linear”
screens, including parent workspaces, conversations, model selection, pairing and
preferences, on 2026-11-08.

## Decision

The mobile home lists main workspaces only, preserving the Mac's sections and
pins. Descendant activity contributes to a parent's status and filters. Searching
for a child presents its parent as the entry point; children are reached within
their parent and retain that context in conversation navigation. Opening a
workspace no longer skips directly to its sole conversation.

An active child whose parent is unavailable remains reachable through a recovery
row explicitly identifying the missing parent. Malformed cycles also keep a
reachable entry point; filtering the normal hierarchy must never hide active work.

Conversations use compact headers, an editable dictation composer, contextual
requests and a focused model sheet. Changing providers still requires explicit
confirmation. Connection, notification preferences and language have distinct
screens. Pairing still requires approval on the Mac; forgetting a Mac requires
confirmation and only clears local pairing and loaded content.

## Consequences

This changes client presentation and navigation, not the mobile host's authority.
Offline views may show their last in-memory snapshot but cannot queue writes.
No global inbox, workspace mutation or GitHub management is implied by these
screens. Browser render checks do not validate physical camera, microphone,
keyboard, rotation, push delivery or operating-system link handling.
