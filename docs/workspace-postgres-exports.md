# Retained PostgreSQL logical exports

`workspace_postgres_export` is an internal source-side adapter. An authenticated
coordinator supplies the original stopping state and exact current development
profile head after registered agents, scripts and terminals have stopped. The
database container stays running while `pg_dump` takes its logical snapshot.
No RPC, service restart, destination execution or controller acknowledgement is
added here.

Profile version 2 declares each export's name, service, database, role and explicit
Unix socket directory/port inside the selected container. The service must already
declare meaningful database data. Database and role identifiers are restricted to
bounded simple names; connection strings, URIs and socket fallback lists refuse.
Profile-free export lists preserve version 1's exact serialized bytes and digests.

The adapter loads the saved profile under the settled stopping fence, takes a
durable preparation barrier and records its plan before starting Docker. It checks
the selected Linux engine and exact container/image/creation/mount pins before and
after the export. Docker routing stays on the selected inode-pinned local socket.
The command uses no shell, interactive password prompt, table/schema filter,
owner/ACL stripping or active volume copy. Libpq service, host-address and options
environment overrides are cleared; database/role/socket/port are explicit. Existing
container authentication may be used, but credentials are never read into the
daemon, logged, or copied into the result.

[PostgreSQL documents](https://www.postgresql.org/docs/18/app-pgdump.html) that
`pg_dump` creates a consistent snapshot of one database while concurrent writers
can continue. Global roles and tablespaces require separate handling. This adapter
does not provide a shared snapshot across databases, prove external writers
stopped, capture cluster-wide configuration or prove the source cannot execute.

Stdout streams through bounded chunks into a new private file under the daemon
home. The maximum is 16 GiB, with five minutes for the command and ten minutes for
the overall attempt. Any stderr output, nonzero exit, timeout or changed pins
refuses success without exposing diagnostics. The daemon syncs and independently
rehashes the custom-format bytes, verifies the declaration and exact file layout,
and durably commits its receipt before settling the preparation barrier. Header
and digest checks verify retained bytes; they are not proof that `pg_restore` can
restore the database or that its application prerequisites exist.

The host task survives observer loss. Every failure keeps the source, partial
files and durable pending plan; no automatic retry, adoption or cleanup can erase
uncertainty about an engine-side `docker exec` after its local CLI exits. Pending
preparation blocks cancellation, legacy stopping and another export. A successful
export leaves the source fence installed. Receipt files are private plaintext and
must enter authenticated encrypted custody before leaving the host.

These artifacts are now retained by version 3 [transfer packages](cloud-workspace-package.md).
A separate internal
[quarantined restore adapter](workspace-postgres-restores.md) consumes explicitly
authenticated exports on a provisioned target; no production worker flow is enabled.
This export alone does not stop the service or prove no write followed it.
[`export_and_stop`](workspace-postgres-quiescence.md) exports every declared database
of a service, stops it and verifies that the cluster's next transaction id did not
advance from before the exports through its shutdown, so no transactional write
happened; it is the only path that stops a service holding database data.
Application validation and cluster prerequisites (roles, tablespaces) remain.
MySQL, meaningful Redis state/tasks and unsupported services remain blocked;
explicitly reconstructible Redis caches use the existing reconstructible path.

Tests use only fake Docker executables and temporary sockets/files. No real
database, Docker engine, native agent, cloud service, daemon or e2e script is run.
