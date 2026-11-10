//! The files of a workspace, for the app's file browser: each repo's worktree and the notes folder.
//! A file is named by its path under the workspace folder (`<repo>/<path>`, `.chauffeur/<path>`);
//! nothing outside those roots can be read or written, symlinks included.

use std::path::{Component, Path, PathBuf};

use base64::Engine;
use chauffeur_protocol::client::{
    FileContent, FileKind, FileRoot, FilesSearchParams, FilesSearchResult, SearchMatch,
};
use chauffeur_protocol::model::{WorkspaceState, WorktreeOrigin};

use crate::error::{Error, Result};
use crate::git::Git;
use crate::paths::NOTES_DIR;
use crate::store::{WorkspaceRow, WorktreeRow};

/// Text files beyond this size are not sent.
const MAX_TEXT_BYTES: u64 = 2 << 20;
/// Images beyond this size are not sent.
const MAX_IMAGE_BYTES: u64 = 10 << 20;
/// Files listed per root at most.
const MAX_LISTED: usize = 100_000;
/// Files walked in the notes folder at most.
const MAX_NOTES: usize = 5_000;
const DEFAULT_MATCHES: u32 = 500;
const MAX_MATCHES: u32 = 5_000;
/// Matches kept per file, so that one generated file cannot fill the results.
const MATCHES_PER_FILE: &str = "50";
/// Characters of a matching line sent at most.
const MAX_LINE_CHARS: usize = 400;

/// A folder of the workspace whose files the browser shows.
pub struct Root {
    pub name: String,
    pub repo: Option<String>,
    pub dir: PathBuf,
}

/// The workspace's repos (their worktrees, as created with the workspace) by name, then its notes
/// folder.
pub fn roots(row: &WorkspaceRow, worktrees: &[WorktreeRow]) -> Vec<Root> {
    let mut roots: Vec<Root> = worktrees
        .iter()
        .filter(|w| !w.removed && w.origin == WorktreeOrigin::Workspace)
        .map(|w| Root {
            name: w.repo.clone(),
            repo: Some(w.repo.clone()),
            dir: PathBuf::from(&w.path),
        })
        .collect();
    // Worktrees are created in parallel: their order says nothing.
    roots.sort_by(|a, b| a.name.cmp(&b.name));
    roots.push(Root {
        name: NOTES_DIR.to_string(),
        repo: None,
        dir: Path::new(&row.path).join(NOTES_DIR),
    });
    roots
}

pub async fn list(git: &Git, roots: &[Root]) -> Result<Vec<FileRoot>> {
    let mut out = Vec::new();
    for root in roots {
        refuse_symlinked_root(root)?;
        let (mut files, truncated) = match root.repo {
            Some(_) => list_repo(git, &root.dir).await?,
            None => list_dir(&root.dir),
        };
        files.sort();
        out.push(FileRoot {
            name: root.name.clone(),
            repo: root.repo.clone(),
            files,
            truncated,
        });
    }
    Ok(out)
}

/// Tracked files that still exist, and untracked ones git does not ignore.
async fn list_repo(git: &Git, dir: &Path) -> Result<(Vec<String>, bool)> {
    let listed = git
        .run(
            dir,
            &[
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ],
        )
        .await?;
    let deleted = git.run(dir, &["ls-files", "--deleted", "-z"]).await?;
    let deleted: std::collections::HashSet<&str> =
        deleted.split('\0').filter(|p| !p.is_empty()).collect();
    let mut seen = std::collections::HashSet::new();
    let mut files: Vec<String> = listed
        .split('\0')
        .filter(|p| !p.is_empty() && !deleted.contains(p))
        // A file with merge conflicts is listed once per stage.
        .filter(|p| seen.insert(*p))
        .map(String::from)
        .collect();
    let truncated = files.len() > MAX_LISTED;
    files.truncate(MAX_LISTED);
    Ok((files, truncated))
}

