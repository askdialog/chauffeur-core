//! The repos a workspace can include: those of every GitHub owner the account reaches (its own,
//! its organizations', the ones it collaborates on; listed with `gh`, cached) and the clones found
//! under the clone roots. Each resolves to a local clone: an override from settings, an existing
//! clone, or one Chauffeur makes in `~/chauffeur/repos` on first use. The main organization's
//! repos are named by their short name, the others' by `owner/name`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use chauffeur_protocol::client::{CreateRepoParams, Forge};
use chauffeur_protocol::model::RepoInfo;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::git::Git;
use crate::paths::Paths;
use crate::settings::{LiveSettings, UserSettings, expand_home};
use crate::shellenv::ShellEnv;
use crate::store::{CachedRepo, Store};
use crate::{Error, Result};

mod gitlab;
mod picker;

const REFRESHED_AT: &str = "repos_refreshed_at";
/// What the cache was listed for (the main organization, every owner or not): another setting
/// lists again.
const LISTED_FOR: &str = "repos_listed_for";

pub struct RepoRegistry {
    paths: Paths,
    settings: Arc<LiveSettings>,
    store: Store,
    /// One lock per clone: worktrees of a clone share its `.git`, and git's own lock files make
    /// concurrent fetches and worktree operations fail.
    locks: std::sync::Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
    /// Why GitHub last failed to list the repos, until a listing succeeds.
    github_error: std::sync::Mutex<Option<String>>,
    picker: picker::Picker,
}

/// The pickable repos, with how their GitHub listing went.
#[derive(Clone, Debug, Default)]
pub struct RepoListing {
    pub repos: Vec<RepoInfo>,
    /// When GitHub last listed them (RFC 3339).
    pub refreshed_at: Option<String>,
    /// Why GitHub could not list them the last time it was asked: the repos are then the cached
    /// ones and the clones found here.
    pub github_error: Option<String>,
}

/// A repo resolved to a local clone.
#[derive(Clone, Debug)]
pub struct ResolvedRepo {
    /// The short name: the worktree's folder.
    pub name: String,
    pub full_name: String,
    pub clone_path: PathBuf,
    pub default_branch: String,
}

impl RepoRegistry {
    pub fn new(paths: Paths, settings: Arc<LiveSettings>, store: Store) -> RepoRegistry {
        RepoRegistry {
            paths,
            settings,
            store,
            locks: Default::default(),
            github_error: Default::default(),
            picker: Default::default(),
        }
    }

