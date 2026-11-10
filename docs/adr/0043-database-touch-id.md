# 0043. Touch ID before non-SELECT SQL

- **Date:** 2026-10-29
- **Status:** accepted

## Context

Justin asked for the behavior of clients such as TablePlus: every query other than a SELECT requires the user's fingerprint before it executes.

## Decision

Require a fresh Touch ID confirmation for every non-SELECT console execution and every batch of staged grid edits, for all environments. The daemon requests macOS LocalAuthentication itself, so the same rule covers the app and CLI. There is no reusable approval token or client flag that enables writes.

Cancellation, a missing or locked biometric device, or an authentication timeout refuses the operation. This policy has no password fallback and cannot be disabled in connection settings. Production's default read-only setting still applies after authentication.

## Consequences

Parsed SELECT queries, including read-only CTEs and unions, do not prompt and execute in PostgreSQL read-only transactions. Writing CTEs, SELECT INTO, row locks and statements the parser cannot classify conservatively require Touch ID. The read-only transaction blocks function writes within that PostgreSQL transaction. External effects from extensions or user-defined functions (for example dblink opening another connection) remain governed by database permissions; this is not a sandbox for arbitrary SELECT functions. SQL still accepts only one statement per execution.

LocalAuthentication runs on the Mac hosting the daemon; database hosts may still be local, staging or production. A daemon without macOS Touch ID refuses operations requiring authentication. Tests inject biometric outcomes only in test builds, and verify refusal before connecting and unchanged database rows after rejection. A physical fingerprint is required to verify the native success path.
