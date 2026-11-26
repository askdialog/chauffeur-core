//! The user's saved prompts (docs/adr/saved-prompts.md): one `<name>.md` file per prompt in
//! `<CHAUFFEUR_HOME>/prompts/`, holding its text. Read on every call, with no cache, so a file
//! edited by hand shows the next time a client lists them.

use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use chauffeur_protocol::prompts::{Prompt, PromptSaveParams};
use serde_json::json;

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

/// The prompts, sorted by name ignoring case. Files that are not `.md`, not UTF-8, not plain
/// files (folders, symlinks) or named as no prompt can be are left out; a missing folder lists
/// nothing.
pub fn list(daemon: &Daemon) -> Result<Vec<Prompt>> {
    let entries = match std::fs::read_dir(root(daemon)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        entries => entries.context("could not read the prompts folder")?,
    };
    // An entry the folder fails to give is an error: save and delete decide on the whole list.
    let entries = entries
        .collect::<std::io::Result<Vec<_>>>()
        .context("could not read the prompts folder")?;
    let mut prompts: Vec<Prompt> = entries
        .into_iter()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|entry| {
            let name = entry
                .file_name()
                .to_str()?
                .strip_suffix(EXTENSION)
                .filter(|name| refusal(name).is_none())?
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
        Some("A prompt needs a name: type one".into())
    } else if name.contains('/') {
        Some(format!(
            "A prompt's name cannot contain \"/\": {name}. Choose a name without it"
        ))
    } else if name.starts_with('.') {
        Some(format!(
            "A prompt's name cannot start with \".\": {name}. Choose a name without it"
        ))
    } else if name.len() > MAX_NAME_BYTES {
        Some(format!(
            "A prompt's name is at most {MAX_NAME_BYTES} bytes and this one has {}: shorten it",
            name.len()
        ))
    } else {
        None
    }
}

/// Whether two paths name one file: on a file system that ignores case and Unicode normalization
/// (macOS), two spellings of a name do.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        _ => false,
    }
}

/// Writes `params.text` to the prompt `params.name`, replacing the text of the prompt with that
/// exact name; with `previous`, renames that prompt, which must exist. Refuses a name another
/// prompt has, ignoring case, or a file of the folder that is not a prompt has.
pub fn save(daemon: &Daemon, params: &PromptSaveParams) -> Result<Vec<Prompt>> {
    let name = params.name.as_str();
    if let Some(message) = refusal(name) {
        return Err(Error::BadRequestWith(message, json!({ "field": "name" })));
    }
    let _writes = WRITES.lock().unwrap();
    let existing = list(daemon)?;
    let previous = params
        .previous
        .as_deref()
        .filter(|previous| *previous != name);
    // Only a listed prompt is renamed: `previous` never reaches a file outside the folder, and a
    // prompt deleted meanwhile does not turn the rename into an overwrite of another.
    if let Some(previous) = previous
        && !existing.iter().any(|p| p.name == previous)
    {
        return Err(Error::NotFound(format!(
            "No prompt is named \"{previous}\" to rename: it was deleted or renamed meanwhile"
        )));
    }
    let root = root(daemon);
    let target = file(&root, name);
    let own = previous.unwrap_or(name);
    let own_listed = existing.iter().any(|p| p.name == own);
    if let Some(other) = existing.iter().find(|p| {
        p.name != own
            && (p.name.to_lowercase() == name.to_lowercase()
                || same_file(&file(&root, &p.name), &target))
    }) {
        return Err(Error::ConflictWith(
            format!(
                "Another prompt is already named \"{}\": choose another name",
                other.name
            ),
            json!({ "field": "name" }),
        ));
    }
    let not_a_prompt = || {
        Error::ConflictWith(
            format!(
                "The prompts folder holds a {name}.md that is not a prompt (a folder, a link, or \
                 not UTF-8): choose another name"
            ),
            json!({ "field": "name" }),
        )
    };
    let replaces_own = own_listed && same_file(&file(&root, own), &target);
    if std::fs::symlink_metadata(&target).is_ok() && !replaces_own {
        return Err(not_a_prompt());
    }
    std::fs::create_dir_all(&root).context("could not create the prompts folder")?;
    // A new file of a random name (`O_EXCL`): never a link someone left at a name known ahead,
    // and a file a crash left behind never blocks the next save. Not `.md`, so never listed.
    let tmp = tempfile::Builder::new()
        .prefix(".")
        .suffix(".tmp")
        .tempfile_in(&root)
        .and_then(|mut tmp| tmp.write_all(params.text.as_bytes()).map(|()| tmp))
        .with_context(|| format!("could not write the prompt {name}"))?;
    // A new name is installed without replacing anything, so that a file put there since the
    // check above is never overwritten; only the prompt's own file is replaced.
    let persisted = if replaces_own {
        tmp.persist(&target)
    } else {
        tmp.persist_noclobber(&target)
    };
    match persisted {
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(not_a_prompt());
        }
        result => result.with_context(|| format!("could not write the prompt {name}"))?,
    };
    if let Some(previous) = previous {
        let old = file(&root, previous);
        // Another spelling of the same name (its case, its accents' encoding) names the file just
        // written on macOS: renaming takes the new spelling, removing would remove the prompt.
        let removed = if same_file(&old, &target) {
            std::fs::rename(&old, &target)
        } else {
            // The previous name stays: the new file goes, so that the prompt is not listed twice
            // and saving again renames it.
            std::fs::remove_file(&old).inspect_err(|_| {
                let _ = std::fs::remove_file(&target);
            })
        };
        removed
            .with_context(|| format!("could not remove the prompt's previous name, {previous}"))?;
    }
    list(daemon)
}

/// Removes the prompt `name`; a name no listed prompt has is not found.
pub fn delete(daemon: &Daemon, name: &str) -> Result<Vec<Prompt>> {
    let _writes = WRITES.lock().unwrap();
    if !list(daemon)?.iter().any(|p| p.name == name) {
        return Err(Error::NotFound(format!(
            "No prompt is named \"{name}\" to delete: it was deleted or renamed meanwhile"
        )));
    }
    std::fs::remove_file(file(&root(daemon), name))
        .with_context(|| format!("could not delete the prompt {name}"))?;
    list(daemon)
}
