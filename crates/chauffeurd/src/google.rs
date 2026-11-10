//! Google sign-in, as a desktop app does it: the browser opens Google's consent page, Google
//! redirects to a one-shot loopback listener with a code, which is exchanged (with PKCE) for a
//! refresh token. Each connection to the office server, and each share, then gets a fresh ID token
//! from it. Core owns sign-in, independently of whether the Office extension is enabled.

use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::secrets::SecretName;
use crate::settings::OfficeSettings;
use crate::{Error, Result};

/// Kept under its first name: users signed in before it left the Office stay signed in.
pub const REFRESH_TOKEN: SecretName<'static> = SecretName {
    service: "chauffeur-office",
    file: "office-google-refresh-token",
    label: "Chauffeur: Office sign-in",
};
const AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
/// How long the user has to finish signing in, in the browser.
const SIGN_IN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

pub struct Session {
    pub email: String,
    /// The account's Google Workspace domain (the ID token's `hd`); none for a personal account.
    pub team_domain: Option<String>,
    /// None for a development server, which needs no Google account.
    pub refresh_token: Option<String>,
}

pub struct Client {
    id: String,
    secret: String,
    /// The Workspace domain sign-in is limited to; empty, any Google account.
    hosted_domain: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct Tokens {
    id_token: Option<String>,
    refresh_token: Option<String>,
    error_description: Option<String>,
    error: Option<String>,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// `application/x-www-form-urlencoded`, for the token endpoint.
fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => out.push(b),
                    Err(_) => out.extend_from_slice(&bytes[i..i + 3]),
                }
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `code` and `state` of Google's redirect, from the request line `GET /?code=…&state=… HTTP/1.1`.
pub fn redirect_params(request: &str) -> (Option<String>, Option<String>, Option<String>) {
    let target = request.split_whitespace().nth(1).unwrap_or("");
    let query = target.split_once('?').map_or("", |(_, q)| q);
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "code" => code = Some(decode(value)),
            "state" => state = Some(decode(value)),
            "error" => error = Some(decode(value)),
            _ => {}
        }
    }
    (code, state, error)
}

/// An ID token's claims. Not a check of the token: the servers it is sent to verify it.
fn claims_of(id_token: &str) -> Option<serde_json::Value> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The e-mail an ID token names.
pub fn email_of(id_token: &str) -> Option<String> {
    claims_of(id_token)?["email"]
        .as_str()
        .map(str::to_lowercase)
}

/// The Google Workspace domain an ID token names (`hd`); none for a personal Google account.
pub fn team_domain_of(id_token: &str) -> Option<String> {
    claims_of(id_token)?["hd"]
        .as_str()
        .map(str::to_lowercase)
        .filter(|domain| !domain.is_empty())
}

impl Client {
    pub fn from_settings(settings: &OfficeSettings) -> Option<Client> {
        let id = settings.google_client_id.trim();
        let secret = settings.google_client_secret.trim();
        (!id.is_empty() && !secret.is_empty()).then(|| Client {
            id: id.to_string(),
            secret: secret.to_string(),
            hosted_domain: settings.google_hosted_domain.trim().to_string(),
            http: reqwest::Client::new(),
        })
    }

