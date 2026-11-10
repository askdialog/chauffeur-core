//! Read-only leases for a sandboxed screen and its relative resources. Its opaque origin
//! cannot send the reviewer's SameSite cookie; the lease never authorizes an API request.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::{Identity, now_ms, random_hex};

const LEASE_MS: u64 = 15 * 60 * 1000;

struct Lease {
    share: String,
    identity: Identity,
    expires: u64,
}

#[derive(Default)]
pub(crate) struct Renders(Mutex<HashMap<String, Lease>>);

impl Renders {
    /// Reuse the URL while an authenticated page renews it, so interactive screens do not reload.
    pub(crate) fn issue(&self, share: &str, identity: &Identity) -> String {
        self.issue_at(share, identity, now_ms())
    }

    fn issue_at(&self, share: &str, identity: &Identity, now: u64) -> String {
        let mut leases = self.0.lock().unwrap();
        leases.retain(|_, lease| lease.expires > now);
        if let Some((token, lease)) = leases
            .iter_mut()
            .find(|(_, lease)| lease.share == share && lease.identity.email == identity.email)
        {
            lease.expires = now + LEASE_MS;
            return token.clone();
        }
        let token = random_hex(32);
        leases.insert(
            token.clone(),
            Lease {
                share: share.into(),
                identity: identity.clone(),
                expires: now + LEASE_MS,
            },
        );
        token
    }

    pub(crate) fn identity(&self, share: &str, token: &str) -> Option<Identity> {
        self.identity_at(share, token, now_ms())
    }

    fn identity_at(&self, share: &str, token: &str, now: u64) -> Option<Identity> {
        let leases = self.0.lock().unwrap();
        let lease = leases.get(token)?;
        (lease.share == share && lease.expires > now).then(|| lease.identity.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_render_lease_is_bound_to_its_share_and_reviewer_and_expires_without_renewal() {
        let renders = Renders::default();
        let alice = Identity::new("alice@example.com", None);
        let bob = Identity::new("bob@example.com", None);
        let token = renders.issue_at("ab12", &alice, 0);
        assert_eq!(token.len(), 64);
        assert_eq!(renders.identity_at("ab12", &token, 1), Some(alice.clone()));
        assert_eq!(renders.identity_at("cd34", &token, 1), None);
        assert_eq!(renders.identity_at("ab12", "unknown", 1), None);
        assert_ne!(renders.issue_at("ab12", &bob, 1), token);
        assert_ne!(renders.issue_at("cd34", &alice, 1), token);
        assert_eq!(renders.issue_at("ab12", &alice, LEASE_MS - 1), token);
        assert_eq!(
            renders.identity_at("ab12", &token, LEASE_MS),
            Some(alice.clone())
        );
        assert_eq!(renders.identity_at("ab12", &token, 2 * LEASE_MS - 1), None);
        assert_ne!(renders.issue_at("ab12", &alice, 2 * LEASE_MS), token);
        assert_eq!(Renders::default().identity_at("ab12", &token, 1), None);
    }
}
