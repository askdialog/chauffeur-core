//! Merge requests, for an installation set to a GitLab (docs/adr/gitlab-as-the-forge.md): read and
//! merged through `glab` in the repo's clone, which knows its GitLab from the clone's remote. What
//! GitLab answers is put in the shape `prs` reads of GitHub's pull requests (`gh pr list --json`),
//! so the header, stacked requests, merging and archiving stay one code.

use std::path::Path;

use chauffeur_protocol::client::MergeMethod;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use crate::shellenv::ShellEnv;

/// What a feature that needs `glab` says when there is none.
pub const NOT_CONNECTED: &str = "GitLab is not connected: connect it in Settings › GitHub / GitLab, or with `chauffeur gitlab login`";

/// `glab api <path>` in the clone, `:id` standing for its project; `None` when glab fails or does
/// not answer JSON.
async fn api(env: &ShellEnv, clone: &Path, path: &str) -> Option<Value> {
    let output = tokio::process::Command::new("glab")
        .args(["api", path])
        .current_dir(clone)
        .env_clear()
        .envs(env.vars())
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .ok()
        .filter(|output| output.status.success())?;
    serde_json::from_slice(&output.stdout).ok()
}

/// The merge requests from the branch `head` in `state` (`all`, `merged`, `open`), newest first,
/// as `gh pr list --json` would give them; `None` when glab fails.
pub async fn list(env: &ShellEnv, clone: &Path, head: &str, state: &str) -> Option<Vec<Value>> {
    let state = match state {
        "open" => "opened",
        "merged" => "merged",
        "closed" => "closed",
        _ => "all",
    };
    let path = format!(
        "projects/:id/merge_requests?source_branch={}&state={state}&per_page=10&order_by=created_at&sort=desc",
        utf8_percent_encode(head, NON_ALPHANUMERIC)
    );
    match api(env, clone, &path).await? {
        Value::Array(mrs) => Some(mrs.iter().map(as_pull_request).collect()),
        _ => None,
    }
}

/// The checks of a merge request, its head pipeline's state as GitHub's rollup reads it; none
/// without a pipeline, or when glab fails.
pub async fn checks(env: &ShellEnv, clone: &Path, number: u64) -> Value {
    let pipeline = api(env, clone, &format!("projects/:id/merge_requests/{number}"))
        .await
        .map(|mr| mr["head_pipeline"]["status"].clone())
        .unwrap_or(Value::Null);
    pipeline_rollup(pipeline.as_str())
}

fn pipeline_rollup(status: Option<&str>) -> Value {
    let Some(status) = status else {
        return json!([]);
    };
    let state = match status {
        "success" | "skipped" | "manual" => "SUCCESS",
        "failed" => "FAILURE",
        "canceled" | "canceling" => "CANCELLED",
        _ => "PENDING",
    };
    json!([{ "state": state }])
}

/// A GitLab merge request as `gh pr list --json` gives a pull request: the fields `prs` reads.
fn as_pull_request(mr: &Value) -> Value {
    let status = mr["detailed_merge_status"].as_str().unwrap_or("");
    let conflicts = mr["has_conflicts"].as_bool() == Some(true);
    json!({
        "number": mr["iid"],
        "baseRefName": mr["target_branch"],
        "url": mr["web_url"],
        "state": match mr["state"].as_str() {
            Some("opened") => "OPEN",
            Some("merged") => "MERGED",
            _ => "CLOSED",
        },
        "title": mr["title"],
        "isDraft": mr["draft"].as_bool().unwrap_or(false),
        "headRefName": mr["source_branch"],
        "headRefOid": mr["sha"],
        "isCrossRepository": mr["source_project_id"] != mr["target_project_id"],
        "reviewDecision": match status {
            "not_approved" => Value::from("REVIEW_REQUIRED"),
            "requested_changes" => Value::from("CHANGES_REQUESTED"),
            _ => Value::Null,
        },
        "mergeable": if conflicts || matches!(status, "conflict" | "need_rebase") {
            "CONFLICTING"
        } else if matches!(status, "checking" | "unchecked" | "preparing" | "approvals_syncing" | "") {
            "UNKNOWN"
        } else {
            "MERGEABLE"
        },
        "mergeBlocker": blocker(status),
        "statusCheckRollup": [],
    })
}

/// What GitLab's merge status says beyond checks, review, conflicts and draft, in a few words.
fn blocker(status: &str) -> Option<&'static str> {
    Some(match status {
        "discussions_not_resolved" => "threads to resolve",
        "blocked_status" => "blocked by another merge request",
        "policies_denied" | "security_policy_violations" => "blocked by a merge request policy",
        "jira_association_missing" => "needs a Jira issue in its title or description",
        "status_checks_must_pass" | "external_status_checks" => "external status checks to pass",
        "merge_time" => "scheduled to merge later",
        "merge_request_blocked" | "locked_paths" | "locked_lfs_files" => "blocked on GitLab",
        _ => return None,
    })
}

/// What a project allows, as `gh repo view` tells it of a repo: its merge methods (squash unless
/// never allowed; a merge commit with the merge method `merge`; a rebase with `rebase_merge` or
/// `ff`; neither when squashing is required), and whether the user may merge (Developer or more).
pub async fn merge_rights(
    env: &ShellEnv,
    clone: &Path,
) -> Option<(Vec<MergeMethod>, Option<bool>)> {
    let project = api(env, clone, "projects/:id").await?;
    Some(parse_merge_rights(&project))
}

