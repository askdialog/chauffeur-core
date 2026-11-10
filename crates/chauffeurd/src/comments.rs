//! A design chat's comments (story 51): `comments.json` beside `canvas.json`, written by the
//! daemon for the app (and the share service later) and read by the agent as a file, which only
//! sets `resolved`. The rules (who edits, who resolves, what is sent) are pure functions over
//! the comments; the handlers read the file, apply one and write it back.

use std::path::{Path, PathBuf};

use chauffeur_protocol::client::{
    CommentAddParams, CommentAuthor, CommentEditParams, CommentParams, CommentPin,
    CommentResolveParams, CommentResolved, CommentSent, CommentsResult, CommentsSendParams,
    CommentsSendResult, DesignComment, Mention,
};
use chauffeur_protocol::model::{ChatInfo, ChatKind};
use serde::{Deserialize, Serialize};

use crate::{Daemon, Error, Result};

pub const FILE: &str = "comments.json";
const TEXT_MAX: usize = 20_000;
/// People one comment names, at most.
const MENTIONS_MAX: usize = 20;

#[derive(Serialize, Deserialize, Default)]
struct CommentsFile {
    comments: Vec<DesignComment>,
}

/// The comments of a design folder, and why the file could not be read (none when valid or
/// absent).
pub fn read(dir: &Path) -> (Vec<DesignComment>, Option<String>) {
    match std::fs::read_to_string(dir.join(FILE)) {
        Ok(text) => match serde_json::from_str::<CommentsFile>(&text) {
            Ok(file) => (file.comments, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        },
        Err(_) => (Vec::new(), None),
    }
}

/// Writes the comments whole, through a temporary file, so that the agent never reads half of
/// them.
/// After an import (story 51): the share's comments as the new chat's, none sent (the new
/// chat's agent has read nothing).
pub fn write_imported(dir: &Path, mut comments: Vec<DesignComment>) -> Result<()> {
    for comment in comments.iter_mut() {
        comment.sent = None;
    }
    write(dir, &comments)
}

fn write(dir: &Path, comments: &[DesignComment]) -> Result<()> {
    let text = serde_json::to_string_pretty(&CommentsFile {
        comments: comments.to_vec(),
    })
    .expect("comments serialize");
    let tmp = dir.join(format!(".{FILE}.tmp"));
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, dir.join(FILE)))
        .map_err(|error| Error::Internal(anyhow::anyhow!("writing {FILE}: {error}")))
}

pub(crate) fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

fn new_id() -> String {
    rand::random::<[u8; 8]>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A screen's file as `canvas.json` names it, without a leading `./`.
fn normalized(file: &str) -> String {
    file.trim_start_matches("./").to_string()
}

// --- the rules ---

/// Adds a thread's first message, or a reply to a thread (never to a reply).
pub fn add_comment(
    comments: &mut Vec<DesignComment>,
    screen: &str,
    pin: Option<CommentPin>,
    text: &str,
    reply_to: Option<&str>,
    author: &CommentAuthor,
    now: u64,
) -> Result<DesignComment> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::BadRequest("a comment needs a text".into()));
    }
    if text.len() > TEXT_MAX {
        return Err(Error::BadRequest(format!(
            "a comment is at most {TEXT_MAX} characters"
        )));
    }
    if let Some(pin) = &pin
        && !((0.0..=100.0).contains(&pin.x) && (0.0..=100.0).contains(&pin.y))
    {
        return Err(Error::BadRequest(
            "a pin's x and y are percents of the screen, 0 to 100".into(),
        ));
    }
    let (screen, pin) = match reply_to {
        Some(parent_id) => {
            let parent = comments
                .iter()
                .find(|c| c.id == parent_id)
                .ok_or_else(|| Error::NotFound(format!("no comment {parent_id} to reply to")))?;
            if parent.reply_to.is_some() {
                return Err(Error::BadRequest(
                    "reply to the thread's first comment, not to a reply".into(),
                ));
            }
            (parent.screen.clone(), None)
        }
        None => (normalized(screen), pin),
    };
    let comment = DesignComment {
        id: new_id(),
        screen,
        quote: None,
        pin,
        author: author.clone(),
        at: now,
        text: text.to_string(),
        reply_to: reply_to.map(str::to_string),
        resolved: None,
        sent: None,
        mentions: Vec::new(),
    };
    comments.push(comment.clone());
    Ok(comment)
}

