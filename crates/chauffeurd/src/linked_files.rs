//! The markdown files a shared markdown file links to (docs/adr/share-linked-files.md): the
//! ones it names as the app's file linker reads a name (`app/src/lib/files.ts`,
//! `resolveFileRef`), or by a path relative to its folder, then the ones those name, and so on.
//! A share carrying them pushes each under its shared name beside `links.json`, which tells the
//! reviewer's page what each name of each file opens.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Component, Path};

use chauffeur_protocol::client::FileRoot;

use crate::file_comments::is_markdown;

/// Linked files a share carries at most, the shared file aside.
pub const MAX_LINKED: usize = 50;
/// The manifest's name in the share: not markdown, so no linked file takes it.
pub const MANIFEST: &str = "links.json";

/// A linked file: its path as `files.read` names it, the name the share holds it under.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkedFile {
    pub path: String,
    pub shared: String,
}

/// What a share carries beside the file it shares.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Linked {
    /// Nearest first.
    pub files: Vec<LinkedFile>,
    /// Shared name → (name as the file writes it, normalized → shared name it opens).
    pub manifest: BTreeMap<String, BTreeMap<String, String>>,
    /// More were linked than [`MAX_LINKED`].
    pub truncated: bool,
}

impl Linked {
    /// The path behind a shared name, the shared file's own included.
    pub fn path_of<'a>(&'a self, shared: &str) -> Option<&'a str> {
        self.files
            .iter()
            .find(|f| f.shared == shared)
            .map(|f| f.path.as_str())
    }

    /// The shared name of a path; none for a file the share does not carry.
    pub fn shared_of<'a>(&'a self, path: &str) -> Option<&'a str> {
        self.files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.shared.as_str())
    }

    pub fn manifest_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.manifest).expect("a manifest serializes")
    }
}

