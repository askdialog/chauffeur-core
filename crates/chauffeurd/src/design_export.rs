//! A design chat's screens as one PNG (story 47): a page framing the screens in a row with their
//! labels, served once to a headless Chrome of the machine, whose screenshot lands in the design
//! folder's `exports/` and on the clipboard. Chrome is found where browsers live, never
//! installed: without one, the export says what to install.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chauffeur_protocol::client::{
    CanvasResult, CanvasScreen, DesignExportParams, DesignExportResult,
};
use chauffeur_protocol::model::ChatKind;

use crate::{Daemon, Error, Result};

pub const EXPORTS_DIR: &str = "exports";
const PADDING: u32 = 32;
const GAP: u32 = 40;
/// A label line and the space under it.
const LABEL: u32 = 30;
/// Chrome renders at 2× for crisp screens, unless the page would be larger than it captures.
const SCALE: u32 = 2;
const MAX_PIXELS_PER_SIDE: u32 = 16_000;
const RENDER_TIMEOUT: Duration = Duration::from_secs(45);
const POLL: Duration = Duration::from_millis(150);
/// How long a page waits for Chrome before it is forgotten.
const PAGE_TTL: Duration = Duration::from_secs(120);
const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(10);
const NAME_MAX: usize = 60;

pub const NO_CHROME: &str = "No Chrome to render the PNG with: install Google Chrome (or Chromium), \
or set CHAUFFEUR_CHROME to a Chrome binary, then export again.";

/// The export pages waiting for Chrome, by token: each is served once, then forgotten. The token
/// is what Chrome's command line shows, never the screens' secret.
#[derive(Default)]
pub struct ExportPages(Mutex<HashMap<String, (String, Instant)>>);

impl ExportPages {
    fn add(&self, html: String) -> String {
        let token = random_hex();
        let mut pages = self.0.lock().unwrap();
        pages.retain(|_, (_, since)| since.elapsed() < PAGE_TTL);
        pages.insert(token.clone(), (html, Instant::now()));
        token
    }

    /// The page of a token, once: a second fetch gets nothing.
    pub fn take(&self, token: &str) -> Option<String> {
        self.0.lock().unwrap().remove(token).map(|(html, _)| html)
    }

    fn forget(&self, token: &str) {
        self.0.lock().unwrap().remove(token);
    }
}

fn random_hex() -> String {
    rand::random::<[u8; 16]>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub async fn export(daemon: &Daemon, params: &DesignExportParams) -> Result<DesignExportResult> {
    let chat = daemon.chats.resolve(&params.chat)?;
    if chat.kind != ChatKind::Design {
        return Err(Error::BadRequest(
            "only a design chat has screens to export".into(),
        ));
    }
    if params.files.is_empty() {
        return Err(Error::BadRequest(
            "name at least one screen to export (chauffeur canvas lists them)".into(),
        ));
    }
    let workspace = daemon.workspaces.row(&chat.workspace_id)?;
    let dir = crate::design::design_dir(&workspace.path, &chat.id)
        .map_err(|error| Error::Internal(anyhow::anyhow!("the design folder: {error}")))?;
    let dir = Path::new(&dir);
    let base = daemon
        .designs
        .base_url(&workspace.id, &chat.id)
        .ok_or_else(|| {
            Error::Unavailable("the screens are not served yet: export again in a moment".into())
        })?;
    let canvas = crate::canvas::read_canvas(&chat.id, dir, Some(&base));
    let screens = pick(&canvas, &params.files)?;
    let (chrome, clipboard) = match daemon.designs.fake_chrome() {
        Some(fake) => (fake, false),
        None => (
            find_chrome(daemon.shell_env.vars().get("PATH").map(String::as_str))
                .ok_or_else(|| Error::Unavailable(NO_CHROME.into()))?,
            true,
        ),
    };
    let page = Page::lay_out(&screens);
    let token = daemon.designs.exports.add(page.html());
    let url = daemon
        .designs
        .export_url(&token)
        .expect("the listener is up: it gave the screens' base URL");
    let name = params
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            screens[0]
                .label
                .clone()
                .unwrap_or_else(|| screens[0].file.clone())
        });
    let out = export_path(dir, &name);
    let rendered = render(&chrome, &url, &page, &out).await;
    daemon.designs.exports.forget(&token);
    rendered?;
    let copied = clipboard && copy_to_clipboard(daemon, &out).await;
    Ok(DesignExportResult {
        path: out.to_string_lossy().into_owned(),
        width: page.width * page.scale,
        height: page.height * page.scale,
        copied,
    })
}

