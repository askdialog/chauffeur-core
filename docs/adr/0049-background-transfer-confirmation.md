# 0049. Transfer confirmation closes immediately

- **Date:** 2026-10-29
- **Status:** accepted

## Context

Waiting for transfer startup in the confirmation dialog blocks use of the app while a machine is prepared.

## Decision

Asked by Justin on 2026-10-29: transfer preparation must run in the background so the user can continue working. Confirmation closes immediately for a single workspace, a batch or a return.

## Consequences

The sidebar’s Transfers button opens progress, startup errors and retry actions. Opening that progress view is optional; the daemon continues the transfer when it is closed.
