//! Design shares (story 51): a design chat's canvas on the share service, live, for named
//! reviewers; and a markdown file of a workspace shared alone the same way (story 57), its link
//! keyed `file:<workspace id>:<path>` where a design's is keyed by its chat. The daemon creates the share over HTTPS with the user's Google ID token (the
//! Office's sign-in), uploads the design's files, and keeps one WebSocket per shared chat: its
//! `comments.json` and its file changes go up, the reviewers' comments come down into
//! `comments.json`. The link runs while the share lives, through restarts of the daemon, and
//! reconnects with backoff; the service keeps the last state while it is away.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use chauffeur_protocol::client::{
    CreateChatParams, DesignComment, DesignImportParams, DesignShareState, FileShareStartParams,
    FilesLinkedResult, ReviewerComment, ShareConnection, ShareStartParams, SharedDesignsResult,
};
use chauffeur_protocol::design_share::{
    FromDaemon, SHARE_PROTOCOL_VERSION, ShareParams, ShareSnapshot, ShareStatus, SharedDesign,
    ToDaemon,
};
use chauffeur_protocol::events::{ChatEvent, WorkspaceEvent};
use chauffeur_protocol::model::{ChatInfo, ChatKind};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;

use crate::google::{self, REFRESH_TOKEN};
use crate::linked_files::{self, Linked};
use crate::secrets::Secrets;
use crate::store::DesignShareRow;
use crate::{Daemon, Error, Result};

const PAST_REVIEWERS_KEY: &str = "design_share_reviewers";
const PAST_REVIEWERS_MAX: usize = 30;
/// How often a shared design's folder is looked at for changes.
const SCAN_EVERY: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
/// Larger files stay on the Mac (the service refuses them too).
const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
const SCAN_DEPTH: usize = 8;
/// Never uploaded: the exports (PNGs the user made), and the comments (they go over the socket).
const SKIPPED_DIRS: [&str; 1] = ["exports"];
/// How long a workspace's list of files serves to find linked files before it is read again.
const LINK_INDEX_FRESH: Duration = Duration::from_secs(30);

pub struct Shares {
    daemon: Weak<Daemon>,
    secrets: Secrets,
    http: reqwest::Client,
    links: Mutex<HashMap<String, Link>>,
    /// Each workspace's files as the file linker indexes them, and when they were listed.
    link_index: Mutex<HashMap<String, (Instant, Arc<linked_files::Index>)>>,
}

/// A share read for a copy (`Shares::read`), its files still on the service.
pub struct SharedCopy {
    base: String,
    pub id: String,
    token: String,
    pub snapshot: ShareSnapshot,
}

/// One shared chat's link, while the share lives.
struct Link {
    stop: Arc<Notify>,
    connection: ShareConnection,
    status: Option<ShareStatus>,
    comments_rev: u64,
    last_comment: Option<ReviewerComment>,
}

/// What the service holds of the link's folder: each file's change time when it was sent.
pub(crate) type Listing = BTreeMap<String, u64>;

/// What a markdown file's share is keyed by, where a design's is keyed by its chat.
const FILE_KEY: &str = "file:";

pub fn file_key(workspace_id: &str, path: &str) -> String {
    format!("{FILE_KEY}{workspace_id}:{path}")
}

/// The workspace and the path of a markdown file's share key; none for a design chat's.
fn file_of(key: &str) -> Option<(&str, &str)> {
    key.strip_prefix(FILE_KEY)?.split_once(':')
}

/// What a link shares.
enum Source {
    /// A design chat's folder, `comments.json` beside its screens.
    Design { dir: PathBuf },
    /// One markdown file, shared by its `name`; its comments are the workspace's notes'
    /// `comments.json` entries on its `path`.
    File {
        notes: PathBuf,
        real: PathBuf,
        path: String,
        name: String,
        /// The files it links to, when the share carries them.
        carried: Option<Arc<Carried>>,
    },
}

/// The linked files a markdown file's share carries, with the files on the Mac behind them.
pub(crate) struct Carried {
    linked: Linked,
    real: HashMap<String, PathBuf>,
    manifest: Vec<u8>,
}

impl Carried {
    fn new(linked: Linked, real: HashMap<String, PathBuf>) -> Carried {
        let manifest = linked.manifest_bytes();
        Carried {
            linked,
            real,
            manifest,
        }
    }
}

/// A manifest's version in a listing, where a file's is its change time: its bytes' hash.
fn manifest_version(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

impl Source {
    fn scan(&self) -> Listing {
        match self {
            Source::Design { dir } => scan(dir),
            Source::File {
                real,
                name,
                carried,
                ..
            } => {
                let fits = |real: &Path| {
                    std::fs::metadata(real).is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE_BYTES)
                };
                let mut listing: Listing = fits(real)
                    .then(|| (name.clone(), modified_ms(real)))
                    .into_iter()
                    .collect();
                if let Some(carried) = carried {
                    for (shared, real) in &carried.real {
                        if fits(real) {
                            listing.insert(shared.clone(), modified_ms(real));
                        }
                    }
                    if !carried.linked.files.is_empty() {
                        listing.insert(
                            linked_files::MANIFEST.to_string(),
                            manifest_version(&carried.manifest),
                        );
                    }
                }
                listing
            }
        }
    }

    /// The bytes of a shared path: a file on the Mac, or the linked files' manifest.
    async fn bytes(&self, shared: &str) -> std::io::Result<Vec<u8>> {
        let file = match self {
            Source::Design { dir } => dir.join(shared),
            Source::File { real, carried, .. } => match carried {
                Some(carried) if shared == linked_files::MANIFEST => {
                    return Ok(carried.manifest.clone());
                }
                Some(carried) if carried.real.contains_key(shared) => carried.real[shared].clone(),
                _ => real.clone(),
            },
        };
        tokio::fs::read(file).await
    }

    /// Carries `linked` beside a markdown file; a design carries nothing.
    fn carry(&mut self, linked: Option<Arc<Carried>>) {
        if let Source::File { carried, .. } = self {
            *carried = linked;
        }
    }

    fn comments_file(&self) -> PathBuf {
        match self {
            Source::Design { dir } => dir.join(crate::comments::FILE),
            Source::File { notes, .. } => notes.join(crate::comments::FILE),
        }
    }

    /// The comments the service holds: a design's, or the file's, named as the share names it.
    fn comments(&self) -> Vec<DesignComment> {
        match self {
            Source::Design { dir } => crate::comments::read(dir).0,
            Source::File {
                notes,
                path,
                name,
                carried,
                ..
            } => crate::comments::read(notes)
                .0
                .into_iter()
                .filter_map(|c| {
                    let screen = if &c.screen == path {
                        name.clone()
                    } else {
                        carried.as_ref()?.linked.shared_of(&c.screen)?.to_string()
                    };
                    Some(DesignComment { screen, ..c })
                })
                .collect(),
        }
    }

    /// A reviewer's comment, new or changed, kept on the Mac.
    fn keep(&self, comment: DesignComment) -> Result<()> {
        match self {
            Source::Design { dir } => crate::comments::merge_remote(dir, comment),
            Source::File {
                notes,
                path,
                carried,
                ..
            } => {
                // A comment on a linked file is kept on that file's path; any other on the file's.
                let screen = carried
                    .as_ref()
                    .and_then(|c| c.linked.path_of(&comment.screen))
                    .unwrap_or(path)
                    .to_string();
                crate::file_comments::merge_remote(notes, DesignComment { screen, ..comment })
            }
        }
    }

    fn remove(&self, id: &str) -> Result<()> {
        match self {
            Source::Design { dir } => crate::comments::remove_remote(dir, id),
            Source::File { notes, .. } => crate::file_comments::remove_remote(notes, id),
        }
    }
}

