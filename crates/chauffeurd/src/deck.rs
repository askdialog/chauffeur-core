//! Slide decks (docs/adr/slide-decks.md): a deliverable folder holding `deck.json` and one HTML
//! file per slide. The reader changes a deck only through here: the daemon writes `deck.json` and
//! the slides, keeps a version, and tells the deck's chat at its next message what the user
//! changed by hand.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chauffeur_protocol::deck::{
    DeckChange, DeckChangeParams, DeckEditTextParams, DeckParams, DeckResult, DeckSlide,
    VersionOrigin,
};
use serde::{Deserialize, Serialize};

use crate::versions;
use crate::{Daemon, Error, Result};

pub const MANIFEST: &str = "deck.json";
const DEFAULT_WIDTH: u32 = 1920;
const DEFAULT_HEIGHT: u32 = 1080;
/// Slides a deck may hold.
const SLIDES_MAX: usize = 200;
/// Characters a retyped text may hold.
const TEXT_MAX: usize = 5_000;
/// Added to a slide's URL for the reader: the daemon appends the script for comments and editing.
pub const READER_QUERY: &str = "deck=1";

/// What the user changed by hand in a chat's decks since its agent last read a message.
static HAND_EDITS: Mutex<Option<HashMap<String, Vec<String>>>> = Mutex::new(None);

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Manifest {
    #[serde(default)]
    title: String,
    #[serde(default = "default_width")]
    width: u32,
    #[serde(default = "default_height")]
    height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    brand: Option<String>,
    slides: Vec<Entry>,
    /// What the agent wrote that the daemon does not read, kept as it was.
    #[serde(flatten)]
    other: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Entry {
    file: String,
    #[serde(default)]
    title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    notes: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    skip: bool,
    #[serde(flatten)]
    other: serde_json::Map<String, serde_json::Value>,
}

fn default_width() -> u32 {
    DEFAULT_WIDTH
}

fn default_height() -> u32 {
    DEFAULT_HEIGHT
}

/// A deck as the daemon finds it.
pub struct Deck {
    pub workspace_id: String,
    pub workspace_path: PathBuf,
    /// `deck.json`'s path as `files.read` names it.
    pub path: String,
    /// The deck's folder on disk, and as `files.read` names it.
    pub dir: PathBuf,
    pub dir_path: String,
    /// The deliverable folder its versions are kept in.
    pub folder: PathBuf,
}

/// Whether `path` names a slide or any HTML file of a deliverable: it takes pinned comments.
pub fn is_slide(path: &str) -> bool {
    versions::folder_of(Path::new("/"), path).is_some()
        && path.to_ascii_lowercase().ends_with(".html")
}

pub fn find(daemon: &Arc<Daemon>, workspace: &str, path: &str) -> Result<Deck> {
    if Path::new(path).file_name().and_then(|n| n.to_str()) != Some(MANIFEST) {
        return Err(Error::BadRequest(format!(
            "{path} is not a deck: a deck is named by its {MANIFEST}"
        )));
    }
    let (row, roots) = crate::rpc::file_roots(daemon, workspace)?;
    let workspace_path = PathBuf::from(&row.path);
    let folder = versions::folder_of(&workspace_path, path).ok_or_else(|| {
        Error::BadRequest(format!(
            "{path} is not under .chauffeur/artifacts/<deck>/: decks live there"
        ))
    })?;
    let real = crate::files::resolve(&roots, path)?;
    let dir = real.parent().expect("a file has a folder").to_path_buf();
    // Compared with the resolved file: the workspace's path may go through a symlink (/tmp on
    // macOS).
    let folder = folder.canonicalize().unwrap_or(folder);
    if !dir.starts_with(&folder) {
        return Err(Error::BadRequest(format!(
            "{path} is not inside its deliverable's folder"
        )));
    }
    let dir_path = Path::new(path)
        .parent()
        .expect("checked to be under artifacts")
        .to_string_lossy()
        .into_owned();
    Ok(Deck {
        workspace_id: row.id,
        workspace_path,
        path: path.to_string(),
        dir,
        dir_path,
        folder,
    })
}

fn read_manifest(dir: &Path) -> Result<Manifest> {
    let text = std::fs::read_to_string(dir.join(MANIFEST))
        .map_err(|error| Error::NotFound(format!("{MANIFEST}: {error}")))?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|error| {
        Error::BadRequest(format!(
            "{MANIFEST} is invalid ({error}): ask the agent to fix it"
        ))
    })?;
    if manifest.slides.len() > SLIDES_MAX {
        return Err(Error::BadRequest(format!(
            "a deck holds {SLIDES_MAX} slides at most"
        )));
    }
    if let Some(bad) = manifest
        .slides
        .iter()
        .find(|s| chauffeur_design::safe_relative(&s.file).is_none())
    {
        return Err(Error::BadRequest(format!(
            "{MANIFEST} names {:?}, which is not a file of the deck's folder",
            bad.file
        )));
    }
    Ok(manifest)
}

