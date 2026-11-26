//! The user's saved prompts (docs/adr/saved-prompts.md): one `<name>.md` file per prompt in
//! `<CHAUFFEUR_HOME>/prompts/`, holding its text. Read on every call, with no cache, so a file
//! edited by hand shows the next time a client lists them.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chauffeur_protocol::prompts::{Prompt, PromptSaveParams};

use crate::{Daemon, Error, Result};

const PROMPTS_DIR: &str = "prompts";
const EXTENSION: &str = ".md";
const MAX_NAME_BYTES: usize = 200;

/// Holds a save's uniqueness check and its writes together.
static WRITES: Mutex<()> = Mutex::new(());

fn root(daemon: &Daemon) -> PathBuf {
    daemon.paths.home.join(PROMPTS_DIR)
}

fn file(root: &Path, name: &str) -> PathBuf {
    root.join(format!("{name}{EXTENSION}"))
}

/// The prompts, sorted by name ignoring case. Files that are not `.md`, not UTF-8 or not plain
/// files (folders, symlinks) are left out; a missing folder lists nothing.
pub fn list(daemon: &Daemon) -> Result<Vec<Prompt>> {
    let mut prompts: Vec<Prompt> = std::fs::read_dir(root(daemon))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|entry| {
            let name = entry
                .file_name()
                .to_str()?
                .strip_suffix(EXTENSION)
                .filter(|name| !name.is_empty())?
                .to_string();
            let text = std::fs::read_to_string(entry.path()).ok()?;
            // An editor ends a file with a newline the user never typed in the prompt.
            let text = text
                .strip_suffix("\r\n")
                .or_else(|| text.strip_suffix('\n'))
                .unwrap_or(&text)
                .to_string();
            Some(Prompt { name, text })
        })
        .collect();
    prompts.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(prompts)
}

/// Why `name` cannot name a prompt, if it cannot.
fn refusal(name: &str) -> Option<String> {
    if name.trim().is_empty() {
        Some("A prompt needs a name".into())
    } else if name.contains('/') {
        Some(format!("A prompt's name cannot contain \"/\": {name}"))
    } else if name.starts_with('.') {
        Some(format!("A prompt's name cannot start with \".\": {name}"))
    } else if name.len() > MAX_NAME_BYTES {
        Some(format!(
            "A prompt's name is at most {MAX_NAME_BYTES} bytes; this one has {}",
            name.len()
        ))
    } else {
        None
    }
}

fn name_error(error: fn(String, serde_json::Value) -> Error, message: String) -> Error {
    error(message, serde_json::json!({ "field": "name" }))
}

/// Writes `params.text` to the prompt `params.name`, replacing the text of the prompt with that
/// exact name; with `previous`, renames that prompt. Refuses a name another prompt has, ignoring
/// case, since macOS's file system ignores it.
pub fn save(daemon: &Daemon, params: &PromptSaveParams) -> Result<Vec<Prompt>> {
    let name = params.name.as_str();
    if let Some(message) = refusal(name) {
        return Err(name_error(Error::BadRequestWith, message));
    }
    let _writes = WRITES.lock().unwrap();
    let existing = list(daemon)?;
    // Only a listed prompt is renamed: `previous` never reaches a file outside the folder.
    let previous = params
        .previous
        .as_deref()
        .filter(|previous| *previous != name && existing.iter().any(|p| p.name == *previous));
    let own = previous.unwrap_or(name);
    if let Some(other) = existing
        .iter()
        .find(|p| p.name != own && p.name.to_lowercase() == name.to_lowercase())
    {
        return Err(name_error(
            Error::ConflictWith,
            format!(
                "Another prompt is already named \"{}\": choose another name",
                other.name
            ),
        ));
    }
    let root = root(daemon);
    std::fs::create_dir_all(&root)?;
    let target = file(&root, name);
    if let Some(previous) = previous
        && previous.to_lowercase() == name.to_lowercase()
    {
        // A change of case only: on a file system that ignores case, the new file would be the
        // old one, and removing the old one would remove it. Renaming changes the case in place.
        std::fs::rename(file(&root, previous), &target)?;
    }
    let tmp = root.join(format!(".{name}{EXTENSION}.tmp"));
    std::fs::write(&tmp, &params.text)?;
    std::fs::rename(&tmp, &target)?;
    if let Some(previous) = previous
        && previous.to_lowercase() != name.to_lowercase()
    {
        std::fs::remove_file(file(&root, previous))?;
    }
    list(daemon)
}

/// Removes the prompt `name`; a name no listed prompt has is not found.
pub fn delete(daemon: &Daemon, name: &str) -> Result<Vec<Prompt>> {
    let _writes = WRITES.lock().unwrap();
    if !list(daemon)?.iter().any(|p| p.name == name) {
        return Err(Error::NotFound(format!("No prompt is named \"{name}\"")));
    }
    std::fs::remove_file(file(&root(daemon), name))?;
    list(daemon)
}
