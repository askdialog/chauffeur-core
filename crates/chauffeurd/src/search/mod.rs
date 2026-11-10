//! Search of every workspace, active and archived (docs/adr/local-hybrid-search.md): keywords
//! through SQLite FTS5 and meaning through sqlite-vec, with vectors from a light local model
//! Ollama serves, both lists merged by Reciprocal Rank Fusion. Nothing leaves the machine.
//!
//! The index follows the event log as events are logged; a background task gives the history
//! its vectors in small batches, resting as long as each batch took, never starting Ollama on
//! battery, and with a short `keep_alive` so that the Ollama the daemon started still stops once
//! idle. Keywords work alone without Ollama, the model, or before the history is indexed.

mod embedding;
mod index;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use chauffeur_protocol::client::{
    ArchivedSearchHit, ArchivedSearchResult, PrInfo, SearchFilters, SearchHit, SearchHitKind,
    SearchMatchedBy, SearchParams, SearchResult, SearchSource, SearchState, SearchStatus,
    SemanticState,
};

use crate::{Daemon, Error, Result};
use embedding::Embedding;
use index::{Index, PullRequest};

/// A pass when nothing wakes the task sooner.
const PASS: Duration = Duration::from_secs(60);
/// Events logged in a burst make one pass.
const DEBOUNCE: Duration = Duration::from_secs(3);
/// How often the archived workspaces' notes are read again, and deletions reconciled.
const FULL_SYNC: Duration = Duration::from_secs(600);
/// Events read per transaction while catching up with the log.
const EVENTS_PER_PAGE: i64 = 5000;
/// Pieces of text embedded per request.
const BATCH: usize = 16;
/// Long enough for the next batch, short enough for the model to unload soon after the last.
const INDEXING_KEEP_ALIVE: &str = "1m";
/// The searches typed one after the other find the model loaded.
const QUERY_KEEP_ALIVE: &str = "5m";
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);
const BATCH_TIMEOUT: Duration = Duration::from_secs(120);
/// Reciprocal Rank Fusion's constant: a document's score is the sum of 1 / (K + its rank).
const RRF_K: f64 = 60.0;
/// Candidates taken from each list before fusing.
const CANDIDATES: usize = 100;
const MAX_HITS: usize = 50;
/// How often the battery is read.
const BATTERY_READ: Duration = Duration::from_secs(60);
/// Workspaces whose pull requests are looked for on GitHub per pass, the history once.
const LOOKUPS_PER_PASS: usize = 5;
/// After `gh` failed, the lookups wait this long.
const LOOKUP_BACKOFF: Duration = Duration::from_secs(1800);

pub struct Search {
    daemon: Weak<Daemon>,
    path: PathBuf,
    store: PathBuf,
    /// The background task's connection, and the searches' (WAL lets them run side by side).
    writer: Mutex<Option<Index>>,
    reader: Mutex<Option<Index>>,
    wake: tokio::sync::Notify,
    /// What the background task last found: ready, or waiting for the charger.
    state: Mutex<SemanticState>,
    /// Ollama is being started for a search.
    starting: AtomicBool,
    battery: Mutex<Option<(Instant, bool)>>,
    /// Tests say whether the machine is on battery.
    assumed_battery: Mutex<Option<bool>>,
    lookups_paused_until: Mutex<Option<Instant>>,
}

impl Search {
    pub fn new(daemon: Weak<Daemon>, paths: &crate::paths::Paths) -> Search {
        Search {
            daemon,
            path: paths.home.join("search.db"),
            store: paths.db.clone(),
            writer: Mutex::new(None),
            reader: Mutex::new(None),
            wake: tokio::sync::Notify::new(),
            state: Mutex::new(SemanticState::Ready),
            starting: AtomicBool::new(false),
            battery: Mutex::new(None),
            assumed_battery: Mutex::new(None),
            lookups_paused_until: Mutex::new(None),
        }
    }

    fn d(&self) -> Arc<Daemon> {
        self.daemon.upgrade().expect("daemon alive")
    }

    /// The background task looks again now: the settings or the index changed.
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// Tests run on battery or on the charger whatever the machine does.
    pub fn assume_battery(&self, on_battery: bool) {
        *self.assumed_battery.lock().unwrap() = Some(on_battery);
    }

