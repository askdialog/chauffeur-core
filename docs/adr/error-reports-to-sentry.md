# Error toasts are reported to Sentry, as the user chooses

- **Date:** 2026-11-03
- **Status:** accepted

## Context

When Chauffeur fails in front of a colleague, the error shows in a toast and stays in the logs of their Mac (`~/.chauffeur/logs`). We only learn about it if they think of telling us, and without the daemon's side of the failure. Justin asked for a way to receive these errors, with a setting to send them automatically, ask each time, or never send them.

## Decision

Decided with Justin 2026-11-03:

- An error is what the app shows as an error toast (`toast(…, "error")`). Logs the user never sees are not reported. Errors that are not toasts (a failed turn shown in its chat, the "chauffeurd is not reachable" banner, errors inside a card or a dialog) are not reported either.
- A setting, `error_reports` in `settings.toml` (Settings › General), offers three choices: `always` (sent automatically), `ask` (default) and `never`.
- With `ask`, the error toast carries a "Send report" button and a "Always send" link, which sets the setting to `always`. Such a toast stays until the user closes it, so the button can still be clicked. With `always`, the toast says the report was sent. With `never`, the toast is as it was.
- Reports go to a Sentry project of the build's distributor (for Dialog's releases, a `chauffeur` project in its Sentry organization), as Sentry events built with the types of Sentry's Rust SDK (`sentry-types`), not as OpenTelemetry logs.
- The app hands the report to the daemon. The daemon adds its own context, removes sensitive data and sends the report. The app never talks to Sentry.
- A report carries: the toast's message, the daemon method that failed and its error code, the daemon's error chain and its log lines for that request, the versions of the app and the daemon, the OS, and an anonymous installation id. It never carries the user's name, email or GitHub login, a chat's text, a tool's output or a file's content.

## Consequences

- The daemon tags each failed request with an id that its error response carries. The toast keeps that id, so that the report finds the daemon's log lines for that request.
- Before sending, the daemon replaces the home folder with `~` and masks anything that looks like a token or a key (`ghp_…`, `gho_…`, `lin_api_…`, `xox…`, `Bearer …`, `sk-…`). Only the listed fields are sent; anything else is dropped.
- Identical errors are grouped: the same error is sent at most once every 10 minutes, and at most 50 reports a day, so that a toast repeated in a loop does not use up the organization's Sentry quota.
- Reports that cannot be sent (no network) wait in the state folder and are sent later; they are dropped after 7 days.
- Events, not logs, because events are what Sentry groups into issues. The DSN is not a user setting: it is `CHAUFFEUR_SENTRY_DSN` at build time, which a distribution's release workflow passes to `scripts/package.sh` and the daemon compiles in (`option_env!`). A build without it (from source, the tests, the end-to-end scripts) sends nothing and hides the setting, which keeps Chauffeur's defaults independent of Dialog ([0009](0009-settings.md), [core-open-source-without-dialog-defaults](core-open-source-without-dialog-defaults.md)).
- The share pages raise toasts without a daemon; their toasts are not reported.
- Each new error toast is covered without further work, since every error toast goes through `toast()`.