    pub fn lock_for(&self, clone: &Path) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .unwrap()
            .entry(clone.to_path_buf())
            .or_default()
            .clone()
    }

    /// Every pickable repo: the GitHub owners', the clones found under the clone roots that no
    /// owner lists, and the ones declared in settings.
    pub async fn list(&self, refresh: bool, env: &ShellEnv) -> Result<Vec<RepoInfo>> {
        Ok(self.listing(refresh, env).await?.repos)
    }

    /// The pickable repos (`list`), with when GitHub last listed them and why it could not.
    pub async fn listing(&self, refresh: bool, env: &ShellEnv) -> Result<RepoListing> {
        let settings = self.settings.get();
        let main = main_owner(&settings);
        let listed = !main.is_empty() || settings.github_sources;
        let listed_for = match settings.forge {
            Forge::Github => format!("{}|{}", main.to_lowercase(), settings.github_sources),
            Forge::Gitlab => format!(
                "gitlab|{}|{}|{}",
                settings.gitlab_host,
                main.to_lowercase(),
                settings.github_sources
            ),
        };
        let was = self.store.kv_get(LISTED_FOR)?;
        let changed = was.as_ref().is_some_and(|was| *was != listed_for);
        // Another forge, or another GitLab: what was listed there is not to be picked here, even
        // while the new listing fails.
        if changed
            && [was.as_deref().unwrap_or(""), listed_for.as_str()]
                .iter()
                .any(|scope| scope.starts_with("gitlab|"))
        {
            self.store.replace_repos_cache(&[])?;
            self.store.kv_delete(REFRESHED_AT)?;
            self.store.kv_set(LISTED_FOR, &listed_for)?;
            self.picker.clear();
        }
        let cached = self.cached(&settings)?;
        let stale = self
            .store
            .kv_get(REFRESHED_AT)?
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(&at).ok())
            .is_none_or(|at| chrono::Utc::now().signed_duration_since(at).num_hours() >= 24);
        if listed && (refresh || stale || changed || cached.is_empty()) {
            let fetched = match settings.forge {
                Forge::Github => fetch_repos(main, settings.github_sources, env).await,
                Forge::Gitlab => {
                    gitlab::fetch_repos(&settings.gitlab_host, main, settings.github_sources, env)
                        .await
                }
            };
            match fetched {
                Ok(repos) => {
                    self.store.replace_repos_cache(&repos)?;
                    self.store.kv_set(REFRESHED_AT, &crate::store::now())?;
                    self.store.kv_set(LISTED_FOR, &listed_for)?;
                    self.picker.clear();
                    *self.github_error.lock().unwrap() = None;
                }
                Err(error) => {
                    tracing::warn!(%error, "could not list the forge's repos");
                    *self.github_error.lock().unwrap() = Some(format!("{error:#}"));
                }
            }
        } else if !listed {
            *self.github_error.lock().unwrap() = None;
        }
        self.cached_listing()
    }

    pub fn cached_listing(&self) -> Result<RepoListing> {
        let settings = self.settings.get();
        let cached = self.cached(&settings)?;
        self.listing_from(cached, &settings)
    }

    fn listing_from(
        &self,
        cached: Vec<CachedRepo>,
        settings: &UserSettings,
    ) -> Result<RepoListing> {
        let listed = !main_owner(settings).is_empty() || settings.github_sources;
        let last_use = self.store.last_use_by_clone()?;
        let mut by_key: BTreeMap<String, RepoInfo> = BTreeMap::new();
        for repo in cached {
            let info = self.repo_info(repo, settings);
            by_key.insert(info.key.clone(), info);
        }
        // The clones no owner lists: a repo of nowhere, or of an owner the account does not reach.
        // A folder named like another owner's repo is still shown when it is not its clone.
        let cloned: HashSet<String> = by_key
            .values()
            .filter_map(|r| r.clone_path.clone())
            .collect();
        for root in &settings.clone_roots {
            let Ok(entries) = std::fs::read_dir(expand_home(root)) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if path.join(".git").is_dir()
                    && validate_repo_name(&name).is_ok()
                    && !cloned.contains(&display(path.clone()))
                    && !by_key.contains_key(&name)
                {
                    by_key.insert(
                        name.clone(),
                        RepoInfo {
                            key: name.clone(),
                            full_name: name.clone(),
                            owner: String::new(),
                            owner_is_org: false,
                            private: false,
                            local: true,
                            clone_path: Some(display(path)),
                            default_branch: None,
                            description: None,
                            pushed_at: None,
                            last_used: None,
                            is_default: settings.default_repos.contains(&name),
                            name,
                        },
                    );
                }
            }
        }
        for (name, over) in &settings.repos {
            let entry = by_key.entry(name.clone()).or_insert_with(|| RepoInfo {
                name: name.clone(),
                key: name.clone(),
                full_name: format!("{}/{}", main_owner(settings), name),
                owner: main_owner(settings).to_string(),
                owner_is_org: false,
                private: false,
                local: false,
                clone_path: None,
                default_branch: None,
                description: None,
                pushed_at: None,
                last_used: None,
                is_default: settings.default_repos.contains(name),
            });
            if let Some(path) = &over.path {
                entry.clone_path = Some(display(expand_home(path)));
                entry.local = false;
            }
            if over.default_branch.is_some() {
                entry.default_branch = over.default_branch.clone();
            }
        }
        for key in &settings.default_repos {
            let Ok((owner, name)) = parse_repo_ref(settings.forge, key) else {
                continue;
            };
            let owner = owner.unwrap_or(main_owner(settings));
            let info = self.repo_info(
                CachedRepo {
                    name: name.to_string(),
                    full_name: format!("{owner}/{name}"),
                    ..Default::default()
                },
                settings,
            );
            by_key.entry(info.key.clone()).or_insert(info);
        }
        let mut repos: Vec<RepoInfo> = by_key.into_values().collect();
        for repo in &mut repos {
            repo.last_used = repo
                .clone_path
                .as_ref()
                .and_then(|path| last_use.get(path))
                .cloned();
        }
        repos.sort_by(|a, b| {
            let rank = |key: &str| {
                settings
                    .repo_order
                    .iter()
                    .position(|repo| repo == key)
                    .unwrap_or(usize::MAX)
            };
            rank(&a.key)
                .cmp(&rank(&b.key))
                .then(b.is_default.cmp(&a.is_default))
                .then(b.pushed_at.cmp(&a.pushed_at))
                .then(a.key.cmp(&b.key))
        });
        Ok(RepoListing {
            repos,
            refreshed_at: self.store.kv_get(REFRESHED_AT)?.filter(|_| listed),
            github_error: self.github_error.lock().unwrap().clone().filter(|_| listed),
        })
    }

    /// The cached GitHub repos the settings show: every owner's, or the main organization's only.
    fn cached(&self, settings: &UserSettings) -> Result<Vec<CachedRepo>> {
        let mut repos: HashMap<String, CachedRepo> = self
            .store
            .repos_cache()?
            .into_iter()
            .map(|repo| (repo.full_name.to_lowercase(), repo))
            .collect();
        repos.extend(self.picker.known(settings));
        Ok(repos
            .into_values()
            .filter(|repo| is_main(settings, repo.owner()) || settings.github_sources)
            .collect())
    }

    fn repo_info(&self, repo: CachedRepo, settings: &UserSettings) -> RepoInfo {
        let owner = repo.owner().to_string();
        let main_owned = is_main(settings, &owner);
        let key = if main_owned || owner.is_empty() {
            repo.name.clone()
        } else {
            repo.full_name.clone()
        };
        let clone_path = if main_owned || owner.is_empty() {
            self.find_clone(&repo.name)
        } else {
            self.find_owned_clone(&owner, &repo.name)
        };
        RepoInfo {
            is_default: settings.default_repos.contains(&key)
                || settings.default_repos.contains(&repo.full_name),
            key,
            clone_path: clone_path.map(display),
            owner,
            owner_is_org: repo.owner_is_org,
            private: repo.private,
            local: false,
            name: repo.name,
            full_name: repo.full_name,
            default_branch: repo.default_branch,
            description: repo.description,
            pushed_at: repo.pushed_at,
            last_used: None,
        }
    }

    /// Creates `owner/name` on GitHub with a README (a worktree needs a commit to branch from)
    /// and caches it, so that the picker lists it and a workspace clones it on first use.
    /// `account` is the login `gh` is signed in to: another owner is one of its organizations.
    /// A repo the cache or a clone already knows is refused before GitHub is asked; one only GitHub
    /// knows is refused by it, also as a `Conflict`.
    pub async fn create(
        &self,
        params: CreateRepoParams,
        account: Option<&str>,
        env: &ShellEnv,
    ) -> Result<RepoInfo> {
        let settings = self.settings.get();
        if settings.forge == Forge::Gitlab {
            return Err(Error::BadRequest(
                "Chauffeur creates repos on GitHub only: create the project on your GitLab, then pick it"
                    .into(),
            ));
        }
        validate_owner(Forge::Github, &params.owner)?;
        validate_repo_name(&params.name)?;
        let full_name = format!("{}/{}", params.owner, params.name);
        if self
            .cached(&settings)?
            .iter()
            .any(|repo| repo.full_name.eq_ignore_ascii_case(&full_name))
        {
            return Err(Error::Conflict(format!(
                "{full_name} already exists: pick it from the list, or choose another name"
            )));
        }
        if let Some(clone) = self.existing_clone(&full_name) {
            return Err(Error::Conflict(format!(
                "a clone named {} is already at {}: choose another name",
                params.name,
                clone.display()
            )));
        }
        let description = params
            .description
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty());
        create_on_github(&full_name, params.private, description.as_deref(), env).await?;
        let repo = CachedRepo {
            name: params.name,
            owner_is_org: is_main(&settings, &params.owner)
                || account.is_some_and(|login| !login.eq_ignore_ascii_case(&params.owner)),
            full_name,
            private: params.private,
            default_branch: None,
            description,
            pushed_at: Some(crate::store::now()),
        };
        self.store.cache_repo(&repo)?;
        Ok(self.repo_info(repo, &settings))
    }

    /// Names of every repo known from the main organization's cache and the settings.
    pub fn known_names(&self) -> Result<std::collections::HashSet<String>> {
        let settings = self.settings.get();
        let mut names: std::collections::HashSet<String> = self
            .store
            .repos_cache()?
            .into_iter()
            .filter(|repo| is_main(&settings, repo.owner()))
            .map(|repo| repo.name)
            .collect();
        names.extend(settings.repos.keys().cloned());
        Ok(names)
    }

    /// Like [`RepoRegistry::known_names`], listing the organization first when nothing is cached
    /// yet (a fresh install).
    pub async fn known_names_loaded(
        &self,
        env: &ShellEnv,
    ) -> Result<std::collections::HashSet<String>> {
        let settings = self.settings.get();
        let known = self.known_names()?;
        if !known.is_empty() || main_owner(&settings).is_empty() {
            return Ok(known);
        }
        self.list(false, env).await?;
        self.known_names()
    }

    /// An existing local clone of `name`.
    pub fn find_clone(&self, name: &str) -> Option<PathBuf> {
        // Joined into paths below: a name like `../../x` must never get that far.
        validate_repo_name(name).ok()?;
        let settings = self.settings.get();
        if let Some(path) = settings.repos.get(name).and_then(|o| o.path.as_ref()) {
            return Some(expand_home(path));
        }
        let roots = settings
            .clone_roots
            .iter()
            .map(|root| expand_home(root))
            .chain(std::iter::once(self.paths.repos_dir.clone()));
        for root in roots {
            let candidate = root.join(name);
            if candidate.join(".git").is_dir() {
                return Some(candidate);
            }
        }
        None
    }

    /// An existing local clone of a repo as a request names it (`name` or `owner/name`).
    pub fn existing_clone(&self, reference: &str) -> Option<PathBuf> {
        let settings = self.settings.get();
        let (owner, name) = parse_repo_ref(settings.forge, reference).ok()?;
        match owner.filter(|owner| !is_main(&settings, owner)) {
            Some(owner) => self.find_owned_clone(owner, name),
            None => self.find_clone(name),
        }
    }

    /// An existing local clone of `owner/name`, a repo outside the main organization: the one
    /// Chauffeur made (`~/chauffeur/repos/<owner>/<name>`), else a clone of that name under the
    /// clone roots whose `origin` is that repo (a folder name alone may be another owner's).
    pub fn find_owned_clone(&self, owner: &str, name: &str) -> Option<PathBuf> {
        let settings = self.settings.get();
        validate_owner(settings.forge, owner).ok()?;
        validate_repo_name(name).ok()?;
        let own = self.paths.repos_dir.join(owner).join(name);
        if own.join(".git").is_dir() {
            return Some(own);
        }
        let full_name = format!("{owner}/{name}");
        let host = forge_host(&settings);
        settings
            .clone_roots
            .iter()
            .map(|root| expand_home(root))
            .chain(std::iter::once(self.paths.repos_dir.clone()))
            .map(|root| root.join(name))
            .find(|clone| clone.join(".git").is_dir() && origin_is(clone, host, &full_name))
    }

    /// Resolves a repo (`name`, or `owner/name` outside the main organization) to a local clone,
    /// cloning it if needed.
    pub async fn resolve(
        &self,
        reference: &str,
        git: &Git,
        env: &ShellEnv,
    ) -> Result<ResolvedRepo> {
        let settings = self.settings.get();
        let (owner, name) = parse_repo_ref(settings.forge, reference)?;
        let default_in_main = !main_owner(&settings).is_empty()
            && settings.default_repos.iter().any(|repo| repo == name);
        let mut cache = self.cached(&settings)?;
        if owner.is_none()
            && !cache.iter().any(|repo| repo.name == name)
            && !settings.repos.contains_key(name)
            && !default_in_main
            && self.find_clone(name).is_none()
        {
            // A name nothing local knows: list GitHub first when the cache is empty (a fresh
            // install, or the one an upgrade emptied) or stale, as the picker would.
            self.list(false, env).await?;
            cache = self.cached(&settings)?;
        }
        let owner = match owner.filter(|owner| !is_main(&settings, owner)) {
            Some(owner) => Some(owner.to_string()),
            None => {
                let in_main = cache
                    .iter()
                    .any(|repo| repo.name == name && is_main(&settings, repo.owner()));
                if in_main
                    || owner.is_some()
                    || settings.repos.contains_key(name)
                    || default_in_main
                    || self.find_clone(name).is_some()
                {
                    None
                } else {
                    // A short name that neither the main organization, the settings nor a clone
                    // knows: another owner's repo of that name, if only one has it.
                    let others: Vec<&CachedRepo> =
                        cache.iter().filter(|repo| repo.name == name).collect();
                    match others.as_slice() {
                        [one] => Some(one.owner().to_string()),
                        [] => {
                            return Err(Error::NotFound(format!(
                                "repo {name} is neither in {} nor in the settings",
                                match (main_owner(&settings), settings.forge) {
                                    ("", Forge::Github) => "your GitHub repos",
                                    ("", Forge::Gitlab) => "your GitLab projects",
                                    (main, _) => main,
                                }
                            )));
                        }
                        many => {
                            return Err(Error::BadRequest(format!(
                                "several owners have a repo named {name}: name it {}",
                                many.iter()
                                    .map(|repo| repo.full_name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(" or ")
                            )));
                        }
                    }
                }
            }
        };
        if let Some(owner) = owner {
            return self.resolve_owned(&owner, name, cache, git, env).await;
        }
        let override_ = settings.repos.get(name);
        let cached = cache
            .into_iter()
            .find(|repo| repo.name == name && is_main(&settings, repo.owner()));
        let full_name = cached
            .as_ref()
            .map(|r| r.full_name.clone())
            .unwrap_or_else(|| format!("{}/{}", main_owner(&settings), name));
        let clone_path = match self.find_clone(name) {
            Some(path) => path,
            None => {
                let target = self.paths.repos_dir.join(name);
                self.clone_into(&full_name, &target, env).await?;
                target
            }
        };
        let default_branch = match override_.and_then(|o| o.default_branch.clone()) {
            Some(branch) => branch,
            None => match cached.and_then(|c| c.default_branch) {
                Some(branch) => branch,
                None => git
                    .remote_default_branch(&clone_path)
                    .await
                    .unwrap_or_else(|| "main".into()),
            },
        };
        Ok(ResolvedRepo {
            name: name.to_string(),
            full_name,
            clone_path,
            default_branch,
        })
    }

    /// Resolves `owner/name`, a repo outside the main organization: listed or not (a URL pasted),
    /// cloned into `~/chauffeur/repos/<owner>/<name>` when no clone of it exists.
    async fn resolve_owned(
        &self,
        owner: &str,
        name: &str,
        cache: Vec<CachedRepo>,
        git: &Git,
        env: &ShellEnv,
    ) -> Result<ResolvedRepo> {
        let asked = format!("{owner}/{name}");
        let cached = cache
            .into_iter()
            .find(|repo| repo.full_name.eq_ignore_ascii_case(&asked));
        // GitHub's own case, when listed: the folder Chauffeur clones into follows it.
        let full_name = cached.as_ref().map_or(asked, |r| r.full_name.clone());
        let (owner, name) = full_name.rsplit_once('/').unwrap_or((owner, name));
        let clone_path = match self.find_owned_clone(owner, name) {
            Some(path) => path,
            None => {
                let target = self.paths.repos_dir.join(owner).join(name);
                self.clone_into(&full_name, &target, env).await?;
                target
            }
        };
        let default_branch = match cached.and_then(|c| c.default_branch) {
            Some(branch) => branch,
            None => git
                .remote_default_branch(&clone_path)
                .await
                .unwrap_or_else(|| "main".into()),
        };
        Ok(ResolvedRepo {
            name: name.to_string(),
            full_name: full_name.clone(),
            clone_path,
            default_branch,
        })
    }

    async fn clone_into(&self, full_name: &str, target: &Path, env: &ShellEnv) -> Result<()> {
        let lock = self.lock_for(target);
        let _guard = lock.lock().await;
        if !target.join(".git").is_dir() {
            let settings = self.settings.get();
            match settings.forge {
                Forge::Github => clone_repo(full_name, target, env).await?,
                Forge::Gitlab => {
                    gitlab::clone_repo(&settings.gitlab_host, full_name, target, env).await?
                }
            }
        }
        Ok(())
    }
}