    async fn token(&self, body: String) -> Result<Tokens> {
        let response = self
            .http
            .post(TOKEN)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("Could not reach Google: {e}")))?;
        let tokens: Tokens = response.json().await.map_err(|e| {
            Error::Unavailable(format!("Google answered something unexpected: {e}"))
        })?;
        if let Some(error) = tokens.error.clone() {
            return Err(Error::Conflict(format!(
                "Google refused the sign-in ({}); sign in to Chauffeur again",
                tokens.error_description.clone().unwrap_or(error)
            )));
        }
        Ok(tokens)
    }

    /// Opens Google's consent page with `open` and waits for the user to finish there.
    pub async fn sign_in(
        &self,
        open: impl FnOnce(String) -> std::io::Result<()>,
    ) -> Result<Session> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let redirect = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
        let verifier = b64(&rand::random::<[u8; 32]>());
        let challenge = b64(&Sha256::digest(verifier.as_bytes()));
        let state = b64(&rand::random::<[u8; 16]>());
        let mut params = vec![
            ("client_id", self.id.as_str()),
            ("redirect_uri", &redirect),
            ("response_type", "code"),
            ("scope", "openid email profile"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ];
        if !self.hosted_domain.is_empty() {
            params.push(("hd", &self.hosted_domain));
        }
        let url = format!("{AUTHORIZE}?{}", form(&params));
        open(url).map_err(|e| Error::Unavailable(format!("Could not open the browser: {e}")))?;

        let code = tokio::time::timeout(SIGN_IN_TIMEOUT, async {
            loop {
                let (mut stream, _) = listener.accept().await?;
                let mut buffer = vec![0u8; 8192];
                let n = stream.read(&mut buffer).await?;
                let request = String::from_utf8_lossy(&buffer[..n]).to_string();
                let (code, got_state, error) = redirect_params(&request);
                // A favicon request, or anything that is not Google's redirect: ignored.
                if code.is_none() && error.is_none() {
                    let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n").await;
                    continue;
                }
                let ok = error.is_none() && got_state.as_deref() == Some(state.as_str());
                let page = if ok {
                    "Signed in to Chauffeur. You can close this tab."
                } else {
                    "The sign-in did not go through. Go back to Chauffeur and try again."
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{page}",
                    page.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                return Ok::<_, std::io::Error>(if ok { code } else { None }.ok_or(error));
            }
        })
        .await
        .map_err(|_| Error::Conflict("The sign-in took longer than 5 minutes: try again".into()))??
        .map_err(|error| {
            Error::Conflict(format!(
                "Google did not sign you in ({}): try again",
                error.unwrap_or_else(|| "the answer did not match".into())
            ))
        })?;

        let tokens = self
            .token(form(&[
                ("code", &code),
                ("client_id", &self.id),
                ("client_secret", &self.secret),
                ("redirect_uri", &redirect),
                ("grant_type", "authorization_code"),
                ("code_verifier", &verifier),
            ]))
            .await?;
        let id_token = tokens
            .id_token
            .as_deref()
            .ok_or_else(|| Error::Unavailable("Google gave no ID token: try again".into()))?;
        let email = email_of(id_token)
            .ok_or_else(|| Error::Unavailable("Google gave no ID token: try again".into()))?;
        let team_domain = team_domain_of(id_token);
        let refresh_token = tokens
            .refresh_token
            .ok_or_else(|| Error::Unavailable("Google gave no refresh token: try again".into()))?;
        Ok(Session {
            email,
            team_domain,
            refresh_token: Some(refresh_token),
        })
    }

    /// A fresh ID token for the office server.
    pub async fn id_token(&self, refresh_token: &str) -> std::result::Result<String, String> {
        let tokens = self
            .token(form(&[
                ("refresh_token", refresh_token),
                ("client_id", &self.id),
                ("client_secret", &self.secret),
                ("grant_type", "refresh_token"),
            ]))
            .await
            .map_err(|e| e.to_string())?;
        tokens
            .id_token
            .ok_or_else(|| "Google gave no ID token: sign in to Chauffeur again".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_redirect_gives_its_code_and_state_and_ignores_the_rest() {
        let (code, state, error) =
            redirect_params("GET /?state=s%2B1&code=4%2F0Ab&scope=email HTTP/1.1\r\nHost: x\r\n");
        assert_eq!(
            (code.as_deref(), state.as_deref(), error),
            (Some("4/0Ab"), Some("s+1"), None)
        );
        let (code, _, error) = redirect_params("GET /?error=access_denied HTTP/1.1");
        assert_eq!((code, error.as_deref()), (None, Some("access_denied")));
        assert_eq!(
            redirect_params("GET /favicon.ico HTTP/1.1"),
            (None, None, None)
        );
    }

    #[test]
    fn a_form_escapes_what_is_not_unreserved() {
        assert_eq!(form(&[("a", "x y/z"), ("b", "é")]), "a=x%20y%2Fz&b=%C3%A9");
    }

    #[test]
    fn the_email_is_read_from_the_id_token_payload() {
        let payload = b64(br#"{"email":"Bob@Example.com"}"#);
        assert_eq!(
            email_of(&format!("h.{payload}.s")).as_deref(),
            Some("bob@example.com")
        );
        assert_eq!(email_of("not-a-token"), None);
    }

    #[test]
    fn the_team_domain_is_the_id_tokens_hd_and_a_personal_account_has_none() {
        let workspace = b64(br#"{"email":"bob@acme.com","hd":"Acme.com"}"#);
        assert_eq!(
            team_domain_of(&format!("h.{workspace}.s")).as_deref(),
            Some("acme.com")
        );
        let personal = b64(br#"{"email":"bob@gmail.com"}"#);
        assert_eq!(team_domain_of(&format!("h.{personal}.s")), None);
    }
}
