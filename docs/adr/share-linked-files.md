# A shared markdown file can carry the files it links to, on the same link

- **Date:** 2026-11-13
- **Status:** accepted

## Context

A markdown file is shared alone ([comment-and-share-markdown-files](comment-and-share-markdown-files.md)): one share, one document. A recap that points at its detail files (`MODELISATION.md`, `AUDIT_CLIENTS.md`…) reached its reviewers with those names as dead text, and the Share dialog never offered to send them along. Justin asked for a checkbox to share the linked files too.

## Decision

Asked and decided with Justin 2026-11-13:

- The Share dialog of a markdown file offers "Also share the linked files", with the files listed beneath. It is checked by default when the file links to any; it does not show when it links to none. `chauffeur share-file --linked` does the same.
- The linked files travel on the file's own link: one share, one invitation. On the reviewer's page a linked file's name opens that file in the same page, with its own passages' comments, and a way back to the file shared.
- Linked files are found recursively: the markdown files of the workspace (repos or notes) that the file names, then the ones those name, and so on.
- The share follows the links live: a file linked after the share started joins it; one no longer linked leaves it.

## Consequences

- A file is linked when the shared file names it as the app's Files tab does (a markdown link or an inline code span: a path under the workspace, a path inside one repo, a base name unique in the workspace) or by a path relative to the file's folder. Only markdown files count; images, code and HTML do not travel.
- The daemon finds the linked files, at most 50, and pushes them beside the file with a `links.json` manifest that tells the reviewer's page what each name opens. The service is unchanged: it already holds several files per share.
- Each file's comments stay on its own path in the notes' `comments.json`: a reviewer's comment on a linked file shows on that file in the Files tab, and is sent to the chat that wrote it.
- Not done: other kinds of linked files (images, PDFs); choosing some of the linked files and not others.
