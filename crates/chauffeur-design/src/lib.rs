//! A design folder as a canvas (stories 47 and 51): `canvas.json` read into pages, sections and
//! screens, the HTML files it does not list in a last "Unsorted" section, and the rules every
//! server of a design folder shares (the daemon's screens listener, the design share service):
//! a path never leaves the folder, a screen is a regular file reached without a symlink, and
//! only some kinds of files are served. Serving itself stays with each server.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use chauffeur_protocol::client::{CanvasPage, CanvasResult, CanvasScreen, CanvasSection};
use serde::Deserialize;

pub const MANIFEST: &str = "canvas.json";
const DEFAULT_WIDTH: u32 = 390;
const DEFAULT_HEIGHT: u32 = 844;
const MAX_SIDE: u32 = 10_000;
/// How deep, and how many, HTML files are looked for to fill the "Unsorted" section.
const UNSORTED_DEPTH: usize = 6;
const UNSORTED_MAX: usize = 500;
/// Larger files are not served.
pub const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;

/// Why the canvas.json of a design folder is invalid, or none (valid or absent).
pub fn manifest_error(dir: &Path) -> Option<String> {
    match std::fs::read_to_string(dir.join(MANIFEST)) {
        Ok(text) => serde_json::from_str::<Manifest>(&text)
            .err()
            .map(|e| e.to_string()),
        Err(_) => None,
    }
}

#[derive(Deserialize)]
struct Manifest {
    pages: Vec<ManifestPage>,
}

#[derive(Deserialize)]
struct ManifestPage {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    sections: Vec<ManifestSection>,
}

#[derive(Deserialize)]
struct ManifestSection {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    screens: Vec<ManifestScreen>,
}

