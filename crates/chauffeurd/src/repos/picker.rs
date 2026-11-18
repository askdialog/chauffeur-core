use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chauffeur_protocol::client::{Forge, ReposPageParams, ReposPageResult};
use serde::{Deserialize, Serialize};

use super::{RepoListing, RepoRegistry, gh_output, main_owner, validate_owner};
use crate::settings::UserSettings;
use crate::shellenv::ShellEnv;
use crate::store::CachedRepo;
use crate::{Error, Result};

const PAGE_SIZE: usize = 50;
const PAGE_TTL: Duration = Duration::from_secs(300);
const PAGE_TIMEOUT: Duration = Duration::from_secs(15);

const BROWSE_QUERY: &str = r#"
query($owner: String!, $mainCursor: String, $accountCursor: String, $mainEnabled: Boolean!, $accountEnabled: Boolean!) {
  repositoryOwner(login: $owner) @include(if: $mainEnabled) {
    repositories(first: 50, after: $mainCursor, isArchived: false, ownerAffiliations: [OWNER], orderBy: {field: PUSHED_AT, direction: DESC}) {
      ...repos
    }
  }
  viewer @include(if: $accountEnabled) {
    repositories(first: 50, after: $accountCursor, isArchived: false, affiliations: [OWNER, COLLABORATOR, ORGANIZATION_MEMBER], ownerAffiliations: [OWNER, COLLABORATOR, ORGANIZATION_MEMBER], orderBy: {field: PUSHED_AT, direction: DESC}) {
      ...repos
    }
  }
}
fragment repos on RepositoryConnection {
  nodes {
    name
    full_name: nameWithOwner
    private: isPrivate
    owner { type: __typename }
    default_branch: defaultBranchRef { name }
    description
    pushed_at: pushedAt
  }
  pageInfo { hasNextPage endCursor }
}
"#;

#[derive(Default)]
pub(super) struct Picker {
    state: Mutex<PickerState>,
}

#[derive(Default)]
struct PickerState {
    generation: u64,
    pages: HashMap<String, Arc<tokio::sync::Mutex<Option<CachedPage>>>>,
    known: HashMap<String, HashMap<String, CachedRepo>>,
}

#[derive(Clone)]
struct CachedPage {
    at: Instant,
    refreshed_at: String,
    page: Page,
}

#[derive(Clone, Default)]
struct Page {
    repos: Vec<CachedRepo>,
    next_cursor: Option<String>,
    truncated: bool,
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    scope: String,
    owner: Option<String>,
    query: String,
    main: Option<String>,
    account: Option<String>,
    main_done: bool,
    account_done: bool,
    search_page: usize,
}

impl Picker {
    pub(super) fn clear(&self) {
        let mut state = self.state.lock().unwrap();
        state.generation += 1;
        state.pages.clear();
        state.known.clear();
    }

    pub(super) fn known(&self, settings: &UserSettings) -> HashMap<String, CachedRepo> {
        self.state
            .lock()
            .unwrap()
            .known
            .get(&scope(settings))
            .cloned()
            .unwrap_or_default()
    }
}

impl RepoRegistry {
    pub fn clear_picker(&self) {
        self.picker.clear();
    }

