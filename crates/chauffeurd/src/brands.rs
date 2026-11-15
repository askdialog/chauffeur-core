//! Brands (docs/adr/brands.md): folders of `~/.chauffeur/brands/`, each made by a brand chat (a
//! design chat whose folder is the brand's) and read by the agents of the chats that build with
//! it. The daemon writes `brand.json` and the default; the brand chat's agent writes the rest.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chauffeur_protocol::client::CreateChatParams;
use chauffeur_protocol::deck::{BrandCreateParams, BrandInfo, BrandsResult, NO_BRAND};
use chauffeur_protocol::model::{AgentKind, ChatInfo, ChatKind};
use serde::{Deserialize, Serialize};

use crate::{Daemon, Error, Result};

const BRANDS_DIR: &str = "brands";
const META: &str = "brand.json";
const DEFAULT: &str = ".default";
/// What a deliverable copies of its brand, into its own `brand/` folder.
pub const COPIED: [&str; 2] = ["brand.css", "assets"];
/// Where a deliverable keeps that copy.
pub const DELIVERABLE_BRAND_DIR: &str = "brand";

#[derive(Serialize, Deserialize)]
struct Meta {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chat: Option<String>,
}

pub fn root(daemon: &Daemon) -> PathBuf {
    daemon.paths.home.join(BRANDS_DIR)
}

/// A brand's id: lowercase letters, digits and dashes, never `none`.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id != NO_BRAND
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A brand's folder, which must exist and be a real folder (never a symlink).
pub fn dir(daemon: &Daemon, id: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        return Err(Error::BadRequest(format!("{id:?} is not a brand")));
    }
    let dir = root(daemon).join(id);
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() => Ok(dir),
        _ => Err(Error::NotFound(format!("no brand {id}"))),
    }
}

fn read_meta(dir: &Path) -> Option<Meta> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(META)).ok()?).ok()
}

pub fn list(daemon: &Daemon) -> BrandsResult {
    let root = root(daemon);
    let mut brands: Vec<BrandInfo> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|entry| {
            let id = entry.file_name().to_str()?.to_string();
            if !valid_id(&id) {
                return None;
            }
            let meta = read_meta(&entry.path());
            Some(BrandInfo {
                name: meta.as_ref().map_or_else(|| id.clone(), |m| m.name.clone()),
                chat: meta.and_then(|m| m.chat),
                path: entry.path().to_string_lossy().into_owned(),
                id,
            })
        })
        .collect();
    brands.sort_by_key(|b| b.name.to_lowercase());
    let default = std::fs::read_to_string(root.join(DEFAULT))
        .ok()
        .map(|id| id.trim().to_string())
        .filter(|id| brands.iter().any(|b| &b.id == id));
    BrandsResult { brands, default }
}

pub fn set_default(daemon: &Daemon, id: Option<&str>) -> Result<BrandsResult> {
    let root = root(daemon);
    match id {
        Some(id) => {
            dir(daemon, id)?;
            std::fs::write(root.join(DEFAULT), id)?;
        }
        None => match std::fs::remove_file(root.join(DEFAULT)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        },
    }
    Ok(list(daemon))
}

pub fn delete(daemon: &Daemon, id: &str) -> Result<BrandsResult> {
    let dir = dir(daemon, id)?;
    if list(daemon).default.as_deref() == Some(id) {
        set_default(daemon, None)?;
    }
    std::fs::remove_dir_all(&dir)?;
    Ok(list(daemon))
}

/// A new brand's folder, named after `name`, and the brand chat that makes it.
pub async fn create(daemon: &Arc<Daemon>, params: BrandCreateParams) -> Result<ChatInfo> {
    let name = params.name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err(Error::BadRequest("a brand needs a name".into()));
    }
    let root = root(daemon);
    std::fs::create_dir_all(&root)?;
    let base = match crate::slug::slugify(&name, 40) {
        slug if slug.is_empty() || slug == NO_BRAND => "brand".to_string(),
        slug => slug,
    };
    let id = (1..)
        .map(|n| match n {
            1 => base.clone(),
            n => format!("{base}-{n}"),
        })
        .find(|id| !root.join(id).exists())
        .expect("some name is free");
    let dir = root.join(&id);
    std::fs::create_dir(&dir)?;
    let chat = daemon
        .chats
        .create(CreateChatParams {
            workspace: params.workspace,
            agent: Some(params.agent.unwrap_or(AgentKind::Claude)),
            model: params.model,
            effort: None,
            fast: None,
            permission_mode: None,
            prompt: None,
            images: None,
            title: Some(format!("Brand: {name}")),
            kind: Some(ChatKind::Design),
            source_chat: None,
        })
        .await;
    let chat = match chat {
        Ok(chat) => chat,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(error);
        }
    };
    write_meta(
        &dir,
        &Meta {
            name,
            chat: Some(chat.id.clone()),
        },
    )?;
    daemon.chats.set_brand(&chat.id, Some(&id))
}

