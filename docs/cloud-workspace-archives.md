# Encrypted workspace archives

`cloud_workspace_archive::seal` and `open` are internal endpoint APIs between
package capture and destination restoration. They add no RPC to clients, upload,
relay provisioning, journal transition or execution permission.

## Trust and confidentiality

The caller supplies authenticated current `snapshot_ready` state, never journal
metadata obtained from an object listing. Encryption and restoration share the
same complete package/operation validation. The archive authenticates workspace,
owner, controller, operation, source, destination, source epoch and snapshot digest
as AES-256-GCM additional authenticated data. Copying ciphertext to another
operation, generation or endpoint cannot authorize its contents.

`ArchiveKey` generates a random 256-bit key. The coordinator must retain the key
at trusted endpoints and deliver it through an authenticated endpoint-only channel;
the relay, controller and object storage must not receive it. The
[endpoint delivery protocol](cloud-workspace-delivery.md) now provides protected
key retention/recovery and pinned host-to-host delivery; runtime dispatch remains
to be integrated. A lost key can be recovered
only from that protected state or by making another archive from the retained source.
No source, package or failed archive is deleted by these APIs.

`cloud-workspace-preparation.md` now supplies durable source-local ownership and a
protected pre-head key reservation before sealing. The source helper now bridges
that reservation into an explicitly selected existing recovery store after fresh
controller approval; it never initializes a missing store or replaces a conflicting
key. Recovery-store creation and authenticated routing remain coordinator work.

Each encryption uses a new random 96-bit IV. The opaque file contains only a format
marker, IV, ciphertext and authentication tag. Filenames, transcripts and operation
metadata are encrypted. The returned SHA-256 covers the entire ciphertext file.
Its exact digest and length must reach the destination over authenticated control;
an untrusted storage listing cannot supply them. The digest is an artifact reference,
not a substitute for the GCM tag or for source-stop/ownership evidence.

Keys travel to the sidecar through its private pipe, never command arguments,
artifacts, public RPCs or diagnostic `Debug` output. Rust's retained key buffer
and the sidecar's decoded key buffer are zeroized when released. No personal
CLI homes or credentials are added to the package.

## Streaming and extraction

The sidecar processes 64 KiB chunks rather than buffering a workspace. The encrypted
stream contains regular-file records (UTF-8 relative path, 64-bit length, content)
and an explicit final marker. Names have no absolute paths, empty/dot components,
backslashes, colons, NUL or `.git` component. Symlinks and special files are refused;
files and folders are created exclusively with private permissions. Duplicate paths,
file/directory conflicts, truncation and bytes after the end marker fail explicitly.
Empty directories are reconstructed only when files need them.

Reception first decrypts into a private scratch file. It checks the complete
ciphertext reference and authentication tag before materializing any package entry.
Then it extracts, synchronizes files/directories and runs the daemon's full package
verification against the authenticated snapshot. A successful decryption installs
no workspace row and clears no source fence or destination quarantine. The ordinary
restoration API is still required afterward.

Output paths must be new; aliases in parent directories are resolved by the daemon.
The sidecar refuses links, noncanonical parent paths and hardlinked input files.
An encrypted output cannot be placed inside its source package. Failures retain
private partial outputs, including possibly unauthenticated scratch bytes, for
explicit reconciliation; these are not returned as packages, imported or executed.
Successful reception removes only its own scratch file in the new output folder.
Retrying a failed request uses a new output path.

V1 limits: 16 GiB of framed plaintext per archive, 200,000 files and 200,000
directories, 64 MiB of combined UTF-8 file path bytes, 64 MiB of directory path
bytes and 8 KiB per path, in addition to package/snapshot/native limits.
Reception temporarily needs space for both the decrypted framed stream and the
extracted package. Resource/capacity preflight remains coordinator work.

## Validation and remaining gates

Sidecar tests cover multi-chunk binary data, Unicode names, empty files, randomized
ciphertext, wrong keys and every authenticated binding, tampered IV/content/tag,
malicious authenticated records, collisions, links and protection of existing output.
The temporary-daemon restoration test now seals its captured package with the real
crypto adapter, copies only opaque ciphertext through stand-in storage, receives and
verifies it, then restores registered worktrees, notes, native contexts and chat state
under the existing durable quarantine. These fixtures start no native agents.

The [daemon artifact store](cloud-workspace-artifacts.md) provides exclusive,
operation-bound ciphertext staging, bounded I/O and verified durable receipts.
The [private host bridge](cloud-workspace-bridge.md) connects Noise delivery to those
daemon-owned artifacts. Host runtime dispatch, archive cleanup,
complete environment/data/account readiness and authenticated ownership activation
remain coordinator integration. No real cloud transfer or model session is enabled.
