//! A browser's authenticated reviewer session handed to the native review WebView using a
//! single-use, short-lived S256 code. Neither the callback nor the return target carries a cookie.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{Identity, Reject, Service, host, now_ms, random_hex};

const CODE_MS: u64 = 60_000;
const MAX_CODES: usize = 4096;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Authorization {
    challenge: String,
    state: String,
    screen: Option<String>,
    thread: Option<String>,
}

fn invalid() -> Reject {
    Reject(
        StatusCode::BAD_REQUEST,
        "This mobile sign-in request is invalid: open the design in Chauffeur again".into(),
    )
}

fn unreserved(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
}

fn valid_target(screen: Option<&str>, thread: Option<&str>) -> bool {
    !screen.is_some_and(|s| {
        s.len() > 1024
            || s.contains([':', '?', '#'])
            || s.chars().any(char::is_control)
            || crate::files::safe_relative(s).is_none()
    }) && !thread.is_some_and(|s| !unreserved(s, 1, 128))
}

pub(crate) fn email_query(
    screen: Option<&str>,
    thread: Option<&str>,
    mobile: Option<&Authorization>,
) -> Result<String, Reject> {
    if !valid_target(screen, thread) {
        return Err(invalid());
    }
    if let Some(mobile) = mobile {
        mobile.validate()?;
        if screen.is_some_and(|s| mobile.screen.as_deref() != Some(s))
            || thread.is_some_and(|t| mobile.thread.as_deref() != Some(t))
        {
            return Err(invalid());
        }
        return Ok(format!("?{}", mobile.query()));
    }
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    if let Some(screen) = screen {
        query.append_pair("screen", screen);
    }
    if let Some(thread) = thread {
        query.append_pair("thread", thread);
    }
    let query = query.finish();
    Ok(if query.is_empty() {
        query
    } else {
        format!("?{query}")
    })
}

impl Authorization {
    pub(crate) fn validate(&self) -> Result<(), Reject> {
        if URL_SAFE_NO_PAD
            .decode(&self.challenge)
            .ok()
            .is_none_or(|b| b.len() != 32)
            || self.challenge.len() != 43
            || !unreserved(&self.state, 32, 128)
            || !valid_target(self.screen.as_deref(), self.thread.as_deref())
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub(crate) fn query(&self) -> String {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query
            .append_pair("mobile-auth", "1")
            .append_pair("challenge", &self.challenge)
            .append_pair("state", &self.state);
        self.target_fields(&mut query);
        query.finish()
    }

    fn target_fields(&self, query: &mut url::form_urlencoded::Serializer<'_, String>) {
        if let Some(screen) = &self.screen {
            query.append_pair("screen", screen);
        }
        if let Some(thread) = &self.thread {
            query.append_pair("thread", thread);
        }
    }

    fn target(&self, id: &str) -> String {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("mobile", "1").append_pair("native", "1");
        self.target_fields(&mut query);
        format!("/s/{id}?{}", query.finish())
    }
}

struct Code {
    identity: Identity,
    share: String,
    authorization: Authorization,
    expires: u64,
}

#[derive(Default)]
pub(crate) struct Codes(Mutex<HashMap<String, Code>>);

impl Codes {
    fn issue(
        &self,
        share: &str,
        identity: Identity,
        authorization: Authorization,
        now: u64,
    ) -> Result<String, Reject> {
        let mut codes = self.0.lock().unwrap();
        codes.retain(|_, code| code.expires > now);
        if codes.len() >= MAX_CODES {
            return Err(Reject(
                StatusCode::TOO_MANY_REQUESTS,
                "Mobile sign-in is busy: try again in a minute".into(),
            ));
        }
        let token = random_hex(32);
        codes.insert(
            token.clone(),
            Code {
                identity,
                share: share.into(),
                authorization,
                expires: now + CODE_MS,
            },
        );
        Ok(token)
    }

