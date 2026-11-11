//! A share's files on the VM's disk, `<data>/shares/<id>/<path>` as the design folder lays them
//! out: written by the owner's daemon, served to the share's reviewers with the same policy the
//! daemon serves screens with. A path never leaves its share's folder.

use std::path::{Path, PathBuf};

pub use chauffeur_design::{content_type, safe_relative};
use chauffeur_protocol::client::CanvasResult;
use chauffeur_protocol::design_share::{DESIGN_CAPTURE_SCRIPT, ShareFile};

/// Larger files are refused and not served.
pub const MAX_FILE_BYTES: usize = chauffeur_design::MAX_FILE_BYTES as usize;
const LIST_DEPTH: usize = 8;
const LIST_MAX: usize = 2000;

/// What a screen may load: anything over https, its own files, inline code; never plain http.
/// Only the share's own page frames it.
pub const SCREEN_CSP: &str = "default-src 'self' https: data: blob: 'unsafe-inline' 'unsafe-eval'; \
connect-src 'self' https:; form-action 'none'; base-uri 'self'; frame-ancestors 'self'";

pub struct Files {
    root: PathBuf,
}

impl Files {
    pub fn new(data_dir: &Path) -> Files {
        Files {
            root: data_dir.join("shares"),
        }
    }

    fn share_dir(&self, share_id: &str) -> Option<PathBuf> {
        (!share_id.is_empty() && share_id.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| self.root.join(share_id))
    }

    fn file_path(&self, share_id: &str, file: &str) -> Option<PathBuf> {
        Some(self.share_dir(share_id)?.join(safe_relative(file)?))
    }

