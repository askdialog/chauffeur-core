//! Conductor's database (`conductor.db`), opened read-only: what the import takes from it. The
//! tables and columns it reads are checked first, so that a Conductor whose schema differs stops
//! the import before anything changes (written against Conductor 0.93).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, OpenFlags, params};

use crate::{Error, Result};

/// The columns read, by table.
const SCHEMA: &[(&str, &[&str])] = &[
    (
        "repos",
        &[
            "id",
            "name",
            "remote_url",
            "root_path",
            "setup_script",
            "run_script",
            "run_script_mode",
            "archive_script",
        ],
    ),
    (
        "workspaces",
        &[
            "local_id",
            "repository_id",
            "directory_name",
            "workspace_name",
            "branch",
            "state",
            "workspace_path",
            "linked_directory_paths",
            "notes",
            "hosting_server_url",
        ],
    ),
    (
        "sessions",
        &[
            "id",
            "workspace_id",
            "agent_type",
            "title",
            "model",
            "created_at",
        ],
    ),
    (
        "session_messages",
        &[
            "session_id",
            "role",
            "content",
            "created_at",
            "cancelled_at",
        ],
    ),
    ("env_vars", &["key", "scope", "context", "value"]),
];

pub struct Data {
    pub repos: Vec<Repo>,
    pub workspaces: Vec<Workspace>,
    pub sessions: Vec<Session>,
    pub env: Vec<EnvVar>,
}

pub struct Repo {
    pub id: String,
    pub name: String,
    pub remote_url: Option<String>,
    pub root_path: String,
    pub setup_script: Option<String>,
    pub run_script: Option<String>,
    pub run_script_mode: Option<String>,
    pub archive_script: Option<String>,
}

pub struct Workspace {
    pub id: String,
    pub repo_id: Option<String>,
    pub directory_name: String,
    pub workspace_name: Option<String>,
    pub branch: Option<String>,
    pub archived: bool,
    pub path: Option<String>,
    /// Its `/add-dir` folders.
    pub linked: Vec<String>,
    pub notes: Option<String>,
    /// A Conductor Cloud workspace.
    pub remote: bool,
}

impl Workspace {
    /// The name the user sees in Conductor.
    pub fn name(&self) -> &str {
        self.workspace_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.directory_name)
    }
}

pub struct Session {
    pub id: String,
    pub workspace_id: String,
    pub agent: String,
    pub title: String,
    pub model: Option<String>,
    pub created_at: String,
    pub messages: usize,
}

pub struct EnvVar {
    pub key: String,
    pub scope: String,
    pub context: String,
    pub value: String,
}

pub struct Message {
    pub role: String,
    pub content: String,
    pub created_at: String,
}

/// Opens Conductor's database without ever writing to it: read-only, and when SQLite cannot open
/// it so (a WAL database without its shared memory file), as an immutable file.
pub fn open(path: &Path) -> Result<Connection> {
    if !path.is_file() {
        return Err(Error::NotFound(format!(
            "No Conductor database at {}: is Conductor installed on this Mac? Name its database with --db",
            path.display()
        )));
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if let Ok(conn) = Connection::open_with_flags(path, flags)
        && conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
            .is_ok()
    {
        return Ok(conn);
    }
    let uri = format!("file:{}?immutable=1", uri_path(path));
    let conn = Connection::open_with_flags(&uri, flags | OpenFlags::SQLITE_OPEN_URI)
        .map_err(|e| Error::Internal(anyhow::anyhow!("opening {}: {e}", path.display())))?;
    Ok(conn)
}

fn uri_path(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| match c {
            '%' | ' ' | '?' | '#' => format!("%{:02X}", c as u32),
            c => c.to_string(),
        })
        .collect()
}

/// The first table or column the import reads that the database lacks.
pub fn missing_schema(conn: &Connection) -> Result<Option<String>> {
    for (table, columns) in SCHEMA {
        let mut statement = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(internal)?;
        let present: HashSet<String> = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(internal)?
            .collect::<rusqlite::Result<_>>()
            .map_err(internal)?;
        if present.is_empty() {
            return Ok(Some(format!("table {table}")));
        }
        if let Some(column) = columns.iter().find(|c| !present.contains(**c)) {
            return Ok(Some(format!("column {table}.{column}")));
        }
    }
    Ok(None)
}

