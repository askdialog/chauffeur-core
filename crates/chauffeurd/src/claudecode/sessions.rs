//! Claude Code's sessions as its transcripts record them: `<claude>/projects/<folder key>/<id>.jsonl`
//! and its subagents' `<id>/subagents/*.jsonl`. What a session wrote (its file tools' paths) and
//! the branches its shell commands created, nothing resolved against git yet; and which sessions
//! still run (`<claude>/sessions/<pid>.json` of a live process).

use std::collections::HashSet;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use regex::Regex;
use serde_json::Value;

/// A file tool's call, as a transcript line spells it.
const CALLS: &[&str] = &[
    "\"name\":\"Write\"",
    "\"name\":\"Edit\"",
    "\"name\":\"MultiEdit\"",
    "\"name\":\"NotebookEdit\"",
];

/// The file tools and the input naming the file they write.
const FILE_TOOLS: &[(&str, &str)] = &[
    ("Write", "file_path"),
    ("Edit", "file_path"),
    ("MultiEdit", "file_path"),
    ("NotebookEdit", "notebook_path"),
];

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub title: String,
    /// The folder it ran in: its first message's.
    pub folder: Option<PathBuf>,
    /// What started it: `cli`, `claude-vscode`… or `sdk-ts` for an app on the Agent SDK.
    pub entrypoint: Option<String>,
    /// Its last message (RFC 3339).
    pub last_active: String,
    pub writes: Vec<Write>,
    pub created: Vec<Created>,
}

/// A file a file tool wrote, with where the session stood then.
#[derive(Debug, Clone, PartialEq)]
pub struct Write {
    pub path: PathBuf,
    pub cwd: PathBuf,
    /// The branch checked out in `cwd` then; none when detached.
    pub branch: Option<String>,
}

/// A branch a shell command created, in the repo of `dir`.
#[derive(Debug, Clone, PartialEq)]
pub struct Created {
    pub dir: PathBuf,
    pub branch: String,
}

/// The sessions of every folder of `projects` whose last message is at `cutoff` or later (all
/// with none). A transcript changed before the cutoff is not read.
pub fn read_all(projects: &Path, cutoff: Option<&str>) -> Vec<Session> {
    let cutoff_time = cutoff
        .and_then(|c| chrono::DateTime::parse_from_rfc3339(c).ok())
        .map(SystemTime::from);
    let mut transcripts = Vec::new();
    for folder in std::fs::read_dir(projects).into_iter().flatten().flatten() {
        for file in std::fs::read_dir(folder.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = file.path();
            let changed = file.metadata().and_then(|m| m.modified()).ok();
            let before =
                matches!((cutoff_time, changed), (Some(cutoff), Some(changed)) if changed < cutoff);
            if path.extension().is_some_and(|e| e == "jsonl") && !before {
                transcripts.push(path);
            }
        }
    }
    // Gigabytes of transcripts: read on every core.
    let workers = std::thread::available_parallelism().map_or(4, usize::from);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut sessions: Vec<Session> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut read_here = Vec::new();
                    while let Some(path) =
                        transcripts.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
                    {
                        if let Some(session) = read(path)
                            && cutoff.is_none_or(|c| session.last_active.as_str() >= c)
                        {
                            read_here.push(session);
                        }
                    }
                    read_here
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    sessions.sort_by(|a, b| {
        b.last_active
            .cmp(&a.last_active)
            .then_with(|| a.id.cmp(&b.id))
    });
    sessions
}

/// A session from its transcript and its subagents'; none when it holds no message.
pub fn read(transcript: &Path) -> Option<Session> {
    let id = transcript.file_stem()?.to_string_lossy().into_owned();
    let mut read = Reading::default();
    read.file(transcript, true);
    if let Ok(agents) = std::fs::read_dir(transcript.with_extension("").join("subagents")) {
        for agent in agents.flatten() {
            if agent.path().extension().is_some_and(|e| e == "jsonl") {
                read.file(&agent.path(), false);
            }
        }
    }
    let last_active = read.last_active?;
    let title = read
        .custom_title
        .or(read.ai_title)
        .or(read.first_prompt)
        .unwrap_or_else(|| id.chars().take(8).collect());
    Some(Session {
        id,
        title,
        folder: read.folder,
        entrypoint: read.entrypoint,
        last_active,
        writes: read.writes,
        created: read.created,
    })
}

