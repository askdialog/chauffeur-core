//! Desktop projection of the local Codex Security workbench.
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use chauffeur_protocol::client::{
    SecurityAudit, SecurityAuditParams, SecurityAuditProgress, SecurityAuditStatus,
};
use chauffeur_protocol::model::{AgentKind, ChatInfo};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, params};

use crate::{Daemon, Error, Result, store::WorktreeRow};

const MAX_REPORT_BYTES: u64 = 2 * 1024 * 1024;
const HISTORY_PER_REPO: i64 = 8;

const SCANS: &str = "
    SELECT s.id, s.mode, s.phase, s.status, s.canceled_at IS NOT NULL,
           COALESCE(s.deep_scan_owner_thread_id, s.continuation_thread_id, w.thread_id),
           s.scan_dir, a.path, p.phase_items_completed, p.phase_items_total, p.phase_progress_unit,
           MAX(s.updated_at, COALESCE(p.updated_at, ''), COALESCE(r.updated_at, '')),
           r.status, r.phase, r.coordinator_generation,
           COUNT(d.id), COUNT(CASE WHEN d.status = 'succeeded' THEN 1 END)
    FROM scans s
    JOIN workspaces w ON w.id = s.workspace_id
    LEFT JOIN scan_progress p ON p.scan_id = s.id
    LEFT JOIN deep_scan_runs r ON r.scan_id = s.id
    LEFT JOIN deep_scan_workers d ON d.scan_id = s.id AND d.kind = 'discovery'
    LEFT JOIN scan_artifacts a ON a.scan_id = s.id AND a.kind = 'markdownReport'
    WHERE (s.target_path = ?1 OR s.target_path = ?2)
      AND (?3 IS NULL OR s.id = ?3)
      AND s.parent_scan_id IS NULL AND COALESCE(s.launch_kind, 'scan') = 'scan'
    GROUP BY s.id
    ORDER BY (s.status = 'running' AND s.canceled_at IS NULL) DESC, s.started_at DESC
    LIMIT ?4";

struct Source {
    root: PathBuf,
    worktrees: Vec<WorktreeRow>,
    chats: Vec<ChatInfo>,
}

struct Scan {
    info: SecurityAudit,
    report: Option<PathBuf>,
    thread: Option<String>,
}

pub async fn list(daemon: &Daemon, workspace: &str) -> Result<Vec<SecurityAudit>> {
    Ok(lookup(daemon, workspace, None)
        .await?
        .into_iter()
        .map(|scan| scan.info)
        .collect())
}

pub async fn report(daemon: &Daemon, params: SecurityAuditParams) -> Result<String> {
    let scan = find(daemon, &params).await?;
    let path = scan
        .report
        .ok_or_else(|| Error::not_found("security audit report"))?;
    tokio::task::spawn_blocking(move || {
        let mut text = String::new();
        let file = File::open(path).context("open the Codex Security report")?;
        file.take(MAX_REPORT_BYTES + 1)
            .read_to_string(&mut text)
            .context("read the Codex Security report")?;
        if text.len() as u64 > MAX_REPORT_BYTES {
            return Err(Error::Unavailable(
                "this Codex Security report is too large to open here".into(),
            ));
        }
        Ok(text)
    })
    .await
    .context("read the Codex Security report")?
}

pub async fn cancel(daemon: &Daemon, params: SecurityAuditParams) -> Result<()> {
    let scan = find(daemon, &params).await?;
    if scan.info.status != SecurityAuditStatus::Running
        && scan.info.status != SecurityAuditStatus::Interrupted
    {
        return Err(Error::Conflict("this audit has already ended".into()));
    }
    let chat_id = scan.info.owner_chat.ok_or_else(|| {
        Error::Conflict("open the original Codex chat to cancel this audit".into())
    })?;
    let chat = daemon
        .store
        .chat(&chat_id)?
        .filter(|chat| {
            !chat.closed && chat.agent == AgentKind::Codex && chat.agent_session_id == scan.thread
        })
        .ok_or_else(|| {
            Error::Conflict("open the original Codex chat to cancel this audit".into())
        })?;
    let id = uuid::Uuid::parse_str(&scan.info.id).context("invalid Codex Security scan ID")?;
    daemon
        .chats
        .send(
            &chat.id,
            &format!(
                "I requested cancellation of Codex Security audit {id}. Call the Codex Security cancellation tool for that exact scan ID. If it already ended, report its status. Report the result of the cancellation request."
            ),
        )
        .await?;
    Ok(())
}

async fn find(daemon: &Daemon, params: &SecurityAuditParams) -> Result<Scan> {
    lookup(daemon, &params.workspace, Some(params.scan.clone()))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| Error::not_found("security audit in this workspace"))
}

async fn lookup(daemon: &Daemon, workspace: &str, scan: Option<String>) -> Result<Vec<Scan>> {
    let row = daemon.workspaces.row(workspace)?;
    let worktrees = daemon.store.worktrees_of(&row.id)?;
    if worktrees.is_empty() {
        return Ok(Vec::new());
    }
    let Some(home) = daemon.shell_env.vars().get("HOME").map(PathBuf::from) else {
        return Ok(Vec::new());
    };
    let source = Source {
        root: home.join(".codex/state/plugins/codex-security"),
        worktrees,
        chats: daemon.store.chats_of(&row.id)?,
    };
    tokio::task::spawn_blocking(move || source.read(scan.as_deref()))
        .await
        .context("read Codex Security audits")?
}

