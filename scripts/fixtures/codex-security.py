"""Seed a synthetic Codex Security workbench under a throwaway home."""

import sqlite3
import sys
from datetime import datetime, timezone
from pathlib import Path

home, target = map(Path, sys.argv[1:3])
root = home / ".codex/state/plugins/codex-security"
active = root / "scans/alpha/current"
previous = root / "scans/alpha/previous"
heartbeat = active / "artifacts/deep_discovery/coordinator-heartbeat-2.json"
heartbeat.parent.mkdir(parents=True, exist_ok=True)
heartbeat.write_text("{}")
previous.mkdir(parents=True, exist_ok=True)
report = previous / "report.md"
report.write_text("# Example audit report\n\nThis report is from the previous scan.\n")
now = datetime.now(timezone.utc).isoformat()
with sqlite3.connect(root / "workbench.sqlite3") as db:
    db.executescript("""
        CREATE TABLE workspaces (id TEXT PRIMARY KEY, thread_id TEXT);
        CREATE TABLE scans (
            id TEXT PRIMARY KEY, workspace_id TEXT, target_path TEXT, scan_dir TEXT,
            mode TEXT, phase TEXT, status TEXT, canceled_at TEXT,
            deep_scan_owner_thread_id TEXT, continuation_thread_id TEXT,
            started_at TEXT, updated_at TEXT, parent_scan_id TEXT, launch_kind TEXT
        );
        CREATE TABLE scan_progress (
            scan_id TEXT PRIMARY KEY, phase_items_completed INTEGER, phase_items_total INTEGER,
            phase_progress_unit TEXT, updated_at TEXT
        );
        CREATE TABLE deep_scan_runs (
            scan_id TEXT PRIMARY KEY, status TEXT, phase TEXT, coordinator_generation INTEGER, updated_at TEXT
        );
        CREATE TABLE deep_scan_workers (id TEXT PRIMARY KEY, scan_id TEXT, kind TEXT, status TEXT);
        CREATE TABLE scan_artifacts (scan_id TEXT, kind TEXT, path TEXT);
        INSERT INTO workspaces VALUES ('fixture-workbench', 'fixture-thread');
        INSERT INTO deep_scan_workers VALUES
            ('discovery-1', 'current', 'discovery', 'succeeded'),
            ('discovery-2', 'current', 'discovery', 'running'),
            ('dedup-1', 'current', 'dedup', 'succeeded');
    """)
    db.executemany(
        "INSERT INTO scans VALUES (?, 'fixture-workbench', ?, ?, 'deep', ?, ?, NULL, NULL, NULL, ?, ?, ?, NULL)",
        [
            ("current", str(target), str(active), "discovery", "running", now, now, None),
            ("previous", str(target), str(previous), "reporting", "complete", now, now, None),
            ("child", str(target), str(active), "discovery", "running", now, now, "current"),
            ("outside", str(target.with_name("unrelated")), str(previous), "reporting", "complete", now, now, None),
        ],
    )
    db.execute("INSERT INTO deep_scan_runs VALUES ('current', 'running', 'discovery', 2, ?)", (now,))
    db.executemany(
        "INSERT INTO scan_artifacts VALUES (?, 'markdownReport', ?)",
        [("previous", str(report)), ("outside", str(report)), ("current", str(report))],
    )