/// The name a share holds a linked file under: its path, a segment's leading `.` made `_`, as
/// the service refuses dotfiles (`.chauffeur/plan.md` → `_chauffeur/plan.md`).
pub fn shared_name(path: &str) -> String {
    path.split('/')
        .map(|segment| match segment.strip_prefix('.') {
            Some(rest) => format!("_{rest}"),
            None => segment.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The workspace's files as the app's file linker indexes them.
#[derive(Default)]
pub struct Index {
    set: HashSet<String>,
    by_inner: HashMap<String, Vec<String>>,
    by_name: HashMap<String, Vec<String>>,
}

impl Index {
    pub fn new(roots: &[FileRoot]) -> Index {
        let mut index = Index::default();
        for root in roots {
            for file in &root.files {
                let path = format!("{}/{file}", root.name);
                index
                    .by_inner
                    .entry(file.clone())
                    .or_default()
                    .push(path.clone());
                let name = file.rsplit('/').next().unwrap_or(file).to_string();
                index.by_name.entry(name).or_default().push(path.clone());
                index.set.insert(path);
            }
        }
        index
    }

    fn only(list: Option<&Vec<String>>) -> Option<String> {
        match list.map(Vec::as_slice) {
            Some([one]) => Some(one.clone()),
            _ => None,
        }
    }
}

/// The names a markdown text writes: its inline code spans and its links' destinations, out of
/// fenced code blocks.
pub fn references(markdown: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fence: Option<&str> = None;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if let Some(open) = fence {
            if trimmed.starts_with(open) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            fence = Some("```");
            continue;
        }
        if trimmed.starts_with("~~~") {
            fence = Some("~~~");
            continue;
        }
        references_in_line(line, &mut out);
    }
    out
}

fn references_in_line(line: &str, out: &mut Vec<String>) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '`' => {
                let run = chars[i..].iter().take_while(|c| **c == '`').count();
                let start = i + run;
                // The closing run of the same length.
                let mut j = start;
                let mut closed = None;
                while j < chars.len() {
                    if chars[j] == '`' {
                        let len = chars[j..].iter().take_while(|c| **c == '`').count();
                        if len == run {
                            closed = Some(j);
                            break;
                        }
                        j += len;
                    } else {
                        j += 1;
                    }
                }
                match closed {
                    Some(end) => {
                        out.push(
                            chars[start..end]
                                .iter()
                                .collect::<String>()
                                .trim()
                                .to_string(),
                        );
                        i = end + run;
                    }
                    None => i = start,
                }
            }
            ']' if chars.get(i + 1) == Some(&'(') => {
                let start = i + 2;
                let mut depth = 0;
                let mut j = start;
                while j < chars.len() {
                    match chars[j] {
                        '(' => depth += 1,
                        ')' if depth == 0 => break,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                let inner: String = chars[start..j.min(chars.len())].iter().collect();
                // `[a](path "title")` and `[a](<path with spaces>)`.
                let destination = match inner.trim().strip_prefix('<') {
                    Some(rest) => rest.split('>').next().unwrap_or_default().to_string(),
                    None => inner
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                };
                if !destination.is_empty() {
                    out.push(percent_decode(&destination));
                }
                i = j + 1;
            }
            _ => i += 1,
        }
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = text
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// A name as the app's file linker reads it (`resolveFileRef`): trimmed, without `file://`,
/// surrounding quotes or punctuation, a `#L42` or `:42:7` suffix, nor a leading `./`. None for
/// a URL or a text with a space. The reviewer's page normalizes the same way before reading the
/// manifest (`app/src/share/linkedFiles.ts`).
pub fn normalize(text: &str) -> Option<String> {
    let mut reference = text.trim();
    reference = reference.strip_prefix("file://").unwrap_or(reference);
    reference = reference.trim_start_matches(['"', '\'', '`', '(', '<']);
    reference = reference.trim_end_matches([')', '"', '\'', '`', '>', '.', ',', ';', ':']);
    let mut reference = reference.to_string();
    if let Some(at) = reference.rfind("#L")
        && line_anchor(&reference[at + 2..])
    {
        reference.truncate(at);
    } else if let Some(at) = line_suffix(&reference) {
        reference.truncate(at);
    }
    if reference.is_empty() || reference.contains(' ') || has_scheme(&reference) {
        return None;
    }
    let reference = reference.strip_prefix("./").unwrap_or(&reference);
    (!reference.is_empty()).then(|| reference.to_string())
}

/// `42`, `42-50`, `42-L50`.
fn line_anchor(rest: &str) -> bool {
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return false;
    }
    let tail = &rest[digits..];
    if tail.is_empty() {
        return true;
    }
    let Some(tail) = tail.strip_prefix('-') else {
        return false;
    };
    let tail = tail.strip_prefix('L').unwrap_or(tail);
    !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit())
}

/// Where a `:42`, `:42:7` or `:42-50` suffix starts.
fn line_suffix(reference: &str) -> Option<usize> {
    let mut parts = reference.rsplitn(4, ':').collect::<Vec<_>>();
    parts.reverse();
    // path:line, path:line:column, path:line-end (the column or end are digits).
    let numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let range = |s: &str| {
        let mut halves = s.splitn(2, '-');
        halves.next().is_some_and(numeric) && halves.next().is_none_or(numeric)
    };
    match parts.as_slice() {
        [.., path, line, column] if numeric(line) && numeric(column) && !path.is_empty() => {
            Some(reference.len() - line.len() - column.len() - 2)
        }
        [.., path, line] if range(line) && !path.is_empty() => {
            Some(reference.len() - line.len() - 1)
        }
        _ => None,
    }
}

fn has_scheme(reference: &str) -> bool {
    reference.split_once("://").is_some_and(|(scheme, _)| {
        !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphabetic())
    })
}

/// `macOS` resolves /tmp and /var to /private/…: an agent may write either.
fn without_private(path: &str) -> &str {
    for prefix in ["/private/tmp/", "/private/var/"] {
        if path.starts_with(prefix) {
            return &path["/private".len()..];
        }
    }
    path
}

