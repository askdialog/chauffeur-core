//! The search index, `search.db` in the state folder: documents (messages, notes, workspaces'
//! titles and tickets, pull requests), their FTS5 index, their chunks and the chunks' vectors
//! (sqlite-vec). It reads the store, attached read-only as `store`, and never writes it: the
//! filters read workspaces as they are now, and deleting the file rebuilds everything.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Context;
use chauffeur_protocol::client::{
    SearchFilters, SearchHitKind, SearchPr, SearchSource, SearchState,
};
use rusqlite::{Connection, OptionalExtension, named_params, params};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS documents (
    id INTEGER PRIMARY KEY,
    -- `m:<seq>`, `w:<workspace>`, `p:<workspace>:<repo>:<number>`, `n:<workspace>:<path>`.
    key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    chat_id TEXT,
    seq INTEGER,
    path TEXT,
    url TEXT,
    at TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    body TEXT NOT NULL,
    -- What the document was read from (a note's modification time and size): unchanged, skipped.
    stamp TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS documents_by_workspace ON documents(workspace_id, kind);
CREATE INDEX IF NOT EXISTS documents_by_seq ON documents(seq) WHERE seq IS NOT NULL;
CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(
    title, body, content = 'documents', content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY,
    document_id INTEGER NOT NULL,
    text TEXT NOT NULL,
    embedded INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS chunks_by_document ON chunks(document_id);
CREATE INDEX IF NOT EXISTS chunks_pending ON chunks(id) WHERE embedded = 0;
CREATE TRIGGER IF NOT EXISTS documents_insert AFTER INSERT ON documents BEGIN
    INSERT INTO documents_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
END;
CREATE TRIGGER IF NOT EXISTS documents_delete AFTER DELETE ON documents BEGIN
    INSERT INTO documents_fts(documents_fts, rowid, title, body) VALUES ('delete', old.id, old.title, old.body);
    DELETE FROM chunks WHERE document_id = old.id;
END;
CREATE TRIGGER IF NOT EXISTS documents_update AFTER UPDATE OF title, body ON documents BEGIN
    INSERT INTO documents_fts(documents_fts, rowid, title, body) VALUES ('delete', old.id, old.title, old.body);
    INSERT INTO documents_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
END;
-- The pull requests the daemon saw, for the "with a PR" and "merged" filters.
CREATE TABLE IF NOT EXISTS pull_requests (
    workspace_id TEXT NOT NULL,
    repo TEXT NOT NULL,
    number INTEGER NOT NULL,
    url TEXT NOT NULL,
    title TEXT NOT NULL,
    state TEXT NOT NULL,
    PRIMARY KEY (workspace_id, repo, number)
);
-- Workspaces whose pull requests were looked for once on GitHub.
CREATE TABLE IF NOT EXISTS pull_request_lookups (
    workspace_id TEXT PRIMARY KEY
);
"#;

/// About 300 tokens: short enough for a precise meaning, long enough for its context.
const CHUNK_CHARS: usize = 1200;
const CHUNK_OVERLAP: usize = 200;
/// A long paste does not monopolise the indexing.
const MAX_CHUNKS: usize = 20;
/// Larger markdown files are skipped: logs or dumps, not notes.
const MAX_NOTE_BYTES: u64 = 512 * 1024;

/// What a path cannot hold as is in an SQLite URI.
const URI_PATH: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'%')
    .add(b'?')
    .add(b'#');

/// Registers sqlite-vec with every connection SQLite opens from now on.
fn register_vec() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: sqlite-vec's entry point is an SQLite extension's init function, the signature
        // `sqlite3_auto_extension` expects; it is linked in statically with SQLite itself.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut std::os::raw::c_char,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> std::os::raw::c_int,
            >(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

pub struct Index {
    conn: Connection,
}

/// A message, as the event log holds it.
pub struct Message {
    pub seq: i64,
    pub at: String,
    pub workspace_id: String,
    pub chat_id: String,
    pub text: String,
}

/// A piece of a document waiting for its vector.
pub struct PendingChunk {
    pub id: i64,
    pub text: String,
    pub title: String,
    pub workspace_id: String,
    pub kind: String,
    pub at: String,
}

/// A document of a hit, as the app shows it.
pub struct DocumentView {
    pub kind: SearchHitKind,
    pub workspace_id: String,
    pub chat_id: Option<String>,
    pub chat_title: Option<String>,
    pub seq: Option<i64>,
    pub path: Option<String>,
    pub url: Option<String>,
    pub title: String,
    pub at: String,
    pub archived_at: Option<String>,
}

/// A pull request as the daemon read it.
pub struct PullRequest {
    pub repo: String,
    pub number: u64,
    pub url: String,
    pub title: String,
    pub state: String,
}

impl Index {
    /// Opens (creating) the index at `path`, with the store at `store` attached read-only.
    pub fn open(path: &Path, store: &Path) -> anyhow::Result<Index> {
        register_vec();
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        // Every chat's messages are in there too: the user's alone.
        for suffix in ["", "-wal", "-shm"] {
            let mut file = path.as_os_str().to_owned();
            file.push(suffix);
            crate::paths::private_file(Path::new(&file))?;
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        let uri = format!(
            "file:{}?mode=ro",
            percent_encoding::utf8_percent_encode(&store.to_string_lossy(), URI_PATH)
        );
        conn.execute("ATTACH DATABASE ?1 AS store", [uri])
            .context("attaching the store")?;
        Ok(Index { conn })
    }

    pub fn meta(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
    }

    fn set_meta(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // --- documents ---

    /// Indexes the messages logged since the last call, at most `limit` events read; returns
    /// whether the log has more.
    pub fn sync_messages(&mut self, limit: i64) -> rusqlite::Result<bool> {
        let cursor: i64 = self
            .meta("messages_seq")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let last: i64 =
            self.conn
                .query_row("SELECT coalesce(max(seq), 0) FROM store.events", [], |r| {
                    r.get(0)
                })?;
        if last <= cursor {
            return Ok(false);
        }
        let until = last.min(cursor + limit);
        let messages = {
            let mut stmt = self.conn.prepare(
                "SELECT seq, ts, workspace_id, chat_id, json_extract(payload, '$.event.event.text')
                 FROM store.events
                 WHERE seq > ?1 AND seq <= ?2 AND workspace_id IS NOT NULL AND chat_id IS NOT NULL
                   AND CASE WHEN json_valid(payload) THEN
                       json_extract(payload, '$.event.scope') = 'chat'
                       AND json_extract(payload, '$.event.event.type') IN ('user_message', 'assistant_text')
                       AND json_extract(payload, '$.event.event.parent_tool_use_id') IS NULL
                       ELSE 0 END",
            )?;
            stmt.query_map([cursor, until], |r| {
                Ok(Message {
                    seq: r.get(0)?,
                    at: r.get(1)?,
                    workspace_id: r.get(2)?,
                    chat_id: r.get(3)?,
                    text: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let tx = self.conn.transaction()?;
        for message in messages.iter().filter(|m| !m.text.trim().is_empty()) {
            insert_document(
                &tx,
                &Document {
                    key: format!("m:{}", message.seq),
                    kind: "message",
                    workspace_id: &message.workspace_id,
                    chat_id: Some(&message.chat_id),
                    seq: Some(message.seq),
                    path: None,
                    url: None,
                    at: &message.at,
                    title: "",
                    body: &message.text,
                    stamp: "",
                },
            )?;
        }
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('messages_seq', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [until.to_string()],
        )?;
        tx.commit()?;
        Ok(until < last)
    }

    /// Indexes every workspace's name, slug, branch and tickets, as they are now.
    pub fn sync_workspaces(&mut self) -> rusqlite::Result<()> {
        let rows = {
            let mut stmt = self.conn.prepare(
                "SELECT id, slug, title, branch, tickets, created_at FROM store.workspaces",
            )?;
            stmt.query_map([], |r| {
                let slug: String = r.get(1)?;
                let title = r
                    .get::<_, Option<String>>(2)?
                    .unwrap_or_else(|| slug.clone());
                let tickets: Vec<chauffeur_protocol::model::TicketRef> =
                    serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default();
                let mut body = vec![slug, r.get::<_, String>(3)?];
                body.extend(tickets.into_iter().map(|t| match t.title {
                    Some(title) => format!("{} {title}", t.identifier),
                    None => t.identifier,
                }));
                Ok((
                    r.get::<_, String>(0)?,
                    title,
                    body.join("\n"),
                    r.get::<_, String>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let tx = self.conn.transaction()?;
        for (id, title, body, created_at) in &rows {
            upsert_document(
                &tx,
                &Document {
                    key: format!("w:{id}"),
                    kind: "workspace",
                    workspace_id: id,
                    chat_id: None,
                    seq: None,
                    path: None,
                    url: None,
                    at: created_at,
                    title,
                    body,
                    stamp: "",
                },
            )?;
        }
        tx.commit()
    }

    /// Indexes the markdown files that changed in the notes folder of each workspace (with
    /// `archived`, of the archived ones too), and forgets the removed ones. A folder that is not
    /// there (a workspace on another machine) is left as is.
    pub fn sync_notes(&mut self, archived: bool) -> rusqlite::Result<()> {
        let workspaces = {
            let mut stmt = self.conn.prepare(
                "SELECT id, path FROM store.workspaces
                 WHERE ?1 OR state NOT IN ('archived', 'archiving')",
            )?;
            stmt.query_map([archived], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (workspace, root) in workspaces {
            let root = Path::new(&root);
            let notes = root.join(crate::paths::NOTES_DIR);
            if !notes.is_dir() {
                continue;
            }
            let known: HashMap<String, String> = {
                let mut stmt = self.conn.prepare(
                    "SELECT key, stamp FROM documents WHERE workspace_id = ?1 AND kind = 'note'",
                )?;
                stmt.query_map([&workspace], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let mut seen = HashSet::new();
            let tx = self.conn.transaction()?;
            for (path, size, modified) in note_files(&notes) {
                let Ok(relative) = path.strip_prefix(root) else {
                    continue;
                };
                let relative = relative.to_string_lossy().into_owned();
                let key = format!("n:{workspace}:{relative}");
                let stamp = format!("{}:{size}", modified.timestamp_millis());
                seen.insert(key.clone());
                if known.get(&key) == Some(&stamp) {
                    continue;
                }
                let Ok(body) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let at = modified.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                let title = path.file_name().map(|n| n.to_string_lossy().into_owned());
                upsert_document(
                    &tx,
                    &Document {
                        key,
                        kind: "note",
                        workspace_id: &workspace,
                        chat_id: None,
                        seq: None,
                        path: Some(&relative),
                        url: None,
                        at: &at,
                        title: title.as_deref().unwrap_or_default(),
                        body: &body,
                        stamp: &stamp,
                    },
                )?;
            }
            for key in known.keys().filter(|key| !seen.contains(*key)) {
                tx.execute("DELETE FROM documents WHERE key = ?1", [key])?;
            }
            tx.commit()?;
        }
        Ok(())
    }

    /// Keeps a workspace's pull requests as the daemon just read them; none read is no news.
    pub fn record_pull_requests(
        &mut self,
        workspace: &str,
        prs: &[PullRequest],
        now: &str,
    ) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        for pr in prs {
            tx.execute(
                "INSERT INTO pull_requests(workspace_id, repo, number, url, title, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(workspace_id, repo, number)
                 DO UPDATE SET url = excluded.url, title = excluded.title, state = excluded.state",
                params![
                    workspace,
                    pr.repo,
                    pr.number as i64,
                    pr.url,
                    pr.title,
                    pr.state
                ],
            )?;
            let key = format!("p:{workspace}:{}:{}", pr.repo, pr.number);
            let at: Option<String> = tx
                .query_row("SELECT at FROM documents WHERE key = ?1", [&key], |r| {
                    r.get(0)
                })
                .optional()?;
            upsert_document(
                &tx,
                &Document {
                    key: key.clone(),
                    kind: "pull_request",
                    workspace_id: workspace,
                    chat_id: None,
                    seq: None,
                    path: None,
                    url: Some(&pr.url),
                    at: at.as_deref().unwrap_or(now),
                    title: &pr.title,
                    body: &format!("{}#{}", pr.repo, pr.number),
                    stamp: "",
                },
            )?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO pull_request_lookups(workspace_id) VALUES (?1)",
            [workspace],
        )?;
        tx.commit()
    }

    /// A workspace whose pull requests were never looked for, if any.
    pub fn workspace_without_pull_request_lookup(&self) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT id FROM store.workspaces
                 WHERE id NOT IN (SELECT workspace_id FROM pull_request_lookups)
                 ORDER BY created_at DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
    }

    /// Forgets what the store no longer has: deleted workspaces, chats and events.
    pub fn reconcile(&mut self) -> rusqlite::Result<usize> {
        let tx = self.conn.transaction()?;
        let mut removed = tx.execute(
            "DELETE FROM documents WHERE workspace_id NOT IN (SELECT id FROM store.workspaces)",
            [],
        )?;
        removed += tx.execute(
            "DELETE FROM documents WHERE kind = 'message'
             AND NOT EXISTS (SELECT 1 FROM store.events e WHERE e.seq = documents.seq)",
            [],
        )?;
        tx.execute(
            "DELETE FROM pull_requests WHERE workspace_id NOT IN (SELECT id FROM store.workspaces)",
            [],
        )?;
        tx.commit()?;
        Ok(removed)
    }

    // --- vectors ---

    /// Pieces of text in all, and those given a vector.
    pub fn counts(&self) -> rusqlite::Result<(u64, u64)> {
        self.conn.query_row(
            "SELECT count(*), coalesce(sum(embedded), 0) FROM chunks",
            [],
            |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
        )
    }

    /// Drops the vectors of another model than `model`: they do not compare with its own.
    pub fn use_model(&self, model: &str) -> rusqlite::Result<()> {
        if self.meta("model")?.as_deref() == Some(model) {
            return Ok(());
        }
        self.conn.execute_batch(
            "BEGIN;
             DROP TRIGGER IF EXISTS chunks_delete;
             DROP TABLE IF EXISTS chunk_vectors;
             UPDATE chunks SET embedded = 0 WHERE embedded = 1;
             DELETE FROM meta WHERE key = 'dimensions';
             COMMIT;",
        )?;
        self.set_meta("model", model)
    }

    /// The newest pieces of text without a vector, the history after them.
    pub fn pending(&self, limit: usize) -> rusqlite::Result<Vec<PendingChunk>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.text, d.title, d.workspace_id, d.kind, d.at
             FROM chunks c JOIN documents d ON d.id = c.document_id
             WHERE c.embedded = 0 ORDER BY c.id DESC LIMIT ?1",
        )?;
        stmt.query_map([limit as i64], |r| {
            Ok(PendingChunk {
                id: r.get(0)?,
                text: r.get(1)?,
                title: r.get(2)?,
                workspace_id: r.get(3)?,
                kind: r.get(4)?,
                at: r.get(5)?,
            })
        })?
        .collect()
    }

    /// Keeps the vectors of `chunks`; a chunk removed meanwhile is skipped.
    pub fn store_vectors(
        &mut self,
        chunks: &[PendingChunk],
        vectors: &[Vec<f32>],
    ) -> anyhow::Result<()> {
        let Some(dimensions) = vectors.first().map(Vec::len) else {
            return Ok(());
        };
        anyhow::ensure!(dimensions > 0, "the embedding model gave empty vectors");
        match self
            .meta("dimensions")?
            .and_then(|d| d.parse::<usize>().ok())
        {
            Some(known) => anyhow::ensure!(
                known == dimensions,
                "the embedding model gave {dimensions} dimensions where the index has {known}"
            ),
            None => {
                self.conn.execute_batch(&format!(
                    "CREATE VIRTUAL TABLE IF NOT EXISTS chunk_vectors USING vec0(
                         embedding float[{dimensions}] distance_metric=cosine,
                         workspace_id text, kind text, at text
                     );
                     CREATE TRIGGER IF NOT EXISTS chunks_delete AFTER DELETE ON chunks BEGIN
                         DELETE FROM chunk_vectors WHERE rowid = old.id;
                     END;"
                ))?;
                self.set_meta("dimensions", &dimensions.to_string())?;
            }
        }
        let tx = self.conn.transaction()?;
        for (chunk, vector) in chunks.iter().zip(vectors) {
            anyhow::ensure!(vector.len() == dimensions, "vectors of different sizes");
            if tx.execute(
                "UPDATE chunks SET embedded = 1 WHERE id = ?1 AND embedded = 0",
                [chunk.id],
            )? == 0
            {
                continue;
            }
            tx.execute(
                "INSERT INTO chunk_vectors(rowid, embedding, workspace_id, kind, at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![chunk.id, vector_bytes(vector), chunk.workspace_id, chunk.kind, chunk.at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    // --- queries ---

    /// The documents matching an FTS5 expression, the best first, with a highlighted excerpt.
    pub fn keyword_hits(
        &self,
        expression: &str,
        filters: &SearchFilters,
        limit: usize,
    ) -> rusqlite::Result<Vec<(i64, String)>> {
        let sql = format!(
            "SELECT d.id, snippet(documents_fts, -1, char(1), char(2), '…', 24)
             FROM documents_fts
             JOIN documents d ON d.id = documents_fts.rowid
             JOIN store.workspaces w ON w.id = d.workspace_id
             WHERE documents_fts MATCH :expression AND {FILTERS}
             ORDER BY bm25(documents_fts, 2.0, 1.0) LIMIT :limit"
        );
        let filter = FilterParams::new(filters);
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.query_map(
            named_params! {
                ":expression": expression,
                ":limit": limit as i64,
                ":kinds": filter.kinds,
                ":since": filters.since,
                ":until": filters.until,
                ":state": filter.state,
                ":repos": filter.repos,
                ":tickets": filter.tickets,
                ":pr": filter.pr,
            },
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .collect()
    }

    /// The documents nearest `vector`, the nearest first, each by its nearest chunk, no farther
    /// than `max_distance`; none before the first vector is stored.
    pub fn vector_hits(
        &self,
        vector: &[f32],
        filters: &SearchFilters,
        max_distance: f32,
        limit: usize,
    ) -> rusqlite::Result<Vec<(i64, i64)>> {
        if self.meta("dimensions")?.is_none() {
            return Ok(Vec::new());
        }
        let filter = FilterParams::new(filters);
        // The workspaces the filters keep go into the nearest-neighbour search itself: a
        // narrow filter would otherwise find nothing among the nearest chunks of all.
        let workspaces: Option<String> = if filter.narrows_workspaces() {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT json_group_array(w.id) FROM store.workspaces w WHERE {WORKSPACE_FILTERS}"
            ))?;
            Some(stmt.query_row(
                named_params! {
                    ":state": filter.state,
                    ":repos": filter.repos,
                    ":tickets": filter.tickets,
                    ":pr": filter.pr,
                },
                |r| r.get(0),
            )?)
        } else {
            None
        };
        let mut sql = String::from(
            "SELECT rowid, distance FROM chunk_vectors WHERE embedding MATCH :vector AND k = :k",
        );
        if workspaces.is_some() {
            sql.push_str(" AND workspace_id IN (SELECT value FROM json_each(:workspaces))");
        }
        if filter.kinds.is_some() {
            sql.push_str(" AND kind IN (SELECT value FROM json_each(:kinds))");
        }
        if filters.since.is_some() {
            sql.push_str(" AND at >= :since");
        }
        if filters.until.is_some() {
            sql.push_str(" AND at < :until");
        }
        let nearest: Vec<(i64, f32)> = {
            let mut stmt = self.conn.prepare(&sql)?;
            let bytes = vector_bytes(vector);
            let mut values: Vec<(&str, &dyn rusqlite::ToSql)> =
                vec![(":vector", &bytes), (":k", &VECTOR_CANDIDATES)];
            if let Some(workspaces) = &workspaces {
                values.push((":workspaces", workspaces));
            }
            if let Some(kinds) = &filter.kinds {
                values.push((":kinds", kinds));
            }
            if let Some(since) = &filters.since {
                values.push((":since", since));
            }
            if let Some(until) = &filters.until {
                values.push((":until", until));
            }
            stmt.query_map(values.as_slice(), |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let chunks: Vec<i64> = nearest
            .iter()
            .filter(|(_, distance)| *distance <= max_distance)
            .map(|(chunk, _)| *chunk)
            .collect();
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        // Each document once, by its nearest chunk, if the filters keep it as it is now (a
        // chat deleted since, say).
        let sql = format!(
            "SELECT d.id, c.id FROM json_each(:chunks) AS j
             JOIN chunks c ON c.id = j.value
             JOIN documents d ON d.id = c.document_id
             JOIN store.workspaces w ON w.id = d.workspace_id
             WHERE {FILTERS} ORDER BY j.key"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            named_params! {
                ":chunks": serde_json::to_string(&chunks).expect("ids serialize"),
                ":kinds": filter.kinds,
                ":since": filters.since,
                ":until": filters.until,
                ":state": filter.state,
                ":repos": filter.repos,
                ":tickets": filter.tickets,
                ":pr": filter.pr,
            },
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )?;
        let mut seen = HashSet::new();
        let mut hits = Vec::new();
        for row in rows {
            let (document, chunk) = row?;
            if seen.insert(document) {
                hits.push((document, chunk));
            }
            if hits.len() == limit {
                break;
            }
        }
        Ok(hits)
    }

    /// The vector of each message logged as event `seqs`, from its first piece, normalised; none
    /// for a message without one yet, and none at all for a model other than `model`.
    pub fn message_vectors(
        &self,
        seqs: &[i64],
        model: &str,
    ) -> rusqlite::Result<HashMap<i64, Vec<f32>>> {
        if self.meta("dimensions")?.is_none() || self.meta("model")?.as_deref() != Some(model) {
            return Ok(HashMap::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT v.embedding FROM documents d
             JOIN chunks c ON c.id = (SELECT min(id) FROM chunks WHERE document_id = d.id)
             JOIN chunk_vectors v ON v.rowid = c.id
             WHERE d.key = ?1",
        )?;
        let mut vectors = HashMap::new();
        for seq in seqs {
            let Some(bytes) = stmt
                .query_row([format!("m:{seq}")], |r| r.get::<_, Vec<u8>>(0))
                .optional()?
            else {
                continue;
            };
            let vector: Vec<f32> = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect();
            let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                vectors.insert(*seq, vector.iter().map(|x| x / norm).collect());
            }
        }
        Ok(vectors)
    }

    pub fn chunk_text(&self, chunk: i64) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row("SELECT text FROM chunks WHERE id = ?1", [chunk], |r| {
                r.get(0)
            })
            .optional()
    }

    pub fn describe(&self, documents: &[i64]) -> rusqlite::Result<HashMap<i64, DocumentView>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.id, d.kind, d.workspace_id, d.chat_id, c.title, d.seq, d.path, d.url,
                    d.title, d.at, w.archived_at
             FROM json_each(?1) AS j
             JOIN documents d ON d.id = j.value
             JOIN store.workspaces w ON w.id = d.workspace_id
             LEFT JOIN store.chats c ON c.id = d.chat_id",
        )?;
        stmt.query_map(
            [serde_json::to_string(documents).expect("ids serialize")],
            |r| {
                let kind = match r.get::<_, String>(1)?.as_str() {
                    "message" => SearchHitKind::Message,
                    "note" => SearchHitKind::Note,
                    "pull_request" => SearchHitKind::PullRequest,
                    _ => SearchHitKind::Workspace,
                };
                Ok((
                    r.get(0)?,
                    DocumentView {
                        kind,
                        workspace_id: r.get(2)?,
                        chat_id: r.get(3)?,
                        chat_title: r.get(4)?,
                        seq: r.get(5)?,
                        path: r.get(6)?,
                        url: r.get(7)?,
                        title: r.get(8)?,
                        at: r.get(9)?,
                        archived_at: r.get(10)?,
                    },
                ))
            },
        )?
        .collect()
    }

    #[cfg(test)]
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }
}

/// How many chunks the nearest-neighbour search returns before they are grouped by document.
const VECTOR_CANDIDATES: i64 = 200;

/// The filters on a document `d` of workspace `w`, as named parameters (NULL: no filter).
const FILTERS: &str = "(:kinds IS NULL OR d.kind IN (SELECT value FROM json_each(:kinds)))
    AND (:since IS NULL OR d.at >= :since)
    AND (:until IS NULL OR d.at < :until)
    AND (d.kind != 'message' OR EXISTS (SELECT 1 FROM store.chats c WHERE c.id = d.chat_id))
    AND (:state IS NULL OR (w.state IN ('archived', 'archiving')) = (:state = 'archived'))
    AND (:repos IS NULL OR EXISTS (SELECT 1 FROM store.worktrees t
         WHERE t.workspace_id = w.id AND t.repo IN (SELECT value FROM json_each(:repos))))
    AND (:tickets IS NULL OR EXISTS (SELECT 1 FROM json_each(w.tickets) t
         WHERE upper(json_extract(t.value, '$.identifier')) IN (SELECT upper(value) FROM json_each(:tickets))))
    AND (:pr IS NULL OR EXISTS (SELECT 1 FROM pull_requests p
         WHERE p.workspace_id = w.id AND (:pr = 'any' OR p.state = 'MERGED')))";

/// The filters on a workspace `w` alone.
const WORKSPACE_FILTERS: &str = "(:state IS NULL OR (w.state IN ('archived', 'archiving')) = (:state = 'archived'))
    AND (:repos IS NULL OR EXISTS (SELECT 1 FROM store.worktrees t
         WHERE t.workspace_id = w.id AND t.repo IN (SELECT value FROM json_each(:repos))))
    AND (:tickets IS NULL OR EXISTS (SELECT 1 FROM json_each(w.tickets) t
         WHERE upper(json_extract(t.value, '$.identifier')) IN (SELECT upper(value) FROM json_each(:tickets))))
    AND (:pr IS NULL OR EXISTS (SELECT 1 FROM pull_requests p
         WHERE p.workspace_id = w.id AND (:pr = 'any' OR p.state = 'MERGED')))";

/// The filters as SQL values: JSON lists, NULL when a filter is left out.
struct FilterParams {
    kinds: Option<String>,
    state: Option<&'static str>,
    repos: Option<String>,
    tickets: Option<String>,
    pr: Option<&'static str>,
}

impl FilterParams {
    fn new(filters: &SearchFilters) -> FilterParams {
        let list = |values: &[String]| {
            (!values.is_empty()).then(|| serde_json::to_string(values).expect("strings serialize"))
        };
        let kinds: Vec<&str> = filters
            .sources
            .iter()
            .flat_map(|source| match source {
                SearchSource::Titles => &["workspace", "pull_request"][..],
                SearchSource::Messages => &["message"][..],
                SearchSource::Notes => &["note"][..],
            })
            .copied()
            .collect();
        FilterParams {
            kinds: (!kinds.is_empty())
                .then(|| serde_json::to_string(&kinds).expect("strings serialize")),
            state: filters.state.map(|state| match state {
                SearchState::Active => "active",
                SearchState::Archived => "archived",
            }),
            repos: list(&filters.repos),
            tickets: list(&filters.tickets),
            pr: filters.pr.map(|pr| match pr {
                SearchPr::Any => "any",
                SearchPr::Merged => "merged",
            }),
        }
    }

    fn narrows_workspaces(&self) -> bool {
        self.state.is_some() || self.repos.is_some() || self.tickets.is_some() || self.pr.is_some()
    }
}

struct Document<'a> {
    key: String,
    kind: &'a str,
    workspace_id: &'a str,
    chat_id: Option<&'a str>,
    seq: Option<i64>,
    path: Option<&'a str>,
    url: Option<&'a str>,
    at: &'a str,
    title: &'a str,
    body: &'a str,
    stamp: &'a str,
}

/// Inserts a document and its chunks; one already there (by key) is left as it is.
fn insert_document(conn: &Connection, doc: &Document) -> rusqlite::Result<()> {
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO documents(key, kind, workspace_id, chat_id, seq, path, url, at, title, body, stamp)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![doc.key, doc.kind, doc.workspace_id, doc.chat_id, doc.seq, doc.path, doc.url, doc.at, doc.title, doc.body, doc.stamp],
    )?;
    if inserted == 1 {
        insert_chunks(conn, conn.last_insert_rowid(), doc.body)?;
    }
    Ok(())
}

/// Inserts a document, or brings the one of its key up to date; its chunks are cut again when
/// its text changed.
fn upsert_document(conn: &Connection, doc: &Document) -> rusqlite::Result<()> {
    let existing: Option<(i64, String, String, String)> = conn
        .query_row(
            "SELECT id, title, body, stamp FROM documents WHERE key = ?1",
            [&doc.key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((id, title, body, stamp)) = existing else {
        return insert_document(conn, doc);
    };
    if title == doc.title && body == doc.body && stamp == doc.stamp {
        return Ok(());
    }
    conn.execute(
        "UPDATE documents SET title = ?2, body = ?3, stamp = ?4, at = ?5, url = ?6 WHERE id = ?1",
        params![id, doc.title, doc.body, doc.stamp, doc.at, doc.url],
    )?;
    if body != doc.body {
        conn.execute("DELETE FROM chunks WHERE document_id = ?1", [id])?;
        insert_chunks(conn, id, doc.body)?;
    }
    Ok(())
}

fn insert_chunks(conn: &Connection, document: i64, body: &str) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare_cached("INSERT INTO chunks(document_id, text) VALUES (?1, ?2)")?;
    for chunk in chunks(body) {
        stmt.execute(params![document, chunk])?;
    }
    Ok(())
}

/// Cuts a text into pieces of about `CHUNK_CHARS` characters that overlap by about
/// `CHUNK_OVERLAP`, at a paragraph, a line, a sentence or a word when one is near; at most
/// `MAX_CHUNKS`, from the start.
pub fn chunks(text: &str) -> Vec<String> {
    let text = text.trim();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let byte = |i: usize| chars.get(i).map_or(text.len(), |(b, _)| *b);
    let mut pieces = Vec::new();
    let mut start = 0;
    while start < chars.len() && pieces.len() < MAX_CHUNKS {
        let mut end = (start + CHUNK_CHARS).min(chars.len());
        if end < chars.len() {
            // Back to the best break in the last quarter of the window.
            let window = &text[byte(start + CHUNK_CHARS * 3 / 4)..byte(end)];
            let cut = ["\n\n", "\n", ". ", " "]
                .iter()
                .find_map(|sep| window.rfind(sep).map(|at| at + sep.len()));
            if let Some(cut) = cut {
                let cut_byte = byte(start + CHUNK_CHARS * 3 / 4) + cut;
                end = chars.partition_point(|(b, _)| *b < cut_byte);
            }
        }
        let piece = text[byte(start)..byte(end)].trim();
        if !piece.is_empty() {
            pieces.push(piece.to_string());
        }
        if end >= chars.len() {
            break;
        }
        // The next piece starts a little before, at a word.
        let mut next = end.saturating_sub(CHUNK_OVERLAP).max(start + 1);
        while next < end && !chars[next - 1].1.is_whitespace() {
            next += 1;
        }
        start = next;
    }
    pieces
}

fn vector_bytes(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// The markdown files of a notes folder: path, size, modification time. Hidden folders and
/// `node_modules` are skipped, and files over `MAX_NOTE_BYTES`.
fn note_files(notes: &Path) -> Vec<(std::path::PathBuf, u64, chrono::DateTime<chrono::Utc>)> {
    walkdir::WalkDir::new(notes)
        .max_depth(6)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !(entry.file_type().is_dir()
                    && (entry.file_name().to_string_lossy().starts_with('.')
                        || entry.file_name() == "node_modules"))
        })
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        })
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            if meta.len() > MAX_NOTE_BYTES {
                return None;
            }
            let modified = meta.modified().ok()?;
            Some((entry.path().to_path_buf(), meta.len(), modified.into()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use chauffeur_protocol::client::SearchFilters;
    use serde_json::json;

    struct Fixture {
        dir: tempfile::TempDir,
        store: Store,
        index: Index,
    }

    /// A store with an archived workspace `w` (chat `c`, repo `api`, ticket XYZ-1) and an active
    /// one `a` (chat `ac`, repo `web`), and its index.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("chauffeur.db")).unwrap();
        let root = dir.path().display();
        store
            .with(|c| {
                c.execute_batch(&format!(
                    "INSERT INTO workspaces(id,slug,path,branch,state,created_at,archived_at,tickets,title)
                     VALUES ('w','w-slug','{root}/w','me/w','archived','2026-10-01T00:00:00.000Z','2026-10-02T00:00:00.000Z',
                             '[{{\"identifier\":\"XYZ-1\",\"title\":\"Facet filters\"}}]','Facettes du catalogue');
                     INSERT INTO workspaces(id,slug,path,branch,state,created_at)
                     VALUES ('a','a-slug','{root}/a','me/a','ready','2026-11-01T00:00:00.000Z');
                     INSERT INTO chats(id,workspace_id,agent,permission_mode,status,created_at,closed,title)
                     VALUES ('c','w','claude','auto','idle','t',1,'Closed chat'),
                            ('ac','a','claude','auto','idle','t',0,'Open chat');
                     INSERT INTO worktrees(id,workspace_id,repo,clone_path,path,branch,base_branch,origin,created_at)
                     VALUES ('t1','w','api','/c/api','{root}/w/api','me/w','main','workspace','t'),
                            ('t2','a','web','/c/web','{root}/a/web','me/a','main','workspace','t');"
                ))
            })
            .unwrap();
        let index = Index::open(
            &dir.path().join("search.db"),
            &dir.path().join("chauffeur.db"),
        )
        .unwrap();
        Fixture { dir, store, index }
    }

    fn message(store: &Store, chat: &str, kind: &str, text: &str, parent: Option<&str>) -> i64 {
        let workspace = if chat == "c" { "w" } else { "a" };
        store
            .append_event(
                "2026-10-01T12:00:00.000Z",
                Some(workspace),
                Some(chat),
                false,
                &json!({"event": {
                    "scope": "chat", "workspace_id": workspace, "chat_id": chat, "event": {
                        "type": kind, "text": text, "parent_tool_use_id": parent
                    }
                }})
                .to_string(),
            )
            .unwrap()
    }

    fn keywords(index: &Index, words: &str, filters: &SearchFilters) -> Vec<(String, String)> {
        let expression = words
            .split_whitespace()
            .map(|w| format!("\"{w}\"*"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let hits = index.keyword_hits(&expression, filters, 50).unwrap();
        let ids: Vec<i64> = hits.iter().map(|(id, _)| *id).collect();
        let views = index.describe(&ids).unwrap();
        hits.into_iter()
            .map(|(id, snippet)| {
                let view = &views[&id];
                let what = match view.kind {
                    SearchHitKind::Message => format!("m:{}", view.seq.unwrap()),
                    SearchHitKind::Note => format!("n:{}", view.path.as_deref().unwrap()),
                    SearchHitKind::Workspace => format!("w:{}", view.workspace_id),
                    SearchHitKind::PullRequest => format!("p:{}", view.title),
                };
                (what, snippet)
            })
            .collect()
    }

    fn all() -> SearchFilters {
        SearchFilters::default()
    }

    #[test]
    fn finds_accented_words_and_prefixes_in_messages_with_a_highlighted_excerpt() {
        let mut f = fixture();
        let seq = message(
            &f.store,
            "c",
            "user_message",
            "La connexion à Shopify est réparée",
            None,
        );
        message(&f.store, "c", "assistant_text", "Shopify fonctionne", None);
        while f.index.sync_messages(1).unwrap() {}
        let hits = keywords(&f.index, "REPA shop", &all());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, format!("m:{seq}"));
        assert!(hits[0].1.contains("\u{1}réparée\u{2}"), "{}", hits[0].1);
    }

    #[test]
    fn leaves_out_thinking_tool_output_and_subagents() {
        let mut f = fixture();
        message(&f.store, "c", "thinking", "secret", None);
        message(&f.store, "c", "tool_result", "secret", None);
        message(&f.store, "c", "assistant_text", "secret", Some("agent"));
        message(&f.store, "c", "assistant_text", "visible", None);
        f.index.sync_messages(100).unwrap();
        assert!(keywords(&f.index, "secret", &all()).is_empty());
        assert_eq!(keywords(&f.index, "visible", &all()).len(), 1);
        let (total, embedded) = f.index.counts().unwrap();
        assert_eq!((total, embedded), (1, 0));
    }

    #[test]
    fn filters_by_state_repo_ticket_pull_request_source_and_date() {
        let mut f = fixture();
        let archived = message(&f.store, "c", "user_message", "catalogue facets", None);
        let active = message(&f.store, "ac", "user_message", "catalogue cart", None);
        f.index.sync_messages(100).unwrap();
        f.index.sync_workspaces().unwrap();
        fn only(index: &Index, filters: SearchFilters) -> Vec<String> {
            keywords(index, "catalogue", &filters)
                .into_iter()
                .map(|(what, _)| what)
                .collect()
        }
        let messages = vec![SearchSource::Messages];
        assert_eq!(
            only(
                &f.index,
                SearchFilters {
                    state: Some(SearchState::Archived),
                    sources: messages.clone(),
                    ..all()
                }
            ),
            [format!("m:{archived}")]
        );
        assert_eq!(
            only(
                &f.index,
                SearchFilters {
                    state: Some(SearchState::Active),
                    ..all()
                }
            ),
            [format!("m:{active}")]
        );
        assert_eq!(
            only(
                &f.index,
                SearchFilters {
                    repos: vec!["web".into()],
                    ..all()
                }
            ),
            [format!("m:{active}")]
        );
        assert_eq!(
            only(
                &f.index,
                SearchFilters {
                    tickets: vec!["xyz-1".into()],
                    sources: messages.clone(),
                    ..all()
                }
            ),
            [format!("m:{archived}")]
        );
        assert_eq!(
            only(
                &f.index,
                SearchFilters {
                    sources: vec![SearchSource::Titles],
                    ..all()
                }
            ),
            ["w:w"],
            "the workspace's title, 'Facettes du catalogue'"
        );
        assert!(
            only(
                &f.index,
                SearchFilters {
                    pr: Some(SearchPr::Any),
                    ..all()
                }
            )
            .is_empty()
        );
        let pr = |state: &str| PullRequest {
            repo: "web".into(),
            number: 7,
            url: "https://github.com/o/web/pull/7".into(),
            title: "Catalogue cart".into(),
            state: state.into(),
        };
        f.index
            .record_pull_requests("a", &[pr("OPEN")], "2026-11-02T00:00:00.000Z")
            .unwrap();
        assert!(
            only(
                &f.index,
                SearchFilters {
                    pr: Some(SearchPr::Merged),
                    ..all()
                }
            )
            .is_empty()
        );
        let mut with_pr = only(
            &f.index,
            SearchFilters {
                pr: Some(SearchPr::Any),
                ..all()
            },
        );
        with_pr.sort();
        assert_eq!(
            with_pr,
            [format!("m:{active}"), "p:Catalogue cart".to_string()]
        );
        f.index
            .record_pull_requests("a", &[pr("MERGED")], "2026-11-03T00:00:00.000Z")
            .unwrap();
        assert_eq!(
            only(
                &f.index,
                SearchFilters {
                    pr: Some(SearchPr::Merged),
                    sources: messages.clone(),
                    ..all()
                }
            ),
            [format!("m:{active}")]
        );
        assert_eq!(
            only(
                &f.index,
                SearchFilters {
                    since: Some("2026-11-01T00:00:00.000Z".into()),
                    ..all()
                }
            ),
            ["p:Catalogue cart"],
            "the pull request keeps the date it was first seen"
        );
        assert_eq!(
            only(
                &f.index,
                SearchFilters {
                    until: Some("2026-10-01T00:00:00.001Z".into()),
                    sources: vec![SearchSource::Titles],
                    ..all()
                }
            ),
            ["w:w"]
        );
    }

    #[test]
    fn a_deleted_chat_disappears_at_once_and_is_forgotten_at_the_next_reconciliation() {
        let mut f = fixture();
        message(&f.store, "c", "assistant_text", "visible", None);
        f.index.sync_messages(100).unwrap();
        f.store.delete_chat("c").unwrap();
        assert!(keywords(&f.index, "visible", &all()).is_empty());
        assert_eq!(f.index.reconcile().unwrap(), 1);
        assert_eq!(f.index.counts().unwrap().0, 0);
    }

    #[test]
    fn workspace_titles_and_tickets_follow_renames() {
        let mut f = fixture();
        f.index.sync_workspaces().unwrap();
        let hits = keywords(&f.index, "facet filters", &all());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "w:w");
        assert!(
            hits[0]
                .1
                .contains("XYZ-1 \u{1}Facet\u{2} \u{1}filters\u{2}"),
            "{}",
            hits[0].1
        );
        f.store
            .with(|c| {
                c.execute(
                    "UPDATE workspaces SET title = 'Recherche hybride' WHERE id = 'w'",
                    [],
                )
            })
            .unwrap();
        f.index.sync_workspaces().unwrap();
        assert_eq!(keywords(&f.index, "hybride", &all()).len(), 1);
        assert!(keywords(&f.index, "facettes", &all()).is_empty());
    }

    #[test]
    fn notes_are_indexed_updated_and_forgotten_with_their_files() {
        let mut f = fixture();
        let notes = f.dir.path().join("w").join(".chauffeur");
        std::fs::create_dir_all(notes.join("plans")).unwrap();
        std::fs::create_dir_all(notes.join(".cache")).unwrap();
        std::fs::create_dir_all(notes.join("node_modules")).unwrap();
        std::fs::write(
            notes.join("plans/rollout.md"),
            "# Rollout\nPhase un : migration",
        )
        .unwrap();
        std::fs::write(notes.join(".cache/hidden.md"), "migration").unwrap();
        std::fs::write(notes.join("node_modules/readme.md"), "migration").unwrap();
        std::fs::write(notes.join("log.txt"), "migration").unwrap();
        f.index.sync_notes(true).unwrap();
        let hits = keywords(&f.index, "migration", &all());
        assert_eq!(
            hits.iter().map(|(w, _)| w.as_str()).collect::<Vec<_>>(),
            ["n:.chauffeur/plans/rollout.md"]
        );
        std::fs::write(
            notes.join("plans/rollout.md"),
            "# Rollout\nPhase deux : bascule",
        )
        .unwrap();
        f.index.sync_notes(true).unwrap();
        assert!(keywords(&f.index, "migration", &all()).is_empty());
        assert_eq!(keywords(&f.index, "bascule", &all()).len(), 1);
        std::fs::remove_file(notes.join("plans/rollout.md")).unwrap();
        f.index.sync_notes(true).unwrap();
        assert!(keywords(&f.index, "bascule", &all()).is_empty());
    }

    #[test]
    fn vectors_are_searched_within_the_filters_and_dropped_for_another_model() {
        let mut f = fixture();
        message(
            &f.store,
            "c",
            "user_message",
            "the fan spins at night",
            None,
        );
        message(&f.store, "ac", "user_message", "the fan is loud", None);
        message(&f.store, "ac", "user_message", "invoices without VAT", None);
        f.index.sync_messages(100).unwrap();
        f.index.use_model("fake").unwrap();
        let pending = f.index.pending(10).unwrap();
        assert_eq!(pending.len(), 3);
        let vectors: Vec<Vec<f32>> = pending
            .iter()
            .map(|chunk| {
                if chunk.text.contains("fan") {
                    vec![1.0, 0.1]
                } else {
                    vec![0.0, 1.0]
                }
            })
            .collect();
        f.index.store_vectors(&pending, &vectors).unwrap();
        assert_eq!(f.index.counts().unwrap(), (3, 3));
        assert!(f.index.pending(10).unwrap().is_empty());

        fn near(index: &Index, filters: SearchFilters) -> usize {
            index
                .vector_hits(&[1.0, 0.0], &filters, 0.5, 10)
                .unwrap()
                .len()
        }
        assert_eq!(
            near(&f.index, all()),
            2,
            "the two messages about the fan, not the invoices"
        );
        assert_eq!(
            near(
                &f.index,
                SearchFilters {
                    state: Some(SearchState::Active),
                    ..all()
                }
            ),
            1
        );
        assert_eq!(
            near(
                &f.index,
                SearchFilters {
                    repos: vec!["api".into()],
                    ..all()
                }
            ),
            1
        );
        assert_eq!(
            near(
                &f.index,
                SearchFilters {
                    sources: vec![SearchSource::Notes],
                    ..all()
                }
            ),
            0
        );
        assert_eq!(
            near(
                &f.index,
                SearchFilters {
                    since: Some("2027-01-01T00:00:00.000Z".into()),
                    ..all()
                }
            ),
            0
        );
        assert_eq!(
            f.index
                .vector_hits(&[1.0, 0.0], &all(), 2.0, 10)
                .unwrap()
                .len(),
            3
        );

        f.store.delete_chat("ac").unwrap();
        assert_eq!(
            near(&f.index, all()),
            1,
            "a deleted chat's messages are not found by meaning either"
        );
        f.index.reconcile().unwrap();
        let vectors: i64 = f
            .index
            .conn()
            .query_row("SELECT count(*) FROM chunk_vectors", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vectors, 1, "the chunks' vectors go with their documents");

        f.index.use_model("another").unwrap();
        assert_eq!(f.index.counts().unwrap(), (1, 0));
        assert!(
            f.index
                .vector_hits(&[1.0, 0.0, 0.0], &all(), 2.0, 10)
                .unwrap()
                .is_empty()
        );
        let pending = f.index.pending(10).unwrap();
        f.index
            .store_vectors(&pending, &[vec![1.0, 0.0, 0.0]])
            .unwrap();
        assert_eq!(
            f.index
                .vector_hits(&[1.0, 0.0, 0.0], &all(), 0.5, 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn long_texts_are_cut_into_overlapping_pieces_at_breaks() {
        assert!(chunks("  ").is_empty());
        assert_eq!(chunks("court"), ["court"]);
        let paragraph = "Une phrase accentuée, assez longue pour compter. ".repeat(12);
        let text = format!("{paragraph}\n\n{paragraph}\n\n{paragraph}");
        let pieces = chunks(&text);
        assert!(pieces.len() >= 2, "{}", pieces.len());
        assert!(
            pieces
                .iter()
                .all(|piece| piece.chars().count() <= CHUNK_CHARS)
        );
        for pair in pieces.windows(2) {
            let tail: String = pair[0]
                .chars()
                .rev()
                .take(40)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            assert!(
                pair[1].contains(tail.trim()),
                "the next piece repeats the end of the previous one"
            );
        }
        let huge = "mot ".repeat(20_000);
        assert_eq!(chunks(&huge).len(), MAX_CHUNKS);
        let unbroken = "é".repeat(5000);
        assert!(chunks(&unbroken).iter().all(|piece| !piece.is_empty()));
    }
}
