# No write after a PostgreSQL export

A logical export (`workspace-postgres-exports.md`) is a consistent snapshot of one
database at one moment. It says nothing of what the service did next: a write that
lands after `pg_dump` took its snapshot is in the source's volume and not in the
export, and a transfer carrying the export would lose it silently. Until now a
service holding database data could not be stopped at all.

`workspace_postgres_quiescence::export_and_stop` exports every database the
development profile declares for one PostgreSQL service, stops the service, and
verifies that the cluster's next transaction id did not advance between the start of
the exports and its shutdown: no transactional write happened (see "Outside the
proof" below for the writes that assign no id).
Trusted handoff code calls it under the exact `Stopping` fence, after the registered
stop and the [process sweep](workspace-process-sweep.md); no RPC calls it yet.

## The proof

Every transactional write assigns a transaction id, and ids are handed out in
order. So:

1. **Before the exports**, one read-only query (`psql` through `docker exec`, on the
   declared Unix socket) records the server's next transaction id
   (`pg_snapshot_xmax(pg_current_snapshot())`, an `xid8`: 64 bits, epoch included),
   its system identifier and data
   directory. It requires an idle cluster: no other client, background worker or
   replication connection, no session holding a transaction id (`backend_xid`,
   autovacuum included) nor any listed in the snapshot, none prepared. With nothing
   in progress, the snapshot's `xmax` is exactly the next id to assign.
2. **The exports** run as before, one `pg_dump` per declared database, each with its
   retained receipt.
3. **The service stops** (`docker container stop --timeout 30`, same pins as the
   selected-service stop) and must end inactive.
4. **After the stop**, the image's own `pg_controldata` reads the shutdown
   checkpoint from the data volume: a throwaway container from the pinned image ID,
   `--pull=never`, no network, read-only root, every capability dropped but
   `DAC_READ_SEARCH`, the data mount read-only, `LC_ALL=C`. Its `NextXID`
   (`epoch:xid`, combined into 64 bits) must equal the id recorded in step 1, its
   system identifier the same, its state `shut down`.

Equal ids mean no id was assigned in the whole cluster from before the first export
to the shutdown: no insert, update, delete or DDL committed or even started. A
cluster stopped uncleanly (`in production` after a kill) or a different volume fails
the check too. Checked on a real PostgreSQL 16 container: an idle export and stop
gave `732` before and `0:732` after; one `INSERT` after the export gave `0:733`.

Outside the proof: writes that assign no id (sequence values handed out from the
range already logged, hint bits, `ALTER SYSTEM`). An automatic `ANALYZE` writes
statistics with an id, so it can refuse a quiet cluster: retrying decides.

## What it refuses, and when

Before any export, with nothing changed:

- a service that is not one whose data its exports carry: it needs at least one
  declared export, a mount declared as database data, its writable layer
  reconstructible and every other resource reconstructible or database data;
- PostgreSQL older than 13 (no 64-bit snapshot functions), asked first with
  `SHOW server_version_num` so the refusal says so;
- a role that cannot read the cluster's state, named;
- other connections, transactions in progress or prepared, named with their counts;
- a database a client can use that no export declares; `postgres`, the maintenance
  database, is not carried (nor are roles and tablespaces), so it refuses when it
  holds any user table or sequence;
- an image without `pg_controldata` (`docker exec … pg_controldata --version`),
  checked before anything stops;
- a data directory outside every mount, or in a mount not declared as database data.

The query needs a superuser role or grants on `pg_control_system()` and
`data_directory`.

A profile changed while the exports ran settles the attempt before anything stops,
asking to retry. After the stop, a write found settles the attempt with its evidence: the receipt,
stored with `quiet: false`, keeps both ids. The service stays stopped, the fence
installed, the exports retained but not to be carried; the user cancels the transfer
to work locally again, or restarts the service and retries. Docker uncertainty (a
stop or a `pg_controldata` that fails or times out) leaves the preparation attempt
pending, as for the other service stops.

## Durability

The plan (fence, profile head, start observation, export digests) is stored in
`workspace_database_stops` under the preparation barrier before the stop; the
receipt (`quiet`, the stop and shutdown observations) after it, both guarded by the
exact fence. The host task survives its observer. The proof keeps the execution
fence installed and advances no journal; the composed source-stop receipt is the
next step.

## Validation

Unit tests parse `pg_controldata`'s output (the epoch counts) and refuse a busy or
partly exported cluster. A lifecycle test with the fixture Docker: other connections
refuse before any export; a `NextXID` one past the recorded id stores a settled
`quiet: false` receipt with the service stopped and the fence kept; after a restart,
equal ids return the proof, a `quiet: true` receipt and two exports in all, and the
fixture's call log shows the idle check before `pg_dump`, then the stop and the
`pg_controldata` run.

`real_postgres_exports_then_stops_with_no_write_after_them` (ignored by default:
`cargo test -p chauffeurd --test lifecycle real_postgres -- --ignored`) runs the whole
path on the local Docker engine with a throwaway `postgres:16` container and volume,
removed afterwards. Its first run found three faults the fixture Docker hid, fixed
here: the inspection template's `.HostConfig.Tmpfs` fails when no tmpfs is declared
(now `index`); OrbStack's `docker` is a link to a multi-call binary that picks its
tool from argv[0], which the canonical path changed (argv[0] is now `docker`); and
`--env PGSERVICE=` made libpq look up a service named "", failing every client
(`env -u` in the container instead, for exports and restores too).
