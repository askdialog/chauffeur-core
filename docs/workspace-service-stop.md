# Selected Docker services during source stop

`workspace_service_stop` adds an internal stop path separate from workspace
archival. It uses explicit full container IDs on one selected local Docker Unix
socket. It never discovers a Compose project, deletes a container or volume,
changes a restart policy, or treats missing Docker as an empty successful result.

## Scope and data safety

A trusted coordinator must authenticate the current `Stopping` state. Selection loads
an exact daemon-owned profile declaring **all** selected services' data reconstructible,
including volatile memory, writable container layers and mounted data. The profile
head is retained in the plan and observation and checked again after the stopping
fence settles, before Docker mutations. A caller-supplied digest cannot authorize
stopping. See [development profiles](workspace-development-profiles.md).
Profile UI and meaningful-data exports remain integration gates.

Meaningful databases, queues or in-memory Redis data must not use this path. A
PostgreSQL service stops through its exports instead
([no write after an export](workspace-postgres-quiescence.md)). Merely retaining a container or volume cannot preserve
all runtime state. Explicitly selected services also must be private to this
workspace; sharing, external orchestration and other mount writers remain blockers
for a complete source-stop receipt.

Selection refuses automatic container removal, restart policies other than `no`,
paused/restarting/dead/removing or inconsistent states, unsupported mount types or
drivers, duplicate destinations and tmpfs. Both `.Mounts` and legacy
`HostConfig.Tmpfs` are checked. Docker documents that
[tmpfs data disappears on stop](https://docs.docker.com/engine/storage/tmpfs/).

## Exact observations and bounded commands

Selection pins full container IDs, image IDs, creation times and a canonical mount
layout digest. Mount metadata is retained for subsequent data inventory; it is not
an exported or consistent snapshot. Output is projected to the selected fields,
excluding environment, credentials, arbitrary labels and engine error text.

Every stop preflights **all** selected containers before the first mutation and
checks the selected Linux engine identity. It issues only
[`docker container stop --timeout 5 <full-id>`](https://docs.docker.com/reference/cli/docker/container/stop/),
then independently re-inspects every selection after the last stop. Success
requires the same immutable pins/mount layout and an inactive `created`/`exited`
state with PID zero and no active flags. An earlier service restarting during a
later stop refuses the whole observation. Exit codes are retained; successful
observation does not claim graceful shutdown or database consistency.

The CLI runner snapshots the selected environment/binary path, removes ambient
Docker context/host/TLS routing, and passes an explicit local Unix host. Socket
device/inode identity is checked around commands. Route overrides are refused.
It uses no shell, discards stderr, bounds stdout and returns generic errors.
The wrapper's process group is killed before its unreaped leader PID is released,
including normal exit, timeout, oversized output and caller cancellation. A child
deliberately escaping that group is outside this observation.

Limits: 32 selected containers, 128 mounts per container, 8 KiB mount paths,
1 MiB inspection output, ten seconds for inspection/engine reads, fifteen seconds
for each stop command, thirty seconds for selection, and five minutes for the
service-stop phase. A command also has at most two seconds of reaping cleanup.
These bound the caller's observation, not asynchronous work
already accepted by the Docker engine. Timeout remains uncertain.

## Persistent ownership and ordering

`stop_registered_and_services` first drains/stops registered native sessions,
scripts and terminals, then [sweeps](workspace-process-sweep.md) the workspace's
remaining processes, so none can restart containers behind a completed Docker
check. If either fails, it does not start the service phase.

The service phase then takes the durable preparation barrier and persists the
sanitized plan in `workspace_service_stops` before issuing Docker commands. Its
attempt ID links to the exact fence, operation and daemon generation. The
observation is synced before the barrier settles. Success leaves the execution
fence installed; it neither advances the controller journal nor authorizes capture
or destination execution.

The host task survives observer disconnection. Missing containers, metadata/engine
changes, command failures, timeout and interrupted persistence leave a pending
barrier and any saved plan/observation. Legacy registered-stop retry, changed
selection and cancellation cannot bypass this uncertainty. A CLI being killed
does not prove its engine request completed. Explicit service/preparation recovery
remains to be implemented; no automatic retry or adoption is attached. Do not
downgrade while a preparation is pending to a daemon that ignores that barrier.

## Evidence and remaining gates

Fake-Docker tests with temporary Unix sockets prove bounded wrapper cleanup,
snapshot routing, socket replacement refusal, all-selection preflight, engine/pin
substitution, tmpfs and unsupported-service refusal, final restart detection,
observer detachment, retained pending plans and cancellation/retry ordering.
Lifecycle fixtures retain a volume canary and prove registered launchers exit
before a selected stop begins. No actual Docker, production daemon, native model
session, cloud service, installation or e2e script is used.

Development-profile UI, full process/descendant and service inventory,
consistent meaningful-data exports/restores, authenticated complete-stop evidence,
remote controller coordination and ownership activation remain gated. These
scoped observations do not close those boundaries by themselves.
