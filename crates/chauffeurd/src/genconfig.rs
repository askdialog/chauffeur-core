//! The files Chauffeur writes at the root of a workspace so that an agent started there works
//! across every repo as it would inside one: instructions (CLAUDE.md, AGENTS.md), each repo's
//! hooks, plugins and permissions merged into `.claude/settings.json`, MCP servers merged into
//! `.mcp.json`, and a SessionStart hook that prints the git state of every repo.
//!
//! Claude Code reads project settings only from the cwd's `.claude/`, never from subfolders, so
//! without this merge a repo's hooks (web's eslint and tsc) would silently not run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chauffeur_protocol::model::TicketRef;
use serde_json::{Map, Value, json};

pub struct RepoEntry {
    pub repo: String,
    /// Absolute path of the worktree.
    pub path: PathBuf,
    /// The clone the worktree was made from (Chauffeur's own copy for a workspace restored from
    /// the cloud); none while a cloud restore writes the config.
    pub clone_path: Option<PathBuf>,
    pub base_branch: String,
}

pub struct WorkspaceConfig<'a> {
    pub slug: &'a str,
    pub branch: &'a str,
    pub root: &'a Path,
    pub repos: &'a [RepoEntry],
    pub tickets: &'a [TicketRef],
    pub port_base: u16,
    pub port_count: u16,
    pub parent_slug: Option<&'a str>,
    /// Absolute path of the `chauffeur` CLI, used in hook commands.
    pub cli: &'a Path,
    /// An exploration: read-only, and without the CLI (its agents have nothing to ask the daemon).
    pub explore: bool,
}

/// Writes every generated file; returns warnings worth showing (e.g. clashing MCP servers).
pub fn write(config: &WorkspaceConfig) -> anyhow::Result<Vec<String>> {
    let mut warnings = Vec::new();
    for planted in [
        ".claude",
        ".claude/settings.json",
        "CLAUDE.md",
        "AGENTS.md",
        ".mcp.json",
    ] {
        refuse_symlink(&config.root.join(planted))?;
    }
    std::fs::create_dir_all(config.root.join(".claude"))?;
    std::fs::write(config.root.join("CLAUDE.md"), claude_md(config))?;
    std::fs::write(config.root.join("AGENTS.md"), agents_md(config))?;
    let settings = merged_settings(config, &mut warnings)?;
    std::fs::write(
        config.root.join(".claude/settings.json"),
        serde_json::to_string_pretty(&settings)? + "\n",
    )?;
    let mcp = merged_mcp(config, &mut warnings)?;
    let mcp_path = config.root.join(".mcp.json");
    if mcp["mcpServers"].as_object().is_some_and(|s| !s.is_empty()) {
        std::fs::write(&mcp_path, serde_json::to_string_pretty(&mcp)? + "\n")?;
    } else if mcp_path.exists() {
        std::fs::remove_file(&mcp_path)?;
    }
    Ok(warnings)
}

/// An agent could replace a generated file (or `.claude`) by a symlink to one of the user's, which
/// the next regeneration would then overwrite: nothing is written through one, nor removed.
fn refuse_symlink(path: &Path) -> anyhow::Result<()> {
    if path
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        anyhow::bail!(
            "{} is a symlink: the generated config is not written through it",
            path.display()
        );
    }
    Ok(())
}

