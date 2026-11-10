//! The editors of this machine reach a GCP worker over SSH (Cursor or VS Code with Remote-SSH,
//! docs/cloud-workers.md): one `Host` entry per ready worker in `ssh_config` under the daemon's
//! state folder, a file only Chauffeur writes, which `~/.ssh/config` includes with one `Include`
//! line Chauffeur adds once. Nothing else in `~/.ssh` is written. An entry carries what
//! `gcloud compute ssh` itself uses for the worker (its `--dry-run`): address, user, key and the
//! known-hosts options, so the editor checks the host key gcloud already recorded. The daemon's
//! forward reaches the worker with the same entry, so that an expired gcloud session never cuts it.
//!
//! `~/.ssh/config` is the user's: it is replaced whole and atomically, only if it did not change
//! since it was read, with its permissions; a symbolic link, a read-only file or one of another
//! user is never changed, and the line to add by hand is named instead.
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

/// How `ssh` reaches a worker.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SshEntry {
    pub host_name: String,
    pub user: String,
    /// `IdentityFile`, `HostKeyAlias`, `UserKnownHostsFile`…, in the order gcloud gave them.
    pub options: Vec<(String, String)>,
}

/// The options of gcloud's ssh command an entry carries; any other (a `ProxyCommand` through IAP)
/// is refused rather than written half-understood.
const OPTIONS: &[&str] = &[
    "CheckHostIP",
    "HashKnownHosts",
    "HostKeyAlias",
    "IdentitiesOnly",
    "StrictHostKeyChecking",
    "UserKnownHostsFile",
];

/// Flags of gcloud's ssh command that take no argument and change nothing for an editor.
const PLAIN_FLAGS: &[&str] = &["-t", "-T", "-q", "-x", "-4", "-6"];

/// The entry `gcloud compute ssh <vm> --dry-run` describes: `ssh -t -i <key> -o K=V … user@host`.
/// The host key is always checked: gcloud asks not to before the key is known, which the forward
/// has recorded by then.
pub fn parse_dry_run(output: &str) -> anyhow::Result<SshEntry> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| line.contains("ssh"))
        .context("gcloud printed no ssh command")?;
    let words = shlex::split(line).context("gcloud printed an unreadable ssh command")?;
    let mut options = Vec::new();
    let mut destination = None;
    let mut words = words.into_iter().skip(1);
    while let Some(word) = words.next() {
        match word.as_str() {
            "-i" => options.push((
                "IdentityFile".into(),
                plain(words.next().context("-i without a key")?)?,
            )),
            "-o" => {
                let option = words.next().context("-o without an option")?;
                let (key, value) = option
                    .split_once('=')
                    .with_context(|| format!("unreadable ssh option {option}"))?;
                let Some(key) = OPTIONS.iter().find(|k| k.eq_ignore_ascii_case(key)) else {
                    bail!(
                        "gcloud reaches the worker with `-o {key}`, which an SSH entry of Chauffeur does not carry"
                    );
                };
                let value = if *key == "StrictHostKeyChecking" {
                    "yes".to_owned()
                } else {
                    plain(value.to_owned())?
                };
                options.push(((*key).to_owned(), value));
            }
            flag if PLAIN_FLAGS.contains(&flag) => {}
            flag if flag.starts_with('-') => {
                bail!(
                    "gcloud reaches the worker with `{flag}`, which an SSH entry of Chauffeur does not carry"
                )
            }
            target => {
                destination = Some(target.to_owned());
                break;
            }
        }
    }
    if !options
        .iter()
        .any(|(key, _)| key == "StrictHostKeyChecking")
    {
        options.push(("StrictHostKeyChecking".into(), "yes".into()));
    }
    let destination = destination.context("gcloud's ssh command names no host")?;
    let Some((user, host_name)) = destination.split_once('@') else {
        bail!("gcloud's ssh command names no user: {destination}");
    };
    Ok(SshEntry {
        host_name: plain(host_name.to_owned())?,
        user: plain(user.to_owned())?,
        options,
    })
}

/// The entry as `ssh` options, for a command that reaches the worker without the entries' file.
pub fn options(entry: &SshEntry) -> Vec<String> {
    entry
        .options
        .iter()
        .flat_map(|(key, value)| ["-o".to_owned(), format!("{key}={value}")])
        .collect()
}

