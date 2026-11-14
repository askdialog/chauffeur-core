use super::*;
use rusqlite::{OptionalExtension, params};

const EVIDENCE: &str = "SELECT e.seq,e.workspace_id,e.chat_id,COALESCE(w.title,w.slug),w.branch,e.ts,
 json_extract(e.payload,'$.event.event.type'),
 substr(COALESCE(json_extract(e.payload,'$.event.event.text'),json_extract(e.payload,'$.event.event.output'),
 json_extract(e.payload,'$.event.event.content'),json_extract(e.payload,'$.event.event.input'),json_extract(e.payload,'$.event.event.outcome'),''),1,CHAR_LIMIT),
 EXISTS(SELECT 1 FROM daily_reviews r WHERE r.workspace_id=w.id) OR EXISTS(SELECT 1 FROM weekly_reviews r WHERE r.workspace_id=w.id),
 json_extract(e.payload,'$.event.event.type')='user_message'
 AND json_extract(e.payload,'$.event.event.suggested_by') IS NULL
 AND json_extract(e.payload,'$.event.event.delegated_answer') IS NULL
 AND c.parent_chat IS NULL AND w.parent_id IS NULL AND c.kind='chat'
 AND NOT EXISTS(SELECT 1 FROM routines r WHERE r.workspace_id=w.id)
 AND COALESCE(json_extract(e.payload,'$.event.event.text'),'') NOT LIKE 'Review my Chauffeur sessions%'
 AND COALESCE(json_extract(e.payload,'$.event.event.text'),'') NOT LIKE 'Write my weekly Chauffeur review%'
 FROM events e JOIN workspaces w ON w.id=e.workspace_id JOIN chats c ON c.id=e.chat_id";

const ORIGINAL: &str = "e.ts>=?1 AND e.ts<?2
 AND NOT EXISTS(SELECT 1 FROM daily_reviews r WHERE r.workspace_id=e.workspace_id)
 AND NOT EXISTS(SELECT 1 FROM weekly_reviews r WHERE r.workspace_id=e.workspace_id)
 AND json_extract(e.payload,'$.event.event.type') IN ('user_message','assistant_text','tool_call','tool_result','turn_ended')";

fn evidence_sql(chars: usize) -> String {
    EVIDENCE.replace("CHAR_LIMIT", &chars.to_string())
}

fn evidence_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewEvidence> {
    let seq = r.get(0)?;
    Ok(ReviewEvidence {
        id: format!("e{seq}"),
        source_seq: seq,
        workspace_id: r.get(1)?,
        chat_id: r.get(2)?,
        workspace: r.get(3)?,
        branch: r.get(4)?,
        at: r.get(5)?,
        role: r.get(6)?,
        quote: r.get(7)?,
        from_review: r.get(8)?,
        human: r.get(9)?,
    })
}

pub(super) fn evidence(store: &Store, seq: i64) -> Result<Option<ReviewEvidence>> {
    store.with(|c| {
        c.query_row(
            &format!("{} WHERE e.seq=?1", evidence_sql(1600)),
            [seq],
            evidence_row,
        )
        .optional()
    })
}

