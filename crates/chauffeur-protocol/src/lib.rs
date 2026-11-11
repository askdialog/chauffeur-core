//! Wire types shared by every Chauffeur process.
//!
//! - [`model`]: the domain objects clients see (workspaces, worktrees, chats).
//! - [`events`]: the event log clients subscribe to.
//! - [`client`]: JSON-RPC methods a client (CLI, app, mobile) calls on the daemon.
//! - [`workspace_share`]: a shared workspace, between the daemon, the share service and a guest.
//! - [`office`]: the Office's presence, between the app, the daemon and the office server.
//! - [`sidecar`]: JSON-RPC methods between the daemon and the agent sidecar.
//! - [`peer`]: JSON-RPC methods a daemon calls on another daemon's `/peer` path.
//! - [`worker`]: GCP workers a workspace can be transferred to.
//! - [`conductor`]: the import from Conductor.
//! - [`routines`]: prompts the daemon runs by itself at set times.
//! - [`rpc`]: the JSON-RPC 2.0 envelopes.
//! - [`methods`]: how a method enum is declared; [`access`]: who may call a client method and
//!   whether it is relayed; [`generate`]: the method maps written to TypeScript.
//!
//! Every type derives `ts_rs::TS`; `cargo test -p chauffeur-protocol` regenerates the TypeScript
//! bindings in `protocol-ts/src/generated`.

#[macro_use]
pub mod methods;

pub mod access;
pub mod client;
pub mod cloud_archive;
pub mod cloud_backup;
pub mod cloud_transfer;
pub mod conductor;
pub mod database;
pub mod design_share;
pub mod dev_services;
pub mod events;
pub mod extension_host;
pub mod extensions;
pub mod generate;
pub mod identity;
pub mod model;
pub mod office;
pub mod peer;
pub mod routines;
pub mod rpc;
pub mod sidecar;
pub mod worker;
pub mod workspace_share;

/// The client protocol's version (`ClientCall`), sent by clients in `hello`. Bumped whenever its
/// methods or their params' fields change: `protocol.lock` keeps the fingerprint of each version,
/// and a test fails when the methods no longer match it.
pub const PROTOCOL_VERSION: u32 = 15;

/// The oldest client protocol the daemon still serves: clients of an older Chauffeur keep working
/// against a newer daemon. Raised only when a method they rely on goes.
pub const MIN_PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use crate::client::ClientMethod;
    use crate::methods::fingerprint;
    use crate::peer::PeerMethod;

    #[test]
    fn each_protocol_matches_the_fingerprint_of_its_version() {
        let lock = include_str!("../protocol.lock");
        for (name, version, fingerprint, constant) in [
            (
                "client",
                super::PROTOCOL_VERSION,
                fingerprint::<ClientMethod>(),
                "PROTOCOL_VERSION in crates/chauffeur-protocol/src/lib.rs",
            ),
            (
                "peer",
                crate::peer::PEER_PROTOCOL_VERSION,
                fingerprint::<PeerMethod>(),
                "PEER_PROTOCOL_VERSION in crates/chauffeur-protocol/src/peer.rs",
            ),
            (
                "extension-host",
                crate::extension_host::HOST_API_VERSION,
                fingerprint::<crate::extension_host::ExtensionHostMethod>(),
                "HOST_API_VERSION in extension_host.rs",
            ),
            (
                "extension-engine",
                crate::extension_host::HOST_API_VERSION,
                fingerprint::<crate::extension_host::ExtensionEngineNotificationMethod>(),
                "HOST_API_VERSION in extension_host.rs",
            ),
        ] {
            let wanted = format!("{name} {version} {fingerprint}");
            let locked = lock
                .lines()
                .find(|line| line.starts_with(&format!("{name} ")));
            assert!(
                locked == Some(wanted.as_str()),
                "The {name} protocol changed (its methods or their params' fields) since \
                 protocol.lock was written: bump {constant} unless this branch already did since \
                 the last release, then set its `{name}` line to `{wanted}`"
            );
        }
    }
}
