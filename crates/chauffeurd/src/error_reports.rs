//! Reports of the errors the app shows as toasts, sent to Chauffeur's Sentry as the user chooses
//! (`error_reports`; docs/adr/error-reports-to-sentry.md).
//!
//! - Each failed request of a client gets an id ([`ErrorReports::failed`]), sent back in its error
//!   response; the daemon keeps the failure, with its log lines (the `request` span, gathered by
//!   [`RequestLogs`]), for the report the app may send with its toast.
//! - [`ErrorReports::report`] builds a Sentry event from the toast and that failure, scrubbed of
//!   the home folder and anything like a token, and posts it as an envelope. The same error is
//!   sent once per 10 minutes, 50 a day at most; one Sentry cannot take waits in
//!   `~/.chauffeur/error-reports/queue`, sent again every 10 minutes for 7 days.
//! - Only a build given `CHAUFFEUR_SENTRY_DSN` at compile time (the release workflow's) sends any.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

use chauffeur_protocol::client::ErrorReportParams;
use chauffeur_protocol::model::{ErrorReportOutcome, ErrorReports as Mode};
use chauffeur_protocol::rpc::RpcError;
use regex::Regex;
use sentry_types::Dsn;
use sentry_types::protocol::v7::{Context, Envelope, Event, Level, OsContext, User};
use tracing::field::{Field, Visit};
use tracing::span;
use tracing_subscriber::layer::Context as LayerContext;
use tracing_subscriber::registry::LookupSpan;

use crate::error::{Error, Result};

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Failed requests kept for a report.
const KEPT_FAILURES: usize = 50;
/// Log lines kept, of every request.
const KEPT_LINES: usize = 2000;
/// Log lines of one request a report carries, its last ones.
const LINES_PER_REPORT: usize = 50;
const SAME_ERROR_EVERY: Duration = Duration::from_secs(10 * 60);
const REPORTS_PER_DAY: u32 = 50;
const QUEUE_KEPT: Duration = Duration::from_secs(7 * 24 * 3600);
const FLUSH_EVERY: Duration = Duration::from_secs(10 * 60);

/// Sentry's address, compiled into packaged builds only.
pub fn compiled_dsn() -> Option<Dsn> {
    option_env!("CHAUFFEUR_SENTRY_DSN")
        .filter(|dsn| !dsn.trim().is_empty())
        .and_then(|dsn| Dsn::from_str(dsn.trim()).ok())
}

pub struct ErrorReports {
    dsn: Option<Dsn>,
    dir: PathBuf,
    /// The user's home folder, which reports name `~`.
    user_home: String,
    failures: Mutex<VecDeque<Failure>>,
    sent: Mutex<Sent>,
    http: reqwest::Client,
}

/// A request that failed, as a report tells it.
#[derive(Clone, Debug)]
struct Failure {
    id: String,
    method: String,
    code: i64,
    message: String,
    lines: Vec<String>,
}

#[derive(Default)]
struct Sent {
    /// When each error (by fingerprint) was last reported.
    last: HashMap<String, Instant>,
    day: Option<chrono::NaiveDate>,
    today: u32,
}

impl ErrorReports {
    pub fn new(home: &Path, dsn: Option<Dsn>) -> ErrorReports {
        ErrorReports {
            dsn,
            dir: home.join("error-reports"),
            user_home: std::env::var("HOME").unwrap_or_default(),
            failures: Mutex::new(VecDeque::new()),
            sent: Mutex::new(Sent::default()),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
        }
    }

    /// This build can send reports.
    pub fn available(&self) -> bool {
        self.dsn.is_some()
    }

    /// Keeps the failure of request `request` (`method`), with its log lines, and gives the error
    /// response its id.
    pub fn failed(&self, request: &str, method: &str, mut error: RpcError) -> RpcError {
        if self.dsn.is_none() {
            return error;
        }
        let failure = Failure {
            id: request.to_string(),
            method: method.to_string(),
            code: error.code,
            message: error.message.clone(),
            lines: request_lines(request),
        };
        let mut failures = self.failures.lock().unwrap();
        if failures.len() == KEPT_FAILURES {
            failures.pop_front();
        }
        failures.push_back(failure);
        error.failure = Some(request.to_string());
        error
    }

