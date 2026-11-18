//! `chauffeur activity`: the user's own activity over a time window, every workspace included
//! (docs/adr/activity-for-recaps.md). What an agent reads to recap the user's work; the text cuts
//! long messages and answers, `--json` gives them whole as the daemon keeps them.
use crate::client::Client;
use anyhow::{Context, bail};
use chauffeur_protocol::activity::{ActivityParams, ActivityResult};
use chauffeur_protocol::client::ClientCall;
use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone};

/// In the text, a message and a chat's last answer are cut to these many characters.
const MESSAGE_CHARS: usize = 300;
const ANSWER_CHARS: usize = 600;

pub async fn run(
    client: &mut Client,
    since: String,
    until: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let now = Local::now();
    let params = ActivityParams {
        since: instant(&since, now).context("--since")?,
        until: until
            .map(|until| instant(&until, now).context("--until"))
            .transpose()?,
    };
    let result: ActivityResult = client.call(ClientCall::ActivityList(params)).await?;
    crate::print_or(json, &result, || render(&result));
    Ok(())
}

/// An instant as the daemon takes it (RFC 3339), from what a user or an agent types: RFC 3339,
/// a local date ("2026-11-10", its midnight), a local date and time ("2026-11-10 09:00"), or a
/// span back from now ("36h", "3d").
fn instant(text: &str, now: DateTime<Local>) -> anyhow::Result<String> {
    let text = text.trim();
    if let Ok(at) = DateTime::parse_from_rfc3339(text) {
        return Ok(at.to_rfc3339());
    }
    let local = if let Ok(day) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        Some(day.and_hms_opt(0, 0, 0).expect("midnight exists"))
    } else {
        [
            "%Y-%m-%d %H:%M",
            "%Y-%m-%dT%H:%M",
            "%Y-%m-%d %H:%M:%S",
            "%Y-%m-%dT%H:%M:%S",
        ]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
    };
    if let Some(local) = local {
        // A time skipped by a daylight-saving change has no instant; a repeated one takes the
        // first.
        return match Local.from_local_datetime(&local).earliest() {
            Some(at) => Ok(at.to_rfc3339()),
            None => bail!("{text} does not exist in local time (a daylight-saving change)"),
        };
    }
    let span = text
        .strip_suffix('h')
        .and_then(|n| n.parse().ok())
        .map(Duration::hours)
        .or_else(|| {
            text.strip_suffix('d')
                .and_then(|n| n.parse().ok())
                .map(Duration::days)
        });
    match span {
        Some(span) => Ok((now - span).to_rfc3339()),
        None => bail!(
            "{text:?} is not a time: give 2026-11-10, 2026-11-10 09:00 (local time), an RFC 3339 instant, or a span back from now such as 36h or 3d"
        ),
    }
}

fn local(rfc3339: &str, format: &str) -> String {
    DateTime::parse_from_rfc3339(rfc3339)
        .map(|at| at.with_timezone(&Local).format(format).to_string())
        .unwrap_or_else(|_| rfc3339.to_string())
}

/// One line, at most `max` characters, an ellipsis marking a cut.
fn line(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", text[..at].trim_end()),
        None => text,
    }
}