impl Shares {
    pub fn new(daemon: Weak<Daemon>, secrets: Secrets) -> Shares {
        Shares {
            daemon,
            secrets,
            http: reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .expect("an HTTP client"),
            links: Mutex::new(HashMap::new()),
            link_index: Mutex::new(HashMap::new()),
        }
    }

    fn d(&self) -> Arc<Daemon> {
        self.daemon
            .upgrade()
            .expect("the daemon outlives its shares")
    }

    /// The service's URL without a trailing slash; none when sharing is turned off.
    pub(crate) fn base_url(&self) -> Option<String> {
        let url = self
            .d()
            .settings
            .get()
            .design_share
            .url
            .trim()
            .trim_end_matches('/')
            .to_string();
        (!url.is_empty()).then_some(url)
    }

    pub(crate) fn email(&self) -> Option<String> {
        crate::identity::email(&self.d())
    }

    /// A fresh Google ID token from Chauffeur's sign-in; `dev:<email>` on a development setup.
    pub(crate) async fn token(&self) -> std::result::Result<String, String> {
        let settings = self.d().settings.get().office.clone();
        let dev = settings.dev_email.trim().to_lowercase();
        if !dev.is_empty() {
            return Ok(format!("dev:{dev}"));
        }
        let client = google::Client::from_settings(&settings).ok_or(
            "Set office.google_client_id and office.google_client_secret in settings.toml",
        )?;
        let refresh = self
            .secrets
            .read(&REFRESH_TOKEN)
            .ok_or("Sign in with Google in Chauffeur to share a design or a workspace")?;
        client.id_token(&refresh).await
    }

    pub(crate) fn past_reviewers(&self) -> Vec<String> {
        self.d()
            .store
            .kv_get(PAST_REVIEWERS_KEY)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    pub(crate) fn remember_reviewers(&self, reviewers: &[String]) {
        let mut all: Vec<String> = reviewers.to_vec();
        for email in self.past_reviewers() {
            if !all.contains(&email) {
                all.push(email);
            }
        }
        all.truncate(PAST_REVIEWERS_MAX);
        let _ = self.d().store.kv_set(
            PAST_REVIEWERS_KEY,
            &serde_json::to_string(&all).expect("e-mails serialize"),
        );
    }

    pub(crate) fn notes_dir(&self, workspace: &str) -> Result<PathBuf> {
        let d = self.d();
        let row = d.workspaces.row(workspace)?;
        let live = Path::new(&row.path).join(crate::paths::NOTES_DIR);
        let archived = d.paths.archive_dir.join(&row.slug);
        let archived_source = matches!(
            row.state,
            chauffeur_protocol::model::WorkspaceState::Archived
                | chauffeur_protocol::model::WorkspaceState::CleanupFailed
                | chauffeur_protocol::model::WorkspaceState::Archiving
        );
        Ok(if archived_source && archived.exists() {
            archived
        } else {
            live
        })
    }

    /// Inventory from persisted owner rows, independent of the open workspace/chat tabs.
    pub fn list(&self) -> Result<Vec<chauffeur_protocol::client::OutgoingShare>> {
        use chauffeur_protocol::client::{OutgoingShare, OutgoingShareSource};
        let d = self.d();
        let mut result = Vec::new();
        for row in d.store.design_shares_live()? {
            let (workspace_id, name, source, closed) =
                if let Some((workspace, path)) = file_of(&row.chat_id) {
                    (
                        workspace.to_string(),
                        Path::new(path)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.to_string()),
                        OutgoingShareSource::File {
                            path: path.to_string(),
                        },
                        false,
                    )
                } else {
                    let Some(chat) = d.store.chat(&row.chat_id)? else {
                        continue;
                    };
                    (
                        chat.workspace_id,
                        chat.title.unwrap_or_else(|| "Untitled design".into()),
                        OutgoingShareSource::Design { chat: chat.id },
                        chat.closed,
                    )
                };
            let workspace = d.workspaces.row(&workspace_id)?;
            result.push(OutgoingShare {
                id: row.share_id,
                url: row.url,
                name,
                workspace: workspace.id,
                workspace_name: workspace.title.unwrap_or(workspace.slug),
                archived: closed || workspace.archived_at.is_some(),
                source,
                connection: self.state_of(&row.chat_id).connection,
            });
        }
        for row in d.store.workspace_shares_live()? {
            let workspace = d.workspaces.row(&row.workspace_id)?;
            let name = workspace.title.unwrap_or(workspace.slug);
            result.push(OutgoingShare {
                id: row.share_id,
                url: row.url,
                name: name.clone(),
                workspace: workspace.id,
                workspace_name: name,
                archived: workspace.archived_at.is_some(),
                source: OutgoingShareSource::Workspace,
                connection: d.workspace_shares.state(&row.workspace_id)?.connection,
            });
        }
        result.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        Ok(result)
    }

    /// The share's state as the app shows it.
    pub fn state(&self, chat_key: &str) -> Result<DesignShareState> {
        let chat = self.d().chats.resolve(chat_key)?;
        if chat.kind != ChatKind::Design {
            return Err(Error::BadRequest("only a design chat is shared".into()));
        }
        Ok(self.state_of(&chat.id))
    }

