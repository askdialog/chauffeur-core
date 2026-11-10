# 0032. Development data on a transfer

- **Date:** 2026-10-26
- **Status:** accepted

## Context

A workspace's Docker services (databases, caches) must follow it to a machine of another architecture without copying volumes.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, Cloud workspace follow-up:

> Development data on a transfer (decided with Justin 2026-10-26): a workspace's
> development profile names, for each Docker service, the repo's compose file and its
> service; the destination recreates the service with `docker compose up` on fresh
> volumes (its image pulled there, so arm64 and amd64 both work), never by copying a
> volume. A user makes the profile by detecting the containers that write in the
> workspace and the PostgreSQL databases they hold, then confirming each service as data
> to carry or a cache to rebuild, in the app's "Development services" panel or the CLI.
> When a workspace comes back to the Mac it left (the journal's next transfer after the
> commit that retired that copy), the returning database is restored into a fresh
> container and volume; the old ones are set aside and removed once the return is
> verified (kept with "keep the local copy"), as the files are. A transfer carries only
> the exports taken when its source stopped; earlier exports, bytes and receipts, stay
> on the machine that received them and are left out of later packages (decided with
> Justin 2026-10-26).

## Consequences

The destination recreates services from the compose file on fresh volumes and restores databases from logical exports. Details: [`docs/workspace-development-services.md`](../workspace-development-services.md).