#[derive(Default)]
struct Reading {
    folder: Option<PathBuf>,
    entrypoint: Option<String>,
    last_active: Option<String>,
    custom_title: Option<String>,
    ai_title: Option<String>,
    first_prompt: Option<String>,
    writes: Vec<Write>,
    created: Vec<Created>,
}

impl Reading {
    /// One transcript; `main` for the session's own, whose folder and titles count.
    fn file(&mut self, path: &Path, main: bool) {
        let Ok(file) = std::fs::File::open(path) else {
            return;
        };
        for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
            // Transcripts run to hundreds of megabytes, mostly API requests and tool results: a
            // line is parsed only when it can matter, its time read as text otherwise.
            if self.wanted(&line, main) {
                if let Ok(entry) = serde_json::from_str::<Value>(&line) {
                    self.entry(&entry, main);
                }
            } else if let Some(stamp) = stamp(&line) {
                self.saw(stamp);
            }
        }
    }

    /// A line that may hold the folder, a title, the first prompt or a tool call.
    fn wanted(&self, line: &str, main: bool) -> bool {
        if line.contains("\"tool_use\"") {
            // A file tool's call, or a command that may create a branch (`git` with a `-b`, `-B`,
            // `-c` or `-C`).
            return CALLS.iter().any(|call| line.contains(call))
                || (line.contains("\"name\":\"Bash\"")
                    && line.contains("git")
                    && [" -b", " -B", " -c", " -C"]
                        .iter()
                        .any(|o| line.contains(o)));
        }
        main && (self.folder.is_none()
            || line.starts_with("{\"type\":\"custom-title\"")
            || line.starts_with("{\"type\":\"ai-title\"")
            || (self.first_prompt.is_none() && line.contains("\"type\":\"user\"")))
    }

    fn saw(&mut self, stamp: &str) {
        if self.last_active.as_deref().is_none_or(|last| stamp > last) {
            self.last_active = Some(stamp.to_string());
        }
    }

    fn entry(&mut self, entry: &Value, main: bool) {
        let text = |key: &str| entry[key].as_str().filter(|t| !t.is_empty());
        if main {
            match text("type") {
                Some("custom-title") => self.custom_title = text("customTitle").map(String::from),
                Some("ai-title") => self.ai_title = text("aiTitle").map(String::from),
                _ => {}
            }
        }
        let Some(cwd) = text("cwd").map(PathBuf::from) else {
            return;
        };
        if let Some(stamp) = text("timestamp") {
            self.saw(stamp);
        }
        if main && self.folder.is_none() {
            self.folder = Some(cwd.clone());
            self.entrypoint = text("entrypoint").map(String::from);
        }
        let branch = text("gitBranch").filter(|b| *b != "HEAD").map(String::from);
        let content = &entry["message"]["content"];
        if main && self.first_prompt.is_none() && text("type") == Some("user") {
            self.first_prompt = prompt(content);
        }
        for block in content.as_array().into_iter().flatten() {
            if block["type"] != "tool_use" {
                continue;
            }
            let input = &block["input"];
            let tool = block["name"].as_str().unwrap_or_default();
            if let Some((_, key)) = FILE_TOOLS.iter().find(|(name, _)| *name == tool)
                && let Some(path) = input[key].as_str().filter(|p| !p.is_empty())
            {
                self.writes.push(Write {
                    path: cwd.join(path),
                    cwd: cwd.clone(),
                    branch: branch.clone(),
                });
            }
            if tool == "Bash"
                && let Some(command) = input["command"].as_str()
            {
                for (dir, branch) in created_branches(command) {
                    self.created.push(Created {
                        dir: dir.map_or_else(|| cwd.clone(), |d| cwd.join(d)),
                        branch,
                    });
                }
            }
        }
    }
}

/// A transcript line's time, read as text: its first `"timestamp":"…"` (keys are never escaped).
fn stamp(line: &str) -> Option<&str> {
    let key = "\"timestamp\":\"";
    let start = line.find(key)? + key.len();
    let end = start + line[start..].find('"')?;
    Some(&line[start..end])
}

/// A user message's text, cut to a title's length.
fn prompt(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(text) => text.as_str(),
        Value::Array(blocks) => blocks.iter().find_map(|b| b["text"].as_str())?,
        _ => return None,
    };
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    // Claude Code's own notes (`<command-name>`, `<local-command-stdout>`…) are no prompt.
    (!line.starts_with('<')).then(|| line.chars().take(80).collect())
}

