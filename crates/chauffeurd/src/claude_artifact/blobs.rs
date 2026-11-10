use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use reqwest::{Client, Url};

use super::{Imported, MAX_FILES, MAX_TOTAL_BYTES, download_typed};
use crate::{Error, Result};

static BLOB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?P<prefix>["'(]\s*)(?P<url>/_blob/(?P<id>[0-9a-f]{16,64})\b)"#)
        .expect("valid Claude asset regex")
});

pub async fn rewrite(
    client: &Client,
    base: &Url,
    path: &str,
    text: &str,
    imported: &mut Imported,
    total: &mut usize,
) -> Result<String> {
    let mut replacements = BTreeMap::new();
    for matched in BLOB.captures_iter(text) {
        let reference = &matched["url"];
        if replacements.contains_key(reference) {
            continue;
        }
        let prefix = format!("claude-assets/{}.", &matched["id"]);
        let local = if let Some(path) = imported.files.keys().find(|path| path.starts_with(&prefix))
        {
            path.clone()
        } else {
            if imported.files.len() >= MAX_FILES {
                return Err(invalid());
            }
            let url = base.join(reference).map_err(anyhow::Error::from)?;
            let (bytes, mime) = download_typed(client, &url).await?;
            let extension = match mime.as_str() {
                "image/svg+xml" => "svg", "image/png" => "png", "image/jpeg" => "jpg",
                "image/gif" => "gif", "image/webp" => "webp", "image/avif" => "avif",
                "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
                "font/woff" => "woff", "font/woff2" => "woff2", "font/ttf" => "ttf", "font/otf" => "otf",
                _ => return Err(Error::BadRequest("A Claude asset has an unsupported file type: export the design's HTML instead.".into())),
            };
            *total += bytes.len();
            if *total > MAX_TOTAL_BYTES {
                return Err(invalid());
            }
            let local = format!("{prefix}{extension}");
            imported.files.insert(local.clone(), bytes);
            local
        };
        let levels = path.split('/').count().saturating_sub(1);
        replacements.insert(
            reference.to_string(),
            format!("{}{local}", "../".repeat(levels)),
        );
    }
    Ok(BLOB
        .replace_all(text, |captures: &regex::Captures<'_>| {
            format!(
                "{}{}",
                &captures["prefix"],
                replacements
                    .get(&captures["url"])
                    .map(String::as_str)
                    .unwrap_or(&captures["url"])
            )
        })
        .into_owned())
}

fn invalid() -> Error {
    Error::BadRequest(
        "This Claude design has too many assets or exceeds 50 MB: export a smaller design instead."
            .into(),
    )
}
