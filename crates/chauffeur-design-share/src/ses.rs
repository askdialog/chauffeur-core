//! Amazon SES, as the share service sends its e-mails (`deploy/aws-mail.sh` sets it up): no AWS
//! key is stored. The relay VM's service account asks the GCP metadata server for a Google ID
//! token, STS trades it for an hour of a role's credentials (`AssumeRoleWithWebIdentity`), and
//! SES v2's `SendEmail` is called with them, signed with Signature Version 4. Invitations go to
//! any address, so the account is out of SES's sandbox; e-mails still leave one a second.

use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

use crate::mail::Email;

pub const REGION: &str = "eu-west-1";
/// The ID token's audience, which the role's trust policy requires.
const AUDIENCE: &str = "chauffeur-design-share";
const METADATA_TOKEN: &str =
    "http://169.254.169.254/computeMetadata/v1/instance/service-accounts/default/identity";
/// The sandbox's sending rate is one e-mail a second.
const SPACING: Duration = Duration::from_millis(1_100);
/// Credentials closer than this to their end are renewed first.
const MARGIN: chrono::TimeDelta = chrono::TimeDelta::minutes(5);

pub struct Ses {
    http: reqwest::Client,
    role_arn: String,
    /// `From:` of every e-mail: the operator's sender name and address (`--mail-from-name`,
    /// `--mail-from`); the owner is named in the e-mail, and replies go to them.
    from: String,
    credentials: tokio::sync::Mutex<Option<Credentials>>,
    /// When the last e-mail left; held while one is sent, so that they leave one at a time.
    last: tokio::sync::Mutex<Option<Instant>>,
}

#[derive(Clone, Debug, PartialEq)]
struct Credentials {
    access_key: String,
    secret_key: String,
    session_token: String,
    expires: chrono::DateTime<chrono::Utc>,
}

impl Ses {
    pub fn new(role_arn: String, from_name: &str, from_address: &str) -> Ses {
        Ses {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("an HTTP client"),
            role_arn,
            from: from_header(from_name, from_address),
            credentials: tokio::sync::Mutex::new(None),
            last: tokio::sync::Mutex::new(None),
        }
    }

    pub async fn send(&self, email: &Email) -> anyhow::Result<()> {
        let mut last = self.last.lock().await;
        if let Some(at) = *last {
            tokio::time::sleep_until((at + SPACING).into()).await;
        }
        let result = self.send_now(email).await;
        *last = Some(Instant::now());
        result
    }

    async fn send_now(&self, email: &Email) -> anyhow::Result<()> {
        let credentials = self.credentials().await?;
        let host = format!("email.{REGION}.amazonaws.com");
        let path = "/v2/email/outbound-emails";
        let body = serde_json::to_vec(&serde_json::json!({
            "FromEmailAddress": self.from,
            "Destination": {"ToAddresses": [email.to]},
            "ReplyToAddresses": email.reply_to.iter().collect::<Vec<_>>(),
            "Content": {"Simple": {
                "Subject": {"Data": email.subject, "Charset": "UTF-8"},
                "Body": {
                    "Text": {"Data": email.text, "Charset": "UTF-8"},
                    "Html": {"Data": email.html, "Charset": "UTF-8"},
                },
            }},
        }))
        .expect("an e-mail serializes");
        let now = chrono::Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let headers = [
            ("content-type", "application/json"),
            ("host", host.as_str()),
            ("x-amz-date", amz_date.as_str()),
            ("x-amz-security-token", credentials.session_token.as_str()),
        ];
        let authorization = authorization(
            &credentials,
            REGION,
            "ses",
            &amz_date,
            "POST",
            path,
            &headers,
            &body,
        );
        let response = self
            .http
            .post(format!("https://{host}{path}"))
            .header("content-type", "application/json")
            .header("x-amz-date", &amz_date)
            .header("x-amz-security-token", &credentials.session_token)
            .header("authorization", authorization)
            .body(body)
            .send()
            .await
            .context("reaching SES")?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            bail!("SES refused the e-mail ({status}): {text}");
        }
        Ok(())
    }

    async fn credentials(&self) -> anyhow::Result<Credentials> {
        let mut cached = self.credentials.lock().await;
        if let Some(credentials) = cached.as_ref()
            && credentials.expires - MARGIN > chrono::Utc::now()
        {
            return Ok(credentials.clone());
        }
        let token = self
            .http
            .get(format!("{METADATA_TOKEN}?audience={AUDIENCE}"))
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .context("asking the metadata server for the VM's Google ID token")?
            .text()
            .await?;
        let form = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("Action", "AssumeRoleWithWebIdentity")
            .append_pair("Version", "2011-06-15")
            .append_pair("RoleArn", &self.role_arn)
            .append_pair("RoleSessionName", AUDIENCE)
            .append_pair("WebIdentityToken", token.trim())
            .finish();
        let response = self
            .http
            .post(format!("https://sts.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form)
            .send()
            .await
            .context("reaching STS")?;
        let status = response.status();
        let xml = response.text().await?;
        if !status.is_success() {
            bail!("STS refused the VM's token ({status}): {xml}");
        }
        let credentials = parse_credentials(&xml).context("reading STS's credentials")?;
        *cached = Some(credentials.clone());
        Ok(credentials)
    }
}

/// `From:`, a name beyond plain ASCII as a MIME encoded word, which SES requires.
fn from_header(name: &str, from: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        from.to_string()
    } else if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || " .-'".contains(c))
    {
        format!("{name} <{from}>")
    } else {
        format!("=?UTF-8?B?{}?= <{from}>", STANDARD.encode(name))
    }
}

fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let start = xml.find(&format!("<{name}>"))? + name.len() + 2;
    let end = start + xml[start..].find(&format!("</{name}>"))?;
    Some(xml[start..end].trim())
}

fn parse_credentials(xml: &str) -> anyhow::Result<Credentials> {
    let field = |name| tag(xml, name).with_context(|| format!("no {name}"));
    Ok(Credentials {
        access_key: field("AccessKeyId")?.to_string(),
        secret_key: field("SecretAccessKey")?.to_string(),
        session_token: field("SessionToken")?.to_string(),
        expires: chrono::DateTime::parse_from_rfc3339(field("Expiration")?)?.into(),
    })
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// The `Authorization` header of a request without a query, Signature Version 4. `headers` are
/// the signed ones, lowercase and sorted, `host` and `x-amz-date` among them.
#[allow(clippy::too_many_arguments)] // each is a part of what is signed
fn authorization(
    credentials: &Credentials,
    region: &str,
    service: &str,
    amz_date: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> String {
    let canonical_headers: String = headers
        .iter()
        .map(|(name, value)| format!("{name}:{}\n", value.trim()))
        .collect();
    let signed_headers = headers
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(";");
    let canonical_request = format!(
        "{method}\n{path}\n\n{canonical_headers}\n{signed_headers}\n{}",
        sha256_hex(body)
    );
    let date = &amz_date[..8];
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let key = [region, service, "aws4_request"].iter().fold(
        hmac(format!("AWS4{}", credentials.secret_key).as_bytes(), date),
        |key, part| hmac(&key, part),
    );
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={}",
        credentials.access_key,
        hex::encode(hmac(&key, &string_to_sign))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Credentials {
        Credentials {
            access_key: "AKIDEXAMPLE".into(),
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            session_token: String::new(),
            expires: chrono::Utc::now(),
        }
    }

    #[test]
    fn a_request_is_signed_as_aws_signs_it() {
        // AWS's Signature Version 4 test suite, `get-vanilla`.
        let headers = [
            ("host", "example.amazonaws.com"),
            ("x-amz-date", "20150830T123600Z"),
        ];
        assert_eq!(
            authorization(
                &example(),
                "us-east-1",
                "service",
                "20150830T123600Z",
                "GET",
                "/",
                &headers,
                b"",
            ),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn the_signing_key_is_derived_as_aws_documents_it() {
        // AWS's documented example of a derived signing key (20120215, us-east-1, iam).
        let key = ["us-east-1", "iam", "aws4_request"].iter().fold(
            hmac(b"AWS4wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "20120215"),
            |key, part| hmac(&key, part),
        );
        assert_eq!(
            hex::encode(key),
            "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d"
        );
    }

    #[test]
    fn sts_credentials_are_read_from_its_answer() {
        let xml = r#"<AssumeRoleWithWebIdentityResponse xmlns="https://sts.amazonaws.com/doc/2011-06-15/">
  <AssumeRoleWithWebIdentityResult>
    <Credentials>
      <AccessKeyId>ASIAEXAMPLE</AccessKeyId>
      <SecretAccessKey>secret/+=</SecretAccessKey>
      <SessionToken>token/+=</SessionToken>
      <Expiration>2026-11-04T13:00:00Z</Expiration>
    </Credentials>
  </AssumeRoleWithWebIdentityResult>
</AssumeRoleWithWebIdentityResponse>"#;
        let credentials = parse_credentials(xml).unwrap();
        assert_eq!(credentials.access_key, "ASIAEXAMPLE");
        assert_eq!(credentials.secret_key, "secret/+=");
        assert_eq!(credentials.session_token, "token/+=");
        assert_eq!(
            credentials.expires.to_rfc3339(),
            "2026-11-04T13:00:00+00:00"
        );
        assert!(parse_credentials("<Error><Code>AccessDenied</Code></Error>").is_err());
    }

    #[test]
    fn the_sender_is_the_operator_s_name_encoded_beyond_ascii() {
        assert_eq!(
            from_header("Chauffeur by Example", "noreply@share.example.com"),
            "Chauffeur by Example <noreply@share.example.com>"
        );
        assert_eq!(
            from_header(
                "Chauffeur chez Exemple, Société",
                "noreply@share.example.com"
            ),
            format!(
                "=?UTF-8?B?{}?= <noreply@share.example.com>",
                STANDARD.encode("Chauffeur chez Exemple, Société")
            )
        );
    }
}