/// The screens asked for, in that order, each one of the canvas and present in the folder.
fn pick(canvas: &CanvasResult, files: &[String]) -> Result<Vec<CanvasScreen>> {
    let all: HashMap<String, &CanvasScreen> = canvas
        .pages
        .iter()
        .flat_map(|page| page.sections.iter())
        .flat_map(|section| section.screens.iter())
        .chain(canvas.unsorted.iter())
        .map(|screen| (normalized(&screen.file), screen))
        .collect();
    files
        .iter()
        .map(|file| {
            let screen = all.get(&normalized(file)).ok_or_else(|| {
                Error::NotFound(format!(
                    "{file} is not a screen of this design (chauffeur canvas lists them)"
                ))
            })?;
            if screen.url.is_none() {
                return Err(Error::NotFound(format!(
                    "{file} is missing from the design folder"
                )));
            }
            Ok((*screen).clone())
        })
        .collect()
}

fn normalized(file: &str) -> String {
    file.trim_start_matches("./").to_string()
}

/// The page Chrome captures: the screens in a row, each under its label, in CSS pixels.
pub struct Page {
    screens: Vec<CanvasScreen>,
    pub width: u32,
    pub height: u32,
    pub scale: u32,
}

impl Page {
    pub fn lay_out(screens: &[CanvasScreen]) -> Page {
        let width = 2 * PADDING
            + screens.iter().map(|s| s.width).sum::<u32>()
            + GAP * (screens.len().saturating_sub(1) as u32);
        let height = 2 * PADDING + LABEL + screens.iter().map(|s| s.height).max().unwrap_or(0);
        let scale = if width.max(height) * SCALE > MAX_PIXELS_PER_SIDE {
            1
        } else {
            SCALE
        };
        Page {
            screens: screens.to_vec(),
            width,
            height,
            scale,
        }
    }

