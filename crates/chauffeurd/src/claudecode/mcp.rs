//! The MCP servers Claude Code keeps for one folder only (local scope, in its state file's
//! `projects["<folder>"].mcpServers`): a workspace's agents run elsewhere and do not get those of
//! the user's clones. Each imported repo's are named in a warning, names only, never their
//! settings (they hold tokens), with the command that gives them to every folder.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::real;

/// A warning per clone in `clones` (the user's clones of each imported repo) with local-scope MCP
/// servers in `state`, Claude Code's state file.
pub(super) fn warnings(state: &Path, clones: &BTreeMap<String, Vec<PathBuf>>) -> Vec<String> {
    let Some(servers) = local_servers(state) else {
        return Vec::new();
    };
    let mut warnings = Vec::new();
    for (repo, paths) in clones {
        for clone in paths {
            let Some(names) = servers.get(&real(clone)) else {
                continue;
            };
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            warnings.push(format!(
                "{repo}: Claude Code's MCP servers {} are local to {}, which its workspaces' agents do not get: add each for every folder with `claude mcp add --scope user <name> …` (`claude mcp get <name>` in {1} shows its settings).",
                names.join(", "),
                clone.display(),
            ));
        }
    }
    warnings
}

/// The local-scope MCP servers' names by folder (resolved), from Claude Code's state file; none
/// when it cannot be read.
fn local_servers(state: &Path) -> Option<BTreeMap<PathBuf, BTreeSet<String>>> {
    let text = match std::fs::read_to_string(state) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(%error, state = %state.display(), "could not read Claude Code's state file");
            }
            return None;
        }
    };
    let json: Value = serde_json::from_str(&text)
        .inspect_err(|error| {
            tracing::warn!(%error, state = %state.display(), "could not read Claude Code's state file")
        })
        .ok()?;
    let mut servers = BTreeMap::new();
    for (folder, project) in json.get("projects")?.as_object()? {
        let names: BTreeSet<String> = project
            .get("mcpServers")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|s| s.keys().cloned())
            .collect();
        if !names.is_empty() {
            servers
                .entry(real(Path::new(folder)))
                .or_insert_with(BTreeSet::new)
                .extend(names);
        }
    }
    Some(servers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_servers_are_named_by_clone_without_their_settings() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let shop = root.join("shop");
        std::fs::create_dir_all(&shop).unwrap();
        // Claude Code names the folder as the session saw it: through a link here.
        let link = root.join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        let state = root.join(".claude.json");
        let json = serde_json::json!({
            "mcpServers": { "everywhere": { "command": "x" } },
            "projects": {
                link.join("shop").to_string_lossy(): {
                    "mcpServers": {
                        "linear": { "command": "npx", "env": { "TOKEN": "s3cret" } },
                        "sentry": { "url": "https://sentry.example" }
                    }
                },
                root.join("docs").to_string_lossy(): { "mcpServers": {} },
                root.join("other").to_string_lossy(): { "mcpServers": { "far": {} } }
            }
        });
        std::fs::write(&state, json.to_string()).unwrap();
        let clones = BTreeMap::from([
            ("shop".to_string(), vec![shop.clone()]),
            ("docs".to_string(), vec![root.join("docs")]),
        ]);
        let named = warnings(&state, &clones);
        assert_eq!(named.len(), 1, "{named:?}");
        let warning = &named[0];
        assert!(
            warning.starts_with(&format!(
                "shop: Claude Code's MCP servers linear, sentry are local to {}",
                shop.display()
            )),
            "{warning}"
        );
        assert!(
            warning.contains("claude mcp add --scope user <name>"),
            "{warning}"
        );
        for value in ["s3cret", "npx", "sentry.example", "everywhere", "far"] {
            assert!(!warning.contains(value), "{warning}");
        }
        // No state file, or one unreadable: nothing to say.
        assert!(warnings(&root.join("none.json"), &clones).is_empty());
        std::fs::write(&state, "{").unwrap();
        assert!(warnings(&state, &clones).is_empty());
    }
}
