//! Derived review insights are replaceable; explicit user decisions are durable and independent.
use crate::{
    Daemon, Error, Result,
    store::{Store, now},
};
use chauffeur_protocol::reviews::*;
use chauffeur_protocol::sidecar::{AgentAskParams, AgentAskResult, SidecarCall};
use chrono::{Datelike, Days, Local, NaiveDate, TimeZone};
use rusqlite::OptionalExtension;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

mod analysis;
mod data;
#[cfg(test)]
mod tests;

#[derive(Default)]
pub(crate) struct ReviewInsights {
    running: Mutex<bool>,
}

pub fn info(d: &Daemon) -> Result<ReviewsInfo> {
    let today = Local::now().date_naive();
    let projection = data::projection(&d.store)?;
    let seq = data::source_seq(&d.store)?;
    let reports = data::reports(&d.store)?;
    let eligible = data::has_completed_text(&d.store)?;
    let revision = format!("{seq}:{today}");
    let last_error = if d.store.kv_get("reviews.error_seq")?.as_deref() == Some(revision.as_str()) {
        d.store.kv_get("reviews.error")?
    } else {
        None
    };
    let mut sunday = today + Days::new(u64::from(6 - today.weekday().num_days_from_monday()));
    if sunday == today && chrono::Timelike::hour(&Local::now()) >= 18 {
        sunday = sunday + Days::new(7);
    }
    let next = sunday
        .and_hms_opt(18, 0, 0)
        .and_then(|t| Local.from_local_datetime(&t).earliest());
    Ok(ReviewsInfo {
        auto_refresh_enabled: d.settings.get().daily_review.enabled && eligible,
        source_seq: seq,
        stale: projection.as_ref().map_or(eligible, |p| {
            p.source_seq != seq
                || p.source_coverage.len() != 28
                || p.source_coverage
                    .last()
                    .is_none_or(|c| c.date != today.to_string())
        }),
        projection,
        reports,
        followups: data::followups(&d.store)?,
        running: *d.review_insights.running.lock().unwrap(),
        last_error,
        coverage_start: (today - Days::new(27)).to_string(),
        coverage_end: today.to_string(),
        next_weekly: if d.settings.get().daily_review.enabled {
            next.map(|t| t.to_rfc3339())
        } else {
            None
        },
    })
}

pub fn refresh(d: &Arc<Daemon>) -> Result<ReviewsInfo> {
    if d.paths.home.join("worker").exists() {
        return Err(Error::Unavailable(
            "Review insights run on your desktop; open Reviews there".into(),
        ));
    }
    if !data::has_completed_text(&d.store)? {
        return Err(Error::Unavailable(
            "No review text has finished in the last 28 days; wait for a review to finish".into(),
        ));
    }
    let settings = d.settings.get();
    let default = if let Some(agent) = settings.daily_review.agent {
        crate::chats::agent_choice(d, agent)?
    } else {
        crate::chats::default_choice(d)?
    };
    let agent = default.agent;
    let model = settings
        .daily_review
        .model
        .clone()
        .or_else(|| (!default.model.is_empty()).then_some(default.model));
    let initial_seq = data::source_seq(&d.store)?;
    let mut running = d.review_insights.running.lock().unwrap();
    if *running {
        drop(running);
        return info(d);
    }
    d.store.kv_delete("reviews.error")?;
    *running = true;
    drop(running);
    let task = d.clone();
    tokio::spawn(async move {
        let d = task;
        let mut revision = format!("{initial_seq}:{}", Local::now().date_naive());
        let result=async {
            let read=d.clone();
            let mut input=tokio::task::spawn_blocking(move ||data::input(&read)).await.map_err(|e|Error::Internal(e.into()))??;
            revision=format!("{}:{}",input.seq,input.source_coverage.last().map(|c|c.date.as_str()).unwrap_or(""));
            let request=std::mem::take(&mut input.request);
            let answer=tokio::select! {
                _=d.shutdown.cancelled()=>return Err(Error::Unavailable("Review insights were interrupted; refresh to retry".into())),
                result=tokio::time::timeout(Duration::from_secs(630),d.chats.sidecar.call(SidecarCall::AgentAsk(AgentAskParams{
                    agent,model,effort:default.effort,instructions:analysis::INSTRUCTIONS.into(),request,timeout_ms:600_000,
                })))=>result.map_err(|_|Error::Unavailable("Review insights timed out; refresh to retry".into()))??,
            };
            let answer:AgentAskResult=serde_json::from_value(answer)?;
            let projection=analysis::parse(&answer.text,&input,data::projection(&d.store)?.as_ref())?;
            d.store.kv_set("reviews.projection",&serde_json::to_string(&projection)?)?;
            Ok::<_,Error>(())
        }.await;
        if let Err(error) = result {
            let _ = d.store.kv_set("reviews.error", &error.to_string());
            let _ = d.store.kv_set("reviews.error_seq", &revision);
            tracing::warn!(%error,"could not refresh review insights");
        }
        *d.review_insights.running.lock().unwrap() = false;
    });
    info(d)
}

