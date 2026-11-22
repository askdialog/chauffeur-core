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
        /// Also save a released checkout's changes, once copied, in a git ref kept as long as an
        /// archive's snapshot (30 days by default), and put the checkout back on its default
        /// branch: one copy of the work is left, Chauffeur's.
        #[arg(long)]
        move_changes: bool,
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
            move_changes,
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
                move_changes,
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
        if !planned.releases.is_empty() {
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
        println!("\n{}", sanitize_terminal(&claude_code_outcome(&plan)));
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
            ClaudeCodeOutcome::Import => format!("→ {lands_in}"),
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
            (ClaudeCodeOutcome::AlreadyImported, Some(slug)) => {
                parts.push(format!("imported earlier → {slug}"))
            }
            (ClaudeCodeOutcome::Failed, _) => parts.push(format!(
                "failed: {}",
                w.reason.as_deref().unwrap_or_default()
            )),
            (ClaudeCodeOutcome::Import, Some(slug)) => {
                parts.push(format!("into {slug} (made by an earlier import)"))
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
    if !plan.setups.is_empty() {
        out.push_str(&claude_code_setups(plan));
    }
    if !plan.releases.is_empty() {
        out.push_str(
            "\n\nBranches to release (HEAD detached on the same commit there, its changes copied):",
        );
        for r in &plan.releases {
            let only = if r.commits_only {
                ", gone or half deleted: its commits only"
            } else if plan.move_changes {
                ", then cleaned (its changes saved in a git ref kept as an archive's snapshot is) and put back on its default branch, unless git hides some of its changes (the import says which it leaves)"
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

/// The setup scripts guessed for the repos without one, and where they go.
fn claude_code_setups(plan: &ClaudeCodeImportResult) -> String {
    let written = if plan.dry_run {
        "to write to"
    } else {
        "written to"
    };
    let mut out = format!(
        "\n\nSetup scripts, guessed from the lockfiles of the repos that have none ({written} your settings' [repos.<name>.scripts] setup, never the repo):"
    );
    for s in &plan.setups {
        let line = match &s.command {
            Some(command) => format!("{command}   (from {})", s.lockfiles.join(", ")),
            None => format!(
                "none proposed: lockfiles only in subfolders ({})",
                s.lockfiles.join(", ")
            ),
        };
        out.push_str(&format!("\n  {}   {line}", s.repo));
    }
    if plan.setups.iter().any(|s| s.command.is_some()) {
        out.push_str("\n  A guess: check it, then commit it in the repo's .chauffeur/settings.toml ([scripts] setup) to share it.");
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
    let (imported, failed) = (
        count(ClaudeCodeOutcome::Imported),
        count(ClaudeCodeOutcome::Failed),
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
        .filter(|r| !r.commits_only && r.moved.is_none() && released_in(plan, r))
        .collect();
    if !released.is_empty() {
        lines.push(
            "Released, these checkouts keep their files and uncommitted changes on a detached HEAD, copied into the workspace; once you no longer need them there, clean each one:".into(),
        );
        for r in released {
            lines.push(format!(
                "  git -C {} stash --include-untracked   ({} {})",
                shell_quoted(&r.folder),
                r.repo,
                r.branch
            ));
        }
    }
    // Moved out: one copy left, Chauffeur's; the other kept as an archive's snapshot is.
    for r in &plan.releases {
        if let Some(moved) = &r.moved {
            let folder = shell_quoted(&r.folder);
            // Quoted: `^` is a pattern to zsh with `extendedglob`.
            let (saved, parent) = (
                shell_quoted(&moved.saved_in),
                shell_quoted(&format!("{}^", moved.saved_in)),
            );
            lines.push(format!(
                "{} is back on {}, cleaned; its changes are saved in {} as an archive's snapshot is. To bring them back as they were (detached on the branch's commit): git -C {folder} switch --detach {parent} && git -C {folder} read-tree -u --reset {saved} && git -C {folder} reset -q",
                r.folder, moved.back_on, moved.saved_in,
            ));
        }
    }
    // Gone or half deleted: their commits came, whatever changes they still hold stay there.
    for r in plan
        .releases
        .iter()
        .filter(|r| r.commits_only && released_in(plan, r))
    {
        lines.push(format!(
            "{} {} came with its commits only: whatever {} still holds stays there.",
            r.repo, r.branch, r.folder
        ));
    }
    if failed > 0 {
        lines.push(format!(
            "{failed} could not be imported (see above). Once that is fixed, run the import again: it takes them."
        ));
    }
    // Copied, the default-branch changes stay in the clone too.
    for w in plan
        .workspaces
        .iter()
        .filter(|w| w.outcome == ClaudeCodeOutcome::Imported && !w.changes_from.is_empty())
    {
        let slug = w.workspace.as_deref().unwrap_or(&w.name);
        lines.push(format!(
            "The uncommitted changes on the default branch of {} are copied into {slug}; the clone keeps them as well: once you no longer need them there, put them away in it (`git stash --include-untracked`).",
            w.changes_from.join(", ")
        ));
    }
    lines.join("\n")
}

/// A word the shell reads as it is: quoted when it holds anything but plain path characters.
fn shell_quoted(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+=:@%,".contains(c))
    {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// The workspace of a release's branch was made by this run.
fn released_in(plan: &ClaudeCodeImportResult, release: &ClaudeCodeRelease) -> bool {
    plan.workspaces.iter().any(|w| {
        w.outcome == ClaudeCodeOutcome::Imported
            && w.branch.as_deref() == Some(release.branch.as_str())
            && w.repos.contains(&release.repo)
    })
}

/// The warning the user answers before any branch is released.
fn claude_code_release_warning(plan: &ClaudeCodeImportResult) -> String {
    let mut out = String::from(
        "These branches will be released where they are checked out, for their workspaces:\n",
    );
    for r in &plan.releases {
        out.push_str(&format!("  {} {}   in {}\n", r.repo, r.branch, r.folder));
    }
    out.push_str(if plan.move_changes {
        "Their uncommitted changes are copied into the workspace (not those of a checkout gone or half deleted), then saved in a git ref kept as an archive's snapshot is (30 days by default); the checkouts are cleaned and put back on their default branch, but for one whose changes git does not fully show (files marked --assume-unchanged or --skip-worktree, a merge or rebase under way), left as it is and named."
    } else {
        "Those checkouts keep their files and uncommitted changes, on a detached HEAD on the same commit; the changes are copied into the workspace (not those of a checkout gone or half deleted)."
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::claudecode::{ClaudeCodeSetup, ClaudeCodeWorkspacePlan};

    fn claude_code_plan(dry_run: bool, outcome: ClaudeCodeOutcome) -> ClaudeCodeImportResult {
        let release = |branch: &str, folder: &str, commits_only| ClaudeCodeRelease {
            repo: "shop".into(),
            branch: branch.into(),
            folder: folder.into(),
            commits_only,
            moved: None,
        };
        let workspace = |branch: &str| ClaudeCodeWorkspacePlan {
            name: branch.into(),
            kind: ClaudeCodeWorkspaceKind::Branch,
            branch: Some(branch.into()),
            repos: vec!["shop".into()],
            parent: None,
            sessions: vec!["s".into()],
            changes_from: Vec::new(),
            outcome,
            reason: None,
            workspace: (outcome == ClaudeCodeOutcome::Imported).then(|| branch.replace('/', "-")),
        };
        ClaudeCodeImportResult {
            dry_run,
            move_changes: false,
            since_days: Some(60),
            since_date: None,
            claude_dir: "/c".into(),
            sessions: Vec::new(),
            workspaces: vec![workspace("feat/a"), workspace("feat/gone")],
            histories: Vec::new(),
            releases: vec![
                release("feat/a", "/code/shop", false),
                release("feat/gone", "/tmp/gone", true),
            ],
            setups: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn guessed_setups_are_shown_as_guesses_to_commit() {
        let mut plan = claude_code_plan(true, ClaudeCodeOutcome::Import);
        plan.setups = vec![
            ClaudeCodeSetup {
                repo: "shop".into(),
                command: Some("pnpm install".into()),
                lockfiles: vec!["pnpm-lock.yaml".into()],
            },
            ClaudeCodeSetup {
                repo: "docs".into(),
                command: None,
                lockfiles: vec!["app/package-lock.json".into()],
            },
        ];
        let out = render_claude_code(&plan);
        assert!(
            out.contains("shop   pnpm install   (from pnpm-lock.yaml)"),
            "{out}"
        );
        assert!(
            out.contains(
                "docs   none proposed: lockfiles only in subfolders (app/package-lock.json)"
            ),
            "{out}"
        );
        assert!(
            out.contains(
                "A guess: check it, then commit it in the repo's .chauffeur/settings.toml"
            ),
            "{out}"
        );
        // Nothing guessed: no guess to check.
        plan.setups.remove(0);
        assert!(!render_claude_code(&plan).contains("A guess"));
    }

    #[test]
    fn with_move_changes_the_plan_says_who_is_cleaned_and_the_outcome_where_the_changes_went() {
        let mut planned = claude_code_plan(true, ClaudeCodeOutcome::Import);
        planned.move_changes = true;
        let plan = render_claude_code(&planned);
        assert!(
            plan.contains("shop feat/a   checked out in /code/shop, then cleaned (its changes saved in a git ref kept as an archive's snapshot is) and put back on its default branch, unless git hides some of its changes (the import says which it leaves)"),
            "{plan}"
        );
        assert!(claude_code_release_warning(&planned).contains("the checkouts are cleaned"));
        let mut done = claude_code_plan(false, ClaudeCodeOutcome::Imported);
        done.move_changes = true;
        done.releases[0].moved = Some(chauffeur_protocol::claudecode::ClaudeCodeMoved {
            saved_in: "refs/chauffeur/claude-code/w/shop".into(),
            back_on: "main".into(),
        });
        let said = claude_code_outcome(&done);
        assert!(
            said.contains("/code/shop is back on main, cleaned; its changes are saved in refs/chauffeur/claude-code/w/shop as an archive's snapshot is. To bring them back as they were (detached on the branch's commit): git -C /code/shop switch --detach 'refs/chauffeur/claude-code/w/shop^' && git -C /code/shop read-tree -u --reset refs/chauffeur/claude-code/w/shop && git -C /code/shop reset -q"),
            "{said}"
        );
        assert!(!said.contains("git -C /code/shop stash"), "{said}");
    }

    #[test]
    fn a_cleanup_command_quotes_a_folder_the_shell_would_split() {
        assert_eq!(shell_quoted("/code/shop"), "/code/shop");
        assert_eq!(shell_quoted("/My Code/shop"), "'/My Code/shop'");
        assert_eq!(shell_quoted("/a/it's"), "'/a/it'\\''s'");
    }

    #[test]
    fn the_claude_code_warning_names_each_release_and_what_its_checkout_keeps() {
        let warning =
            claude_code_release_warning(&claude_code_plan(true, ClaudeCodeOutcome::Import));
        assert!(warning.contains("shop feat/a   in /code/shop"), "{warning}");
        assert!(
            warning.contains("shop feat/gone   in /tmp/gone"),
            "{warning}"
        );
        assert!(
            warning.contains("detached HEAD on the same commit"),
            "{warning}"
        );
    }

    #[test]
    fn after_a_claude_code_import_each_released_checkout_says_how_to_clean_it() {
        let done = claude_code_outcome(&claude_code_plan(false, ClaudeCodeOutcome::Imported));
        assert!(
            done.contains("git -C /code/shop stash --include-untracked   (shop feat/a)"),
            "{done}"
        );
        assert!(!done.contains("git -C /tmp/gone"), "{done}");
        // A folder with a space is quoted, so the line runs as it is.
        let mut spaced = claude_code_plan(false, ClaudeCodeOutcome::Imported);
        for r in &mut spaced.releases {
            r.folder = r.folder.replace("/code/", "/My Code/");
        }
        let done_spaced = claude_code_outcome(&spaced);
        assert!(
            done_spaced.contains("git -C '/My Code/shop' stash --include-untracked"),
            "{done_spaced}"
        );
        assert!(
            done.contains("shop feat/gone came with its commits only: whatever /tmp/gone still holds stays there."),
            "{done}"
        );
        // A workspace that failed released nothing: no line for it.
        let failed = claude_code_outcome(&claude_code_plan(false, ClaudeCodeOutcome::Failed));
        assert!(!failed.contains("git -C"), "{failed}");
        assert!(!failed.contains("commits only"), "{failed}");
    }

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