/// A value ssh_config reads as written, once quoted when it has a space: no control character,
/// quote or backslash.
fn plain(value: String) -> anyhow::Result<String> {
    if value.is_empty()
        || value
            .chars()
            .any(|c| c.is_control() || c == '"' || c == '\\')
    {
        bail!("gcloud's ssh command has a value an ssh config cannot hold: {value:?}");
    }
    Ok(value)
}

/// The file the entries live in.
pub fn config_file(state: &Path) -> PathBuf {
    state.join("ssh_config")
}

/// The file's whole content, one `Host` block per worker, by its alias.
pub fn render(entries: &[(String, SshEntry)]) -> String {
    let mut text = String::from(
        "# Written by Chauffeur: one entry per GCP worker, removed with it (docs/cloud-workers.md).\n# Edits here are lost.\n",
    );
    for (host, entry) in entries {
        text.push_str(&format!(
            "\nHost {host}\n  HostName {}\n  User {}\n",
            entry.host_name, entry.user
        ));
        for (key, value) in &entry.options {
            let value = if value.contains(' ') {
                format!("\"{value}\"")
            } else {
                value.clone()
            };
            text.push_str(&format!("  {key} {value}\n"));
        }
    }
    text
}

/// Writes the file whole, readable by the user only.
pub fn write(file: &Path, entries: &[(String, SshEntry)]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let partial = file.with_extension("partial");
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&partial)
        .with_context(|| format!("writing {}", partial.display()))?;
    out.write_all(render(entries).as_bytes())?;
    out.sync_all()?;
    std::fs::rename(&partial, file).with_context(|| format!("writing {}", file.display()))?;
    Ok(())
}

/// The line that includes `file`, quoted as ssh_config reads it. A path ssh would misread (a
/// control character, a quote, a backslash, or a glob character Include expands) is refused.
pub fn include_line(file: &Path) -> anyhow::Result<String> {
    let path = file
        .to_str()
        .with_context(|| format!("{} is not UTF-8", file.display()))?;
    if path
        .chars()
        .any(|c| c.is_control() || matches!(c, '"' | '\\' | '*' | '?' | '[' | ']'))
    {
        bail!(
            "{path:?} cannot be named in an ssh config (it has a quote, a backslash, a control or a glob character); give Chauffeur's state folder a plainer path"
        );
    }
    Ok(format!("Include \"{path}\""))
}

/// Whether `<ssh_dir>/config`, or the file it links to, has a line including `file`.
pub fn is_included(ssh_dir: &Path, file: &Path) -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    std::fs::read_to_string(ssh_dir.join("config"))
        .is_ok_and(|config| includes(&config, file, home.as_deref()))
}

/// Adds `Include "<file>"` at the top of `<ssh_dir>/config` (where it applies to every host),
/// creating them owner-only if missing, unless a line includes it already. Returns whether it
/// added it.
pub fn ensure_included(ssh_dir: &Path, file: &Path) -> anyhow::Result<bool> {
    add_include(ssh_dir, file, &|| {}, &|out, bytes| {
        use std::io::Write;
        out.write_all(bytes)
    })
}

type Writer = dyn Fn(&mut std::fs::File, &[u8]) -> std::io::Result<()>;

/// A temporary file removed unless it was put in place.
struct Temporary(Option<PathBuf>);

