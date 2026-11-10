//! Files dropped on the main window from the desktop, on Linux: WebKitGTK gives the page their
//! types but neither their names nor their bytes, so the shell takes the drop instead
//! (`tauri.linux.conf.json` turns Tauri's drag-drop handler on there only) and tells the UI where
//! it happened. The UI then reads the files of that drop by their index, never by a path.
//!
//! Tauri's handler on WebKitGTK (wry's `drag_drop.rs`) claims only a drag that carries a URI list;
//! drags inside the page (the sidebar's, the repo order's) carry none and stay WebKit's.
//!
//! The other side of it: any drag with a URI list is claimed, so a link or an image dragged
//! within the app or in from a browser shows the composer's "Drop files" overlay, is taken by the
//! shell, then has no local file (`local_files`) and is dropped without a word.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{DragDropEvent, Emitter, Manager, State, Window, WindowEvent};

/// The daemon's limit for one attached file, `MAX_FILE_BYTES` in the app's `lib/images.ts`.
const MAX_FILE_BYTES: u64 = 20 << 20;

/// The number of the last drop, and its files.
#[derive(Default)]
pub struct LastDrop(Mutex<(u64, Vec<PathBuf>)>);

#[derive(Serialize, Clone)]
struct FileDrop {
    drop: u64,
    /// Where the files were dropped, in the web view's pixels: wry reports GTK widget
    /// coordinates, already logical though Tauri calls them physical.
    x: f64,
    y: f64,
    files: Vec<DroppedFile>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct DroppedFile {
    name: String,
    /// An image's type, as a browser names it from the extension; empty for any other file, whose
    /// type the daemon takes from its name.
    media_type: &'static str,
    size: u64,
}

/// Keeps the files of a drop on the main window and emits `file-drop` to it.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    let WindowEvent::DragDrop(DragDropEvent::Drop { paths, position }) = event else {
        return;
    };
    let paths = local_files(paths);
    // The phone pairing window, built in Rust, gets Tauri's drag-drop handler by default.
    if window.label() != "main" || paths.is_empty() {
        return;
    }
    let files = paths.iter().map(|path| describe(path)).collect();
    let drop = {
        let mut last = window.state::<LastDrop>().inner().0.lock().unwrap();
        last.0 += 1;
        last.1 = paths;
        last.0
    };
    let event = FileDrop {
        drop,
        x: position.x,
        y: position.y,
        files,
    };
    if let Err(error) = window.emit_to(window.label(), "file-drop", event) {
        eprintln!("chauffeur: could not hand a file drop to the window: {error}");
    }
}

/// The bytes of the `index`th file of drop number `drop`.
#[tauri::command]
pub async fn read_dropped_file(
    drop: u64,
    index: usize,
    last: State<'_, LastDrop>,
) -> Result<tauri::ipc::Response, String> {
    let path = {
        let last = last.0.lock().unwrap();
        if last.0 != drop {
            return Err("other files were dropped since; drop it again".into());
        }
        last.1.get(index).cloned().ok_or("no such dropped file")?
    };
    tauri::async_runtime::spawn_blocking(move || read_file(&path))
        .await
        .map_err(|error| error.to_string())?
        .map(tauri::ipc::Response::new)
}

/// The paths of a drop that name local files: a URI list dragged from a browser (`https://…`)
/// reaches wry as a relative "path".
fn local_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter(|path| path.is_absolute())
        .cloned()
        .collect()
}

fn describe(path: &Path) -> DroppedFile {
    DroppedFile {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        media_type: image_media_type(path).unwrap_or(""),
        size: std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
    }
}

/// The types the app reads as images, by extension as a browser does.
fn image_media_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// A file's bytes; an image of any size (the app scales it down), any other file up to the
/// daemon's limit, never a folder.
fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if meta.is_dir() {
        return Err("it is a folder; drop the files in it".into());
    }
    if image_media_type(path).is_none() && meta.len() > MAX_FILE_BYTES {
        return Err(format!("it is over the {} MB limit", MAX_FILE_BYTES >> 20));
    }
    std::fs::read(path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_files_of_a_drop_are_kept() {
        let paths = [
            PathBuf::from("/home/me/Screenshot.png"),
            PathBuf::from("https://example.com/page"),
            PathBuf::from("relative.txt"),
        ];
        assert_eq!(
            local_files(&paths),
            [PathBuf::from("/home/me/Screenshot.png")]
        );
    }

    #[test]
    fn images_are_typed_by_extension_and_other_files_left_to_the_daemon() {
        for (name, expected) in [
            ("shot.png", Some("image/png")),
            ("photo.JPG", Some("image/jpeg")),
            ("photo.jpeg", Some("image/jpeg")),
            ("anim.gif", Some("image/gif")),
            ("pic.webp", Some("image/webp")),
            ("report.html", None),
            ("archive.zip", None),
            ("README", None),
        ] {
            assert_eq!(image_media_type(Path::new(name)), expected, "{name}");
        }
    }

    #[test]
    fn a_dropped_file_is_named_typed_and_sized() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("Screenshot 1.png");
        std::fs::write(&path, b"12345").expect("write");
        assert_eq!(
            describe(&path),
            DroppedFile {
                name: "Screenshot 1.png".into(),
                media_type: "image/png",
                size: 5
            }
        );
        assert_eq!(read_file(&path).expect("read"), b"12345");
    }

    #[test]
    fn a_folder_and_a_file_over_the_limit_are_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(read_file(dir.path()).unwrap_err().contains("folder"));
        let big = dir.path().join("big.zip");
        std::fs::File::create(&big)
            .and_then(|file| file.set_len(MAX_FILE_BYTES + 1))
            .expect("sparse file");
        assert!(read_file(&big).unwrap_err().contains("20 MB limit"));
    }

    #[test]
    fn the_linux_window_differs_from_the_others_only_by_native_drops() {
        let main: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        let linux: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.linux.conf.json"))
                .expect("tauri.linux.conf.json");
        // Tauri merges the platform file as a JSON merge patch, which replaces arrays whole.
        let mut windows = main["app"]["windows"].clone();
        windows[0]["dragDropEnabled"] = true.into();
        assert_eq!(linux, serde_json::json!({ "app": { "windows": windows } }));
    }
}
