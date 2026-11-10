# Core and official extensions have separate source repositories

- **Date:** 2026-11-04
- **Status:** accepted

## Context

Database and Office were optional modules inside the original Chauffeur repository (Dialog's), compiled into one release.
Their direct imports of the daemon and app prevented independently owned source packages.
Justin wants the extensions extracted and a new Core repository with a clean history.

## Decision

Decided with Justin 2026-11-04: create chauffeur-core, chauffeur-extension-database and
chauffeur-extension-office from clean initial snapshots after extraction and verification,
without renaming or rewriting the original repository. Core hosts registered contributions; the
extension implementations depend on Core's public services and UI exports. The original
repository retains history and temporarily remains the distribution and updater destination.
The full distribution still ships both official extensions, enabled by default, in one DMG.

## Consequences

The distribution registers the daemon runtimes and UI contributions. Core builds and runs with
an empty registry and has no implementation dependency on either extension. Identity, secrets,
workspace admission, persistence and RPC access control stay in the daemon.

Source snapshots are pinned to commits and checked for drift. The distribution keeps verified
copies so CI needs no new cross-repository credentials. Release version fields and the assembled
Cargo workspace are explicit distribution overlays. Implementation changes originate in their
source repository, then update the distribution's pins and copies together.

The source repositories start with the final code and a provenance record. Historical commits,
existing releases and download URLs remain in the original repository. This changes source ownership,
not independent installation, arbitrary third-party code execution or process isolation.