    fn state_of(&self, chat_id: &str) -> DesignShareState {
        let links = self.links.lock().unwrap();
        let link = links.get(chat_id);
        let row = self.d().store.design_share(chat_id).ok().flatten();
        let linked = row
            .as_ref()
            .is_some_and(|r| r.stopped_at.is_none() && r.linked);
        let share = link.and_then(|l| l.status.clone()).or_else(|| {
            // Before the service answered (or while it is away): what the daemon knows.
            row.filter(|r| r.stopped_at.is_none()).map(|r| ShareStatus {
                id: r.share_id,
                url: r.url,
                title: String::new(),
                owner: chauffeur_protocol::client::CommentAuthor {
                    name: String::new(),
                    email: self.email().unwrap_or_default(),
                },
                people: known_people(self.email(), &r.reviewers),
                reviewers: r.reviewers,
                team: r.team,
                viewers: Vec::new(),
                online: false,
                last_push_at: None,
                stopped: false,
                notice: None,
            })
        });
        DesignShareState {
            chat: chat_id.to_string(),
            configured: self.base_url().is_some(),
            email: self.email(),
            share,
            connection: link
                .map(|l| l.connection.clone())
                .unwrap_or(ShareConnection::Off),
            past_reviewers: self.past_reviewers(),
            comments_rev: link.map(|l| l.comments_rev).unwrap_or(0),
            last_reviewer_comment: link.and_then(|l| l.last_comment.clone()),
            linked,
        }
    }

    /// What a markdown file's share carries beside it this time: none when it carries nothing;
    /// an error when the linked files could not be found (the last ones then stay).
    async fn carry(
        &self,
        key: &str,
        workspace_id: &str,
        source: &Source,
    ) -> Result<Option<Arc<Carried>>> {
        let Source::File { path, name, .. } = source else {
            return Ok(None);
        };
        let linked = self.d().store.design_share(key)?.is_some_and(|r| r.linked);
        if !linked || !crate::file_comments::is_markdown(path) {
            return Ok(None);
        }
        let (linked, real) = self.find_linked(workspace_id, path, name).await?;
        Ok(Some(Arc::new(Carried::new(linked, real))))
    }

    /// The markdown files a markdown file links to, recursively, as its share carries them.
    pub async fn linked(&self, workspace: &str, path: &str) -> Result<FilesLinkedResult> {
        let target = crate::file_comments::shared_file(&self.d(), workspace, path)?;
        if !crate::file_comments::is_markdown(path) {
            return Ok(FilesLinkedResult {
                files: Vec::new(),
                truncated: false,
            });
        }
        let (linked, _) = self.find_linked(&target.workspace_id, path, "").await?;
        Ok(FilesLinkedResult {
            files: linked.files.into_iter().map(|f| f.path).collect(),
            truncated: linked.truncated,
        })
    }

    /// The files `path` links to, and the files on the Mac behind them.
    async fn find_linked(
        &self,
        workspace_id: &str,
        path: &str,
        name: &str,
    ) -> Result<(Linked, HashMap<String, PathBuf>)> {
        let d = self.d();
        let (row, roots) = crate::rpc::file_roots(&d, workspace_id)?;
        let fresh = self
            .link_index
            .lock()
            .unwrap()
            .get(workspace_id)
            .filter(|(at, _)| at.elapsed() < LINK_INDEX_FRESH)
            .map(|(_, index)| index.clone());
        let index = match fresh {
            Some(index) => index,
            None => {
                let git = crate::git::Git::new(d.shell_env.clone());
                let index = Arc::new(linked_files::Index::new(
                    &crate::files::list(&git, &roots).await?,
                ));
                self.link_index
                    .lock()
                    .unwrap()
                    .insert(workspace_id.to_string(), (Instant::now(), index.clone()));
                index
            }
        };
        let read = |file: &str| {
            let real = crate::files::resolve(&roots, file).ok()?;
            let fits = std::fs::metadata(&real).ok()?.len() <= MAX_FILE_BYTES;
            fits.then(|| std::fs::read_to_string(&real).ok()).flatten()
        };
        let linked = linked_files::find(path, name, Path::new(&row.path), &index, read);
        let real = linked
            .files
            .iter()
            .filter_map(|f| {
                let real = crate::files::resolve(&roots, &f.path).ok()?;
                Some((f.shared.clone(), real))
            })
            .collect();
        Ok((linked, real))
    }

    /// Who a comment of the design chat may mention: the people its live share is open to, as
    /// the service last told them (what the daemon knows while it is away); none when the
    /// design is not shared.
    pub fn audience(&self, chat_id: &str) -> Option<crate::comments::Audience> {
        let share = self.state_of(chat_id).share.filter(|s| !s.stopped)?;
        let people = if share.people.is_empty() {
            known_people(self.email(), &share.reviewers)
        } else {
            share.people
        };
        let team_domain = if share.team {
            crate::identity::state(&self.d())
                .ok()
                .and_then(|identity| identity.team_domain)
        } else {
            None
        };
        Some(crate::comments::Audience {
            people,
            team_domain,
        })
    }

    fn publish(&self, workspace_id: &str, key: &str) {
        let state = Box::new(self.state_of(key));
        if file_of(key).is_some() {
            self.d()
                .events
                .workspace(workspace_id, WorkspaceEvent::FileShare { state });
        } else {
            self.d()
                .events
                .chat(workspace_id, key, ChatEvent::DesignShare { state });
        }
    }

    /// A markdown file's share state as the app shows it.
    pub fn file_state(&self, workspace: &str, path: &str) -> Result<DesignShareState> {
        let target = crate::file_comments::shared_file(&self.d(), workspace, path)?;
        Ok(self.state_of(&file_key(&target.workspace_id, path)))
    }

    fn update_link(&self, chat_id: &str, edit: impl FnOnce(&mut Link)) {
        if let Some(link) = self.links.lock().unwrap().get_mut(chat_id) {
            edit(link);
        }
    }

    /// Starts the links of the shares alive in the store: at the daemon's start.
    pub fn resume(self: &Arc<Self>) {
        let rows = self.d().store.design_shares_live().unwrap_or_default();
        for row in rows {
            self.spawn(row);
        }
    }

    fn spawn(self: &Arc<Self>, row: DesignShareRow) {
        let Some((workspace_id, _)) = self.source(&row.chat_id) else {
            // A shared file gone (deleted or moved while the daemon was away): its last copy
            // must not stay readable on the service.
            if let Some((workspace_id, path)) = file_of(&row.chat_id) {
                tracing::info!(path, "a shared file is gone; its share stops");
                let shares = self.clone();
                let workspace_id = workspace_id.to_string();
                tokio::spawn(async move { shares.end(&row, &workspace_id).await });
            }
            return;
        };
        let stop = Arc::new(Notify::new());
        {
            let mut links = self.links.lock().unwrap();
            if let Some(old) = links.insert(
                row.chat_id.clone(),
                Link {
                    stop: stop.clone(),
                    connection: ShareConnection::Connecting,
                    status: None,
                    comments_rev: 0,
                    last_comment: None,
                },
            ) {
                old.stop.notify_waiters();
            }
        }
        let shares = self.clone();
        tokio::spawn(async move {
            shares.run(row, workspace_id, stop).await;
        });
    }

