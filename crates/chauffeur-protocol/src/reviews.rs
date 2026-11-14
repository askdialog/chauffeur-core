//! Review insights and explicit, dated user follow-up; source conversations remain authoritative.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewCadence {
    Daily,
    Weekly,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewReportStatus {
    Complete,
    Incomplete,
    Failed,
    Unavailable,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewTrend {
    Recurring,
    Improving,
    Unclear,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewRecommendationStatus {
    Proposed,
    Decided,
    Tried,
    ObservedEffect,
    EffectUnknown,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ReviewReport {
    pub id: String,
    pub kind: ReviewCadence,
    pub date: String,
    pub workspace_id: Option<String>,
    pub chat_id: Option<String>,
    pub title: String,
    pub status: ReviewReportStatus,
    pub archived: bool,
    pub shortened: bool,
    pub summary: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ReviewEvidence {
    pub id: String,
    pub workspace_id: String,
    pub chat_id: String,
    pub workspace: String,
    pub branch: String,
    pub at: String,
    pub quote: String,
    #[ts(type = "number")]
    pub source_seq: i64,
    pub from_review: bool,
    pub role: String,
    #[serde(default)]
    pub human: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ReviewPattern {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub trend: ReviewTrend,
    pub confidence: String,
    pub observations: Vec<ReviewEvidence>,
    pub counterexamples: Vec<ReviewEvidence>,
    pub gaps: Vec<String>,
    pub report_ids: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ReviewExperiment {
    pub id: String,
    pub title: String,
    pub origin_report_id: String,
    pub origin_date: String,
    pub pattern_id: Option<String>,
    pub experiment: String,
    pub expected_benefit: String,
    pub success_measure: String,
    pub effort: Option<String>,
    pub next_check: Option<String>,
    pub status: ReviewRecommendationStatus,
    pub evidence: Vec<ReviewEvidence>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ReviewFollowUp {
    pub id: String,
    pub experiment_id: String,
    pub status: Option<ReviewRecommendationStatus>,
    pub note: String,
    pub source: Option<ReviewEvidence>,
    pub recorded_at: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ReviewSourceCoverage {
    pub date: String,
    pub available: u32,
    pub included: u32,
    pub complete: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ReviewProjection {
    #[serde(default)]
    pub source_coverage: Vec<ReviewSourceCoverage>,
    pub generated_at: String,
    #[ts(type = "number")]
    pub source_seq: i64,
    pub overview: String,
    pub patterns: Vec<ReviewPattern>,
    pub experiments: Vec<ReviewExperiment>,
    pub gaps: Vec<String>,
    pub report_ids: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ReviewsInfo {
    pub auto_refresh_enabled: bool,
    #[ts(type = "number")]
    pub source_seq: i64,
    pub projection: Option<ReviewProjection>,
    pub reports: Vec<ReviewReport>,
    pub followups: Vec<ReviewFollowUp>,
    pub running: bool,
    pub stale: bool,
    pub last_error: Option<String>,
    pub coverage_start: String,
    pub coverage_end: String,
    pub next_weekly: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ReviewFollowUpParams {
    pub experiment_id: String,
    pub status: Option<ReviewRecommendationStatus>,
    pub note: String,
    #[ts(type = "number | null")]
    pub source_seq: Option<i64>,
    /// Retrying the same user action saves it once.
    pub idempotency_key: String,
}