pub fn follow_up(d: &Daemon, params: ReviewFollowUpParams) -> Result<ReviewsInfo> {
    let note = params.note.trim();
    if note.chars().count() < 10 || note.chars().count() > 6000 {
        return Err(Error::BadRequest(
            "Describe the decision or result in 10–6000 characters".into(),
        ));
    }
    if uuid::Uuid::parse_str(&params.idempotency_key).is_err() {
        return Err(Error::BadRequest(
            "The follow-up request ID is invalid; reload Reviews and try again".into(),
        ));
    }
    // A committed retry still succeeds after its original evidence or projection was deleted.
    let existing: Option<String> = d.store.with(|c| {
        c.query_row(
            "SELECT json FROM review_followups WHERE id=?1",
            [&params.idempotency_key],
            |r| r.get(0),
        )
        .optional()
    })?;
    if let Some(existing) = existing {
        let old: ReviewFollowUp = serde_json::from_str(&existing)?;
        if old.experiment_id == params.experiment_id
            && old.status == params.status
            && old.note == note
            && old.source.as_ref().map(|s| s.source_seq) == params.source_seq
        {
            return info(d);
        }
        return Err(Error::Conflict(
            "This follow-up request was already used for another update; submit a new update"
                .into(),
        ));
    }
    let projection = data::projection(&d.store)?.ok_or_else(|| {
        Error::NotFound("Review insights are not available; refresh Reviews first".into())
    })?;
    if !projection
        .experiments
        .iter()
        .any(|e| e.id == params.experiment_id)
    {
        return Err(Error::NotFound(
            "This recommendation is no longer available; refresh Reviews first".into(),
        ));
    }
    let source = params
        .source_seq
        .map(|seq| data::evidence(&d.store, seq))
        .transpose()?
        .flatten();
    if params.source_seq.is_some() && source.as_ref().is_none_or(|s| s.quote.is_empty()) {
        return Err(Error::BadRequest(
            "The evidence source is unavailable; choose another source".into(),
        ));
    }
    if params.status == Some(ReviewRecommendationStatus::ObservedEffect)
        && (source.is_none()
            || note.chars().count() < 20
            || !note.chars().any(|c| c.is_ascii_digit()))
    {
        return Err(Error::BadRequest("An observed effect needs a dated evidence source and a note describing its measurement".into()));
    }
    let followup = ReviewFollowUp {
        id: params.idempotency_key,
        experiment_id: params.experiment_id,
        status: params.status,
        note: note.into(),
        source,
        recorded_at: now(),
    };
    let json = serde_json::to_string(&followup)?;
    let conflict=d.store.with(|c| {
        let tx=c.unchecked_transaction()?;
        let mut conflict=false;
        let existing:Option<String>=tx.query_row("SELECT json FROM review_followups WHERE id=?1",[&followup.id],|r|r.get(0)).optional()?;
        if let Some(existing)=existing {
            let old:ReviewFollowUp=serde_json::from_str(&existing).map_err(|e|rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            if old.experiment_id!=followup.experiment_id || old.note!=followup.note || old.status!=followup.status || old.source.as_ref().map(|s|s.source_seq)!=followup.source.as_ref().map(|s|s.source_seq) {
                conflict=true;
            }
        } else {
            tx.execute("INSERT INTO review_followups(id,experiment_id,json,recorded_at) VALUES(?1,?2,?3,?4)",rusqlite::params![followup.id,followup.experiment_id,json,followup.recorded_at])?;
        }
        tx.commit()?;
        Ok(conflict)
    })?;
    if conflict {
        return Err(Error::Conflict(
            "This follow-up request was already used for another update; submit a new update"
                .into(),
        ));
    }
    info(d)
}

pub(crate) fn followups(store: &Store) -> Result<Vec<ReviewFollowUp>> {
    data::followups(store)
}