fn header(config: &WorkspaceConfig) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Chauffeur workspace `{}`\n\nThis folder is a Chauffeur workspace: one git worktree per repo, side by side, all on branch `{}`. The folder itself is not a git repository.\n\n",
        config.slug, config.branch
    ));
    out.push_str("| Repo | Folder | Base branch |\n|---|---|---|\n");
    for repo in config.repos {
        out.push_str(&format!(
            "| {} | `{}/` | `{}` |\n",
            repo.repo, repo.repo, repo.base_branch
        ));
    }
    out.push('\n');
    if !config.tickets.is_empty() {
        out.push_str("Linear tickets: ");
        let tickets: Vec<String> = config
            .tickets
            .iter()
            .map(|t| match &t.title {
                Some(title) => format!("{} ({title})", t.identifier),
                None => t.identifier.clone(),
            })
            .collect();
        out.push_str(&tickets.join(", "));
        out.push_str("\n\n");
    }
    if let Some(parent) = config.parent_slug {
        out.push_str(&format!(
            "This is a child workspace of `{parent}`: an orchestrating agent created it and may follow it with `chauffeur wait`.\n\n"
        ));
    }
    if config.explore {
        out.push_str(
            r#"## Working here

This workspace is an exploration, read-only: each repo folder is a detached checkout of its base branch. Read, search, run read-only commands and explain; edits, commits, pushes, branches, worktrees and GitHub writes are refused. When the user wants code changed, tell them to create a Chauffeur workspace for it.

When the question needs a repo this exploration lacks, propose it with the `add_repo` tool (`list_repos` lists the organization's), saying why: the user answers Add or Not now. Never clone a repo yourself; without these tools, ask the user to add the repo from Chauffeur.

To have the other agent (Codex from Claude, Claude from Codex) look into something here, use the `delegate` tool: it starts a delegated chat the user sees, read-only as this one, and returns its answer.
"#,
        );
        return out;
    }
    let last_port = config.port_base + config.port_count.saturating_sub(1);
    out.push_str(&format!(
        r#"## Working here

- Each repo folder is its own git repository: run git inside it (`git -C <repo> status`), commit in each repo that changed, and open one PR per repo.
- Never run `git worktree add`: Chauffeur tracks and cleans every worktree. Run `chauffeur worktree add --repo <repo> [--name <name>]` instead; it prints the folder to work in. `chauffeur worktree remove <name>` removes it.
- To hand a ticket to another agent with its own chat, worktrees and branch: `chauffeur new --parent "$CHAUFFEUR_WORKSPACE_ID" --ticket <ID> --repo <repo> --prompt -` with the brief on standard input, then `chauffeur wait <child>` and `chauffeur send <child> -` the same way. Pass a brief or a message through a quoted heredoc (`<<'EOF'`), never between double quotes: the shell runs a backticked command and expands `$x` there, and the child gets the brief without them. Write the brief from the ticket: a limit you set (a folder not to touch) names the ticket's exceptions. `wait` also returns when the child asks a question or waits on a permission, and prints each request with the `chauffeur respond` line that answers it; the user sees it in your chat too and may answer it there. Answer it yourself when what you know settles it; otherwise tell the user what you know that bears on it, and leave the answer to them in the child's card, without asking it again yourself. A `respond` that finds no open request means it was answered already: never send the answer again as a message.
- For a review or a second opinion, or a long or parallel piece of work the user may want to follow, use the `delegate` tool: it starts a delegated chat the user sees in this workspace's folders, waits for its answer and returns it; pass its `chat` to give it more work. It runs on your own agent, a review on the other one (Codex from Claude, Claude from Codex); pick its model with `tier`. A quick search or a small sub-task stays with your own subagents. Leave the files it changes alone until it answered.
- When the work needs a repo this workspace lacks, propose it with the `add_repo` tool (`list_repos` lists the organization's), saying why: the user answers Add or Not now, and an added repo comes on this workspace's branch. Never clone a repo yourself nor run `chauffeur add-repo`; without these tools, ask the user to add the repo from Chauffeur.
- Skills written for plain git (implement-ticket, run-milestone…) create and remove worktrees themselves: follow them, but with these commands. A worktree a skill would add becomes `chauffeur worktree add`; a sub-agent per ticket becomes a child workspace (`chauffeur new --parent`); removing one becomes `chauffeur worktree remove`.
- To start a repo's dev server or local stack, use its run scripts (declared in its `.chauffeur/settings.toml`), not a background command of your own: `chauffeur ps "$CHAUFFEUR_WORKSPACE_ID"` lists the scripts and what already runs (start nothing twice), `chauffeur run "$CHAUFFEUR_WORKSPACE_ID" <repo> [script]` starts one, `chauffeur ps "$CHAUFFEUR_WORKSPACE_ID" --json` gives each process's last lines of output, `chauffeur stop <process id>` stops it. The user then sees it in the Processes tab and can stop it. A repo without run scripts: run it yourself.
- Ports {}–{} are reserved for this workspace (`$CHAUFFEUR_PORT` is the first). Paths of the repos: `$CHAUFFEUR_REPO_PATH_<REPO>` (upper case, `-` as `_`).
- Notes, plans and scratch files go in `.chauffeur/`: it is kept when the workspace is archived, the repos are not.
"#,
        config.port_base, last_port
    ));
    // Written by the import from Conductor (docs/adr/conductor-import.md).
    if config.root.join(".chauffeur/conductor").is_dir() {
        out.push_str("- This workspace was imported from Conductor. Its earlier chats are in `.chauffeur/conductor/`, one Markdown file per chat (Conductor's notes in `.chauffeur/conductor/context/`): read them when the user refers to earlier work.\n");
    }
    out
}

fn claude_md(config: &WorkspaceConfig) -> String {
    let mut out = header(config);
    // A repo's CLAUDE.md, or its AGENTS.md when it has no CLAUDE.md (api).
    let imports: Vec<String> = config
        .repos
        .iter()
        .filter_map(|repo| {
            ["CLAUDE.md", "AGENTS.md"]
                .into_iter()
                .find(|file| repo.path.join(file).exists())
                .map(|file| format!("@{}/{file}", repo.repo))
        })
        .collect();
    if !imports.is_empty() {
        out.push_str("\n## Instructions of each repo\n\n");
        out.push_str(&imports.join("\n"));
        out.push('\n');
    }
    out
}

fn agents_md(config: &WorkspaceConfig) -> String {
    let mut out = header(config);
    for repo in config.repos {
        let file = ["AGENTS.md", "CLAUDE.md"]
            .iter()
            .map(|name| repo.path.join(name))
            .find(|path| path.exists());
        let Some(file) = file else { continue };
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        out.push_str(&format!(
            "\n## Instructions of `{}` (from `{}/{}`)\n\n",
            repo.repo,
            repo.repo,
            file.file_name().unwrap().to_string_lossy()
        ));
        out.push_str(&text);
        out.push('\n');
    }
    out
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn merged_settings(config: &WorkspaceConfig, warnings: &mut Vec<String>) -> anyhow::Result<Value> {
    let cli = shell_quote(&config.cli.to_string_lossy());
    let mut hooks: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    hooks.entry("SessionStart".into()).or_default().push(json!({
        "hooks": [{ "type": "command", "command": format!("{cli} git-context") }]
    }));
    let mut plugins = Map::new();
    let mut marketplaces = Map::new();
    let mut allow: Vec<Value> = Vec::new();
    let mut deny: Vec<Value> = Vec::new();
    let mut ask: Vec<Value> = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    for repo in config.repos {
        // The repo's folder under the root; a repo elsewhere keeps its rules as written.
        let folder = repo
            .path
            .strip_prefix(config.root)
            .ok()
            .map(|folder| folder.display().to_string())
            .filter(|folder| !folder.is_empty());
        let clone_paths = repo
            .clone_path
            .as_deref()
            .map(|clone| clone_spellings(clone, home.as_deref()))
            .unwrap_or_default();
        for file in settings_files(repo) {
            let Some(settings) = read_json(&file) else {
                continue;
            };
            if let Some(events) = settings.get("hooks").and_then(Value::as_object) {
                for (event, groups) in events {
                    for group in groups.as_array().into_iter().flatten() {
                        hooks
                            .entry(event.clone())
                            .or_default()
                            .push(wrap_hook_group(group, &repo.path, &cli));
                    }
                }
            }
            for (key, target) in [
                ("enabledPlugins", &mut plugins),
                ("extraKnownMarketplaces", &mut marketplaces),
            ] {
                if let Some(map) = settings.get(key).and_then(Value::as_object) {
                    for (name, value) in map {
                        let keep = target.get(name).and_then(Value::as_bool) == Some(true);
                        if !keep {
                            target.insert(name.clone(), value.clone());
                        }
                    }
                }
            }
            if let Some(permissions) = settings.get("permissions") {
                for (key, list) in [
                    ("allow", &mut allow),
                    ("deny", &mut deny),
                    ("ask", &mut ask),
                ] {
                    for rule in permissions
                        .get(key)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        // Ask and deny rules only restrict: kept whatever they name.
                        if key == "allow"
                            && let Some(text) = rule.as_str()
                            && names_path(text, &clone_paths)
                        {
                            warnings.push(format!(
                                "{}: left out `{text}` from {}: it names the clone, and workspace \
                                 agents work in its worktree; write it relative to the repo \
                                 (`Edit(/src/**)`) to have it in workspaces",
                                repo.repo,
                                file.display()
                            ));
                            continue;
                        }
                        let rule = folder.as_deref().map_or_else(
                            || rule.clone(),
                            |folder| anchor_rule_at_repo(rule, key, folder),
                        );
                        if !list.contains(&rule) {
                            list.push(rule);
                        }
                    }
                }
            }
        }
    }
    let mut out = Map::new();
    out.insert(
        "$comment".into(),
        json!(
            "Generated by Chauffeur from each repo's .claude/settings.json and \
             .claude/settings.local.json (its clone's when the worktree has none); edits are \
             overwritten."
        ),
    );
    out.insert("hooks".into(), json!(hooks));
    if !plugins.is_empty() {
        out.insert("enabledPlugins".into(), Value::Object(plugins));
    }
    if !marketplaces.is_empty() {
        out.insert("extraKnownMarketplaces".into(), Value::Object(marketplaces));
    }
    if !(allow.is_empty() && deny.is_empty() && ask.is_empty()) {
        out.insert(
            "permissions".into(),
            json!({ "allow": allow, "deny": deny, "ask": ask }),
        );
    }
    Ok(Value::Object(out))
}

/// Claude Code resolves the path of a `Read(…)` or `Edit(…)` rule (the only rules with paths)
/// from the session's working directory: a `/src/**` path is anchored there. A path without a
/// leading `/` (`./` aside) is read as in a `.gitignore`: anchored there too when a `/` is left
/// before its end (`src/**`), at any depth below it otherwise (`*.ts`, `dist/`); a deny or ask
/// rule's `src/**` also blocks every `src` folder. A deny or ask rule starting with `!` carves its
/// paths out of the relative rules listed before it. A repo's rules were written for a session
/// started in the repo; merged here, at the workspace root, the anchored ones would miss the repo
/// and the others would reach every repo. They are rewritten to keep their reach inside the repo's
/// `folder`: `/src/**` becomes `/web/src/**`, an allow `*.ts` becomes `/web/**/*.ts`, a deny
/// `*.env` becomes `web/**/*.env` (relative, at least as wide as in the repo, so that a `!` rule
/// can still carve out of it) and `!sample.env` becomes `!web/**/sample.env`. Absolute (`//`) and
/// home (`~`) paths, and rules without a path, are kept.
fn anchor_rule_at_repo(rule: &Value, kind: &str, folder: &str) -> Value {
    let Some((tool, path)) = rule
        .as_str()
        .and_then(|text| text.strip_suffix(')'))
        .and_then(|text| text.split_once('('))
        .filter(|(tool, _)| matches!(*tool, "Read" | "Edit"))
    else {
        return rule.clone();
    };
    let (negated, path) = path
        .strip_prefix('!')
        .map_or((false, path), |path| (true, path));
    if path.starts_with("//") || path.starts_with('~') {
        return rule.clone();
    }
    if let Some(path) = path.strip_prefix('/').filter(|_| !negated) {
        return json!(format!("{tool}(/{folder}/{path})"));
    }
    let (path, anchored) = match path.strip_prefix('/') {
        Some(path) => (path, true),
        None => {
            let path = path.strip_prefix("./").unwrap_or(path);
            (path, path.trim_end_matches('/').contains('/'))
        }
    };
    let rewritten = match (negated, kind == "allow", anchored) {
        (true, _, true) => format!("!{folder}/{path}"),
        (true, _, false) => format!("!{folder}/**/{path}"),
        (false, true, true) => format!("/{folder}/{path}"),
        (false, true, false) => format!("/{folder}/**/{path}"),
        (false, false, _) => format!("{folder}/**/{path}"),
    };
    json!(format!("{tool}({rewritten})"))
}

/// A repo's settings files: its `.claude/settings.json`, and its `.claude/settings.local.json`,
/// the clone's when the worktree has none (the file is gitignored, and Claude Code itself reads
/// it from the main checkout in a worktree).
fn settings_files(repo: &RepoEntry) -> [PathBuf; 2] {
    let local = repo.path.join(".claude/settings.local.json");
    let local = match &repo.clone_path {
        Some(clone) if !local.exists() => clone.join(".claude/settings.local.json"),
        _ => local,
    };
    [repo.path.join(".claude/settings.json"), local]
}

/// How a rule can name the clone: its path as stored and resolved, and from the home folder
/// (`~/code/web`, `$HOME/code/web`, `${HOME}/code/web`).
fn clone_spellings(clone: &Path, home: Option<&Path>) -> Vec<String> {
    let clone: PathBuf = clone.components().collect();
    let mut paths = vec![clone.clone()];
    if let Ok(resolved) = clone.canonicalize()
        && resolved != clone
    {
        paths.push(resolved);
    }
    let mut spellings: Vec<String> = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    for rest in paths
        .iter()
        .filter_map(|path| path.strip_prefix(home?).ok())
    {
        for prefix in ["~", "$HOME", "${HOME}"] {
            spellings.push(format!("{prefix}/{}", rest.display()));
        }
    }
    spellings
}

/// Whether `text` holds one of `spellings` as a whole path: `/code/web/x` and `/code/web)` do,
/// `/code/web.old` and `/backup/code/web` do not.
fn names_path(text: &str, spellings: &[String]) -> bool {
    let in_a_name = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '.');
    spellings.iter().any(|path| {
        text.match_indices(path.as_str()).any(|(at, _)| {
            !text[..at].chars().next_back().is_some_and(in_a_name)
                && !text[at + path.len()..]
                    .chars()
                    .next()
                    .is_some_and(in_a_name)
        })
    })
}

