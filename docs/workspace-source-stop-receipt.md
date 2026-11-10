# The complete source-stop receipt

The journal's `Stopped` state carries a `source_stop_digest`, and source preparation
captures a workspace only under that acknowledgement. Until now nothing produced the
receipt behind the digest: tests made one up. `workspace_source_stop::stop_source`
now stops the whole source, stores what it observed, and returns the receipt and its
digest. `cloud_workspace_preparation::capture` refuses any `Stopped` acknowledgement
whose digest names no receipt this daemon stored under the same fence.

Trusted handoff code calls it under the exact `Stopping` fence; no RPC calls it yet.

## Order

```text
stop_source
  stop_registered                 agents, scripts, terminals          registered-source-stop.md
  sweep (own processes)           detached descendants, lost launches workspace-process-sweep.md
  reconstructible services        docker stop, pinned                 workspace-service-stop.md
  each PostgreSQL service         exports + no-write proof            workspace-postgres-quiescence.md
  containers::check               no running container writes there   (below)
  last scan                       no process of the workspace, no other program writing there
  receipt → SHA-256 → workspace_source_stops(digest, workspace, fence, receipt)
```

Application containers stop before the databases, so none still writes to one when
its idle check runs. Every step refuses with what to do; a refusal leaves the fence
installed and what already stopped stopped. A retry starts again from the top: the
registered stop and the sweep find nothing left, stopped services stay stopped, and
a database already proven quiet under the same fence and profile is not exported
again (it could not be: it is stopped). Its proof is reused only while the same
pinned container is still stopped at the very shutdown checkpoint the proof read
(`Latest checkpoint location`: every start and stop writes a new one). Running
again, it is exported afresh; started and stopped again by hand, the stop refuses,
asking to start it so it can be exported.

The profile decides the services: a service its PostgreSQL exports cover goes
through the no-write proof; every other one must be entirely reconstructible, or the
stop refuses. A workspace without a profile has no service to stop, but still gets
the container check.

## Containers writing in the workspace

`workspace_containers::check` lists the running containers of every engine the
user's `docker` could reach and inspects each: the profile's socket; `DOCKER_HOST`
when set, which as for the docker CLI overrides the rest; otherwise the current
docker context's endpoint (read from `~/.docker`) and every known engine socket
(Docker Desktop, OrbStack, Colima, Rancher Desktop, rootless, `/var/run`). Sockets
are canonicalized and an engine reached twice is counted once. A remote engine
(`tcp://`, `ssh://`) refuses. A container writes in the workspace when it has a
writable bind mount of one of its folders or of a folder containing it (the home
folder), sources canonicalized so `/tmp` matches `/private/tmp`, or of a host folder
a declared service keeps data in,
belongs to a compose project whose files lie in the workspace, or mounts a volume a
declared service keeps data in. Any such container refuses, named:
`Containers still run and write in this workspace: extra-writer (ddddddddd) mounts …
writable. Stop them, or declare them as services of its development profile, then
retry the transfer`. A socket where nothing listens (missing, or connection refused)
is no engine; one that listens but does not answer refuses, and so does a profile's
socket answering with another engine than the one declared.

## The receipt

`SourceStop` holds the source state, the profile head, the sweep, the service stop,
each database proof and the container check. A retried stop stores a new receipt;
preparation accepts only the latest one under the fence, and the package carries
exactly the exports it names (an export taken again after a restart supersedes the
earlier one of the same name). Its canonical JSON's SHA-256 (base64url,
43 characters) is the digest the controller records in `Stopped`. The row is written
only while the exact fence is installed and no preparation is pending; the same
receipt stored twice is one row. The receipt does not advance the journal: the
controller does, after authenticating the source.

What it still does not cover is listed in each step's document: other users'
processes, memory-mapped writes, a macOS process of the workspace that does not
touch it, writes a database makes without a transaction id, services other than
PostgreSQL holding data (refused), another container engine.

## Validation

A fixture-Docker lifecycle test: a container bind-mounting the workspace makes the
complete stop refuse after the database stopped; once it is gone a retry reuses the
database proof (one `pg_dump` in all), stores the receipt whose SHA-256 is the
returned digest, keeps the fence; preparation refuses a made-up digest and captures
under the real one. The preparation and package tests now acknowledge a real
complete stop. A unit test covers the three ways a container writes in a workspace.
The opt-in real test (`cargo test -p chauffeurd --test lifecycle real_postgres --
--ignored`) runs `stop_source` on the local engine with a throwaway `postgres:16`:
the container check reads the real engine (other containers running), changing
nothing there.