fn write_meta(dir: &Path, meta: &Meta) -> Result<()> {
    let text = serde_json::to_string_pretty(meta).expect("brand.json serializes");
    std::fs::write(dir.join(META), text)?;
    Ok(())
}

/// The brand a chat builds with: its own choice, else the default; none for `none`, a design
/// chat, or a brand that no longer exists.
pub fn of_chat(daemon: &Daemon, chat: &ChatInfo) -> Option<BrandInfo> {
    if chat.kind == ChatKind::Design {
        return None;
    }
    let brands = list(daemon);
    let id = match chat.brand.as_deref() {
        Some(NO_BRAND) => return None,
        Some(id) => id.to_string(),
        None => brands.default?,
    };
    brands.brands.into_iter().find(|b| b.id == id)
}

/// What a chat's agent is told of its brand.
pub fn notice(brand: &BrandInfo) -> String {
    format!(
        "Brand: build decks and documents with the brand \u{201c}{name}\u{201d} (id `{id}`), in {path}. Read its BRAND.md first and follow it; use the variables of its brand.css, its assets/ and the models in layouts/. Copy brand.css and assets/ into the deliverable's own `{copy}/` folder and link them from there, so that the deliverable stays whole if the brand changes later; in a deck, set \"brand\": \"{id}\" in deck.json. Never change the brand's folder: the user changes a brand in its own chat.",
        name = brand.name,
        id = brand.id,
        path = brand.path,
        copy = DELIVERABLE_BRAND_DIR,
    )
}

/// What a brand chat's agent is told, instead of a design chat's prompt.
pub fn system_prompt(dir: &str) -> String {
    let name = read_meta(Path::new(dir)).map_or_else(|| "this brand".to_string(), |m| m.name);
    format!(
        "This chat is a Chauffeur brand chat: you make the brand \u{201c}{name}\u{201d}, which agents of other chats then build slide decks and documents with. You never write or change code.\n\
\n\
The user gives you what they have: an existing deck (PPTX or PDF), brand guidelines, a website address, a logo. Extract the colours, fonts, logos, tone and layout habits from it; ask for what is missing rather than inventing a brand.\n\
\n\
Write the brand in your folder, {dir} (your working directory), and only there:\n\
- BRAND.md: the tone, the rules (what a slide holds, how a chart looks, what never to do), for the agents that build with it.\n\
- brand.css: the colours, fonts, sizes and spacing as CSS variables (`--brand-primary`, `--brand-font-heading`…), with @font-face rules for the fonts in assets/fonts/ and a universal fallback for each font.\n\
- assets/: the logos (one for light backgrounds, one for dark ones), the fonts, the brand's images.\n\
- layouts/: one self-contained HTML model per kind, 1920 × 1080 for slides (title, section, content, quote, key figure, chart, closing) and an A4 page for documents (cover, headings, footer), each linking ../brand.css.\n\
- specimen.html: one page showing the colours, the fonts and every layout.\n\
\n\
List the specimen and the layouts in canvas.json so the user sees them beside this chat:\n\
{{\"pages\": [{{\"name\": \"Brand\", \"sections\": [{{\"title\": \"Slides\", \"screens\": [{{\"file\": \"layouts/title.html\", \"label\": \"Title\", \"width\": 1920, \"height\": 1080}}]}}]}}]}}\n\
\n\
Check each render with the headless-browser MCP tools. The user corrects the brand by chatting with you; keep the same files as you revise them."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_brand_id_is_a_plain_slug_and_never_none() {
        assert!(valid_id("acme-2"));
        for id in ["", "none", "Acme", "../x", "a/b", ".default"] {
            assert!(!valid_id(id), "{id}");
        }
    }

    #[test]
    fn the_notice_names_the_folder_and_asks_for_a_copy_in_the_deliverable() {
        let brand = BrandInfo {
            id: "acme".into(),
            name: "Acme".into(),
            path: "/home/me/.chauffeur/brands/acme".into(),
            chat: None,
        };
        let notice = notice(&brand);
        assert!(notice.contains("/home/me/.chauffeur/brands/acme"));
        assert!(notice.contains("\"brand\": \"acme\""));
        assert!(notice.contains("`brand/`"));
    }
}
