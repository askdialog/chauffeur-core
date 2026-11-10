//! Thin async wrapper over the `git` CLI. The CLI, not a library, so that the user's credentials,
//! SSH config, hooks and repo features (sparse checkout, LFS) behave exactly as in a terminal.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, bail};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::shellenv::ShellEnv;

/// What a git command may take: one that stalls (a fetch whose connection the machine's sleep
/// cut) never ends by itself, and the per-clone lock it holds would keep every other workspace of
/// that repo waiting for good.
const GIT_TIMEOUT: Duration = Duration::from_secs(600);

/// A fetch moves little data: past this it is stalled, and the caller goes on with the local state.
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// Once git has exited, what is left in its pipes is read within this.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct Git {
    env: ShellEnv,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub prunable: bool,
}

impl Git {
    pub fn new(env: ShellEnv) -> Git {
        Git { env }
    }

    fn command(&self, dir: &Path) -> Command {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(dir)
            .env_clear()
            .envs(self.env.vars())
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }

    /// Runs git and returns trimmed stdout; fails with stderr on a non-zero exit.
    pub async fn run(&self, dir: &Path, args: &[&str]) -> anyhow::Result<String> {
        self.run_with(dir, args, &[]).await
    }

    pub async fn run_with(
        &self,
        dir: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> anyhow::Result<String> {
        self.run_within(dir, args, envs, GIT_TIMEOUT).await
    }

    async fn run_within(
        &self,
        dir: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
        limit: Duration,
    ) -> anyhow::Result<String> {
        let mut command = self.command(dir);
        command.args(args);
        for (key, value) in envs {
            command.env(key, value);
        }
        let output = output_within(command, args, limit).await?;
        if !output.status.success() {
            bail!(
                "git {} failed in {}: {}",
                args.join(" "),
                dir.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    }

    /// Binary-safe output for Git bundles, NUL-delimited paths and index patches.
    pub async fn run_bytes(&self, dir: &Path, args: &[&str]) -> anyhow::Result<Vec<u8>> {
        let output = output(self.command(dir).args(args)).await?;
        if !output.status.success() {
            bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output.stdout)
    }

    /// Like [`Git::run`], but also accepts the given non-zero exit codes.
    pub async fn run_allowing(
        &self,
        dir: &Path,
        args: &[&str],
        ok_codes: &[i32],
    ) -> anyhow::Result<String> {
        let mut command = self.command(dir);
        command.args(args);
        let output = output_within(command, args, GIT_TIMEOUT).await?;
        let code = output.status.code().unwrap_or(-1);
        if code != 0 && !ok_codes.contains(&code) {
            bail!(
                "git {} failed in {}: {}",
                args.join(" "),
                dir.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub async fn ok(&self, dir: &Path, args: &[&str]) -> bool {
        self.run(dir, args).await.is_ok()
    }

    /// Fetches `branch` from origin into `origin/<branch>`. Names come from clients: after
    /// `--end-of-options` git reads them as refs only, never as options (`--depth=1`,
    /// `--upload-pack=...`).
    pub async fn fetch(&self, clone: &Path, branch: &str) -> anyhow::Result<()> {
        self.run_within(
            clone,
            &["fetch", "--quiet", "--end-of-options", "origin", branch],
            &[],
            FETCH_TIMEOUT,
        )
        .await
        .map(|_| ())
    }

    pub async fn rev_parse(&self, dir: &Path, rev: &str) -> anyhow::Result<String> {
        self.run(
            dir,
            &["rev-parse", "--verify", "--quiet", "--end-of-options", rev],
        )
        .await
    }

    pub async fn branch_exists(&self, clone: &Path, branch: &str) -> bool {
        self.ok(
            clone,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .await
    }

    /// The remote's default branch (`origin/HEAD`), if known locally.
    pub async fn remote_default_branch(&self, clone: &Path) -> Option<String> {
        let head = self
            .run(
                clone,
                &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
            )
            .await
            .ok()?;
        head.strip_prefix("refs/remotes/origin/").map(String::from)
    }

    /// Creates a worktree at `path`: on a new `branch` cut from `start`, or on the existing
    /// `branch` when it already exists.
    pub async fn worktree_add(
        &self,
        clone: &Path,
        path: &Path,
        branch: &str,
        start: &str,
    ) -> anyhow::Result<()> {
        let path_str = path.to_string_lossy();
        if self.branch_exists(clone, branch).await {
            self.run(
                clone,
                &[
                    "worktree",
                    "add",
                    "--quiet",
                    "--end-of-options",
                    &path_str,
                    branch,
                ],
            )
            .await?;
        } else {
            self.run(
                clone,
                &[
                    "worktree",
                    "add",
                    "--quiet",
                    "--no-track",
                    "-b",
                    branch,
                    "--end-of-options",
                    &path_str,
                    start,
                ],
            )
            .await?;
        }
        Ok(())
    }

    /// A worktree with no branch, checked out at `start` (an exploration's).
    pub async fn worktree_add_detached(
        &self,
        clone: &Path,
        path: &Path,
        start: &str,
    ) -> anyhow::Result<()> {
        let path_str = path.to_string_lossy();
        self.run(
            clone,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                "--end-of-options",
                &path_str,
                start,
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn worktree_remove(&self, clone: &Path, path: &Path) -> anyhow::Result<()> {
        let path_str = path.to_string_lossy();
        let removed = self
            .run(
                clone,
                &["worktree", "remove", "--force", "--force", &path_str],
            )
            .await;
        if removed.is_err() && path.exists() {
            // A worktree git no longer recognises (moved, half-created): delete the folder.
            std::fs::remove_dir_all(path)
                .with_context(|| format!("removing {}", path.display()))?;
        }
        self.run(clone, &["worktree", "prune"]).await?;
        Ok(())
    }

    pub async fn worktree_list(&self, clone: &Path) -> anyhow::Result<Vec<WorktreeEntry>> {
        let out = self
            .run(clone, &["worktree", "list", "--porcelain"])
            .await?;
        let mut entries = Vec::new();
        for block in out.split("\n\n") {
            let mut entry = WorktreeEntry {
                path: PathBuf::new(),
                branch: None,
                prunable: false,
            };
            for line in block.lines() {
                if let Some(path) = line.strip_prefix("worktree ") {
                    entry.path = PathBuf::from(path);
                } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                    entry.branch = Some(branch.to_string());
                } else if line.starts_with("prunable") {
                    entry.prunable = true;
                }
            }
            if !entry.path.as_os_str().is_empty() {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Commits ahead of every remote branch: work that exists nowhere else.
    pub async fn unpushed_commits(&self, worktree: &Path) -> anyhow::Result<usize> {
        let out = self
            .run(
                worktree,
                &["rev-list", "--count", "HEAD", "--not", "--remotes"],
            )
            .await?;
        Ok(out.trim().parse().unwrap_or(0))
    }

    /// Records HEAD, the index and the working tree (untracked files included, ignored ones
    /// excluded) in a commit stored under `ref_name`. The worktree itself is not modified.
    pub async fn snapshot(&self, worktree: &Path, ref_name: &str) -> anyhow::Result<String> {
        let head = self.rev_parse(worktree, "HEAD").await?;
        let index_tree = self.run(worktree, &["write-tree"]).await?;
        let scratch = tempfile_path("chauffeur-index");
        let scratch_str = scratch.to_string_lossy().to_string();
        let git_dir = self.run(worktree, &["rev-parse", "--git-dir"]).await?;
        let index = absolutize(worktree, Path::new(&git_dir)).join("index");
        if index.exists() {
            std::fs::copy(&index, &scratch)?;
        }
        let envs = [("GIT_INDEX_FILE", scratch_str.as_str())];
        let result = async {
            self.run_with(worktree, &["add", "--all"], &envs).await?;
            let tree = self.run_with(worktree, &["write-tree"], &envs).await?;
            let message = format!("chauffeur snapshot\n\nhead {head}\nindex-tree {index_tree}\n");
            let commit = self
                .run(
                    worktree,
                    &["commit-tree", &tree, "-p", &head, "-m", &message],
                )
                .await?;
            self.run(worktree, &["update-ref", ref_name, &commit])
                .await?;
            anyhow::Ok(commit)
        }
        .await;
        let _ = std::fs::remove_file(&scratch);
        result
    }

    /// Recreates a snapshot's working tree and index in `worktree`, whose HEAD must be the
    /// snapshot's HEAD.
    pub async fn restore_snapshot(&self, worktree: &Path, snapshot: &str) -> anyhow::Result<()> {
        let message = self
            .run(worktree, &["log", "-1", "--format=%B", snapshot])
            .await?;
        let index_tree = message
            .lines()
            .find_map(|line| line.strip_prefix("index-tree "))
            .map(str::to_string);
        // Files of HEAD the snapshot no longer has were deleted in the working tree.
        let deleted = self
            .run(
                worktree,
                &["diff", "--name-only", "--diff-filter=D", "HEAD", snapshot],
            )
            .await?;
        self.run(worktree, &["checkout", snapshot, "--", "."])
            .await?;
        for file in deleted.lines().filter(|l| !l.is_empty()) {
            let _ = std::fs::remove_file(worktree.join(file));
        }
        match index_tree {
            Some(tree) => self.run(worktree, &["read-tree", &tree]).await?,
            None => self.run(worktree, &["reset", "--quiet"]).await?,
        };
        Ok(())
    }

    pub async fn delete_ref(&self, clone: &Path, ref_name: &str) -> anyhow::Result<()> {
        self.run(clone, &["update-ref", "-d", ref_name])
            .await
            .map(|_| ())
    }

    pub async fn delete_branch(&self, clone: &Path, branch: &str) -> anyhow::Result<()> {
        self.run(clone, &["branch", "-D", "--end-of-options", branch])
            .await
            .map(|_| ())
    }

    /// Gitignored entries of a checkout; fully ignored folders appear once, with a trailing `/`.
    pub async fn ignored_entries(&self, dir: &Path) -> anyhow::Result<Vec<String>> {
        let out = self
            .run(
                dir,
                &[
                    "ls-files",
                    "--others",
                    "--ignored",
                    "--exclude-standard",
                    "--directory",
                    "-z",
                ],
            )
            .await?;
        Ok(out
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .map(String::from)
            .collect())
    }

    /// What a checkout holds beyond its commit: changed, deleted, new and ignored files. A folder
    /// git does not track (or ignores) whole appears once, with a trailing `/`.
    pub async fn leftovers(&self, worktree: &Path) -> anyhow::Result<Vec<String>> {
        let out = self
            .run_allowing(
                worktree,
                &[
                    "status",
                    "--porcelain=v1",
                    "-z",
                    "--ignored",
                    "--untracked-files=normal",
                ],
                &[],
            )
            .await?;
        let mut files = Vec::new();
        let mut entries = out.split('\0').filter(|entry| !entry.is_empty());
        while let Some(entry) = entries.next() {
            let (status, path) = entry.split_at(entry.len().min(3));
            // A rename or copy is followed by the path it came from.
            if status.starts_with('R') || status.starts_with('C') {
                entries.next();
            }
            files.push(path.to_string());
        }
        Ok(files)
    }

    /// Puts a checkout back at its commit: changes undone, new and ignored files deleted (nested
    /// repos too). Only on the user's say-so: nothing it deletes can be recovered.
    pub async fn discard_leftovers(&self, worktree: &Path) -> anyhow::Result<()> {
        self.run(worktree, &["reset", "--hard", "-q", "HEAD"])
            .await?;
        self.run(worktree, &["clean", "-ffdxq"]).await?;
        Ok(())
    }

    /// Whether the checkout tracks a docker compose file, at any depth. Reads the index only
    /// (`git ls-files` with pathspecs), never the working tree: fast however big `node_modules`.
    pub async fn tracks_compose_file(&self, worktree: &Path) -> anyhow::Result<bool> {
        let mut args = vec!["ls-files", "-z", "--"];
        args.extend(COMPOSE_PATHSPECS);
        Ok(!self.run(worktree, &args).await?.is_empty())
    }
}

/// The file names `docker compose` reads by default, and their variants (`compose.prod.yml`,
/// `docker-compose.override.yaml`), anywhere in a repo.
const COMPOSE_PATHSPECS: [&str; 8] = [
    ":(glob)**/compose.yml",
    ":(glob)**/compose.yaml",
    ":(glob)**/compose.*.yml",
    ":(glob)**/compose.*.yaml",
    ":(glob)**/docker-compose.yml",
    ":(glob)**/docker-compose.yaml",
    ":(glob)**/docker-compose.*.yml",
    ":(glob)**/docker-compose.*.yaml",
];

/// Refuses a branch name git would refuse (`git check-ref-format --branch`), before it reaches any
/// git command: a client-chosen name is otherwise an argument to `fetch` in the user's clone.
pub fn validate_branch_name(branch: &str) -> crate::Result<()> {
    let bad =
        |why: &str| crate::Error::BadRequest(format!("branch name {branch:?} is not valid: {why}"));
    if branch.is_empty() {
        return Err(bad("it is empty"));
    }
    if branch.starts_with('-') {
        return Err(bad("it starts with -"));
    }
    if branch == "@" || branch == "HEAD" {
        return Err(bad("it is reserved"));
    }
    if branch.contains("..") || branch.contains("@{") || branch.contains("//") {
        return Err(bad("it contains .., @{ or //"));
    }
    if let Some(c) = branch
        .chars()
        .find(|c| c.is_control() || " ~^:?*[\\".contains(*c))
    {
        return Err(bad(&format!("it contains {c:?}")));
    }
    if branch.starts_with('/') || branch.ends_with('/') {
        return Err(bad("it starts or ends with /"));
    }
    for part in branch.split('/') {
        if part.starts_with('.') || part.ends_with('.') {
            return Err(bad("a component starts or ends with ."));
        }
        if part.ends_with(".lock") {
            return Err(bad("a component ends with .lock"));
        }
    }
    Ok(())
}

fn tempfile_path(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::now_v7()))
}

fn absolutize(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// Runs `command` to its end, or kills it (the command is `kill_on_drop`) once `limit` passes.
async fn output_within(
    mut command: Command,
    args: &[&str],
    limit: Duration,
) -> anyhow::Result<std::process::Output> {
    match tokio::time::timeout(limit, output(&mut command)).await {
        Ok(output) => output.with_context(|| format!("running git {}", args.join(" "))),
        Err(_) => bail!(
            "git {} did not finish within {} seconds and was stopped",
            args.join(" "),
            limit.as_secs()
        ),
    }
}

/// `Command::output`, over once git exits rather than once its pipes close. On macOS std creates a
/// pipe, then marks it close-on-exec: a process another thread spawns in between (the transfer
/// controller, a setup script) inherits its write end and keeps it open as long as it lives, so
/// reading to the end would wait for that process long after git is gone.
async fn output(command: &mut Command) -> std::io::Result<std::process::Output> {
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().expect("git's stdout is piped");
    let mut stderr = child.stderr.take().expect("git's stderr is piped");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let exited = {
        let read =
            async { tokio::try_join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err)) };
        tokio::pin!(read);
        let exited = tokio::select! {
            read = &mut read => {
                read?;
                None
            }
            status = child.wait() => Some(status?),
        };
        if exited.is_some() {
            // What git wrote is in the pipes already; past this, someone else holds them open.
            let _ = tokio::time::timeout(DRAIN_TIMEOUT, &mut read).await;
        }
        exited
    };
    let status = match exited {
        Some(status) => status,
        None => child.wait().await?,
    };
    Ok(std::process::Output {
        status,
        stdout: out,
        stderr: err,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_git_command_that_never_ends_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let git = Git::new(ShellEnv::new());
        let started = std::time::Instant::now();
        let error = git
            .run_within(
                dir.path(),
                &["-c", "alias.hang=!sleep 30", "hang"],
                &[],
                Duration::from_millis(300),
            )
            .await
            .expect_err("the command outlives its limit");
        assert!(error.to_string().contains("did not finish"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn a_git_command_ends_with_git_while_another_process_holds_its_output() {
        // The background sleep keeps git's stdout open after git exits, as a process spawned
        // meanwhile on another thread does when it inherits the pipe.
        let dir = tempfile::tempdir().unwrap();
        let git = Git::new(ShellEnv::new());
        let started = std::time::Instant::now();
        let out = git
            .run(
                dir.path(),
                &["-c", "alias.leak=!sleep 30 & echo done", "leak"],
            )
            .await
            .unwrap();
        assert_eq!(out, "done");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn branch_names_follow_check_ref_format() {
        for ok in [
            "main",
            "feature/ok-1.2",
            "ID-12/slug",
            "tester/xyz-1--sub-task",
            "a/-b",
            "a--b",
            "é",
        ] {
            assert!(validate_branch_name(ok).is_ok(), "{ok} should be accepted");
        }
        for bad in [
            "",
            "-x",
            "--depth=1",
            "--upload-pack=touch /tmp/pwned",
            "+refs/heads/x:refs/heads/staging",
            "a..b",
            "a@{b}",
            "@",
            "HEAD",
            "a/",
            "/a",
            "a.lock",
            "a.lock/b",
            "a//b",
            "a b",
            "a~",
            "a^",
            "a:",
            "a?",
            "a*",
            "a[",
            "a\\b",
            "a.",
            ".a",
            "a/.b",
            "a\u{7f}b",
            "a\nb",
        ] {
            let error = validate_branch_name(bad).expect_err(bad);
            assert!(matches!(error, crate::Error::BadRequest(_)), "{bad}");
        }
    }
}
