//! Slide decks, the versions of a deliverable and brands (docs/adr/slide-decks.md,
//! deliverable-versions.md, brands.md).
//!
//! A deck is a deliverable folder `.chauffeur/artifacts/<deck>/` holding `deck.json` and one HTML
//! file per slide. Every change by hand goes through the daemon, which writes the files and keeps
//! a version. A brand is a folder of `~/.chauffeur/brands/` an agent builds decks and documents
//! from.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A deliverable, named by its primary file as `files.read` names it (a deck: its `deck.json`).
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct DeckParams {
    pub workspace: String,
    pub path: String,
    /// An earlier version to read, read-only; the current files when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct DeckSlide {
    /// Relative to the deck's folder.
    pub file: String,
    pub title: String,
    #[serde(default)]
    pub notes: String,
    /// Left out of Present and of the downloads, kept in the deck.
    #[serde(default)]
    pub skip: bool,
    /// Where the reader frames the slide, with the script for comments and text editing; none
    /// before the screens' listener is up or when the file is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub url: Option<String>,
    /// Changes whenever the slide's file changes, so the reader reloads it.
    pub version: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct DeckResult {
    pub path: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    /// The brand the deck was made with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub brand: Option<String>,
    /// That brand changed since the deck copied it: the reader offers to apply it.
    #[serde(default)]
    pub brand_changed: bool,
    pub slides: Vec<DeckSlide>,
    /// The version read, when an earlier one was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub version: Option<u32>,
}

/// A change by hand to a deck's slides, written by the daemon into `deck.json`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
#[ts(export)]
pub enum DeckChange {
    /// The slides in this order: every file of the deck, once.
    Reorder {
        files: Vec<String>,
    },
    Skip {
        file: String,
        skip: bool,
    },
    Notes {
        file: String,
        notes: String,
    },
    /// A copy of the slide right after it.
    Duplicate {
        file: String,
    },
    Delete {
        file: String,
    },
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct DeckChangeParams {
    pub workspace: String,
    pub path: String,
    pub change: DeckChange,
    /// The chat the deck belongs to: its agent is told at its next turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
}

/// A text of a slide retyped in the reader.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct DeckEditTextParams {
    pub workspace: String,
    pub path: String,
    /// The slide's file, relative to the deck's folder.
    pub slide: String,
    /// The element's tag, lowercase, and its rank among the slide's elements of that tag in the
    /// order of the page, as the slide's script found them.
    pub tag: String,
    pub index: u32,
    /// The element's text before the edit: the daemon refuses when the slide no longer holds it.
    pub before: String,
    /// The new content: text, `<b>`, `<strong>`, `<i>`, `<em>` and `<br>` only; anything else is
    /// written as text.
    pub html: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DeckExportFormat {
    /// One page per slide.
    Pdf,
    /// One slide.
    Png,
    /// One self-contained page that plays the deck.
    Html,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct DeckExportParams {
    pub workspace: String,
    pub path: String,
    pub format: DeckExportFormat,
    /// The slide a PNG shows; the first one shown when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slide: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct DeckExportResult {
    /// The file written, as `files.read` names it, for `files.download`.
    pub path: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum VersionOrigin {
    /// The agent's turn changed it.
    Agent,
    /// A change in the reader.
    Hand,
    /// An earlier version brought back.
    Restore,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct DeliverableVersion {
    pub n: u32,
    /// RFC 3339.
    pub at: String,
    pub by: VersionOrigin,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct VersionRestoreParams {
    pub workspace: String,
    pub path: String,
    pub n: u32,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct BrandInfo {
    pub id: String,
    pub name: String,
    /// Its folder.
    pub path: String,
    /// The design chat that makes it, while it exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub chat: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct BrandsResult {
    pub brands: Vec<BrandInfo>,
    /// The brand a chat uses unless it chose another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default: Option<String>,
}

/// A new brand: its folder, and a brand chat in `workspace` that makes it.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct BrandCreateParams {
    pub name: String,
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<crate::model::AgentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct BrandParams {
    pub id: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct BrandDefaultParams {
    /// None: no default brand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// The brand a chat builds its decks and documents with.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ChatBrandParams {
    pub chat: String,
    /// A brand's id, `none` for no brand, absent for the default brand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
}

/// What `ChatBrandParams::brand` says to use no brand at all.
pub const NO_BRAND: &str = "none";
