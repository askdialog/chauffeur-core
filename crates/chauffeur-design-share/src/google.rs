//! Checks a Google ID token: signed by one of Google's current keys, issued for one of our OAuth
//! clients (the daemons' desktop client, the page's web client), of a verified e-mail. Any Google
//! account gets in; its Google Workspace domain (`hd`, none for a personal account) comes along,
//! for shares opened to a team domain.

use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::Identity;

const CERTS: &str = "https://www.googleapis.com/oauth2/v3/certs";
const ISSUERS: [&str; 2] = ["accounts.google.com", "https://accounts.google.com"];
/// Google rotates its keys over days: an hour's cache is safe.
const KEYS_FOR: Duration = Duration::from_secs(3600);
/// An unknown key id makes the server fetch the keys again, at most this often.
const REFETCH_AFTER: Duration = Duration::from_secs(60);

#[derive(Deserialize, Clone)]
struct Jwk {
    kid: String,
    n: String,
    e: String,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct Claims {
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: bool,
    /// The Workspace domain of the account; absent for a personal Gmail account.
    pub hd: Option<String>,
    /// The account's display name (the `profile` scope).
    pub name: Option<String>,
}

pub struct Verifier {
    client_ids: Vec<String>,
    http: reqwest::Client,
    keys: Mutex<Option<(Instant, Vec<Jwk>)>>,
}

impl Verifier {
    pub fn new(client_ids: Vec<String>) -> Verifier {
        Verifier {
            client_ids,
            http: reqwest::Client::new(),
            keys: Mutex::new(None),
        }
    }

    async fn key(&self, kid: &str) -> Result<Jwk, String> {
        let mut keys = self.keys.lock().await;
        let fresh = |at: &Instant, max: Duration| at.elapsed() < max;
        if let Some((at, list)) = keys.as_ref()
            && fresh(at, KEYS_FOR)
            && let Some(key) = list.iter().find(|k| k.kid == kid)
        {
            return Ok(key.clone());
        }
        if keys
            .as_ref()
            .is_some_and(|(at, _)| fresh(at, REFETCH_AFTER))
        {
            return Err("The ID token is signed with a key Google does not publish".into());
        }
        let jwks: Jwks = async {
            self.http
                .get(CERTS)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await
        }
        .await
        .map_err(|error: reqwest::Error| format!("Could not read Google's keys: {error}"))?;
        let key = jwks.keys.iter().find(|k| k.kid == kid).cloned();
        *keys = Some((Instant::now(), jwks.keys));
        key.ok_or_else(|| "The ID token is signed with a key Google does not publish".into())
    }

    /// Who the token is, once checked.
    pub async fn verify(&self, token: &str) -> Result<Identity, String> {
        let header = decode_header(token).map_err(|error| format!("Not an ID token: {error}"))?;
        let kid = header.kid.ok_or("The ID token names no key")?;
        let jwk = self.key(&kid).await?;
        let key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
            .map_err(|error| format!("Google published a key that is not RSA: {error}"))?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&self.client_ids);
        validation.set_issuer(&ISSUERS);
        let data = decode::<Claims>(token, &key, &validation)
            .map_err(|error| format!("The ID token was refused: {error}"))?;
        check_claims(&data.claims)
    }
}

/// A Google account whose e-mail Google verified, with its Workspace domain when it has one; or
/// why not.
pub fn check_claims(claims: &Claims) -> Result<Identity, String> {
    let email = claims
        .email
        .clone()
        .ok_or("The ID token has no e-mail: sign in again")?;
    if !claims.email_verified {
        return Err(format!("{email} is not verified by Google"));
    }
    Ok(Identity::new(&email, claims.name.as_deref()).in_workspace(claims.hd.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(email: &str, verified: bool, hd: Option<&str>, name: Option<&str>) -> Claims {
        Claims {
            email: Some(email.into()),
            email_verified: verified,
            hd: hd.map(Into::into),
            name: name.map(Into::into),
        }
    }

    #[test]
    fn any_verified_google_account_gets_in_with_its_workspace_domain() {
        let bob = check_claims(&claims(
            "Bob@example.com",
            true,
            Some("Example.com"),
            Some("Bob L."),
        ))
        .unwrap();
        assert_eq!(
            (bob.email.as_str(), bob.name.as_str(), bob.hd.as_deref()),
            ("bob@example.com", "Bob L.", Some("example.com"))
        );
        let gmail = check_claims(&claims("alice.p@gmail.com", true, None, None)).unwrap();
        assert_eq!(
            (gmail.email.as_str(), gmail.name.as_str(), gmail.hd),
            ("alice.p@gmail.com", "Alice", None)
        );
        assert!(
            check_claims(&claims("bob@example.com", false, Some("example.com"), None)).is_err()
        );
        assert!(
            check_claims(&Claims {
                email: None,
                email_verified: true,
                hd: None,
                name: None
            })
            .is_err()
        );
    }
}
