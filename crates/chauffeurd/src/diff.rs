//! What each worktree of a workspace changed against the merge-base with its base branch:
//! commits, staged and unstaged edits, and untracked files.

use std::collections::HashMap;
use std::path::Path;

use chauffeur_protocol::client::{FileDiff, FileStatus, RepoDiff};

use crate::git::Git;
use crate::store::WorktreeRow;

/// Patches beyond this size are cut (the file list stays complete).
const MAX_PATCH_BYTES: usize = 2 << 20;
/// Untracked files beyond this count are listed without their content.
const MAX_UNTRACKED_PATCHES: usize = 200;

pub async fn repo_diff(
    git: &Git,
    worktree: &WorktreeRow,
    stat_only: bool,
) -> anyhow::Result<RepoDiff> {
    let dir = Path::new(&worktree.path);
    let remote_base = format!("origin/{}", worktree.base_branch);
    let base = if git.rev_parse(dir, &remote_base).await.is_ok() {
        remote_base
    } else {
        worktree.base_branch.clone()
    };
    let merge_base = git.run(dir, &["merge-base", "HEAD", &base]).await.ok();
    let against = merge_base
        .clone()
        .or_else(|| worktree.base_commit.clone())
        .unwrap_or_else(|| "HEAD".to_string());
    let ahead = match &merge_base {
        Some(mb) => git
            .run(dir, &["rev-list", "--count", &format!("{mb}..HEAD")])
            .await
            .ok()
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0),
        None => 0,
    };

    let name_status = git
        .run(dir, &["diff", "--name-status", "-z", "-M", &against])
        .await?;
    let numstat = git
        .run(dir, &["diff", "--numstat", "-z", "-M", &against])
        .await?;
    let counts = parse_numstat(&numstat);
    let mut files: Vec<FileDiff> = parse_name_status(&name_status)
        .into_iter()
        .map(|(status, old_path, path)| {
            let (additions, deletions, binary) =
                counts.get(&path).copied().unwrap_or((0, 0, false));
            FileDiff {
                path,
                old_path,
                status,
                additions,
                deletions,
                binary,
            }
        })
        .collect();

    let untracked = git
        .run(dir, &["ls-files", "--others", "--exclude-standard", "-z"])
        .await?;
    let untracked: Vec<String> = untracked
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();
    for path in &untracked {
        let (additions, binary) = count_lines(&dir.join(path));
        files.push(FileDiff {
            path: path.clone(),
            old_path: None,
            status: FileStatus::Untracked,
            additions,
            deletions: 0,
            binary,
        });
    }

    let mut patch = String::new();
    let mut truncated = false;
    if !stat_only {
        patch = git.run(dir, &["diff", "-M", &against]).await?;
        for path in untracked.iter().take(MAX_UNTRACKED_PATCHES) {
            if patch.len() > MAX_PATCH_BYTES {
                break;
            }
            // `git diff --no-index` exits 1 when files differ, which they always do here.
            if let Ok(text) = git
                .run_allowing(dir, &["diff", "--no-index", "--", "/dev/null", path], &[1])
                .await
            {
                if !patch.is_empty() && !patch.ends_with('\n') {
                    patch.push('\n');
                }
                patch.push_str(&text);
            }
        }
        if patch.len() > MAX_PATCH_BYTES {
            let mut cut = MAX_PATCH_BYTES;
            while !patch.is_char_boundary(cut) {
                cut -= 1;
            }
            patch.truncate(cut);
            truncated = true;
        }
        truncated |= untracked.len() > MAX_UNTRACKED_PATCHES;
    }

    Ok(RepoDiff {
        repo: worktree.repo.clone(),
        worktree_id: worktree.id.clone(),
        path: worktree.path.clone(),
        branch: worktree.branch.clone(),
        base_branch: worktree.base_branch.clone(),
        merge_base,
        ahead,
        files,
        patch,
        truncated,
    })
}

