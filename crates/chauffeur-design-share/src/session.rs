//! A reader's session for the page and API: a signed token in an HttpOnly cookie, made once their
//! Google ID token or their sign-in link was checked, for 30 days. The key is the operator's
//! (`--session-key`, kept in GCP Secret Manager), so that a restart signs no one out; without
//! one, a key drawn at start does, as in development. The same key signs the links that stop
//! e-mails to an address.
//!
//! With a cookie domain (`--cookie-domain`), a session made on a host of that domain is that
//! domain's, so one sign-in serves every host of the service under it (its design and workspace
//! hosts); on any other host, as on loopback, it stays the host's own.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use crate::Identity;

pub const COOKIE: &str = "share_session";
pub const SESSION_FOR: Duration = Duration::from_secs(30 * 24 * 3600);

pub struct Sessions {
    key: [u8; 32],
    /// Without `Secure`, for a server on loopback in development.
    secure: bool,
    /// The parent domain sessions are scoped to (`example.com`), lowercase; none keeps each
    /// session to the host it was made on.
    domain: Option<String>,
}

impl Sessions {
    /// With the operator's key, else one drawn now.
    pub fn new(key: Option<[u8; 32]>, secure: bool) -> Sessions {
        Sessions {
            key: key.unwrap_or_else(rand::random),
            secure,
            domain: None,
        }
    }

    /// Sessions made on `domain` or a host under it are the whole domain's.
    pub fn for_domain(self, domain: Option<&str>) -> Sessions {
        Sessions {
            domain: domain
                .map(|d| d.trim().trim_matches('.').to_lowercase())
                .filter(|d| !d.is_empty()),
            ..self
        }
    }

    /// `; Domain=<domain>` when `host` (a `Host` header) is the cookie domain or under it: a
    /// browser refuses a cookie for a domain its host is not part of.
    fn domain_attribute(&self, host: Option<&str>) -> String {
        let (Some(domain), Some(host)) = (&self.domain, host) else {
            return String::new();
        };
        let host = host.rsplit_once(':').map_or(host, |(name, port)| {
            if port.bytes().all(|b| b.is_ascii_digit()) {
                name
            } else {
                host
            }
        });
        let host = host.trim_end_matches('.').to_lowercase();
        let under = host
            .strip_suffix(domain.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.ends_with('.'));
        if under {
            format!("; Domain={domain}")
        } else {
            String::new()
        }
    }

    /// A signature of `message` by this service's key, hex: what a link of the service carries
    /// to prove the service wrote it.
    pub fn sign(&self, message: &str) -> String {
        hex::encode(hmac(&self.key, message.as_bytes()))
    }

    /// The `Set-Cookie` value for an identity signing in on `host`.
    pub fn issue(&self, identity: &Identity, host: Option<&str>) -> String {
        let expires = now() + SESSION_FOR.as_secs();
        let token = self.token(identity, expires);
        format!(
            "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}{}",
            SESSION_FOR.as_secs(),
            if self.secure { "; Secure" } else { "" },
            self.domain_attribute(host)
        )
    }