    pub fn html(&self) -> String {
        let mut html = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"color-scheme\" content=\"light\"><style>\
html,body{{margin:0;background:#f3f4f6;font:12px/1.4 -apple-system,system-ui,sans-serif;color:#374151}}\
body{{display:flex;gap:{GAP}px;padding:{PADDING}px;width:max-content}}\
.s{{display:flex;flex-direction:column;gap:8px}}\
.s span{{height:22px;line-height:22px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}}\
iframe{{border:0;background:#fff;box-shadow:0 8px 24px rgba(0,0,0,.18);display:block}}\
</style></head><body>"
        );
        for screen in &self.screens {
            let label = screen.label.as_deref().unwrap_or(&screen.file);
            html.push_str(&format!(
                "<div class=\"s\" style=\"width:{}px\"><span>{}</span><iframe src=\"{}\" width=\"{}\" height=\"{}\"></iframe></div>",
                screen.width,
                escape(label),
                escape(screen.url.as_deref().unwrap_or_default()),
                screen.width,
                screen.height
            ));
        }
        html.push_str("</body></html>");
        html
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `exports/<date>-<name>.png` in the design folder, not over an earlier export.
fn export_path(dir: &Path, name: &str) -> PathBuf {
    let exports = dir.join(EXPORTS_DIR);
    let mut slug = crate::slug::slugify(name, NAME_MAX);
    if slug.is_empty() {
        slug = "screens".into();
    }
    let stamp = chrono::Local::now().format("%Y-%m-%d-%H%M%S");
    let first = exports.join(format!("{stamp}-{slug}.png"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| exports.join(format!("{stamp}-{slug}-{n}.png")))
        .find(|path| !path.exists())
        .expect("some suffix is free")
}

/// Chrome's screenshot of `url` into `out`. Chrome is ended as soon as the PNG is whole: new
/// headless Chrome takes ten seconds and more to leave on its own.
async fn render(chrome: &Path, url: &str, page: &Page, out: &Path) -> Result<()> {
    if let Some(parent) = out.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| Error::Internal(anyhow::anyhow!("the exports folder: {error}")))?;
    }
    let profile = std::env::temp_dir().join(format!("chauffeur-export-{}", random_hex()));
    let mut child = tokio::process::Command::new(chrome)
        .args([
            "--headless".to_string(),
            "--disable-gpu".into(),
            "--hide-scrollbars".into(),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--disable-crash-reporter".into(),
            format!("--user-data-dir={}", profile.display()),
            format!("--window-size={},{}", page.width, page.height),
            format!("--force-device-scale-factor={}", page.scale),
            format!("--screenshot={}", out.display()),
            url.to_string(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            Error::Unavailable(format!("could not start {}: {error}", chrome.display()))
        })?;
    let started = Instant::now();
    let outcome = loop {
        if png_complete(out) {
            break Ok(());
        }
        match child.try_wait() {
            Ok(Some(_)) if png_complete(out) => break Ok(()),
            Ok(Some(status)) => {
                break Err(Error::Unavailable(format!(
                    "Chrome ended ({status}) without writing the PNG"
                )));
            }
            Ok(None) => {}
            Err(error) => {
                break Err(Error::Internal(anyhow::anyhow!(
                    "waiting for Chrome: {error}"
                )));
            }
        }
        if started.elapsed() > RENDER_TIMEOUT {
            break Err(Error::Unavailable(format!(
                "Chrome did not render the screens within {} seconds",
                RENDER_TIMEOUT.as_secs()
            )));
        }
        tokio::time::sleep(POLL).await;
    };
    if let Some(pid) = child.id() {
        // SAFETY: the group Chrome leads, made at spawn; a stale one fails with ESRCH.
        unsafe {
            libc::killpg(pid as i32, libc::SIGKILL);
        }
    }
    let _ = child.wait().await;
    let _ = tokio::fs::remove_dir_all(&profile).await;
    outcome
}

/// Whether `path` is a PNG written to its end: Chrome writes it in one go, but a reader may see
/// it before the last chunk.
pub fn png_complete(path: &Path) -> bool {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    const END: &[u8] = b"IEND\xaeB`\x82";
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 8];
    if file.read_exact(&mut head).is_err() || head != SIGNATURE {
        return false;
    }
    let mut tail = [0u8; 8];
    file.seek(SeekFrom::End(-8)).is_ok() && file.read_exact(&mut tail).is_ok() && tail == END
}

/// The PNG on the clipboard; false when the machine has no way to do it (no Wayland or X
/// clipboard tool on Linux) or it failed.
async fn copy_to_clipboard(daemon: &Daemon, png: &Path) -> bool {
    let Some((program, args, stdin)) = clipboard_command(daemon, png) else {
        return false;
    };
    let mut command = tokio::process::Command::new(&program);
    command
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(not(target_os = "macos"))]
    {
        let mut env = daemon.shell_env.vars();
        for (key, value) in crate::open::desktop_session(&env) {
            env.entry(key).or_insert(value);
        }
        command.env_clear().envs(env);
    }
    match stdin {
        Some(file) => match std::fs::File::open(file) {
            Ok(file) => {
                command.stdin(file);
            }
            Err(_) => return false,
        },
        None => {
            command.stdin(std::process::Stdio::null());
        }
    }
    let Ok(child) = command.spawn() else {
        tracing::warn!(program, "could not start the clipboard tool");
        return false;
    };
    match tokio::time::timeout(CLIPBOARD_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => true,
        Ok(Ok(output)) => {
            tracing::warn!(program, status = %output.status, "the PNG was not put on the clipboard");
            false
        }
        _ => false,
    }
}

