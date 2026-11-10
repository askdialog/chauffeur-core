# A workspace's development services

The user declares which Docker containers of a workspace carry data that must follow it to
another machine, and which are caches rebuilt there (decided with Justin 2026-10-26, see
`SPEC.md`). The declaration is the workspace's development profile
([profiles](workspace-development-profiles.md)); this is how a user makes one.

## Detecting

`services.detect {workspace}` (CLI `chauffeur services <workspace> detect`) reads every
container of each local Docker engine the transfer's container check knows (`DOCKER_HOST`, the
docker CLI's context, the usual sockets), stopped ones included, and keeps those that work or
write in the workspace: a writable bind of its folder or worktrees, a compose project whose
file lies in it, or a service already declared. For each it shows:

- its name: its compose service, else its container name;
- its image as named (`postgres:16`), whether it runs, and its compose project, file (relative
  to the workspace folder) and service, when the file lies in the workspace, recorded so that
  another machine can start it again under the same project (the next slice);
- for a PostgreSQL server (an image named `postgres*`/`postgis*`, or `PG_MAJOR` set): its user
  (`POSTGRES_USER`, else `postgres`) and, while it runs, its databases besides `postgres`, read
  with `psql` over its local socket (`/var/run/postgresql`, port `PGPORT` or 5432);
- a suggestion: data for a PostgreSQL server, a cache otherwise;
- a problem, when it cannot be declared as it is: Docker restarts it on its own (`restart`
  other than `no`), removes it when it stops (`--rm`) or keeps files in a tmpfs, since the
  transfer must stop it and find it exactly as declared; or it was not started from a compose
  file of the workspace, so another machine cannot start it again and only a cache can be
  declared.

Detection only reads: it never starts, stops or changes a container.

## Declaring

