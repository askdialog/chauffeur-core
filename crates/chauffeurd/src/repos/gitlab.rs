//! The projects of a GitLab the installation is set to (docs/adr/gitlab-as-the-forge.md), listed
//! through `glab api` and cloned over HTTPS through `glab`'s credential helper. A project is a
//! repo whose owner is its namespace, which may hold subgroups (`team/sub/project`).

use std::path::Path;

use anyhow::Context;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use tokio::process::Command;

use crate::shellenv::ShellEnv;
use crate::store::CachedRepo;
use crate::{Error, Result};

/// Pages read at most for one listing: 100 projects each.
const MAX_PAGES: usize = 10;

#[derive(Deserialize)]
pub(super) struct Project {
    path: String,
    path_with_namespace: String,
    default_branch: Option<String>,
    description: Option<String>,
    last_activity_at: Option<String>,
    visibility: Option<String>,
    namespace: Option<Namespace>,
}

#[derive(Deserialize)]
struct Namespace {
    kind: Option<String>,
}

impl From<Project> for CachedRepo {
    fn from(project: Project) -> Self {
        CachedRepo {
            name: project.path,
            full_name: project.path_with_namespace,
            owner_is_org: project
                .namespace
                .and_then(|namespace| namespace.kind)
                .is_some_and(|kind| kind == "group"),
            private: project.visibility.as_deref() != Some("public"),
            default_branch: project.default_branch,
            description: project.description.filter(|d| !d.is_empty()),
            pushed_at: project.last_activity_at,
        }
    }
}

/// `glab api --hostname <host> <path>`'s answer.
pub(super) async fn api(host: &str, path: &str, env: &ShellEnv) -> anyhow::Result<Vec<u8>> {
    let output = Command::new("glab")
        .args(["api", "--hostname", host, path])
        .env_clear()
        .envs(env.vars())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => anyhow::anyhow!(crate::mrs::NOT_CONNECTED),
            _ => anyhow::Error::from(error).context("running glab api"),
        })?;
    anyhow::ensure!(
        output.status.success(),
        "GitLab did not list the projects: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

/// The projects of the endpoint `projects` (the account's, or a group's), up to [`MAX_PAGES`].
async fn every_page(host: &str, projects: &str, env: &ShellEnv) -> anyhow::Result<Vec<CachedRepo>> {
    let mut all = Vec::new();
    for page in 1..=MAX_PAGES {
        let listed = page_of(host, projects, page, 100, env).await?;
        let last = listed.len() < 100;
        all.extend(listed);
        if last {
            break;
        }
    }
    Ok(all)
}

/// One page of the endpoint `projects`, newest activity first, archived ones left out.
pub(super) async fn page_of(
    host: &str,
    projects: &str,
    page: usize,
    per_page: usize,
    env: &ShellEnv,
) -> anyhow::Result<Vec<CachedRepo>> {
    let separator = if projects.contains('?') { '&' } else { '?' };
    let path = format!(
        "{projects}{separator}archived=false&order_by=last_activity_at&sort=desc&per_page={per_page}&page={page}"
    );
    let stdout = api(host, &path, env).await?;
    let projects: Vec<Project> =
        serde_json::from_slice(&stdout).context("reading GitLab's projects")?;
    Ok(projects.into_iter().map(Into::into).collect())
}

/// The endpoint of a group's projects, its subgroups' included.
pub(super) fn group_projects(group: &str) -> String {
    format!(
        "groups/{}/projects?include_subgroups=true",
        utf8_percent_encode(group, NON_ALPHANUMERIC)
    )
}

/// The endpoint of the projects the account is a member of.
pub(super) const ACCOUNT_PROJECTS: &str = "projects?membership=true";

/// The projects to pick from: the main group's (its subgroups' included) and, with `sources`,
/// every project the account is a member of. Fails only when nothing could be listed.
pub(super) async fn fetch_repos(
    host: &str,
    main: &str,
    sources: bool,
    env: &ShellEnv,
) -> anyhow::Result<Vec<CachedRepo>> {
    anyhow::ensure!(!host.is_empty(), "no GitLab host is set");
    let (main_listed, account_listed) = tokio::join!(
        async {
            if main.is_empty() {
                None
            } else {
                Some(every_page(host, &group_projects(main), env).await)
            }
        },
        async {
            if sources {
                Some(every_page(host, ACCOUNT_PROJECTS, env).await)
            } else {
                None
            }
        },
    );
    let mut by_name = std::collections::HashMap::new();
    let mut errors = Vec::new();
    for listed in [main_listed, account_listed].into_iter().flatten() {
        match listed {
            Ok(repos) => {
                for repo in repos {
                    by_name.insert(repo.full_name.to_lowercase(), repo);
                }
            }
            Err(error) => errors.push(error),
        }
    }
    if let Some(error) = errors.into_iter().next() {
        if by_name.is_empty() {
            return Err(error);
        }
        tracing::warn!(%error, "some GitLab projects could not be listed");
    }
    Ok(by_name.into_values().collect())
}

/// Clones `team/sub/project` from the GitLab over HTTPS: `glab auth login` made `glab` git's
/// credential helper for that host.
pub(super) async fn clone_repo(
    host: &str,
    full_name: &str,
    target: &Path,
    env: &ShellEnv,
) -> Result<()> {
    if host.is_empty() {
        return Err(Error::BadRequest(format!(
            "could not clone {full_name}: no GitLab host is set"
        )));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let url = format!("https://{host}/{full_name}.git");
    let output = Command::new("git")
        .args(["clone", "--", &url])
        .arg(target)
        .env_clear()
        .envs(env.vars())
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(|error| anyhow::Error::from(error).context("running git clone"))?;
    if !output.status.success() {
        return Err(Error::Unavailable(format!(
            "could not clone {full_name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_reads_as_a_repo_of_its_namespace() {
        let project: Project = serde_json::from_value(serde_json::json!({
            "path": "web",
            "path_with_namespace": "team/shop/web",
            "default_branch": "main",
            "description": "",
            "last_activity_at": "2026-11-18T10:00:00Z",
            "visibility": "internal",
            "namespace": { "kind": "group" },
        }))
        .unwrap();
        let repo = CachedRepo::from(project);
        assert_eq!((repo.name.as_str(), repo.owner()), ("web", "team/shop"));
        assert!(repo.owner_is_org && repo.private && repo.description.is_none());
        assert_eq!(repo.default_branch.as_deref(), Some("main"));
    }

    #[test]
    fn a_group_s_projects_include_its_subgroups() {
        assert_eq!(
            group_projects("team/shop"),
            "groups/team%2Fshop/projects?include_subgroups=true"
        );
    }
}
