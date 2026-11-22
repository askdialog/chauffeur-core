//! What an imported chat's agent is told, and kept off, of the folders its session ran in: the
//! first resume names the user's folders, where each repo now lives and its own folder
//! (docs/adr/claude-code-import.md); its clone guard also covers the session's own checkouts
//! (a worktree, a scratchpad), each standing for the repo's worktree in the workspace.

use std::path::{Path, PathBuf};

use chauffeur_protocol::sidecar::SourceClone;
use serde::{Deserialize, Serialize};

use anyhow::Context;

use crate::Daemon;
use crate::Result;
use crate::conductor::same_folder;
use crate::repos::repo_folder;
use crate::store::{WorkspaceRow, WorktreeRow};
use chauffeur_protocol::model::{ChatInfo, WorkspaceState, WorktreeOrigin};

/// `<prefix><chat id>` → where the Claude Code session a chat was imported from ran ([`Origin`]).
const ORIGIN: &str = "claudecode_origin:";
/// `<prefix><chat id>` → set once the chat's agent was told where its repos now live.
const TOLD: &str = "claudecode_told:";

/// Where an imported session ran: its folder, and the checkouts it stood or wrote in.
#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct Origin {
    pub(crate) folder: PathBuf,
    pub(crate) checkouts: Vec<Checkout>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct Checkout {
    /// Chauffeur's name of its repo (`name` or `owner/name`).
    pub(crate) repo: String,
    pub(crate) folder: PathBuf,
    /// The branch the session worked on there; none when detached.
    #[serde(default)]
    pub(crate) branch: Option<String>,
}

/// The `kv` record (key, value) of where the session imported as `chat` ran.
pub(crate) fn origin_entry(chat: &str, origin: &Origin) -> Result<(String, String)> {
    let value = serde_json::to_string(origin).context("recording where a session ran")?;
    Ok((format!("{ORIGIN}{chat}"), value))
}

fn origin(d: &Daemon, chat: &str) -> Result<Option<Origin>> {
    let Some(text) = d.store.kv_get(&format!("{ORIGIN}{chat}"))? else {
        return Ok(None);
    };
    Ok(Some(
        serde_json::from_str(&text).context("reading where an imported chat's session ran")?,
    ))
}

/// A fork of `source` keeps where its session ran; its agent is told anew, the conversation it
/// goes on from holding the note or not.
pub(crate) fn forked(d: &Daemon, source: &str, fork: &str) -> Result<()> {
    if let Some(origin) = d.store.kv_get(&format!("{ORIGIN}{source}"))? {
        d.store.kv_set(&format!("{ORIGIN}{fork}"), &origin)?;
    }
    Ok(())
}

/// One of the workspace's own worktrees, not removed.
fn live(worktree: &WorktreeRow) -> bool {
    !worktree.removed && worktree.origin == WorktreeOrigin::Workspace
}

/// The worktree standing for `checkout`, of its repo (its folder's name, made from a clone of
/// that repo: another owner's repo of the same name is not): the one on the branch the session
/// worked on there, among the workspace's (`own`) and its children's; else the workspace's own.
fn worktree_of<'a>(
    d: &Daemon,
    own: &'a [WorktreeRow],
    children: &'a [WorktreeRow],
    checkout: &Checkout,
) -> Option<&'a WorktreeRow> {
    let of_repo = |w: &&WorktreeRow| {
        live(w)
            && w.repo.eq_ignore_ascii_case(repo_folder(&checkout.repo))
            && d.repos
                .clone_matches(Path::new(&w.clone_path), &checkout.repo)
    };
    own.iter()
        .chain(children)
        .filter(of_repo)
        .find(|w| checkout.branch.as_deref() == Some(w.branch.as_str()))
        .or_else(|| own.iter().find(of_repo))
}

/// The children of `workspace` still there: an orchestration's branches, after an import.
fn children(d: &Daemon, workspace: &str) -> Result<Vec<WorkspaceRow>> {
    Ok(d.store
        .list_workspaces()?
        .into_iter()
        .filter(|w| w.parent_id.as_deref() == Some(workspace))
        .filter(|w| {
            !matches!(
                w.state,
                WorkspaceState::Archiving | WorkspaceState::Archived
            )
        })
        .collect())
}

/// The worktrees of `workspace`'s children.
fn children_worktrees(d: &Daemon, workspace: &str) -> Result<Vec<WorktreeRow>> {
    let mut worktrees = Vec::new();
    for child in children(d, workspace)? {
        worktrees.extend(d.store.worktrees_of(&child.id)?);
    }
    Ok(worktrees)
}

