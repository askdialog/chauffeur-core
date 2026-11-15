//! Comments on a markdown file of a workspace (story 57): a note, a plan or a `.md` of a repo.
//! They follow a design's rules (`comments.rs`: who edits, who resolves, what is sent) and are
//! kept in the workspace's `.chauffeur/comments.json`, each thread on one file (`screen` is its
//! path under the workspace folder) and a passage of it (`quote`). The user sends them to the
//! chat whose agent last wrote the file, or to another; the agent never reads that file.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chauffeur_protocol::client::{
    CommentsSendResult, DesignComment, FileCommentAddParams, FileCommentEditParams,
    FileCommentParams, FileCommentResolveParams, FileCommentsResult,
};
use chauffeur_protocol::events::{ChatEvent, Event, EventEnvelope};
use serde_json::Value;

use crate::comments;
use crate::paths::NOTES_DIR;
use crate::store::WorkspaceRow;
use crate::{Daemon, Error, Result};

/// Characters of a passage kept at most: a selection beyond it is not a passage.
const QUOTE_MAX: usize = 2_000;
/// Characters of an agent's answer kept in a slide comment's thread.
const ANSWER_MAX: usize = 20_000;
/// Characters of a passage repeated in the message to the agent.
const QUOTE_IN_MESSAGE: usize = 300;
/// Tool calls naming the file read at most, newest first, to find who last wrote it.
const WRITES_SCANNED: u32 = 200;

/// One writer of the workspace's `comments.json` at a time: the app may add two comments at once.
static WRITING: Mutex<()> = Mutex::new(());