    /// What a link's key shares, and its workspace; none once the chat, the workspace or the
    /// file is gone.
    fn source(&self, key: &str) -> Option<(String, Source)> {
        let d = self.d();
        if let Some((workspace, path)) = file_of(key) {
            let target = crate::file_comments::shared_file(&d, workspace, path).ok()?;
            let name = Path::new(path).file_name()?.to_string_lossy().into_owned();
            return Some((
                target.workspace_id,
                Source::File {
                    notes: target.notes,
                    real: target.real,
                    path: path.to_string(),
                    name,
                    carried: None,
                },
            ));
        }
        let chat = d.store.chat(key).ok()??;
        let workspace = d.workspaces.row(&chat.workspace_id).ok()?;
        let dir = self
            .notes_dir(&workspace.id)
            .ok()?
            .join(crate::paths::DESIGNS_DIR)
            .join(&chat.id);
        Some((chat.workspace_id, Source::Design { dir }))
    }

    /// Shares a design chat: a new share on the service, a link for it.
    pub async fn start(self: &Arc<Self>, params: &ShareStartParams) -> Result<DesignShareState> {
        let d = self.d();
        let chat = d.chats.resolve(&params.chat)?;
        if chat.kind != ChatKind::Design {
            return Err(Error::BadRequest("only a design chat is shared".into()));
        }
        let title = share_title(
            chat.title.as_deref(),
            d.workspaces.row(&chat.workspace_id)?.title.as_deref(),
        );
        self.open(
            &chat.id,
            &chat.workspace_id,
            ShareParams {
                title: Some(title),
                reviewers: Some(params.reviewers.clone()),
                team: Some(params.team.unwrap_or(false)),
                document: None,
            },
            false,
        )
        .await
    }

    /// Shares a markdown file of a workspace alone: a new share on the service holding that
    /// file, a link for it.
    pub async fn start_file(
        self: &Arc<Self>,
        params: &FileShareStartParams,
    ) -> Result<DesignShareState> {
        let d = self.d();
        let target = crate::file_comments::shared_file(&d, &params.workspace, &params.path)?;
        let name = Path::new(&params.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| params.path.clone());
        let workspace = d.workspaces.row(&target.workspace_id)?;
        let title = match workspace.title.as_deref().map(str::trim) {
            Some(workspace) if !workspace.is_empty() => format!("{workspace} · {name}"),
            _ => name.clone(),
        };
        self.open(
            &file_key(&target.workspace_id, &params.path),
            &target.workspace_id,
            ShareParams {
                title: Some(title),
                reviewers: Some(params.reviewers.clone()),
                team: Some(params.team.unwrap_or(false)),
                document: Some(name),
            },
            params.linked.unwrap_or(false),
        )
        .await
    }