fn render(result: &ActivityResult) -> String {
    let chats: usize = result.workspaces.iter().map(|w| w.chats.len()).sum();
    let messages: usize = result
        .workspaces
        .iter()
        .flat_map(|w| &w.chats)
        .map(|c| c.messages.len())
        .sum();
    let mut out = vec![format!(
        "Your messages from {} to {} (local time): {} workspaces, {chats} chats, {messages} messages.",
        local(&result.since, "%a %Y-%m-%d %H:%M"),
        local(&result.until, "%a %Y-%m-%d %H:%M"),
        result.workspaces.len(),
    )];
    for w in &result.workspaces {
        let mut facts = vec![format!("branch {}", w.branch)];
        if !w.repos.is_empty() {
            facts.push(format!("repos {}", w.repos.join(", ")));
        }
        if !w.tickets.is_empty() {
            let tickets: Vec<&str> = w.tickets.iter().map(|t| t.identifier.as_str()).collect();
            facts.push(format!("tickets {}", tickets.join(", ")));
        }
        if let Some(archived) = &w.archived_at {
            facts.push(format!("archived {}", local(archived, "%Y-%m-%d")));
        }
        out.push(String::new());
        out.push(format!("## {} ({}) — {}", w.name, w.id, facts.join("; ")));
        for c in &w.chats {
            out.push(format!(
                "### {} ({}, {})",
                c.title.as_deref().unwrap_or("Untitled chat"),
                c.agent.as_str(),
                c.id
            ));
            for m in &c.messages {
                out.push(format!(
                    "- {}  {}",
                    local(&m.at, "%a %d %H:%M"),
                    line(&m.text, MESSAGE_CHARS)
                ));
            }
            if let Some(answer) = &c.last_answer {
                out.push(format!(
                    "  Last answer, {}: {}",
                    local(&answer.at, "%a %d %H:%M"),
                    line(&answer.text, ANSWER_CHARS)
                ));
            }
        }
    }
    if !result.workspaces.is_empty() {
        out.push(String::new());
        out.push("Messages and answers are cut here; --json gives them whole. Pull requests and tickets: gh and Linear.".into());
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::activity::{ActivityChat, ActivityMessage, ActivityWorkspace};
    use chauffeur_protocol::model::AgentKind;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 12, 10, 0, 0).unwrap()
    }

    fn at(text: &str) -> DateTime<Local> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Local)
    }

    #[test]
    fn local_dates_times_and_spans_become_instants() {
        let day = Local.with_ymd_and_hms(2026, 11, 10, 0, 0, 0).unwrap();
        let nine = Local.with_ymd_and_hms(2026, 11, 10, 9, 0, 0).unwrap();
        assert_eq!(at(&instant("2026-11-10", now()).unwrap()), day);
        assert_eq!(at(&instant("2026-11-10 09:00", now()).unwrap()), nine);
        assert_eq!(at(&instant("2026-11-10T09:00", now()).unwrap()), nine);
        assert_eq!(
            at(&instant("36h", now()).unwrap()),
            now() - Duration::hours(36)
        );
        assert_eq!(
            at(&instant("2d", now()).unwrap()),
            now() - Duration::days(2)
        );
        assert_eq!(
            instant("2026-11-10T09:00:00+01:00", now()).unwrap(),
            "2026-11-10T09:00:00+01:00"
        );
    }

    #[test]
    fn an_unreadable_time_says_what_to_give() {
        let error = instant("yesterday", now()).unwrap_err().to_string();
        assert!(error.contains("2026-11-10 09:00"), "{error}");
    }

    #[test]
    fn the_text_keeps_each_message_on_one_cut_line_and_points_to_json() {
        let long = "word ".repeat(200);
        let result = ActivityResult {
            since: "2026-11-10T08:00:00.000Z".into(),
            until: "2026-11-12T09:00:00.000Z".into(),
            workspaces: vec![ActivityWorkspace {
                id: "w1".into(),
                name: "Slack cache invalidation".into(),
                branch: "justin/slack-cache".into(),
                path: "/tmp/w1".into(),
                repos: vec!["dialog-ecom".into()],
                tickets: vec![],
                archived_at: None,
                chats: vec![ActivityChat {
                    id: "c1".into(),
                    title: Some("Cache".into()),
                    agent: AgentKind::Claude,
                    messages: vec![ActivityMessage {
                        at: "2026-11-10T09:17:00.000Z".into(),
                        text: format!("first line\nsecond line {long}"),
                    }],
                    last_answer: Some(ActivityMessage {
                        at: "2026-11-10T09:18:00.000Z".into(),
                        text: "Done.\n\nThe cache is cleared.".into(),
                    }),
                }],
            }],
        };
        let text = render(&result);
        assert!(text.contains("1 workspaces, 1 chats, 1 messages"), "{text}");
        assert!(
            text.contains(
                "## Slack cache invalidation (w1) — branch justin/slack-cache; repos dialog-ecom"
            ),
            "{text}"
        );
        let message = text
            .lines()
            .find(|l| l.contains("first line"))
            .expect("the message is listed");
        assert!(message.contains("first line second line"), "{message}");
        assert!(
            message.ends_with('…') && message.chars().count() < 330,
            "{message}"
        );
        assert!(text.contains(": Done. The cache is cleared."), "{text}");
        assert!(text.contains("--json gives them whole"), "{text}");
    }
}