    /// Sends the report of an error toast; the app asks once the user agreed, or on its own when
    /// the setting is `always`.
    pub async fn report(
        &self,
        mode: Mode,
        params: ErrorReportParams,
    ) -> Result<ErrorReportOutcome> {
        let Some(dsn) = &self.dsn else {
            return Err(Error::Unavailable(
                "this build of Chauffeur does not report errors".into(),
            ));
        };
        if mode == Mode::Never {
            return Err(Error::Conflict(
                "error reports are turned off: turn them on in Settings › General".into(),
            ));
        }
        let failure = params.failure.as_deref().and_then(|id| {
            let failures = self.failures.lock().unwrap();
            failures.iter().rev().find(|f| f.id == id).cloned()
        });
        let message = scrub(&params.message, &self.user_home);
        let fingerprint = vec![
            failure
                .as_ref()
                .map_or_else(|| "app".to_string(), |f| f.method.clone()),
            normalized(&message),
        ];
        if let Some(outcome) = self.throttle(&fingerprint.join("\n")) {
            return Ok(outcome);
        }
        let event = self.event(message, fingerprint, failure.as_ref(), &params);
        let event_id = event.event_id.to_string();
        let mut envelope = Envelope::new();
        envelope.add_item(event);
        let mut body = Vec::new();
        envelope.to_writer(&mut body)?;
        match self.post(dsn, &body).await {
            Ok(()) => Ok(ErrorReportOutcome::Sent),
            Err(Delivery::Later(why)) => {
                tracing::warn!(%why, "could not send the error report: kept to send later");
                self.keep(&event_id, &body)?;
                Ok(ErrorReportOutcome::Queued)
            }
            Err(Delivery::Refused(why)) => Err(Error::Unavailable(format!(
                "Sentry refused the report: {why}"
            ))),
        }
    }

    /// Sends the reports kept for later, every 10 minutes while the daemon runs; one older than 7
    /// days is dropped.
    pub fn spawn_flush(self: &std::sync::Arc<Self>, shutdown: tokio_util::sync::CancellationToken) {
        if self.dsn.is_none() {
            return;
        }
        let reports = self.clone();
        tokio::spawn(async move {
            loop {
                reports.flush().await;
                tokio::select! {
                    _ = tokio::time::sleep(FLUSH_EVERY) => {}
                    _ = shutdown.cancelled() => break,
                }
            }
        });
    }