/// Everything the import plans from, but the messages.
pub fn read(path: &Path) -> Result<Data> {
    let conn = open(path)?;
    if let Some(missing) = missing_schema(&conn)? {
        return Err(Error::Unavailable(format!(
            "Conductor's database at {} is not one this Chauffeur knows (no {missing}): it imports from Conductor 0.93. Nothing was changed; update Chauffeur, or tell us your Conductor version.",
            path.display()
        )));
    }
    read_checked(&conn).map_err(internal)
}

fn read_checked(conn: &Connection) -> rusqlite::Result<Data> {
    let repos = conn
        .prepare(
            "SELECT id, COALESCE(name, ''), remote_url, COALESCE(root_path, ''), setup_script,
                    run_script, run_script_mode, archive_script
             FROM repos ORDER BY name",
        )?
        .query_map([], |row| {
            Ok(Repo {
                id: row.get(0)?,
                name: row.get(1)?,
                remote_url: row.get(2)?,
                root_path: row.get(3)?,
                setup_script: row.get(4)?,
                run_script: row.get(5)?,
                run_script_mode: row.get(6)?,
                archive_script: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let workspaces = conn
        .prepare(
            "SELECT local_id, repository_id, COALESCE(directory_name, ''), workspace_name, branch,
                    COALESCE(state, ''), workspace_path, linked_directory_paths, notes,
                    COALESCE(hosting_server_url, '')
             FROM workspaces ORDER BY created_at",
        )?
        .query_map([], |row| {
            let linked: Option<String> = row.get(7)?;
            let host: String = row.get(9)?;
            Ok(Workspace {
                id: row.get(0)?,
                repo_id: row.get(1)?,
                directory_name: row.get(2)?,
                workspace_name: row.get(3)?,
                branch: row.get(4)?,
                archived: row.get::<_, String>(5)? == "archived",
                path: row.get(6)?,
                linked: linked
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or_default(),
                notes: row.get(8)?,
                remote: !matches!(host.as_str(), "" | "/"),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let counts: HashMap<String, usize> = conn
        .prepare(
            "SELECT session_id, count(*) FROM session_messages
             WHERE cancelled_at IS NULL GROUP BY session_id",
        )?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let sessions = conn
        .prepare(
            "SELECT id, COALESCE(workspace_id, ''), COALESCE(agent_type, 'claude'),
                    COALESCE(title, ''), model, created_at
             FROM sessions ORDER BY created_at",
        )?
        .query_map([], |row| {
            Ok(Session {
                id: row.get(0)?,
                workspace_id: row.get(1)?,
                agent: row.get(2)?,
                title: row.get(3)?,
                model: row.get(4)?,
                created_at: row.get(5)?,
                messages: 0,
            })
        })?
        .map(|session| {
            session.map(|mut session| {
                session.messages = counts.get(&session.id).copied().unwrap_or(0);
                session
            })
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let env = conn
        .prepare(
            "SELECT COALESCE(key, ''), COALESCE(scope, ''), COALESCE(context, ''),
                    COALESCE(value, '')
             FROM env_vars ORDER BY key",
        )?
        .query_map([], |row| {
            Ok(EnvVar {
                key: row.get(0)?,
                scope: row.get(1)?,
                context: row.get(2)?,
                value: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Data {
        repos,
        workspaces,
        sessions,
        env,
    })
}

/// A chat's messages, in order; messages cancelled before they were sent left out.
pub fn messages(path: &Path, session: &str) -> Result<Vec<Message>> {
    let conn = open(path)?;
    let mut statement = conn
        .prepare(
            "SELECT COALESCE(role, ''), COALESCE(content, ''), created_at FROM session_messages
             WHERE session_id = ?1 AND cancelled_at IS NULL ORDER BY created_at, rowid",
        )
        .map_err(internal)?;
    statement
        .query_map(params![session], |row| {
            Ok(Message {
                role: row.get(0)?,
                content: row.get(1)?,
                created_at: row.get(2)?,
            })
        })
        .map_err(internal)?
        .collect::<rusqlite::Result<_>>()
        .map_err(internal)
}

fn internal(error: rusqlite::Error) -> Error {
    Error::Internal(anyhow::Error::new(error).context("reading Conductor's database"))
}