impl Drop for Temporary {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// `ensure_included`, with what happens between reading the config and writing it, and how bytes
/// are written, given: tests stand for another editor and for a full disk there.
fn add_include(
    ssh_dir: &Path,
    file: &Path,
    between: &dyn Fn(),
    write: &Writer,
) -> anyhow::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let line = include_line(file)?;
    let config = ssh_dir.join("config");
    let by_hand = |why: String| {
        anyhow::anyhow!(
            "{} {why}; add this line at its top yourself: {line}",
            config.display()
        )
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    for _ in 0..3 {
        let meta = metadata(&config).map_err(|e| by_hand(format!("could not be read ({e})")))?;
        if let Some(meta) = &meta {
            if meta.file_type().is_symlink() {
                return Err(by_hand(
                    "is a symbolic link, which Chauffeur leaves alone".into(),
                ));
            }
            // SAFETY: getuid cannot fail.
            if meta.uid() != unsafe { libc::getuid() } || meta.mode() & 0o200 == 0 {
                return Err(by_hand(
                    "is read-only or not yours, which Chauffeur leaves alone".into(),
                ));
            }
        }
        let before = contents(&config).map_err(|e| by_hand(format!("could not be read ({e})")))?;
        let current = String::from_utf8(before.clone().unwrap_or_default())
            .map_err(|_| by_hand("is not UTF-8, which Chauffeur leaves alone".into()))?;
        if includes(&current, file, home.as_deref()) {
            return Ok(false);
        }
        between();
        let new = format!("{line}\n{current}");
        match replace(
            ssh_dir,
            &config,
            meta.as_ref(),
            &before,
            new.as_bytes(),
            write,
        ) {
            Ok(true) => return Ok(true),
            // Saved meanwhile by someone else: start again from what is there now.
            Ok(false) => continue,
            Err(error) => return Err(by_hand(format!("could not be replaced ({error})"))),
        }
    }
    Err(by_hand(
        "kept changing while Chauffeur added its line".into(),
    ))
}

/// Puts `new` in place of `config` through a unique file beside it, synced and renamed, with the
/// config's permissions; returns false, changing nothing, when the config is no longer the one
/// read (`meta`, `before`). Another program can still save it in the microseconds between that
/// check and the rename.
fn replace(
    ssh_dir: &Path,
    config: &Path,
    meta: Option<&std::fs::Metadata>,
    before: &Option<Vec<u8>>,
    new: &[u8],
    write: &Writer,
) -> std::io::Result<bool> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    if !ssh_dir.exists() {
        std::fs::create_dir_all(ssh_dir)?;
        std::fs::set_permissions(ssh_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let mode = meta.map_or(0o600, |meta| meta.mode() & 0o7777);
    let path = ssh_dir.join(format!(".config.chauffeur-{}", uuid::Uuid::now_v7()));
    let mut out = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)?;
    let mut temporary = Temporary(Some(path.clone()));
    out.set_permissions(std::fs::Permissions::from_mode(mode))?;
    write(&mut out, new)?;
    out.sync_all()?;
    drop(out);
    let same = |a: Option<&std::fs::Metadata>, b: Option<&std::fs::Metadata>| match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            (a.dev(), a.ino(), a.mtime(), a.mtime_nsec(), a.size())
                == (b.dev(), b.ino(), b.mtime(), b.mtime_nsec(), b.size())
        }
        _ => false,
    };
    if !same(meta, metadata(config)?.as_ref()) || contents(config)? != *before {
        return Ok(false);
    }
    std::fs::rename(&path, config)?;
    temporary.0 = None;
    if let Ok(dir) = std::fs::File::open(ssh_dir) {
        let _ = dir.sync_all();
    }
    Ok(true)
}