    async fn flush(&self) {
        let Some(dsn) = &self.dsn else { return };
        let Ok(entries) = std::fs::read_dir(self.dir.join("queue")) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|at| SystemTime::now().duration_since(at).ok())
                .is_some_and(|age| age > QUEUE_KEPT);
            if old {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            let Ok(body) = std::fs::read(&path) else {
                continue;
            };
            match self.post(dsn, &body).await {
                Ok(()) | Err(Delivery::Refused(_)) => {
                    let _ = std::fs::remove_file(&path);
                }
                // Still out of reach: the next round tries again.
                Err(Delivery::Later(_)) => return,
            }
        }
    }

    /// `Some` when this report is not sent: the same error went less than 10 minutes ago, or
    /// today's reports are spent.
    fn throttle(&self, fingerprint: &str) -> Option<ErrorReportOutcome> {
        let mut sent = self.sent.lock().unwrap();
        let now = Instant::now();
        if sent
            .last
            .get(fingerprint)
            .is_some_and(|at| now.duration_since(*at) < SAME_ERROR_EVERY)
        {
            return Some(ErrorReportOutcome::Duplicate);
        }
        let today = chrono::Local::now().date_naive();
        if sent.day != Some(today) {
            sent.day = Some(today);
            sent.today = 0;
        }
        if sent.today >= REPORTS_PER_DAY {
            return Some(ErrorReportOutcome::Limited);
        }
        sent.today += 1;
        sent.last
            .retain(|_, at| now.duration_since(*at) < SAME_ERROR_EVERY);
        sent.last.insert(fingerprint.to_string(), now);
        None
    }

    fn event(
        &self,
        message: String,
        fingerprint: Vec<String>,
        failure: Option<&Failure>,
        params: &ErrorReportParams,
    ) -> Event<'static> {
        let mut event = Event::new();
        event.level = Level::Error;
        event.message = Some(message);
        event.fingerprint = fingerprint.into_iter().map(Into::into).collect();
        event.release = Some(format!("chauffeur@{VERSION}").into());
        event.user = Some(User {
            id: Some(self.installation()),
            ..Default::default()
        });
        event.tags.insert("daemon_version".into(), VERSION.into());
        if let Some(app) = &params.app_version {
            event
                .tags
                .insert("app_version".into(), scrub(app, &self.user_home));
        }
        event.contexts.insert(
            "os".into(),
            Context::Os(Box::new(OsContext {
                name: Some(std::env::consts::OS.into()),
                version: os_version(),
                kernel_version: Some(std::env::consts::ARCH.into()),
                ..Default::default()
            })),
        );
        if let Some(failure) = failure {
            event.tags.insert("method".into(), failure.method.clone());
            event.tags.insert("code".into(), failure.code.to_string());
            event.extra.insert(
                "daemon_error".into(),
                scrub(&failure.message, &self.user_home).into(),
            );
            if !failure.lines.is_empty() {
                let lines: Vec<String> = failure
                    .lines
                    .iter()
                    .map(|l| scrub(l, &self.user_home))
                    .collect();
                event
                    .extra
                    .insert("daemon_log".into(), lines.join("\n").into());
            }
        }
        event
    }

    async fn post(&self, dsn: &Dsn, body: &[u8]) -> Result<(), Delivery> {
        let auth = dsn.to_auth(Some(&format!("chauffeur/{VERSION}")));
        let response = self
            .http
            .post(dsn.envelope_api_url().as_str())
            .header("X-Sentry-Auth", auth.to_string())
            .header("Content-Type", "application/x-sentry-envelope")
            .body(body.to_vec())
            .send()
            .await
            .map_err(|error| Delivery::Later(error.to_string()))?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else if status.as_u16() == 429 || status.is_server_error() {
            Err(Delivery::Later(status.to_string()))
        } else {
            Err(Delivery::Refused(status.to_string()))
        }
    }

    fn keep(&self, id: &str, body: &[u8]) -> Result<()> {
        let queue = self.dir.join("queue");
        std::fs::create_dir_all(&queue)?;
        std::fs::write(queue.join(format!("{id}.envelope")), body)?;
        Ok(())
    }

    /// An id of this installation, random, made with its first report: nothing about the user.
    fn installation(&self) -> String {
        let path = self.dir.join("installation");
        if let Ok(id) = std::fs::read_to_string(&path)
            && !id.trim().is_empty()
        {
            return id.trim().to_string();
        }
        let id = uuid::Uuid::new_v4().to_string();
        let _ = std::fs::create_dir_all(&self.dir).and_then(|()| std::fs::write(&path, &id));
        id
    }
}

enum Delivery {
    /// Sentry could not be reached, or asked to wait.
    Later(String),
    /// Sentry will not take it (a bad DSN, a malformed envelope): sending again would not help.
    Refused(String),
}

fn os_version() -> Option<String> {
    static VERSION: LazyLock<Option<String>> = LazyLock::new(|| {
        let output = if cfg!(target_os = "macos") {
            std::process::Command::new("sw_vers")
                .arg("-productVersion")
                .output()
        } else {
            std::process::Command::new("uname").arg("-r").output()
        };
        output
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|v| !v.is_empty())
    });
    VERSION.clone()
}