    /// Runs `f` on one of the index's connections, opening it first. An index that does not
    /// open (a corrupt file) is removed and built again: everything in it comes from elsewhere.
    fn with_index<T>(
        &self,
        writer: bool,
        f: impl FnOnce(&mut Index) -> anyhow::Result<T>,
    ) -> Result<T> {
        let mut slot = if writer { &self.writer } else { &self.reader }
            .lock()
            .unwrap();
        if slot.is_none() {
            let index = Index::open(&self.path, &self.store).or_else(|error| {
                tracing::warn!(error = %format!("{error:#}"), "could not open the search index; building it again");
                for suffix in ["", "-wal", "-shm"] {
                    let mut file = self.path.as_os_str().to_owned();
                    file.push(suffix);
                    let _ = std::fs::remove_file(PathBuf::from(file));
                }
                Index::open(&self.path, &self.store)
            });
            *slot = Some(index.map_err(|error| {
                Error::Unavailable(format!("The search index could not be opened: {error:#}"))
            })?);
        }
        Ok(f(slot.as_mut().expect("opened above"))?)
    }

    /// `with_index` off the async runtime.
    async fn blocking<T: Send + 'static>(
        &self,
        writer: bool,
        f: impl FnOnce(&mut Index) -> anyhow::Result<T> + Send + 'static,
    ) -> Result<T> {
        let d = self.d();
        tokio::task::spawn_blocking(move || d.search.with_index(writer, f))
            .await
            .map_err(|error| Error::Internal(anyhow::anyhow!("search task failed: {error}")))?
    }

    /// The messages (event seq, chat) that ask for the same work in different chats, by meaning,
    /// as groups of seqs; none while the messages have no vectors (search by meaning off, or the
    /// history not indexed yet). Messages of the same chat are only grouped through another one.
    pub async fn similar_messages(&self, messages: Vec<(i64, String)>) -> Result<Vec<Vec<i64>>> {
        let model = self.d().settings.get().search.embedding_model.clone();
        self.blocking(false, move |index| {
            let seqs: Vec<i64> = messages.iter().map(|(seq, _)| *seq).collect();
            let vectors = index.message_vectors(&seqs, &model)?;
            Ok(group_similar(
                &messages,
                &vectors,
                Embedding::new(&model).same_request(),
            ))
        })
        .await
    }

    /// Keeps the pull requests a status read found, for the filters and their titles.
    pub fn record_pull_requests(&self, workspace: &str, prs: &[PrInfo]) {
        let prs: Vec<PullRequest> = prs
            .iter()
            .filter_map(|pr| {
                Some(PullRequest {
                    repo: pr.repo.clone(),
                    number: pr.number?,
                    url: pr.url.clone()?,
                    title: pr.title.clone().unwrap_or_default(),
                    state: pr.state.clone().unwrap_or_default(),
                })
            })
            .collect();
        let d = self.d();
        let workspace = workspace.to_string();
        tokio::spawn(async move {
            if let Err(error) = d.search.keep_pull_requests(&workspace, prs).await {
                tracing::warn!(%error, workspace, "could not keep the workspace's pull requests for search");
            }
        });
    }

    async fn keep_pull_requests(&self, workspace: &str, prs: Vec<PullRequest>) -> Result<()> {
        let workspace = workspace.to_string();
        let now = crate::store::now();
        self.blocking(true, move |index| {
            Ok(index.record_pull_requests(&workspace, &prs, &now)?)
        })
        .await
    }

    // --- the background task ---

    /// Whether search by meaning can run, as the settings and Ollama say now.
    fn availability(&self, d: &Daemon) -> SemanticState {
        let settings = d.settings.get();
        if !settings.search.semantic {
            SemanticState::Disabled
        } else if !d.ollama.installed() {
            SemanticState::NoOllama
        } else if !d.ollama.has_model(&settings.search.embedding_model) {
            SemanticState::NoModel
        } else {
            SemanticState::Ready
        }
    }

    async fn on_battery(&self) -> bool {
        if let Some(assumed) = *self.assumed_battery.lock().unwrap() {
            return assumed;
        }
        if let Some((at, on_battery)) = *self.battery.lock().unwrap()
            && at.elapsed() < BATTERY_READ
        {
            return on_battery;
        }
        let on_battery = tokio::task::spawn_blocking(|| {
            crate::power::read_battery(&crate::power::Tools::system())
                .is_some_and(|battery| battery.on_battery)
        })
        .await
        .unwrap_or(false);
        *self.battery.lock().unwrap() = Some((Instant::now(), on_battery));
        on_battery
    }

    /// One pass of the background task: the keywords up to date with the store (with `full`,
    /// the archived workspaces' notes and the deletions too), then the vectors.
    pub async fn pass(&self, full: bool) -> Result<()> {
        self.sync(full).await?;
        self.embed_pending(&self.d()).await;
        Ok(())
    }

    /// Brings the keywords up to date with the store: every new message, the workspaces' titles
    /// and the active ones' notes; with `full`, the archived ones' notes and what was deleted.
    async fn sync(&self, full: bool) -> Result<()> {
        while self
            .blocking(true, |index| Ok(index.sync_messages(EVENTS_PER_PAGE)?))
            .await?
        {
            // Catching up with a long history: leave the disk to others between pages.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.blocking(true, move |index| {
            index.sync_workspaces()?;
            index.sync_notes(full)?;
            if full {
                let removed = index.reconcile()?;
                if removed > 0 {
                    tracing::info!(removed, "search forgot deleted documents");
                }
            }
            Ok(())
        })
        .await
    }

    /// Gives the pieces of text waiting their vectors, the newest first, until none waits or it
    /// should stop: on battery, Ollama is not started and one batch at most goes to one running.
    async fn embed_pending(&self, d: &Arc<Daemon>) {
        let available = self.availability(d);
        if available != SemanticState::Ready {
            return;
        }
        let model = d.settings.get().search.embedding_model.clone();
        let embedding = Embedding::new(&model);
        let used = model.clone();
        if let Err(error) = self
            .blocking(true, move |index| Ok(index.use_model(&used)?))
            .await
        {
            tracing::warn!(%error, "could not prepare the search index for the embedding model");
            return;
        }
        let mut batches = 0;
        loop {
            if d.shutdown.is_cancelled() || d.settings.get().search.embedding_model != model {
                return;
            }
            let pending = match self.blocking(true, |index| Ok(index.pending(BATCH)?)).await {
                Ok(pending) => pending,
                Err(error) => {
                    tracing::warn!(%error, "could not read the text waiting for search vectors");
                    return;
                }
            };
            if pending.is_empty() {
                *self.state.lock().unwrap() = SemanticState::Ready;
                return;
            }
            let on_battery = self.on_battery().await;
            let running = d.ollama.answers().await;
            if on_battery && (!running || batches > 0) {
                *self.state.lock().unwrap() = SemanticState::OnBattery;
                return;
            }
            if !running && let Err(error) = d.ollama.ensure().await {
                tracing::warn!(%error, "could not start ollama for search");
                return;
            }
            *self.state.lock().unwrap() = SemanticState::Ready;
            let inputs: Vec<String> = pending
                .iter()
                .map(|chunk| embedding.document(&chunk.title, &chunk.text))
                .collect();
            let started = Instant::now();
            let vectors = match d
                .ollama
                .embed(&model, &inputs, INDEXING_KEEP_ALIVE, BATCH_TIMEOUT)
                .await
            {
                Ok(vectors) => vectors,
                Err(error) => {
                    tracing::warn!(%error, model, "could not embed text for search");
                    return;
                }
            };
            if let Err(error) = self
                .blocking(true, move |index| index.store_vectors(&pending, &vectors))
                .await
            {
                tracing::warn!(%error, "could not keep search vectors");
                return;
            }
            batches += 1;
            // Rest as long as the batch took: the machine stays at half its GPU at most.
            tokio::select! {
                _ = d.shutdown.cancelled() => return,
                _ = tokio::time::sleep(started.elapsed()) => {}
            }
        }
    }

    /// Looks for the pull requests of a few workspaces read by no status since this index
    /// exists (the history), on GitHub through `gh`.
    pub async fn look_up_pull_requests(&self, d: &Arc<Daemon>) {
        if self
            .lookups_paused_until
            .lock()
            .unwrap()
            .is_some_and(|until| Instant::now() < until)
        {
            return;
        }
        let git = crate::git::Git::new(d.shell_env.clone());
        for _ in 0..LOOKUPS_PER_PASS {
            let Ok(Some(workspace)) = self
                .blocking(false, |index| {
                    Ok(index.workspace_without_pull_request_lookup()?)
                })
                .await
            else {
                return;
            };
            let worktrees = d.store.worktrees_of(&workspace).unwrap_or_default();
            let mut found = Vec::new();
            let mut failed = false;
            // A clone removed since has no pull request to tell.
            for worktree in worktrees
                .iter()
                .filter(|worktree| std::path::Path::new(&worktree.clone_path).is_dir())
            {
                match crate::prs::find_pr_checked(
                    &d.store,
                    &git,
                    &d.shell_env,
                    worktree,
                    "all",
                    "number,title,state,url",
                )
                .await
                {
                    Ok(Some((pr, _))) => {
                        found.extend(pr["number"].as_u64().map(|number| PullRequest {
                            repo: worktree.repo.clone(),
                            number,
                            url: pr["url"].as_str().unwrap_or_default().to_string(),
                            title: pr["title"].as_str().unwrap_or_default().to_string(),
                            state: pr["state"].as_str().unwrap_or_default().to_string(),
                        }))
                    }
                    Ok(None) => {}
                    Err(()) => failed = true,
                }
            }
            // Kept even when `gh` failed: a workspace it always fails on (a repo deleted on
            // GitHub) must not hold up the others.
            if let Err(error) = self.keep_pull_requests(&workspace, found).await {
                tracing::warn!(%error, workspace, "could not keep the workspace's pull requests for search");
                return;
            }
            if failed {
                *self.lookups_paused_until.lock().unwrap() = Some(Instant::now() + LOOKUP_BACKOFF);
                return;
            }
        }
    }

    // --- searches ---

    pub async fn status(&self) -> Result<SearchStatus> {
        let d = self.d();
        let (total, embedded) = self.blocking(false, |index| Ok(index.counts()?)).await?;
        Ok(SearchStatus {
            semantic: self.semantic_state(&d),
            model: d.settings.get().search.embedding_model.clone(),
            embedded,
            total,
        })
    }

    /// Whether search by meaning runs now, as `status` says it.
    pub fn semantic(&self) -> SemanticState {
        self.semantic_state(&self.d())
    }

    fn semantic_state(&self, d: &Daemon) -> SemanticState {
        match self.availability(d) {
            SemanticState::Ready if self.starting.load(Ordering::SeqCst) => SemanticState::Starting,
            SemanticState::Ready => *self.state.lock().unwrap(),
            other => other,
        }
    }

    pub async fn query(&self, params: SearchParams) -> Result<SearchResult> {
        if params.query.len() > 512 {
            return Err(Error::BadRequest(
                "Search is too long; use fewer words".into(),
            ));
        }
        let d = self.d();
        let Some(expression) = fts_expression(&params.query) else {
            return Ok(SearchResult {
                hits: Vec::new(),
                status: self.status().await?,
            });
        };
        let filters = params.filters.clone();
        let keywords = {
            let filters = filters.clone();
            self.blocking(false, move |index| {
                Ok(index.keyword_hits(&expression, &filters, CANDIDATES)?)
            })
        };
        let (keywords, vector) = tokio::join!(keywords, self.query_vector(&d, &params.query));
        let keywords = keywords?;
        let meaning = match vector {
            Some((vector, max_distance)) => {
                let filters = filters.clone();
                self.blocking(false, move |index| {
                    Ok(index.vector_hits(&vector, &filters, max_distance, CANDIDATES)?)
                })
                .await?
            }
            None => Vec::new(),
        };
        let fused = fuse(&keywords, &meaning);
        let (views, fused, chunk_texts) = self
            .blocking(false, move |index| {
                let ids: Vec<i64> = fused.iter().map(|hit| hit.document).collect();
                let views = index.describe(&ids)?;
                let mut texts = HashMap::new();
                for hit in &fused {
                    if hit.snippet.is_none()
                        && let Some(chunk) = hit.chunk
                        && let Some(text) = index.chunk_text(chunk)?
                    {
                        texts.insert(hit.document, text);
                    }
                }
                Ok((views, fused, texts))
            })
            .await?;
        let mut hits: Vec<(SearchHit, f64, Option<String>)> = fused
            .into_iter()
            .filter_map(|hit| {
                let view = views.get(&hit.document)?;
                let snippet = hit
                    .snippet
                    .or_else(|| chunk_texts.get(&hit.document).map(|text| excerpt(text)))
                    .unwrap_or_default();
                Some((
                    SearchHit {
                        workspace_id: view.workspace_id.clone(),
                        kind: view.kind,
                        chat_id: view.chat_id.clone(),
                        chat_title: view.chat_title.clone(),
                        seq: view.seq,
                        path: view.path.clone(),
                        url: view.url.clone(),
                        title: (view.kind != SearchHitKind::Message).then(|| view.title.clone()),
                        snippet,
                        at: view.at.clone(),
                        matched: hit.matched,
                    },
                    hit.score,
                    view.archived_at.clone(),
                ))
            })
            .collect();
        hits.sort_by(|(a, a_score, _), (b, b_score, _)| {
            b_score.total_cmp(a_score).then_with(|| b.at.cmp(&a.at))
        });
        if params.filters.state == Some(SearchState::Archived) {
            // The newest archive first (docs/adr/0041-archive-search-order.md); relevance next.
            hits.sort_by(|(a, _, a_archived), (b, _, b_archived)| {
                b_archived
                    .is_some()
                    .cmp(&a_archived.is_some())
                    .then_with(|| b_archived.cmp(a_archived))
                    .then_with(|| a.workspace_id.cmp(&b.workspace_id))
            });
        }
        Ok(SearchResult {
            hits: hits
                .into_iter()
                .take(MAX_HITS)
                .map(|(hit, _, _)| hit)
                .collect(),
            status: self.status().await?,
        })
    }

    /// `workspace.search_archived` of older clients: archived conversations, by keywords.
    pub async fn search_archived(&self, query: String) -> Result<ArchivedSearchResult> {
        let found = self
            .query(SearchParams {
                query,
                filters: SearchFilters {
                    state: Some(SearchState::Archived),
                    sources: vec![SearchSource::Messages],
                    ..Default::default()
                },
            })
            .await?;
        Ok(ArchivedSearchResult {
            hits: found
                .hits
                .into_iter()
                .filter_map(|hit| {
                    Some(ArchivedSearchHit {
                        workspace_id: hit.workspace_id,
                        chat_id: hit.chat_id?,
                        chat_title: hit.chat_title,
                        seq: hit.seq?,
                        snippet: hit.snippet,
                    })
                })
                .collect(),
        })
    }

    /// The query's vector and the farthest a match may be; none when search by meaning cannot
    /// run now. An Ollama that does not run is started for the next search, not waited for.
    async fn query_vector(&self, d: &Arc<Daemon>, query: &str) -> Option<(Vec<f32>, f32)> {
        if self.availability(d) != SemanticState::Ready {
            return None;
        }
        if !d.ollama.answers().await {
            self.start_for_searches(d);
            return None;
        }
        let model = d.settings.get().search.embedding_model.clone();
        let embedding = Embedding::new(&model);
        match d
            .ollama
            .embed(
                &model,
                &[embedding.query(query)],
                QUERY_KEEP_ALIVE,
                QUERY_TIMEOUT,
            )
            .await
        {
            Ok(mut vectors) => vectors
                .pop()
                .map(|vector| (vector, embedding.max_distance())),
            Err(error) => {
                tracing::warn!(%error, model, "could not embed a search");
                None
            }
        }
    }

    fn start_for_searches(&self, d: &Arc<Daemon>) {
        if self.starting.swap(true, Ordering::SeqCst) {
            return;
        }
        let d = d.clone();
        tokio::spawn(async move {
            let model = d.settings.get().search.embedding_model.clone();
            let started = match d.ollama.ensure().await {
                // Loads the model too, so that the next search finds it ready.
                Ok(()) => d
                    .ollama
                    .embed(&model, &["search".into()], QUERY_KEEP_ALIVE, BATCH_TIMEOUT)
                    .await
                    .map(|_| ()),
                Err(error) => Err(error),
            };
            if let Err(error) = started {
                tracing::warn!(%error, "could not start ollama for searches");
            }
            d.search.starting.store(false, Ordering::SeqCst);
        });
    }
}