fn find_mut<'a>(comments: &'a mut [DesignComment], id: &str) -> Result<&'a mut DesignComment> {
    comments
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| Error::NotFound(format!("no comment {id}")))
}

/// Who may change a comment's text or remove it: its author, until it was sent to the agent.
fn changeable(comment: &DesignComment, author: &CommentAuthor) -> Result<()> {
    if comment.author.email != author.email {
        return Err(Error::Conflict(format!(
            "only {} can change this comment",
            comment.author.name
        )));
    }
    if comment.sent.is_some() {
        return Err(Error::Conflict(
            "this comment was sent to the agent: reply to it instead".into(),
        ));
    }
    Ok(())
}

pub fn edit_comment(
    comments: &mut [DesignComment],
    id: &str,
    text: &str,
    author: &CommentAuthor,
) -> Result<DesignComment> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::BadRequest("a comment needs a text".into()));
    }
    let comment = find_mut(comments, id)?;
    changeable(comment, author)?;
    comment.text = text.to_string();
    Ok(comment.clone())
}

/// Who a shared design's comment may mention: the people its share is open to.
#[derive(Clone, Debug, Default)]
pub struct Audience {
    /// The owner and the reviewers, with the names the service knows (the owner first).
    pub people: Vec<CommentAuthor>,
    /// The share's team domain, when it is open to one: any of its addresses may be mentioned.
    pub team_domain: Option<String>,
}

impl Audience {
    fn person(&self, email: &str) -> Option<&CommentAuthor> {
        self.people.iter().find(|p| p.email == email)
    }

    fn in_team(&self, email: &str) -> bool {
        self.team_domain
            .as_deref()
            .is_some_and(|team| email.rsplit_once('@').is_some_and(|(_, d)| d == team))
    }
}

/// The e-mail's first name, capitalized: how a person is named until the service knows better.
pub fn first_name(email: &str) -> String {
    let local = email.split('@').next().unwrap_or(email);
    let first = local.split(['.', '_', '-', '+']).next().unwrap_or(local);
    let mut chars = first.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// The mentions a comment of `author` may carry: lowercase, each once, never the author, each
/// someone `audience` is open to (named as the share knows them). None: the design is not
/// shared, and nobody can be mentioned.
pub fn mentions_for(
    asked: &[Mention],
    audience: Option<&Audience>,
    author: &CommentAuthor,
) -> Result<Vec<Mention>> {
    if asked.is_empty() {
        return Ok(Vec::new());
    }
    let Some(audience) = audience else {
        return Err(Error::BadRequest(
            "Only a shared design's comments mention people: share the design first".into(),
        ));
    };
    let mut out: Vec<Mention> = Vec::new();
    for mention in asked {
        let email = mention.email.trim().to_lowercase();
        if email.is_empty() || email == author.email.to_lowercase() {
            continue;
        }
        if out.iter().any(|m| m.email == email) {
            continue;
        }
        let name = match audience.person(&email) {
            Some(person) => person.name.clone(),
            None if audience.in_team(&email) => match mention.name.trim() {
                "" => first_name(&email),
                name => name.to_string(),
            },
            None => {
                return Err(Error::BadRequest(format!(
                    "{email} doesn't have access to this design: add them to its reviewers first"
                )));
            }
        };
        out.push(Mention { email, name });
    }
    if out.len() > MENTIONS_MAX {
        return Err(Error::BadRequest(format!(
            "a comment mentions {MENTIONS_MAX} people at most"
        )));
    }
    Ok(out)
}

/// Sets the people a comment names (already checked by `mentions_for`).
pub fn set_mentions(
    comments: &mut [DesignComment],
    id: &str,
    mentions: Vec<Mention>,
) -> Result<DesignComment> {
    let comment = find_mut(comments, id)?;
    comment.mentions = mentions;
    Ok(comment.clone())
}

/// Removes a comment, and the replies of a thread's first comment with it.
pub fn delete_comment(
    comments: &mut Vec<DesignComment>,
    id: &str,
    author: &CommentAuthor,
) -> Result<()> {
    let comment = find_mut(comments, id)?;
    changeable(comment, author)?;
    if comments
        .iter()
        .any(|c| c.reply_to.as_deref() == Some(id) && c.sent.is_some())
    {
        return Err(Error::Conflict(
            "a reply of this comment was sent to the agent: resolve it instead".into(),
        ));
    }
    comments.retain(|c| c.id != id && c.reply_to.as_deref() != Some(id));
    Ok(())
}

/// Resolves or reopens a thread (its first comment); anyone may.
pub fn resolve_comment(
    comments: &mut [DesignComment],
    id: &str,
    resolved: bool,
    by: &str,
    now: u64,
) -> Result<DesignComment> {
    let comment = find_mut(comments, id)?;
    if comment.reply_to.is_some() {
        return Err(Error::BadRequest(
            "a reply is resolved with its thread".into(),
        ));
    }
    comment.resolved = resolved.then(|| CommentResolved {
        by: by.to_string(),
        at: now,
    });
    Ok(comment.clone())
}

/// The message asking the agent to address every comment not sent yet, grouped by thread (a
/// thread sent before is named again when it has a new reply), and marks them sent. Empty, and
/// nothing marked, when there is nothing new. With `only`, the message holds that thread alone,
/// sent before or not, and marks what of it was new; a reply's id is refused. `labels` gives a
/// screen's label by file.
pub fn send_comments(
    comments: &mut [DesignComment],
    labels: &dyn Fn(&str) -> Option<String>,
    only: Option<&str>,
    now: u64,
) -> Result<CommentsSendResult> {
    if let Some(id) = only {
        let thread = comments
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| Error::NotFound(format!("no comment {id}")))?;
        if thread.reply_to.is_some() {
            return Err(Error::BadRequest(
                "a reply is sent with its thread: send the thread's first comment".into(),
            ));
        }
    }
    let place = |thread: &DesignComment| {
        let label = labels(&thread.screen)
            .map(|l| format!(" ({l})"))
            .unwrap_or_default();
        let place = match &thread.pin {
            Some(pin) => match &pin.element {
                Some(element) if !element.text.trim().is_empty() => format!(
                    "on \"{}\" ({}) at {}%, {}%",
                    element.text.trim(),
                    element.selector,
                    pin.x.round(),
                    pin.y.round()
                ),
                Some(element) => {
                    format!(
                        "on {} at {}%, {}%",
                        element.selector,
                        pin.x.round(),
                        pin.y.round()
                    )
                }
                None => format!("at {}%, {}%", pin.x.round(), pin.y.round()),
            },
            None => "whole screen".to_string(),
        };
        format!("`{}`{label}, {place}", thread.screen)
    };
    Ok(send_threads(
        comments,
        "Comments on the design, from comments.json:",
        &place,
        "Address each one. Once a thread is handled, set \"resolved\": {\"by\": \"agent\", \"at\": <milliseconds since the epoch>} on its entry in comments.json (found by its id) and change nothing else in that file.",
        only,
        now,
    ))
}

