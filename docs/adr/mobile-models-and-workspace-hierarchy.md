# Mobile model selection and the Mac's workspace organization

- **Date:** 2026-11-08
- **Status:** accepted

## Context

Louis's first four mobile feedback items concern landscape orientation, choosing
models, the Mac's folders and parent/child workspaces. Justin requested their
implementation on 2026-11-08.

## Decision

The companion supports portrait and landscape and displays the Mac's pinned
workspaces, ordered sections and parent/child hierarchy. Section and child folds
can be changed locally for navigation; search reveals matching descendants and
their ancestors. This does not add workspace or section mutations on the phone.

An approved phone can read `models.list` scoped to a workspace and call
`chat.configure` with model, effort, fast mode and provider confirmation. Switching
providers shows a confirmation and follows the daemon's existing busy/session
rules. The mobile host rejects permission-mode fields and any unrelated settings.
Model catalogs come from the execution host, including for a moved workspace.

## Consequences

The mobile host's allowlist must be updated for model selection; an older host
gets an update message on the phone. The wire schemas already exist, so the mobile
SDK snapshot need not change. Portrait/landscape configuration requires a new
native build. JavaScript tests and browser renders do not establish rotation,
keyboard or safe-area behavior on a physical device.