/// The command that puts a PNG on the clipboard: `osascript` on macOS; on Linux `wl-copy`, else
/// `xclip`, when installed. The path, when given through stdin, is the third item.
#[cfg(target_os = "macos")]
fn clipboard_command(
    _daemon: &Daemon,
    png: &Path,
) -> Option<(String, Vec<String>, Option<PathBuf>)> {
    let path = png
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    Some((
        "/usr/bin/osascript".into(),
        vec![
            "-e".into(),
            format!("set the clipboard to (read (POSIX file \"{path}\") as «class PNGf»)"),
        ],
        None,
    ))
}

#[cfg(not(target_os = "macos"))]
fn clipboard_command(
    daemon: &Daemon,
    png: &Path,
) -> Option<(String, Vec<String>, Option<PathBuf>)> {
    let env = daemon.shell_env.vars();
    let found = |name: &str| {
        env.get("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(path))
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
            .map(|path| path.to_string_lossy().into_owned())
    };
    if let Some(wl_copy) = found("wl-copy") {
        return Some((
            wl_copy,
            vec!["--type".into(), "image/png".into()],
            Some(png.to_path_buf()),
        ));
    }
    let xclip = found("xclip")?;
    Some((
        xclip,
        vec![
            "-selection".into(),
            "clipboard".into(),
            "-t".into(),
            "image/png".into(),
            "-i".into(),
            png.to_string_lossy().into_owned(),
        ],
        None,
    ))
}

/// A Chrome of the machine: `CHAUFFEUR_CHROME`, else Playwright's headless shell (the fastest to
/// leave), else the browsers installed (Google Chrome, Chromium, Edge, Brave), in the app folders
/// on macOS and on the user's PATH elsewhere.
pub fn find_chrome(path_var: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CHAUFFEUR_CHROME") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    headless_shell()
        .into_iter()
        .chain(installed_browsers(path_var))
        .find(|path| path.is_file())
}

/// A full browser for public sites that require a normal verification window.
pub(crate) fn find_windowed_chrome(path_var: Option<&str>) -> Option<PathBuf> {
    std::env::var_os("CHAUFFEUR_CHROME")
        .map(PathBuf::from)
        .into_iter()
        .chain(installed_browsers(path_var))
        .find(|path| {
            path.is_file()
                && path
                    .file_name()
                    .is_some_and(|name| name != "chrome-headless-shell")
        })
}

/// Playwright's `chrome-headless-shell`, the newest version first.
fn headless_shell() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cache = if cfg!(target_os = "macos") {
        home.map(|home| home.join("Library/Caches/ms-playwright"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|home| home.join(".cache")))
            .map(|cache| cache.join("ms-playwright"))
    };
    let Some(cache) = cache else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&cache) else {
        return Vec::new();
    };
    let mut versions: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("chromium_headless_shell-"))
        })
        .collect();
    versions.sort();
    versions.reverse();
    versions
        .into_iter()
        .filter_map(|version| {
            std::fs::read_dir(&version)
                .ok()?
                .flatten()
                .map(|entry| entry.path())
                .find(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("chrome-headless-shell-"))
                })
                .map(|dir| dir.join("chrome-headless-shell"))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn installed_browsers(_path_var: Option<&str>) -> Vec<PathBuf> {
    const APPS: &[&str] = &[
        "Google Chrome.app/Contents/MacOS/Google Chrome",
        "Chromium.app/Contents/MacOS/Chromium",
        "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "Brave Browser.app/Contents/MacOS/Brave Browser",
    ];
    let mut dirs = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Applications"));
    }
    APPS.iter()
        .flat_map(|app| dirs.iter().map(move |dir| dir.join(app)))
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn installed_browsers(path_var: Option<&str>) -> Vec<PathBuf> {
    const NAMES: &[&str] = &[
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
        "brave-browser",
    ];
    let dirs: Vec<PathBuf> = path_var
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    NAMES
        .iter()
        .flat_map(|name| dirs.iter().map(move |dir| dir.join(name)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(
        file: &str,
        label: Option<&str>,
        width: u32,
        height: u32,
        present: bool,
    ) -> CanvasScreen {
        CanvasScreen {
            file: file.into(),
            label: label.map(str::to_string),
            width,
            height,
            url: present.then(|| format!("http://127.0.0.1:9/s/w/c/{file}")),
            modified_ms: present.then_some(1),
        }
    }

    #[test]
    fn the_page_puts_the_screens_in_a_row_under_their_labels() {
        let page = Page::lay_out(&[
            screen("a.html", Some("Repos <b>"), 390, 844, true),
            screen("b.html", None, 1440, 900, true),
        ]);
        assert_eq!(page.width, 2 * PADDING + 390 + GAP + 1440);
        assert_eq!(page.height, 2 * PADDING + LABEL + 900);
        assert_eq!(page.scale, 2);
        let html = page.html();
        assert!(html.contains("<span>Repos &lt;b&gt;</span>"));
        assert!(html.contains("<span>b.html</span>"));
        assert!(
            html.contains("src=\"http://127.0.0.1:9/s/w/c/b.html\" width=\"1440\" height=\"900\"")
        );
    }

    #[test]
    fn a_page_too_large_for_two_times_is_rendered_at_one() {
        let page = Page::lay_out(&[screen("a.html", None, 9_000, 900, true)]);
        assert_eq!(page.scale, 1);
    }

    #[test]
    fn only_screens_of_the_canvas_that_are_present_are_exported() {
        let canvas = CanvasResult {
            chat: "c".into(),
            dir: "/d".into(),
            pages: vec![chauffeur_protocol::client::CanvasPage {
                name: "Page 1".into(),
                sections: vec![chauffeur_protocol::client::CanvasSection {
                    title: "Sign in".into(),
                    screens: vec![
                        screen("sign-in/empty.html", Some("Empty"), 390, 844, true),
                        screen("gone.html", None, 390, 844, false),
                    ],
                }],
            }],
            unsorted: vec![screen("stray.html", None, 390, 844, true)],
            error: None,
        };
        let picked = pick(
            &canvas,
            &["stray.html".to_string(), "./sign-in/empty.html".to_string()],
        )
        .unwrap();
        assert_eq!(
            picked.iter().map(|s| s.file.as_str()).collect::<Vec<_>>(),
            ["stray.html", "sign-in/empty.html"]
        );
        let missing = pick(&canvas, &["gone.html".to_string()]);
        assert!(matches!(missing, Err(Error::NotFound(_))), "{missing:?}");
        let unknown = pick(&canvas, &["other.html".to_string()]);
        assert!(matches!(unknown, Err(Error::NotFound(_))), "{unknown:?}");
    }

    #[test]
    fn an_export_is_named_after_the_date_and_what_it_shows_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let first = export_path(dir.path(), "Sign in — états");
        let name = first.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.ends_with("-sign-in-etats.png"), "{name}");
        assert_eq!(first.parent().unwrap(), dir.path().join(EXPORTS_DIR));
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::write(&first, "x").unwrap();
        let second = export_path(dir.path(), "Sign in — états");
        assert_ne!(first, second);
        assert!(second.to_string_lossy().ends_with("-sign-in-etats-2.png"));
        assert!(
            export_path(dir.path(), "   ")
                .to_string_lossy()
                .ends_with("-screens.png")
        );
    }

    #[test]
    fn a_png_is_complete_once_its_end_chunk_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        assert!(!png_complete(&path), "absent");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
        assert!(!png_complete(&path), "cut short");
        std::fs::write(
            &path,
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\0IEND\xaeB`\x82",
        )
        .unwrap();
        assert!(png_complete(&path));
        std::fs::write(&path, b"not a png at all, with an IEND\xaeB`\x82").unwrap();
        assert!(!png_complete(&path), "not a PNG");
    }

    #[test]
    fn export_pages_are_served_once() {
        let pages = ExportPages::default();
        let token = pages.add("<p>page</p>".into());
        assert_eq!(pages.take(&token).as_deref(), Some("<p>page</p>"));
        assert_eq!(pages.take(&token), None);
        assert_eq!(pages.take("other"), None);
    }
}