/// `send_comments`' message for any comments: `header`, a line per thread starting with what
/// `place` says of it, its replies beneath, then `footer`. With `only`, that thread alone, even
/// when nothing of it is new.
pub fn send_threads(
    comments: &mut [DesignComment],
    header: &str,
    place: &dyn Fn(&DesignComment) -> String,
    footer: &str,
    only: Option<&str>,
    now: u64,
) -> CommentsSendResult {
    let in_scope = |c: &DesignComment| match only {
        Some(id) => c.id == id || c.reply_to.as_deref() == Some(id),
        None => true,
    };
    let new: Vec<String> = comments
        .iter()
        .filter(|c| in_scope(c) && c.sent.is_none())
        .map(|c| c.id.clone())
        .collect();
    if new.is_empty() && only.is_none() {
        return CommentsSendResult {
            message: String::new(),
            sent: 0,
        };
    }
    let mut lines = vec![header.to_string()];
    let threads: Vec<DesignComment> = comments
        .iter()
        .filter(|c| c.reply_to.is_none() && in_scope(c))
        .cloned()
        .collect();
    for thread in threads {
        let replies: Vec<&DesignComment> = comments
            .iter()
            .filter(|c| c.reply_to.as_deref() == Some(&thread.id))
            .collect();
        let thread_new = thread.sent.is_none();
        if only.is_none() && !thread_new && replies.iter().all(|r| r.sent.is_some()) {
            continue;
        }
        let mark = if thread_new { "" } else { " (sent before)" };
        lines.push(format!(
            "- {}, {} [{}]{mark}: {}",
            place(&thread),
            thread.author.name,
            thread.id,
            quoted(&thread.text)
        ));
        for reply in replies {
            let mark = if reply.sent.is_none() {
                ""
            } else {
                " (sent before)"
            };
            lines.push(format!(
                "  - reply from {}{mark}: {}",
                reply.author.name,
                quoted(&reply.text)
            ));
        }
    }
    lines.push(String::new());
    lines.push(footer.to_string());
    let sent = new.len() as u32;
    for comment in comments.iter_mut() {
        if new.contains(&comment.id) {
            comment.sent = Some(CommentSent { at: now });
        }
    }
    CommentsSendResult {
        message: lines.join("\n"),
        sent,
    }
}