    pub fn picker_listing(&self) -> Result<RepoListing> {
        let settings = self.settings.get();
        let mut used: Vec<_> = self.store.last_use_by_clone()?.into_iter().collect();
        used.sort_by(|a, b| b.1.cmp(&a.1));
        let mut local_names: HashSet<_> = used
            .iter()
            .take(4)
            .filter_map(|(path, _)| std::path::Path::new(path).file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        for root in &settings.clone_roots {
            if let Ok(entries) = std::fs::read_dir(crate::settings::expand_home(root)) {
                local_names.extend(
                    entries
                        .flatten()
                        .filter(|entry| entry.path().join(".git").is_dir())
                        .map(|entry| entry.file_name().to_string_lossy().into_owned()),
                );
            }
        }
        let mut cached = self.cached(&settings)?;
        cached.sort_by(|a, b| {
            b.pushed_at
                .cmp(&a.pushed_at)
                .then(a.full_name.cmp(&b.full_name))
        });
        let mut remaining = PAGE_SIZE;
        cached.retain(|repo| {
            if local_names.contains(&repo.name)
                || [&repo.name, &repo.full_name].into_iter().any(|name| {
                    settings.default_repos.contains(name) || settings.repo_order.contains(name)
                })
            {
                true
            } else if remaining > 0 {
                remaining -= 1;
                true
            } else {
                false
            }
        });
        let mut listing = self.listing_from(cached, &settings)?;
        let mut recent: Vec<_> = listing
            .repos
            .iter()
            .filter(|repo| repo.last_used.is_some())
            .collect();
        recent.sort_by(|a, b| b.last_used.cmp(&a.last_used));
        let recent: Vec<_> = recent.iter().take(4).map(|repo| repo.key.clone()).collect();
        let mut remaining = PAGE_SIZE;
        listing.repos.retain(|repo| {
            if repo.is_default || repo.local || recent.contains(&repo.key) {
                true
            } else if remaining > 0 {
                remaining -= 1;
                true
            } else {
                false
            }
        });
        Ok(listing)
    }

    pub async fn page(&self, params: ReposPageParams, env: &ShellEnv) -> Result<ReposPageResult> {
        let settings = self.settings.get();
        let params = ReposPageParams {
            owner: params
                .owner
                .filter(|owner| !owner.is_empty())
                .map(|owner| owner.to_lowercase()),
            query: params.query.trim().to_string(),
            ..params
        };
        if let Some(owner) = &params.owner {
            validate_owner(settings.forge, owner)?;
            if !settings.github_sources && !super::is_main(&settings, owner) {
                return Err(Error::BadRequest(match settings.forge {
                    Forge::Github => {
                        "only the main GitHub owner is enabled: change Sources in Settings › GitHub"
                            .into()
                    }
                    Forge::Gitlab => "only the main GitLab group is enabled: change Sources in Settings › Workspaces".into(),
                }));
            }
        }
        if params.query.len() > 256 {
            return Err(Error::BadRequest("use a shorter repo search".into()));
        }
        if !params.query.is_empty() && params.owner.is_none() {
            return Err(Error::BadRequest(match settings.forge {
                Forge::Github => "choose an owner to search its repos on GitHub".into(),
                Forge::Gitlab => "choose a group to search its projects on GitLab".into(),
            }));
        }
        let cursor = match params.cursor.as_deref() {
            Some(encoded) => {
                let invalid =
                    || Error::BadRequest("repo page expired: start the search again".into());
                if encoded.len() > 4096 {
                    return Err(invalid());
                }
                let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?;
                let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
                if cursor.scope != scope(&settings)
                    || cursor.owner != params.owner
                    || cursor.query != params.query
                    || !(1..=20).contains(&cursor.search_page)
                {
                    return Err(invalid());
                }
                cursor
            }
            None => Cursor {
                scope: scope(&settings),
                owner: params.owner.clone(),
                query: params.query.clone(),
                main: None,
                account: None,
                main_done: params.owner.is_none() && main_owner(&settings).is_empty(),
                account_done: params.owner.is_some() || !settings.github_sources,
                search_page: 1,
            },
        };
        let key = serde_json::to_string(&cursor)?;
        let asked = Instant::now();
        let (generation, entry) = {
            let mut state = self.picker.state.lock().unwrap();
            if state.pages.len() >= 128 && !state.pages.contains_key(&key) {
                state.pages.clear();
            }
            let entry = state.pages.entry(key).or_default().clone();
            (state.generation, entry)
        };
        let mut cached = entry.lock().await;
        let fresh = cached.as_ref().is_some_and(|page| {
            page.at.elapsed() < PAGE_TTL && (!params.refresh || page.at >= asked)
        });
        let mut github_error = None;
        if !fresh {
            let fetched = match settings.forge {
                Forge::Github => {
                    tokio::time::timeout(PAGE_TIMEOUT, fetch_page(&settings, cursor, env)).await
                }
                Forge::Gitlab => {
                    tokio::time::timeout(PAGE_TIMEOUT, fetch_gitlab_page(&settings, cursor, env))
                        .await
                }
            };
            let fetched = fetched.unwrap_or_else(|_| {
                Err(anyhow::anyhow!(match settings.forge {
                    Forge::Github => "GitHub took too long to list repos: try again",
                    Forge::Gitlab => "GitLab took too long to list projects: try again",
                }))
            });
            match fetched {
                Ok(page) => {
                    let mut state = self.picker.state.lock().unwrap();
                    if generation != state.generation {
                        return Err(Error::Conflict(
                            "GitHub sources changed: reopen the repo picker".into(),
                        ));
                    }
                    state.known.entry(scope(&settings)).or_default().extend(
                        page.repos
                            .iter()
                            .cloned()
                            .map(|repo| (repo.full_name.to_lowercase(), repo)),
                    );
                    *cached = Some(CachedPage {
                        at: Instant::now(),
                        refreshed_at: crate::store::now(),
                        page,
                    });
                }
                Err(error) => {
                    tracing::warn!(%error, "could not list a github repo page");
                    github_error = Some(format!("{error:#}"));
                }
            }
        }
        let page = cached
            .as_ref()
            .map(|page| page.page.clone())
            .unwrap_or_default();
        if generation != self.picker.state.lock().unwrap().generation {
            return Err(Error::Conflict(
                "GitHub sources changed: reopen the repo picker".into(),
            ));
        }
        let last_use = self.store.last_use_by_clone()?;
        let repos = page
            .repos
            .into_iter()
            .map(|repo| {
                let mut info = self.repo_info(repo, &settings);
                info.last_used = info
                    .clone_path
                    .as_ref()
                    .and_then(|path| last_use.get(path))
                    .cloned();
                info
            })
            .collect();
        Ok(ReposPageResult {
            repos,
            next_cursor: page.next_cursor,
            truncated: page.truncated,
            refreshed_at: cached.as_ref().map(|page| page.refreshed_at.clone()),
            github_error,
        })
    }
}

fn scope(settings: &UserSettings) -> String {
    match settings.forge {
        Forge::Github => format!(
            "{}|{}",
            settings.github_org.to_lowercase(),
            settings.github_sources
        ),
        Forge::Gitlab => format!(
            "gitlab|{}|{}|{}",
            settings.gitlab_host,
            settings.gitlab_group.to_lowercase(),
            settings.github_sources
        ),
    }
}

/// A page of a GitLab's projects: the cursor's `main` and `account` are the next page numbers of
/// the main group's (or the chosen group's) and the account's; a search reads the chosen group's.
async fn fetch_gitlab_page(
    settings: &UserSettings,
    mut cursor: Cursor,
    env: &ShellEnv,
) -> anyhow::Result<Page> {
    use super::gitlab::{ACCOUNT_PROJECTS, group_projects, page_of};
    let host = settings.gitlab_host.as_str();
    anyhow::ensure!(!host.is_empty(), "no GitLab host is set");
    if !cursor.query.is_empty() {
        let owner = cursor.owner.as_deref().expect("a search has an owner");
        let search = format!(
            "{}&search={}",
            group_projects(owner),
            percent_encoding::utf8_percent_encode(
                &cursor.query,
                percent_encoding::NON_ALPHANUMERIC
            )
        );
        let repos = page_of(host, &search, cursor.search_page, PAGE_SIZE, env).await?;
        let more = repos.len() == PAGE_SIZE && cursor.search_page < 20;
        cursor.search_page += 1;
        return Ok(Page {
            repos,
            next_cursor: more.then(|| encode(&cursor)).transpose()?,
            truncated: false,
        });
    }
    let owner = cursor
        .owner
        .clone()
        .unwrap_or_else(|| main_owner(settings).to_string());
    let mut repos = HashMap::new();
    for (endpoint, next, done) in [
        (
            group_projects(&owner),
            &mut cursor.main,
            &mut cursor.main_done,
        ),
        (
            ACCOUNT_PROJECTS.to_string(),
            &mut cursor.account,
            &mut cursor.account_done,
        ),
    ] {
        if *done {
            continue;
        }
        let page = next
            .as_deref()
            .and_then(|n| n.parse().ok())
            .unwrap_or(1usize);
        let listed = page_of(host, &endpoint, page, PAGE_SIZE, env).await?;
        *done = listed.len() < PAGE_SIZE || page >= 20;
        *next = Some((page + 1).to_string());
        for repo in listed {
            repos.insert(repo.full_name.to_lowercase(), repo);
        }
    }
    let mut repos: Vec<CachedRepo> = repos.into_values().collect();
    repos.sort_by(|a, b| {
        b.pushed_at
            .cmp(&a.pushed_at)
            .then(a.full_name.cmp(&b.full_name))
    });
    Ok(Page {
        repos,
        next_cursor: (!(cursor.main_done && cursor.account_done))
            .then(|| encode(&cursor))
            .transpose()?,
        truncated: false,
    })
}

fn encode(cursor: &Cursor) -> anyhow::Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(cursor)?))
}

