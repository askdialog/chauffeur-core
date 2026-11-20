# Shared HTML documents take passage comments

- **Date:** 2026-11-20
- **Status:** accepted

## Context

The shared Markdown reader let reviewers select and comment on a passage. The shared HTML reader only offered viewing and download. Justin asked to align the two.

## Decision

Reviewers can select a passage in a shared HTML document and comment on it. The reader shows highlighted passages and their threads, with the same replies, mentions and resolution as for shared Markdown. Threads whose passage disappears after an update are marked outdated.

## Consequences

The HTML stays sandboxed. The reader exchanges selection and highlight information through a channel set up inside the sandbox. Comment editing, posting and synchronization use the existing file-share controls and protocol. Shared PDFs keep their viewing and download controls.