/// `git diff --name-status -z -M`: `M\0path\0`, `R100\0old\0new\0`.
fn parse_name_status(out: &str) -> Vec<(FileStatus, Option<String>, String)> {
    let mut fields = out.split('\0').filter(|f| !f.is_empty());
    let mut entries = Vec::new();
    while let Some(code) = fields.next() {
        let status = match code.chars().next() {
            Some('A') => FileStatus::Added,
            Some('D') => FileStatus::Deleted,
            Some('R') | Some('C') => FileStatus::Renamed,
            _ => FileStatus::Modified,
        };
        if status == FileStatus::Renamed {
            let (Some(old), Some(new)) = (fields.next(), fields.next()) else {
                break;
            };
            entries.push((status, Some(old.to_string()), new.to_string()));
        } else if let Some(path) = fields.next() {
            entries.push((status, None, path.to_string()));
        }
    }
    entries
}

/// `git diff --numstat -z -M`: `add\tdel\tpath\0`, or `add\tdel\t\0old\0new\0` for renames;
/// binary files count `-`.
fn parse_numstat(out: &str) -> HashMap<String, (u32, u32, bool)> {
    let mut counts = HashMap::new();
    let mut fields = out.split('\0');
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        let mut parts = field.splitn(3, '\t');
        let (Some(add), Some(del), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let path = if path.is_empty() {
            let _old = fields.next();
            fields.next().unwrap_or_default().to_string()
        } else {
            path.to_string()
        };
        let binary = add == "-";
        counts.insert(
            path,
            (add.parse().unwrap_or(0), del.parse().unwrap_or(0), binary),
        );
    }
    counts
}

/// Lines of an untracked file, or "binary" for what is not worth reading: a huge file, and
/// anything but a regular file (a symlink to `/dev/zero` or a FIFO would never end).
fn count_lines(path: &Path) -> (u32, bool) {
    let Ok(meta) = path.symlink_metadata() else {
        return (0, false);
    };
    if !meta.file_type().is_file() || meta.len() > 8 << 20 {
        return (0, true);
    }
    match std::fs::read(path) {
        Ok(bytes) if bytes.contains(&0) => (0, true),
        Ok(bytes) => {
            let lines = bytes.iter().filter(|b| **b == b'\n').count();
            let trailing = usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
            ((lines + trailing) as u32, false)
        }
        Err(_) => (0, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untracked_files_are_counted_without_reading_what_is_not_a_regular_file() {
        let tmp = tempfile::tempdir().unwrap();
        let text = tmp.path().join("text.txt");
        std::fs::write(&text, "a\nb\nc").unwrap();
        assert_eq!(count_lines(&text), (3, false));
        assert_eq!(count_lines(&tmp.path().join("missing")), (0, false));

        let zero = tmp.path().join("zero");
        std::os::unix::fs::symlink("/dev/zero", &zero).unwrap();
        assert_eq!(count_lines(&zero), (0, true));

        let fifo = tmp.path().join("fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        assert_eq!(count_lines(&fifo), (0, true));

        let huge = tmp.path().join("huge");
        std::fs::File::create(&huge)
            .unwrap()
            .set_len((8 << 20) + 1)
            .unwrap();
        assert_eq!(count_lines(&huge), (0, true));
    }

    #[test]
    fn renames_and_binaries_are_parsed() {
        let status = parse_name_status("M\0a.txt\0R090\0old.txt\0new.txt\0A\0bin.png\0");
        assert_eq!(status.len(), 3);
        assert_eq!(
            status[1],
            (
                FileStatus::Renamed,
                Some("old.txt".into()),
                "new.txt".into()
            )
        );
        let counts = parse_numstat(concat!(
            "3\t1\ta.txt\0",
            "1\t0\t\0old.txt\0new.txt\0",
            "-\t-\tbin.png\0"
        ));
        assert_eq!(counts["a.txt"], (3, 1, false));
        assert_eq!(counts["new.txt"], (1, 0, false));
        assert_eq!(counts["bin.png"], (0, 0, true));
    }
}
