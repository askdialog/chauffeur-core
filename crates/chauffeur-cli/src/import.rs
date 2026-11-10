//! `chauffeur import conductor` (docs/adr/conductor-import.md): the daemon plans and imports; this
//! prints the plan, lists the branches Conductor will release and asks before anything changes.
use crate::client::Client;
use crate::render::sanitize_terminal;
use anyhow::bail;
use chauffeur_protocol::client::ClientCall;
use chauffeur_protocol::conductor::{
    ConductorClone, ConductorImportParams, ConductorImportResult, ConductorOutcome,
    ConductorWorkspacePlan,
};
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Source {
    /// Bring Conductor's repos, active workspaces and chat history into Chauffeur. Each workspace
    /// is copied onto its branch with its uncommitted changes; its branch is released in
    /// Conductor (a detached HEAD there, its files untouched), which is listed and asked first.
    /// Conductor Cloud workspaces: transfer them to local in Conductor first.
    Conductor {
        /// The Conductor workspaces to import, by name, folder or branch; all active ones by default.
        workspaces: Vec<String>,
        /// Print what would be imported and released, and change nothing.
        #[arg(long)]
        dry_run: bool,
        /// Archived Conductor workspaces too, as archived workspaces holding their chat history.
        #[arg(long)]
        include_archived: bool,
        /// Go ahead without asking.
        #[arg(long)]
        yes: bool,
        /// Conductor's database, when it is not the Conductor app's on this Mac.
        #[arg(long)]
        db: Option<String>,
    },
}

pub async fn run(source: Source, client: &mut Client, json: bool) -> anyhow::Result<()> {
    let Source::Conductor {
        workspaces,
        dry_run,
        include_archived,
        yes,
        db,
    } = source;
    let params = ConductorImportParams {
        db,
        workspaces,
        include_archived,
        dry_run: true,
    };
    let plan: ConductorImportResult = client
        .call(ClientCall::ConductorImport(params.clone()))
        .await?;
    let warning = release_warning(&plan.workspaces);
    if dry_run {
        if json {
            println!("{}", serde_json::to_string_pretty(&plan)?);
        } else {
            println!("{}", sanitize_terminal(&render(&plan)));
            if let Some(warning) = &warning {
                println!("\n{}", sanitize_terminal(warning));
            }
            println!("\nDry run: nothing changed.");
        }
        return Ok(());
    }
    let to_import = plan
        .workspaces
        .iter()
        .filter(|w| w.outcome == ConductorOutcome::Import)
        .count();
    if !json {
        println!("{}", sanitize_terminal(&render(&plan)));
    }
    if !yes {
        match &warning {
            Some(warning) => {
                eprintln!("\n{}", sanitize_terminal(warning));
                if !crate::confirm("Continue?")? {
                    bail!("Nothing was imported.");
                }
            }
            None if to_import > 0 => {
                let question = format!(
                    "Import {to_import} workspace{}?",
                    if to_import == 1 { "" } else { "s" }
                );
                if !crate::confirm(&question)? {
                    bail!("Nothing was imported.");
                }
            }
            None => {}
        }
    }
    let result: ConductorImportResult = client
        .call(ClientCall::ConductorImport(ConductorImportParams {
            dry_run: false,
            ..params
        }))
        .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("\n{}", sanitize_terminal(&outcomes(&result)));
    }
    let failed = result
        .workspaces
        .iter()
        .filter(|w| w.outcome == ConductorOutcome::Failed)
        .count();
    if failed > 0 {
        bail!("{failed} Conductor workspace(s) could not be imported");
    }
    Ok(())
}

/// The warning the user answers before any branch is released.
fn release_warning(workspaces: &[ConductorWorkspacePlan]) -> Option<String> {
    let released: Vec<&ConductorWorkspacePlan> = workspaces
        .iter()
        .filter(|w| w.release && w.outcome == ConductorOutcome::Import)
        .collect();
    if released.is_empty() {
        return None;
    }
    let width = released.iter().map(|w| w.name.len()).max().unwrap_or(0);
    let mut out = String::from("These Conductor workspaces will have their branch released:\n");
    for w in released {
        out.push_str(&format!("  {:width$}   {}\n", w.name, w.branch));
    }
    out.push_str(
        "Conductor keeps their files and uncommitted changes, on a detached HEAD;\n\
         re-attach with `git switch <branch>` in that folder to go back.",
    );
    Some(out)
}