/// Runs until the daemon stops.
/// Groups the messages whose normalised vectors are at least `floor` alike, across chats; only
/// the groups spanning two chats or more, each sorted.
fn group_similar(
    messages: &[(i64, String)],
    vectors: &HashMap<i64, Vec<f32>>,
    floor: f32,
) -> Vec<Vec<i64>> {
    let known: Vec<(&i64, &String, &Vec<f32>)> = messages
        .iter()
        .filter_map(|(seq, chat)| vectors.get(seq).map(|v| (seq, chat, v)))
        .collect();
    let mut parent: Vec<usize> = (0..known.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for i in 0..known.len() {
        for j in i + 1..known.len() {
            if known[i].1 == known[j].1 {
                continue;
            }
            let similarity: f32 = known[i].2.iter().zip(known[j].2).map(|(a, b)| a * b).sum();
            if similarity >= floor {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..known.len() {
        let r = root(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut out: Vec<Vec<i64>> = groups
        .into_values()
        .filter(|members| {
            let first = known[members[0]].1;
            members.iter().any(|&m| known[m].1 != first)
        })
        .map(|members| {
            let mut seqs: Vec<i64> = members.iter().map(|&m| *known[m].0).collect();
            seqs.sort();
            seqs
        })
        .collect();
    out.sort();
    out
}

pub fn spawn(daemon: Arc<Daemon>) {
    let mut events = daemon.events.subscribe();
    tokio::spawn(async move {
        let mut last_full: Option<Instant> = None;
        loop {
            let full = last_full.is_none_or(|at| at.elapsed() >= FULL_SYNC);
            match daemon.search.pass(full).await {
                Ok(()) if full => last_full = Some(Instant::now()),
                Ok(()) => {}
                Err(error) => tracing::warn!(%error, "could not bring the search index up to date"),
            }
            daemon.search.look_up_pull_requests(&daemon).await;
            tokio::select! {
                _ = daemon.shutdown.cancelled() => return,
                _ = daemon.search.wake.notified() => last_full = None,
                _ = logged(&mut events) => {}
                _ = tokio::time::sleep(PASS) => {}
            }
            tokio::select! {
                _ = daemon.shutdown.cancelled() => return,
                _ = tokio::time::sleep(DEBOUNCE) => {}
            }
        }
    });
}

/// Returns once an event is logged (one kept in the store).
async fn logged(
    events: &mut tokio::sync::broadcast::Receiver<Arc<chauffeur_protocol::events::EventEnvelope>>,
) {
    loop {
        match events.recv().await {
            Ok(envelope) if envelope.seq.is_some() => return,
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => std::future::pending().await,
        }
    }
}

/// The user's words as an FTS5 expression: each word a prefix, each "quoted phrase" as written,
/// all of them required. Punctuation and FTS operators stay literal. None without a word.
fn fts_expression(query: &str) -> Option<String> {
    let mut parts = Vec::new();
    for (i, segment) in query.split('"').enumerate() {
        let words: Vec<&str> = segment
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect();
        if i % 2 == 1 {
            if !words.is_empty() {
                parts.push(format!("\"{}\"", words.join(" ")));
            }
        } else {
            parts.extend(words.iter().map(|word| format!("\"{word}\"*")));
        }
    }
    (!parts.is_empty()).then(|| parts.join(" AND "))
}

struct Fused {
    document: i64,
    score: f64,
    matched: SearchMatchedBy,
    /// The keywords' highlighted excerpt.
    snippet: Option<String>,
    /// The nearest chunk, for an excerpt when the keywords found nothing there.
    chunk: Option<i64>,
}

/// Reciprocal Rank Fusion of the keywords' documents (with their excerpt) and the meaning's
/// (with their nearest chunk), both the best first.
fn fuse(keywords: &[(i64, String)], meaning: &[(i64, i64)]) -> Vec<Fused> {
    let mut fused: Vec<Fused> = Vec::new();
    let mut at: HashMap<i64, usize> = HashMap::new();
    for (rank, (document, snippet)) in keywords.iter().enumerate() {
        at.insert(*document, fused.len());
        fused.push(Fused {
            document: *document,
            score: 1.0 / (RRF_K + rank as f64 + 1.0),
            matched: SearchMatchedBy::Keywords,
            snippet: Some(snippet.clone()),
            chunk: None,
        });
    }
    for (rank, (document, chunk)) in meaning.iter().enumerate() {
        let score = 1.0 / (RRF_K + rank as f64 + 1.0);
        match at.get(document) {
            Some(&i) => {
                fused[i].score += score;
                fused[i].matched = SearchMatchedBy::Both;
                fused[i].chunk = Some(*chunk);
            }
            None => fused.push(Fused {
                document: *document,
                score,
                matched: SearchMatchedBy::Meaning,
                snippet: None,
                chunk: Some(*chunk),
            }),
        }
    }
    fused
}

/// The start of a chunk on one line, for a hit found by meaning alone.
fn excerpt(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.char_indices().nth(240) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alike_messages_of_different_chats_group_and_a_chat_alone_does_not() {
        let messages: Vec<(i64, String)> =
            [(1, "a"), (2, "b"), (3, "a"), (4, "c"), (5, "d"), (6, "d")]
                .into_iter()
                .map(|(seq, chat)| (seq, chat.to_string()))
                .collect();
        let vectors: HashMap<i64, Vec<f32>> = [
            (1, vec![1.0, 0.0]),
            (2, vec![0.8, 0.6]),
            // Alike only through 2: still the same group.
            (3, vec![0.28, 0.96]),
            (4, vec![0.0, -1.0]),
            // Alike, but both of chat d: no group.
            (5, vec![-1.0, 0.0]),
            (6, vec![-1.0, 0.0]),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            group_similar(&messages, &vectors, 0.75),
            vec![vec![1, 2, 3]]
        );
        assert!(group_similar(&messages, &HashMap::new(), 0.75).is_empty());
    }

    #[test]
    fn words_match_by_prefix_quoted_phrases_as_written_and_operators_stay_literal() {
        assert_eq!(
            fts_expression("REPA shop").as_deref(),
            Some("\"REPA\"* AND \"shop\"*")
        );
        assert_eq!(
            fts_expression("bug \"tri des archives\" NOT").as_deref(),
            Some("\"bug\"* AND \"tri des archives\" AND \"NOT\"*")
        );
        assert_eq!(fts_expression("\" OR * ()").as_deref(), Some("\"OR\""));
        assert_eq!(fts_expression("  ** \"\" "), None);
        assert_eq!(
            fts_expression("\"unclosed phrase").as_deref(),
            Some("\"unclosed phrase\"")
        );
    }

    #[test]
    fn fusion_ranks_what_both_lists_found_first_and_says_how_each_was_found() {
        let keywords = vec![(1, "a".to_string()), (2, "b".to_string())];
        let meaning = vec![(3, 30), (2, 20)];
        let mut fused = fuse(&keywords, &meaning);
        fused.sort_by(|a, b| b.score.total_cmp(&a.score));
        let order: Vec<_> = fused
            .iter()
            .map(|hit| (hit.document, hit.matched))
            .collect();
        assert_eq!(
            order,
            [
                (2, SearchMatchedBy::Both),
                (1, SearchMatchedBy::Keywords),
                (3, SearchMatchedBy::Meaning),
            ]
        );
        assert_eq!(fused[0].snippet.as_deref(), Some("b"));
        assert_eq!(fused[2].chunk, Some(30));
    }

    #[test]
    fn an_excerpt_is_one_line_cut_after_240_characters() {
        assert_eq!(excerpt("a\n\n  b"), "a b");
        let long = excerpt(&"é ".repeat(200));
        assert_eq!(long.chars().count(), 241);
        assert!(long.ends_with('…'));
    }
}