impl Source {
    fn read(&self, scan: Option<&str>) -> Result<Vec<Scan>> {
        let path = self.root.join("workbench.sqlite3");
        match fs::metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            result => result.context("find the Codex Security workbench")?,
        };
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .context("open the Codex Security workbench")?;
        connection
            .busy_timeout(Duration::from_millis(300))
            .context("set Codex Security workbench read timeout")?;
        self.query(&connection, scan)
    }

    fn query(&self, connection: &Connection, scan: Option<&str>) -> Result<Vec<Scan>> {
        let mut statement = connection
            .prepare(SCANS)
            .context("read the Codex Security workbench schema")?;
        let scan_root = self.root.join("scans").canonicalize().ok();
        let mut scans = Vec::new();
        for tree in &self.worktrees {
            let canonical_path = Path::new(&tree.path).canonicalize().ok();
            let rows = statement
                .query_map(
                    params![
                        tree.path,
                        canonical_path.as_deref().and_then(Path::to_str),
                        scan,
                        HISTORY_PER_REPO
                    ],
                    |row| {
                        let status: String = row.get(3)?;
                        let deep_status: Option<String> = row.get(12)?;
                        let status = if row.get(4)? {
                            SecurityAuditStatus::Canceled
                        } else {
                            match (status.as_str(), deep_status.as_deref()) {
                                ("complete", _) => SecurityAuditStatus::Completed,
                                ("failed", _) | (_, Some("failed")) => SecurityAuditStatus::Failed,
                                (_, Some("canceled")) => SecurityAuditStatus::Canceled,
                                (_, Some("interrupted")) => SecurityAuditStatus::Interrupted,
                                _ => SecurityAuditStatus::Running,
                            }
                        };
                        let mut phase: String = row.get(2)?;
                        if row.get::<_, Option<String>>(13)?.as_deref() == Some("reducing") {
                            phase = "reducing".into();
                        }
                        let worker_total: u32 = row.get(15)?;
                        let phase_total: u32 = row.get::<_, Option<u32>>(9)?.unwrap_or(0);
                        let progress = if phase == "discovery" && worker_total > 0 {
                            Some(SecurityAuditProgress {
                                completed: row.get(16)?,
                                total: worker_total,
                                unit: "passes".into(),
                            })
                        } else if phase != "reducing" && phase_total > 0 {
                            Some(SecurityAuditProgress {
                                completed: row.get(8)?,
                                total: phase_total,
                                unit: row
                                    .get::<_, Option<String>>(10)?
                                    .unwrap_or_else(|| "items".into()),
                            })
                        } else {
                            None
                        };
                        let thread = row.get::<_, Option<String>>(5)?;
                        let owner_chat = self
                            .chats
                            .iter()
                            .find(|chat| {
                                !chat.closed
                                    && chat.agent == AgentKind::Codex
                                    && thread.is_some()
                                    && chat.agent_session_id == thread
                            })
                            .map(|chat| chat.id.clone());
                        let scan_dir = row.get::<_, String>(6)?;
                        let scan_dir = scan_root.as_ref().and_then(|root| {
                            let path = Path::new(&scan_dir).canonicalize().ok()?;
                            (path != *root && path.starts_with(root)).then_some(path)
                        });
                        let report = row.get::<_, Option<String>>(7)?.and_then(|path| {
                            let dir = scan_dir.as_deref()?;
                            let path = Path::new(&path).canonicalize().ok()?;
                            (path == dir.join("report.md") && path.is_file()).then_some(path)
                        });
                        let heartbeat_at = if status == SecurityAuditStatus::Running {
                            heartbeat(scan_dir.as_deref(), row.get(14)?)
                        } else {
                            None
                        };
                        let last_activity_at: String = row.get(11)?;
                        let last_activity_at = match (
                            &heartbeat_at,
                            DateTime::parse_from_rfc3339(&last_activity_at),
                        ) {
                            (Some(heartbeat), Ok(recorded)) if *heartbeat > recorded => {
                                heartbeat.to_rfc3339()
                            }
                            _ => last_activity_at,
                        };
                        Ok(Scan {
                            info: SecurityAudit {
                                id: row.get(0)?,
                                repo: tree.repo.clone(),
                                mode: row.get(1)?,
                                phase,
                                status,
                                progress,
                                last_activity_at,
                                heartbeat_at: heartbeat_at.map(|time| time.to_rfc3339()),
                                owner_chat,
                                has_report: report.is_some(),
                            },
                            report,
                            thread,
                        })
                    },
                )
                .context("query Codex Security audits")?;
            for row in rows {
                let row = row.context("read a Codex Security audit")?;
                if !scans.iter().any(|scan: &Scan| scan.info.id == row.info.id) {
                    scans.push(row);
                }
            }
        }
        scans.sort_by(|left, right| {
            let active = |scan: &Scan| {
                matches!(
                    scan.info.status,
                    SecurityAuditStatus::Running | SecurityAuditStatus::Interrupted
                )
            };
            active(right)
                .cmp(&active(left))
                .then_with(|| right.info.last_activity_at.cmp(&left.info.last_activity_at))
        });
        Ok(scans)
    }
}

fn heartbeat(scan_dir: Option<&Path>, generation: Option<u32>) -> Option<DateTime<Utc>> {
    let path = scan_dir?.join(format!(
        "artifacts/deep_discovery/coordinator-heartbeat-{}.json",
        generation?
    ));
    Some(fs::metadata(path).ok()?.modified().ok()?.into())
}