/// `text` without the home folder (`~` in its place) nor anything that looks like a credential.
pub fn scrub(text: &str, home: &str) -> String {
    static SECRETS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
        [
            // Credentials in a URL: `https://user:token@host`, `https://token@host`.
            (r"(?i)\b([a-z][a-z0-9+.-]*://)[^/\s@]+@", "${1}[redacted]@"),
            (r"\bgh[pousr]_[A-Za-z0-9]{16,}", "[redacted]"),
            (r"\bgithub_pat_[A-Za-z0-9_]{16,}", "[redacted]"),
            (r"\blin_(?:api|oauth)_[A-Za-z0-9]{16,}", "[redacted]"),
            (r"\bxox[abeoprs]-[A-Za-z0-9-]{8,}", "[redacted]"),
            (r"\bsk-[A-Za-z0-9_-]{16,}", "[redacted]"),
            (r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", "[redacted]"),
            (r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]+", "[redacted]"),
            (r"(?i)\b(bearer|basic|token)\s+[A-Za-z0-9._~+/=-]{8,}", "${1} [redacted]"),
            (
                r#"(?i)\b([a-z_]*(?:token|secret|password|passwd|api_?key|access_?key)[a-z_]*)(["']?\s*[=:]\s*["']?)[^\s"'&,;]+"#,
                "${1}${2}[redacted]",
            ),
        ]
        .into_iter()
        .map(|(pattern, with)| (Regex::new(pattern).expect("a valid pattern"), with))
        .collect()
    });
    let mut text = if home.len() > 1 {
        text.replace(home, "~")
    } else {
        text.to_string()
    };
    for (pattern, with) in SECRETS.iter() {
        text = pattern.replace_all(&text, *with).into_owned();
    }
    text
}

/// The message with what changes from one occurrence to the next (numbers, ids, quoted names)
/// taken out, so that the same error groups as one.
fn normalized(message: &str) -> String {
    static VARYING: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"`[^`]*`|"[^"]*"|'[^']*'|[0-9a-fA-F-]{8,}|\d+"#).expect("a valid pattern")
    });
    VARYING.replace_all(message, "_").into_owned()
}

/// The log lines of every request, newest last, each with its request's id.
static LINES: Mutex<VecDeque<(String, String)>> = Mutex::new(VecDeque::new());

fn request_lines(request: &str) -> Vec<String> {
    let lines = LINES.lock().unwrap();
    let mut mine: Vec<String> = lines
        .iter()
        .filter(|(id, _)| id == request)
        .map(|(_, line)| line.clone())
        .collect();
    if mine.len() > LINES_PER_REPORT {
        mine.drain(..mine.len() - LINES_PER_REPORT);
    }
    mine
}

/// A layer of the daemon's log that keeps the lines logged inside a `request` span (its `id`
/// field), for the report of that request's failure.
pub struct RequestLogs;

struct RequestId(String);

impl<S> tracing_subscriber::Layer<S> for RequestLogs
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: LayerContext<'_, S>) {
        if attrs.metadata().name() != "request" {
            return;
        }
        let mut visitor = FieldText::default();
        attrs.record(&mut visitor);
        if let (Some(request), Some(span)) = (visitor.id, ctx.span(id)) {
            span.extensions_mut().insert(RequestId(request));
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: LayerContext<'_, S>) {
        let Some(scope) = ctx.event_scope(event) else {
            return;
        };
        let Some(request) = scope
            .from_root()
            .find_map(|span| span.extensions().get::<RequestId>().map(|r| r.0.clone()))
        else {
            return;
        };
        let mut visitor = FieldText::default();
        event.record(&mut visitor);
        let meta = event.metadata();
        let line = format!(
            "{} {} {}: {}{}",
            chrono::Utc::now().format("%H:%M:%S%.3f"),
            meta.level(),
            meta.target(),
            visitor.message,
            visitor.fields
        );
        let mut lines = LINES.lock().unwrap();
        if lines.len() == KEPT_LINES {
            lines.pop_front();
        }
        lines.push_back((request, line));
    }
}