fn list_dir(dir: &Path) -> (Vec<String>, bool) {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                if files.len() >= MAX_NOTES {
                    return (files, true);
                }
                if let Ok(relative) = path.strip_prefix(dir) {
                    files.push(relative.to_string_lossy().into_owned());
                }
            }
        }
    }
    (files, false)
}

/// The file at `path` (under the workspace folder), checked to stay inside its root.
pub fn resolve(roots: &[Root], path: &str) -> Result<PathBuf> {
    let real = resolve_entry(roots, path)?;
    if !real.is_file() {
        return Err(Error::BadRequest(format!("{path} is not a file")));
    }
    Ok(real)
}

/// The file or folder at `path` (under the workspace folder), checked to stay inside its root.
fn resolve_entry(roots: &[Root], path: &str) -> Result<PathBuf> {
    let bad = || {
        Error::BadRequest(format!(
            "{path}: not a path under the workspace's repos or notes"
        ))
    };
    let mut components = Path::new(path).components();
    let Some(Component::Normal(first)) = components.next() else {
        return Err(bad());
    };
    let rest = components.as_path();
    if rest.as_os_str().is_empty() || !rest.components().all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(bad());
    }
    let root = roots
        .iter()
        .find(|r| r.name.as_str() == first)
        .ok_or_else(|| {
            Error::not_found(format!("{} in this workspace", first.to_string_lossy()))
        })?;
    refuse_symlinked_root(root)?;
    let file = root.dir.join(rest);
    // Symlinks may point anywhere: the real file must still be under the root.
    let real = file
        .canonicalize()
        .map_err(|_| Error::not_found(path.to_string()))?;
    let real_root = root.dir.canonicalize()?;
    if !real.starts_with(&real_root) {
        return Err(bad());
    }
    Ok(real)
}

/// What Finder may be shown for `path`: a file or folder of the workspace (`<repo>/<path>`), or
/// the absolute path of one inside any Chauffeur workspace (a link to another workspace's notes),
/// symlinks resolved first so that none leads out.
pub fn resolve_to_reveal(roots: &[Root], workspaces_dir: &Path, path: &str) -> Result<PathBuf> {
    if !Path::new(path).is_absolute() {
        return resolve_entry(roots, path);
    }
    let real = Path::new(path)
        .canonicalize()
        .map_err(|_| Error::not_found(path.to_string()))?;
    let base = workspaces_dir.canonicalize()?;
    // Inside a workspace's folder: the folder holding the workspaces is no workspace.
    if real == base || !real.starts_with(&base) {
        return Err(Error::BadRequest(format!(
            "{path}: not a path inside a Chauffeur workspace"
        )));
    }
    Ok(real)
}

pub fn read(row: &WorkspaceRow, roots: &[Root], path: &str) -> Result<FileContent> {
    let file = resolve(roots, path)?;
    let size = file.metadata()?.len();
    let mime = image_mime(path);
    let mut content = FileContent {
        path: path.to_string(),
        kind: FileKind::TooLarge,
        text: None,
        base64: None,
        mime: None,
        size,
        version: String::new(),
        read_only: read_only(row),
        preview_url: None,
    };
    let limit = if mime.is_some() {
        MAX_IMAGE_BYTES
    } else {
        MAX_TEXT_BYTES
    };
    if let Some(mime) = media_type(&file, path) {
        // Shown from `/media`, in ranges: a video may be far larger than a message.
        content.kind = FileKind::Media;
        content.mime = Some(mime.to_string());
        return Ok(content);
    }
    if size > limit {
        return Ok(content);
    }
    let bytes = std::fs::read(&file)?;
    content.version = version(&bytes);
    if let Some(mime) = mime {
        content.kind = FileKind::Image;
        content.base64 = Some(base64::engine::general_purpose::STANDARD.encode(&bytes));
        content.mime = Some(mime.to_string());
        return Ok(content);
    }
    let has_nul = bytes[..bytes.len().min(8192)].contains(&0);
    match String::from_utf8(bytes) {
        Ok(text) if !has_nul => {
            content.kind = FileKind::Text;
            content.text = Some(text);
        }
        _ => content.kind = FileKind::Binary,
    }
    Ok(content)
}

