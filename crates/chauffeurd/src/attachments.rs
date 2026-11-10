//! Files sent with a message (images, and any other file: a zip, an HTML page, a log) and images a
//! tool returned: saved in the workspace's notes folder, so they survive the worktrees and
//! archiving. Images are served back to clients over HTTP; other files are only named to the agent.

use std::path::{Path, PathBuf};

use base64::Engine;
use chauffeur_protocol::model::{Attachment, ImageInput};

use crate::paths::NOTES_DIR;
use crate::{Error, Result};

pub const DIR: &str = "attachments";
/// Per message. Claude takes up to 100 images a request; a chat message rarely needs more.
pub const MAX_ATTACHMENTS: usize = 20;
pub const MAX_BYTES: usize = 20 << 20;

/// What may be saved: a user attaches any file, a tool result only gives images.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Accept {
    Images,
    Files,
}

/// The workspace folder's attachments folder.
pub fn dir(workspace_path: &str) -> PathBuf {
    Path::new(workspace_path).join(NOTES_DIR).join(DIR)
}

/// Attachments saved under a workspace folder that has since been renamed, at their place now.
pub fn moved_to(workspace_path: &str, attachments: Vec<Attachment>) -> Vec<Attachment> {
    let folder = dir(workspace_path);
    attachments
        .into_iter()
        .map(|attachment| Attachment {
            path: folder.join(&attachment.file).to_string_lossy().into_owned(),
            ..attachment
        })
        .collect()
}

/// Saves the files of a message and describes them for the agent and the clients.
pub fn save(
    workspace_path: &str,
    images: &[ImageInput],
    accept: Accept,
) -> Result<Vec<Attachment>> {
    if images.len() > MAX_ATTACHMENTS {
        return Err(Error::BadRequest(format!(
            "{} files in one message; at most {MAX_ATTACHMENTS}",
            images.len()
        )));
    }
    if images.is_empty() {
        return Ok(Vec::new());
    }
    let folder = dir(workspace_path);
    std::fs::create_dir_all(&folder)?;
    images
        .iter()
        .map(|image| save_one(&folder, image, accept))
        .collect()
}

fn save_one(folder: &Path, image: &ImageInput, accept: Accept) -> Result<Attachment> {
    let (bytes, source_name) = match (&image.data, &image.path) {
        (Some(data), _) => (
            base64::engine::general_purpose::STANDARD
                .decode(data.trim())
                .map_err(|e| Error::BadRequest(format!("attachment data is not base64: {e}")))?,
            None,
        ),
        (None, Some(path)) => {
            let path = Path::new(path);
            if !path.is_absolute() || !path.is_file() {
                return Err(Error::BadRequest(format!(
                    "{} is not a file",
                    path.display()
                )));
            }
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            (std::fs::read(path)?, name)
        }
        (None, None) => {
            return Err(Error::BadRequest(
                "an attachment needs data or a path".into(),
            ));
        }
    };
    if bytes.len() > MAX_BYTES {
        return Err(Error::BadRequest(format!(
            "a file of {} MB is over the {} MB limit",
            bytes.len() >> 20,
            MAX_BYTES >> 20
        )));
    }
    let name = image.name.clone().or(source_name);
    // An image is known by its bytes, never by its name or declared type; any other file keeps the
    // extension of its name, so that the agent knows what it reads.
    let (media_type, ext) = match sniff(&bytes) {
        Some(sniffed) => {
            if let Some(declared) = &image.media_type
                && declared != sniffed
            {
                tracing::debug!(
                    declared,
                    sniffed,
                    "an image's declared type differs from its bytes"
                );
            }
            (sniffed.to_string(), extension(sniffed).to_string())
        }
        None if accept == Accept::Images => {
            return Err(Error::BadRequest(
                "only PNG, JPEG, GIF and WebP images can be attached".into(),
            ));
        }
        None => {
            let ext = name.as_deref().and_then(file_extension);
            let media_type = ext
                .as_deref()
                .and_then(known_media_type)
                .map(str::to_string)
                .or_else(|| {
                    // Never an image type for bytes that are not one: the agent would be sent them as an image.
                    image
                        .media_type
                        .clone()
                        .filter(|m| !m.is_empty() && !m.starts_with("image/"))
                })
                .unwrap_or_else(|| "application/octet-stream".into());
            (media_type, ext.unwrap_or_else(|| "bin".into()))
        }
    };
    let name = name.unwrap_or_else(|| format!("attachment.{ext}"));
    let file = format!(
        "{}-{}.{}",
        &uuid::Uuid::now_v7().simple().to_string()[..12],
        stem(&name),
        ext
    );
    let path = folder.join(&file);
    std::fs::write(&path, &bytes)?;
    Ok(Attachment {
        file,
        name,
        media_type,
        size: bytes.len() as u64,
        path: path.to_string_lossy().into_owned(),
    })
}

/// The media type from the first bytes; never trust a name or a declared type for that.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn extension(media_type: &str) -> &'static str {
    match media_type {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "png",
    }
}