fn parse_merge_rights(project: &Value) -> (Vec<MergeMethod>, Option<bool>) {
    let squash = project["squash_option"].as_str().unwrap_or("default_off");
    let method = project["merge_method"].as_str().unwrap_or("merge");
    let mut methods = Vec::new();
    if squash != "never" {
        methods.push(MergeMethod::Squash);
    }
    if squash != "always" {
        methods.push(if method == "merge" {
            MergeMethod::Merge
        } else {
            MergeMethod::Rebase
        });
    }
    let level = ["project_access", "group_access"]
        .iter()
        .filter_map(|key| project["permissions"][key]["access_level"].as_u64())
        .max();
    (methods, level.map(|level| level >= 30))
}

/// `glab mr merge` now, never on its own later: glab sets auto-merge unless told not to.
pub fn merge_args(number: &str, method: MergeMethod) -> Vec<String> {
    let mut args: Vec<String> = ["mr", "merge", number, "--yes", "--auto-merge=false"]
        .map(String::from)
        .into();
    match method {
        MergeMethod::Squash => args.push("--squash".into()),
        MergeMethod::Rebase => args.push("--rebase".into()),
        MergeMethod::Merge => {}
    }
    args
}

/// Points a merge request at another target branch.
pub fn retarget_args(number: &str, onto: &str) -> Vec<String> {
    ["mr", "update", number, "--target-branch", onto]
        .map(String::from)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mr(status: &str) -> Value {
        json!({
            "iid": 42,
            "target_branch": "main",
            "web_url": "https://gitlab.example.com/group/sub/web/-/merge_requests/42",
            "state": "opened",
            "title": "Fix the price filter",
            "draft": false,
            "source_branch": "jc/price-filter",
            "sha": "9bbee8096dbd863e90f9b53a4de4bd16755fd577",
            "source_project_id": 7,
            "target_project_id": 7,
            "detailed_merge_status": status,
            "has_conflicts": false,
        })
    }

    #[test]
    fn a_merge_request_reads_as_a_pull_request() {
        let pr = as_pull_request(&mr("mergeable"));
        assert_eq!(pr["number"], 42);
        assert_eq!(pr["baseRefName"], "main");
        assert_eq!(pr["state"], "OPEN");
        assert_eq!(pr["headRefName"], "jc/price-filter");
        assert_eq!(pr["isCrossRepository"], false);
        assert_eq!(pr["mergeable"], "MERGEABLE");
        assert!(pr["reviewDecision"].is_null() && pr["mergeBlocker"].is_null());
        let mut fork = mr("mergeable");
        fork["source_project_id"] = json!(8);
        fork["state"] = json!("merged");
        let fork = as_pull_request(&fork);
        assert_eq!(
            (fork["isCrossRepository"].as_bool(), fork["state"].as_str()),
            (Some(true), Some("MERGED"))
        );
    }

    #[test]
    fn gitlab_s_merge_status_reads_as_review_conflicts_or_a_blocker() {
        let read = |status: &str| {
            let pr = as_pull_request(&mr(status));
            (
                pr["reviewDecision"].as_str().map(String::from),
                pr["mergeable"].as_str().unwrap().to_string(),
                pr["mergeBlocker"].as_str().map(String::from),
            )
        };
        assert_eq!(read("not_approved").0.as_deref(), Some("REVIEW_REQUIRED"));
        assert_eq!(
            read("requested_changes").0.as_deref(),
            Some("CHANGES_REQUESTED")
        );
        assert_eq!(read("conflict").1, "CONFLICTING");
        assert_eq!(read("need_rebase").1, "CONFLICTING");
        assert_eq!(read("checking").1, "UNKNOWN");
        assert_eq!(
            read("discussions_not_resolved").2.as_deref(),
            Some("threads to resolve")
        );
        assert_eq!(read("ci_still_running"), (None, "MERGEABLE".into(), None));
    }

    #[test]
    fn a_pipeline_reads_as_github_s_checks() {
        assert_eq!(pipeline_rollup(None), json!([]));
        assert_eq!(
            pipeline_rollup(Some("success")),
            json!([{ "state": "SUCCESS" }])
        );
        assert_eq!(
            pipeline_rollup(Some("failed")),
            json!([{ "state": "FAILURE" }])
        );
        assert_eq!(
            pipeline_rollup(Some("running")),
            json!([{ "state": "PENDING" }])
        );
    }

    #[test]
    fn a_project_s_settings_say_how_it_merges_and_who_may() {
        let rights = |squash: &str, method: &str, level: u64| {
            parse_merge_rights(&json!({
                "squash_option": squash,
                "merge_method": method,
                "permissions": { "project_access": null, "group_access": { "access_level": level } },
            }))
        };
        assert_eq!(
            rights("default_off", "merge", 40),
            (vec![MergeMethod::Squash, MergeMethod::Merge], Some(true))
        );
        assert_eq!(rights("never", "ff", 30).0, vec![MergeMethod::Rebase]);
        assert_eq!(
            rights("always", "merge", 20),
            (vec![MergeMethod::Squash], Some(false))
        );
        assert_eq!(parse_merge_rights(&json!({})).1, None);
    }

    #[test]
    fn merging_never_leaves_it_to_auto_merge() {
        assert_eq!(
            merge_args("42", MergeMethod::Squash),
            [
                "mr",
                "merge",
                "42",
                "--yes",
                "--auto-merge=false",
                "--squash"
            ]
        );
        assert_eq!(
            merge_args("42", MergeMethod::Merge)
                .last()
                .map(String::as_str),
            Some("--auto-merge=false")
        );
        assert_eq!(
            retarget_args("42", "main"),
            ["mr", "update", "42", "--target-branch", "main"]
        );
    }
}
