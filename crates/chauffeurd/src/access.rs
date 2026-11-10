//! Who is calling: an agent is told apart from the user by its process, a descendant of a sidecar
//! the daemon started. The sidecar runs in its own process group, killed when it exits, so that
//! its agents do not outlive it under PID 1, where the walk would not find them. A process that
//! detaches itself from that tree (`setsid`, a double fork), or the agents of a daemon killed
//! outright, escape this check; it is a guard rail against a prompt injection, not a sandbox.
//! What each kind of connection may call is declared with the methods
//! (`chauffeur_protocol::access`).

pub(crate) use chauffeur_protocol::access::Access;
use chauffeur_protocol::client::ClientCall;

use crate::{Daemon, Error};

/// Refuses a call the connection may not make, saying who may.
pub(crate) fn check(access: Access, call: &ClientCall) -> Result<(), Error> {
    chauffeur_protocol::access::check(access, call).map_err(Error::BadRequest)
}

/// Whether the process behind a Unix socket connection runs under one of the daemon's sidecars.
/// A peer the socket cannot name (no credentials, no pid) counts as one: unknown is the least
/// trusted, never the user.
pub(crate) fn is_agent(daemon: &Daemon, peer_pid: Option<i32>) -> bool {
    let Some(pid) = peer_pid.and_then(|pid| u32::try_from(pid).ok()) else {
        return true;
    };
    descends_from(pid, &daemon.chats.sidecar.pids())
}

/// Whether `pid` is one of `ancestors` or has one of them above it.
pub(crate) fn descends_from(mut pid: u32, ancestors: &[u32]) -> bool {
    if ancestors.is_empty() {
        return false;
    }
    // Deep enough for any shell under an agent; a cycle cannot happen but a bound costs nothing.
    for _ in 0..64 {
        if pid <= 1 {
            return false;
        }
        if ancestors.contains(&pid) {
            return true;
        }
        match crate::cloud_process::parent(pid) {
            Some(parent) => pid = parent,
            None => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_without_a_pid_is_taken_for_an_agent() {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(
            temp.path().join("home"),
            temp.path().join("workspaces"),
            temp.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(&paths.settings, "linear_enabled = false\n").unwrap();
        let daemon = Daemon::open(paths).unwrap();
        assert!(is_agent(&daemon, None));
        assert!(is_agent(&daemon, Some(-1)));
        assert!(!is_agent(&daemon, Some(std::process::id() as i32)));
        daemon.shutdown.cancel();
    }

    #[test]
    fn a_child_process_descends_from_this_one_and_not_the_reverse() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let me = std::process::id();
        assert!(descends_from(child.id(), &[me]));
        assert!(descends_from(child.id(), &[child.id()]));
        assert!(!descends_from(me, &[child.id()]));
        assert!(!descends_from(child.id(), &[]));
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