/// A file name's extension, lower case, when it is a plain one (`report.HTML` → `html`).
fn file_extension(name: &str) -> Option<String> {
    let (_, ext) = name.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    (!ext.is_empty() && ext.len() <= 10 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .then_some(ext)
}

/// The media type of the files people attach most, by extension.
fn known_media_type(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "zip" => "application/zip",
        "html" | "htm" => "text/html",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "csv" => "text/csv",
        "md" => "text/markdown",
        "txt" | "log" => "text/plain",
        "xml" => "application/xml",
        "gz" | "tgz" => "application/gzip",
        "tar" => "application/x-tar",
        _ => return None,
    })
}

/// A file-system-safe, readable stem: `Screenshot 2026-10-13 at 10.27.png` → `screenshot-2026-10-13-at-10-27`.
fn stem(name: &str) -> String {
    let base = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let mut out = String::new();
    for c in base.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "attachment".into()
    } else {
        out
    }
}

/// Whether a requested file name may be served: one plain name, nothing that walks the tree.
pub fn valid_file_name(file: &str) -> bool {
    !file.is_empty()
        && !file.starts_with('.')
        && file
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    #[test]
    fn images_are_saved_under_a_safe_unique_name() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_str().unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(PNG);
        let saved = save(
            workspace,
            &[ImageInput {
                name: Some("Screenshot 2026-10-13 at 10.27.37.png".into()),
                data: Some(data),
                ..Default::default()
            }],
            Accept::Files,
        )
        .unwrap();
        assert_eq!(saved.len(), 1);
        let a = &saved[0];
        assert!(
            a.file.ends_with("-screenshot-2026-10-13-at-10-27-37.png"),
            "{}",
            a.file
        );
        assert_eq!(a.name, "Screenshot 2026-10-13 at 10.27.37.png");
        assert_eq!(a.media_type, "image/png");
        assert_eq!(std::fs::read(&a.path).unwrap(), PNG);
        assert!(a.path.contains("/.chauffeur/attachments/"));
        assert!(valid_file_name(&a.file));
    }

    #[test]
    fn a_path_is_copied_and_the_type_read_from_the_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("shot.jpeg");
        std::fs::write(&source, [0xFF, 0xD8, 0xFF, 0xE0, 0, 0]).unwrap();
        let saved = save(
            dir.path().to_str().unwrap(),
            &[ImageInput {
                path: Some(source.to_string_lossy().into_owned()),
                media_type: Some("image/png".into()),
                ..Default::default()
            }],
            Accept::Files,
        )
        .unwrap();
        assert_eq!(saved[0].name, "shot.jpeg");
        assert_eq!(saved[0].media_type, "image/jpeg");
        assert!(saved[0].file.ends_with("-shot.jpg"));
    }

    #[test]
    fn any_file_a_user_attaches_keeps_its_extension_and_a_type_that_is_not_an_image() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_str().unwrap();
        let encode = |bytes: &[u8]| Some(base64::engine::general_purpose::STANDARD.encode(bytes));
        let saved = save(
            workspace,
            &[
                ImageInput {
                    name: Some("Report.HTML".into()),
                    data: encode(b"<html></html>"),
                    ..Default::default()
                },
                ImageInput {
                    name: Some("logs.zip".into()),
                    data: encode(b"PK\x03\x04"),
                    media_type: Some("application/x-zip-compressed".into()),
                    ..Default::default()
                },
                // A declared image type is not believed for bytes that are not one.
                ImageInput {
                    name: Some("notes".into()),
                    data: encode(b"hello"),
                    media_type: Some("image/png".into()),
                    ..Default::default()
                },
            ],
            Accept::Files,
        )
        .unwrap();
        assert!(saved[0].file.ends_with("-report.html"), "{}", saved[0].file);
        assert_eq!(saved[0].media_type, "text/html");
        assert_eq!(saved[0].name, "Report.HTML");
        assert!(saved[1].file.ends_with("-logs.zip"));
        assert_eq!(saved[1].media_type, "application/zip");
        assert!(saved[2].file.ends_with("-notes.bin"), "{}", saved[2].file);
        assert_eq!(saved[2].media_type, "application/octet-stream");
        assert_eq!(std::fs::read(&saved[0].path).unwrap(), b"<html></html>");
        assert!(saved.iter().all(|a| valid_file_name(&a.file)));
    }

    #[test]
    fn a_tool_result_gives_images_only_and_bad_inputs_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_str().unwrap();
        let text = base64::engine::general_purpose::STANDARD.encode(b"hello");
        let refused =
            |image: ImageInput, accept| save(workspace, &[image], accept).unwrap_err().to_string();
        assert!(
            refused(
                ImageInput {
                    data: Some(text),
                    ..Default::default()
                },
                Accept::Images
            )
            .contains("only PNG")
        );
        assert!(
            refused(
                ImageInput {
                    path: Some("relative.png".into()),
                    ..Default::default()
                },
                Accept::Files
            )
            .contains("not a file")
        );
        assert!(refused(ImageInput::default(), Accept::Files).contains("data or a path"));
        let many = vec![ImageInput::default(); MAX_ATTACHMENTS + 1];
        assert!(save(workspace, &many, Accept::Files).is_err());
    }

    #[test]
    fn only_plain_file_names_are_served() {
        assert!(valid_file_name("0192abc-shot.png"));
        for bad in ["", "../x.png", "a/b.png", ".hidden", "x%2f.png"] {
            assert!(!valid_file_name(bad), "{bad}");
        }
    }
}