    /// A new share on the service for `key`, kept and linked.
    async fn open(
        self: &Arc<Self>,
        key: &str,
        workspace_id: &str,
        params: ShareParams,
        linked: bool,
    ) -> Result<DesignShareState> {
        let d = self.d();
        let what = if params.document.is_some() {
            "file"
        } else {
            "design"
        };
        let base = self.base_url().ok_or_else(|| {
            Error::Unavailable("Sharing is off: set design_share.url in settings.toml".into())
        })?;
        let token = self.token().await.map_err(Error::Unavailable)?;
        if let Some(row) = d.store.design_share(key)?
            && row.stopped_at.is_none()
        {
            return Err(Error::Conflict(format!(
                "This {what} is shared already: change its reviewers, or stop the share first"
            )));
        }
        let response = self
            .http
            .post(format!("{base}/v1/shares"))
            .bearer_auth(&token)
            .json(&params)
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("Could not reach the share service: {e}")))?;
        let mut status: ShareStatus = accepted(response)
            .await
            .map_err(Error::Unavailable)?
            .json()
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("the share service's answer: {e}")))?;
        let notice = status.notice.take();
        let row = DesignShareRow {
            chat_id: key.to_string(),
            share_id: status.id.clone(),
            url: status.url.clone(),
            reviewers: status.reviewers.clone(),
            team: status.team,
            linked,
            created_at: crate::store::now(),
            stopped_at: None,
        };
        d.store.design_share_put(&row)?;
        self.remember_reviewers(&status.reviewers);
        self.spawn(row);
        self.update_link(key, |link| link.status = Some(status));
        self.publish(workspace_id, key);
        tracing::info!(key, "{what} shared");
        held_back(notice)?;
        Ok(self.state_of(key))
    }

    /// A design shared with the user, taken over: its files into a new design chat of the
    /// workspace, with its comments (none sent: the new chat's agent has read nothing). The
    /// share's title names the chat; nothing is said to the agent until the user does.
    pub async fn import(&self, params: &DesignImportParams) -> Result<ChatInfo> {
        let d = self.d();
        let share = self.read(&params.url).await?;
        let workspace = d.workspaces.row(&params.workspace)?;
        let chat = d
            .chats
            .create(CreateChatParams {
                workspace: workspace.id.clone(),
                kind: Some(ChatKind::Design),
                title: Some(share.snapshot.title.clone()),
                ..CreateChatParams::default()
            })
            .await?;
        let dir = crate::design::design_dir(&workspace.path, &chat.id)
            .map_err(|error| Error::Internal(anyhow::anyhow!("the design folder: {error}")))?;
        self.copy(&share, Path::new(&dir)).await?;
        tracing::info!(chat = chat.id, share = share.id, "design imported");
        Ok(chat)
    }

    /// A share's link read as its reviewers' daemons read it: what it holds, and the token that
    /// reads its files.
    pub async fn read(&self, url: &str) -> Result<SharedCopy> {
        let base = self.base_url().ok_or_else(|| {
            Error::Unavailable("Sharing is off: set design_share.url in settings.toml".into())
        })?;
        let id = share_id_of(url).ok_or_else(|| {
            Error::BadRequest(format!(
                "Paste a design share link, {base}/s/<id>, not {url}"
            ))
        })?;
        let token = self.token().await.map_err(Error::Unavailable)?;
        let snapshot: ShareSnapshot = self
            .http
            .get(format!("{base}/v1/shares/{id}/snapshot"))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("Could not reach the share service: {e}")))?
            .error_for_status()
            .map_err(|e| Error::Unavailable(refusal(e)))?
            .json()
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("the share service's answer: {e}")))?;
        Ok(SharedCopy {
            base,
            id,
            token,
            snapshot,
        })
    }

    /// A share's files and comments into a design folder.
    pub async fn copy(&self, share: &SharedCopy, dir: &Path) -> Result<()> {
        let SharedCopy {
            base, id, token, ..
        } = share;
        for file in &share.snapshot.files {
            let Some(relative) = crate::canvas::safe_relative(&file.path) else {
                continue;
            };
            let bytes = self
                .http
                .get(format!(
                    "{base}/v1/shares/{id}/files/{}",
                    crate::canvas::encode_path(&file.path)
                ))
                .bearer_auth(token)
                .send()
                .await
                .map_err(|e| Error::Unavailable(format!("Could not read {}: {e}", file.path)))?
                .error_for_status()
                .map_err(|e| Error::Unavailable(refusal(e)))?
                .bytes()
                .await
                .map_err(|e| Error::Unavailable(format!("Could not read {}: {e}", file.path)))?;
            let path = dir.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    Error::Internal(anyhow::anyhow!("creating {}: {e}", parent.display()))
                })?;
            }
            std::fs::write(&path, &bytes)
                .map_err(|e| Error::Internal(anyhow::anyhow!("writing {}: {e}", path.display())))?;
        }
        // The comments live in the service's base, not among the files.
        crate::comments::write_imported(dir, share.snapshot.comments.clone())
    }

    /// The live designs colleagues opened to the user, named or as a member of the team, as the
    /// service lists them for the Office's Design window.
    pub async fn shared(&self) -> Result<SharedDesignsResult> {
        let base = self.base_url().ok_or_else(|| {
            Error::Unavailable("Sharing is off: set design_share.url in settings.toml".into())
        })?;
        let token = self.token().await.map_err(Error::Unavailable)?;
        let designs: Vec<SharedDesign> = self
            .http
            .get(format!("{base}/v1/shares"))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("Could not reach the share service: {e}")))?
            .error_for_status()
            .map_err(|e| Error::Unavailable(refusal(e)))?
            .json()
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("the share service's answer: {e}")))?;
        Ok(SharedDesignsResult { designs })
    }

    /// The reviewers of a live share, and whether the team sees it.
    pub async fn update(&self, params: &ShareStartParams) -> Result<DesignShareState> {
        let chat = self.d().chats.resolve(&params.chat)?;
        self.change(
            &chat.id,
            &chat.workspace_id,
            &params.reviewers,
            params.team,
            None,
        )
        .await
    }

    /// The reviewers of a markdown file's live share, and whether the team sees it.
    pub async fn update_file(&self, params: &FileShareStartParams) -> Result<DesignShareState> {
        let target = crate::file_comments::shared_file(&self.d(), &params.workspace, &params.path)?;
        let key = file_key(&target.workspace_id, &params.path);
        self.change(
            &key,
            &target.workspace_id,
            &params.reviewers,
            params.team,
            params.linked,
        )
        .await
    }

    async fn change(
        &self,
        key: &str,
        workspace_id: &str,
        reviewers: &[String],
        team: Option<bool>,
        linked: Option<bool>,
    ) -> Result<DesignShareState> {
        let d = self.d();
        let row = d
            .store
            .design_share(key)?
            .filter(|r| r.stopped_at.is_none())
            .ok_or_else(|| Error::NotFound("This is not shared".into()))?;
        let base = self.base_url().ok_or_else(|| {
            Error::Unavailable("Sharing is off: set design_share.url in settings.toml".into())
        })?;
        let token = self.token().await.map_err(Error::Unavailable)?;
        let response = self
            .http
            .patch(format!("{base}/v1/shares/{}", row.share_id))
            .bearer_auth(&token)
            .json(&ShareParams {
                title: None,
                reviewers: Some(reviewers.to_vec()),
                team,
                document: None,
            })
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("Could not reach the share service: {e}")))?;
        let mut status: ShareStatus = accepted(response)
            .await
            .map_err(Error::Unavailable)?
            .json()
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("the share service's answer: {e}")))?;
        let notice = status.notice.take();
        d.store.design_share_put(&DesignShareRow {
            reviewers: status.reviewers.clone(),
            team: status.team,
            linked: linked.unwrap_or(row.linked),
            ..row
        })?;
        self.remember_reviewers(&status.reviewers);
        self.update_link(key, |link| link.status = Some(status));
        self.publish(workspace_id, key);
        held_back(notice)?;
        Ok(self.state_of(key))
    }

    /// Stops a share: the service drops its copy, the link ends, the comments stay local.
    pub async fn stop(&self, chat_key: &str) -> Result<DesignShareState> {
        let d = self.d();
        let chat = d.chats.resolve(chat_key)?;
        let row = d
            .store
            .design_share(&chat.id)?
            .filter(|r| r.stopped_at.is_none())
            .ok_or_else(|| Error::NotFound("This design is not shared".into()))?;
        self.end(&row, &chat.workspace_id).await;
        Ok(self.state_of(&chat.id))
    }

    /// Stops a markdown file's share: the service drops its copy, the comments stay local.
    pub async fn stop_file(&self, workspace: &str, path: &str) -> Result<DesignShareState> {
        let d = self.d();
        let row = d.workspaces.row(workspace)?;
        let key = file_key(&row.id, path);
        let share = d
            .store
            .design_share(&key)?
            .filter(|r| r.stopped_at.is_none())
            .ok_or_else(|| Error::NotFound(format!("{path} is not shared")))?;
        self.end(&share, &row.id).await;
        Ok(self.state_of(&key))
    }

    async fn end(&self, row: &DesignShareRow, workspace_id: &str) {
        if let (Some(base), Ok(token)) = (self.base_url(), self.token().await) {
            let deleted = self
                .http
                .delete(format!("{base}/v1/shares/{}", row.share_id))
                .bearer_auth(&token)
                .send()
                .await;
            if let Err(error) = deleted {
                tracing::warn!(%error, chat = row.chat_id, "the share service did not take the stop; the link ends anyway");
            }
        }
        if let Some(link) = self.links.lock().unwrap().remove(&row.chat_id) {
            link.stop.notify_waiters();
        }
        let _ = self.d().store.design_share_put(&DesignShareRow {
            stopped_at: Some(crate::store::now()),
            ..row.clone()
        });
        self.publish(workspace_id, &row.chat_id);
        tracing::info!(chat = row.chat_id, "design share stopped");
    }

    /// A chat deleted: its share stops with it (best effort, in the background).
    pub fn stop_for_chat(self: &Arc<Self>, chat_id: &str, workspace_id: &str) {
        let Ok(Some(row)) = self.d().store.design_share(chat_id) else {
            return;
        };
        if row.stopped_at.is_some() {
            return;
        }
        let shares = self.clone();
        let workspace_id = workspace_id.to_string();
        tokio::spawn(async move { shares.end(&row, &workspace_id).await });
    }

    /// A workspace archived: file shares stop; design links remain active.
    pub fn stop_for_workspace(self: &Arc<Self>, workspace_id: &str) {
        let d = self.d();
        let files = d.store.design_shares_live().unwrap_or_default();
        for row in files
            .into_iter()
            .filter(|r| file_of(&r.chat_id).is_some_and(|(w, _)| w == workspace_id))
        {
            let shares = self.clone();
            let workspace_id = workspace_id.to_string();
            tokio::spawn(async move { shares.end(&row, &workspace_id).await });
        }
    }

    /// The link: connects and connects again while the share lives.
    async fn run(self: Arc<Self>, row: DesignShareRow, workspace_id: String, stop: Arc<Notify>) {
        let mut backoff = Duration::from_secs(1);
        let mut listing: Option<Listing> = None;
        loop {
            self.update_link(&row.chat_id, |l| l.connection = ShareConnection::Connecting);
            self.publish(&workspace_id, &row.chat_id);
            let outcome = tokio::select! {
                outcome = self.session(&row, &workspace_id, &mut listing) => outcome,
                _ = stop.notified() => return,
            };
            match outcome {
                Ok(Ended::Stopped) => {
                    // The service says the share is gone: nothing to reconnect to.
                    self.links.lock().unwrap().remove(&row.chat_id);
                    let _ = self.d().store.design_share_put(&DesignShareRow {
                        stopped_at: Some(crate::store::now()),
                        ..row.clone()
                    });
                    self.publish(&workspace_id, &row.chat_id);
                    return;
                }
                Ok(Ended::Closed) => {
                    self.update_link(&row.chat_id, |l| l.connection = ShareConnection::Connecting);
                    backoff = Duration::from_secs(1);
                }
                Err(message) => {
                    tracing::warn!(%message, chat = row.chat_id, "the share link failed");
                    self.update_link(&row.chat_id, |l| {
                        l.connection = ShareConnection::Failed { message };
                    });
                    self.publish(&workspace_id, &row.chat_id);
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = stop.notified() => return,
            }
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    /// One connection: hello, the design pushed, then changes both ways until it closes.
    async fn session(
        &self,
        row: &DesignShareRow,
        workspace_id: &str,
        listing: &mut Option<Listing>,
    ) -> std::result::Result<Ended, String> {
        let base = self
            .base_url()
            .ok_or("Sharing is off: set design_share.url in settings.toml")?;
        let mut token = self.token().await?;
        let ws_base = base
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        let (socket, _) = tokio_tungstenite::connect_async(format!(
            "{ws_base}/v1/shares/{}/daemon",
            row.share_id
        ))
        .await
        .map_err(|e| format!("Could not reach the share service at {base}: {e}"))?;
        let (mut sink, mut stream) = socket.split();
        let say = |message: &FromDaemon| {
            Message::Text(
                serde_json::to_string(message)
                    .expect("share messages serialize")
                    .into(),
            )
        };
        sink.send(say(&FromDaemon::Hello {
            id_token: token.clone(),
            version: SHARE_PROTOCOL_VERSION,
        }))
        .await
        .map_err(|e| format!("The share service closed: {e}"))?;
        let welcome = loop {
            match stream.next().await {
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<ToDaemon>(&text) {
                    Ok(ToDaemon::Welcome { share }) => break share,
                    Ok(ToDaemon::Refused { reason })
                        if reason.contains("stopped") || reason.contains("no such share") =>
                    {
                        return Ok(Ended::Stopped);
                    }
                    Ok(ToDaemon::Refused { reason }) => return Err(reason),
                    _ => {}
                },
                Some(Ok(_)) => {}
                _ => return Err("The share service closed before its welcome".into()),
            }
        };
        self.update_link(&row.chat_id, |l| {
            l.connection = ShareConnection::Online;
            l.status = Some(welcome);
        });
        self.publish(workspace_id, &row.chat_id);

        // The whole design on the first connection, what changed since on the next ones.
        let mut comments_seen: Option<u64> = None;
        // The linked files the last look found: a reviewer's comment on one is kept on its path.
        let mut carried: Option<Arc<Carried>> = None;
        let mut interval = tokio::time::interval(SCAN_EVERY);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if self.d().workspaces.row(workspace_id).is_ok_and(|w| w.state == chauffeur_protocol::model::WorkspaceState::Archiving) { continue; }
                    let Some((_, mut source)) = self.source(&row.chat_id) else { continue };
                    match self.carry(&row.chat_id, workspace_id, &source).await {
                        Ok(now) => carried = now,
                        Err(error) => tracing::warn!(%error, "could not find the files a shared file links to"),
                    }
                    source.carry(carried.clone());
                    let mut now = source.scan();
                    let (changed, removed) = diff(listing.as_ref(), &now);
                    let mut uploaded = Vec::new();
                    for path in &changed {
                        let bytes = match source.bytes(path).await {
                            Ok(bytes) => bytes,
                            Err(error) => {
                                tracing::warn!(%error, path, "could not read a file to share");
                                unsent(&mut now, listing.as_ref(), path);
                                continue;
                            }
                        };
                        let url = format!("{base}/v1/shares/{}/files/{}", row.share_id, crate::canvas::encode_path(path));
                        let response = authorized(&mut token, || self.token(), |token| {
                            self.http.put(&url).bearer_auth(token).body(bytes.clone())
                        })
                        .await
                        .map_err(|e| format!("Could not upload {path}: {e}"))?;
                        let status = response.status();
                        if status == reqwest::StatusCode::GONE {
                            return Ok(Ended::Stopped);
                        }
                        if status == reqwest::StatusCode::UNAUTHORIZED {
                            return Err(refusal(response.error_for_status().expect_err("a 401 is an error")));
                        }
                        if !status.is_success() {
                            tracing::warn!(path, %status, "the share service refused a file");
                            if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                                unsent(&mut now, listing.as_ref(), path);
                            }
                            continue;
                        }
                        uploaded.push(path.clone());
                    }
                    for path in &removed {
                        let url = format!("{base}/v1/shares/{}/files/{}", row.share_id, crate::canvas::encode_path(path));
                        let _ = authorized(&mut token, || self.token(), |token| {
                            self.http.delete(&url).bearer_auth(token)
                        })
                        .await;
                    }
                    *listing = Some(now);
                    if !uploaded.is_empty() || !removed.is_empty() {
                        sink.send(say(&FromDaemon::Changed { changed: uploaded, removed }))
                            .await
                            .map_err(|e| format!("The share service closed: {e}"))?;
                    }
                    let comments_mtime = modified_ms(&source.comments_file());
                    if comments_seen != Some(comments_mtime) {
                        comments_seen = Some(comments_mtime);
                        let comments = source.comments();
                        sink.send(say(&FromDaemon::Comments { comments }))
                            .await
                            .map_err(|e| format!("The share service closed: {e}"))?;
                    }
                }
                incoming = stream.next() => {
                    let message = match incoming {
                        Some(Ok(Message::Text(text))) => match serde_json::from_str::<ToDaemon>(&text) {
                            Ok(message) => message,
                            Err(_) => continue,
                        },
                        Some(Ok(Message::Ping(payload))) => {
                            let _ = sink.send(Message::Pong(payload)).await;
                            continue;
                        }
                        Some(Ok(Message::Close(_))) | None => return Ok(Ended::Closed),
                        Some(Ok(_)) => continue,
                        Some(Err(e)) => return Err(format!("The share service closed: {e}")),
                    };
                    match message {
                        ToDaemon::Welcome { share } => {
                            self.update_link(&row.chat_id, |l| l.status = Some(share));
                            self.publish(workspace_id, &row.chat_id);
                        }
                        ToDaemon::Comment { comment } => {
                            let me = self.email().unwrap_or_default();
                            let mine = comment.author.email == me;
                            let mentioned = comment.mentions.iter().any(|m| m.email == me);
                            if let Some((_, mut source)) = self.source(&row.chat_id) && let Err(error) = { source.carry(carried.clone()); source.keep(comment.clone()) } {
                                tracing::warn!(%error, "a reviewer's comment could not be kept");
                                continue;
                            }
                            self.update_link(&row.chat_id, |l| {
                                l.comments_rev += 1;
                                if !mine {
                                    l.last_comment = Some(ReviewerComment {
                                        author: comment.author.name.clone(),
                                        screen: comment.screen.clone(),
                                        text: comment.text.clone(),
                                        at: comment.at,
                                        mentioned,
                                    });
                                }
                            });
                            self.publish(workspace_id, &row.chat_id);
                        }
                        ToDaemon::CommentRemoved { id } => {
                            if let Some((_, source)) = self.source(&row.chat_id) { let _ = source.remove(&id); }
                            self.update_link(&row.chat_id, |l| l.comments_rev += 1);
                            self.publish(workspace_id, &row.chat_id);
                        }
                        ToDaemon::Viewers { viewers } => {
                            self.update_link(&row.chat_id, |l| {
                                if let Some(status) = &mut l.status {
                                    status.viewers = viewers;
                                }
                            });
                            self.publish(workspace_id, &row.chat_id);
                        }
                        ToDaemon::Reviewers { reviewers } => {
                            // Changed from the owner's page: the app's dialog follows. The row
                            // is read again: the app may have opened the share to the team since.
                            let current = self
                                .d()
                                .store
                                .design_share(&row.chat_id)
                                .ok()
                                .flatten()
                                .unwrap_or_else(|| row.clone());
                            if let Err(error) = self.d().store.design_share_put(&DesignShareRow {
                                reviewers: reviewers.clone(),
                                ..current
                            }) {
                                tracing::warn!(%error, "the share's reviewers could not be kept");
                            }
                            self.remember_reviewers(&reviewers);
                            self.update_link(&row.chat_id, |l| {
                                if let Some(status) = &mut l.status {
                                    status.reviewers = reviewers;
                                }
                            });
                            self.publish(workspace_id, &row.chat_id);
                        }
                        ToDaemon::Refused { reason } => {
                            if reason.contains("stopped") {
                                return Ok(Ended::Stopped);
                            }
                            return Err(reason);
                        }
                    }
                }
            }
        }
    }
}