/// `owner` is the main organization, or the main group on GitLab (both ignore case).
fn is_main(settings: &UserSettings, owner: &str) -> bool {
    let main = main_owner(settings);
    !main.is_empty() && owner.eq_ignore_ascii_case(main)
}

/// The owner whose repos are named short: GitHub's main organization, or GitLab's main group
/// (docs/adr/gitlab-as-the-forge.md).
fn main_owner(settings: &UserSettings) -> &str {
    match settings.forge {
        Forge::Github => &settings.github_org,
        Forge::Gitlab => &settings.gitlab_group,
    }
}

/// The host a clone's `origin` names: github.com, or the GitLab's.
fn forge_host(settings: &UserSettings) -> &str {
    match settings.forge {
        Forge::Github => "github.com",
        Forge::Gitlab => &settings.gitlab_host,
    }
}

/// A repo as a request names it: `name`, or `owner/name`; on GitLab the owner is a group's path,
/// which may hold subgroups (`team/sub/name`).
pub fn parse_repo_ref(forge: Forge, reference: &str) -> Result<(Option<&str>, &str)> {
    let split = match forge {
        Forge::Github => reference.split_once('/'),
        Forge::Gitlab => reference.rsplit_once('/'),
    };
    match split {
        Some((owner, name)) => {
            validate_owner(forge, owner)?;
            validate_repo_name(name)?;
            Ok((Some(owner), name))
        }
        None => {
            validate_repo_name(reference)?;
            Ok((None, reference))
        }
    }
}