/// Rewrites a repo's hook group so its command hooks run in the repo, and only for files of the
/// repo: `chauffeur hook-run --repo <path> --shell '<command>'`.
fn wrap_hook_group(group: &Value, repo_path: &Path, cli: &str) -> Value {
    let mut group = group.clone();
    if let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) {
        for hook in hooks {
            if hook.get("type").and_then(Value::as_str) != Some("command") {
                continue;
            }
            if let Some(command) = hook.get("command").and_then(Value::as_str) {
                let wrapped = format!(
                    "{cli} hook-run --repo {} --shell {}",
                    shell_quote(&repo_path.to_string_lossy()),
                    shell_quote(command)
                );
                hook["command"] = Value::String(wrapped);
            }
        }
    }
    group
}

fn merged_mcp(config: &WorkspaceConfig, warnings: &mut Vec<String>) -> anyhow::Result<Value> {
    let mut servers = Map::new();
    for repo in config.repos {
        let Some(mcp) = read_json(&repo.path.join(".mcp.json")) else {
            continue;
        };
        for (name, server) in mcp
            .get("mcpServers")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if servers.contains_key(name) {
                if servers[name] != *server {
                    warnings.push(format!(
                        "MCP server {name} of {} clashes with another repo's; the first one is kept",
                        repo.repo
                    ));
                }
                continue;
            }
            servers.insert(name.clone(), server.clone());
        }
    }
    Ok(json!({ "mcpServers": servers }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_hooks_are_wrapped_to_run_in_their_repo_and_plugins_are_merged() {
        let root = tempfile::tempdir().unwrap();
        let ecom = root.path().join("web");
        std::fs::create_dir_all(ecom.join(".claude")).unwrap();
        std::fs::write(ecom.join("CLAUDE.md"), "# ecom").unwrap();
        std::fs::write(
            ecom.join(".claude/settings.json"),
            r#"{"hooks":{"PostToolUse":[{"matcher":"Edit|Write","hooks":[{"type":"command","command":"pnpm test-linter \"$CLAUDE_FILE_PATHS\" --fix"}]}]},
                "enabledPlugins":{"acme-skills@acme-skills":true}}"#,
        )
        .unwrap();
        let api = root.path().join("api");
        std::fs::create_dir_all(api.join(".claude")).unwrap();
        std::fs::write(
            api.join(".claude/settings.json"),
            r#"{"enabledPlugins":{"acme-skills@acme-skills":true,"other@x":false}}"#,
        )
        .unwrap();
        let repos = vec![
            RepoEntry {
                repo: "web".into(),
                path: ecom.clone(),
                clone_path: None,
                base_branch: "staging".into(),
            },
            RepoEntry {
                repo: "api".into(),
                path: api.clone(),
                clone_path: None,
                base_branch: "staging".into(),
            },
        ];
        let config = WorkspaceConfig {
            slug: "xyz-1-x",
            branch: "alice/xyz-1-x",
            root: root.path(),
            repos: &repos,
            tickets: &[],
            port_base: 41000,
            port_count: 10,
            parent_slug: None,
            cli: Path::new("/opt/chauffeur"),
            explore: false,
        };
        write(&config).unwrap();

        let settings: Value = serde_json::from_str(
            &std::fs::read_to_string(root.path().join(".claude/settings.json")).unwrap(),
        )
        .unwrap();
        let command = settings["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(command.starts_with("'/opt/chauffeur' hook-run --repo '"));
        assert!(command.contains("pnpm test-linter"));
        assert_eq!(settings["hooks"]["PostToolUse"][0]["matcher"], "Edit|Write");
        assert_eq!(
            settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "'/opt/chauffeur' git-context"
        );
        assert_eq!(settings["enabledPlugins"]["acme-skills@acme-skills"], true);

        let claude_md = std::fs::read_to_string(root.path().join("CLAUDE.md")).unwrap();
        assert!(claude_md.contains("@web/CLAUDE.md"));
        assert!(!claude_md.contains("@api/"));
        assert!(claude_md.contains("chauffeur worktree add"));
        assert!(claude_md.contains("Skills written for plain git"));
        let agents_md = std::fs::read_to_string(root.path().join("AGENTS.md")).unwrap();
        assert!(agents_md.contains("# ecom"));
    }

    #[test]
    fn a_repos_relative_rules_keep_pointing_into_the_repo_once_merged() {
        let root = tempfile::tempdir().unwrap();
        let web = root.path().join("web");
        let api = root.path().join("api");
        for (repo, rules) in [
            (
                &web,
                r#"{"allow":["Edit(/src/**)","Read(//etc/hosts)","Read(~/notes/**)","Edit(lib/**)",
                    "Read(./docs/**)","Edit(*.ts)","Edit(./*.md)","Edit(dist/)",
                    "Bash(pnpm test *)","WebFetch","mcp__linear"],
                   "ask":["Read(/config/*.json)","Read(config/**)"],
                   "deny":["Edit(/secrets/**)","Edit(./secrets/**)","Read(*.env)","Read(!sample.env)",
                    "Read(!/local.env)","Read(!config/test.env)","Read(!~/x.env)"]}"#,
            ),
            (
                &api,
                r#"{"allow":["Edit(*.ts)"],"deny":["Edit(/secrets/**)"]}"#,
            ),
        ] {
            std::fs::create_dir_all(repo.join(".claude")).unwrap();
            std::fs::write(
                repo.join(".claude/settings.json"),
                format!(r#"{{"permissions":{rules}}}"#),
            )
            .unwrap();
        }
        let repos = [&web, &api].map(|path| RepoEntry {
            repo: path.file_name().unwrap().to_string_lossy().into_owned(),
            path: path.clone(),
            clone_path: None,
            base_branch: "main".into(),
        });
        write(&WorkspaceConfig {
            slug: "xyz-1-x",
            branch: "alice/xyz-1-x",
            root: root.path(),
            repos: &repos,
            tickets: &[],
            port_base: 41000,
            port_count: 10,
            parent_slug: None,
            cli: Path::new("/opt/chauffeur"),
            explore: false,
        })
        .unwrap();

        let settings: Value = serde_json::from_str(
            &std::fs::read_to_string(root.path().join(".claude/settings.json")).unwrap(),
        )
        .unwrap();
        let permissions = &settings["permissions"];
        assert_eq!(
            permissions["allow"],
            json!([
                "Edit(/web/src/**)",
                "Read(//etc/hosts)",
                "Read(~/notes/**)",
                "Edit(/web/lib/**)",
                "Read(/web/docs/**)",
                "Edit(/web/**/*.ts)",
                "Edit(/web/**/*.md)",
                "Edit(/web/**/dist/)",
                "Bash(pnpm test *)",
                "WebFetch",
                "mcp__linear",
                "Edit(/api/**/*.ts)"
            ])
        );
        assert_eq!(
            permissions["ask"],
            json!(["Read(/web/config/*.json)", "Read(web/**/config/**)"])
        );
        assert_eq!(
            permissions["deny"],
            json!([
                "Edit(/web/secrets/**)",
                "Edit(web/**/secrets/**)",
                "Read(web/**/*.env)",
                "Read(!web/**/sample.env)",
                "Read(!web/local.env)",
                "Read(!web/config/test.env)",
                "Read(!~/x.env)",
                "Edit(/api/secrets/**)"
            ])
        );
    }

    /// Writes `settings` as a folder's `.claude/<file>`.
    fn claude_file(folder: &Path, file: &str, settings: &str) {
        std::fs::create_dir_all(folder.join(".claude")).unwrap();
        std::fs::write(folder.join(".claude").join(file), settings).unwrap();
    }

    fn generate(root: &Path, repos: &[RepoEntry]) -> (Value, Vec<String>) {
        let warnings = write(&WorkspaceConfig {
            slug: "xyz-1-x",
            branch: "alice/xyz-1-x",
            root,
            repos,
            tickets: &[],
            port_base: 41000,
            port_count: 10,
            parent_slug: None,
            cli: Path::new("/opt/chauffeur"),
            explore: false,
        })
        .unwrap();
        let settings = serde_json::from_str(
            &std::fs::read_to_string(root.join(".claude/settings.json")).unwrap(),
        )
        .unwrap();
        (settings, warnings)
    }

    #[test]
    fn a_clones_local_settings_apply_without_its_rules_aimed_at_the_clone() {
        let root = tempfile::tempdir().unwrap();
        let clones = tempfile::tempdir().unwrap();
        let worktree = root.path().join("web");
        std::fs::create_dir_all(&worktree).unwrap();
        let clone = clones.path().join("web");
        // An absolute path: `/{c}` is the rule form `//<clone>`.
        let c = clone.display();
        let local = format!(
            r#"{{"hooks":{{"PostToolUse":[{{"matcher":"Edit","hooks":[{{"type":"command","command":"pnpm lint"}}]}}]}},
                "enabledPlugins":{{"mine@x":true}},
                "permissions":{{
                  "allow":["Bash(pnpm test *)","Edit(/{c}/**)","Bash(cd {c} && make)"],
                  "ask":["Read(/{c}/notes/**)"],
                  "deny":["Edit(/{c}/secrets/**)","Edit(/secrets/**)"]}}}}"#
        );
        claude_file(&clone, "settings.local.json", &local);
        let repos = [RepoEntry {
            repo: "web".into(),
            path: worktree.clone(),
            clone_path: Some(clone.clone()),
            base_branch: "main".into(),
        }];

        let (settings, warnings) = generate(root.path(), &repos);

        let hook = settings["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(
            hook.contains(&format!("--repo '{}'", worktree.display())),
            "{hook}"
        );
        assert_eq!(settings["enabledPlugins"]["mine@x"], true);
        let permissions = &settings["permissions"];
        assert_eq!(permissions["allow"], json!(["Bash(pnpm test *)"]));
        // Ask and deny rules only restrict: kept, even when they name the clone.
        assert_eq!(permissions["ask"], json!([format!("Read(/{c}/notes/**)")]));
        assert_eq!(
            permissions["deny"],
            json!([format!("Edit(/{c}/secrets/**)"), "Edit(/web/secrets/**)"])
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        for rule in [format!("Edit(/{c}/**)"), format!("Bash(cd {c} && make)")] {
            assert!(
                warnings
                    .iter()
                    .any(|w| w.contains("web") && w.contains(&rule)),
                "no warning for {rule}: {warnings:?}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(clone.join(".claude/settings.local.json")).unwrap(),
            local
        );
    }

    #[test]
    fn a_worktrees_own_local_settings_win_over_its_clones_and_are_filtered_alike() {
        let root = tempfile::tempdir().unwrap();
        let clone = tempfile::tempdir().unwrap();
        let worktree = root.path().join("web");
        // A copy of the clone's file (`.worktreeinclude`) still names the clone.
        claude_file(
            &worktree,
            "settings.local.json",
            &format!(
                r#"{{"permissions":{{"allow":["Bash(make)","Edit(/{}/**)"]}}}}"#,
                clone.path().display()
            ),
        );
        claude_file(
            clone.path(),
            "settings.local.json",
            r#"{"permissions":{"allow":["Bash(rm *)"]}}"#,
        );
        let repos = [RepoEntry {
            repo: "web".into(),
            path: worktree,
            clone_path: Some(clone.path().to_path_buf()),
            base_branch: "main".into(),
        }];

        let (settings, warnings) = generate(root.path(), &repos);

        assert_eq!(settings["permissions"]["allow"], json!(["Bash(make)"]));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn a_rule_names_the_clone_only_by_its_whole_path_or_from_home() {
        // Stored with a trailing slash, as a user may type it.
        let spellings = clone_spellings(
            Path::new("/Users/al/code/web/"),
            Some(Path::new("/Users/al")),
        );
        for named in [
            "Edit(//Users/al/code/web/**)",
            "Edit(//Users/al/code/web)",
            "Read(~/code/web/src/**)",
            "Bash(cd /Users/al/code/web && make)",
            "Bash(ls ~/code/web)",
            "Bash(cd $HOME/code/web && make)",
            "Bash(cd ${HOME}/code/web *)",
        ] {
            assert!(names_path(named, &spellings), "{named}");
        }
        for other in [
            "Edit(//Users/al/code/web.old/**)",
            "Edit(//Users/al/code/web-api/**)",
            "Read(~/code/webby/**)",
            "Read(//Volumes/backup/Users/al/code/web/**)",
            "Edit(/web/**)",
            "Bash(pnpm test *)",
        ] {
            assert!(!names_path(other, &spellings), "{other}");
        }
    }

    #[test]
    fn nothing_is_written_through_a_planted_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude/settings.json"), "{\"mine\":true}").unwrap();
        std::fs::write(home.join("notes.md"), "mine").unwrap();
        let root = tmp.path().join("ws");
        std::fs::create_dir_all(&root).unwrap();
        let config = WorkspaceConfig {
            slug: "ws",
            branch: "x/ws",
            root: &root,
            repos: &[],
            tickets: &[],
            port_base: 41000,
            port_count: 10,
            parent_slug: None,
            cli: Path::new("/opt/chauffeur"),
            explore: false,
        };

        std::os::unix::fs::symlink(home.join(".claude"), root.join(".claude")).unwrap();
        let error = write(&config).unwrap_err().to_string();
        assert!(
            error.contains(".claude") && error.contains("symlink"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(home.join(".claude/settings.json")).unwrap(),
            "{\"mine\":true}"
        );
        assert!(!root.join("CLAUDE.md").exists());

        std::fs::remove_file(root.join(".claude")).unwrap();
        std::os::unix::fs::symlink(home.join("notes.md"), root.join("CLAUDE.md")).unwrap();
        assert!(write(&config).is_err());
        assert_eq!(
            std::fs::read_to_string(home.join("notes.md")).unwrap(),
            "mine"
        );
        assert!(
            root.join("CLAUDE.md").symlink_metadata().is_ok(),
            "nothing removed"
        );

        std::fs::remove_file(root.join("CLAUDE.md")).unwrap();
        write(&config).unwrap();
        assert!(root.join(".claude/settings.json").is_file());
    }

    #[test]
    fn agents_are_told_to_start_dev_servers_through_the_run_scripts() {
        let root = tempfile::tempdir().unwrap();
        let config = WorkspaceConfig {
            slug: "ws",
            branch: "x/ws",
            root: root.path(),
            repos: &[],
            tickets: &[],
            port_base: 41000,
            port_count: 10,
            parent_slug: None,
            cli: Path::new("/opt/chauffeur"),
            explore: false,
        };
        write(&config).unwrap();

        for file in ["CLAUDE.md", "AGENTS.md"] {
            let text = std::fs::read_to_string(root.path().join(file)).unwrap();
            assert!(
                text.contains("chauffeur run \"$CHAUFFEUR_WORKSPACE_ID\" <repo> [script]"),
                "{file}"
            );
            assert!(
                text.contains("not a background command of your own"),
                "{file}"
            );
        }
    }

    #[test]
    fn agents_of_a_workspace_imported_from_conductor_are_pointed_at_its_chat_history() {
        let root = tempfile::tempdir().unwrap();
        let config = WorkspaceConfig {
            slug: "ws",
            branch: "x/ws",
            root: root.path(),
            repos: &[],
            tickets: &[],
            port_base: 41000,
            port_count: 10,
            parent_slug: None,
            cli: Path::new("/opt/chauffeur"),
            explore: false,
        };
        write(&config).unwrap();
        let text = std::fs::read_to_string(root.path().join("CLAUDE.md")).unwrap();
        assert!(!text.contains("imported from Conductor"));

        std::fs::create_dir_all(root.path().join(".chauffeur/conductor")).unwrap();
        write(&config).unwrap();
        for file in ["CLAUDE.md", "AGENTS.md"] {
            let text = std::fs::read_to_string(root.path().join(file)).unwrap();
            assert!(
                text.contains("Its earlier chats are in `.chauffeur/conductor/`"),
                "{file}"
            );
        }
    }

    #[test]
    fn quoting_survives_single_quotes() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