/// Replaces a text file, unless it changed since `expected_version` was read.
pub fn write(
    row: &WorkspaceRow,
    roots: &[Root],
    path: &str,
    text: &str,
    expected_version: Option<&str>,
) -> Result<FileContent> {
    crate::workspaces::read_only(row, "its files cannot be edited")?;
    if row.state != WorkspaceState::Ready {
        return Err(Error::Conflict(format!(
            "{} is {}: its files cannot be edited",
            row.slug,
            row.state.as_str()
        )));
    }
    let file = resolve(roots, path)?;
    if let Some(expected) = expected_version {
        let current = version(&std::fs::read(&file)?);
        if current != expected {
            return Err(Error::Conflict(format!(
                "{path} changed on disk since it was opened"
            )));
        }
    }
    // In place, so that the file keeps its permissions and hard links.
    std::fs::write(&file, text)?;
    read(row, roots, path)
}

pub async fn search(
    git: &Git,
    roots: &[Root],
    params: &FilesSearchParams,
) -> Result<FilesSearchResult> {
    if params.query.is_empty() {
        return Err(Error::BadRequest("an empty search".into()));
    }
    let max = params
        .max_results
        .unwrap_or(DEFAULT_MATCHES)
        .clamp(1, MAX_MATCHES) as usize;
    let mut matches = Vec::new();
    let mut truncated = false;
    for root in roots {
        if params.root.as_ref().is_some_and(|r| r != &root.name) {
            continue;
        }
        refuse_symlinked_root(root)?;
        if !root.dir.is_dir() {
            continue;
        }
        let mut args = vec![
            "grep",
            "-n",
            "--column",
            "-I",
            "-z",
            "--max-count",
            MATCHES_PER_FILE,
        ];
        // The notes folder is no repo.
        args.push(if root.repo.is_some() {
            "--untracked"
        } else {
            "--no-index"
        });
        if !params.case_sensitive {
            args.push("-i");
        }
        args.push(if params.regex { "-E" } else { "-F" });
        args.extend(["-e", &params.query]);
        // Exit code 1: no match.
        let output = git
            .run_allowing(&root.dir, &args, &[1])
            .await
            .map_err(|e| Error::BadRequest(format!("{e:#}")))?;
        for line in output.lines() {
            if matches.len() >= max {
                truncated = true;
                break;
            }
            if let Some(found) = parse_match(&root.name, line) {
                matches.push(found);
            }
        }
        if truncated {
            break;
        }
    }
    Ok(FilesSearchResult { matches, truncated })
}

/// `path\0line\0column\0text`, as `git grep -z -n --column` prints it.
fn parse_match(root: &str, line: &str) -> Option<SearchMatch> {
    let mut parts = line.splitn(4, '\0');
    let path = parts.next()?;
    let number = parts.next()?.parse().ok()?;
    let column = parts.next()?.parse().ok()?;
    let text = parts.next().unwrap_or("");
    let text = match text.char_indices().nth(MAX_LINE_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    };
    Some(SearchMatch {
        path: format!("{root}/{path}"),
        line: number,
        column,
        text,
    })
}

/// A root replaced by a symlink (`ln -s ~ <ws>/<repo>`, by an agent) would pass the check above,
/// the real file being under the real root: it is refused outright, nothing is followed.
fn refuse_symlinked_root(root: &Root) -> Result<()> {
    let symlink = root
        .dir
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink());
    if symlink {
        return Err(Error::Conflict(format!(
            "{} is a symlink, not the {} folder: refused",
            root.dir.display(),
            root.name
        )));
    }
    Ok(())
}

fn read_only(row: &WorkspaceRow) -> bool {
    row.explore || row.state != WorkspaceState::Ready
}

/// An HTML file, which the Files tab may render (its `preview_url`).
pub fn is_html(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
}

fn image_mime(path: &str) -> Option<&'static str> {
    let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        _ => return None,
    })
}

