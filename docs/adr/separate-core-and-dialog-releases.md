# Separate Core and Dialog release artifacts

- **Date:** 2026-11-11
- **Status:** accepted

## Context

The public repository's first v0.36.0 download was the Dialog distribution, including its private extensions. Its Core source snapshot still declared 0.30.0. A community packaging attempt exposed the ambiguity between public MIT sources and distribution downloads.

## Decision

Justin approved separate Core builds, aligned source and binary versions, and isolated updates on 2026-11-11. No additional repository is needed. Both editions use the existing public release repository, with distinct tags and asset names. Core uses `core-vX.Y.Z` and `Chauffeur-Core-X.Y.Z.dmg`; Dialog retains `vX.Y.Z` and `Chauffeur-X.Y.Z.dmg` so installed clients continue to update. Core releases never replace GitHub's latest release, which legacy Dialog clients consult. Core's updater selects only complete, stable Core releases, across every page. Its workers also use Core's own tag and bundle.

Core packages are built only from the public checkout, without `distribution/` or private extension sources. The source version, tag suffix, and binary version must agree. Third-party tools retain their own licenses. The release workflow can receive Apple signing credentials but never extension signing keys. Builds and drafts do not publish automatically; publication follows review.

## Consequences

Existing public tags are immutable: the historical v0.36.0 tag remains a distribution release. New releases identify their edition explicitly. The public workflow builds Core independently and can be called by the private distribution for coordinated releases. Both editions currently install the same app and CLI paths; they are alternatives, not side-by-side installations. Independent extension tags remain separate from both application channels.