fn write_manifest(dir: &Path, manifest: &Manifest) -> Result<()> {
    let text = serde_json::to_string_pretty(manifest).expect("deck.json serializes");
    let tmp = dir.join(format!(".{MANIFEST}.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, dir.join(MANIFEST))?;
    Ok(())
}

fn file_version(path: &Path) -> String {
    std::fs::metadata(path)
        .ok()
        .map(|m| {
            let modified = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis());
            format!("{modified}-{}", m.len())
        })
        .unwrap_or_default()
}

/// The deck, or one of its earlier versions, with the URL each slide is framed at.
pub fn read(daemon: &Arc<Daemon>, params: &DeckParams) -> Result<DeckResult> {
    let deck = find(daemon, &params.workspace, &params.path)?;
    let (dir, dir_path) = match params.version {
        None => (deck.dir.clone(), deck.dir_path.clone()),
        Some(n) => {
            let version = versions::version_dir(&deck.folder, n)?;
            let inside = deck
                .dir
                .strip_prefix(&deck.folder)
                .map_err(|_| Error::BadRequest(format!("{} is not in its folder", deck.path)))?
                .to_path_buf();
            let folder_path = Path::new(&deck.dir_path)
                .ancestors()
                .nth(inside.components().count())
                .expect("the deck's path holds its folder")
                .to_path_buf();
            (
                version.join(&inside),
                folder_path
                    .join(versions::VERSIONS_DIR)
                    .join(n.to_string())
                    .join(&inside)
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    };
    let manifest = read_manifest(&dir)?;
    Ok(result(
        daemon,
        &deck,
        &dir,
        &dir_path,
        manifest,
        params.version,
    ))
}

fn result(
    daemon: &Daemon,
    deck: &Deck,
    dir: &Path,
    dir_path: &str,
    manifest: Manifest,
    version: Option<u32>,
) -> DeckResult {
    let brand_changed = version.is_none()
        && manifest.brand.as_deref().is_some_and(|id| {
            crate::brands::dir(daemon, id).is_ok_and(|brand| {
                versions::digest_of(&brand, &crate::brands::COPIED)
                    != versions::digest_of(
                        &dir.join(crate::brands::DELIVERABLE_BRAND_DIR),
                        &crate::brands::COPIED,
                    )
            })
        });
    let slides = manifest
        .slides
        .into_iter()
        .map(|entry| {
            let file = dir.join(&entry.file);
            let present = file.is_file();
            let url = present
                .then(|| {
                    daemon
                        .designs
                        .file_url(&deck.workspace_id, &format!("{dir_path}/{}", entry.file))
                })
                .flatten()
                .map(|url| format!("{url}?{READER_QUERY}"));
            DeckSlide {
                title: entry.title,
                notes: entry.notes,
                skip: entry.skip,
                url,
                version: file_version(&file),
                file: entry.file,
            }
        })
        .collect();
    DeckResult {
        path: deck.path.clone(),
        title: manifest.title,
        width: manifest.width.clamp(320, 7680),
        height: manifest.height.clamp(180, 4320),
        brand: manifest.brand,
        brand_changed,
        slides,
        version,
    }
}

fn current(daemon: &Daemon, deck: &Deck) -> Result<DeckResult> {
    let manifest = read_manifest(&deck.dir)?;
    Ok(result(
        daemon,
        deck,
        &deck.dir,
        &deck.dir_path,
        manifest,
        None,
    ))
}

/// After a change by hand: a version, and a line for the deck's chat.
fn changed_by_hand(deck: &Deck, chat: Option<&str>, what: String) {
    if let Err(error) = versions::snapshot(&deck.folder, VersionOrigin::Hand) {
        tracing::warn!(%error, deck = deck.path, "could not keep a version of the deck");
    }
    if let Some(chat) = chat {
        let mut edits = HAND_EDITS.lock().unwrap();
        edits
            .get_or_insert_with(HashMap::new)
            .entry(chat.to_string())
            .or_default()
            .push(format!("{} — {what}", deck.path));
    }
}

/// A message followed by what the user changed by hand in the chat's decks since the agent's
/// last message, so that it keeps those changes; the list is then forgotten.
pub fn with_hand_edits(chat: &str, text: &str) -> String {
    let edits = HAND_EDITS
        .lock()
        .unwrap()
        .as_mut()
        .and_then(|all| all.remove(chat))
        .unwrap_or_default();
    if edits.is_empty() {
        return text.to_string();
    }
    let lines: Vec<String> = edits.iter().map(|e| format!("- {e}")).collect();
    format!(
        "{text}\n\n(Chauffeur: since your last message, the user changed decks by hand; keep these changes, read the files again before editing them:\n{})",
        lines.join("\n")
    )
}

fn label(manifest: &Manifest, file: &str) -> String {
    match manifest.slides.iter().position(|s| s.file == file) {
        Some(i) if !manifest.slides[i].title.is_empty() => {
            format!("slide {} ({})", i + 1, manifest.slides[i].title)
        }
        Some(i) => format!("slide {}", i + 1),
        None => file.to_string(),
    }
}

pub fn change(daemon: &Arc<Daemon>, params: &DeckChangeParams) -> Result<DeckResult> {
    let deck = find(daemon, &params.workspace, &params.path)?;
    let mut manifest = read_manifest(&deck.dir)?;
    let position = |manifest: &Manifest, file: &str| {
        manifest
            .slides
            .iter()
            .position(|s| s.file == file)
            .ok_or_else(|| Error::NotFound(format!("no slide {file} in this deck")))
    };
    let what = match &params.change {
        DeckChange::Reorder { files } => {
            let mut sorted = files.clone();
            sorted.sort();
            let mut present: Vec<String> = manifest.slides.iter().map(|s| s.file.clone()).collect();
            present.sort();
            if sorted != present {
                return Err(Error::Conflict(
                    "the deck's slides changed meanwhile: reorder them again".into(),
                ));
            }
            manifest
                .slides
                .sort_by_key(|s| files.iter().position(|f| *f == s.file));
            "reordered the slides".to_string()
        }
        DeckChange::Skip { file, skip } => {
            let i = position(&manifest, file)?;
            manifest.slides[i].skip = *skip;
            let what = if *skip { "skipped" } else { "showed again" };
            format!("{what} {}", label(&manifest, file))
        }
        DeckChange::Notes { file, notes } => {
            let i = position(&manifest, file)?;
            if notes.chars().count() > TEXT_MAX * 4 {
                return Err(Error::BadRequest("these notes are too long".into()));
            }
            manifest.slides[i].notes = notes.clone();
            format!("changed the speaker notes of {}", label(&manifest, file))
        }
        DeckChange::Duplicate { file } => {
            let i = position(&manifest, file)?;
            if manifest.slides.len() >= SLIDES_MAX {
                return Err(Error::BadRequest(format!(
                    "a deck holds {SLIDES_MAX} slides at most"
                )));
            }
            let copy = copy_name(&deck.dir, file);
            std::fs::copy(deck.dir.join(file), deck.dir.join(&copy))?;
            let mut entry = manifest.slides[i].clone();
            entry.file = copy;
            manifest.slides.insert(i + 1, entry);
            format!("duplicated {}", label(&manifest, file))
        }
        DeckChange::Delete { file } => {
            let i = position(&manifest, file)?;
            if manifest.slides.len() == 1 {
                return Err(Error::BadRequest(
                    "a deck keeps one slide at least: ask the agent to remove the deck".into(),
                ));
            }
            let what = format!("deleted {}", label(&manifest, file));
            manifest.slides.remove(i);
            if !manifest.slides.iter().any(|s| s.file == *file) {
                let _ = std::fs::remove_file(deck.dir.join(file));
            }
            what
        }
    };
    write_manifest(&deck.dir, &manifest)?;
    changed_by_hand(&deck, params.chat.as_deref(), what);
    current(daemon, &deck)
}

/// A free name beside `file` for its copy: `intro-copy.html`, `intro-copy-2.html`…
fn copy_name(dir: &Path, file: &str) -> String {
    let (stem, ext) = match file.rsplit_once('.') {
        Some((stem, ext)) => (stem, format!(".{ext}")),
        None => (file, String::new()),
    };
    (1..)
        .map(|n| match n {
            1 => format!("{stem}-copy{ext}"),
            n => format!("{stem}-copy-{n}{ext}"),
        })
        .find(|name| !dir.join(name).exists())
        .expect("some name is free")
}

pub fn edit_text(daemon: &Arc<Daemon>, params: &DeckEditTextParams) -> Result<DeckResult> {
    let deck = find(daemon, &params.workspace, &params.path)?;
    let manifest = read_manifest(&deck.dir)?;
    if !manifest.slides.iter().any(|s| s.file == params.slide) {
        return Err(Error::NotFound(format!(
            "no slide {} in this deck",
            params.slide
        )));
    }
    if params.html.chars().count() > TEXT_MAX {
        return Err(Error::BadRequest(format!(
            "a text holds {TEXT_MAX} characters at most"
        )));
    }
    let file = deck.dir.join(&params.slide);
    let source = std::fs::read_to_string(&file)?;
    let edited = html::replace_text(
        &source,
        &params.tag.to_ascii_lowercase(),
        params.index as usize,
        &params.before,
        &params.html,
    )?;
    let tmp = deck
        .dir
        .join(format!(".{}.tmp", crate::slug::slugify(&params.slide, 60)));
    std::fs::write(&tmp, edited)?;
    std::fs::rename(&tmp, &file)?;
    let before = html::normalized(&params.before);
    let after = html::normalized(&html::text_of(&html::sanitized(&params.html)));
    changed_by_hand(
        &deck,
        params.chat.as_deref(),
        format!(
            "retyped a text of {}: \"{before}\" became \"{after}\"",
            label(&manifest, &params.slide)
        ),
    );
    current(daemon, &deck)
}

/// Copies the deck's brand again from the brand's folder: its `brand.css` and `assets/`.
pub fn apply_brand(daemon: &Arc<Daemon>, params: &DeckParams) -> Result<DeckResult> {
    let deck = find(daemon, &params.workspace, &params.path)?;
    let manifest = read_manifest(&deck.dir)?;
    let id = manifest
        .brand
        .as_deref()
        .ok_or_else(|| Error::BadRequest("this deck was not made with a brand".into()))?;
    let brand = crate::brands::dir(daemon, id)?;
    let target = deck.dir.join(crate::brands::DELIVERABLE_BRAND_DIR);
    for name in crate::brands::COPIED {
        let from = brand.join(name);
        let to = target.join(name);
        if to.is_dir() {
            std::fs::remove_dir_all(&to)?;
        } else if to.exists() {
            std::fs::remove_file(&to)?;
        }
        copy_tree(&from, &to)?;
    }
    changed_by_hand(&deck, None, format!("applied the brand {id} again"));
    current(daemon, &deck)
}

/// Copies a file, or a folder's regular files (never through a symlink).
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let Ok(meta) = std::fs::symlink_metadata(from) else {
        return Ok(());
    };
    if meta.is_file() {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to)?;
    } else if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)?.flatten() {
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// Retyping a text in a slide's source: the elements of an HTML file with where their content
/// starts and ends, found without a full parser (agents write well-formed slides; what this
/// misreads is refused, never written).
pub mod html {
    use crate::{Error, Result};

    const VOID: [&str; 14] = [
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr",
    ];
    const RAW: [&str; 4] = ["script", "style", "textarea", "title"];
    /// Opening one of these closes an open one of the same tag, as browsers do.
    const SELF_CLOSING_SIBLINGS: [&str; 8] = ["p", "li", "dt", "dd", "tr", "td", "th", "option"];
    /// What a retyped text may hold.
    const INLINE: [&str; 5] = ["b", "strong", "i", "em", "br"];

    #[derive(Debug, Clone)]
    pub struct Element {
        pub tag: String,
        /// Where its content starts (after its start tag) and ends (before its end tag).
        pub start: usize,
        pub end: usize,
    }

    /// Where the `>` ending a tag that starts at `from` is, skipping quoted attribute values.
    fn tag_end(src: &str, from: usize) -> Option<usize> {
        let bytes = src.as_bytes();
        let mut quote: Option<u8> = None;
        for (i, &b) in bytes.iter().enumerate().skip(from) {
            match quote {
                Some(q) if b == q => quote = None,
                Some(_) => {}
                None if b == b'"' || b == b'\'' => quote = Some(b),
                None if b == b'>' => return Some(i),
                None => {}
            }
        }
        None
    }

    fn name_at(src: &str, from: usize) -> String {
        src[from..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect::<String>()
            .to_ascii_lowercase()
    }

    /// The elements of `src` in the order their start tags come.
    pub fn elements(src: &str) -> Vec<Element> {
        let mut out: Vec<Element> = Vec::new();
        let mut open: Vec<usize> = Vec::new();
        let mut i = 0;
        while let Some(found) = src[i..].find('<') {
            let at = i + found;
            let rest = &src[at..];
            if rest.starts_with("<!--") {
                i = src[at + 4..]
                    .find("-->")
                    .map_or(src.len(), |e| at + 4 + e + 3);
                continue;
            }
            if rest.starts_with("<!") || rest.starts_with("<?") {
                i = tag_end(src, at).map_or(src.len(), |e| e + 1);
                continue;
            }
            if let Some(stripped) = rest.strip_prefix("</") {
                let name = name_at(stripped, 0);
                if let Some(depth) = open.iter().rposition(|&e| out[e].tag == name) {
                    for &e in &open[depth..] {
                        out[e].end = at;
                    }
                    open.truncate(depth);
                }
                i = tag_end(src, at).map_or(src.len(), |e| e + 1);
                continue;
            }
            let name = name_at(src, at + 1);
            if name.is_empty() {
                i = at + 1;
                continue;
            }
            let Some(close) = tag_end(src, at) else {
                break;
            };
            let start = close + 1;
            if SELF_CLOSING_SIBLINGS.contains(&name.as_str())
                && let Some(&top) = open.last()
                && out[top].tag == name
            {
                out[top].end = at;
                open.pop();
            }
            if VOID.contains(&name.as_str()) || src[..close].ends_with('/') {
                out.push(Element {
                    tag: name,
                    start,
                    end: start,
                });
                i = start;
            } else if RAW.contains(&name.as_str()) {
                let lower = src[start..].to_ascii_lowercase();
                let end = lower
                    .find(&format!("</{name}"))
                    .map_or(src.len(), |e| start + e);
                out.push(Element {
                    tag: name,
                    start,
                    end,
                });
                i = tag_end(src, end).map_or(src.len(), |e| e + 1);
            } else {
                open.push(out.len());
                out.push(Element {
                    tag: name,
                    start,
                    end: src.len(),
                });
                i = start;
            }
        }
        out
    }

    fn decode(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find('&') {
            out.push_str(&rest[..at]);
            let tail = &rest[at..];
            let Some(end) = tail[..tail.len().min(12)].find(';') else {
                out.push('&');
                rest = &tail[1..];
                continue;
            };
            let entity = &tail[1..end];
            let decoded = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            match decoded {
                Some(c) => {
                    out.push(c);
                    rest = &tail[end + 1..];
                }
                None => {
                    out.push('&');
                    rest = &tail[1..];
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// The text of some markup, as a browser's `textContent` reads it.
    pub fn text_of(markup: &str) -> String {
        let mut out = String::new();
        let mut rest = markup;
        while let Some(at) = rest.find('<') {
            out.push_str(&rest[..at]);
            rest = match rest[at..].find('>') {
                Some(end) => &rest[at + end + 1..],
                None => "",
            };
        }
        out.push_str(rest);
        decode(&out)
    }

    /// Whitespace collapsed, ends trimmed: how two texts are compared.
    pub fn normalized(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The tags of some markup, lowercase, with whether they carry attributes.
    fn tags(markup: &str) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        let mut rest = markup;
        while let Some(at) = rest.find('<') {
            let tail = &rest[at + 1..];
            let end = tail.find('>').unwrap_or(tail.len());
            let inner = tail[..end].trim_start_matches('/').trim_end_matches('/');
            let name = name_at(inner, 0);
            out.push((name.clone(), inner.trim().len() > name.len()));
            rest = &tail[end.min(tail.len())..];
        }
        out
    }

    /// Markup holding only text and bare `<b>`, `<strong>`, `<i>`, `<em>` and `<br>`: every other
    /// tag is dropped (its text kept), a lone `<` or `>` escaped.
    pub fn sanitized(markup: &str) -> String {
        let mut out = String::new();
        let mut rest = markup;
        while let Some(at) = rest.find('<') {
            push_text(&mut out, &rest[..at]);
            let tail = &rest[at..];
            let Some(end) = tail.find('>') else {
                push_text(&mut out, tail);
                rest = "";
                break;
            };
            let tag = &tail[1..end];
            let closing = tag.starts_with('/');
            let name = name_at(tag.trim_start_matches('/'), 0);
            if INLINE.contains(&name.as_str()) {
                out.push_str(&match (name.as_str(), closing) {
                    ("br", _) => "<br>".to_string(),
                    (name, true) => format!("</{name}>"),
                    (name, false) => format!("<{name}>"),
                });
            } else if !closing && matches!(name.as_str(), "div" | "p") && !out.is_empty() {
                // A line the browser's editor started with a block: a line break.
                out.push_str("<br>");
            }
            rest = &tail[end + 1..];
        }
        push_text(&mut out, rest);
        out
    }

    fn push_text(out: &mut String, text: &str) {
        for c in text.chars() {
            match c {
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                c => out.push(c),
            }
        }
    }

    /// `src` with the content of the `index`-th `tag` element replaced by `html` (sanitized),
    /// provided it still reads `before`; else the one element of that tag reading `before`.
    /// Refused when none or several do, or when the element holds more than inline text.
    pub fn replace_text(
        src: &str,
        tag: &str,
        index: usize,
        before: &str,
        html: &str,
    ) -> Result<String> {
        let all = elements(src);
        let of_tag: Vec<&Element> = all.iter().filter(|e| e.tag == tag).collect();
        let before = normalized(before);
        let reads = |e: &Element| normalized(&text_of(&src[e.start..e.end])) == before;
        let element = match of_tag.get(index) {
            Some(e) if reads(e) => *e,
            _ => {
                let matching: Vec<&&Element> = of_tag.iter().filter(|e| reads(e)).collect();
                match matching.as_slice() {
                    [one] => **one,
                    [] => {
                        return Err(Error::Conflict(
                            "This slide changed since it was shown: try again".into(),
                        ));
                    }
                    _ => {
                        return Err(Error::Conflict(
                            "This text appears several times in the slide: ask the agent to change it".into(),
                        ));
                    }
                }
            }
        };
        let inner = &src[element.start..element.end];
        if tags(inner)
            .iter()
            .any(|(name, attributes)| !INLINE.contains(&name.as_str()) || *attributes)
        {
            return Err(Error::BadRequest(
                "This text holds other elements: ask the agent to change it".into(),
            ));
        }
        let new = sanitized(html);
        if normalized(&text_of(&new)).is_empty() {
            return Err(Error::BadRequest(
                "A text cannot be emptied here: ask the agent to remove it".into(),
            ));
        }
        Ok(format!(
            "{}{new}{}",
            &src[..element.start],
            &src[element.end..]
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::html::*;
    use super::*;

    const SLIDE: &str = r#"<!doctype html>
<html><head><title>Intro</title><style>h1 { color: red } /* <h1> */</style></head>
<body>
  <!-- <h1>not this</h1> -->
  <h1 class="title">Grow <span class="accent">3×</span> faster</h1>
  <p>First &amp; <b>bold</b> point</p>
  <p>Second point
  <ul><li>One<li>Two</ul>
  <img src="logo.png" alt="a > b">
  <h2>Q3 &lt;plan&gt;</h2>
</body></html>"#;

    #[test]
    fn elements_skip_comments_scripts_and_quoted_brackets() {
        let tags: Vec<String> = elements(SLIDE).into_iter().map(|e| e.tag).collect();
        assert_eq!(
            tags,
            [
                "html", "head", "title", "style", "body", "h1", "span", "p", "b", "p", "ul", "li",
                "li", "img", "h2"
            ]
        );
    }

    #[test]
    fn a_text_is_retyped_in_place_and_the_rest_of_the_file_is_left_as_it_was() {
        let out = replace_text(
            SLIDE,
            "p",
            0,
            "First & bold point",
            "First &amp; <b>only</b> point",
        )
        .unwrap();
        assert!(out.contains("<p>First &amp; <b>only</b> point</p>"));
        assert_eq!(out.replace("<b>only</b>", "<b>bold</b>"), SLIDE);
        let out = replace_text(SLIDE, "h2", 0, "Q3 <plan>", "Q4 plan").unwrap();
        assert!(out.contains("<h2>Q4 plan</h2>"));
    }

    #[test]
    fn a_shifted_index_still_finds_the_one_element_reading_the_text() {
        // A script of the page added a paragraph before: the browser counts one more.
        let out = replace_text(SLIDE, "p", 1, "First & bold point", "New").unwrap();
        assert!(out.contains("<p>New</p>"));
    }

    #[test]
    fn a_text_changed_meanwhile_or_holding_other_elements_is_refused() {
        assert!(matches!(
            replace_text(SLIDE, "p", 0, "Old text", "New"),
            Err(Error::Conflict(_))
        ));
        assert!(matches!(
            replace_text(SLIDE, "h1", 0, "Grow 3× faster", "Grow"),
            Err(Error::BadRequest(_))
        ));
        assert!(matches!(
            replace_text(SLIDE, "p", 0, "First & bold point", "  "),
            Err(Error::BadRequest(_))
        ));
    }

    #[test]
    fn only_bold_italic_and_line_breaks_are_kept_from_an_edit() {
        assert_eq!(
            sanitized(
                r#"a <b>b</b> <span style="x">c</span><div>d</div><script>e</script> <i>f</i><br/>"#
            ),
            "a <b>b</b> c<br>de <i>f</i><br>"
        );
        assert_eq!(sanitized("1 < 2"), "1 &lt; 2");
    }

    #[test]
    fn hand_edits_are_told_once_to_the_chat_s_next_message() {
        let deck = Deck {
            workspace_id: "w".into(),
            workspace_path: PathBuf::from("/nonexistent"),
            path: ".chauffeur/artifacts/pitch/deck.json".into(),
            dir: PathBuf::from("/nonexistent/pitch"),
            dir_path: ".chauffeur/artifacts/pitch".into(),
            folder: PathBuf::from("/nonexistent/pitch"),
        };
        changed_by_hand(&deck, Some("chat-hand"), "skipped slide 2".into());
        let first = with_hand_edits("chat-hand", "Hello");
        assert!(first.starts_with("Hello\n\n(Chauffeur:"));
        assert!(first.contains(".chauffeur/artifacts/pitch/deck.json — skipped slide 2"));
        assert_eq!(with_hand_edits("chat-hand", "Again"), "Again");
    }

    #[test]
    fn slides_of_a_deliverable_folder_take_pinned_comments() {
        assert!(is_slide(".chauffeur/artifacts/pitch/slides/01.html"));
        assert!(!is_slide(".chauffeur/artifacts/alone.html"));
        assert!(!is_slide(".chauffeur/plans/x.html"));
    }
}
