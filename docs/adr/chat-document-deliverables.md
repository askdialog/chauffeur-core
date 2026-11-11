# HTML and PDF deliverables stay in their conversation

- **Date:** 2026-11-10
- **Status:** accepted

## Context

Justin requested a styled one-pager and received Markdown, a twelve-slide deck and then interface mockups. The Files tab can preview HTML and PDF, but design chats explicitly produce application screens on a canvas.

## Decision

Decided with Justin 2026-11-10: documents are deliverables of their normal conversation. Requests to improve styling preserve the requested format, language and page count. The agent chooses HTML, PDF or both according to the user's request and intended use; no format is forced when unspecified, and both are not automatically generated. HTML is self-contained; PDFs are verified. Interface mockups continue to use Design.

A document card opens an HTML or PDF reader beside its chat, with download, reload, expand and close. A format switch appears only when the agent has produced multiple versions. Follow-up changes continue in that chat. Start with HTML and PDF; dedicated slide presentation and editing are later work.

## Consequences

Agents declare checked deliverables in a `deliverable` JSON fence with `title`, `path` and optional `exports` (paths). Files live under `.chauffeur/artifacts/`; declarations survive in the existing conversation history. The primary path identifies the document across revisions; exports listed by the agent must be regenerated with it. No separate document database or version history is introduced.

The preview reuses the daemon's HTML and PDF serving, reloading after turns and reconnection. HTML stays on an isolated origin in a sandbox; ordinary file links keep their existing behaviour. Downloads use single-use 60-second document tickets issued through the authenticated `files.download` RPC; the system browser never receives the daemon token. They are scoped to workspace roots, restricted to HTML/PDF artifacts, capped at 32 MB and forced to attachments. Sharing is explicit through the existing Share dialog: the selected HTML or PDF is shared alone with named reviewers or the team, on the same service as existing shares. HTML/PDF document links use the workspace share host (`share.chauffeur.askdialog.com/s/<id>` in Dialog), including invitations and sign-in links. Recipients see a document reader, not a canvas; updates follow the file and Stop sharing revokes access. HTML stays sandboxed and self-contained. Markdown retains its passage comments; HTML/PDF initially provide viewing and download. Deploy the share service before the desktop app/daemon: older services do not accept HTML/PDF document shares. Markdown links keep their existing host. On narrow panes the reader appears below the conversation and can expand.
