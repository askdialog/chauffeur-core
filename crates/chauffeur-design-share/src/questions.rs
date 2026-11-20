use std::{collections::HashSet, sync::Arc};

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use chauffeur_protocol::design_share::{DesignQuestion, DesignQuestionResult, DesignVoteParams};

use crate::{Reject, Service, store::questions::QuestionChange};

fn response(results: Vec<DesignQuestionResult>) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(results),
    )
}

pub(super) async fn list(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    Path(share): Path<String>,
) -> Result<impl IntoResponse, Reject> {
    let who = service.viewer_identity(&headers).await?;
    Ok(response(
        service.store.design_questions(&share, &who, None)?,
    ))
}

pub(super) async fn publish(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    Path(share): Path<String>,
    Json(mut question): Json<DesignQuestion>,
) -> Result<impl IntoResponse, Reject> {
    let who = service.viewer_identity(&headers).await?;
    question.text = question.text.trim().to_owned();
    if question.id.is_empty()
        || question.id.len() > 64
        || !question
            .id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        || question.text.is_empty()
        || question.text.chars().count() > 240
        || !(2..=20).contains(&question.options.len())
    {
        return Err(Reject(
            StatusCode::BAD_REQUEST,
            "Enter a question up to 240 characters with 2 to 20 variants".into(),
        ));
    }
    let mut unique = HashSet::new();
    for option in &mut question.options {
        option.label = option.label.trim().to_owned();
        if option.label.is_empty()
            || option.label.chars().count() > 240
            || !unique.insert(&option.file)
        {
            return Err(Reject(
                StatusCode::BAD_REQUEST,
                "Each variant needs a distinct screen and a label up to 240 characters".into(),
            ));
        }
    }
    let validate = |question: &DesignQuestion| {
        let canvas = service.files.canvas(&share, "/");
        let available: HashSet<_> = canvas
            .pages
            .iter()
            .flat_map(|page| page.sections.iter().flat_map(|section| &section.screens))
            .chain(&canvas.unsorted)
            .filter(|screen| screen.url.is_some())
            .map(|screen| screen.file.as_str())
            .collect();
        if question
            .options
            .iter()
            .any(|option| !available.contains(option.file.as_str()))
        {
            return Err(Reject(StatusCode::CONFLICT, "Some variants are missing from the shared canvas. Wait for the design to sync or choose available screens".into()));
        }
        Ok(())
    };
    Ok(response(service.store.design_questions(
        &share,
        &who,
        Some(QuestionChange::Publish(&question, &validate)),
    )?))
}

pub(super) async fn vote(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    Path((share, question)): Path<(String, String)>,
    Json(mut answer): Json<DesignVoteParams>,
) -> Result<impl IntoResponse, Reject> {
    let who = service.viewer_identity(&headers).await?;
    answer.note = answer.note.trim().to_owned();
    if answer.note.chars().count() > 1000 {
        return Err(Reject(
            StatusCode::BAD_REQUEST,
            "Keep the justification to 1,000 characters".into(),
        ));
    }
    Ok(response(service.store.design_questions(
        &share,
        &who,
        Some(QuestionChange::Vote(&question, &answer)),
    )?))
}