/// What the agent of an imported chat is told before what it reads next, once: the folder its
/// session ran in, its own folder now, where each repo lives, and which folders are the user's.
/// None for a chat not imported, or told.
pub(crate) fn note(d: &Daemon, chat: &ChatInfo) -> Result<Option<String>> {
    let Some(origin) = origin(d, &chat.id)? else {
        return Ok(None);
    };
    if d.store.kv_get(&format!("{TOLD}{}", chat.id))?.is_some() {
        return Ok(None);
    }
    let Some(workspace) = d.store.find_workspace(&chat.workspace_id)? else {
        return Ok(None);
    };
    let lives: Vec<String> = d
        .store
        .worktrees_of(&workspace.id)?
        .iter()
        .filter(|w| live(w))
        .map(|w| format!("{} lives in {}", w.repo, w.path))
        .collect();
    let mut users: Vec<String> = Vec::new();
    for folder in std::iter::once(&origin.folder).chain(origin.checkouts.iter().map(|c| &c.folder))
    {
        let text = folder.display().to_string();
        if !users.contains(&text) {
            users.push(text);
        }
    }
    let lives = if lives.is_empty() {
        String::new()
    } else {
        format!(" {}.", lives.join("; "))
    };
    // An orchestration's: its branches are its children now.
    let mut described = Vec::new();
    for child in children(d, &workspace.id)? {
        let on = d.store.worktrees_of(&child.id)?.into_iter().find(live);
        described.push(match on {
            Some(worktree) => format!("{} (on {}, in {})", child.slug, worktree.branch, child.path),
            None => format!("{} (in {})", child.slug, child.path),
        });
    }
    let children_line = if described.is_empty() {
        String::new()
    } else {
        format!(
            " The branches it worked on are now child workspaces of this one: {}. Follow each with `chauffeur status <child>`, `chauffeur wait <child>` and `chauffeur send <child> -` (the message on standard input).",
            described.join(", ")
        )
    };
    let (read, change) = if users.len() == 1 {
        ("is", "it")
    } else {
        ("are", "them")
    };
    // An exploration changes nothing: no workspace to make changes in.
    let instead = if workspace.explore {
        ""
    } else {
        "; make every change in the workspace"
    };
    Ok(Some(format!(
        "(Chauffeur: this conversation was imported from Claude Code, where it ran in {}. It goes on in a Chauffeur workspace, your working folder now: {}.{lives}{children_line} {} {read} the user's own: read {change} if you need to, but never change {change}{instead}.)\n\n",
        origin.folder.display(),
        workspace.path,
        users.join(", "),
    )))
}

/// Keeps that `chat`'s agent was told where its repos live.
pub(crate) fn told(d: &Daemon, chat: &str) -> Result<()> {
    d.store
        .kv_set(&format!("{TOLD}{chat}"), &chrono::Utc::now().to_rfc3339())
}

/// The checkouts the session imported as `chat` stood or wrote in, as clones the chat's guard
/// keeps it off, each standing for its repo's worktree in the workspace (not those `clones`
/// already holds, nor those of a repo the workspace lacks). They go after `clones`: the guard
/// reads the branches git shares from the first entry holding a worktree, the clone's.
pub(crate) fn guarded(d: &Daemon, chat: &ChatInfo, clones: &[SourceClone]) -> Vec<SourceClone> {
    let found = || -> Result<Vec<SourceClone>> {
        let Some(origin) = origin(d, &chat.id)? else {
            return Ok(Vec::new());
        };
        let own = d.store.worktrees_of(&chat.workspace_id)?;
        let children = children_worktrees(d, &chat.workspace_id)?;
        let mut guarded: Vec<SourceClone> = Vec::new();
        for checkout in &origin.checkouts {
            let Some(worktree) = worktree_of(d, &own, &children, checkout) else {
                continue;
            };
            let known =
                |clone: &SourceClone| same_folder(Path::new(&clone.clone), &checkout.folder);
            if clones.iter().chain(&guarded).any(known) {
                continue;
            }
            guarded.push(SourceClone {
                clone: checkout.folder.display().to_string(),
                worktree: worktree.path.clone(),
                branch: None,
                default_branch: None,
            });
        }
        Ok(guarded)
    };
    found().unwrap_or_else(|error| {
        tracing::warn!(%error, chat = chat.id, "could not read where an imported chat's session ran");
        Vec::new()
    })
}
