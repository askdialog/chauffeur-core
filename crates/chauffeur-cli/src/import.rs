//! `chauffeur import conductor` (docs/adr/conductor-import.md): the daemon plans and imports; this
//! prints the plan, lists the branches Conductor will release and asks before anything changes.
//! `chauffeur import claude-code` (docs/adr/claude-code-import.md): the plan only, for now.
use crate::client::Client;
use crate::render::sanitize_terminal;
use anyhow::bail;
use chauffeur_protocol::claudecode::{
    ClaudeCodeImportParams, ClaudeCodeImportResult, ClaudeCodeOutcome, ClaudeCodeRelease,
    ClaudeCodeWorkspaceKind,
};
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
    /// Bring the work of Claude Code's sessions into Chauffeur: a workspace per live branch, a
    /// parent and its children for a session that worked on several, the rest in each repo's
    /// history exploration. For now only `--dry-run` runs: it prints the plan.
    #[command(name = "claude-code")]
    ClaudeCode {
        /// Print what would be imported and released, and change nothing.
        #[arg(long)]
        dry_run: bool,
        /// Release the branches to release without asking.
        #[arg(long)]
        yes: bool,
        /// Sessions active in the last DAYS days, or since a day (YYYY-MM-DD); 60 days by default.
        #[arg(long, value_name = "DAYS|DATE", conflicts_with = "all")]
        since: Option<String>,
        /// Every session, whatever its age.
        #[arg(long)]
        all: bool,
        /// Claude Code's folder, when it is not `CLAUDE_CONFIG_DIR` or `~/.claude`.
        #[arg(long)]
        claude_dir: Option<String>,
    },
}

pub async fn run(source: Source, client: &mut Client, json: bool) -> anyhow::Result<()> {
    match source {
        Source::Conductor {
            workspaces,
            dry_run,
            include_archived,
            yes,
            db,
        } => {
            let params = ConductorImportParams {
                db,
                workspaces,
                include_archived,
                dry_run: true,
            };
            conductor(params, dry_run, yes, client, json).await
        }
        Source::ClaudeCode {
            dry_run,
            yes,
            since,
            all,
            claude_dir,
        } => {
            // A number of days, or a date the daemon reads.
            let (since_days, since_date) = match since {
                Some(since) => match since.trim().parse::<u32>() {
                    Ok(days) => (Some(days), None),
                    Err(_) => (None, Some(since)),
                },
                None => (None, None),
            };
            // The daemon runs elsewhere: `~` is the home folder, a relative folder the one from here.
            let claude_dir = claude_dir
                .map(
                    |dir| match (dir.strip_prefix("~/"), std::env::var_os("HOME")) {
                        (Some(rest), Some(home)) => Ok(std::path::Path::new(&home)
                            .join(rest)
                            .to_string_lossy()
                            .into_owned()),
                        _ => std::path::absolute(&dir).map(|p| p.to_string_lossy().into_owned()),
                    },
                )
                .transpose()?;
            let params = ClaudeCodeImportParams {
                since_days,
                since_date,
                all,
                dry_run,
                claude_dir,
            };
            claude_code(params, yes, client, json).await
        }
    }
}

async fn claude_code(
    params: ClaudeCodeImportParams,
    yes: bool,
    client: &mut Client,
    json: bool,
) -> anyhow::Result<()> {
    // Branches to release: the plan first, and the user's yes before anything is released.
    if !params.dry_run && !yes {
        let planned: ClaudeCodeImportResult = client
            .call(ClientCall::ClaudeCodeImport(ClaudeCodeImportParams {
                dry_run: true,
                ..params.clone()
            }))
            .await?;
        if planned.releases.iter().any(|r| released_now(&planned, r)) {
            if !json {
                println!("{}", sanitize_terminal(&render_claude_code(&planned)));
            }
            eprintln!(
                "\n{}",
                sanitize_terminal(&claude_code_release_warning(&planned))
            );
            if !crate::confirm("Continue?")? {
                bail!("Nothing was imported.");
            }
        }
    }
    let plan: ClaudeCodeImportResult = client.call(ClientCall::ClaudeCodeImport(params)).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        println!("{}", sanitize_terminal(&render_claude_code(&plan)));
        println!("\n{}", claude_code_outcome(&plan));
    }
    let failed = plan
        .sessions
        .iter()
        .filter(|s| s.outcome == ClaudeCodeOutcome::Failed)
        .count();
    if failed > 0 {
        bail!("{failed} Claude Code session(s) could not be imported");
    }
    Ok(())
}