/// `a/b/../c` → `a/c`; none when it climbs out of the workspace.
fn lexical(path: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The markdown file of the workspace a normalized name in `from` opens, if one.
pub fn resolve(reference: &str, from: &str, workspace: &Path, index: &Index) -> Option<String> {
    let mut reference = reference.to_string();
    if reference.starts_with('/') {
        let base = workspace.to_string_lossy();
        let base = without_private(base.trim_end_matches('/'));
        reference = without_private(&reference)
            .strip_prefix(base)?
            .strip_prefix('/')?
            .to_string();
    } else if let Some(dir) = Path::new(from).parent()
        && let Some(relative) = lexical(&dir.join(&reference))
        && index.set.contains(&relative)
    {
        return is_markdown(&relative).then_some(relative);
    }
    let found = if index.set.contains(&reference) {
        Some(reference.clone())
    } else if let Some(inner) = Index::only(index.by_inner.get(&reference)) {
        Some(inner)
    } else if !reference.contains('/')
        && reference.rsplit_once('.').is_some_and(|(_, ext)| {
            !ext.is_empty() && ext.chars().all(|c| c.is_alphanumeric() || c == '_')
        })
    {
        Index::only(index.by_name.get(&reference))
    } else {
        None
    };
    found.filter(|path| is_markdown(path))
}

/// The files `main` links to, breadth first; `read` gives a file's text (none: unreadable).
pub fn find(
    main: &str,
    main_shared: &str,
    workspace: &Path,
    index: &Index,
    read: impl Fn(&str) -> Option<String>,
) -> Linked {
    let mut linked = Linked::default();
    let mut seen: HashSet<String> = HashSet::from([main.to_string()]);
    let mut queue = VecDeque::from([(main.to_string(), main_shared.to_string())]);
    while let Some((path, shared)) = queue.pop_front() {
        let Some(text) = read(&path) else { continue };
        let mut names = BTreeMap::new();
        for written in references(&text) {
            let Some(reference) = normalize(&written) else {
                continue;
            };
            let Some(target) = resolve(&reference, &path, workspace, index) else {
                continue;
            };
            let target_shared = if target == main {
                main_shared.to_string()
            } else if let Some(known) = linked.shared_of(&target) {
                known.to_string()
            } else if seen.contains(&target) {
                // Unreadable, or beyond the limit.
                continue;
            } else if linked.files.len() >= MAX_LINKED {
                linked.truncated = true;
                seen.insert(target);
                continue;
            } else {
                let name = shared_name(&target);
                seen.insert(target.clone());
                if read(&target).is_none() {
                    continue;
                }
                linked.files.push(LinkedFile {
                    path: target.clone(),
                    shared: name.clone(),
                });
                queue.push_back((target, name.clone()));
                name
            };
            names.insert(reference, target_shared);
        }
        if !names.is_empty() {
            linked.manifest.insert(shared, names);
        }
    }
    linked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(paths: &[&str]) -> Index {
        let mut roots: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for path in paths {
            let (root, rest) = path.split_once('/').unwrap();
            roots
                .entry(root.to_string())
                .or_default()
                .push(rest.to_string());
        }
        Index::new(
            &roots
                .into_iter()
                .map(|(name, files)| FileRoot {
                    name,
                    repo: None,
                    files,
                    truncated: false,
                })
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn a_text_names_its_code_spans_and_links_but_not_its_code_blocks() {
        let text = "See `MODELISATION.md` and [the audit](AUDIT%20X.md \"title\").\n```\n`IGNORED.md`\n```\n``a `b` c`` [x](<with space.md>)";
        assert_eq!(
            references(text),
            vec!["MODELISATION.md", "AUDIT X.md", "a `b` c", "with space.md"]
        );
    }

    #[test]
    fn a_name_is_read_as_the_app_reads_it() {
        assert_eq!(normalize("`./docs/a.md:42`").as_deref(), Some("docs/a.md"));
        assert_eq!(normalize("docs/a.md#L3-L9").as_deref(), Some("docs/a.md"));
        assert_eq!(normalize("a.md:4:2").as_deref(), Some("a.md"));
        assert_eq!(normalize("a.md,").as_deref(), Some("a.md"));
        assert_eq!(normalize("https://x.com/a.md"), None);
        assert_eq!(normalize("two words.md"), None);
    }

    #[test]
    fn a_name_opens_a_path_an_inner_path_a_unique_base_name_or_a_relative_path() {
        let files = index(&[
            ".chauffeur/subscriptions/RECAP.md",
            ".chauffeur/subscriptions/MODELISATION.md",
            ".chauffeur/other/README.md",
            "core/README.md",
            "core/docs/SPEC.md",
            "core/src/main.rs",
            "core/a/NOTES.md",
            "core/b/NOTES.md",
        ]);
        let from = ".chauffeur/subscriptions/RECAP.md";
        let ws = Path::new("/w");
        let at = |r: &str| resolve(r, from, ws, &files);
        assert_eq!(
            at("MODELISATION.md").as_deref(),
            Some(".chauffeur/subscriptions/MODELISATION.md")
        );
        assert_eq!(at("docs/SPEC.md").as_deref(), Some("core/docs/SPEC.md"));
        assert_eq!(
            at("core/docs/SPEC.md").as_deref(),
            Some("core/docs/SPEC.md")
        );
        assert_eq!(
            at("/w/core/docs/SPEC.md").as_deref(),
            Some("core/docs/SPEC.md")
        );
        assert_eq!(
            at("../other/README.md").as_deref(),
            Some(".chauffeur/other/README.md")
        );
        // A repo's root file by its path inside the repo, as the app reads it.
        assert_eq!(at("README.md").as_deref(), Some("core/README.md"));
        // Two NOTES.md, neither at a repo's root: a bare base name names neither.
        assert_eq!(at("NOTES.md"), None);
        // Not markdown.
        assert_eq!(at("main.rs"), None);
        assert_eq!(at("/elsewhere/core/docs/SPEC.md"), None);
    }

    #[test]
    fn linked_files_are_found_recursively_once_each() {
        let files = index(&[
            ".chauffeur/s/RECAP.md",
            ".chauffeur/s/A.md",
            ".chauffeur/s/B.md",
            ".chauffeur/s/C.md",
        ]);
        let texts: HashMap<&str, &str> = HashMap::from([
            (".chauffeur/s/RECAP.md", "`A.md`, [b](B.md), `missing.md`"),
            (".chauffeur/s/A.md", "back to `RECAP.md`, on to `C.md`"),
            (".chauffeur/s/B.md", "`A.md`"),
            (".chauffeur/s/C.md", "nothing"),
        ]);
        let linked = find(
            ".chauffeur/s/RECAP.md",
            "RECAP.md",
            Path::new("/w"),
            &files,
            |p| texts.get(p).map(|t| t.to_string()),
        );
        let paths: Vec<_> = linked.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                ".chauffeur/s/A.md",
                ".chauffeur/s/B.md",
                ".chauffeur/s/C.md"
            ]
        );
        assert_eq!(linked.files[0].shared, "_chauffeur/s/A.md");
        assert_eq!(linked.manifest["RECAP.md"]["A.md"], "_chauffeur/s/A.md");
        assert_eq!(linked.manifest["_chauffeur/s/A.md"]["RECAP.md"], "RECAP.md");
        assert_eq!(
            linked.manifest["_chauffeur/s/B.md"]["A.md"],
            "_chauffeur/s/A.md"
        );
        assert!(!linked.manifest.contains_key("_chauffeur/s/C.md"));
        assert!(!linked.truncated);
    }

    #[test]
    fn a_share_carries_at_most_its_limit_of_linked_files() {
        let names: Vec<String> = (0..MAX_LINKED + 5).map(|i| format!("n/f{i}.md")).collect();
        let mut all: Vec<&str> = names.iter().map(String::as_str).collect();
        all.push("n/main.md");
        let files = index(&all);
        let main_text = (0..MAX_LINKED + 5)
            .map(|i| format!("`f{i}.md`"))
            .collect::<Vec<_>>()
            .join(" ");
        let linked = find("n/main.md", "main.md", Path::new("/w"), &files, |p| {
            Some(if p == "n/main.md" {
                main_text.clone()
            } else {
                String::new()
            })
        });
        assert_eq!(linked.files.len(), MAX_LINKED);
        assert!(linked.truncated);
    }
}