#[derive(Default)]
struct FieldText {
    id: Option<String>,
    message: String,
    fields: String,
}

impl Visit for FieldText {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "message" => self.message = format!("{value:?}"),
            "id" => self.id = Some(format!("{value:?}")),
            name => self.fields.push_str(&format!(" {name}={value:?}")),
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_string(),
            "id" => self.id = Some(value.to_string()),
            name => self.fields.push_str(&format!(" {name}={value}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU16, Ordering};
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn scrubbing_takes_out_the_home_folder_and_credentials() {
        let home = "/Users/ada";
        let cases = [
            (
                "could not read /Users/ada/code/app/x.ts",
                "could not read ~/code/app/x.ts",
            ),
            (
                "git push https://x-access-token:ghs_abcdefghijklmnopqrstuvwxyz@github.com/a/b failed",
                "git push https://[redacted]@github.com/a/b failed",
            ),
            (
                "token ghp_ABCDEFGHIJKLMNOPQRSTUVWX refused",
                "token [redacted] refused",
            ),
            (
                "Linear: lin_api_abcdefghijklmnopqrstuvwx is invalid",
                "Linear: [redacted] is invalid",
            ),
            ("Slack xoxb-1234-5678-abcdefgh", "Slack [redacted]"),
            (
                "Authorization: Bearer abc.def.ghi123",
                "Authorization: Bearer [redacted]",
            ),
            (
                "api_key=sk-abcdefghijklmnopqrstu&x=1",
                "api_key=[redacted]&x=1",
            ),
            (
                "password: hunter22 was wrong",
                "password: [redacted] was wrong",
            ),
            ("AWS AKIAABCDEFGHIJKLMNOP", "AWS [redacted]"),
            ("nothing to hide in 42 files", "nothing to hide in 42 files"),
        ];
        for (text, scrubbed) in cases {
            assert_eq!(scrub(text, home), scrubbed, "{text}");
        }
    }

    #[test]
    fn the_same_error_with_other_names_and_numbers_normalizes_alike() {
        assert_eq!(
            normalized("Could not move `feat/a` to 3: 018f2c3a-aaaa exists"),
            normalized("Could not move `fix/b` to 12: 018f2c3a-bbbb exists"),
        );
        assert_ne!(
            normalized("Could not move it"),
            normalized("Could not pin it")
        );
    }

    #[test]
    fn the_same_error_goes_once_per_ten_minutes_and_fifty_a_day() {
        let temp = tempfile::tempdir().unwrap();
        let reports = ErrorReports::new(temp.path(), None);
        assert_eq!(reports.throttle("a"), None);
        assert_eq!(reports.throttle("a"), Some(ErrorReportOutcome::Duplicate));
        for n in 1..REPORTS_PER_DAY {
            assert_eq!(reports.throttle(&n.to_string()), None);
        }
        assert_eq!(reports.throttle("b"), Some(ErrorReportOutcome::Limited));
    }

    #[test]
    fn a_failure_carries_the_log_lines_of_its_request_only() {
        let subscriber = tracing_subscriber::registry().with(RequestLogs);
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("outside any request");
            tracing::error_span!("request", id = "r1").in_scope(|| {
                tracing::warn!(repo = "app", "gh is slow");
            });
            tracing::error_span!("request", id = "r2").in_scope(|| {
                tracing::info!("another request");
            });
        });
        let lines = request_lines("r1");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].ends_with("WARN chauffeurd::error_reports::tests: gh is slow repo=app"));
    }

    /// A Sentry of the test's own: answers each envelope with `status`, counting them.
    async fn sentry(status: Arc<AtomicU16>) -> (Dsn, Arc<Mutex<Vec<String>>>) {
        let received = Arc::new(Mutex::new(Vec::new()));
        let app = axum::Router::new().route(
            "/api/1/envelope/",
            axum::routing::post({
                let received = received.clone();
                move |headers: axum::http::HeaderMap, body: String| async move {
                    assert!(
                        headers["x-sentry-auth"]
                            .to_str()
                            .unwrap()
                            .contains("sentry_key=public")
                    );
                    received.lock().unwrap().push(body);
                    axum::http::StatusCode::from_u16(status.load(Ordering::SeqCst)).unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dsn = Dsn::from_str(&format!("http://public@127.0.0.1:{port}/1")).unwrap();
        (dsn, received)
    }

    fn params(message: &str, failure: Option<&str>) -> ErrorReportParams {
        ErrorReportParams {
            message: message.into(),
            failure: failure.map(Into::into),
            app_version: Some("0.29.1".into()),
        }
    }

    #[tokio::test]
    async fn a_report_carries_the_daemon_s_failure_scrubbed() {
        let temp = tempfile::tempdir().unwrap();
        let (dsn, received) = sentry(Arc::new(AtomicU16::new(200))).await;
        let reports = ErrorReports::new(temp.path(), Some(dsn));
        let error = reports.failed(
            "req-1",
            "workspace.archive",
            RpcError {
                code: 409,
                message: "git push https://user:secretvalue123@github.com/x failed".into(),
                data: None,
                failure: None,
            },
        );
        assert_eq!(error.failure.as_deref(), Some("req-1"));
        let outcome = reports
            .report(
                Mode::Ask,
                params("Could not archive: git push failed", Some("req-1")),
            )
            .await
            .unwrap();
        assert_eq!(outcome, ErrorReportOutcome::Sent);
        let body = received.lock().unwrap().join("\n");
        assert!(
            body.contains("Could not archive: git push failed"),
            "{body}"
        );
        assert!(body.contains(r#""method":"workspace.archive""#), "{body}");
        assert!(body.contains("https://[redacted]@github.com/x"), "{body}");
        assert!(!body.contains("secretvalue123"), "{body}");
        assert!(!body.contains("server_name"), "{body}");
        // The same error again is not sent.
        let again = reports
            .report(
                Mode::Always,
                params("Could not archive: git push failed", Some("req-1")),
            )
            .await
            .unwrap();
        assert_eq!(again, ErrorReportOutcome::Duplicate);
        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_report_sentry_cannot_take_waits_in_the_queue_then_goes() {
        let temp = tempfile::tempdir().unwrap();
        let status = Arc::new(AtomicU16::new(503));
        let (dsn, received) = sentry(status.clone()).await;
        let reports = ErrorReports::new(temp.path(), Some(dsn));
        let outcome = reports
            .report(Mode::Always, params("Could not pin it", None))
            .await
            .unwrap();
        assert_eq!(outcome, ErrorReportOutcome::Queued);
        let queue = temp.path().join("error-reports/queue");
        assert_eq!(std::fs::read_dir(&queue).unwrap().count(), 1);
        status.store(200, Ordering::SeqCst);
        reports.flush().await;
        assert_eq!(std::fs::read_dir(&queue).unwrap().count(), 0);
        assert_eq!(received.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn nothing_is_sent_without_a_dsn_or_when_turned_off() {
        let temp = tempfile::tempdir().unwrap();
        let reports = ErrorReports::new(temp.path(), None);
        let error = RpcError {
            code: 500,
            message: "x".into(),
            data: None,
            failure: None,
        };
        assert_eq!(reports.failed("r", "m", error).failure, None);
        assert!(matches!(
            reports.report(Mode::Always, params("x", None)).await,
            Err(Error::Unavailable(_))
        ));
        let (dsn, received) = sentry(Arc::new(AtomicU16::new(200))).await;
        let reports = ErrorReports::new(temp.path(), Some(dsn));
        assert!(matches!(
            reports.report(Mode::Never, params("x", None)).await,
            Err(Error::Conflict(_))
        ));
        assert!(received.lock().unwrap().is_empty());
    }
}
