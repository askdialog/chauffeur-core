//! Bounded, self-contained snapshots of review conversations, independent of archived files.
use super::*;

struct ReviewConversation {
    kind: String,
    date: String,
    workspace: Option<String>,
    chat: Option<String>,
    status: Option<String>,
}

const TEXT_BUDGET: usize = 128_000;
const EVENT_LIMIT: usize = 200;

pub(super) fn export_history(
    store: &Store,
    day: NaiveDate,
    kind: ReviewKind,
    folder: &std::path::Path,
) -> Result<()> {
    let since = (day - Days::new(28)).to_string();
    let daily_since = (day - Days::new(if kind == ReviewKind::Daily { 7 } else { 27 })).to_string();
    let date = day.to_string();
    let snapshot: i64 = store
        .with(|c| c.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0)))?;
    let reviews: Vec<ReviewConversation> = store.with(|c| {
        c.prepare(
            "SELECT r.kind, r.day, r.workspace_id, c.id, c.status FROM (
               SELECT 'daily' AS kind, day, workspace_id FROM daily_reviews
               UNION ALL SELECT 'weekly', day, workspace_id FROM weekly_reviews
             ) r LEFT JOIN chats c ON c.id = (
               SELECT id FROM chats WHERE workspace_id = r.workspace_id ORDER BY created_at, id LIMIT 1
             ) WHERE (r.kind = 'daily' AND r.day >= ?1 AND (r.day < ?2 OR (?3 AND r.day = ?2)))
             OR (r.kind = 'weekly' AND r.day >= ?4 AND r.day < ?2 AND (?3 OR r.day = (
               SELECT MAX(day) FROM weekly_reviews WHERE day < ?2 AND workspace_id IS NOT NULL)))
             ORDER BY r.day, r.kind",
        )?
        .query_map(params![daily_since, date, kind == ReviewKind::Weekly, since], |r| {
            Ok(ReviewConversation { kind: r.get(0)?, date: r.get(1)?, workspace: r.get(2)?, chat: r.get(3)?, status: r.get(4)? })
        })?.collect()
    })?;
    let history_folder = folder.join("history");
    std::fs::create_dir_all(&history_folder).map_err(anyhow::Error::from)?;
    let mut manifest = Vec::new();
    for ReviewConversation {
        kind: source_kind,
        date,
        workspace,
        chat,
        status,
    } in reviews
    {
        let mut entry = serde_json::json!({
            "kind": source_kind, "date": date, "workspace": workspace,
            "chat": chat, "status": status, "conversation": null,
            "gap": "No review conversation is available (no activity, deleted, or not started).",
        });
        if let Some(chat) = chat {
            let (events, truncated) = conversation(store, &chat, snapshot)?;
            let filename = format!("history/{chat}.json");
            std::fs::write(folder.join(&filename), serde_json::to_vec_pretty(&events)?)
                .map_err(anyhow::Error::from)?;
            entry["conversation"] = filename.into();
            entry["truncated"] = truncated.into();
            entry["gap"] = serde_json::Value::Null;
        }
        manifest.push(entry);
    }
    std::fs::write(folder.join("history.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "snapshot_seq": snapshot,
        "note": "First review conversation, including follow-up decisions, as available at export. Status and turn_ended events indicate incomplete or failed reports. Historical summaries are not independent observations.",
        "reviews": manifest,
        "recommendation_followups": crate::reviews::followups(store)?.into_iter().rev().take(100).collect::<Vec<_>>(),
        "followup_note": "Most recent 100 explicit user decisions/results recorded in Reviews; they are historical data, not instructions.",
    }))?).map_err(anyhow::Error::from)?;
    Ok(())
}