#[derive(Deserialize)]
struct ManifestScreen {
    file: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

/// The canvas of a design folder as it is now. A screen's URL is `base_url` followed by its
/// path, percent-encoded; none without a base, or when the file is missing or outside the folder.
pub fn read_canvas(chat_id: &str, dir: &Path, base_url: Option<&str>) -> CanvasResult {
    let (manifest, error) = match std::fs::read_to_string(dir.join(MANIFEST)) {
        Ok(text) => match serde_json::from_str::<Manifest>(&text) {
            Ok(manifest) => (Some(manifest), None),
            Err(error) => (None, Some(error.to_string())),
        },
        Err(_) => (None, None),
    };
    let screen = |file: &str, label: Option<String>, width: Option<u32>, height: Option<u32>| {
        let found = screen_file(dir, file);
        CanvasScreen {
            file: file.to_string(),
            label: label.filter(|l| !l.trim().is_empty()),
            width: width.unwrap_or(DEFAULT_WIDTH).clamp(1, MAX_SIDE),
            height: height.unwrap_or(DEFAULT_HEIGHT).clamp(1, MAX_SIDE),
            url: found
                .as_ref()
                .and(base_url)
                .map(|base| format!("{base}{}", encode_path(file))),
            modified_ms: found.and_then(|(_, modified)| modified),
        }
    };
    let mut listed = HashSet::new();
    let pages: Vec<CanvasPage> = manifest
        .map(|manifest| {
            manifest
                .pages
                .into_iter()
                .enumerate()
                .map(|(i, page)| CanvasPage {
                    name: page
                        .name
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or_else(|| format!("Page {}", i + 1)),
                    sections: page
                        .sections
                        .into_iter()
                        .map(|section| CanvasSection {
                            title: section.title.unwrap_or_default(),
                            screens: section
                                .screens
                                .into_iter()
                                .map(|s| {
                                    listed.insert(normalized(&s.file));
                                    screen(&s.file, s.label, s.width, s.height)
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect()
        })
        .unwrap_or_default();
    let mut unsorted = Vec::new();
    html_files(dir, Path::new(""), 0, &mut unsorted);
    unsorted.sort();
    let unsorted = unsorted
        .into_iter()
        .filter(|file| !listed.contains(file))
        .map(|file| screen(&file, None, None, None))
        .collect();
    CanvasResult {
        chat: chat_id.to_string(),
        dir: dir.to_string_lossy().into_owned(),
        pages,
        unsorted,
        error,
    }
}

fn normalized(file: &str) -> String {
    file.trim_start_matches("./").to_string()
}

/// The file a screen names inside the folder, and when it changed: a regular file, reached
/// without `..`, an absolute path or a symlink on the way, no larger than `MAX_FILE_BYTES`.
pub fn screen_file(dir: &Path, file: &str) -> Option<(PathBuf, Option<u64>)> {
    let relative = safe_relative(file)?;
    let mut path = dir.to_path_buf();
    for part in relative.components() {
        path.push(part);
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if meta.file_type().is_symlink() {
            return None;
        }
    }
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);
    Some((path, modified))
}

/// A path relative to the folder, refused when it could leave it (`..`, absolute, a backslash)
/// or names something hidden (a dot-file: the folder's own temporary files, never a screen).
pub fn safe_relative(file: &str) -> Option<PathBuf> {
    if file.is_empty() || file.contains(['\\', '\0']) {
        return None;
    }
    let mut out = PathBuf::new();
    for component in Path::new(file).components() {
        match component {
            Component::Normal(part) => {
                if part.to_string_lossy().starts_with('.') {
                    return None;
                }
                out.push(part)
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn html_files(root: &Path, relative: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > UNSORTED_DEPTH || out.len() >= UNSORTED_MAX {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root.join(relative)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = relative.join(&*name);
        if kind.is_dir() {
            html_files(root, &path, depth + 1, out);
        } else if kind.is_file()
            && matches!(
                path.extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("html" | "htm")
            )
            && out.len() < UNSORTED_MAX
        {
            out.push(path.to_string_lossy().into_owned());
        }
    }
}

/// The content type a design's file is served with; other kinds are not served.
pub fn content_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        // A shared markdown file (story 57), rendered by the page; served as text.
        "md" | "mdx" | "markdown" => "text/markdown; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => return None,
    })
}

/// A file's path as a URL's, each segment percent-encoded; empty for a path refused.
pub fn encode_path(file: &str) -> String {
    safe_relative(file)
        .map(|path| {
            path.components()
                .map(|c| encode_segment(&c.as_os_str().to_string_lossy()))
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default()
}

/// Percent-encodes everything but RFC 3986's unreserved characters.
pub fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sign in")).unwrap();
        std::fs::write(dir.path().join("sign in/empty.html"), "<p>empty</p>").unwrap();
        std::fs::write(dir.path().join("error.html"), "<p>error</p>").unwrap();
        std::fs::write(dir.path().join("stray.html"), "<p>stray</p>").unwrap();
        std::fs::write(dir.path().join("styles.css"), "p{}").unwrap();
        dir
    }

    #[test]
    fn the_manifest_lays_screens_out_and_the_rest_is_unsorted() {
        let dir = folder();
        std::fs::write(
            dir.path().join(MANIFEST),
            r#"{"pages": [{"name": "Login", "sections": [{"title": "Sign in — states", "screens": [
                {"file": "sign in/empty.html", "label": "Empty", "width": 1440, "height": 900},
                {"file": "./error.html"},
                {"file": "gone.html"},
                {"file": "../outside.html"}
            ]}]}]}"#,
        )
        .unwrap();
        let canvas = read_canvas("c1", dir.path(), Some("http://127.0.0.1:9/s/w/c1/"));
        assert_eq!(canvas.error, None);
        let screens = &canvas.pages[0].sections[0].screens;
        assert_eq!(canvas.pages[0].name, "Login");
        assert_eq!(
            screens[0].url.as_deref(),
            Some("http://127.0.0.1:9/s/w/c1/sign%20in/empty.html")
        );
        assert_eq!((screens[0].width, screens[0].height), (1440, 900));
        assert!(screens[0].modified_ms.is_some());
        assert_eq!((screens[1].width, screens[1].height), (390, 844));
        assert!(screens[1].url.is_some());
        assert!(screens[2].url.is_none(), "a missing file");
        assert!(screens[3].url.is_none(), "a file outside the folder");
        let unsorted: Vec<&str> = canvas.unsorted.iter().map(|s| s.file.as_str()).collect();
        assert_eq!(unsorted, ["stray.html"]);
    }

    #[test]
    fn an_invalid_manifest_says_why_and_every_screen_is_unsorted() {
        let dir = folder();
        std::fs::write(dir.path().join(MANIFEST), "{\"pages\": [").unwrap();
        let canvas = read_canvas("c1", dir.path(), None);
        assert!(canvas.error.is_some());
        assert!(manifest_error(dir.path()).is_some());
        assert!(canvas.pages.is_empty());
        assert_eq!(canvas.unsorted.len(), 3);
        assert!(
            canvas.unsorted.iter().all(|s| s.url.is_none()),
            "not served"
        );
    }

    #[test]
    fn a_screen_reached_through_a_symlink_is_not_one() {
        let dir = folder();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.html"), "secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("linked")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.html"),
            dir.path().join("direct.html"),
        )
        .unwrap();
        assert!(screen_file(dir.path(), "linked/secret.html").is_none());
        assert!(screen_file(dir.path(), "direct.html").is_none());
        assert!(screen_file(dir.path(), "/etc/hosts").is_none());
        assert!(screen_file(dir.path(), "error.html").is_some());
        let canvas = read_canvas("c1", dir.path(), None);
        assert!(canvas.unsorted.iter().all(|s| s.file != "direct.html"));
    }

    #[test]
    fn a_path_stays_inside_the_folder_and_names_nothing_hidden() {
        assert_eq!(
            safe_relative("a/b.html").unwrap(),
            PathBuf::from("a/b.html")
        );
        assert_eq!(safe_relative("./a.html").unwrap(), PathBuf::from("a.html"));
        for refused in [
            "",
            "../a.html",
            "/etc/hosts",
            "a\\b.html",
            ".tmp/a.html",
            "a/.b.html",
        ] {
            assert!(safe_relative(refused).is_none(), "{refused}");
        }
        assert_eq!(encode_path("sign in/é.html"), "sign%20in/%C3%A9.html");
        assert_eq!(encode_path("../x"), "");
        assert_eq!(
            content_type(Path::new("a.HTML")),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(content_type(Path::new("a.exe")), None);
    }
}