static CREATES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"\bgit(?:\s+-C\s+("[^"]+"|'[^']+'|\S+))?\s+(?:worktree\s+add\b[^;&|\n]*?\s-[bB]|switch\b[^;&|\n]*?\s-[cC]|checkout\b[^;&|\n]*?\s-[bB])\s+("[^"]+"|'[^']+'|[^\s;&|]+)"#,
    )
    .expect("valid regex")
});

/// The branches a shell command creates (`git worktree add -b`, `git switch -c`, `git checkout
/// -b`), with the folder given to `-C`.
pub fn created_branches(command: &str) -> Vec<(Option<String>, String)> {
    let unquote = |s: &str| s.trim_matches(|c| c == '"' || c == '\'').to_string();
    CREATES
        .captures_iter(command)
        .filter_map(|c| {
            let branch = unquote(c.get(2)?.as_str());
            (!branch.is_empty() && !branch.starts_with('-'))
                .then(|| (c.get(1).map(|d| unquote(d.as_str())), branch))
        })
        .collect()
}

/// The sessions still running: those whose `<claude>/sessions/<pid>.json` names a live process.
pub fn running(sessions: &Path) -> HashSet<String> {
    let mut running = HashSet::new();
    let Ok(files) = std::fs::read_dir(sessions) else {
        return running;
    };
    for file in files.flatten() {
        if file.path().extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(entry) = std::fs::read(file.path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        else {
            continue;
        };
        let (Some(pid), Some(id)) = (entry["pid"].as_i64(), entry["sessionId"].as_str()) else {
            continue;
        };
        if i32::try_from(pid).is_ok_and(alive) {
            running.insert(id.to_string());
        }
    }
    running
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    pid > 0 && unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_branches_a_command_creates_are_read_with_the_folder_it_names() {
        type Case<'a> = (&'a str, &'a [(Option<&'a str>, &'a str)]);
        let cases: &[Case] = &[
            ("git switch -c feat/x", &[(None, "feat/x")]),
            ("git checkout -b fix-1 origin/main", &[(None, "fix-1")]),
            ("git checkout -B fix-2", &[(None, "fix-2")]),
            (
                "git worktree add -b dat-2 ../.wt/dat-2 origin/main",
                &[(None, "dat-2")],
            ),
            ("git worktree add ../.wt/x -b 'dat-3'", &[(None, "dat-3")]),
            (
                "git -C /code/web switch -c y && git checkout -b z",
                &[(Some("/code/web"), "y"), (None, "z")],
            ),
            (
                "cd web && git fetch && git switch -C rel/1.2",
                &[(None, "rel/1.2")],
            ),
            ("git switch main", &[]),
            ("git checkout -- file.ts", &[]),
            ("git worktree add ../.wt/x existing", &[]),
            ("echo git is fine", &[]),
        ];
        for (command, expected) in cases {
            let expected: Vec<(Option<String>, String)> = expected
                .iter()
                .map(|(d, b)| (d.map(String::from), b.to_string()))
                .collect();
            assert_eq!(created_branches(command), expected, "{command}");
        }
    }

    #[test]
    fn a_transcript_gives_its_title_folder_last_message_writes_and_created_branches() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("s1.jsonl");
        let lines = [
            r#"{"type":"user","cwd":"/code/web","gitBranch":"main","entrypoint":"claude-vscode","timestamp":"2026-11-01T10:00:00.000Z","message":{"content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","cwd":"/code/web","gitBranch":"main","timestamp":"2026-11-01T10:00:01.000Z","message":{"content":[{"type":"text","text":"Fix the login\nthen ship"}]}}"#,
            r#"{"type":"ai-title","aiTitle":"Login fix"}"#,
            r#"{"type":"assistant","cwd":"/code/web","gitBranch":"main","timestamp":"2026-11-01T10:00:02.000Z","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"git switch -c fix/login && git -C ../api checkout -b fix/api"}}]}}"#,
            r#"{"type":"assistant","cwd":"/code/web","gitBranch":"fix/login","timestamp":"2026-11-01T10:00:03.000Z","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"src/a.ts"}},{"type":"tool_use","name":"Read","input":{"file_path":"/code/web/b.ts"}}]}}"#,
            r#"{"type":"assistant","cwd":"/code/web","gitBranch":"HEAD","timestamp":"2026-11-01T10:00:04.000Z","message":{"content":[{"type":"tool_use","name":"NotebookEdit","input":{"notebook_path":"/code/web/n.ipynb"}}]}}"#,
            "not json",
            // A line read for its time only.
            r#"{"type":"attachment","cwd":"/code/web","timestamp":"2026-11-03T00:00:00.000Z","attachment":{"type":"environment"}}"#,
        ];
        std::fs::write(&transcript, lines.join("\n")).unwrap();
        let agents = dir.path().join("s1").join("subagents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("agent-a.jsonl"),
            r#"{"type":"assistant","cwd":"/code/web/.wt/x","gitBranch":"x","timestamp":"2026-11-02T08:00:00.000Z","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"/code/web/.wt/x/c.ts"}}]}}
{"type":"ai-title","aiTitle":"Not the session's"}"#,
        )
        .unwrap();
        let session = read(&transcript).unwrap();
        assert_eq!(session.id, "s1");
        assert_eq!(session.title, "Login fix");
        assert_eq!(session.folder.as_deref(), Some(Path::new("/code/web")));
        assert_eq!(session.entrypoint.as_deref(), Some("claude-vscode"));
        assert_eq!(session.last_active, "2026-11-03T00:00:00.000Z");
        let written: Vec<(&Path, Option<&str>)> = session
            .writes
            .iter()
            .map(|w| (w.path.as_path(), w.branch.as_deref()))
            .collect();
        assert_eq!(
            written,
            [
                (Path::new("/code/web/src/a.ts"), Some("fix/login")),
                (Path::new("/code/web/n.ipynb"), None),
                (Path::new("/code/web/.wt/x/c.ts"), Some("x")),
            ]
        );
        assert_eq!(
            session.created,
            [
                Created {
                    dir: PathBuf::from("/code/web"),
                    branch: "fix/login".into()
                },
                Created {
                    dir: PathBuf::from("/code/web/../api"),
                    branch: "fix/api".into()
                }
            ]
        );
    }

    #[test]
    fn a_title_falls_back_on_a_custom_one_first_then_the_first_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, lines: &[&str]| {
            let path = dir.path().join(format!("{name}.jsonl"));
            std::fs::write(&path, lines.join("\n")).unwrap();
            read(&path).unwrap().title
        };
        let message = r#"{"type":"user","cwd":"/w","timestamp":"2026-11-01T10:00:00.000Z","message":{"content":"Plan the import"}}"#;
        assert_eq!(
            write(
                "a",
                &[
                    message,
                    r#"{"type":"ai-title","aiTitle":"AI"}"#,
                    r#"{"type":"custom-title","customTitle":"Mine"}"#
                ]
            ),
            "Mine"
        );
        let note = r#"{"type":"user","cwd":"/w","timestamp":"2026-11-01T09:00:00.000Z","message":{"content":"<command-name>/clear</command-name>"}}"#;
        assert_eq!(write("b", &[note, message]), "Plan the import");
        assert!(read(&dir.path().join("missing.jsonl")).is_none());
    }

    #[test]
    fn only_sessions_of_the_window_are_read_and_the_most_recent_comes_first() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("-code-web");
        std::fs::create_dir_all(&folder).unwrap();
        for (id, stamp) in [
            ("old", "2026-09-01T00:00:00.000Z"),
            ("new", "2026-11-10T00:00:00.000Z"),
            ("mid", "2026-11-05T00:00:00.000Z"),
        ] {
            std::fs::write(
                folder.join(format!("{id}.jsonl")),
                format!(r#"{{"type":"user","cwd":"/code/web","timestamp":"{stamp}","message":{{"content":"x"}}}}"#),
            )
            .unwrap();
        }
        let ids = |cutoff| {
            read_all(dir.path(), cutoff)
                .into_iter()
                .map(|s| s.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(Some("2026-11-01T00:00:00.000Z")), ["new", "mid"]);
        assert_eq!(ids(None), ["new", "mid", "old"]);
    }

    #[test]
    fn a_session_runs_while_the_process_its_file_names_lives() {
        let dir = tempfile::tempdir().unwrap();
        let me = std::process::id();
        std::fs::write(
            dir.path().join(format!("{me}.json")),
            format!(r#"{{"pid":{me},"sessionId":"live"}}"#),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("1.json"),
            r#"{"pid":999999999,"sessionId":"gone"}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("x.key"), "k").unwrap();
        assert_eq!(running(dir.path()), HashSet::from(["live".to_string()]));
    }
}