/// The config's own metadata, not its target's; none when it does not exist.
fn metadata(config: &Path) -> std::io::Result<Option<std::fs::Metadata>> {
    match std::fs::symlink_metadata(config) {
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The config's bytes; none when it does not exist.
fn contents(config: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(config) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Whether a line of an ssh config includes `file`, by its path or its `~/` form under `home`,
/// quoted or not.
fn includes(config: &str, file: &Path, home: Option<&Path>) -> bool {
    let absolute = file.display().to_string();
    let tilde = home
        .and_then(|home| file.strip_prefix(home).ok().map(Path::to_path_buf))
        .map(|rest| format!("~/{}", rest.display()));
    config.lines().any(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return false;
        }
        // `Include path` or `Include=path`.
        let line = match line.split_once('=') {
            Some((keyword, rest)) if !keyword.trim().contains(char::is_whitespace) => {
                format!("{} {rest}", keyword.trim())
            }
            _ => line.to_owned(),
        };
        let Some(words) = shlex::split(&line) else {
            return false;
        };
        let mut words = words.into_iter();
        words
            .next()
            .is_some_and(|keyword| keyword.eq_ignore_ascii_case("include"))
            && words.any(|path| path == absolute || Some(path.as_str()) == tilde.as_deref())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRY_RUN: &str = "/usr/bin/ssh -t -i /Users/me/.ssh/google_compute_engine -o CheckHostIP=no -o HashKnownHosts=no -o HostKeyAlias=compute.4242 -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=/Users/me/.ssh/google_compute_known_hosts me@34.155.1.2\n";

    #[test]
    fn a_worker_entry_carries_what_gcloud_ssh_uses() {
        let entry = parse_dry_run(DRY_RUN).unwrap();
        assert_eq!(entry.host_name, "34.155.1.2");
        assert_eq!(entry.user, "me");
        assert_eq!(
            entry.options[0],
            (
                "IdentityFile".into(),
                "/Users/me/.ssh/google_compute_engine".into()
            )
        );
        assert!(
            entry
                .options
                .contains(&("HostKeyAlias".into(), "compute.4242".into()))
        );
        let text = render(&[("chauffeur-worker-w1".into(), entry)]);
        assert!(text.contains("\nHost chauffeur-worker-w1\n  HostName 34.155.1.2\n  User me\n  IdentityFile /Users/me/.ssh/google_compute_engine\n"), "{text}");
        assert!(text.contains("  StrictHostKeyChecking yes\n"), "{text}");
        assert!(parse_dry_run("nothing here").is_err());
        assert!(parse_dry_run("/usr/bin/ssh -t 34.155.1.2").is_err());
    }

    #[test]
    fn an_entry_always_checks_the_host_key_and_never_misreads_gcloud() {
        // Before the worker's key is known, gcloud asks ssh not to check it: the entry still does.
        let first = parse_dry_run(
            "/usr/bin/ssh -t -i /k -o HostKeyAlias=compute.1 -o StrictHostKeyChecking=no me@10.0.0.1",
        )
        .unwrap();
        assert!(
            first
                .options
                .contains(&("StrictHostKeyChecking".into(), "yes".into())),
            "{first:?}"
        );
        // A flag or an option it does not know (a port, IAP's ProxyCommand) is refused, not
        // misread as the host or quoted into a broken command.
        for odd in [
            "/usr/bin/ssh -t -p 2222 -i /k me@10.0.0.1",
            "/usr/bin/ssh -t -i /k -o ProxyCommand=gcloud compute start-iap-tunnel me@10.0.0.1",
            "/usr/bin/ssh -t -i /k -o \"UserKnownHostsFile=/a\"b\" me@10.0.0.1",
            // What gcloud prints when it falls back to IAP: one quoted word to shlex, which the
            // parser before these checks carried into the entry as it was.
            "/usr/bin/ssh -t -i /k -o 'ProxyCommand=/usr/bin/python3 -S /google-cloud-sdk/lib/gcloud.py compute start-iap-tunnel chauffeur-worker-w1 %p --listen-on-stdin --project=p --zone=z' me@10.0.0.1",
            // A jump host, which that parser skipped, taking the jump host for the worker.
            "/usr/bin/ssh -t -i /k -J me@jump me@10.0.0.1",
        ] {
            assert!(parse_dry_run(odd).is_err(), "{odd} was accepted");
        }
    }

    #[test]
    fn the_include_counts_only_when_a_line_of_the_config_has_it() {
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let file = temp.path().join("state/ssh_config");
        assert!(!is_included(&ssh, &file));
        std::fs::write(
            ssh.join("config"),
            format!("include \"{}\"\n", file.display()),
        )
        .unwrap();
        assert!(is_included(&ssh, &file));
        std::fs::write(
            ssh.join("config"),
            format!("# Include {}\nHost x\n", file.display()),
        )
        .unwrap();
        assert!(!is_included(&ssh, &file), "a comment counted");
    }

    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_write_cut_short_leaves_the_users_config_as_it_was() {
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(ssh.join("config"), "Host mine\n  User me\n").unwrap();
        let file = temp.path().join("state/ssh_config");
        let cut = add_include(&ssh, &file, &|| {}, &|out, bytes| {
            use std::io::Write;
            out.write_all(&bytes[..5])?;
            Err(std::io::Error::other("disk full"))
        });
        assert!(cut.is_err());
        assert_eq!(
            std::fs::read_to_string(ssh.join("config")).unwrap(),
            "Host mine\n  User me\n"
        );
        let left: Vec<_> = std::fs::read_dir(&ssh)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, ["config"], "a temporary file was left behind");
    }

    #[test]
    fn an_edit_saved_between_reading_and_writing_the_config_is_kept() {
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(ssh.join("config"), "Host mine\n").unwrap();
        let file = temp.path().join("state/ssh_config");
        let edits = std::cell::Cell::new(0);
        let added = add_include(
            &ssh,
            &file,
            &|| {
                // The user saves the file once, between Chauffeur's read and its write.
                if edits.get() == 0 {
                    std::fs::write(ssh.join("config"), "Host mine\nHost theirs\n").unwrap();
                }
                edits.set(edits.get() + 1);
            },
            &|out, bytes| {
                use std::io::Write;
                out.write_all(bytes)
            },
        )
        .unwrap();
        assert!(added);
        assert_eq!(
            std::fs::read_to_string(ssh.join("config")).unwrap(),
            format!("Include \"{}\"\nHost mine\nHost theirs\n", file.display())
        );
    }

    #[test]
    fn a_config_that_is_a_symbolic_link_is_left_alone_and_the_line_to_add_is_named() {
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let dotfiles = temp.path().join("dotfiles-ssh-config");
        std::fs::write(&dotfiles, "Host mine\n").unwrap();
        std::os::unix::fs::symlink(&dotfiles, ssh.join("config")).unwrap();
        let file = temp.path().join("state/ssh_config");
        let refused = ensure_included(&ssh, &file).unwrap_err().to_string();
        assert!(refused.contains("symbolic link"), "{refused}");
        assert!(
            refused.contains(&format!("Include \"{}\"", file.display())),
            "{refused}"
        );
        assert_eq!(std::fs::read_to_string(&dotfiles).unwrap(), "Host mine\n");
        assert!(
            std::fs::symlink_metadata(ssh.join("config"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn an_unusual_state_path_is_quoted_and_one_ssh_cannot_read_is_refused_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(ssh.join("config"), "Host mine\n").unwrap();
        let spaced = temp.path().join("Application Support/chauffeur/ssh_config");
        assert!(ensure_included(&ssh, &spaced).unwrap());
        assert_eq!(
            std::fs::read_to_string(ssh.join("config")).unwrap(),
            format!("Include \"{}\"\nHost mine\n", spaced.display())
        );
        assert!(
            !ensure_included(&ssh, &spaced).unwrap(),
            "the quoted line was not recognised"
        );
        for bad in ["quo\"te", "new\nline", "back\\slash", "glob*"] {
            let file = temp.path().join(bad).join("ssh_config");
            let before = std::fs::read_to_string(ssh.join("config")).unwrap();
            assert!(ensure_included(&ssh, &file).is_err(), "{bad} was written");
            assert_eq!(std::fs::read_to_string(ssh.join("config")).unwrap(), before);
        }
    }

    #[test]
    fn a_read_only_config_is_left_alone_and_a_tilde_include_counts() {
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(ssh.join("config"), "Host mine\n").unwrap();
        std::fs::set_permissions(
            ssh.join("config"),
            std::os::unix::fs::PermissionsExt::from_mode(0o444),
        )
        .unwrap();
        let file = temp.path().join("state/ssh_config");
        let refused = ensure_included(&ssh, &file).unwrap_err().to_string();
        assert!(refused.contains("read-only"), "{refused}");
        assert_eq!(
            std::fs::read_to_string(ssh.join("config")).unwrap(),
            "Host mine\n"
        );
        assert_eq!(mode(&ssh.join("config")), 0o444);
        // Under the home folder, the `~/` form of the path counts as including it.
        let home = Path::new("/Users/someone");
        let under_home = home.join(".chauffeur/ssh_config");
        for line in [
            "Include ~/.chauffeur/ssh_config\n",
            "Include \"~/.chauffeur/ssh_config\"\n",
            "Include=~/.chauffeur/ssh_config\n",
        ] {
            assert!(includes(line, &under_home, Some(home)), "{line}");
        }
        assert!(!includes(
            "Include ~/.chauffeur/ssh_config\n",
            &under_home,
            None
        ));
    }

    #[test]
    fn adding_the_include_keeps_the_configs_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(ssh.join("config"), "Host mine\n").unwrap();
        std::fs::set_permissions(
            ssh.join("config"),
            std::os::unix::fs::PermissionsExt::from_mode(0o644),
        )
        .unwrap();
        assert!(ensure_included(&ssh, &temp.path().join("state/ssh_config")).unwrap());
        assert_eq!(mode(&ssh.join("config")), 0o644);
    }

    #[test]
    fn the_include_is_added_once_at_the_top_and_keeps_the_rest() {
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join(".ssh");
        let file = temp.path().join("state/ssh_config");
        assert!(ensure_included(&ssh, &file).unwrap());
        let mode = |path: &Path| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777
        };
        assert_eq!(mode(&ssh), 0o700);
        assert_eq!(mode(&ssh.join("config")), 0o600);
        std::fs::write(
            ssh.join("config"),
            format!("Include {}\nHost mine\n  User me\n", file.display()),
        )
        .unwrap();
        assert!(!ensure_included(&ssh, &file).unwrap());
        let other = temp.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("config"), "Host mine\n  User me\n").unwrap();
        assert!(ensure_included(&other, &file).unwrap());
        assert_eq!(
            std::fs::read_to_string(other.join("config")).unwrap(),
            format!("Include \"{}\"\nHost mine\n  User me\n", file.display())
        );
    }

    #[test]
    fn a_config_the_user_cannot_write_is_left_alone_even_if_others_can() {
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(ssh.join("config"), "Host mine\n").unwrap();
        // Readable by its owner, writable by its group only: not read-only to the standard
        // library, but not the user's to write.
        std::fs::set_permissions(
            ssh.join("config"),
            std::os::unix::fs::PermissionsExt::from_mode(0o460),
        )
        .unwrap();
        let file = temp.path().join("state/ssh_config");
        let refused = ensure_included(&ssh, &file).unwrap_err().to_string();
        assert!(refused.contains("read-only"), "{refused}");
        assert_eq!(mode(&ssh.join("config")), 0o460);
        std::fs::set_permissions(
            ssh.join("config"),
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(ssh.join("config")).unwrap(),
            "Host mine\n"
        );
    }

    #[test]
    fn a_config_that_cannot_be_replaced_names_the_line_to_add_by_hand() {
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(ssh.join("config"), "Host mine\n").unwrap();
        let file = temp.path().join("state/ssh_config");
        let line = format!("Include \"{}\"", file.display());
        // No file can be made beside it.
        std::fs::set_permissions(&ssh, std::os::unix::fs::PermissionsExt::from_mode(0o500))
            .unwrap();
        let refused = ensure_included(&ssh, &file).map_err(|e| e.to_string());
        std::fs::set_permissions(&ssh, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        let refused = refused.unwrap_err();
        assert!(refused.contains(&line), "{refused}");
        assert!(refused.contains("could not be replaced"), "{refused}");
        assert_eq!(
            std::fs::read_to_string(ssh.join("config")).unwrap(),
            "Host mine\n"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_immutable_config_names_the_line_to_add_by_hand() {
        use std::os::unix::ffi::OsStrExt;
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let config = ssh.join("config");
        std::fs::write(&config, "Host mine\n").unwrap();
        let file = temp.path().join("state/ssh_config");
        let path = std::ffi::CString::new(config.as_os_str().as_bytes()).unwrap();
        let flag = |flags| {
            // SAFETY: a valid C string.
            assert_eq!(unsafe { libc::chflags(path.as_ptr(), flags) }, 0);
        };
        flag(libc::UF_IMMUTABLE);
        let refused = ensure_included(&ssh, &file).map_err(|e| e.to_string());
        flag(0);
        let refused = refused.unwrap_err();
        assert!(
            refused.contains(&format!("Include \"{}\"", file.display())),
            "{refused}"
        );
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "Host mine\n");
        let left = std::fs::read_dir(&ssh).unwrap().count();
        assert_eq!(left, 1, "a temporary file was left behind");
    }

    #[test]
    fn a_config_replaced_by_a_link_of_the_same_content_meanwhile_is_left_alone() {
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let config = ssh.join("config");
        std::fs::write(&config, "Host mine\n").unwrap();
        let dotfiles = temp.path().join("dotfiles-ssh-config");
        std::fs::write(&dotfiles, "Host mine\n").unwrap();
        let file = temp.path().join("state/ssh_config");
        let refused = add_include(
            &ssh,
            &file,
            &|| {
                // A dotfiles tool links the config to its own copy, with the same content.
                if !std::fs::symlink_metadata(&config)
                    .unwrap()
                    .file_type()
                    .is_symlink()
                {
                    std::fs::remove_file(&config).unwrap();
                    std::os::unix::fs::symlink(&dotfiles, &config).unwrap();
                }
            },
            &|out, bytes| {
                use std::io::Write;
                out.write_all(bytes)
            },
        )
        .unwrap_err()
        .to_string();
        assert!(refused.contains("symbolic link"), "{refused}");
        assert!(
            std::fs::symlink_metadata(&config)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&dotfiles).unwrap(), "Host mine\n");
    }
}
