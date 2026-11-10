# 0036. Release automation

- **Date:** 2026-10-28
- **Status:** accepted

## Context

Releases were bumped, built and published by hand from a developer's Mac.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, Release automation:

> Release automation (decided with Justin 2026-10-28): Actions → Prepare release chooses patch or minor and opens a version PR. After its merge, a macOS runner verifies the frozen commit and builds both the DMG and Linux worker bundle, then creates a draft with both checksums and French notes (optional Claude summary, complete PR/commit list on failure). A human reviews and publishes; the updater follows. Main requires a PR, green Validate CI on an up-to-date branch and resolved conversations, including for administrators; force-push and deletion are forbidden. No mandatory approval, so an author can merge their own PR. The workflow lives in the distribution that publishes releases.

## Consequences

Only a human publishes a release. The procedure lives with the distribution that publishes the releases, not in Core.