/// The type of a video, an audio file or a PDF the app shows from `/media`, by its extension.
pub fn media_mime(path: &str) -> Option<&'static str> {
    let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        _ => return None,
    })
}

/// Whether the bytes a file starts with are those of the `mime` its name gave (a PDF, else a video
/// or an audio file), whatever its name says: anything else (an HTML page renamed `.mp4` or
/// `.pdf`) is never served.
pub fn is_media(mime: &str, head: &[u8]) -> bool {
    let at = |offset: usize, magic: &[u8]| head.get(offset..offset + magic.len()) == Some(magic);
    if mime == "application/pdf" {
        return at(0, b"%PDF-");
    }
    // ISO media (MP4, MOV, M4A) by its first box; old QuickTime files may start with `moov`.
    [b"ftyp", b"moov", b"mdat"]
        .iter()
        .any(|magic| at(4, *magic))
        || at(0, &[0x1a, 0x45, 0xdf, 0xa3]) // WebM / Matroska
        || at(0, b"ID3") // MP3 with tags
        || (head.len() >= 2 && head[0] == 0xff && head[1] & 0xe0 == 0xe0) // MP3, AAC frames
        || (at(0, b"RIFF") && at(8, b"WAVE"))
        || at(0, b"OggS")
        || at(0, b"fLaC")
}

/// The type of `file` (named `path`) when both its name and its first bytes make it a video, an
/// audio file or a PDF: one `/media` serves and `files.open` hands to the system's default app,
/// which only shows such content, never runs it (a script, an app, an installer).
pub fn media_type(file: &Path, path: &str) -> Option<&'static str> {
    use std::io::Read;
    let mime = media_mime(path)?;
    let mut head = Vec::with_capacity(16);
    std::fs::File::open(file)
        .ok()?
        .take(16)
        .read_to_end(&mut head)
        .ok()?;
    is_media(mime, &head).then_some(mime)
}

