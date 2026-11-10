# Independent extension releases share the Core download repository

- **Date:** 2026-11-10
- **Status:** accepted

## Context

Justin consolidated Office and Database source into the private Dialog distribution while
explicitly retaining independent updates. Separate source repositories are no longer needed.

## Decision

Decided with Justin on 2026-11-10: extensions keep separate versions and signed releases,
built from the private distribution. Public binary downloads use Core releases with
`office-v<version>` or `database-v<version>` tags and must never become the latest application
release. Public tags point to the pinned Core source, not private extension source.

The client paginates releases, filters by extension and stable version, then checks the exact
signed version and existing trusted key. Application and other-extension tags are ignored.

## Consequences

Existing installations continue to use their old download repositories until a desktop
transition release installs the new client. Those repositories must remain readable meanwhile.
Private source and signing keys are never published. This migration changes no saved data,
extension activation or update policy. Signing a package remains a protected release operation.