/// The owner then the reviewers, each named by their e-mail's first name: who the share is open
/// to while the service has not said their names.
fn known_people(
    owner: Option<String>,
    reviewers: &[String],
) -> Vec<chauffeur_protocol::client::CommentAuthor> {
    owner
        .into_iter()
        .chain(reviewers.iter().cloned())
        .map(|email| chauffeur_protocol::client::CommentAuthor {
            name: crate::comments::first_name(&email),
            email,
        })
        .collect()
}

/// What a share is called for its reviewers: the design chat's own title; when the chat kept
/// its default name, the workspace's title (a reviewer knows "Acme — provider detection",
/// not "Design"); "Design" when neither says anything.
fn share_title(chat: Option<&str>, workspace: Option<&str>) -> String {
    let chat = chat
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "Design");
    let workspace = workspace.map(str::trim).filter(|t| !t.is_empty());
    match (chat, workspace) {
        (Some(chat), Some(workspace)) => format!("{workspace} · {chat}"),
        (Some(chat), None) => chat.to_string(),
        (None, Some(workspace)) => workspace.to_string(),
        (None, None) => "Design".to_string(),
    }
}

/// The share id of a page's address, `…/s/<id>`; none for anything else. The host is not
/// checked: the daemon only ever asks its own service, with its own token, about that id.
fn share_id_of(url: &str) -> Option<String> {
    let (_, rest) = url.trim().rsplit_once("/s/")?;
    let id = rest.split(['/', '?', '#']).next()?;
    (id.len() >= 8 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| id.to_lowercase())
}

