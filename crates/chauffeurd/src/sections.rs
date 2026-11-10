//! Sidebar sections: user-named groups of workspaces, between the pinned workspaces and the
//! others. The daemon keeps them, as it keeps the sidebar's order, so that every window and the
//! CLI agree; every change is published as `RegistryEvent::SectionsChanged`.

use chauffeur_protocol::events::RegistryEvent;
use chauffeur_protocol::model::SectionInfo;

use crate::{Daemon, Error, Result};

/// The longest name kept, in characters.
const NAME_MAX: usize = 60;

pub fn list(d: &Daemon) -> Result<Vec<SectionInfo>> {
    d.store.list_sections()
}

/// A section by its id, else by its name (any case).
pub fn resolve(d: &Daemon, key: &str) -> Result<SectionInfo> {
    let sections = list(d)?;
    let key = key.trim();
    let lower = key.to_lowercase();
    sections
        .iter()
        .find(|s| s.id == key)
        .or_else(|| sections.iter().find(|s| s.name.to_lowercase() == lower))
        .cloned()
        .ok_or_else(|| Error::not_found(format!("section {key}")))
}

/// The name, its spaces collapsed; refused when empty, taken by another section (any case) or
/// one of the sidebar's own headings.
fn clean_name(d: &Daemon, name: &str, except: Option<&str>) -> Result<String> {
    let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(NAME_MAX)
        .collect();
    if name.is_empty() {
        return Err(Error::BadRequest("a section needs a name".into()));
    }
    let lower = name.to_lowercase();
    if ["pinned", "workspaces", "archived"].contains(&lower.as_str()) {
        return Err(Error::BadRequest(format!(
            "\"{name}\" is a heading of the sidebar already; pick another name"
        )));
    }
    if list(d)?
        .iter()
        .any(|s| Some(s.id.as_str()) != except && s.name.to_lowercase() == lower)
    {
        return Err(Error::Conflict(format!(
            "a section is already named {name}"
        )));
    }
    Ok(name)
}

/// Tells every client the sections as they are now.
pub fn publish(d: &Daemon) -> Result<Vec<SectionInfo>> {
    let sections = list(d)?;
    d.events.registry(RegistryEvent::SectionsChanged {
        sections: sections.clone(),
    });
    Ok(sections)
}

/// A new section, last of them; `workspace`, when given, is filed in it.
pub fn create(d: &Daemon, name: &str, workspace: Option<&str>) -> Result<SectionInfo> {
    insert(d, name, workspace, false)
}

/// A section the daemon files its own workspaces in (the daily reviews', the routines'), its id
/// kept under `key`: the one recorded, even renamed; else one the user named `name` (any case);
/// else a new one, `folded` or not. Once made, folding or moving it is the user's; deleted, it
/// comes back with the next workspace filed in it.
pub(crate) fn owned(d: &Daemon, key: &str, name: &str, folded: bool) -> Result<String> {
    let sections = list(d)?;
    if let Some(id) = d.store.kv_get(key)?
        && sections.iter().any(|s| s.id == id)
    {
        return Ok(id);
    }
    let id = match sections
        .iter()
        .find(|s| s.name.to_lowercase() == name.to_lowercase())
    {
        Some(section) => section.id.clone(),
        None => insert(d, name, None, folded)?.id,
    };
    d.store.kv_set(key, &id)?;
    Ok(id)
}

fn insert(d: &Daemon, name: &str, workspace: Option<&str>, folded: bool) -> Result<SectionInfo> {
    let name = clean_name(d, name, None)?;
    // Checked first: a section is not left behind by a workspace that does not exist.
    let workspace = workspace.map(|w| d.workspaces.row(w)).transpose()?;
    let id = uuid::Uuid::now_v7().to_string();
    d.store.insert_section(&id, &name, folded)?;
    publish(d)?;
    if let Some(workspace) = workspace {
        d.workspaces.set_section(&workspace.id, Some(&id))?;
    }
    resolve(d, &id)
}

pub fn rename(d: &Daemon, key: &str, name: &str) -> Result<SectionInfo> {
    let section = resolve(d, key)?;
    let name = clean_name(d, name, Some(&section.id))?;
    d.store.rename_section(&section.id, &name)?;
    publish(d)?;
    // Each of its workspaces carries the name.
    for row in d.store.list_workspaces()? {
        if row.section_id.as_deref() == Some(section.id.as_str()) {
            d.workspaces.publish(&row.id);
        }
    }
    resolve(d, &section.id)
}

pub fn fold(d: &Daemon, key: &str, folded: bool) -> Result<SectionInfo> {
    let section = resolve(d, key)?;
    if section.folded != folded {
        d.store.set_section_folded(&section.id, folded)?;
        publish(d)?;
    }
    resolve(d, &section.id)
}

/// Moves a section above `before`, or last.
pub fn move_to(d: &Daemon, key: &str, before: Option<&str>) -> Result<Vec<SectionInfo>> {
    let section = resolve(d, key)?;
    let before = before.map(|b| resolve(d, b)).transpose()?;
    let mut ids: Vec<String> = list(d)?
        .into_iter()
        .map(|s| s.id)
        .filter(|id| *id != section.id)
        .collect();
    let at = before
        .as_ref()
        .and_then(|b| ids.iter().position(|id| *id == b.id))
        .unwrap_or(ids.len());
    ids.insert(at, section.id);
    d.store.order_sections(&ids)?;
    publish(d)
}

/// Deletes a section: its workspaces, archived ones included, go back to no section. Nothing is
/// lost, so nothing is asked.
pub fn delete(d: &Daemon, key: &str) -> Result<Vec<SectionInfo>> {
    let section = resolve(d, key)?;
    let filed = d.store.delete_section(&section.id)?;
    for id in &filed {
        d.workspaces.publish(id);
    }
    publish(d)
}