    fn redeem(&self, id: &str, params: &Exchange, now: u64) -> Result<Code, Reject> {
        let mut codes = self.0.lock().unwrap();
        codes.retain(|_, code| code.expires > now);
        let code = codes.get(&params.code).ok_or_else(invalid)?;
        if code.share != id
            || code.authorization.state != params.state
            || !unreserved(&params.verifier, 43, 128)
            || URL_SAFE_NO_PAD.encode(Sha256::digest(params.verifier.as_bytes()))
                != code.authorization.challenge
        {
            return Err(invalid());
        }
        codes.remove(&params.code).ok_or_else(invalid)
    }
}

fn same_origin(service: &Service, headers: &HeaderMap) -> Result<(), Reject> {
    let expected = url::Url::parse(&service.config.public_url)
        .map_err(|_| invalid())?
        .origin()
        .ascii_serialization();
    if headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) != Some(expected.as_str())
        || headers
            .get("x-chauffeur-mobile")
            .and_then(|h| h.to_str().ok())
            != Some("1")
    {
        return Err(Reject(
            StatusCode::FORBIDDEN,
            "Open mobile sign-in from the shared design in Chauffeur".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn authorize(
    State(service): State<Arc<Service>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(params): Json<Authorization>,
) -> Result<Response, Reject> {
    same_origin(&service, &headers)?;
    params.validate()?;
    let identity = service.reviewer_identity(&headers)?;
    service.viewable(&id, &identity)?;
    let state = params.state.clone();
    let code = service.mobile.issue(&id, identity, params, now_ms())?;
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("code", &code)
        .append_pair("state", &state)
        .finish();
    Ok((
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Json(serde_json::json!({"callback": format!("chauffeur://review/auth?{query}")})),
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Exchange {
    code: String,
    verifier: String,
    state: String,
}

pub(crate) async fn exchange(
    State(service): State<Arc<Service>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Form(params): Form<Exchange>,
) -> Result<Response, Reject> {
    same_origin(&service, &headers)?;
    let code = service.mobile.redeem(&id, &params, now_ms())?;
    service.viewable(&id, &code.identity)?;
    Ok((
        [
            (
                header::SET_COOKIE,
                service.sessions.issue(&code.identity, host(&headers)),
            ),
            (header::CACHE_CONTROL, "no-store".into()),
            (header::REFERRER_POLICY, "no-referrer".into()),
        ],
        Redirect::to(&code.authorization.target(&id)),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authorization() -> Authorization {
        Authorization {
            challenge: URL_SAFE_NO_PAD.encode(Sha256::digest("a".repeat(64).as_bytes())),
            state: "b".repeat(64),
            screen: Some("missing screen.html".into()),
            thread: Some("missing-thread".into()),
        }
    }

    #[test]
    fn mobile_codes_expire_at_one_minute_and_are_lost_on_restart() {
        let codes = Codes::default();
        let auth = authorization();
        let identity = Identity::new("bob@example.com", None);
        let code = codes
            .issue("share", identity.clone(), auth.clone(), 100)
            .unwrap();
        let exchange = Exchange {
            code,
            state: auth.state.clone(),
            verifier: "a".repeat(64),
        };
        assert!(codes.redeem("share", &exchange, 100 + CODE_MS).is_err());
        let code = codes.issue("share", identity, auth, 100).unwrap();
        let exchange = Exchange { code, ..exchange };
        assert!(Codes::default().redeem("share", &exchange, 101).is_err());
        assert!(codes.redeem("share", &exchange, 100 + CODE_MS - 1).is_ok());
        assert!(codes.redeem("share", &exchange, 100 + CODE_MS - 1).is_err());
    }

    #[test]
    fn email_links_keep_exact_targets_without_requiring_them_to_exist() {
        let auth = authorization();
        let query = email_query(None, None, Some(&auth)).unwrap();
        let pairs: HashMap<_, _> = url::form_urlencoded::parse(&query.as_bytes()[1..]).collect();
        assert_eq!(pairs["screen"], "missing screen.html");
        assert_eq!(pairs["thread"], "missing-thread");
        assert_eq!(pairs["challenge"], auth.challenge);
        assert_eq!(pairs["state"], auth.state);
        assert_eq!(pairs["mobile-auth"], "1");
        assert_eq!(
            email_query(Some("a.html"), Some("thread"), None).unwrap(),
            "?screen=a.html&thread=thread"
        );
        assert!(email_query(Some("other.html"), None, Some(&auth)).is_err());
        for screen in [
            "../private.html",
            "//evil.example",
            "https://evil.example",
            "a.html#other",
            "a.html\r\n",
        ] {
            assert!(email_query(Some(screen), None, None).is_err(), "{screen:?}");
        }
    }
}