    /// Writes a file whole (through a temporary name, so that a reader never sees half of it).
    pub fn write(&self, share_id: &str, file: &str, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(format!(
                "{file} is larger than {} MB",
                MAX_FILE_BYTES / 1024 / 1024
            ));
        }
        let path = self
            .file_path(share_id, file)
            .ok_or_else(|| format!("{file} is not a path inside the design folder"))?;
        let parent = path.parent().ok_or("a file needs a folder")?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let tmp = parent.join(format!(
            ".{}.tmp",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
        ));
        std::fs::write(&tmp, bytes)
            .and_then(|()| std::fs::rename(&tmp, &path))
            .map_err(|error| error.to_string())
    }

    pub fn remove(&self, share_id: &str, file: &str) -> Result<(), String> {
        let path = self
            .file_path(share_id, file)
            .ok_or_else(|| format!("{file} is not a path inside the design folder"))?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Removes everything of a stopped share.
    pub fn remove_all(&self, share_id: &str) {
        if let Some(dir) = self.share_dir(share_id) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// A file's bytes and content type; none when missing, too large, or not a kind served.
    pub fn read(
        &self,
        share_id: &str,
        file: &str,
        document: bool,
    ) -> Option<(Vec<u8>, &'static str)> {
        let path = self.file_path(share_id, file)?;
        let media_type = if document
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
        {
            "application/pdf"
        } else {
            content_type(&path)?
        };
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if !meta.is_file() || meta.len() as usize > MAX_FILE_BYTES {
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        if media_type == "application/pdf" && !bytes.starts_with(b"%PDF-") {
            return None;
        }
        Some((bytes, media_type))
    }

    /// The share's canvas as its page lays it out: each screen's URL under `files_url`
    /// (`/v1/shares/<id>/files/`, relative to the page's origin). `dir` is left empty: the VM's
    /// folders are nobody's business.
    pub fn canvas(&self, share_id: &str, files_url: &str) -> CanvasResult {
        let mut canvas = match self.share_dir(share_id) {
            Some(dir) => chauffeur_design::read_canvas(share_id, &dir, Some(files_url)),
            None => chauffeur_design::read_canvas(share_id, Path::new(""), None),
        };
        canvas.dir = String::new();
        canvas
    }

    /// Every file of the share, relative, with when it changed.
    pub fn list(&self, share_id: &str) -> Vec<ShareFile> {
        let mut out = Vec::new();
        if let Some(dir) = self.share_dir(share_id) {
            walk(&dir, Path::new(""), 0, &mut out);
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }
}

/// A screen's HTML with the capture script, when its URL asked for it (`capture=1`).
pub fn with_capture(bytes: Vec<u8>, media_type: &str, query: Option<&str>) -> Vec<u8> {
    let wanted = query.is_some_and(|q| q.split('&').any(|pair| pair == "capture=1"));
    if !media_type.starts_with("text/html") || !wanted {
        return bytes;
    }
    let mut out = bytes;
    out.extend_from_slice(b"\n<script>");
    out.extend_from_slice(DESIGN_CAPTURE_SCRIPT.as_bytes());
    out.extend_from_slice(b"</script>\n");
    out
}

fn walk(root: &Path, relative: &Path, depth: usize, out: &mut Vec<ShareFile>) {
    if depth > LIST_DEPTH || out.len() >= LIST_MAX {
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
            walk(root, &path, depth + 1, out);
        } else if kind.is_file() && out.len() < LIST_MAX {
            let modified_ms = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            out.push(ShareFile {
                path: path.to_string_lossy().into_owned(),
                modified_ms,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_stay_inside_their_share_and_are_listed_with_their_time() {
        let dir = tempfile::tempdir().unwrap();
        let files = Files::new(dir.path());
        files
            .write("ab12", "sign in/empty.html", b"<p>empty</p>")
            .unwrap();
        files.write("ab12", "./styles.css", b"p{}").unwrap();
        assert!(files.write("ab12", "../outside.html", b"x").is_err());
        assert!(files.write("ab12", "/etc/passwd", b"x").is_err());
        assert!(files.write("ab12", ".hidden/x.html", b"x").is_err());
        assert!(
            files.write("../x", "a.html", b"x").is_err(),
            "a share id that is not hex"
        );
        let listed: Vec<String> = files.list("ab12").into_iter().map(|f| f.path).collect();
        assert_eq!(listed, ["sign in/empty.html", "styles.css"]);
        assert!(files.list("ab12").iter().all(|f| f.modified_ms > 0));
        let (bytes, media_type) = files.read("ab12", "sign in/empty.html", false).unwrap();
        assert_eq!(
            (bytes.as_slice(), media_type),
            (&b"<p>empty</p>"[..], "text/html; charset=utf-8")
        );
        assert!(files.read("ab12", "../outside.html", false).is_none());
        files.write("ab12", "notes.txt", b"x").unwrap();
        assert!(
            files.read("ab12", "notes.txt", false).is_none(),
            "not a kind served"
        );
        files.remove("ab12", "styles.css").unwrap();
        files.remove("ab12", "styles.css").unwrap();
        assert_eq!(files.list("ab12").len(), 2);
        files
            .write("ab12", "report.pdf", b"%PDF-1.4\nfixture")
            .unwrap();
        assert!(files.read("ab12", "report.pdf", false).is_none());
        assert!(files.read("ab12", "report.pdf", true).is_some());
        files
            .write("ab12", "fake.pdf", b"<script>bad()</script>")
            .unwrap();
        assert!(files.read("ab12", "fake.pdf", true).is_none());
        files.remove_all("ab12");
        assert!(files.list("ab12").is_empty());
    }

    #[test]
    fn the_capture_script_is_appended_to_html_asked_for_it_only() {
        let html = b"<p>x</p>".to_vec();
        assert_eq!(
            with_capture(html.clone(), "text/html; charset=utf-8", None),
            html
        );
        assert_eq!(
            with_capture(html.clone(), "text/css; charset=utf-8", Some("capture=1")),
            html
        );
        let with = with_capture(html, "text/html; charset=utf-8", Some("v=2&capture=1"));
        assert!(
            String::from_utf8(with)
                .unwrap()
                .contains("chauffeur:element")
        );
    }
}