pub fn quoted(text: &str) -> String {
    let one_line = text.trim().replace('\n', " ");
    format!("\"{one_line}\"")
}

// --- the handlers ---

struct Folder {
    chat: ChatInfo,
    workspace_path: String,
    dir: PathBuf,
}

fn folder(daemon: &Daemon, chat_key: &str) -> Result<Folder> {
    let chat = daemon.chats.resolve(chat_key)?;
    if chat.kind != ChatKind::Design {
        return Err(Error::BadRequest("only a design chat has comments".into()));
    }
    let workspace = daemon.workspaces.row(&chat.workspace_id)?;
    let dir = crate::design::design_dir(&workspace.path, &chat.id)
        .map_err(|error| Error::Internal(anyhow::anyhow!("the design folder: {error}")))?;
    Ok(Folder {
        chat,
        workspace_path: workspace.path,
        dir: PathBuf::from(dir),
    })
}

/// The user, as their comments are signed: the Office's Google sign-in for the e-mail when there
/// is one, git's `user.email` otherwise; git's `user.name`, else the e-mail's local part. Git's
/// config is read in the workspace's worktrees (by repo name: a repo's own user applies), the
/// first answer kept, then in the workspace's folder (the global config).
pub async fn author(daemon: &Daemon, workspace_id: &str, workspace_path: &str) -> CommentAuthor {
    let git = crate::git::Git::new(daemon.shell_env.clone());
    let mut dirs: Vec<(String, PathBuf)> = daemon
        .store
        .worktrees_of(workspace_id)
        .unwrap_or_default()
        .into_iter()
        .map(|row| (row.repo, PathBuf::from(row.path)))
        .collect();
    dirs.sort();
    let dirs: Vec<PathBuf> = dirs
        .into_iter()
        .map(|(_, path)| path)
        .chain(std::iter::once(PathBuf::from(workspace_path)))
        .collect();
    let config = |key: &'static str| {
        let git = &git;
        let dirs = &dirs;
        async move {
            for dir in dirs {
                if let Ok(value) = git.run(dir, &["config", "--get", key]).await
                    && !value.trim().is_empty()
                {
                    return Some(value);
                }
            }
            None
        }
    };
    let email = daemon
        .store
        .kv_get("office_email")
        .ok()
        .flatten()
        .filter(|email| !email.trim().is_empty());
    let email = match email {
        Some(email) => email,
        None => config("user.email").await.unwrap_or_default(),
    };
    let name = match config("user.name").await {
        Some(name) => name,
        None => email
            .split('@')
            .next()
            .filter(|local| !local.is_empty())
            .unwrap_or("me")
            .to_string(),
    };
    CommentAuthor { name, email }
}

pub async fn list(daemon: &Daemon, chat_key: &str) -> Result<CommentsResult> {
    let folder = folder(daemon, chat_key)?;
    let (comments, error) = read(&folder.dir);
    Ok(CommentsResult {
        chat: folder.chat.id,
        comments,
        author: author(daemon, &folder.chat.workspace_id, &folder.workspace_path).await,
        error,
    })
}

/// Reads the comments, applies `change` and writes them back; an invalid file is never written
/// over.
pub(crate) fn update<T>(
    dir: &Path,
    change: impl FnOnce(&mut Vec<DesignComment>) -> Result<T>,
) -> Result<T> {
    let (mut comments, error) = read(dir);
    if let Some(error) = error {
        return Err(Error::Conflict(format!(
            "{FILE} is invalid ({error}): fix it, or ask the agent to"
        )));
    }
    let result = change(&mut comments)?;
    write(dir, &comments)?;
    Ok(result)
}