/// The folder a repo (`name` or `owner/name`) gets inside a workspace: its short name.
pub fn repo_folder(reference: &str) -> &str {
    reference
        .rsplit_once('/')
        .map_or(reference, |(_, name)| name)
}

/// Refuses an owner that is not a GitHub login (letters, digits and inner hyphens), or on GitLab
/// a group's path ([`is_gitlab_namespace`]): it is joined into the folder a repo is cloned into.
fn validate_owner(forge: Forge, owner: &str) -> Result<()> {
    match forge {
        Forge::Github => {
            let valid = !owner.is_empty()
                && !owner.starts_with('-')
                && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
            if valid {
                Ok(())
            } else {
                Err(Error::BadRequest(format!(
                    "{owner:?} is not a GitHub account or organization"
                )))
            }
        }
        Forge::Gitlab if is_gitlab_namespace(owner) => Ok(()),
        Forge::Gitlab => Err(Error::BadRequest(format!(
            "{owner:?} is not a GitLab group or user"
        ))),
    }
}

/// A GitLab namespace path (`team`, `team/sub`): segments of letters, digits, `_`, `-` and `.`,
/// none empty nor starting with `.` or `-`, so every one is a plain folder name.
pub fn is_gitlab_namespace(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && !segment.starts_with(['.', '-'])
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        })
}

/// The clone's `origin` remote is the repo `full_name` on `host` (`https://github.com/o/n.git`,
/// `git@github.com:o/n`, `ssh://git@gitlab.example.com:2222/team/sub/n.git`, any case).
fn origin_is(clone: &Path, host: &str, full_name: &str) -> bool {
    let Ok(config) = std::fs::read_to_string(clone.join(".git").join("config")) else {
        return false;
    };
    let mut in_origin = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            in_origin = line.replace(' ', "") == "[remote\"origin\"]";
            continue;
        }
        if !in_origin {
            continue;
        }
        let Some(url) = line
            .strip_prefix("url")
            .and_then(|rest| rest.trim_start().strip_prefix('='))
        else {
            continue;
        };
        let url = url.trim().trim_end_matches('/');
        let url = url.strip_suffix(".git").unwrap_or(url);
        if host.is_empty() {
            return false;
        }
        let Some((_, path)) = url.split_once(host) else {
            return false;
        };
        // On a GitLab, an `ssh://` URL may name a port after the host (`host:2222/path`), which is
        // not part of the path; `host:1234/path` without the scheme is a path, as on GitHub.
        let path = match path.strip_prefix(':').map(|rest| rest.split_once('/')) {
            Some(Some((port, rest)))
                if host != "github.com"
                    && url.starts_with("ssh://")
                    && !port.is_empty()
                    && port.bytes().all(|b| b.is_ascii_digit()) =>
            {
                rest
            }
            _ => path,
        };
        return path
            .trim_start_matches([':', '/'])
            .eq_ignore_ascii_case(full_name);
    }
    false
}

