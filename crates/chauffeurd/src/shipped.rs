//! Pull requests merged since a moment, for the Office's Shipped window (the user's own) and
//! its EOW board (everyone's in the GitHub organization of the settings, their open ones too):
//! GitHub searches through `gh api graphql`, each pull request given the tickets of the
//! workspace that worked on its branch, else the Linear issue its branch names. Chauffeur's own
//! repository is left out: what is merged there does not go to production.

use std::collections::HashMap;
use std::sync::Arc;

use chauffeur_protocol::client::{ShippedPr, ShippedProject};
use chauffeur_protocol::model::TicketRef;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use futures_util::future::try_join_all;
use serde_json::Value;

use crate::linear::BoardIssue;
use crate::store::WorkspaceRow;
use crate::{Daemon, Error, Result};

/// GitHub's search gives 100 a page, one after the other: the team's merged pull requests are
/// asked a day at a time, every day at once, five pages a day at most.
const PAGES: usize = 5;

/// Bots' pull requests are dropped anyway (`parse_page`): left out by the search, they take none
/// of the pages (most of the team's open ones are Dependabot's).
const NO_BOTS: &str = "-author:app/dependabot -author:app/renovate -author:app/github-actions";

/// Chauffeur's repository, by name.
const CHAUFFEUR_REPO: &str = "dialog-chauffeur";

const QUERY: &str = r#"query($q: String!, $after: String) {
  search(query: $q, type: ISSUE, first: 100, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes { ... on PullRequest {
      number title url createdAt mergedAt headRefName repository { nameWithOwner }
      author { __typename login ... on User { name } }
    } }
  }
}"#;