async fn conductor(
    params: ConductorImportParams,
    dry_run: bool,
    yes: bool,
    client: &mut Client,
    json: bool,
) -> anyhow::Result<()> {
    let params = ConductorImportParams {
        dry_run: true,
        ..params
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

fn render_claude_code(plan: &ClaudeCodeImportResult) -> String {
    let window = match (&plan.since_date, plan.since_days) {
        (Some(date), _) => format!("active since {date}"),
        (None, Some(days)) => format!("active in the last {days} days"),
        (None, None) => "all".into(),
    };
    let mut out = format!(
        "Claude Code: {}\n\nSessions ({}, {window}):",
        plan.claude_dir,
        plan.sessions.len()
    );
    if plan.sessions.is_empty() {
        out.push_str(" none");
    }
    for s in &plan.sessions {
        let day = s.last_active.get(..10).unwrap_or(&s.last_active);
        let lands_in = s.lands_in.as_deref().unwrap_or("nowhere");
        let state = match s.outcome {
            ClaudeCodeOutcome::Import if plan.dry_run => format!("→ {lands_in}"),
            ClaudeCodeOutcome::Import => format!("→ {lands_in}, in a later version"),
            ClaudeCodeOutcome::Imported => format!("imported → {lands_in}"),
            ClaudeCodeOutcome::AlreadyImported => match &s.lands_in {
                Some(into) => format!("imported earlier → {into}"),
                None => "imported earlier".into(),
            },
            ClaudeCodeOutcome::Failed => {
                format!("failed: {}", s.reason.as_deref().unwrap_or_default())
            }
            ClaudeCodeOutcome::Running => {
                format!("refused: {}", s.reason.as_deref().unwrap_or_default())
            }
            ClaudeCodeOutcome::Skipped => {
                format!("skipped: {}", s.reason.as_deref().unwrap_or_default())
            }
        };
        out.push_str(&format!("\n  {day}  {}  ({})  {state}", s.title, s.folder));
        let live: Vec<String> = s
            .branches
            .iter()
            .filter(|b| b.live)
            .map(|b| format!("{} {}", b.repo, b.branch))
            .collect();
        let others = s.branches.len() - live.len();
        if !s.branches.is_empty() {
            let mut line = if live.is_empty() {
                "no live branch".to_string()
            } else {
                format!("live: {}", live.join(", "))
            };
            if others > 0 {
                line.push_str(&format!(" (+{others} merged or deleted)"));
            }
            out.push_str(&format!("\n      {line}"));
        }
    }
    out.push_str("\n\nWorkspaces:");
    if plan.workspaces.is_empty() {
        out.push_str(" none");
    }
    for w in &plan.workspaces {
        let kind = match w.kind {
            ClaudeCodeWorkspaceKind::Branch if w.parent.is_some() => "child",
            ClaudeCodeWorkspaceKind::Branch => "branch",
            ClaudeCodeWorkspaceKind::Parent => "parent",
            ClaudeCodeWorkspaceKind::Changes => "changes",
        };
        let mut parts = vec![
            w.branch.clone().unwrap_or_else(|| "a new branch".into()),
            w.repos.join(" + "),
            format!("{} chat(s)", w.sessions.len()),
        ];
        if let Some(parent) = &w.parent {
            parts.push(format!("child of {parent}"));
        }
        if !w.changes_from.is_empty() {
            parts.push(format!(
                "carries the uncommitted changes on the default branch of {}",
                w.changes_from.join(", ")
            ));
        }
        match (w.outcome, &w.workspace) {
            (ClaudeCodeOutcome::Imported, Some(slug)) => parts.push(format!("imported → {slug}")),
            (ClaudeCodeOutcome::Failed, _) => parts.push(format!(
                "failed: {}",
                w.reason.as_deref().unwrap_or_default()
            )),
            (ClaudeCodeOutcome::Import, Some(slug)) => {
                parts.push(format!("into {slug} (made by an earlier import)"))
            }
            (ClaudeCodeOutcome::Import, None) if !plan.dry_run => {
                parts.push("in a later version".into())
            }
            _ => {}
        }
        out.push_str(&format!("\n  {kind:7}  {}   {}", w.name, parts.join(", ")));
    }
    out.push_str("\n\nHistory explorations:");
    if plan.histories.is_empty() {
        out.push_str(" none");
    }
    for h in &plan.histories {
        let into = match (&h.workspace, plan.dry_run) {
            (Some(slug), true) => format!(", into {slug} (made by an earlier import)"),
            (Some(slug), false) => format!(", in {slug}"),
            (None, _) => String::new(),
        };
        out.push_str(&format!(
            "\n  {} at {}   {} session(s), closed tabs resumable read-only{into}",
            h.repo,
            h.base,
            h.sessions.len()
        ));
    }
    if !plan.releases.is_empty() {
        out.push_str(
            "\n\nBranches to release (HEAD detached on the same commit there, its changes copied):",
        );
        for r in &plan.releases {
            let only = if r.commits_only {
                ", gone or half deleted: its commits only"
            } else {
                ""
            };
            out.push_str(&format!(
                "\n  {} {}   checked out in {}{only}",
                r.repo, r.branch, r.folder
            ));
        }
    }
    if !plan.warnings.is_empty() {
        out.push_str("\n\nWarnings:");
        for warning in &plan.warnings {
            out.push_str(&format!("\n  {warning}"));
        }
    }
    out
}

/// What the import did, after its plan.
fn claude_code_outcome(plan: &ClaudeCodeImportResult) -> String {
    if plan.dry_run {
        return "Dry run: nothing changed.".into();
    }
    let count = |outcome| {
        plan.sessions
            .iter()
            .filter(|s| s.outcome == outcome)
            .count()
    };
    let (imported, failed, later) = (
        count(ClaudeCodeOutcome::Imported),
        count(ClaudeCodeOutcome::Failed),
        count(ClaudeCodeOutcome::Import),
    );
    let mut lines = vec![if imported == 0 {
        "No session imported.".to_string()
    } else {
        format!(
            "{imported} session(s) imported. Each is a copy: the session in Claude Code and its chat in Chauffeur no longer sync."
        )
    }];
    // Where a branch was released, the user's checkout keeps its files and changes.
    let released: Vec<&ClaudeCodeRelease> = plan
        .releases
        .iter()
        .filter(|r| !r.commits_only && released_in(plan, r))
        .collect();
    if !released.is_empty() {
        lines.push(
            "Released, these checkouts keep their files and uncommitted changes on a detached HEAD, copied into the workspace; once you no longer need them there, clean each one:".into(),
        );
        for r in released {
            lines.push(format!(
                "  git -C {} stash --include-untracked   ({} {})",
                r.folder, r.repo, r.branch
            ));
        }
    }
    if failed > 0 {
        lines.push(format!(
            "{failed} could not be imported (see above). Once that is fixed, run the import again: it takes them."
        ));
    }
    if later > 0 {
        lines.push(format!(
            "{later} in an orchestration or with default-branch changes come in a later version: nothing changed for them."
        ));
    }
    lines.join("\n")
}

/// The workspace of a release's branch was made by this run.
fn released_in(plan: &ClaudeCodeImportResult, release: &ClaudeCodeRelease) -> bool {
    plan.workspaces.iter().any(|w| {
        w.outcome == ClaudeCodeOutcome::Imported
            && w.branch.as_deref() == Some(release.branch.as_str())
            && w.repos.contains(&release.repo)
    })
}

/// A release this version makes: of a branch's own workspace (not an orchestration's).
fn released_now(plan: &ClaudeCodeImportResult, release: &ClaudeCodeRelease) -> bool {
    plan.workspaces.iter().any(|w| {
        w.kind == ClaudeCodeWorkspaceKind::Branch
            && w.parent.is_none()
            && w.workspace.is_none()
            && w.branch.as_deref() == Some(release.branch.as_str())
            && w.repos.contains(&release.repo)
    })
}

/// The warning the user answers before any branch is released.
fn claude_code_release_warning(plan: &ClaudeCodeImportResult) -> String {
    let mut out = String::from(
        "These branches will be released where they are checked out, for their workspaces:\n",
    );
    for r in plan.releases.iter().filter(|r| released_now(plan, r)) {
        out.push_str(&format!("  {} {}   in {}\n", r.repo, r.branch, r.folder));
    }
    out.push_str(
        "Those checkouts keep their files and uncommitted changes, on a detached HEAD on the same commit; the changes are copied into the workspace.",
    );
    out
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
