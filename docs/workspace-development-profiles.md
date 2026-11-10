# Daemon-owned development profiles

`workspace_development_profile` stores bounded, versioned declarations in the
daemon's SQLite store, outside repositories. A user makes one by detecting and
confirming the workspace's services ([development services](workspace-development-services.md));
there is no command executor yet. A service also names the compose file and
service it was started from and its image, absent from earlier profiles, whose
canonical encoding and digests stay unchanged.

A profile declares Linux setup/startup commands, selected Docker services and
private preview ports, each by its place in the workspace's port block
([web previews](workspace-development-services.md#web-previews)). Each service binds an explicit local socket and Linux
engine to an exact container/image/creation time/mount layout. Runtime memory,
the writable container layer and every mount destination each require a data
classification. `Preserve` retains the meaningful-data kind and explanation;
it cannot authorize the current reconstructible-only service-stop path. Optional
services receive the same refusal until explicit warning acceptance exists.

Writes take the workspace's execution admission and use an exact expected
revision/digest. Immutable versions and the current head are committed durably;
missing, corrupt or inconsistent state refuses loading instead of resetting it.
A fence drains earlier admissions and prevents subsequent profile edits.

Limits are 256 KiB per canonical profile, 32 services, 128 mounts per service,
64 commands per setup/startup list (8 KiB each) and 64 distinct preview ports.
Transfers retain at most 32 distinct declarations and 1 MiB of their canonical
serialized content; exceeding either limit refuses capture.

Docker selection loads the daemon's exact current profile, verifies classifications
and compares observed resource pins. After registered launchers drain and the
stopping fence settles, the stop path checks that the same profile is still
current before issuing any Docker stop. A digest supplied by a caller is not
classification authority. The observation and retained plan name the saved head.

Packages containing profiles use manifest version 2. Profile-free packages retain
version 1. Verification checks profile scope, digest, bounds and duplicates.
Restore writes declarations into a separate provenance table in the same
transaction as workspace metadata: they never become an active destination
profile. A later package preserves both imported declarations and the current
local profile, deduplicating exact matches and refusing overflow rather than
discarding any. Historical local profile revisions stay in the source store;
the package carries the current declaration, not its edit history.

Imported source socket paths and container identities cannot authorize operations
on the destination. Explicit target configuration/rebinding remains future work,
as do consistent meaningful-data export/restore, setup/startup execution, complete
source quiescence, authenticated coordination and activation.
These declarations alone establish none of those properties.

Version 2 adds explicit [PostgreSQL logical export declarations](workspace-postgres-exports.md)
without changing serialized version 1 profiles. Exporting a declared database
does not authorize stopping its meaningful-data service.
