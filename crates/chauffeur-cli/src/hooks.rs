//! Commands Chauffeur's generated workspace settings call from Claude Code hooks.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `chauffeur hook-run --repo <path> --shell <command>`: runs one repo's hook command from the
/// workspace root, as if the session had started in the repo. Tool hooks about files outside the
/// repo are skipped, so web's eslint never runs on an api file.
pub fn hook_run(repo: &Path, command: &str) -> anyhow::Result<i32> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let payload: serde_json::Value = serde_json::from_str(&input).unwrap_or_default();
    let is_tool_event = payload.get("tool_name").is_some();
    let paths = touched_paths(&payload);
    let inside: Vec<&PathBuf> = paths.iter().filter(|p| p.starts_with(repo)).collect();
    if is_tool_event && !paths.is_empty() && inside.is_empty() {
        return Ok(0);
    }
    let mut child = Command::new("/bin/bash")
        .arg("-c")
        .arg(command)
        .current_dir(repo)
        .env("CLAUDE_PROJECT_DIR", repo)
        .envs(if inside.is_empty() {
            None
        } else {
            Some((
                "CLAUDE_FILE_PATHS",
                inside
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(" "),
            ))
        })
        .stdin(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(input.as_bytes());
    }
    Ok(child.wait()?.code().unwrap_or(1))
}

/// File paths a tool call touches, made absolute against the hook's cwd.
fn touched_paths(payload: &serde_json::Value) -> Vec<PathBuf> {
    let cwd = payload
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_default();
    let input = payload.get("tool_input");
    let mut paths = Vec::new();
    for key in ["file_path", "notebook_path", "path"] {
        if let Some(path) = input.and_then(|i| i.get(key)).and_then(|v| v.as_str()) {
            paths.push(absolute(&cwd, path));
        }
    }
    if let Some(edits) = input
        .and_then(|i| i.get("edits"))
        .and_then(|v| v.as_array())
    {
        for edit in edits {
            if let Some(path) = edit.get("file_path").and_then(|v| v.as_str()) {
                paths.push(absolute(&cwd, path));
            }
        }
    }
    paths
}

fn absolute(cwd: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// `chauffeur git-context`: the git state of every repo of the workspace in the current folder,
/// printed for Claude's SessionStart hook (its output joins the agent's context).
pub fn git_context() -> anyhow::Result<()> {
    let root = std::env::var_os("CLAUDE_PROJECT_DIR")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let mut repos: Vec<PathBuf> = std::fs::read_dir(&root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join(".git").exists())
        .collect();
    repos.sort();
    if repos.is_empty() {
        return Ok(());
    }
    println!("Git state of this workspace's repos (each folder is its own repository):");
    for repo in repos {
        let name = repo.file_name().unwrap().to_string_lossy().into_owned();
        let branch = git(&repo, &["branch", "--show-current"]);
        println!("\n## {name} (branch {})", branch.trim());
        let status = git(&repo, &["status", "--short"]);
        let lines: Vec<&str> = status.lines().collect();
        if lines.is_empty() {
            println!("clean");
        } else {
            for line in lines.iter().take(20) {
                println!("{line}");
            }
            if lines.len() > 20 {
                println!("… {} more changed files", lines.len() - 20);
            }
        }
        let log = git(&repo, &["log", "--oneline", "-5"]);
        if !log.trim().is_empty() {
            println!("recent commits:\n{}", log.trim_end());
        }
    }
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> String {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_read_from_the_tool_input() {
        let payload = serde_json::json!({
            "cwd": "/ws",
            "tool_name": "Edit",
            "tool_input": {"file_path": "web/src/a.ts"}
        });
        assert_eq!(
            touched_paths(&payload),
            vec![PathBuf::from("/ws/web/src/a.ts")]
        );
    }
}