#[derive(Deserialize)]
struct RemoteRepo {
    name: String,
    full_name: String,
    private: bool,
    owner: RemoteOwner,
    default_branch: Option<Branch>,
    description: Option<String>,
    pushed_at: Option<String>,
}

#[derive(Deserialize)]
struct RemoteOwner {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Branch {
    Name(String),
    Ref { name: String },
}

impl From<RemoteRepo> for CachedRepo {
    fn from(repo: RemoteRepo) -> Self {
        Self {
            name: repo.name,
            full_name: repo.full_name,
            private: repo.private,
            owner_is_org: repo.owner.kind == "Organization",
            default_branch: repo.default_branch.map(|branch| match branch {
                Branch::Name(name) | Branch::Ref { name } => name,
            }),
            description: repo
                .description
                .filter(|description| !description.is_empty()),
            pushed_at: repo.pushed_at,
        }
    }
}

async fn fetch_page(
    settings: &UserSettings,
    mut cursor: Cursor,
    env: &ShellEnv,
) -> anyhow::Result<Page> {
    if !cursor.query.is_empty() {
        let owner = cursor.owner.as_deref().expect("a search has an owner");
        let query = format!(
            "{} in:name,description user:{owner} archived:false fork:true",
            serde_json::to_string(&cursor.query)?
        );
        let query =
            percent_encoding::utf8_percent_encode(&query, percent_encoding::NON_ALPHANUMERIC);
        let endpoint = format!(
            "search/repositories?q={query}&per_page={PAGE_SIZE}&page={}&sort=updated&order=desc",
            cursor.search_page
        );
        let stdout = gh_output(&["api", &endpoint], env).await?;
        #[derive(Deserialize)]
        struct Search {
            items: Vec<RemoteRepo>,
            total_count: usize,
            incomplete_results: bool,
        }
        let search: Search = serde_json::from_slice(&stdout)?;
        let more = !search.items.is_empty()
            && cursor.search_page * PAGE_SIZE < search.total_count.min(1000);
        cursor.search_page += 1;
        return Ok(Page {
            repos: search.items.into_iter().map(Into::into).collect(),
            next_cursor: more.then(|| encode(&cursor)).transpose()?,
            truncated: search.incomplete_results || search.total_count > 1000,
        });
    }
    if cursor.main_done && cursor.account_done {
        return Ok(Page::default());
    }
    let owner = cursor.owner.as_deref().unwrap_or(&settings.github_org);
    let mut args = vec![
        "api".to_string(),
        "graphql".to_string(),
        "-f".to_string(),
        format!("query={BROWSE_QUERY}"),
        "-f".to_string(),
        format!("owner={owner}"),
        "-F".to_string(),
        format!("mainEnabled={}", !cursor.main_done),
        "-F".to_string(),
        format!("accountEnabled={}", !cursor.account_done),
    ];
    for (name, value) in [
        ("mainCursor", &cursor.main),
        ("accountCursor", &cursor.account),
    ] {
        if let Some(value) = value {
            args.extend(["-f".into(), format!("{name}={value}")]);
        }
    }
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    let stdout = gh_output(&args, env).await?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Response {
        data: Data,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Data {
        repository_owner: Option<Owner>,
        viewer: Option<Owner>,
    }
    #[derive(Deserialize)]
    struct Owner {
        repositories: Connection,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Connection {
        nodes: Vec<Option<RemoteRepo>>,
        page_info: PageInfo,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PageInfo {
        has_next_page: bool,
        end_cursor: Option<String>,
    }
    let response: Response = serde_json::from_slice(&stdout)?;
    let mut repos = HashMap::new();
    for (listed, after, done) in [
        (
            response.data.repository_owner,
            &mut cursor.main,
            &mut cursor.main_done,
        ),
        (
            response.data.viewer,
            &mut cursor.account,
            &mut cursor.account_done,
        ),
    ] {
        if *done {
            continue;
        }
        let listed = listed.ok_or_else(|| anyhow::anyhow!("GitHub could not find {owner}"))?;
        let page = listed.repositories;
        anyhow::ensure!(
            !page.page_info.has_next_page || page.page_info.end_cursor.is_some(),
            "GitHub did not return the next repo page"
        );
        *after = page.page_info.end_cursor;
        *done = !page.page_info.has_next_page;
        for repo in page.nodes.into_iter().flatten() {
            repos.insert(repo.full_name.to_lowercase(), repo.into());
        }
    }
    let mut repos: Vec<CachedRepo> = repos.into_values().collect();
    repos.sort_by(|a, b| {
        b.pushed_at
            .cmp(&a.pushed_at)
            .then(a.full_name.cmp(&b.full_name))
    });
    Ok(Page {
        repos,
        next_cursor: (!(cursor.main_done && cursor.account_done))
            .then(|| encode(&cursor))
            .transpose()?,
        truncated: false,
    })
}
