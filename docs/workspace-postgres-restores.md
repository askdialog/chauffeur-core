# Quarantined PostgreSQL restoration

`workspace_postgres_restore` is an internal destination adapter for the
[retained logical export](workspace-postgres-exports.md). The coordinator must
authenticate both the exact published journal and the separate export receipt,
and provision a pristine private PostgreSQL target service. There is no public
RPC, automatic binding adoption, container provisioning or destination activation.
An archive contains executable SQL from the trusted source; its digest alone is
not authority to run an arbitrary archive.

Before target commands, the daemon verifies the complete source artifact and
copies the expected dump bytes into its own unlinked temporary file, checks the
copy's digest/length and syncs it. Later replacement or editing of the original
pathname cannot change the input sent to PostgreSQL. The original source artifact
is never modified or deleted. This uses up to one additional dump's worth of
temporary disk space, released when the private input descriptor closes.

The saved source declaration must be retained in the restored workspace's
provenance. The published journal must be the exact successor of the export's
stopping operation. The target is explicitly configured with its own local Docker
socket, Linux engine, full container/image/creation/mount pins and PostgreSQL
Unix socket/port/role. Reusing the source container, equal volume names or
overlapping mount source paths on the same engine refuses. Filesystem aliases,
external mount writers and shared-service isolation still require the coordinator's
provisioning proof; string comparisons do not establish physical isolation.

The daemon reserves a durable plan against the exact completed workspace
restoration receipt. An incomplete, missing, altered or differently targeted
quarantine refuses before database effects. A pending attempt excludes every
other database restoration in that workspace. The same export cannot be rerun or
adopted after success, failure or restart. The original quarantine stays installed
throughout and after successful restoration.

Read-only `psql`, with startup scripts disabled and strict error handling, checks
that the target cluster lists exactly `postgres`, `template0` and `template1`.
This inventory check rejects existing user databases; it does not prove template
contents or configuration are pristine. Restoring system databases is unsupported.
The daemon then runs `pg_restore --create --exit-on-error --no-password` on the
exact verified input, connecting initially to `postgres`. No clean/drop, owner/ACL
stripping or object filter is used. Missing roles, tablespaces or extensions fail;
they are never silently removed or synthesized.

[PostgreSQL documents](https://www.postgresql.org/docs/18/app-pgrestore.html) that
`--create` also restores database-level privileges and settings, and fails when
creation encounters an existing database. It cannot be combined with a single
transaction. Errors can therefore leave a newly created, partially restored
database: the daemon preserves it for reconciliation and never drops it or retries
into it automatically. A database appearing after the inventory check still
causes creation to fail instead of being overwritten.

Input/output streams are bounded and concurrently drained. The input is capped at
16 GiB, command output at 64 KiB, each query at ten seconds, the restore command
at five minutes and the overall host task at ten minutes. Any stderr byte,
nonzero exit, changed input metadata/digest, output overflow or timeout refuses a
receipt. A detached environment snapshot keeps all commands on the original
runtime across an environment reload. The wrapper's owned process group is killed
before reaping; its exit cannot prove an engine-side operation stopped.

After command success, the daemon independently checks the same runtime pins and
exact inventory including the declared database, then durably records a receipt
bound to this attempt, export and target. This is a tool-based restoration
observation, not an application-level data comparison or permission to execute.
Observer loss leaves host work running; failure or interrupted persistence retains
the pending plan, source dump, target database and quarantine.

Version 3 [workspace packages](cloud-workspace-package.md) retain independent copies
of their exports in the completed quarantine. `restore_packaged` selects an index
from the daemon's bounded metadata and requires its exact current operation's
restoration path. It accepts no caller-provided artifact root or receipt and
does not adopt historical snapshots as current database restore authority.

Application/schema checks, complete writer coordination,
cluster prerequisites, readiness publication and ownership activation remain
gated. Meaningful-data service stopping stays refused. Tests use fake Docker and
temporary sockets/files only; no real PostgreSQL, Docker or GCP service, production
daemon, native model session or e2e script is run.
