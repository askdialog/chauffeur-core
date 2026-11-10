# Official extensions download and update independently

- **Date:** 2026-11-04
- **Status:** accepted

## Context

Database and Office have separate source repositories but their compiled code and UI still ship
inside each Chauffeur release. Fixing either extension consequently requires a new DMG.

## Decision

Decided with Justin 2026-11-04: allow downloading and updating the official Database and Office
extensions independently of Chauffeur. Both remain installed and enabled by default, preserving
existing data and activation preferences. Updates are automatic by default, with a separate
setting for each extension in Settings → Extensions, as in VS Code. Manual updates remain
available when automatic updates are disabled.

## Consequences

Core owns package installation, policy, state and process supervision. An extension release
contains its own engine, UI and assets and declares supported Core and host/UI API versions.
Authenticated downloads come from fixed official repositories and signatures are checked before
any downloaded engine or UI runs. Arbitrary third-party extensions remain outside this decision.

The DMG and Linux bundle contain signed initial packages for offline first launch. Installed
updates live outside the signed app. A later Core installation preserves a compatible newer
extension and does not reinstall one the user removed. The product updater and release URLs stay
in the distribution repository; subsequent extension fixes need only an extension release.

The implementation stages downloads in the background and applies them when views have closed
and admitted operations have finished. An extension update does not restart Core or its agents.
The package journal, previous version and protected state snapshot allow recovery from an
interrupted switch or a candidate that does not start. Core identity and credentials remain in
Core-owned services, available through scoped private engine pipes.