pub fn is_markdown(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".md", ".mdx", ".markdown"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

struct Target {
    row: WorkspaceRow,
    /// The notes folder, where `comments.json` lives.
    notes: PathBuf,
    /// The file itself, symlinks resolved.
    real: PathBuf,
}

fn target(daemon: &Arc<Daemon>, workspace: &str, path: &str) -> Result<Target> {
    if !is_markdown(path) && !crate::deck::is_slide(path) {
        return Err(Error::BadRequest(format!(
            "{path} takes no comments: markdown files and the slides of a deck do"
        )));
    }
    let (row, roots) = crate::rpc::file_roots(daemon, workspace)?;
    let real = crate::files::resolve(&roots, path)?;
    let notes = Path::new(&row.path).join(NOTES_DIR);
    Ok(Target { row, notes, real })
}

/// A document of a workspace, as a share holds it.
pub struct SharedFile {
    pub workspace_id: String,
    pub notes: PathBuf,
    pub real: PathBuf,
}

/// Markdown files and HTML/PDF deliverables may be shared independently of a canvas.
pub fn shared_file(daemon: &Arc<Daemon>, workspace: &str, path: &str) -> Result<SharedFile> {
    let target = if is_markdown(path) {
        target(daemon, workspace, path)?
    } else {
        if !crate::server::deliverable_path(path) {
            return Err(Error::BadRequest(
                "Share a markdown file or an HTML/PDF document under .chauffeur/artifacts".into(),
            ));
        }
        let (row, roots) = crate::rpc::file_roots(daemon, workspace)?;
        let real = crate::files::resolve(&roots, path)?;
        let metadata = std::fs::metadata(&real)?;
        if !metadata.is_file() || metadata.len() > chauffeur_design::MAX_FILE_BYTES {
            return Err(Error::BadRequest(
                "Shared documents must be files no larger than 25 MB".into(),
            ));
        }
        if path.to_ascii_lowercase().ends_with(".pdf")
            && crate::files::media_type(&real, path) != Some("application/pdf")
        {
            return Err(Error::BadRequest(
                "This document is not a PDF: regenerate it before sharing".into(),
            ));
        }
        let notes = Path::new(&row.path).join(NOTES_DIR);
        Target { row, notes, real }
    };
    Ok(SharedFile {
        workspace_id: target.row.id,
        notes: target.notes,
        real: target.real,
    })
}

/// A reviewer's comment from the file's share, new or changed, kept by its id in its place.
pub fn merge_remote(notes: &Path, comment: DesignComment) -> Result<()> {
    update(notes, |all| {
        match all.iter_mut().find(|c| c.id == comment.id) {
            Some(existing) => *existing = comment,
            None => all.push(comment),
        }
        Ok(())
    })
}

/// A reviewer removed their comment on the file's share.
pub fn remove_remote(notes: &Path, id: &str) -> Result<()> {
    update(notes, |all| {
        all.retain(|c| c.id != id);
        Ok(())
    })
}

fn notes_of(daemon: &Arc<Daemon>, workspace: &str) -> Result<(WorkspaceRow, PathBuf)> {
    let row = daemon.workspaces.row(workspace)?;
    let notes = Path::new(&row.path).join(NOTES_DIR);
    Ok((row, notes))
}

/// Applies `change` to the workspace's comments and writes them back (see `comments::update`).
fn update<T>(notes: &Path, change: impl FnOnce(&mut Vec<DesignComment>) -> Result<T>) -> Result<T> {
    let _writing = WRITING.lock().unwrap();
    std::fs::create_dir_all(notes)?;
    comments::update(notes, change)
}

pub async fn list(daemon: &Arc<Daemon>, workspace: &str, path: &str) -> Result<FileCommentsResult> {
    let target = target(daemon, workspace, path)?;
    let (all, error) = comments::read(&target.notes);
    Ok(FileCommentsResult {
        path: path.to_string(),
        comments: all.into_iter().filter(|c| c.screen == path).collect(),
        author: comments::author(daemon, &target.row.id, &target.row.path).await,
        writer: last_writer(daemon, &target.row, &target.real),
        error,
    })
}

pub async fn add(daemon: &Arc<Daemon>, params: &FileCommentAddParams) -> Result<DesignComment> {
    let target = target(daemon, &params.workspace, &params.path)?;
    let quote = match params.quote.as_deref().map(str::trim) {
        Some(quote) if quote.chars().count() > QUOTE_MAX => {
            return Err(Error::BadRequest(format!(
                "a comment is on a passage of {QUOTE_MAX} characters at most: select less"
            )));
        }
        Some("") | None => None,
        Some(quote) => Some(quote.to_string()),
    };
    if params.pin.is_some() && !crate::deck::is_slide(&params.path) {
        return Err(Error::BadRequest(
            "only a slide's comments are pinned to a point of it".into(),
        ));
    }
    let author = comments::author(daemon, &target.row.id, &target.row.path).await;
    update(&target.notes, |all| {
        add_comment(
            all,
            &params.path,
            Anchor {
                quote,
                pin: params.pin.clone(),
            },
            &params.text,
            params.reply_to.as_deref(),
            &author,
            comments::now_ms(),
        )
    })
}

/// Where a thread lies: a passage of a markdown file, a point of a slide, or the whole file.
#[derive(Default)]
struct Anchor {
    quote: Option<String>,
    pin: Option<chauffeur_protocol::client::CommentPin>,
}

/// A thread on a passage of `path` (or the whole file), or a reply to one of its threads.
fn add_comment(
    all: &mut Vec<DesignComment>,
    path: &str,
    anchor: Anchor,
    text: &str,
    reply_to: Option<&str>,
    author: &chauffeur_protocol::client::CommentAuthor,
    now: u64,
) -> Result<DesignComment> {
    if let Some(parent) = reply_to.and_then(|id| all.iter().find(|c| c.id == id))
        && parent.screen != path
    {
        return Err(Error::BadRequest(format!(
            "comment {} is on {}, not on {path}",
            parent.id, parent.screen
        )));
    }
    let mut comment = comments::add_comment(all, path, anchor.pin, text, reply_to, author, now)?;
    if reply_to.is_none() {
        comment.quote = anchor.quote;
        let last = all.last_mut().expect("the comment was just added");
        last.quote = comment.quote.clone();
    }
    Ok(comment)
}

pub async fn edit(daemon: &Arc<Daemon>, params: &FileCommentEditParams) -> Result<DesignComment> {
    let (row, notes) = notes_of(daemon, &params.workspace)?;
    let author = comments::author(daemon, &row.id, &row.path).await;
    update(&notes, |all| {
        comments::edit_comment(all, &params.id, &params.text, &author)
    })
}

pub async fn delete(daemon: &Arc<Daemon>, params: &FileCommentParams) -> Result<()> {
    let (row, notes) = notes_of(daemon, &params.workspace)?;
    let author = comments::author(daemon, &row.id, &row.path).await;
    update(&notes, |all| {
        comments::delete_comment(all, &params.id, &author)
    })
}

pub async fn resolve(
    daemon: &Arc<Daemon>,
    params: &FileCommentResolveParams,
) -> Result<DesignComment> {
    let (row, notes) = notes_of(daemon, &params.workspace)?;
    let author = comments::author(daemon, &row.id, &row.path).await;
    update(&notes, |all| {
        comments::resolve_comment(
            all,
            &params.id,
            params.resolved,
            &author.email,
            comments::now_ms(),
        )
    })
}

pub fn send(daemon: &Arc<Daemon>, workspace: &str, path: &str) -> Result<CommentsSendResult> {
    let target = target(daemon, workspace, path)?;
    let file = Path::new(&target.row.path).join(path);
    let slide = crate::deck::is_slide(path).then(|| slide_label(&file));
    update(&target.notes, |all| {
        Ok(send_comments(
            all,
            path,
            &file,
            slide.as_ref(),
            comments::now_ms(),
        ))
    })
}

/// A slide as its deck names it: "slide 3 (Pricing)", read from the `deck.json` beside it.
fn slide_label(file: &Path) -> String {
    let manifest = file
        .parent()
        .and_then(|dir| std::fs::read_to_string(dir.join(crate::deck::MANIFEST)).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let slides = manifest
        .as_ref()
        .and_then(|m| m.get("slides"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    match slides
        .iter()
        .position(|s| s.get("file").and_then(Value::as_str) == Some(name))
    {
        Some(i) => match slides[i].get("title").and_then(Value::as_str) {
            Some(title) if !title.trim().is_empty() => {
                format!("slide {} ({})", i + 1, title.trim())
            }
            _ => format!("slide {}", i + 1),
        },
        None => "the slide".to_string(),
    }
}

/// After a turn that was sent slide comments: the agent's answer as a reply in each of their
/// threads, marked sent (the agent wrote it). A thread is one of them when its id is in one of
/// the turn's messages, as `send` writes it.
pub fn answer_slide_threads(
    notes: &Path,
    messages: &[String],
    answer: &str,
    agent: &str,
) -> Result<()> {
    if messages.is_empty() || answer.trim().is_empty() || !notes.join(comments::FILE).exists() {
        return Ok(());
    }
    update(notes, |all| {
        let asked: Vec<(String, String)> = all
            .iter()
            .filter(|c| {
                c.reply_to.is_none()
                    && c.sent.is_some()
                    && crate::deck::is_slide(&c.screen)
                    && messages.iter().any(|m| m.contains(&format!("[{}]", c.id)))
            })
            .map(|c| (c.id.clone(), c.screen.clone()))
            .collect();
        let author = chauffeur_protocol::client::CommentAuthor {
            name: agent.to_string(),
            email: "agent".to_string(),
        };
        let text: String = answer.trim().chars().take(ANSWER_MAX).collect();
        let now = comments::now_ms();
        for (id, screen) in asked {
            comments::add_comment(all, &screen, None, &text, Some(&id), &author, now)?;
            let reply = all.last_mut().expect("the reply was just added");
            reply.sent = Some(chauffeur_protocol::client::CommentSent { at: now });
        }
        Ok(())
    })
}

/// The message asking the agent to address the comments of `path` not sent yet (see
/// `comments::send_threads`), naming the file by `file`, its absolute path; marks them sent.
fn send_comments(
    all: &mut [DesignComment],
    path: &str,
    file: &Path,
    slide: Option<&String>,
    now: u64,
) -> CommentsSendResult {
    let mut of_file: Vec<DesignComment> =
        all.iter().filter(|c| c.screen == path).cloned().collect();
    let place = |thread: &DesignComment| match (&thread.pin, thread.quote.as_deref()) {
        (Some(pin), _) => match &pin.element {
            Some(element) if !element.text.trim().is_empty() => {
                format!("on \"{}\" ({})", shortened(&element.text), element.selector)
            }
            Some(element) => format!("on {}", element.selector),
            None => format!("at {}%, {}% of the slide", pin.x.round(), pin.y.round()),
        },
        (None, Some(quote)) => format!("on {}", comments::quoted(&shortened(quote))),
        (None, None) if slide.is_some() => "on the whole slide".to_string(),
        (None, None) => "on the whole file".to_string(),
    };
    let (header, footer) = match slide {
        Some(label) => (
            format!("Comments on {label} of the deck, `{}`:", file.display()),
            "Change that slide where each comment asks for it, then say in a few sentences what you changed, or why you did not: Chauffeur posts your answer in each comment's thread.",
        ),
        None => (
            format!("Comments on `{}`:", file.display()),
            "Address each one: change the file where a comment asks for it, and say in your answer what you changed, or why you did not.",
        ),
    };
    let result = comments::send_threads(&mut of_file, &header, &place, footer, None, now);
    for comment in of_file {
        if let Some(slot) = all.iter_mut().find(|c| c.id == comment.id) {
            *slot = comment;
        }
    }
    result
}

fn shortened(quote: &str) -> String {
    let quote = quote.split_whitespace().collect::<Vec<_>>().join(" ");
    if quote.chars().count() <= QUOTE_IN_MESSAGE {
        return quote;
    }
    let cut: String = quote.chars().take(QUOTE_IN_MESSAGE).collect();
    format!("{}…", cut.trim_end())
}

/// The open chat whose agent last wrote or edited `real` (Claude's Write, Edit and MultiEdit,
/// Codex's patches), read from the workspace's tool calls. Writes through a shell command are not
/// seen.
fn last_writer(daemon: &Daemon, row: &WorkspaceRow, real: &Path) -> Option<String> {
    let name = real.file_name()?.to_str()?;
    let calls = daemon
        .store
        .workspace_tool_calls_mentioning(&row.id, name, WRITES_SCANNED)
        .inspect_err(|error| tracing::warn!(%error, "could not read the workspace's tool calls"))
        .ok()?;
    let open = |chat_id: &str| {
        daemon
            .store
            .chat(chat_id)
            .ok()
            .flatten()
            .is_some_and(|chat| !chat.closed)
    };
    writer_of(calls, &row.path, real, &open)
}

/// The chat of the first of `calls` (newest first) writing `real` whose chat is `open`.
fn writer_of(
    calls: Vec<EventEnvelope>,
    workspace: &str,
    real: &Path,
    open: &dyn Fn(&str) -> bool,
) -> Option<String> {
    calls.into_iter().find_map(|envelope| {
        let Event::Chat { chat_id, event, .. } = envelope.event else {
            return None;
        };
        let ChatEvent::ToolCall { name, input, .. } = event else {
            return None;
        };
        let wrote = written_paths(&name, &input)
            .iter()
            .any(|written| same_file(workspace, written, real));
        (wrote && open(&chat_id)).then_some(chat_id)
    })
}

/// The files a tool call writes, as the agent named them.
fn written_paths(tool: &str, input: &Value) -> Vec<String> {
    match tool {
        "Write" | "Edit" | "MultiEdit" => input
            .get("file_path")
            .and_then(Value::as_str)
            .map(str::to_string)
            .into_iter()
            .collect(),
        "apply_patch" => input
            .get("changes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|change| change.get("path").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether the agent's `written` (absolute, or relative to the workspace folder) is `real`.
fn same_file(workspace: &str, written: &str, real: &Path) -> bool {
    let path = Path::new(workspace).join(written);
    path.canonicalize().is_ok_and(|path| path == real)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::client::CommentAuthor;

    fn me() -> CommentAuthor {
        CommentAuthor {
            name: "Justin".into(),
            email: "justin@example.com".into(),
        }
    }

    #[test]
    fn the_agent_s_answer_lands_in_each_slide_thread_its_turn_was_sent() {
        let tmp = tempfile::tempdir().unwrap();
        let slide = ".chauffeur/artifacts/pitch/02.html";
        let mut all = Vec::new();
        let pin = chauffeur_protocol::client::CommentPin {
            x: 40.0,
            y: 20.0,
            element: Some(chauffeur_protocol::client::PinElement {
                selector: "h1".into(),
                text: "Our plan".into(),
            }),
        };
        let asked = add_comment(
            &mut all,
            slide,
            Anchor {
                quote: None,
                pin: Some(pin),
            },
            "Shorter title",
            None,
            &me(),
            1,
        )
        .unwrap();
        let other =
            add_comment(&mut all, slide, Anchor::default(), "Later", None, &me(), 2).unwrap();
        let file = Path::new("/ws/.chauffeur/artifacts/pitch/02.html");
        let label = "slide 2 (Plan)".to_string();
        let sent = send_comments(&mut all, slide, file, Some(&label), 3);
        assert!(
            sent.message
                .starts_with("Comments on slide 2 (Plan) of the deck")
        );
        assert!(
            sent.message
                .contains(&format!("on \"Our plan\" (h1), Justin [{}]", asked.id))
        );
        comments::write_imported(tmp.path(), Vec::new()).unwrap();
        update(tmp.path(), |stored| {
            *stored = all.clone();
            Ok(())
        })
        .unwrap();
        let message = sent.message.replace(&format!("[{}]", other.id), "");
        answer_slide_threads(tmp.path(), &[message], "Done: shortened it.", "Claude Code").unwrap();
        let (stored, _) = comments::read(tmp.path());
        let replies: Vec<_> = stored.iter().filter(|c| c.reply_to.is_some()).collect();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].reply_to.as_deref(), Some(asked.id.as_str()));
        assert_eq!(replies[0].author.name, "Claude Code");
        assert!(replies[0].sent.is_some());
    }

    #[test]
    fn only_markdown_files_take_comments() {
        assert!(is_markdown(".chauffeur/plans/plan.md"));
        assert!(is_markdown("core/docs/README.MD"));
        assert!(is_markdown("site/page.mdx"));
        assert!(!is_markdown("core/src/main.rs"));
        assert!(!is_markdown("core/md"));
    }

    #[test]
    fn a_reply_stays_on_its_threads_file_and_passage() {
        let mut all = Vec::new();
        let thread = add_comment(
            &mut all,
            ".chauffeur/plans/a.md",
            Anchor {
                quote: Some("Step 2".into()),
                pin: None,
            },
            "Why this order?",
            None,
            &me(),
            1,
        )
        .unwrap();
        assert_eq!(thread.quote.as_deref(), Some("Step 2"));
        assert_eq!(all[0].quote.as_deref(), Some("Step 2"));
        let reply = add_comment(
            &mut all,
            ".chauffeur/plans/a.md",
            Anchor {
                quote: Some("ignored".into()),
                pin: None,
            },
            "Fine",
            Some(&thread.id),
            &me(),
            2,
        )
        .unwrap();
        assert_eq!(reply.quote, None);
        assert_eq!(reply.screen, ".chauffeur/plans/a.md");
        let elsewhere = add_comment(
            &mut all,
            ".chauffeur/plans/b.md",
            Anchor::default(),
            "Here?",
            Some(&thread.id),
            &me(),
            3,
        );
        assert!(matches!(elsewhere, Err(Error::BadRequest(_))));
    }

    #[test]
    fn sending_names_the_file_and_quotes_each_passage_of_that_file_only() {
        let mut all = Vec::new();
        let louis = CommentAuthor {
            name: "Louis".into(),
            email: "louis@example.com".into(),
        };
        let thread = add_comment(
            &mut all,
            ".chauffeur/plans/a.md",
            Anchor {
                quote: Some("Ship   it\non Friday".into()),
                pin: None,
            },
            "Too early",
            None,
            &louis,
            1,
        )
        .unwrap();
        add_comment(
            &mut all,
            ".chauffeur/plans/a.md",
            Anchor::default(),
            "Agreed",
            Some(&thread.id),
            &me(),
            2,
        )
        .unwrap();
        add_comment(
            &mut all,
            ".chauffeur/plans/a.md",
            Anchor::default(),
            "Add a summary",
            None,
            &me(),
            3,
        )
        .unwrap();
        add_comment(
            &mut all,
            "core/README.md",
            Anchor::default(),
            "Not this one",
            None,
            &me(),
            4,
        )
        .unwrap();

        let file = Path::new("/ws/.chauffeur/plans/a.md");
        let sent = send_comments(&mut all, ".chauffeur/plans/a.md", file, None, 10);
        assert_eq!(sent.sent, 3);
        let id = &thread.id;
        let expected = format!(
            "Comments on `/ws/.chauffeur/plans/a.md`:\n\
             - on \"Ship it on Friday\", Louis [{id}]: \"Too early\"\n  \
             - reply from Justin: \"Agreed\"\n\
             - on the whole file, Justin [{}]: \"Add a summary\"\n\n\
             Address each one: change the file where a comment asks for it, and say in your answer what you changed, or why you did not.",
            all[2].id
        );
        assert_eq!(sent.message, expected);
        assert!(all[..3].iter().all(|c| c.sent.is_some()));
        assert!(all[3].sent.is_none(), "another file's comment stays unsent");

        let again = send_comments(&mut all, ".chauffeur/plans/a.md", file, None, 11);
        assert_eq!(again.sent, 0);
        assert!(again.message.is_empty());
    }

    #[test]
    fn a_long_passage_is_shortened_in_the_message() {
        let long = "word ".repeat(200);
        let short = shortened(&long);
        assert!(short.ends_with('…'));
        assert!(short.chars().count() <= QUOTE_IN_MESSAGE + 1);
    }

    fn call(chat: &str, tool: &str, input: Value) -> EventEnvelope {
        EventEnvelope {
            seq: None,
            ts: "2026-11-09T00:00:00Z".into(),
            event: Event::Chat {
                workspace_id: "w".into(),
                chat_id: chat.into(),
                event: ChatEvent::ToolCall {
                    tool_use_id: "t".into(),
                    name: tool.into(),
                    input,
                    parent_tool_use_id: None,
                },
            },
        }
    }

    #[test]
    fn the_writer_is_the_latest_open_chat_whose_agent_wrote_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".chauffeur/plans")).unwrap();
        let plan = workspace.join(".chauffeur/plans/plan.md");
        std::fs::write(&plan, "# Plan").unwrap();
        std::fs::write(workspace.join(".chauffeur/plans/other-plan.md"), "").unwrap();
        let workspace_path = workspace.to_str().unwrap();
        let calls = vec![
            // Newest first: a read, another file, a closed chat, then the writers.
            call("reader", "Read", serde_json::json!({"file_path": plan})),
            call(
                "other",
                "Write",
                serde_json::json!({"file_path": workspace.join(".chauffeur/plans/other-plan.md")}),
            ),
            call("closed", "Edit", serde_json::json!({"file_path": plan})),
            call(
                "codex",
                "apply_patch",
                serde_json::json!({"changes": [{"path": ".chauffeur/plans/plan.md"}]}),
            ),
            call("claude", "Write", serde_json::json!({"file_path": plan})),
        ];
        let open = |chat: &str| chat != "closed";
        assert_eq!(
            writer_of(calls.clone(), workspace_path, &plan, &open).as_deref(),
            Some("codex")
        );
        assert_eq!(
            writer_of(calls[..3].to_vec(), workspace_path, &plan, &open),
            None
        );
    }

    #[test]
    fn claude_and_codex_writes_name_their_files() {
        let write = serde_json::json!({"file_path": "/ws/.chauffeur/plans/a.md", "content": "x"});
        assert_eq!(
            written_paths("Write", &write),
            vec!["/ws/.chauffeur/plans/a.md"]
        );
        let patch = serde_json::json!({"changes": [{"path": "/ws/core/a.md"}, {"path": "b.md"}]});
        assert_eq!(
            written_paths("apply_patch", &patch),
            vec!["/ws/core/a.md", "b.md"]
        );
        assert!(written_paths("Read", &write).is_empty());
    }
}