/// How a session ended.
enum Ended {
    /// The connection dropped: connect again.
    Closed,
    /// The service says the share is gone.
    Stopped,
}

/// The service's answer when it took the request; else its refusal in a sentence, in the
/// service's own words when it gave some (a limit, an address that is not one).
pub(crate) async fn accepted(response: reqwest::Response) -> Result<reqwest::Response, String> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    if let Err(error) = response.error_for_status_ref()
        && matches!(
            status,
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::GONE
        )
    {
        return Err(refusal(error));
    }
    let said = response
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|body| body.get("error")?.as_str().map(str::to_string));
    Err(said.unwrap_or_else(|| format!("The share service refused ({status})")))
}

/// What the service held back of a share it saved (invitations not e-mailed), told to the owner
/// through the error path: the share is live and published all the same.
pub(crate) fn held_back(notice: Option<String>) -> Result<()> {
    match notice {
        Some(notice) => Err(Error::Conflict(notice)),
        None => Ok(()),
    }
}

/// A refusal of the service as reqwest reports it, in a sentence.
pub(crate) fn refusal(error: reqwest::Error) -> String {
    match error.status() {
        Some(reqwest::StatusCode::UNAUTHORIZED) => {
            "The share service refused the sign-in: sign in with Google again".into()
        }
        Some(reqwest::StatusCode::GONE) => "This share was stopped".into(),
        Some(status) => format!("The share service refused ({status})"),
        None => format!("Could not reach the share service: {error}"),
    }
}