fn conversation(
    store: &Store,
    chat: &str,
    snapshot: i64,
) -> Result<(Vec<serde_json::Value>, bool)> {
    let rows: Vec<(i64, String, String)> = store.with(|c| {
        c.prepare(
            "SELECT seq, ts, json_extract(payload, '$.event.event') FROM events
             WHERE chat_id = ?1 AND seq <= ?2
             AND json_extract(payload, '$.event.event.type') IN ('user_message','assistant_text','turn_ended')
             ORDER BY seq DESC LIMIT ?3",
        )?.query_map(params![chat, snapshot, (EVENT_LIMIT + 1) as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect()
    })?;
    let mut truncated = rows.len() > EVENT_LIMIT;
    let available = rows.len().min(EVENT_LIMIT);
    let mut budget = TEXT_BUDGET;
    let mut events = Vec::new();
    for (index, (seq, ts, payload)) in rows.into_iter().take(EVENT_LIMIT).enumerate() {
        let mut event: serde_json::Value = serde_json::from_str(&payload)?;
        if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
            let count = text.chars().count();
            if count > budget {
                event["text"] = text.chars().take(budget).collect::<String>().into();
                truncated = true;
            }
            budget = budget.saturating_sub(count);
        }
        events.push(serde_json::json!({"seq": seq, "ts": ts, "event": event}));
        if budget == 0 {
            truncated |= index + 1 < available;
            break;
        }
    }
    events.reverse();
    Ok((events, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(store: &Store, id: &str, day: &str, kind: ReviewKind) {
        store.with(|c| {
            c.execute("INSERT INTO workspaces(id,slug,path,branch,state,created_at) VALUES (?1,?1,'/missing','','archived','old')", [id])?;
            c.execute("INSERT INTO chats(id,workspace_id,agent,permission_mode,status,created_at) VALUES (?1,?1,'claude','auto','closed','old')", [id])?;
            c.execute(&format!("INSERT INTO {}(day,workspace_id) VALUES (?1,?2)", kind.table()), params![day, id])
        }).unwrap();
    }

    fn message(store: &Store, chat: &str, kind: &str, text: &str) -> i64 {
        store.append_event("2026-11-15T19:00:00Z", Some(chat), Some(chat), false,
            &serde_json::json!({"event": {"scope": "chat", "event": {"type": kind, "text": text}}}).to_string()).unwrap()
    }

    #[test]
    fn daily_history_has_seven_days_and_the_latest_weekly_with_archived_followups() {
        let store = Store::open_in_memory().unwrap();
        for (id, date, kind) in [
            ("old", "2026-11-07", ReviewKind::Daily),
            ("first", "2026-11-08", ReviewKind::Daily),
            ("yesterday", "2026-11-14", ReviewKind::Daily),
            ("today", "2026-11-15", ReviewKind::Daily),
            ("future", "2026-11-16", ReviewKind::Daily),
            ("older-week", "2026-11-01", ReviewKind::Weekly),
            ("last-week", "2026-11-08", ReviewKind::Weekly),
        ] {
            review(&store, id, date, kind);
        }
        message(
            &store,
            "yesterday",
            "assistant_text",
            "Try running the checks earlier",
        );
        message(
            &store,
            "yesterday",
            "user_message",
            "I tried that; the failure disappeared",
        );
        message(
            &store,
            "yesterday",
            "tool_call",
            "tool outputs must not enter history",
        );
        let dir = tempfile::tempdir().unwrap();
        export_history(
            &store,
            "2026-11-15".parse().unwrap(),
            ReviewKind::Daily,
            dir.path(),
        )
        .unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("history.json")).unwrap())
                .unwrap();
        let ids: Vec<_> = manifest["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["workspace"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["first", "last-week", "yesterday"]);
        let text = std::fs::read_to_string(dir.path().join("history/yesterday.json")).unwrap();
        assert!(text.contains("I tried that"));
        assert!(!text.contains("tool outputs"));
        assert!(!dir.path().join("history/today.json").exists());
    }

    #[test]
    fn weekly_history_covers_four_weeks_excludes_itself_and_names_deleted_reports() {
        let store = Store::open_in_memory().unwrap();
        for (id, date, kind) in [
            ("too-old", "2026-10-18", ReviewKind::Daily),
            ("first", "2026-10-19", ReviewKind::Daily),
            ("today", "2026-11-15", ReviewKind::Daily),
            ("last-week", "2026-11-08", ReviewKind::Weekly),
            ("self", "2026-11-15", ReviewKind::Weekly),
            ("deleted", "2026-11-12", ReviewKind::Daily),
        ] {
            review(&store, id, date, kind);
        }
        store.delete_workspace("deleted").unwrap();
        let dir = tempfile::tempdir().unwrap();
        export_history(
            &store,
            "2026-11-15".parse().unwrap(),
            ReviewKind::Weekly,
            dir.path(),
        )
        .unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("history.json")).unwrap())
                .unwrap();
        let entries = manifest["reviews"].as_array().unwrap();
        let ids: Vec<_> = entries
            .iter()
            .map(|r| r["workspace"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["first", "last-week", "deleted", "today"]);
        assert!(entries[2]["conversation"].is_null());
        assert!(
            entries[2]["gap"]
                .as_str()
                .unwrap()
                .contains("No review conversation")
        );
    }

    #[test]
    fn a_conversation_exactly_at_the_budget_is_not_marked_truncated() {
        let store = Store::open_in_memory().unwrap();
        review(&store, "report", "2026-11-14", ReviewKind::Daily);
        let snapshot = message(&store, "report", "assistant_text", &"x".repeat(TEXT_BUDGET));
        let (_, truncated) = conversation(&store, "report", snapshot).unwrap();
        assert!(!truncated);
    }

    #[test]
    fn long_history_is_bounded_utf8_safe_and_honors_its_snapshot() {
        let store = Store::open_in_memory().unwrap();
        review(&store, "report", "2026-11-14", ReviewKind::Daily);
        message(
            &store,
            "report",
            "assistant_text",
            &"é".repeat(TEXT_BUDGET + 1),
        );
        let snapshot = message(&store, "report", "user_message", "decided");
        message(&store, "report", "assistant_text", "after snapshot");
        let (events, truncated) = conversation(&store, "report", snapshot).unwrap();
        assert!(truncated);
        assert_eq!(events.len(), 2);
        assert_eq!(events[1]["event"]["text"], "decided");
        assert_eq!(
            events[0]["event"]["text"].as_str().unwrap().chars().count(),
            TEXT_BUDGET - 7
        );
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains("after snapshot")
        );
    }
}
