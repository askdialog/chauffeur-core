use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use reqwest::{Client, Url};

use super::{Imported, MAX_FILES, MAX_TOTAL_BYTES, download};
use crate::{Error, Result};

static ATTR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(?:src|href|poster)\s*=\s*["']([^"']+)["']"#)
        .expect("valid asset attribute regex")
});
static CSS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)url\(\s*["']?([^\s"')]+)["']?\s*\)|@import\s+["']([^"']+)["']"#)
        .expect("valid CSS asset regex")
});
static SUPPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<script\s+src=["'](?:\./)?support\.js["']\s*>\s*</script>"#)
        .expect("valid legacy support regex")
});
static WRAPPER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)</?(?:x-dc|helmet)(?:\s[^>]*)?>").expect("valid canvas wrapper regex")
});

pub async fn copy(
    client: &Client,
    base: &Url,
    screens: &[String],
    imported: &mut Imported,
) -> Result<()> {
    let mut queue: VecDeque<String> = screens.iter().cloned().collect();
    let mut seen = HashSet::new();
    let mut total = 0;
    while let Some(path) = queue.pop_front() {
        if !seen.insert(path.clone()) || imported.files.contains_key(&path) {
            continue;
        }
        if seen.len() > MAX_FILES {
            return Err(invalid("too many linked files"));
        }
        let url = base
            .join(&chauffeur_design::encode_path(&path))
            .map_err(anyhow::Error::from)?;
        let bytes = download(client, &url).await;
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(_) if path == "support.js" && !screens.contains(&path) => {
                imported.preview.warnings.push(
                    "A linked script is unavailable; imported screens may lose some interactions."
                        .into(),
                );
                continue;
            }
            Err(error) => return Err(error),
        };
        total += bytes.len();
        if total > MAX_TOTAL_BYTES {
            return Err(invalid("more than 50 MB of files"));
        }
        let extension = Path::new(&path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let mut bytes = bytes;
        if matches!(extension, "html" | "css" | "svg") {
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| invalid("a screen or stylesheet is not UTF-8"))?;
            let text =
                super::blobs::rewrite(client, base, &path, text, imported, &mut total).await?;
            for reference in references(&text) {
                if let Some(dependency) = dependency(base, &url, &reference)? {
                    queue.push_back(dependency);
                }
            }
            if extension == "html" {
                bytes = WRAPPER.replace_all(&text, "").into_owned().into_bytes();
            } else {
                bytes = text.into_bytes();
            }
        }
        if imported.files.len() >= MAX_FILES {
            return Err(invalid("too many linked files"));
        }
        imported.files.insert(path, bytes);
    }
    if !imported.files.contains_key("support.js") {
        for screen in screens {
            if let Some(bytes) = imported.files.get_mut(screen) {
                *bytes = SUPPORT
                    .replace_all(&String::from_utf8_lossy(bytes), "")
                    .into_owned()
                    .into_bytes();
            }
        }
    }
    Ok(())
}

fn references(text: &str) -> Vec<String> {
    ATTR.captures_iter(text)
        .map(|c| c[1].to_string())
        .chain(
            CSS.captures_iter(text)
                .filter_map(|c| c.get(1).or_else(|| c.get(2)))
                .map(|m| m.as_str().to_string()),
        )
        .collect()
}

fn dependency(base: &Url, from: &Url, reference: &str) -> Result<Option<String>> {
    if reference.starts_with('#') || Url::parse(reference).is_ok() {
        return Ok(None);
    }
    if reference.starts_with('/') {
        return Err(invalid("a linked file outside the design"));
    }
    let mut url = from
        .join(reference)
        .map_err(|_| invalid("an invalid linked file"))?;
    url.set_query(None);
    url.set_fragment(None);
    let path = url
        .as_str()
        .strip_prefix(base.as_str())
        .ok_or_else(|| invalid("a linked file outside the design"))?;
    let path = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .map_err(|_| invalid("an invalid linked file path"))?;
    if chauffeur_design::safe_relative(&path).is_none()
        || path == "canvas.json"
        || chauffeur_design::content_type(Path::new(path.as_ref())).is_none()
    {
        return Err(invalid("an unsupported linked file"));
    }
    Ok(Some(path.to_string()))
}

fn invalid(reason: &str) -> Error {
    Error::BadRequest(format!(
        "This Claude design cannot be imported ({reason}): export its HTML instead."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linked_images_fonts_and_styles_stay_inside_the_project() {
        let base = Url::parse("https://example.com/project/").unwrap();
        let from = base.join("screens/a.html").unwrap();
        assert_eq!(
            dependency(&base, &from, "../styles.css?v=1").unwrap(),
            Some("styles.css".into())
        );
        assert_eq!(
            dependency(&base, &from, "../assets/my%20logo.svg").unwrap(),
            Some("assets/my logo.svg".into())
        );
        assert!(dependency(&base, &from, "../../secret.js").is_err());
        assert!(dependency(&base, &from, "../.secret.js").is_err());
        assert_eq!(
            dependency(&base, &from, "https://fonts.example.com/a.woff2").unwrap(),
            None
        );
        let refs = references(
            r#"<img src="logo.svg"><link href='main.css'><style>@import "colors.css"; a { background:url(images/bg.png) }</style>"#,
        );
        assert_eq!(
            refs,
            ["logo.svg", "main.css", "colors.css", "images/bg.png"]
        );
    }
}