pub(crate) fn modified_ms(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The files a share carries: the folder's, by relative path, with when they changed. Dotfiles,
/// the exports, `comments.json`, kinds not served and files too large are left on the Mac.
pub fn scan(dir: &Path) -> Listing {
    let mut out = Listing::new();
    walk(dir, Path::new(""), 0, &mut out);
    out
}

fn walk(root: &Path, relative: &Path, depth: usize, out: &mut Listing) {
    if depth > SCAN_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root.join(relative)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = relative.join(&*name);
        if kind.is_dir() {
            if depth == 0 && SKIPPED_DIRS.contains(&&*name) {
                continue;
            }
            walk(root, &path, depth + 1, out);
        } else if kind.is_file() {
            if depth == 0 && name == crate::comments::FILE {
                continue;
            }
            if crate::canvas::content_type(&path).is_none() {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.len() > MAX_FILE_BYTES {
                continue;
            }
            out.insert(
                path.to_string_lossy().into_owned(),
                modified_ms(&root.join(&path)),
            );
        }
    }
}

/// What changed since the last listing (everything, without one), and what went.
pub fn diff(before: Option<&Listing>, now: &Listing) -> (Vec<String>, Vec<String>) {
    let changed = now
        .iter()
        .filter(|(path, mtime)| before.and_then(|b| b.get(*path)) != Some(mtime))
        .map(|(path, _)| path.clone())
        .collect();
    let removed = before
        .map(|b| {
            b.keys()
                .filter(|path| !now.contains_key(*path))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    (changed, removed)
}

/// The service's answer to a request made with the session's token, made again with a fresh one
/// when the service refuses the sign-in: a Google ID token lasts an hour, a share's connection
/// days. The fresh token stays the session's.
async fn authorized<Renew, Fresh>(
    token: &mut String,
    renew: Renew,
    request: impl Fn(&str) -> reqwest::RequestBuilder,
) -> std::result::Result<reqwest::Response, String>
where
    Renew: FnOnce() -> Fresh,
    Fresh: std::future::Future<Output = std::result::Result<String, String>>,
{
    let response = request(token).send().await.map_err(|e| e.to_string())?;
    if response.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Ok(response);
    }
    *token = renew().await?;
    request(token).send().await.map_err(|e| e.to_string())
}

/// Leaves `path` in the listing as it was before the scan, so that the next scan sends it again.
fn unsent(now: &mut Listing, before: Option<&Listing>, path: &str) {
    match before.and_then(|b| b.get(path)) {
        Some(mtime) => now.insert(path.to_string(), *mtime),
        None => now.remove(path),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_link_names_its_share() {
        assert_eq!(
            share_id_of("https://design.askdialog.ai/s/92A223191A1188AD").as_deref(),
            Some("92a223191a1188ad")
        );
        assert_eq!(
            share_id_of(" https://design.askdialog.ai/s/92a223191a1188ad/?x=1 ").as_deref(),
            Some("92a223191a1188ad")
        );
        for other in [
            "https://design.askdialog.ai/s/",
            "https://design.askdialog.ai/s/not-hex",
            "92a223191a1188ad",
            "https://design.askdialog.ai/v1/shares/92a223191a1188ad",
        ] {
            assert!(share_id_of(other).is_none(), "{other}");
        }
    }

    #[test]
    fn a_share_is_named_for_its_reviewers() {
        assert_eq!(
            share_title(Some("Design"), Some("Acme — CTA")),
            "Acme — CTA"
        );
        assert_eq!(share_title(None, Some("Acme — CTA")), "Acme — CTA");
        assert_eq!(
            share_title(Some("Widget CTA"), Some("Acme")),
            "Acme · Widget CTA"
        );
        assert_eq!(share_title(Some("Widget CTA"), None), "Widget CTA");
        assert_eq!(share_title(Some(" "), None), "Design");
    }

    #[test]
    fn a_share_carries_the_design_s_files_and_not_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join("a")).unwrap();
        std::fs::create_dir_all(d.join("exports")).unwrap();
        std::fs::create_dir_all(d.join(".claude")).unwrap();
        for (path, body) in [
            ("a/default.html", "<p>a</p>"),
            ("widget.css", "p{}"),
            ("canvas.json", "{}"),
            ("comments.json", "{\"comments\": []}"),
            ("notes.txt", "x"),
            ("private.pdf", "%PDF-1.4"),
            ("exports/shot.png", "png"),
            (".claude/settings.local.json", "{}"),
        ] {
            std::fs::write(d.join(path), body).unwrap();
        }
        let listing = scan(d);
        assert_eq!(
            listing.keys().collect::<Vec<_>>(),
            ["a/default.html", "canvas.json", "widget.css"]
        );
        assert!(listing.values().all(|m| *m > 0));
    }

    #[test]
    fn a_diff_names_what_changed_and_what_went() {
        let before: Listing = [
            ("a.html".to_string(), 1u64),
            ("b.html".to_string(), 2),
            ("c.html".to_string(), 3),
        ]
        .into_iter()
        .collect();
        let now: Listing = [
            ("a.html".to_string(), 1u64),
            ("b.html".to_string(), 5),
            ("d.html".to_string(), 1),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            diff(Some(&before), &now),
            (
                vec!["b.html".into(), "d.html".into()],
                vec!["c.html".into()]
            )
        );
        assert_eq!(diff(None, &now).0.len(), 3, "everything, the first time");
        assert!(diff(None, &now).1.is_empty());
    }

    #[test]
    fn a_file_the_service_did_not_take_is_sent_again_and_not_removed() {
        let before: Listing = [("canvas.json".to_string(), 1u64)].into_iter().collect();
        let scanned: Listing = [
            ("canvas.json".to_string(), 2u64),
            ("new.html".to_string(), 2),
        ]
        .into_iter()
        .collect();
        let mut kept = scanned.clone();
        unsent(&mut kept, Some(&before), "canvas.json");
        unsent(&mut kept, Some(&before), "new.html");
        assert_eq!(kept, before);
        assert_eq!(
            diff(Some(&kept), &scanned),
            (vec!["canvas.json".into(), "new.html".into()], vec![])
        );
    }

    #[tokio::test]
    async fn an_expired_sign_in_is_renewed_and_the_request_sent_again() {
        use axum::http::{HeaderMap, StatusCode};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        let router = axum::Router::new().route(
            "/file",
            axum::routing::put(|headers: HeaderMap| async move {
                match headers["authorization"].to_str().unwrap() {
                    "Bearer fresh" => StatusCode::NO_CONTENT,
                    _ => StatusCode::UNAUTHORIZED,
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let http = reqwest::Client::new();
        let put = |token: &str| http.put(&url).bearer_auth(token);

        let mut token = "expired".to_string();
        let response = authorized(&mut token, || async { Ok("fresh".into()) }, put)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(token, "fresh", "the session keeps the fresh token");

        let renewed = std::cell::Cell::new(false);
        let response = authorized(
            &mut token,
            || async {
                renewed.set(true);
                Ok("other".into())
            },
            put,
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(!renewed.get(), "a token the service takes is not renewed");

        let mut token = "expired".to_string();
        let refused = authorized(&mut token, || async { Ok("still-wrong".into()) }, put)
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    }
}