pub async fn add(daemon: &Daemon, params: &CommentAddParams) -> Result<DesignComment> {
    let folder = folder(daemon, &params.chat)?;
    if params.reply_to.is_none() {
        let canvas = crate::canvas::read_canvas(&folder.chat.id, &folder.dir, None);
        let wanted = normalized(&params.screen);
        let known = canvas
            .pages
            .iter()
            .flat_map(|page| page.sections.iter())
            .flat_map(|section| section.screens.iter())
            .chain(canvas.unsorted.iter())
            .any(|screen| normalized(&screen.file) == wanted);
        if !known {
            return Err(Error::NotFound(format!(
                "{} is not a screen of this design (chauffeur canvas lists them)",
                params.screen
            )));
        }
    }
    let author = author(daemon, &folder.chat.workspace_id, &folder.workspace_path).await;
    let mentions = mentions_for(
        params.mentions.as_deref().unwrap_or_default(),
        daemon.shares.audience(&folder.chat.id).as_ref(),
        &author,
    )?;
    update(&folder.dir, |comments| {
        let comment = add_comment(
            comments,
            &params.screen,
            params.pin.clone(),
            &params.text,
            params.reply_to.as_deref(),
            &author,
            now_ms(),
        )?;
        if mentions.is_empty() {
            return Ok(comment);
        }
        set_mentions(comments, &comment.id, mentions)
    })
}

pub async fn edit(daemon: &Daemon, params: &CommentEditParams) -> Result<DesignComment> {
    let folder = folder(daemon, &params.chat)?;
    let author = author(daemon, &folder.chat.workspace_id, &folder.workspace_path).await;
    let mentions = match &params.mentions {
        Some(asked) => Some(mentions_for(
            asked,
            daemon.shares.audience(&folder.chat.id).as_ref(),
            &author,
        )?),
        None => None,
    };
    update(&folder.dir, |comments| {
        let comment = edit_comment(comments, &params.id, &params.text, &author)?;
        match mentions {
            Some(mentions) => set_mentions(comments, &comment.id, mentions),
            None => Ok(comment),
        }
    })
}

pub async fn delete(daemon: &Daemon, params: &CommentParams) -> Result<()> {
    let folder = folder(daemon, &params.chat)?;
    let author = author(daemon, &folder.chat.workspace_id, &folder.workspace_path).await;
    update(&folder.dir, |comments| {
        delete_comment(comments, &params.id, &author)
    })
}

pub async fn resolve(daemon: &Daemon, params: &CommentResolveParams) -> Result<DesignComment> {
    let folder = folder(daemon, &params.chat)?;
    let author = author(daemon, &folder.chat.workspace_id, &folder.workspace_path).await;
    update(&folder.dir, |comments| {
        resolve_comment(
            comments,
            &params.id,
            params.resolved,
            &author.email,
            now_ms(),
        )
    })
}

pub async fn send(daemon: &Daemon, params: &CommentsSendParams) -> Result<CommentsSendResult> {
    let folder = folder(daemon, &params.chat)?;
    let canvas = crate::canvas::read_canvas(&folder.chat.id, &folder.dir, None);
    let labels = move |file: &str| {
        let wanted = normalized(file);
        canvas
            .pages
            .iter()
            .flat_map(|page| page.sections.iter())
            .flat_map(|section| section.screens.iter())
            .find(|screen| normalized(&screen.file) == wanted)
            .and_then(|screen| screen.label.clone())
    };
    update(&folder.dir, |comments| {
        send_comments(comments, &labels, params.thread.as_deref(), now_ms())
    })
}

/// A reviewer's comment from the share service, new or changed, kept by its id in its place.
pub fn merge_remote(dir: &Path, comment: DesignComment) -> Result<()> {
    update(dir, |comments| {
        match comments.iter_mut().find(|c| c.id == comment.id) {
            Some(existing) => *existing = comment,
            None => comments.push(comment),
        }
        Ok(())
    })
}

/// A reviewer removed their comment on the share.
pub fn remove_remote(dir: &Path, id: &str) -> Result<()> {
    update(dir, |comments| {
        comments.retain(|c| c.id != id);
        Ok(())
    })
}

