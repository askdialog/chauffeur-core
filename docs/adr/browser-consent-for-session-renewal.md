# Open the browser directly for sessions with browser consent

- **Date:** 2026-11-20
- **Status:** accepted

## Context

Louis reported three clicks to renew Dialog CLI while signed in to the dashboard. On a Mac the browser already asks which terminal to authorize and shows the account and environment.

## Decision

Decided with Justin on 2026-11-20. A local login that requires consent in its browser may declare `browser_consent = true`. When its pending request is shown in the focused Mac app, Chauffeur starts the login and opens its page once. The user authorizes the terminal there. The waiting command resumes once its session check succeeds.

## Consequences

Dialog CLI opts in for staging and production. Requests can still be handled explicitly from the phone or CLI. Replacing a local login command in settings requires declaring browser consent again for that command.
