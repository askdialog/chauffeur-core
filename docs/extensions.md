# Official extensions

Settings → Extensions manages Database and Office. Both are installed and enabled by default.
Updates are automatic by default, with a separate switch for each extension; turning this off
keeps manual updates available. Disabling or removing an extension preserves its data and policy.

## Ownership and execution

Core owns workspace/chat lifecycle, agents, identity, protected secrets, persistence, installation
and request admission. Official implementations live in their own repositories and run as
supervised native executables. Private framed pipes carry requests, host callbacks and events.
The public socket cannot issue a privileged engine callback. Process separation is an update and
failure boundary for trusted official code, not a sandbox for arbitrary third-party extensions.

Database's SQL, cancellation, read-only rules and Touch ID stay in its engine. Core keeps a
workspace admission lease until a native operation returns or its process has been stopped and
reaped, even if its client disconnects. Disabling or removing a busy extension is refused.
Connections and history migrate once from Core's old tables without changing credential IDs.
The engine's private SQLite is brokered by Core; the engine never opens Core's store or Keychain.

Office obtains workspace metadata, identity tokens, scoped preferences and activity through its
broker. Google sign-in and sharing work with Office disabled. Refresh tokens remain in Core.
The browser gateway and its restricted sessions also belong to Core, so an engine replacement
does not invalidate the browser's session.

## Packages and compatibility

Each release contains native platform archives plus `extension.json` and a raw Ed25519 signature
in `extension.json.sig`. Core trusts the public key and repository fixed for that extension.
It verifies the descriptor, platform, Core range, host/UI API versions, archive size and SHA-256
before extracting regular files into an immutable revision. Links, traversal and oversized
archives are refused. An incompatible newer release leaves the installed compatible one working.
Downloads use the user's authenticated GitHub CLI; no public mirror or anonymous fallback.

The DMG and Linux bundle carry signed initial packages for offline first launch. Subsequent
versions live in daemon-owned storage outside the signed app. A Core upgrade preserves a newer
compatible extension and explicit removal choices. Removing an extension keeps its cached package
and data; a cached version can be installed again offline.

A downloaded update waits for admitted operations and mounted views to finish. Database views
retain their lease while hidden, preserving SQL drafts; close the view/workspace or restart the
app to release it. Office views and browser subscriptions also pin their engine. Core's journal
records the pending/current/previous revisions and a switch in progress. Before starting a
candidate, Core snapshots extension SQLite and Office preferences. A failed or interrupted switch
restores state and the last working engine. `state_format = 1` requires backward-readable schema
changes so the previous engine can still use data written after a successful update.

## Protocol and UI

The method declarations live in `chauffeur-protocol`; generated TypeScript is in `protocol-ts`:

| Method | Purpose |
|---|---|
| `extensions.list` | Catalogue, activation, package status and installed UI metadata |
| `extensions.set` | Persist activation |
| `extensions.configure` | Persist `auto_update` for one extension |
| `extensions.manage` | Check, install, update, remove or restore |
| `extensions.call` | User-only opaque request to that engine's namespace |
| `extensions.watch` | Mounted-view lease and that extension's opaque events |

These lifecycle and generic calls are user-only. Older typed Database and Office methods remain
compatibility adapters with their existing access rules. A package cannot expand agent, guest,
phone or Office-browser permissions by adding metadata. `Event::Extension` preserves new event
fields without a Core release; older clients retain the typed Office adapter.

Each UI is built independently as an IIFE with its CSS and assets. UI SDK 1 shares Core's React,
CodeMirror and explicit Core exports; the extension calls `registerExtension(id, setup)` and
receives an engine client and asset resolver. Core loads the installed revision transactionally,
retaining the previous contribution if the new script or stylesheet fails. A view renders only
after acquiring its lease. Contribution IDs remain stable for saved layouts and shortcuts.

Only verified UI files are served under a random read-only revision capability, never Core's RPC
token. HTTP checks the Host, allowed origin, canonical path and recorded digest; the engine and
manifest are not served. The Office's browser obtains Office-only UI metadata with its own session
and loads assets from its gateway's origin. It never receives the daemon token.

## Build and releases

Core alone builds with `cargo build -p chauffeurd -p chauffeur-cli` and `cd app && bun run build`.
It has an empty registration list and no Database/Office implementation or Three.js dependency.
The distribution's `chauffeur-bundled` selects `managed::registrations(bootstrap_root)`; its daemon
is still installed as `chauffeurd`. Source copies and bootstrap packages remain pinned and verified
in the distribution, retaining existing product release URLs and the same DMG.

Each extension has its own version and tag workflow. Its frozen, hashed Core SDK snapshot permits
CI builds without a cross-repository credential. PR checks build unsigned archives;
only a tag release signs the complete descriptor in the release environment. Signing keys are
separate from user GitHub credentials. See the distribution's `BOOTSTRAP.md` for bootstrap signing
and the extension READMEs for publishing their independent releases.

For local development, build `chauffeurd --example extension-daemon` and set
`CHAUFFEUR_BOOTSTRAP_DIR` to signed `database/` and `office/` payload directories. Use a throwaway
`CHAUFFEUR_HOME`, workspace/repo directories, `CHAUFFEUR_PORT_OVERRIDE=0`, `linear_enabled=false`
and an empty Office URL, as in the verify-in-app skill. The `extension-host` example separately
checks real engine/Broker communication and restart persistence without starting a daemon.

See [independent updates](adr/independent-official-extension-updates.md) and
[separate source repositories](adr/separate-extension-repositories.md).