/// What a design chat's agent is told about comments, after the design prompt.
pub fn system_prompt() -> &'static str {
    "The user's comments on your screens (and their colleagues', once shared) are in comments.json in your folder: \
each entry names a screen, a point of it with the element that was there, an author and a text; replies carry reply_to. \
Read it when a message asks you to address comments. Once a thread is handled, set \"resolved\": {\"by\": \"agent\", \"at\": <milliseconds since the epoch>} on its entry (found by its id) and change nothing else in that file: never add, edit or remove comments, the user does."
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::client::PinElement;

    fn me() -> CommentAuthor {
        CommentAuthor {
            name: "Alice".into(),
            email: "alice@example.com".into(),
        }
    }

    fn bob() -> CommentAuthor {
        CommentAuthor {
            name: "Bob".into(),
            email: "bob@example.com".into(),
        }
    }

    fn pin(text: &str) -> CommentPin {
        CommentPin {
            x: 48.2,
            y: 83.0,
            element: Some(PinElement {
                selector: "button.cta".into(),
                text: text.into(),
            }),
        }
    }

    #[test]
    fn a_thread_is_added_and_a_reply_follows_it_on_its_screen() {
        let mut comments = Vec::new();
        let thread = add_comment(
            &mut comments,
            "./a/hover.html",
            Some(pin("Contacter")),
            "  trop bas ",
            None,
            &me(),
            1,
        )
        .unwrap();
        assert_eq!(thread.screen, "a/hover.html");
        assert_eq!(thread.text, "trop bas");
        let reply =
            add_comment(&mut comments, "", None, "vu", Some(&thread.id), &bob(), 2).unwrap();
        assert_eq!(reply.screen, "a/hover.html");
        assert!(reply.pin.is_none());
        let nested = add_comment(&mut comments, "", None, "encore", Some(&reply.id), &me(), 3);
        assert!(matches!(nested, Err(Error::BadRequest(_))), "{nested:?}");
        let empty = add_comment(&mut comments, "a.html", None, "  ", None, &me(), 3);
        assert!(matches!(empty, Err(Error::BadRequest(_))), "{empty:?}");
        let off = add_comment(
            &mut comments,
            "a.html",
            Some(CommentPin {
                x: 101.0,
                y: 0.0,
                element: None,
            }),
            "x",
            None,
            &me(),
            3,
        );
        assert!(matches!(off, Err(Error::BadRequest(_))), "{off:?}");
    }

    #[test]
    fn only_the_author_changes_a_comment_and_only_until_it_is_sent() {
        let mut comments = Vec::new();
        let thread = add_comment(&mut comments, "a.html", None, "first", None, &me(), 1).unwrap();
        let other = edit_comment(&mut comments, &thread.id, "changed", &bob());
        assert!(matches!(other, Err(Error::Conflict(_))), "{other:?}");
        assert_eq!(
            edit_comment(&mut comments, &thread.id, "changed", &me())
                .unwrap()
                .text,
            "changed"
        );
        send_comments(&mut comments, &|_| None, None, 2).unwrap();
        let late = edit_comment(&mut comments, &thread.id, "again", &me());
        assert!(matches!(late, Err(Error::Conflict(_))), "{late:?}");
        let gone = delete_comment(&mut comments, &thread.id, &me());
        assert!(matches!(gone, Err(Error::Conflict(_))), "{gone:?}");
    }

    #[test]
    fn deleting_a_thread_takes_its_replies_unless_one_was_sent() {
        let mut comments = Vec::new();
        let thread = add_comment(&mut comments, "a.html", None, "first", None, &me(), 1).unwrap();
        add_comment(
            &mut comments,
            "",
            None,
            "reply",
            Some(&thread.id),
            &bob(),
            2,
        )
        .unwrap();
        delete_comment(&mut comments, &thread.id, &me()).unwrap();
        assert!(comments.is_empty());
        let thread = add_comment(&mut comments, "a.html", None, "first", None, &me(), 3).unwrap();
        send_comments(&mut comments, &|_| None, None, 4).unwrap();
        let late = add_comment(&mut comments, "a.html", None, "late", None, &me(), 5).unwrap();
        add_comment(&mut comments, "", None, "reply", Some(&late.id), &bob(), 6).unwrap();
        send_comments(&mut comments, &|_| None, None, 7).unwrap();
        let kept = delete_comment(&mut comments, &late.id, &me());
        assert!(matches!(kept, Err(Error::Conflict(_))), "{kept:?}");
        assert_eq!(comments.len(), 3);
        let _ = thread;
    }

    #[test]
    fn anyone_resolves_a_thread_and_reopens_it_but_not_a_reply() {
        let mut comments = Vec::new();
        let thread = add_comment(&mut comments, "a.html", None, "first", None, &me(), 1).unwrap();
        let reply = add_comment(
            &mut comments,
            "",
            None,
            "reply",
            Some(&thread.id),
            &bob(),
            2,
        )
        .unwrap();
        let resolved =
            resolve_comment(&mut comments, &thread.id, true, "bob@example.com", 3).unwrap();
        assert_eq!(resolved.resolved.as_ref().unwrap().by, "bob@example.com");
        assert!(
            resolve_comment(&mut comments, &thread.id, false, "agent", 4)
                .unwrap()
                .resolved
                .is_none()
        );
        let on_reply = resolve_comment(&mut comments, &reply.id, true, "agent", 5);
        assert!(
            matches!(on_reply, Err(Error::BadRequest(_))),
            "{on_reply:?}"
        );
    }

    #[test]
    fn sending_names_the_new_comments_by_thread_and_marks_them_sent_once() {
        let mut comments = Vec::new();
        let hover = add_comment(
            &mut comments,
            "a/hover.html",
            Some(pin("Contacter un conseiller")),
            "trop bas,\nremonte-le",
            None,
            &me(),
            1,
        )
        .unwrap();
        add_comment(
            &mut comments,
            "b/gone.html",
            None,
            "contraste",
            None,
            &me(),
            2,
        )
        .unwrap();
        let labels = |file: &str| (file == "a/hover.html").then(|| "Survol".to_string());
        let first = send_comments(&mut comments, &labels, None, 10).unwrap();
        assert_eq!(first.sent, 2);
        assert!(
            first.message.contains(
                "- `a/hover.html` (Survol), on \"Contacter un conseiller\" (button.cta) at 48%, 83%, Alice ["
            ),
            "{}",
            first.message
        );
        assert!(
            first.message.contains("]: \"trop bas, remonte-le\""),
            "{}",
            first.message
        );
        assert!(
            first
                .message
                .contains("- `b/gone.html`, whole screen, Alice ["),
            "{}",
            first.message
        );
        assert!(
            first.message.contains("set \"resolved\""),
            "{}",
            first.message
        );
        assert!(comments.iter().all(|c| c.sent.is_some()));

        let nothing = send_comments(&mut comments, &labels, None, 11).unwrap();
        assert_eq!((nothing.sent, nothing.message.as_str()), (0, ""));

        add_comment(
            &mut comments,
            "",
            None,
            "fait ?",
            Some(&hover.id),
            &bob(),
            12,
        )
        .unwrap();
        let again = send_comments(&mut comments, &labels, None, 13).unwrap();
        assert_eq!(again.sent, 1);
        assert!(
            again
                .message
                .contains("(sent before): \"trop bas, remonte-le\""),
            "{}",
            again.message
        );
        assert!(
            again.message.contains("  - reply from Bob: \"fait ?\""),
            "{}",
            again.message
        );
        assert!(!again.message.contains("b/gone.html"), "{}", again.message);
    }

    #[test]
    fn sending_one_thread_names_it_alone_even_sent_before_and_leaves_the_others_new() {
        let mut comments = Vec::new();
        let hover = add_comment(&mut comments, "a.html", None, "trop bas", None, &me(), 1).unwrap();
        let reply = add_comment(
            &mut comments,
            "",
            None,
            "d'accord",
            Some(&hover.id),
            &bob(),
            2,
        )
        .unwrap();
        let other =
            add_comment(&mut comments, "b.html", None, "contraste", None, &me(), 3).unwrap();
        let labels = |_: &str| None;

        let one = send_comments(&mut comments, &labels, Some(&hover.id), 10).unwrap();
        assert_eq!(one.sent, 2);
        assert!(one.message.contains("\"trop bas\""), "{}", one.message);
        assert!(
            one.message.contains("  - reply from Bob: \"d'accord\""),
            "{}",
            one.message
        );
        assert!(!one.message.contains("b.html"), "{}", one.message);
        let other_now = comments.iter().find(|c| c.id == other.id).unwrap();
        assert!(other_now.sent.is_none());

        let again = send_comments(&mut comments, &labels, Some(&hover.id), 11).unwrap();
        assert_eq!(again.sent, 0);
        assert!(
            again.message.contains("(sent before): \"trop bas\""),
            "{}",
            again.message
        );

        let rest = send_comments(&mut comments, &labels, None, 12).unwrap();
        assert_eq!(rest.sent, 1);
        assert!(!rest.message.contains("a.html"), "{}", rest.message);

        let on_reply = send_comments(&mut comments, &labels, Some(&reply.id), 13);
        assert!(
            matches!(on_reply, Err(Error::BadRequest(_))),
            "{on_reply:?}"
        );
        let unknown = send_comments(&mut comments, &labels, Some("nope"), 13);
        assert!(matches!(unknown, Err(Error::NotFound(_))), "{unknown:?}");
    }

    fn mention(email: &str, name: &str) -> Mention {
        Mention {
            email: email.into(),
            name: name.into(),
        }
    }

    fn audience() -> Audience {
        Audience {
            people: vec![
                me(),
                CommentAuthor {
                    name: "Bob Martin".into(),
                    email: "bob@example.com".into(),
                },
            ],
            team_domain: Some("example.org".into()),
        }
    }

    #[test]
    fn a_comment_mentions_only_those_the_share_is_open_to() {
        let asked = [
            mention(" Bob@Example.com ", "Bobby"),
            mention("bob@example.com", "Bob"),
            mention("alice@example.com", "Alice"),
            mention("dan@example.org", ""),
            mention("eve@example.org", "Eve L."),
        ];
        let kept = mentions_for(&asked, Some(&audience()), &me()).unwrap();
        assert_eq!(
            kept,
            [
                mention("bob@example.com", "Bob Martin"),
                mention("dan@example.org", "Dan"),
                mention("eve@example.org", "Eve L."),
            ],
            "lowercase, once, never the author, named as the share knows them"
        );
        let outsider = mentions_for(
            &[mention("carol@other.example", "Carol")],
            Some(&audience()),
            &me(),
        );
        assert!(
            matches!(&outsider, Err(Error::BadRequest(m)) if m == "carol@other.example doesn't have access to this design: add them to its reviewers first"),
            "{outsider:?}"
        );
        let unshared = mentions_for(&[mention("bob@example.com", "Bob")], None, &me());
        assert!(
            matches!(&unshared, Err(Error::BadRequest(m)) if m.starts_with("Only a shared design")),
            "{unshared:?}"
        );
        assert!(mentions_for(&[], None, &me()).unwrap().is_empty());
        let crowd: Vec<Mention> = (0..=MENTIONS_MAX)
            .map(|i| mention(&format!("p{i}@example.org"), ""))
            .collect();
        assert!(mentions_for(&crowd, Some(&audience()), &me()).is_err());
    }

    #[test]
    fn mentions_are_kept_on_the_comment_and_back_from_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut comments = Vec::new();
        let thread =
            add_comment(&mut comments, "a.html", None, "@Bob look", None, &me(), 1).unwrap();
        let with = set_mentions(
            &mut comments,
            &thread.id,
            vec![mention("bob@example.com", "Bob")],
        )
        .unwrap();
        assert_eq!(with.mentions.len(), 1);
        // An edit of the text alone keeps them (the handler sets them only when given).
        let edited = edit_comment(&mut comments, &thread.id, "@Bob look again", &me()).unwrap();
        assert_eq!(edited.mentions, with.mentions);
        write(dir.path(), &comments).unwrap();
        assert_eq!(read(dir.path()).0, comments);
        // A reviewer's comment from the service keeps its mentions.
        let remote = DesignComment {
            id: "r1".into(),
            author: bob(),
            mentions: vec![mention("alice@example.com", "Alice")],
            ..thread.clone()
        };
        merge_remote(dir.path(), remote.clone()).unwrap();
        assert!(read(dir.path()).0.contains(&remote));
        // Without mentions, the file has no `mentions` key: older readers see what they knew.
        let plain = add_comment(&mut comments, "a.html", None, "plain", None, &me(), 2).unwrap();
        assert!(!serde_json::to_string(&plain).unwrap().contains("mentions"));
    }

    #[test]
    fn the_file_is_read_back_as_written_and_an_invalid_one_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let mut comments = Vec::new();
        add_comment(
            &mut comments,
            "a.html",
            Some(pin("x")),
            "first",
            None,
            &me(),
            1,
        )
        .unwrap();
        write(dir.path(), &comments).unwrap();
        let (back, error) = read(dir.path());
        assert_eq!((back, error), (comments, None));
        std::fs::write(dir.path().join(FILE), "{\"comments\": [").unwrap();
        let (none, error) = read(dir.path());
        assert!(none.is_empty() && error.is_some());
        let refused = update(dir.path(), |_| Ok(()));
        assert!(matches!(refused, Err(Error::Conflict(_))), "{refused:?}");
    }
}