fn render(plan: &ConductorImportResult) -> String {
    let mut out = format!("Conductor database: {}\n\nRepos:", plan.db);
    if plan.repos.is_empty() {
        out.push_str(" none");
    }
    for repo in &plan.repos {
        let name = repo.name.as_deref().unwrap_or(&repo.conductor_name);
        let clone = match repo.clone {
            ConductorClone::Conductor => {
                format!(
                    "Conductor's clone, its folder joins the clone folders: {}",
                    repo.conductor_path
                )
            }
            ConductorClone::Shared => format!("same clone as Chauffeur's: {}", repo.conductor_path),
            ConductorClone::Chauffeur => format!(
                "Chauffeur's own clone: {} (branches fetched from {})",
                repo.clone_path.as_deref().unwrap_or_default(),
                repo.conductor_path
            ),
            ConductorClone::None => {
                format!("skipped: {}", repo.skipped.as_deref().unwrap_or_default())
            }
        };
        out.push_str(&format!("\n  {name}   {clone}"));
        if !repo.scripts.is_empty() {
            out.push_str(&format!(
                "\n    scripts typed in Conductor, to your settings: {}",
                repo.scripts.join(", ")
            ));
        }
        if !repo.env.is_empty() {
            out.push_str(&format!(
                "\n    variables, to your settings: {}",
                repo.env.join(", ")
            ));
        }
    }
    out.push_str("\n\nWorkspaces:");
    if plan.workspaces.is_empty() {
        out.push_str(" none");
    }
    for w in &plan.workspaces {
        out.push_str(&format!("\n  {}   {}", w.name, workspace_line(w)));
        for warning in &w.warnings {
            out.push_str(&format!("\n    warning: {warning}"));
        }
    }
    if plan.archived_left_out > 0 {
        out.push_str(&format!(
            "\n  ({} archived in Conductor, left out: --include-archived brings their chat history)",
            plan.archived_left_out
        ));
    }
    out
}

fn workspace_line(w: &ConductorWorkspacePlan) -> String {
    match w.outcome {
        ConductorOutcome::Import | ConductorOutcome::Imported => {
            let mut parts = vec![w.branch.clone()];
            if !w.repos.is_empty() {
                parts.push(w.repos.join(" + "));
            }
            if w.archived {
                parts.push("archived".into());
            } else {
                parts.push(format!("{} uncommitted file(s)", w.changes));
            }
            parts.push(format!("{} chat(s)", w.chats));
            if let Some(ticket) = &w.ticket {
                parts.push(ticket.clone());
            }
            if w.release {
                parts.push("branch released in Conductor".into());
            }
            parts.join(", ")
        }
        ConductorOutcome::AlreadyImported => match &w.workspace {
            Some(slug) => format!("already imported, as {slug}"),
            None => "already imported".into(),
        },
        ConductorOutcome::Skipped => {
            format!("skipped: {}", w.reason.as_deref().unwrap_or_default())
        }
        ConductorOutcome::Failed => {
            format!("failed: {}", w.reason.as_deref().unwrap_or_default())
        }
    }
}

fn outcomes(result: &ConductorImportResult) -> String {
    let mut lines = Vec::new();
    for w in &result.workspaces {
        let line = match w.outcome {
            ConductorOutcome::Imported => format!(
                "imported {} as {}{}",
                w.name,
                w.workspace.as_deref().unwrap_or_default(),
                if w.release {
                    format!(" (branch {} released in Conductor)", w.branch)
                } else {
                    String::new()
                }
            ),
            _ => format!("{}: {}", w.name, workspace_line(w)),
        };
        lines.push(line);
        for warning in &w.warnings {
            lines.push(format!("  warning: {warning}"));
        }
    }
    if lines.is_empty() {
        lines.push("Nothing to import.".into());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(
        name: &str,
        branch: &str,
        release: bool,
        outcome: ConductorOutcome,
    ) -> ConductorWorkspacePlan {
        ConductorWorkspacePlan {
            conductor_id: name.into(),
            name: name.into(),
            repo: Some("web".into()),
            branch: branch.into(),
            path: None,
            archived: false,
            repos: vec!["web".into()],
            release,
            changes: 0,
            chats: 0,
            ticket: None,
            outcome,
            reason: None,
            workspace: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn the_warning_lists_only_the_branches_this_import_releases() {
        let warning = release_warning(&[
            workspace(
                "pricing-filters",
                "eng-1374-price-filter",
                true,
                ConductorOutcome::Import,
            ),
            workspace(
                "done-before",
                "eng-1",
                true,
                ConductorOutcome::AlreadyImported,
            ),
            workspace("other-clone", "eng-2", false, ConductorOutcome::Import),
        ])
        .unwrap();
        assert_eq!(
            warning,
            "These Conductor workspaces will have their branch released:\n  pricing-filters   eng-1374-price-filter\nConductor keeps their files and uncommitted changes, on a detached HEAD;\nre-attach with `git switch <branch>` in that folder to go back."
        );
        assert!(release_warning(&[workspace("x", "b", false, ConductorOutcome::Import)]).is_none());
    }
}
