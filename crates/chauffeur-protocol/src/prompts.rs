//! The user's saved prompts (docs/adr/saved-prompts.md): one Markdown file per prompt in
//! `~/.chauffeur/prompts/`, managed in Settings › Prompts.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct Prompt {
    /// Its file's name, without `.md`.
    pub name: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct PromptSaveParams {
    pub name: String,
    pub text: String,
    /// The prompt's name before a rename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct PromptParams {
    pub name: String,
}