`services.save {workspace, services: [{name, data}], revision}` (CLI `chauffeur services
<workspace> save --data <name> --cache <name>`; in the app, **Development services…** in a
workspace's menu) detects again, then saves a new revision of the profile with exactly the
chosen services, each pinned to its container as it runs now (engine, container, image id,
mounts), with its compose file and service and its image name:

- **data**: only a running PostgreSQL server with at least one database and a compose file.
  Each mount (or its writable layer when it has none) is classified as database data, and
  every database it holds is declared exported: the transfer's source stop requires each one.
- **cache**: everything classified reconstructible; the transfer stops it and carries nothing
  of it (starting it again on the destination is the next slice).
- a service left out is not declared; a transfer then refuses while it runs and writes in the
  workspace.

`revision` is the one the choices were made on: a profile saved since refuses, to detect again.
Services of one engine only can be declared together. Linux setup/startup commands and preview
ports already in the profile are kept.

The app's dialog starts from what is declared, else from the suggestion, and declares every
detected container by default; a choice a service cannot take is disabled, its problem shown. A
server declared as data stays so while it cannot be read (stopped): saving waits until it runs
or another kind is chosen, so its exports are never dropped unseen. A declared service whose
container no engine shows now (its engine stopped, the container removed) is listed as gone,
never left out: it can only be left out explicitly, and declaring it again refuses. Every
declared service thus keeps its kind until the user chooses otherwise. Declarations saved
before the compose project was recorded keep their exact encoding; detecting and saving again
records it.

## Web previews

Decided with Justin 2026-10-27 (`SPEC.md`): a workspace's web previews are declared the same way,
detected then confirmed, and open on the phone while it runs on another machine
([relay](workspace-relay.md#previews-on-the-phone)).

- `services.detect` also lists `previews`: each port of the workspace's block that answers HTTP
  now (a `HEAD /` whose answer starts `HTTP/`, so a database or debugger port is left out), and
  each declared one even when nothing answers. One not declared is named after the run script
  whose process group listens on it (`lsof`), else `port-<port>`.
- `services.save` takes `previews: [{name, port}]`, the ports of this machine's block, and keeps
  each by its place in the block (`offset`), so that it names the same service on a machine
  whose block starts elsewhere. Absent, the declared previews are kept; empty, none is. Names are
  letters, digits, `_` and `-`, one per preview.
- `services.get` lists them with their port here; CLI `chauffeur services <workspace> save
  --preview web=41000` (repeatable; `--no-preview` declares none).
- In the app's dialog, under **Web previews**: each port that answers, unchecked with its
  suggested name, and each declared one, checked; the user checks and names those to declare.

A profile with previews and no service still follows the workspace: the machine it arrives at
keeps its previews (and setup and startup commands) with no service.

## On the machine it arrives at

Before anything stops, the destination check needs Docker with compose there and no container
or volume of the workspace's compose projects yet (`compose_projects`, from the source's
declaration): the services start on fresh volumes, and removing what an attempt started never
reaches data that was there before. Coming back to the machine it left (the journal's next
transfer after the one that retired its copy there), its containers there may stay but must be
stopped: their volumes are set aside and the containers removed when it arrives. A service
without a named volume keeps its data in an anonymous one, which carries no compose label: the
one its declaration there pins is set aside with the others, never an external volume a pin
also names. Leaving a machine, its containers go with their anonymous volumes (`rm -v`).

While the restored workspace is still quarantined, before the commit
(`workspace_service_provision`):

- each declared service with a compose project starts again from its compose file in the
  restored workspace, `docker compose -p <project> -f <file> up --detach --wait <services>`,
  so its volumes are fresh and the repo's own `docker compose` commands find it; images are
  pulled there, so arm64 and amd64 machines both work;
- each PostgreSQL server waits until it accepts connections; a database the image created at
  first start (`POSTGRES_DB`, else the user's name) holding no table is dropped, one holding
  any refuses;
- each database the source exported is restored into it (`pg_restore --create`, one after the
  other in the same server);
- the profile pinned to these containers is recorded. At activation it becomes the
  workspace's declaration here, so its next transfer exports these databases, and the records
  of the quarantine's restorations go.

A failure leaves the transfer resumable or cancellable: a retry removes what the earlier attempt
started (`docker compose -p <project> down -v`) and starts again from fresh volumes; a
cancellation removes it too. The source still holds everything until the commit. A
declaration saved before compose projects were recorded does not start; detect and save it
again on the source.

Caches start too, before the commit. Files the repo ignores (`.env`) do not travel: a compose
file that needs one fails to start, its message shown, until it exists there.

## Coming back

When the workspace comes back to a machine it left (the journal's next transfer after the
one that retired its copy there), that machine still holds its compose projects' containers,
stopped since it left, and their volumes, with the data they had then. Before the services
start again:

- the services start again on the engine that declaration pins them on, which must still
  answer as that engine;
- only what this machine's own declaration of the workspace pins is touched: a container or
  volume of the same compose project it does not declare (another clone's) refuses, to remove
  or rename by hand;
- each volume of the project is copied into an aside volume
  (`chauffeur-aside-<operation tail>-<volume>`, labelled `chauffeur.workspace` and
  `chauffeur.operation`), with the project's own image (`cp -a`, no network), or a pulled
  `busybox` when that image has no `cp`;
- the project's containers and volumes are removed, so the services start on fresh volumes
  and the returning databases restore into them, as on any destination.

After activation the aside volumes are removed in the background, unless the user asked to
keep the local copy (`--keep-local-copy`, the dialog's checkbox): then they stay, to compare
and remove by hand. A failure before the commit keeps them too; a retry copies again from the
originals while they exist.

The machine the workspace left, once its copy there is deleted, removes its declared
services' containers and volumes with it: their data was exported when the workspace left and
restored where it runs now. A container still running there keeps the deletion pending until
it is stopped. The deletion and a return restoring the workspace on that machine never run at
the same time.

## Earlier exports

A package carries only the exports its source stop took: those an earlier transfer brought to
the source stay there, bytes and receipts, and never travel again.

## Validation

`development_services_are_detected_then_declared_as_data_or_cache` (lifecycle, a fake `docker`)
detects a compose project's PostgreSQL server with two databases and a Redis cache in the
workspace, leaves out a container of another project, refuses Redis as data, saves (version 2,
two exports, compose and image recorded), refuses a save on a stale revision and saves an empty
declaration. A real transfer to a GCP VM (Ubuntu, Docker 29, amd64) from a Mac (OrbStack, arm64) moved a
workspace whose compose file declares `postgres:16` with a table holding one row: on the VM
the container `shop-web-db-1` ran under project `shop-web` on a fresh volume, the row read
back through `docker compose exec` from the workspace folder, and `chauffeur services` there
showed the service declared. Brought back after a row was written on the VM, the VM stopped
and exported its database (two exports in the package). With the return handled, a second
round trip (a row added on the Mac, then on the VM) brought the Mac a fresh container holding
all three rows; the aside volume was gone after activation, and the VM kept no container,
volume or workspace. That trial also resumed a transfer whose restoration had failed: the retry
removed what the attempt had started and restored from fresh volumes. Unit tests cover PostgreSQL recognition, pinning problems, compose paths and names;
`app/src/lib/devServices.test.ts` the dialog's choices.
`web_previews_are_detected_declared_by_their_place_and_listed_with_their_state` (lifecycle)
starts a run script serving HTTP on the workspace's first port: detected there under the run
script's name, declared by its place, refused outside the block, kept when services alone are
saved, listed ready, then stopped once it stops, starting while another script runs, an error
once the last one fails. Seen in the app on a throwaway daemon
with a fake `docker`: the menu entry, the dialog (data, cache, a `restart: unless-stopped`
container refused) and a saved change.