    /// The `Set-Cookie` values that end a session on `host`: the domain's and, made before the
    /// service had a cookie domain, the host's own.
    pub fn clear(&self, host: Option<&str>) -> Vec<String> {
        let secure = if self.secure { "; Secure" } else { "" };
        let host_only = format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}");
        match self.domain_attribute(host) {
            domain if domain.is_empty() => vec![host_only],
            domain => vec![format!("{host_only}{domain}"), host_only],
        }
    }

    fn token(&self, identity: &Identity, expires: u64) -> String {
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&(expires, &identity.email, &identity.name, &identity.hd))
                .expect("a session serializes"),
        );
        format!(
            "{payload}.{}",
            URL_SAFE_NO_PAD.encode(hmac(&self.key, payload.as_bytes()))
        )
    }

    /// Who a `Cookie` header is, when it carries a session of this server that has not expired.
    /// A host's own session and its domain's can both be there: the first valid one counts.
    pub fn identity(&self, cookie_header: Option<&str>) -> Option<Identity> {
        cookie_header?
            .split(';')
            .map(str::trim)
            .filter_map(|pair| pair.strip_prefix(&format!("{COOKIE}=")))
            .find_map(|token| self.token_identity(token))
    }

    fn token_identity(&self, token: &str) -> Option<Identity> {
        let (payload, signature) = token.split_once('.')?;
        let given = URL_SAFE_NO_PAD.decode(signature).ok()?;
        if given != hmac(&self.key, payload.as_bytes()) {
            return None;
        }
        let (expires, email, name, hd): (u64, String, String, Option<String>) =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
        (expires > now()).then_some(Identity { email, name, hd })
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// HMAC-SHA256 (RFC 2104) with a 32-byte key.
fn hmac(key: &[u8; 32], message: &[u8]) -> [u8; 32] {
    let mut inner = [0x36u8; 64];
    let mut outer = [0x5cu8; 64];
    for (i, byte) in key.iter().enumerate() {
        inner[i] ^= byte;
        outer[i] ^= byte;
    }
    let inner_hash = Sha256::new()
        .chain_update(inner)
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(outer)
        .chain_update(inner_hash)
        .finalize()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alice() -> Identity {
        Identity::new("alice@example.com", Some("Alice")).in_workspace(Some("example.com"))
    }

    #[test]
    fn a_session_comes_back_from_its_cookie_and_from_nothing_else() {
        let sessions = Sessions::new(None, true);
        let set = sessions.issue(&alice(), None);
        assert!(
            set.starts_with("share_session=") && set.contains("HttpOnly") && set.contains("Secure")
        );
        let cookie = set.split(';').next().unwrap().to_string();
        assert_eq!(
            sessions.identity(Some(&format!("other=1; {cookie}"))),
            Some(alice())
        );
        assert_eq!(sessions.identity(None), None);
        assert_eq!(sessions.identity(Some("share_session=not.a.token")), None);
        let (payload, _) = cookie.split_once('.').unwrap();
        assert_eq!(
            sessions.identity(Some(&format!("{payload}.AAAA"))),
            None,
            "a wrong signature"
        );
        assert_eq!(
            Sessions::new(None, true).identity(Some(&cookie)),
            None,
            "another server's key"
        );
        assert!(
            !Sessions::new(None, false)
                .issue(&alice(), None)
                .contains("Secure")
        );
    }

    #[test]
    fn a_session_lasts_thirty_days_and_outlives_a_restart_with_the_same_key() {
        let key = [7u8; 32];
        let set = Sessions::new(Some(key), true).issue(&alice(), None);
        assert!(set.contains(&format!("Max-Age={}", 30 * 24 * 3600)));
        let cookie = set.split(';').next().unwrap().to_string();
        let restarted = Sessions::new(Some(key), true);
        assert_eq!(restarted.identity(Some(&cookie)), Some(alice()));
        assert_eq!(
            restarted.identity(Some(&cookie)).unwrap().hd.as_deref(),
            Some("example.com")
        );
        assert_ne!(restarted.sign("a"), Sessions::new(None, true).sign("a"));
    }

    #[test]
    fn a_session_made_under_the_cookie_domain_is_the_whole_domains() {
        let sessions = Sessions::new(None, true).for_domain(Some("Chauffeur.Example.com"));
        for host in [
            "design.chauffeur.example.com",
            "share.chauffeur.example.com:443",
            "chauffeur.example.com",
        ] {
            let set = sessions.issue(&alice(), Some(host));
            assert!(
                set.ends_with("; Domain=chauffeur.example.com")
                    && set.contains("HttpOnly")
                    && set.contains("Secure")
                    && set.contains("SameSite=Lax"),
                "{host}: {set}"
            );
            let cleared = sessions.clear(Some(host));
            assert_eq!(cleared.len(), 2, "{host}");
            assert!(
                cleared[0].ends_with("; Domain=chauffeur.example.com")
                    && cleared[0].contains("Max-Age=0")
            );
            assert!(!cleared[1].contains("Domain=") && cleared[1].contains("Max-Age=0"));
        }
        for host in [
            Some("design.example.org"),
            Some("evilchauffeur.example.com"),
            Some("127.0.0.1:8090"),
            None,
        ] {
            assert!(
                !sessions.issue(&alice(), host).contains("Domain="),
                "{host:?}"
            );
            assert_eq!(sessions.clear(host).len(), 1, "{host:?}");
        }
        assert!(
            !Sessions::new(None, true)
                .issue(&alice(), Some("design.chauffeur.example.com"))
                .contains("Domain="),
            "no cookie domain"
        );
    }

    #[test]
    fn a_valid_session_counts_beside_a_stale_one_of_the_same_name() {
        let sessions = Sessions::new(None, true);
        let stale = sessions.token(&alice(), now() - 1);
        let valid = sessions.token(&alice(), now() + 60);
        assert_eq!(
            sessions.identity(Some(&format!(
                "share_session={stale}; share_session={valid}"
            ))),
            Some(alice())
        );
    }

    #[test]
    fn an_expired_session_is_none() {
        let sessions = Sessions::new(None, true);
        let token = sessions.token(&alice(), now() - 1);
        assert_eq!(
            sessions.identity(Some(&format!("share_session={token}"))),
            None
        );
    }

    #[test]
    fn the_signature_depends_on_the_key_and_on_every_byte_of_the_message() {
        let key = [0x0bu8; 32];
        let a = hmac(&key, b"what do ya want for nothing?");
        let b = hmac(&key, b"what do ya want for nothing?");
        assert_eq!(a, b);
        assert_ne!(a, hmac(&[0x0cu8; 32], b"what do ya want for nothing?"));
        assert_ne!(a, hmac(&key, b"what do ya want for nothing?!"));
    }
}