// Only a finished turn changes the input revision: streaming text never starts another model call.
pub(super) fn source_seq(store: &Store) -> Result<i64> {
    store.with(|c| c.query_row("SELECT COALESCE(MAX(e.seq),0) FROM events e
        JOIN (SELECT day,workspace_id FROM daily_reviews UNION ALL SELECT day,workspace_id FROM weekly_reviews) r
        ON r.workspace_id=e.workspace_id
        WHERE e.chat_id=(SELECT id FROM chats WHERE workspace_id=r.workspace_id ORDER BY created_at,id LIMIT 1)
        AND json_extract(e.payload,'$.event.event.type')='turn_ended'", [], |r| r.get(0)))
}

pub(super) fn has_completed_text(store: &Store) -> Result<bool> {
    let since = (Local::now().date_naive() - Days::new(27)).to_string();
    store.with(|c|c.query_row("SELECT EXISTS(SELECT 1 FROM events e JOIN
        (SELECT day,workspace_id FROM daily_reviews UNION ALL SELECT day,workspace_id FROM weekly_reviews) r
        ON r.workspace_id=e.workspace_id WHERE r.day>=?1
        AND e.chat_id=(SELECT id FROM chats WHERE workspace_id=r.workspace_id ORDER BY created_at,id LIMIT 1)
        AND json_extract(e.payload,'$.event.event.type')='assistant_text'
        AND e.seq<=(SELECT MAX(seq) FROM events WHERE chat_id=e.chat_id AND json_extract(payload,'$.event.event.type')='turn_ended'))",
        [since],|r|r.get(0)))
}

struct ReportRow {
    kind: String,
    date: String,
    workspace: String,
    state: Option<String>,
    title: Option<String>,
    chat: Option<String>,
    status: Option<String>,
}

struct ReportFacts {
    summary: Option<String>,
    chars: i64,
    messages: i64,
    outcome: Option<String>,
    ended: i64,
    latest: i64,
}

fn report_facts(store: &Store, chat: &str) -> Result<ReportFacts> {
    store.with(|c|c.query_row("SELECT
        (SELECT substr(json_extract(payload,'$.event.event.text'),1,220) FROM events WHERE chat_id=?1 AND json_extract(payload,'$.event.event.type')='assistant_text' ORDER BY seq DESC LIMIT 1),
        COALESCE(SUM(length(json_extract(payload,'$.event.event.text'))),0),COUNT(*),
        (SELECT json_extract(payload,'$.event.event.outcome') FROM events WHERE chat_id=?1 AND json_extract(payload,'$.event.event.type')='turn_ended' ORDER BY seq DESC LIMIT 1),
        COALESCE(MAX(CASE WHEN json_extract(payload,'$.event.event.type')='turn_ended' THEN seq END),0),
        COALESCE(MAX(CASE WHEN json_extract(payload,'$.event.event.type') IN ('user_message','assistant_text') THEN seq END),0)
        FROM events WHERE chat_id=?1 AND json_extract(payload,'$.event.event.type') IN ('user_message','assistant_text','turn_ended')
        AND NOT(json_extract(payload,'$.event.event.type')='user_message' AND
          (COALESCE(json_extract(payload,'$.event.event.text'),'') LIKE 'Review my Chauffeur sessions%' OR
           COALESCE(json_extract(payload,'$.event.event.text'),'') LIKE 'Write my weekly Chauffeur review%'))",
        [chat],|r|Ok(ReportFacts{summary:r.get(0)?,chars:r.get(1)?,messages:r.get(2)?,outcome:r.get(3)?,ended:r.get(4)?,latest:r.get(5)?})))
}

pub(super) fn reports(store: &Store) -> Result<Vec<ReviewReport>> {
    let required: HashSet<String> = projection(store)?
        .map(|p| {
            p.experiments
                .into_iter()
                .map(|e| e.origin_report_id)
                .collect()
        })
        .unwrap_or_default();
    let rows:Vec<ReportRow>=store.with(|c|c.prepare("SELECT r.kind,r.day,r.workspace_id,w.state,w.title,c.id,c.status FROM (
        SELECT 'daily' AS kind,day,workspace_id FROM daily_reviews UNION ALL SELECT 'weekly',day,workspace_id FROM weekly_reviews) r
        LEFT JOIN workspaces w ON w.id=r.workspace_id
        LEFT JOIN chats c ON c.id=(SELECT id FROM chats WHERE workspace_id=r.workspace_id ORDER BY created_at,id LIMIT 1)
        WHERE r.workspace_id IS NOT NULL ORDER BY r.day DESC,r.kind DESC")?
        .query_map([],|r|Ok(ReportRow{kind:r.get(0)?,date:r.get(1)?,workspace:r.get(2)?,state:r.get(3)?,title:r.get(4)?,chat:r.get(5)?,status:r.get(6)?}))?.collect())?;
    rows.into_iter()
        .enumerate()
        .filter(|(i, r)| *i < 90 || required.contains(&format!("{}:{}", r.kind, r.date)))
        .map(|(_, r)| {
            let facts = r
                .chat
                .as_deref()
                .map(|chat| report_facts(store, chat))
                .transpose()?;
            let status = if r.chat.is_none() || r.state.is_none() {
                ReviewReportStatus::Unavailable
            } else if r.status.as_deref() == Some("error")
                || facts
                    .as_ref()
                    .and_then(|f| f.outcome.as_deref())
                    .is_some_and(|s| s != "success")
            {
                ReviewReportStatus::Failed
            } else if facts.as_ref().is_some_and(|f| {
                f.summary.is_some()
                    && f.outcome.as_deref() == Some("success")
                    && f.ended >= f.latest
            }) && !matches!(
                r.status.as_deref(),
                Some("starting" | "working" | "needs_input")
            ) {
                ReviewReportStatus::Complete
            } else {
                ReviewReportStatus::Incomplete
            };
            Ok(ReviewReport {
                id: format!("{}:{}", r.kind, r.date),
                kind: if r.kind == "weekly" {
                    ReviewCadence::Weekly
                } else {
                    ReviewCadence::Daily
                },
                date: r.date.clone(),
                workspace_id: r.state.as_ref().map(|_| r.workspace),
                chat_id: r.chat,
                title: r
                    .title
                    .unwrap_or_else(|| format!("{} review — {}", r.kind, r.date)),
                status,
                archived: matches!(r.state.as_deref(), Some("archived" | "archiving")),
                shortened: facts
                    .as_ref()
                    .is_some_and(|f| f.chars > 12_000 || f.messages > 100),
                summary: facts.and_then(|f| f.summary),
            })
        })
        .collect()
}

pub(super) fn projection(store: &Store) -> Result<Option<ReviewProjection>> {
    store
        .kv_get("reviews.projection")?
        .map(|s| serde_json::from_str(&s).map_err(|e| Error::Internal(e.into())))
        .transpose()
}

pub(crate) fn followups(store: &Store) -> Result<Vec<ReviewFollowUp>> {
    let rows: Vec<String> = store.with(|c| {
        c.prepare("SELECT json FROM review_followups ORDER BY recorded_at,rowid")?
            .query_map([], |r| r.get(0))?
            .collect()
    })?;
    rows.into_iter()
        .map(|s| serde_json::from_str(&s).map_err(|e| Error::Internal(e.into())))
        .collect()
}

pub(super) struct Input {
    pub seq: i64,
    pub reports: Vec<ReviewReport>,
    pub evidence: HashMap<String, ReviewEvidence>,
    pub source_coverage: Vec<ReviewSourceCoverage>,
    pub followed_ids: HashSet<String>,
    pub request: String,
}

fn sample_day(
    store: &Store,
    day: NaiveDate,
) -> Result<(Vec<ReviewEvidence>, ReviewSourceCoverage)> {
    let (start, end) = crate::daily_review::bounds(day)?;
    let available: u32 = store.with(|c| {
        c.query_row(
            &format!(
                "SELECT COUNT(*) FROM events e JOIN chats c ON c.id=e.chat_id WHERE {ORIGINAL}"
            ),
            params![start, end],
            |r| r.get(0),
        )
    })?;
    let rows:Vec<ReviewEvidence>=store.with(|c|c.prepare(&format!("{} WHERE e.seq IN (
        SELECT seq FROM (SELECT e.seq,ROW_NUMBER() OVER(PARTITION BY e.chat_id ORDER BY
          CASE json_extract(e.payload,'$.event.event.type') WHEN 'user_message' THEN 0 WHEN 'turn_ended' THEN 1
               WHEN 'assistant_text' THEN 2 WHEN 'tool_result' THEN 3 ELSE 4 END,e.seq DESC) AS slot
          FROM events e JOIN chats c ON c.id=e.chat_id WHERE {ORIGINAL})
        WHERE slot<=4 ORDER BY slot,seq DESC LIMIT 32) ORDER BY e.seq",evidence_sql(800)))?
        .query_map(params![start,end],evidence_row)?.collect())?;
    let mut budget = 100_000 / 28;
    let mut out = Vec::new();
    let mut shortened = false;
    for mut row in rows {
        if row.quote.is_empty() {
            continue;
        }
        if budget <= 400 {
            break;
        }
        let count = row.quote.chars().count();
        shortened |= count >= 800;
        let take = count.min(budget - 400);
        if take < count {
            shortened = true;
            row.quote = row.quote.chars().take(take).collect();
        }
        budget -= take + 400;
        out.push(row);
    }
    let included = out.len() as u32;
    Ok((
        out,
        ReviewSourceCoverage {
            date: day.to_string(),
            available,
            included,
            complete: available > 0 && included == available && !shortened,
        },
    ))
}

pub(super) fn input(d: &Daemon) -> Result<Input> {
    let seq = source_seq(&d.store)?;
    let today = Local::now().date_naive();
    let start = (today - Days::new(27)).to_string();
    let mut reports = reports(&d.store)?
        .into_iter()
        .filter(|r| r.date >= start && r.date <= today.to_string())
        .collect::<Vec<_>>();
    let mut evidence = HashMap::new();
    let mut report_text = Vec::new();
    let mut remaining = 80_000usize;
    for report in &mut reports {
        let Some(chat) = &report.chat_id else {
            continue;
        };
        let rows:Vec<ReviewEvidence>=d.store.with(|c|c.prepare(&format!("{} WHERE e.chat_id=?1 AND e.seq<=?2
            AND e.seq<=(SELECT MAX(seq) FROM events WHERE chat_id=?1 AND seq<=?2 AND json_extract(payload,'$.event.event.type')='turn_ended')
            AND json_extract(e.payload,'$.event.event.type') IN ('assistant_text','user_message','turn_ended') ORDER BY e.seq DESC LIMIT 101",evidence_sql(12_000)))?
            .query_map(params![chat,seq],evidence_row)?.collect())?;
        let mut budget = 12_000usize.min(remaining);
        let mut messages = Vec::new();
        if rows.len() > 100 {
            report.shortened = true;
        }
        for mut row in rows.into_iter().take(100) {
            if row.role == "user_message"
                && (row.quote.starts_with("Review my Chauffeur sessions")
                    || row.quote.starts_with("Write my weekly Chauffeur review"))
            {
                continue;
            }
            if budget == 0 {
                report.shortened = true;
                break;
            }
            let count = row.quote.chars().count();
            let used = count.min(budget);
            if used < count {
                row.quote = row.quote.chars().take(used).collect();
                report.shortened = true;
            }
            budget -= used;
            remaining -= used;
            evidence.insert(row.id.clone(), row.clone());
            messages.push(row);
        }
        report.summary = messages
            .iter()
            .find(|r| r.role == "assistant_text")
            .map(|r| r.quote.chars().take(220).collect());
        messages.reverse();
        report_text.push(serde_json::json!({"report":report,"messages":messages}));
    }
    let mut original = Vec::new();
    let mut source_coverage = Vec::new();
    for offset in (0..28).rev() {
        let (rows, coverage) = sample_day(&d.store, today - Days::new(offset))?;
        for row in rows {
            evidence.insert(row.id.clone(), row.clone());
            original.push(row);
        }
        source_coverage.push(coverage);
    }
    let saved = projection(&d.store)?;
    let previous_patterns = saved.as_ref().map(|p| {
        p.patterns
            .iter()
            .map(|p| serde_json::json!({"id":p.id,"title":p.title}))
            .collect::<Vec<_>>()
    });
    let updates = followups(&d.store)?;
    let followed_ids = updates.iter().map(|f| f.experiment_id.clone()).collect();
    let previous=saved.map(|p|p.experiments.into_iter().take(100).map(|e|serde_json::json!({"id":e.id,"title":e.title,"origin_report_id":e.origin_report_id,"origin_date":e.origin_date,"status":e.status})).collect::<Vec<_>>());
    let updates = updates
        .into_iter()
        .rev()
        .take(50)
        .map(|mut f| {
            f.note = f.note.chars().take(1000).collect();
            f
        })
        .collect::<Vec<_>>();
    let request = serde_json::to_string(
        &serde_json::json!({"today":today.to_string(),"coverage_start":start,
        "reports":report_text,"original_observations":original,"source_coverage":source_coverage,
        "previous_recommendations":previous,"previous_patterns":previous_patterns,"user_followups":updates,
        "limits":"Completed report portions only, at most 80,000 report text characters. Original sources are sampled separately for each local day and chat: up to 32 events/day, 4/chat/day, 800 characters/excerpt and 100,000 characters overall. source_coverage says how much was omitted on each day. A sampled period never establishes zero incidents or a reliable failure rate. Messages with human:false are not the user's decision. A user follow-up with status:null is a note only, not adoption."}),
    )?;
    Ok(Input {
        seq,
        reports,
        evidence,
        source_coverage,
        followed_ids,
        request,
    })
}
