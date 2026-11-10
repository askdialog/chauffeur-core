# Installed archive helper runtime

The fixed `chauffeur-cloud-archive --private-archive-job` executable now connects the
finite host core to private Unix RPC, durable relay channels and encrypted GCS
checkpoint storage. Build/install scripts place it beside `chauffeurd`, matching the
existing daemon supervisor's fixed sibling lookup. No handoff action is exposed yet.

## Input, lifetime and output

The executable requires the daemon-selected absolute `CHAUFFEUR_HOME` and exactly
its private flag. It uses no ambient HOME, provider token, PATH executable discovery,
ADC, interactive login or child process. It waits for EOF on at most 8 KiB of strict
UTF-8 private JSON before opening a daemon connection. Intake has a ten-second
limit; SIGINT/SIGTERM also abort it. Unknown fields, trailing JSON and malformed
input refuse without returning a receipt.

Both configuration/control and artifact connections use the fixed private socket
in that home. The shared Unix connector now supports cancellation during handshake,
hello and pending RPCs. It uses the actual pinned npm `ws` implementation under the
`ws-unix` alias: Bun's built-in `ws` shim ignores `createConnection`. A fixed HTTP
upgrade URL plus explicit `net.connect({path})` keeps spaces, Unicode, colons and
percent characters in the socket path out of URL parsing. It never tries TCP or
starts a daemon. Existing relay/mobile callers retain the same method surface.

Controller admission is authenticated before checkpoint storage opens. The helper
opens an existing recovery vault, joins only the configured durable relay channels,
and passes fresh admission into every archive key/block/final check. Deadline,
configuration expiry, cancellation and disconnect close its capabilities. A
successful stdout value is only the exact bound pipe receipt. Failure emits one
constant recovery instruction to stderr and exits unsuccessfully; tokens, file
paths, archive keys and private RPC error text are never printed.

## Explicit GCS authority

Protected host configuration is now version 2 and requires a storage authorization:

- `access_token`: an explicitly selected GCS token, at most 8192 characters. Its
  supplied expiry must cover the full configuration lifetime, and publication
  refuses a token expiry more than one hour away. This permits the owner to reuse
  its approved GCP identity without the helper reading or spawning `gcloud`.
- `worker_metadata`: an explicit selection for the enrolled worker, refused for
  the owner identity and on macOS. Each GCS request obtains a validated short-lived
  token from the existing GCE metadata adapter. Invalid metadata never falls back
  to another identity.

The internal trusted coordinator must obtain the selected authority and check its
bucket permissions before publishing configuration. Syntax and claimed expiry do
not prove GCP identity or IAM scope. This slice does not mint owner tokens or
implement token/configuration renewal. Short owner leases and immutable expiry
therefore bound a transfer attempt; recovery must be explicit before another one.

Configured tokens stay in protected configuration; metadata tokens remain in
process memory. Neither reaches SQLite metadata, process arguments, logs or the
transfer channel. GCS requests are restricted to the selected bucket
and prefix, carry authorization in headers with redirect refusal, and inherit the
job's abort signal as well as existing per-request bounds. No Claude/Codex credential discovery
or native account renewal capability is added to the archive runtime.

Legacy version-1 configuration has no selected storage authority and is refused;
its protected material is retained. Neither read nor publication upgrades it,
replaces its pin or invents credentials.

## Build and evidence

`bun run build:archive` builds the native executable. Mac installation copies it
beside the daemon; Mac packaging also signs it. The Linux worker artifact compiles
the Linux x64 baseline binary and requires it in the verified manifest before installation. Build scripts
include the new sources in their allowlist. No install, deployment or service start
is performed by the tests.

Evidence includes native compilation and Linux x64 baseline cross-compilation
(the Linux artifact is inspected as ELF, not executed on a Mac). The compiled
native helper is exercised with an empty personal environment against a temporary
Unix mock: exact bound configuration read, missing-state refusal, invalid/oversized
input and SIGTERM while a reply is lost. Separate real mock-socket tests cover
spaces/Unicode/colon/percent paths. Input and storage-adapter tests cover fragmented
JSON, abort before EOF, both explicit authority modes, metadata validation, no
fallback, selected GCS location and cancellation during metadata/GCS requests.
Existing host tests compose full multi-block delivery and preserve partial recovery
material after later-block refusal. No real native sessions, model APIs, GCP
credentials, production daemons or e2e scripts are used.

Private job dispatch and finite controller admission listeners are described in
`cloud-workspace-dispatch.md`. Authenticated configuration publication/coordinator,
source stop/preparation, destination readiness/activation, cleanup and desktop/mobile
actions remain integration gates. The executable cannot create trusted configuration,
initialize a vault, acquire ownership or make a destination runnable by itself.
