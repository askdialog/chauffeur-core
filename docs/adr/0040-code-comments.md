# 0040. Render code review comments in chat

- **Date:** 2026-10-29
- **Status:** accepted

## Context

Justin reported Codex review comments appearing as raw `::code-comment` directives in the desktop chat.

## Decision

Decided with Justin 2026-10-29: render each code comment as a card with its title and priority, readable description and a clickable file and line range. Technical fields such as confidence do not clutter the card. Apply the rendering to existing messages too.

## Consequences

The desktop Markdown renderer interprets complete directives without changing stored messages. Incomplete or malformed directives and code examples stay visible as source. Search and rich HTML copy use the readable content; plain text copy keeps the original Markdown source.