/// FNV-1a over the content: stable across restarts, enough to tell that a file changed.
fn version(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}-{}", bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(dir: &Path, name: &str) -> Root {
        Root {
            name: name.into(),
            repo: Some(name.into()),
            dir: dir.to_path_buf(),
        }
    }

    #[test]
    fn finder_is_shown_files_and_folders_of_chauffeur_workspaces_only() {
        let dir = tempfile::tempdir().unwrap();
        let workspaces = dir.path().join("workspaces");
        let mine = workspaces.join("mine");
        let other = workspaces.join("other/.chauffeur/recipes");
        std::fs::create_dir_all(mine.join("repo/src")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, mine.join("repo/escape")).unwrap();
        let roots = [root(&mine.join("repo"), "repo")];
        let real = |p: &Path| p.canonicalize().unwrap();

        // A folder of the workspace, relative; another workspace's folder, absolute.
        assert_eq!(
            resolve_to_reveal(&roots, &workspaces, "repo/src").unwrap(),
            real(&mine.join("repo/src"))
        );
        assert_eq!(
            resolve_to_reveal(&roots, &workspaces, other.to_str().unwrap()).unwrap(),
            real(&other)
        );
        // Not the folder of the workspaces, nor anything beside it, nor a symlink leading out.
        for path in [
            workspaces.to_str().unwrap(),
            outside.to_str().unwrap(),
            mine.join("repo/escape").to_str().unwrap(),
            "repo/escape",
            "repo/../../outside",
            "/nowhere/at/all",
        ] {
            assert!(
                resolve_to_reveal(&roots, &workspaces, path).is_err(),
                "{path}"
            );
        }
        // `files.read` still takes files only.
        assert!(resolve(&roots, "repo/src").is_err());
    }

    #[test]
    fn media_is_known_by_its_name_and_served_by_its_bytes() {
        assert_eq!(media_mime("notes/teaser.MP4"), Some("video/mp4"));
        assert_eq!(media_mime("a/b.wav"), Some("audio/wav"));
        assert_eq!(media_mime("a/b.PDF"), Some("application/pdf"));
        assert_eq!(media_mime("a/b.html"), None);
        assert!(is_media("video/mp4", b"\0\0\0\x20ftypisom\0\0"));
        assert!(is_media("audio/wav", b"RIFF\x24\0\0\0WAVEfmt "));
        assert!(is_media("audio/mpeg", b"ID3\x04\0"));
        assert!(is_media("video/webm", &[0x1a, 0x45, 0xdf, 0xa3, 0x01]));
        assert!(is_media("application/pdf", b"%PDF-1.7\n"));
        assert!(!is_media("video/mp4", b"<html><script>"));
        assert!(!is_media("video/mp4", b""));
        // A PDF must be one, and a PDF is no video.
        assert!(!is_media("application/pdf", b"<html><script>"));
        assert!(!is_media("application/pdf", b"\0\0\0\x20ftypisom\0\0"));
        assert!(!is_media("video/mp4", b"%PDF-1.7\n"));
    }

    #[test]
    fn only_files_that_show_content_open_in_their_default_app() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let file = dir.path().join(name);
            std::fs::write(&file, bytes).unwrap();
            file
        };
        let pdf = write("report.PDF", b"%PDF-1.4\n");
        assert_eq!(media_type(&pdf, "x/report.PDF"), Some("application/pdf"));
        let video = write("teaser.mp4", b"\0\0\0\x20ftypisom");
        assert_eq!(media_type(&video, "x/teaser.mp4"), Some("video/mp4"));
        // A page or a script named like a PDF is not one.
        let fake = write("fake.pdf", b"<html><script>");
        assert_eq!(media_type(&fake, "x/fake.pdf"), None);
        for name in ["run.command", "a.sh", "setup.pkg", "page.html", "noext"] {
            let file = write(name, b"%PDF-1.4\n");
            assert_eq!(media_type(&file, &format!("x/{name}")), None, "{name}");
        }
    }

    #[test]
    fn resolves_files_under_their_root_only() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let repo = base.join("alpha");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/a.ts"), "a").unwrap();
        std::fs::write(base.join("secret.txt"), "s").unwrap();
        std::os::unix::fs::symlink(base.join("secret.txt"), repo.join("link.txt")).unwrap();
        let roots = vec![root(&repo, "alpha")];

        assert_eq!(
            resolve(&roots, "alpha/src/a.ts").unwrap(),
            repo.join("src/a.ts")
        );
        for bad in [
            "alpha/../secret.txt",
            "/etc/passwd",
            "alpha",
            "alpha/link.txt",
            "alpha/src",
        ] {
            assert!(resolve(&roots, bad).is_err(), "{bad} should be refused");
        }
        assert!(matches!(
            resolve(&roots, "beta/a.ts"),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn a_root_replaced_by_a_symlink_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let home = base.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("secret.txt"), "s").unwrap();
        let workspace = base.join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        std::os::unix::fs::symlink(&home, workspace.join("alpha")).unwrap();
        let roots = vec![root(&workspace.join("alpha"), "alpha")];

        let refused = resolve(&roots, "alpha/secret.txt");
        assert!(matches!(refused, Err(Error::Conflict(_))), "{refused:?}");
        // A real folder in its place reads again.
        std::fs::remove_file(workspace.join("alpha")).unwrap();
        std::fs::create_dir_all(workspace.join("alpha")).unwrap();
        std::fs::write(workspace.join("alpha/secret.txt"), "mine").unwrap();
        assert!(resolve(&roots, "alpha/secret.txt").is_ok());
    }

    #[test]
    fn parses_git_grep_lines() {
        let found = parse_match("alpha", "src/a.ts\x0012\x005\x00let x = hello;").unwrap();
        assert_eq!(found.path, "alpha/src/a.ts");
        assert_eq!((found.line, found.column), (12, 5));
        assert_eq!(found.text, "let x = hello;");
        assert!(parse_match("alpha", "garbage").is_none());
    }

    #[test]
    fn versions_differ_with_the_content() {
        assert_eq!(version(b"abc"), version(b"abc"));
        assert_ne!(version(b"abc"), version(b"abd"));
    }
}