/// Refuses a repo name that is not a single plain folder name (GitHub's charset, no leading dot):
/// names are joined under the workspace folder and the clone roots.
pub fn validate_repo_name(name: &str) -> Result<()> {
    let bad = |why: &str| Error::BadRequest(format!("repo name {name:?} is not valid: {why}"));
    if name.is_empty() {
        return Err(bad("it is empty"));
    }
    let mut components = Path::new(name).components();
    let single = matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(_)), None)
    );
    if !single || name.contains('/') {
        return Err(bad("it must be one folder name"));
    }
    if name.starts_with('.') {
        return Err(bad("it starts with ."));
    }
    if let Some(c) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    {
        return Err(bad(&format!(
            "it contains {c:?} (letters, digits, ., _ and - only)"
        )));
    }
    Ok(())
}

fn display(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

/// The repos to pick from: the main organization's (`gh repo list`, which also reaches an
/// organization the account is not a member of) and, with `sources`, every repo the account
/// reaches (`user/repos`: its own, its organizations', the ones it collaborates on). Archived ones
/// are left out. Fails only when nothing could be listed.
async fn fetch_repos(main: &str, sources: bool, env: &ShellEnv) -> anyhow::Result<Vec<CachedRepo>> {
    let (main_listed, sources_listed) = tokio::join!(
        async {
            if main.is_empty() {
                None
            } else {
                Some(fetch_owner_repos(main, env).await)
            }
        },
        async {
            if sources {
                Some(fetch_account_repos(env).await)
            } else {
                None
            }
        },
    );
    let mut by_name: HashMap<String, CachedRepo> = HashMap::new();
    let mut errors = Vec::new();
    // The account's listing last: it knows whether an owner is an organization.
    for listed in [main_listed, sources_listed].into_iter().flatten() {
        match listed {
            Ok(repos) => {
                for repo in repos {
                    by_name.insert(repo.full_name.to_lowercase(), repo);
                }
            }
            Err(error) => errors.push(error),
        }
    }
    // The main organization's failure first: it is the one that says which repos are missing.
    if let Some(error) = errors.into_iter().next() {
        if by_name.is_empty() {
            return Err(error);
        }
        tracing::warn!(%error, "some GitHub repos could not be listed");
    }
    Ok(by_name.into_values().collect())
}

async fn gh_output(args: &[&str], env: &ShellEnv) -> anyhow::Result<Vec<u8>> {
    let output = Command::new("gh")
        .args(args)
        .env_clear()
        .envs(env.vars())
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("running gh {}", args[..2].join(" ")))?;
    anyhow::ensure!(
        output.status.success(),
        "gh {} failed: {}",
        args[..2].join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

async fn fetch_owner_repos(owner: &str, env: &ShellEnv) -> anyhow::Result<Vec<CachedRepo>> {
    let stdout = gh_output(
        &[
            "repo",
            "list",
            owner,
            "--limit",
            "300",
            "--no-archived",
            "--json",
            "name,nameWithOwner,defaultBranchRef,description,pushedAt,isPrivate,isInOrganization",
        ],
        env,
    )
    .await?;
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GhRepo {
        name: String,
        name_with_owner: String,
        default_branch_ref: Option<GhRef>,
        description: Option<String>,
        pushed_at: Option<String>,
        #[serde(default)]
        is_private: bool,
        #[serde(default)]
        is_in_organization: bool,
    }
    #[derive(serde::Deserialize)]
    struct GhRef {
        name: String,
    }
    let repos: Vec<GhRepo> = serde_json::from_slice(&stdout)?;
    Ok(repos
        .into_iter()
        .map(|repo| CachedRepo {
            name: repo.name,
            full_name: repo.name_with_owner,
            owner_is_org: repo.is_in_organization,
            private: repo.is_private,
            default_branch: repo.default_branch_ref.map(|r| r.name),
            description: repo.description.filter(|d| !d.is_empty()),
            pushed_at: repo.pushed_at,
        })
        .collect())
}

/// Every repo the account reaches, one JSON object per line (`gh api --paginate --jq`).
async fn fetch_account_repos(env: &ShellEnv) -> anyhow::Result<Vec<CachedRepo>> {
    let stdout = gh_output(
        &[
            "api",
            "--paginate",
            "user/repos?per_page=100&sort=pushed&affiliation=owner,collaborator,organization_member",
            "--jq",
            ".[] | select(.archived | not) | {name, full_name, private, owner_type: .owner.type, default_branch, description, pushed_at}",
        ],
        env,
    )
    .await?;
    #[derive(serde::Deserialize)]
    struct GhRepo {
        name: String,
        full_name: String,
        #[serde(default)]
        private: bool,
        owner_type: Option<String>,
        default_branch: Option<String>,
        description: Option<String>,
        pushed_at: Option<String>,
    }
    serde_json::Deserializer::from_slice(&stdout)
        .into_iter::<GhRepo>()
        .map(|repo| {
            let repo = repo?;
            Ok(CachedRepo {
                name: repo.name,
                full_name: repo.full_name,
                owner_is_org: repo.owner_type.as_deref() == Some("Organization"),
                private: repo.private,
                default_branch: repo.default_branch,
                description: repo.description.filter(|d| !d.is_empty()),
                pushed_at: repo.pushed_at,
            })
        })
        .collect()
}

async fn create_on_github(
    full_name: &str,
    private: bool,
    description: Option<&str>,
    env: &ShellEnv,
) -> Result<()> {
    let mut command = Command::new("gh");
    command
        .args(["repo", "create", full_name, "--add-readme"])
        .arg(if private { "--private" } else { "--public" });
    if let Some(description) = description {
        command.args(["--description", description]);
    }
    let output = command
        .env_clear()
        .envs(env.vars())
        .env("GH_PROMPT_DISABLED", "1")
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => Error::Unavailable(format!(
                "could not create {full_name}: {}",
                crate::github::NOT_CONNECTED
            )),
            _ => anyhow::Error::from(error)
                .context("running gh repo create")
                .into(),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = format!("could not create {full_name} on GitHub: {}", stderr.trim());
        // A name taken on GitHub (a repo the cache does not list yet) is the client's to change,
        // like one the cache knows; anything else is GitHub's or gh's.
        return Err(if stderr.contains("already exists") {
            Error::Conflict(message)
        } else {
            Error::Unavailable(message)
        });
    }
    Ok(())
}

async fn clone_repo(full_name: &str, target: &Path, env: &ShellEnv) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let output = Command::new("gh")
        .args(["repo", "clone", full_name])
        .arg(target)
        .env_clear()
        .envs(env.vars())
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => Error::Unavailable(format!(
                "could not clone {full_name}: {}",
                crate::github::NOT_CONNECTED
            )),
            _ => anyhow::Error::from(error)
                .context("running gh repo clone")
                .into(),
        })?;
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

    fn cached(full_name: &str) -> CachedRepo {
        CachedRepo {
            name: full_name.split_once('/').unwrap().1.into(),
            full_name: full_name.into(),
            owner_is_org: full_name.starts_with("example/"),
            default_branch: Some("main".into()),
            ..Default::default()
        }
    }

    /// A registry over `repos`, listed just now: nothing asks GitHub.
    fn registry(dir: &Path, repos: &[CachedRepo]) -> (RepoRegistry, Arc<LiveSettings>) {
        let paths = Paths::new(dir.join("home"), dir.join("workspaces"), dir.join("repos"));
        let settings = Arc::new(LiveSettings::load(&paths).unwrap());
        let store = Store::open_in_memory().unwrap();
        store.replace_repos_cache(repos).unwrap();
        store.kv_set(REFRESHED_AT, &crate::store::now()).unwrap();
        (RepoRegistry::new(paths, settings.clone(), store), settings)
    }

    fn git_repo(path: &Path, origin: &str) {
        std::fs::create_dir_all(path.join(".git")).unwrap();
        std::fs::write(
            path.join(".git/config"),
            format!("[core]\n\tbare = false\n[remote \"origin\"]\n\turl = {origin}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n"),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn the_main_organization_names_its_repos_short_and_the_others_by_owner() {
        let dir = tempfile::tempdir().unwrap();
        let (registry, settings) = registry(
            dir.path(),
            &[
                cached("example/my-app"),
                cached("example/blog"),
                cached("someone/blog"),
                cached("someone/dotfiles"),
            ],
        );
        let keys = |repos: Vec<RepoInfo>| {
            let mut keys: Vec<String> = repos.into_iter().map(|r| r.key).collect();
            keys.sort();
            keys
        };
        let env = ShellEnv::new();
        // No main organization: every repo by its owner.
        assert_eq!(
            keys(registry.list(false, &env).await.unwrap()),
            [
                "example/blog",
                "example/my-app",
                "someone/blog",
                "someone/dotfiles"
            ]
        );
        assert!(registry.known_names().unwrap().is_empty());
        settings
            .update(|doc| doc["github_org"] = toml_edit::value("EXAMPLE"))
            .unwrap();
        let listed = registry.list(false, &env).await.unwrap();
        let blog = listed.iter().find(|r| r.key == "someone/blog").unwrap();
        assert_eq!(
            (blog.name.as_str(), blog.owner.as_str()),
            ("blog", "someone")
        );
        assert!(!blog.owner_is_org);
        assert_eq!(
            keys(listed),
            ["blog", "my-app", "someone/blog", "someone/dotfiles"]
        );
        assert!(registry.known_names().unwrap().contains("my-app"));
        assert!(!registry.known_names().unwrap().contains("dotfiles"));
        // Only the main organization's.
        settings
            .update(|doc| doc["github_sources"] = toml_edit::value(false))
            .unwrap();
        assert_eq!(
            keys(registry.list(false, &env).await.unwrap()),
            ["blog", "my-app"]
        );
        settings
            .update(|doc| doc["github_org"] = toml_edit::value("another-team"))
            .unwrap();
        assert!(registry.known_names().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_repo_resolves_to_the_clone_of_its_owner() {
        let dir = tempfile::tempdir().unwrap();
        let (registry, settings) = registry(
            dir.path(),
            &[
                cached("example/blog"),
                cached("someone/blog"),
                cached("someone/dotfiles"),
                cached("other/dotfiles"),
                cached("someone/notes"),
                cached("someone/tool"),
            ],
        );
        let code = dir.path().join("code");
        git_repo(&code.join("blog"), "git@github.com:example/blog.git");
        git_repo(&code.join("notes"), "https://github.com/Someone/Notes");
        git_repo(
            &dir.path().join("repos/someone/blog"),
            "https://github.com/someone/blog.git",
        );
        git_repo(
            &dir.path().join("repos/someone/tool"),
            "https://github.com/someone/tool.git",
        );
        settings
            .update(|doc| {
                doc["github_org"] = toml_edit::value("example");
                doc["clone_roots"] = toml_edit::value(toml_edit::Array::from_iter([code
                    .to_string_lossy()
                    .as_ref()]));
            })
            .unwrap();
        let git = Git::new(ShellEnv::new());
        let env = ShellEnv::new();
        let resolve = |reference: &'static str| {
            let (git, env, registry) = (&git, &env, &registry);
            async move { registry.resolve(reference, git, env).await }
        };
        // The main organization's by its name; another owner's by `owner/name`, never by a
        // folder of that name that is someone else's.
        assert_eq!(resolve("blog").await.unwrap().clone_path, code.join("blog"));
        let theirs = resolve("someone/blog").await.unwrap();
        assert_eq!(theirs.clone_path, dir.path().join("repos/someone/blog"));
        assert_eq!(
            (theirs.name.as_str(), theirs.full_name.as_str()),
            ("blog", "someone/blog")
        );
        assert_eq!(
            resolve("Example/blog").await.unwrap().clone_path,
            code.join("blog")
        );
        // A clone under the clone roots is reused when its origin is that repo.
        assert_eq!(
            resolve("someone/notes").await.unwrap().clone_path,
            code.join("notes")
        );
        // A short name outside the main organization: the one owner that has it, else refused.
        let tool = resolve("tool").await.unwrap();
        assert_eq!(
            (tool.full_name.as_str(), tool.clone_path.clone()),
            ("someone/tool", dir.path().join("repos/someone/tool"))
        );
        let error = resolve("dotfiles").await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("other/dotfiles or someone/dotfiles")
                || error
                    .to_string()
                    .contains("someone/dotfiles or other/dotfiles"),
            "{error}"
        );
        assert!(matches!(resolve("../x").await, Err(Error::BadRequest(_))));
        assert!(matches!(resolve("-x/y").await, Err(Error::BadRequest(_))));

        // The listing says where each is cloned, and that a clone no owner lists is local.
        git_repo(&code.join("scratch"), "https://gitlab.com/me/scratch.git");
        let listed = registry.list(false, &env).await.unwrap();
        let find = |key: &str| {
            listed
                .iter()
                .find(|r| r.key == key)
                .unwrap_or_else(|| panic!("{key}"))
        };
        assert_eq!(
            find("someone/blog").clone_path.as_deref(),
            Some(
                dir.path()
                    .join("repos/someone/blog")
                    .to_string_lossy()
                    .as_ref()
            )
        );
        assert_eq!(find("someone/dotfiles").clone_path, None);
        assert!(find("scratch").local);
        assert!(!find("blog").local);
        assert_eq!(listed.iter().filter(|r| r.name == "notes").count(), 1);
    }

    #[tokio::test]
    async fn on_gitlab_the_main_group_names_its_projects_short_and_clones_are_found_by_origin() {
        let dir = tempfile::tempdir().unwrap();
        let project = |full_name: &str| CachedRepo {
            name: full_name.rsplit_once('/').unwrap().1.into(),
            full_name: full_name.into(),
            owner_is_org: true,
            default_branch: Some("main".into()),
            ..Default::default()
        };
        let (registry, settings) = registry(dir.path(), &[]);
        let code = dir.path().join("code");
        settings
            .update(|doc| {
                doc["forge"] = toml_edit::value("gitlab");
                doc["gitlab_host"] = toml_edit::value("gitlab.example.com");
                doc["gitlab_group"] = toml_edit::value("Team");
                doc["github_sources"] = toml_edit::value(true);
                let mut roots = toml_edit::Array::new();
                roots.push(code.to_string_lossy().as_ref());
                doc["clone_roots"] = toml_edit::value(roots);
            })
            .unwrap();
        // What GitLab listed, as `fetch_repos` caches it.
        registry
            .store
            .replace_repos_cache(&[
                project("team/web"),
                project("team/shop/api"),
                project("other/tool"),
            ])
            .unwrap();
        registry
            .store
            .kv_set(REFRESHED_AT, &crate::store::now())
            .unwrap();
        registry
            .store
            .kv_set(LISTED_FOR, "gitlab|gitlab.example.com|team|true")
            .unwrap();
        git_repo(&code.join("web"), "https://gitlab.example.com/team/web.git");
        git_repo(
            &code.join("api"),
            "git@gitlab.example.com:team/shop/api.git",
        );
        git_repo(
            &code.join("tool"),
            "ssh://git@gitlab.example.com:2222/other/tool.git",
        );
        let listed = registry.list(false, &ShellEnv::new()).await.unwrap();
        let find = |key: &str| listed.iter().find(|r| r.key == key).unwrap();
        assert_eq!(find("web").clone_path, Some(display(code.join("web"))));
        assert_eq!(
            (
                find("team/shop/api").owner.as_str(),
                find("team/shop/api").clone_path.clone()
            ),
            ("team/shop", Some(display(code.join("api"))))
        );
        assert_eq!(
            find("other/tool").clone_path,
            Some(display(code.join("tool")))
        );
        assert_eq!(
            registry.existing_clone("team/shop/api"),
            Some(code.join("api"))
        );
        // Back on GitHub (nothing listed there, so nothing asked of `gh`), what the GitLab listed
        // is no longer offered.
        settings
            .update(|doc| {
                doc["forge"] = toml_edit::value("github");
                doc["github_sources"] = toml_edit::value(false);
            })
            .unwrap();
        registry.list(false, &ShellEnv::new()).await.unwrap();
        assert!(registry.store.repos_cache().unwrap().is_empty());
    }

    #[test]
    fn a_gitlab_reference_may_name_subgroups_a_github_one_may_not() {
        assert_eq!(
            parse_repo_ref(Forge::Gitlab, "team/shop/api").unwrap(),
            (Some("team/shop"), "api")
        );
        assert_eq!(parse_repo_ref(Forge::Gitlab, "web").unwrap(), (None, "web"));
        for bad in [
            "team/../api",
            "team//api",
            ".team/api",
            "team/-x/api",
            "team/sh op/api",
        ] {
            assert!(parse_repo_ref(Forge::Gitlab, bad).is_err(), "{bad}");
        }
        assert!(parse_repo_ref(Forge::Github, "team/shop/api").is_err());
        assert!(parse_repo_ref(Forge::Github, "my.org/api").is_err());
    }

    #[test]
    fn origins_match_gitlab_projects_and_github_keeps_its_reading() {
        let dir = tempfile::tempdir().unwrap();
        let host = "gitlab.example.com";
        for (url, full_name, matches) in [
            (
                "https://gitlab.example.com/team/shop/api.git",
                "team/shop/api",
                true,
            ),
            (
                "git@gitlab.example.com:Team/Shop/API.git",
                "team/shop/api",
                true,
            ),
            (
                "ssh://git@gitlab.example.com:2222/team/shop/api.git",
                "team/shop/api",
                true,
            ),
            (
                "https://gitlab.example.com/team/api.git",
                "team/shop/api",
                false,
            ),
            ("https://github.com/team/shop.git", "team/shop", false),
        ] {
            let clone = dir.path().join(format!("g{}", url.len()));
            git_repo(&clone, url);
            assert_eq!(origin_is(&clone, host, full_name), matches, "{url}");
        }
        // A GitHub owner of digits is an owner, not a port.
        let clone = dir.path().join("digits");
        git_repo(&clone, "git@github.com:1234/repo.git");
        assert!(origin_is(&clone, "github.com", "1234/repo"));
        assert!(!origin_is(&clone, "", "1234/repo"));
    }

    #[test]
    fn origins_match_github_repos_in_any_form() {
        let dir = tempfile::tempdir().unwrap();
        for (url, full_name, matches) in [
            ("https://github.com/owner/repo.git", "owner/repo", true),
            ("git@github.com:Owner/Repo", "owner/repo", true),
            ("ssh://git@github.com/owner/repo/", "owner/repo", true),
            ("https://github.com/owner/repo-2.git", "owner/repo", false),
            ("https://gitlab.com/owner/repo.git", "owner/repo", false),
        ] {
            let clone = dir.path().join(format!("c{}", url.len()));
            git_repo(&clone, url);
            assert_eq!(origin_is(&clone, "github.com", full_name), matches, "{url}");
        }
        assert_eq!(repo_folder("owner/repo"), "repo");
        assert_eq!(repo_folder("repo"), "repo");
        assert!(parse_repo_ref(Forge::Github, "owner/repo/x").is_err());
        assert_eq!(
            parse_repo_ref(Forge::Github, "o-1/r.x").unwrap(),
            (Some("o-1"), "r.x")
        );
    }

    #[test]
    fn repo_names_are_single_plain_folder_names() {
        for ok in ["web", "repo_1", "a.b", "x", "My-API"] {
            assert!(validate_repo_name(ok).is_ok(), "{ok} should be accepted");
        }
        for bad in [
            "",
            ".",
            "..",
            "../x",
            "../../x",
            "a/b",
            "/etc",
            "a/",
            ".git",
            ".chauffeur",
            "a b",
            "a\0b",
            "a:b",
            "é",
        ] {
            let error = validate_repo_name(bad).expect_err(bad);
            assert!(matches!(error, Error::BadRequest(_)), "{bad}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_github_listing_says_why_and_keeps_the_cached_repos_and_the_clones() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (registry, settings) = registry(dir.path(), &[cached("example/my-app")]);
        let clones = dir.path().join("clones");
        git_repo(&clones.join("notes"), "");
        let roots = clones.display().to_string();
        settings
            .update(|doc| {
                doc["github_org"] = toml_edit::value("example");
                doc["clone_roots"] =
                    toml_edit::value(toml_edit::Array::from_iter([roots.as_str()]));
            })
            .unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let gh = bin.join("gh");
        std::fs::write(
            &gh,
            "#!/bin/sh\necho 'authentication token expired' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = ShellEnv::new();
        env.set_vars(BTreeMap::from([(
            "PATH".to_string(),
            format!("{}:/usr/bin:/bin", bin.display()),
        )]));

        let listing = registry.listing(true, &env).await.unwrap();
        let keys: Vec<&str> = listing.repos.iter().map(|r| r.key.as_str()).collect();
        assert!(
            keys.contains(&"my-app") && keys.contains(&"notes"),
            "{keys:?}"
        );
        assert!(listing.refreshed_at.is_some());
        let error = listing.github_error.expect("the failure is told");
        assert!(error.contains("authentication token expired"), "{error}");

        // Asked again without a refresh, the failure is still told until a listing succeeds.
        let again = registry.listing(false, &env).await.unwrap();
        assert!(again.github_error.is_some());
    }

    #[tokio::test]
    async fn a_created_repo_is_listed_at_once_and_one_that_exists_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (registry, settings) = registry(dir.path(), &[cached("someone/blog")]);
        settings
            .update(|doc| doc["github_org"] = toml_edit::value("example"))
            .unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let gh = bin.join("gh");
        let log = dir.path().join("gh.log");
        std::fs::write(
            &gh,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$3\" in *taken*) echo 'Name already exists on this account' >&2; exit 1;; esac\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = ShellEnv::new();
        env.set_vars(BTreeMap::from([(
            "PATH".to_string(),
            format!("{}:/usr/bin:/bin", bin.display()),
        )]));
        let params =
            |owner: &str, name: &str, private: bool, description: Option<&str>| CreateRepoParams {
                owner: owner.into(),
                name: name.into(),
                private,
                description: description.map(Into::into),
            };

        let mine = registry
            .create(
                params("someone", "fresh", true, Some(" A new app ")),
                Some("someone"),
                &env,
            )
            .await
            .unwrap();
        assert_eq!(mine.key, "someone/fresh");
        assert!(mine.private && !mine.owner_is_org);
        assert_eq!(mine.description.as_deref(), Some("A new app"));
        let org = registry
            .create(
                params("example", "tool", false, None),
                Some("someone"),
                &env,
            )
            .await
            .unwrap();
        assert_eq!(
            org.key, "tool",
            "the main organization's repo is named short"
        );
        assert!(org.owner_is_org && !org.private);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "repo create someone/fresh --add-readme --private --description A new app\n\
             repo create example/tool --add-readme --public\n"
        );
        let keys: Vec<String> = registry
            .picker_listing()
            .unwrap()
            .repos
            .into_iter()
            .map(|r| r.key)
            .collect();
        assert!(
            keys.contains(&"someone/fresh".to_string()) && keys.contains(&"tool".to_string()),
            "{keys:?}"
        );

        for existing in ["someone/BLOG", "someone/fresh"] {
            let (owner, name) = existing.split_once('/').unwrap();
            let refused = registry
                .create(params(owner, name, true, None), Some("someone"), &env)
                .await;
            assert!(matches!(refused, Err(Error::Conflict(_))), "{refused:?}");
        }
        let clone = dir.path().join("repos").join("local-only");
        git_repo(&clone, "");
        let refused = registry
            .create(params("example", "local-only", true, None), None, &env)
            .await;
        assert!(matches!(refused, Err(Error::Conflict(_))), "{refused:?}");
        assert!(matches!(
            registry
                .create(params("some one", "x", true, None), None, &env)
                .await,
            Err(Error::BadRequest(_))
        ));
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 2);

        // A name only GitHub knows is refused by it, as a conflict too.
        let Err(Error::Conflict(error)) = registry
            .create(params("someone", "taken", true, None), None, &env)
            .await
        else {
            panic!("GitHub's refusal is told");
        };
        assert!(error.contains("Name already exists"), "{error}");
        let org = registry
            .create(params("example", "unknown-account", true, None), None, &env)
            .await
            .unwrap();
        assert!(
            org.owner_is_org,
            "the main organization is one, whatever gh says of the account"
        );
        assert!(
            !registry
                .cached_listing()
                .unwrap()
                .repos
                .iter()
                .any(|r| r.name == "taken")
        );
    }
}