pub async fn shipped(daemon: &Arc<Daemon>, since: &str, team: bool) -> Result<Vec<ShippedPr>> {
    let since_at = DateTime::parse_from_rfc3339(since)
        .map_err(|e| Error::BadRequest(format!("`since` is no RFC 3339 date: {e}")))?
        .with_timezone(&Utc);
    let since = since_at.to_rfc3339_opts(SecondsFormat::Secs, true);
    let settings = daemon.settings.get();
    let org = settings.github_org.clone();
    // The board's repositories, `repo:` qualifiers GitHub joins with an OR; else the whole org.
    let scope = match settings.office.board_repos.as_slice() {
        [] => format!("org:{org}"),
        repos => repos
            .iter()
            .map(|repo| format!("repo:{org}/{repo}"))
            .collect::<Vec<_>>()
            .join(" "),
    };
    let mut searches = Vec::new();
    if team {
        if org.is_empty() {
            return Err(Error::BadRequest(
                "no GitHub organization in the settings: set one to see the team's pull requests"
                    .into(),
            ));
        }
        for range in days(since_at, Utc::now()) {
            searches.push(format!("{scope} is:pr is:merged merged:{range} {NO_BOTS}"));
        }
        searches.push(format!("{scope} is:pr is:open {NO_BOTS}"));
    } else {
        searches.push(format!("author:@me is:pr is:merged merged:>={since}"));
    }
    // Left out by the search too, so that it takes none of the pages.
    let searches = searches.into_iter().map(|search| match org.is_empty() {
        true => search,
        false => format!("{search} -repo:{org}/{CHAUFFEUR_REPO}"),
    });
    let found =
        try_join_all(searches.map(|search| async move { search_all(daemon, &search).await }))
            .await?;
    // Two days' ranges share their bound: a pull request merged right then comes twice.
    let mut seen = std::collections::HashSet::new();
    let mut prs: Vec<ShippedPr> = found
        .into_iter()
        .flatten()
        .filter(|pr| seen.insert(pr.url.clone()))
        .collect();
    prs.retain(|pr| pr.repo.rsplit('/').next() != Some(CHAUFFEUR_REPO));
    prs.sort_by(|a, b| b.merged_at.cmp(&a.merged_at));

    let workspaces = daemon.store.list_workspaces()?;
    let by_branch = tickets_by_branch(&workspaces);
    for pr in &mut prs {
        if let Some(tickets) = by_branch.get(pr.branch.as_str()) {
            pr.tickets = tickets.clone();
        }
    }
    if daemon.settings.get().linear_enabled && daemon.linear.is_configured() {
        let mut branches: Vec<String> = prs
            .iter()
            .filter(|pr| pr.tickets.is_empty())
            .map(|pr| pr.branch.clone())
            .collect();
        branches.sort();
        branches.dedup();
        match daemon.linear.issues_of_branches(&branches).await {
            Ok(issues) => {
                for pr in prs.iter_mut().filter(|pr| pr.tickets.is_empty()) {
                    if let Some(ticket) = issues.get(&pr.branch) {
                        pr.tickets = vec![ticket.clone()];
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "could not read the Linear issues of merged branches")
            }
        }
    }
    Ok(prs)
}

/// What Linear says of the pull requests' tickets, for the EOW board: their projects, each with
/// its tickets, the first seen first; and the tickets nobody is assigned to, which it leaves out.
/// Nothing without Linear, or when it does not answer: the board then shows every ticket itself.
pub async fn board(daemon: &Daemon, prs: &[ShippedPr]) -> (Vec<ShippedProject>, Vec<String>) {
    if !daemon.settings.get().linear_enabled || !daemon.linear.is_configured() {
        return (Vec::new(), Vec::new());
    }
    let mut identifiers: Vec<String> = Vec::new();
    for ticket in prs.iter().flat_map(|pr| &pr.tickets) {
        if !identifiers.contains(&ticket.identifier) {
            identifiers.push(ticket.identifier.clone());
        }
    }
    match daemon.linear.board_issues(&identifiers).await {
        Ok(of) => {
            let unassigned = identifiers
                .iter()
                .filter(|identifier| of.get(*identifier).is_some_and(|issue| !issue.assigned))
                .cloned()
                .collect();
            (grouped(&identifiers, &of), unassigned)
        }
        Err(error) => {
            tracing::warn!(%error, "could not read the Linear projects of the team's tickets");
            (Vec::new(), Vec::new())
        }
    }
}

/// The projects of `identifiers`, in the order their first ticket comes, each with its tickets.
fn grouped(identifiers: &[String], of: &HashMap<String, BoardIssue>) -> Vec<ShippedProject> {
    let mut projects: Vec<ShippedProject> = Vec::new();
    for identifier in identifiers {
        let Some(project) = of.get(identifier).and_then(|issue| issue.project.as_ref()) else {
            continue;
        };
        match projects.iter_mut().find(|p| p.id == project.id) {
            Some(known) => known.tickets.push(identifier.clone()),
            None => projects.push(ShippedProject {
                id: project.id.clone(),
                name: project.name.clone(),
                url: project.url.clone(),
                progress: project.progress,
                tickets: vec![identifier.clone()],
            }),
        }
    }
    projects
}

/// GitHub's `merged:` ranges a day long from `since` on, the last one open-ended.
fn days(since: DateTime<Utc>, now: DateTime<Utc>) -> Vec<String> {
    let at = |t: DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut ranges = Vec::new();
    let mut from = since;
    while from + Duration::days(1) < now {
        let to = from + Duration::days(1);
        ranges.push(format!("{}..{}", at(from), at(to)));
        from = to;
    }
    ranges.push(format!(">={}", at(from)));
    ranges
}

/// Every page of a search, up to [`PAGES`].
async fn search_all(daemon: &Daemon, search: &str) -> Result<Vec<ShippedPr>> {
    let mut prs = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..PAGES {
        let page = search_page(daemon, search, after.as_deref()).await?;
        let (found, next) = parse_page(&page);
        prs.extend(found);
        match next {
            Some(cursor) => after = Some(cursor),
            None => break,
        }
    }
    Ok(prs)
}

async fn search_page(daemon: &Daemon, search: &str, after: Option<&str>) -> Result<Value> {
    let mut args = vec![
        "api".to_string(),
        "graphql".into(),
        "-f".into(),
        format!("query={QUERY}"),
        "-f".into(),
        format!("q={search}"),
    ];
    if let Some(after) = after {
        args.extend(["-f".into(), format!("after={after}")]);
    }
    let output = tokio::process::Command::new("gh")
        .args(&args)
        .env_clear()
        .envs(daemon.shell_env.vars())
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| Error::Unavailable(format!("could not run gh: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Unavailable(format!("GitHub: {}", stderr.trim())));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| Error::Unavailable(format!("GitHub answered no JSON: {e}")))
}

/// A search page's pull requests by people (bots' left out), and the cursor of the next page if
/// there is one.
fn parse_page(page: &Value) -> (Vec<ShippedPr>, Option<String>) {
    let search = &page["data"]["search"];
    let prs = search["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|node| node["author"]["__typename"] == "User")
        .filter_map(|node| {
            Some(ShippedPr {
                repo: node["repository"]["nameWithOwner"].as_str()?.to_string(),
                number: node["number"].as_u64()?,
                title: node["title"].as_str()?.to_string(),
                url: node["url"].as_str()?.to_string(),
                branch: node["headRefName"].as_str()?.to_string(),
                author: node["author"]["login"].as_str()?.to_string(),
                author_name: node["author"]["name"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .map(String::from),
                created_at: node["createdAt"].as_str()?.to_string(),
                merged_at: node["mergedAt"].as_str().map(String::from),
                tickets: Vec::new(),
            })
        })
        .collect();
    let next = search["pageInfo"]["hasNextPage"]
        .as_bool()
        .unwrap_or(false)
        .then(|| search["pageInfo"]["endCursor"].as_str().map(String::from))
        .flatten();
    (prs, next)
}

/// The tickets of each workspace's branch, archived workspaces included; the newest workspace
/// wins a branch two of them shared.
fn tickets_by_branch(rows: &[WorkspaceRow]) -> HashMap<&str, Vec<TicketRef>> {
    let mut rows: Vec<&WorkspaceRow> = rows
        .iter()
        .filter(|r| !r.branch.is_empty() && !r.tickets.is_empty())
        .collect();
    rows.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    rows.into_iter()
        .map(|r| (r.branch.as_str(), r.tickets.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::model::WorkspaceState;
    use serde_json::json;

    #[test]
    fn the_merged_pull_requests_are_asked_a_day_at_a_time_the_last_day_open_ended() {
        let at = |t: &str| DateTime::parse_from_rfc3339(t).unwrap().with_timezone(&Utc);
        assert_eq!(
            days(at("2026-10-16T00:00:00Z"), at("2026-10-18T12:00:00Z")),
            [
                "2026-10-16T00:00:00Z..2026-10-17T00:00:00Z",
                "2026-10-17T00:00:00Z..2026-10-18T00:00:00Z",
                ">=2026-10-18T00:00:00Z",
            ]
        );
        assert_eq!(
            days(at("2026-10-16T08:00:00Z"), at("2026-10-16T09:00:00Z")),
            [">=2026-10-16T08:00:00Z"]
        );
    }

    #[test]
    fn tickets_of_one_project_make_one_project_and_those_without_none() {
        let project = |id: &str| BoardIssue {
            project: Some(crate::linear::LinearProject {
                id: id.into(),
                name: format!("Project {id}"),
                url: format!("https://linear.app/x/project/{id}"),
                progress: 0.5,
            }),
            assigned: true,
        };
        let of = HashMap::from([
            ("DAT-1".to_string(), project("b")),
            ("DAT-2".to_string(), project("a")),
            ("DAT-3".to_string(), project("b")),
        ]);
        let ids = ["DAT-1", "DAT-9", "DAT-2", "DAT-3"].map(String::from);
        let projects = grouped(&ids, &of);
        assert_eq!(
            projects
                .iter()
                .map(|p| (p.id.as_str(), p.tickets.join(",")))
                .collect::<Vec<_>>(),
            vec![("b", "DAT-1,DAT-3".to_string()), ("a", "DAT-2".to_string())]
        );
    }

    fn node(number: u64, branch: &str, merged_at: Value, author: Value) -> Value {
        json!({
            "number": number,
            "title": format!("PR {number}"),
            "url": format!("https://github.com/o/r/pull/{number}"),
            "createdAt": "2026-10-26T09:00:00Z",
            "mergedAt": merged_at,
            "headRefName": branch,
            "repository": { "nameWithOwner": "o/r" },
            "author": author,
        })
    }

    #[test]
    fn a_page_gives_people_s_pull_requests_and_the_next_page_s_cursor() {
        let ben = json!({ "__typename": "User", "login": "ben", "name": "Benjamin C" });
        let bot = json!({ "__typename": "Bot", "login": "dependabot" });
        let page = json!({ "data": { "search": {
            "pageInfo": { "hasNextPage": true, "endCursor": "c2" },
            "nodes": [
                node(1, "a", json!("2026-10-27T10:00:00Z"), ben.clone()),
                node(2, "b", Value::Null, ben),
                node(3, "c", json!("2026-10-27T11:00:00Z"), bot),
                // An issue matched by the search has none of a pull request's fields.
                {},
            ],
        }}});
        let (prs, next) = parse_page(&page);
        assert_eq!(prs.iter().map(|pr| pr.number).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(prs[0].repo, "o/r");
        assert_eq!(prs[0].author_name.as_deref(), Some("Benjamin C"));
        assert_eq!(prs[1].merged_at, None, "an open one");
        assert_eq!(next.as_deref(), Some("c2"));

        let last = json!({ "data": { "search": {
            "pageInfo": { "hasNextPage": false, "endCursor": "c3" },
            "nodes": [],
        }}});
        assert_eq!(parse_page(&last).1, None);
    }

    fn workspace(branch: &str, ticket: &str, created_at: &str) -> WorkspaceRow {
        WorkspaceRow {
            id: ticket.into(),
            slug: ticket.into(),
            path: String::new(),
            branch: branch.into(),
            parent_id: None,
            state: WorkspaceState::Archived,
            tickets: vec![TicketRef {
                identifier: ticket.into(),
                title: None,
                url: Some(format!("https://linear.app/x/issue/{ticket}")),
            }],
            created_at: created_at.into(),
            archived_at: None,
            last_error: None,
            attention_at: None,
            seen_at: None,
            explore: false,
            title: None,
            auto_title: false,
            title_source: None,
            pinned: false,
            position: 0.0,
            section_id: None,
            house_position: None,
        }
    }

    #[test]
    fn a_branch_takes_the_tickets_of_its_newest_workspace() {
        let rows = [
            workspace("b/one", "DAT-2", "2026-10-02T00:00:00Z"),
            workspace("b/one", "DAT-1", "2026-10-01T00:00:00Z"),
            workspace("", "DAT-3", "2026-10-03T00:00:00Z"),
        ];
        let tickets = tickets_by_branch(&rows);
        assert_eq!(tickets["b/one"][0].identifier, "DAT-2");
        assert!(!tickets.contains_key(""), "an exploration has no branch");
    }
}
