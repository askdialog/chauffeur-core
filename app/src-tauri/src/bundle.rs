//! A packaged Chauffeur.app (scripts/package.sh) carries the daemon, the CLI and the agent sidecar
//! next to the app's own executable, so that colleagues need nothing but the disk image. This
//! module makes such a bundle usable: runnable from a terminal and reachable as `chauffeur`.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `Chauffeur.app/Contents/MacOS` when this app carries the daemon and the CLI, else `None`
/// (development, or an app installed by scripts/install.sh next to `~/.chauffeur/bin`).
pub fn bundled_bin_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let dir = exe.parent()?.to_path_buf();
    ((dir.ends_with("Contents/MacOS")
        || (cfg!(target_os = "linux") && dir.ends_with("lib/chauffeur")))
        && dir.join("chauffeurd").is_file()
        && dir.join("chauffeur").is_file())
    .then_some(dir)
}

/// An app copied out of a downloaded disk image is quarantined. The user approved it once to open
/// it, but the daemon, CLI and sidecar inside keep the flag, and macOS would block each of them
/// when started from a terminal. Lifts the flag from the whole bundle.
pub fn lift_quarantine(bin_dir: &Path) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let Some(bundle) = bin_dir.parent().and_then(Path::parent) else {
        return;
    };
    let _ = Command::new("/usr/bin/xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(bundle)
        .status();
}

/// Makes the bundled CLI reachable as `<home>/.local/bin/chauffeur`. Replaces a missing or dangling
/// link and a link into another Chauffeur.app (an older copy); leaves anything else alone, such as
/// a link to a build installed by scripts/install.sh.
pub fn link_cli(bin_dir: &Path, home: &Path) -> io::Result<()> {
    let target = bin_dir.join("chauffeur");
    let link = home.join(".local/bin/chauffeur");
    match std::fs::symlink_metadata(&link) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
        Ok(meta) if meta.file_type().is_symlink() => {
            let current = std::fs::read_link(&link)?;
            let dangling = std::fs::metadata(&link).is_err();
            let into_an_app = current
                .components()
                .any(|part| part.as_os_str().to_string_lossy().ends_with(".app"));
            if current == target || !(dangling || into_an_app) {
                return Ok(());
            }
            std::fs::remove_file(&link)?;
        }
        Ok(_) => return Ok(()),
    }
    std::fs::create_dir_all(link.parent().expect("a link path has a parent"))?;
    std::os::unix::fs::symlink(&target, &link)
}

/// Opens `bundle` again once this process has exited: after an update the app at that place is
/// the new one, which replaces the daemon of the old build. The waiting shell outlives the app.
pub fn reopen_after_exit(bundle: &Path) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    Command::new("/bin/sh")
        .args([
            "-c",
            r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open "$2""#,
            "sh",
            &std::process::id().to_string(),
        ])
        .arg(bundle)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
}

/// Whether a running daemon must make way for this app's: only a packaged app replaces it, and
/// only when the builds differ (the app was updated while the old daemon kept running).
pub fn must_replace_daemon(running: Option<&str>, ours: &str, bundled: bool) -> bool {
    bundled && running.is_some_and(|build| build != ours)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("chauffeur-bundle-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn fake_bundle(root: &Path, name: &str) -> PathBuf {
        let bin = root.join(name).join("Contents/MacOS");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("chauffeur"), "").unwrap();
        bin
    }

    #[test]
    fn links_the_cli_when_nothing_is_there() {
        let root = temp("fresh");
        let bin = fake_bundle(&root, "Chauffeur.app");
        link_cli(&bin, &root).unwrap();
        assert_eq!(
            std::fs::read_link(root.join(".local/bin/chauffeur")).unwrap(),
            bin.join("chauffeur")
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn replaces_a_link_into_another_app_or_a_dangling_one() {
        let root = temp("replace");
        let old = fake_bundle(&root, "Old/Chauffeur.app");
        let new = fake_bundle(&root, "Chauffeur.app");
        link_cli(&old, &root).unwrap();
        link_cli(&new, &root).unwrap();
        let link = root.join(".local/bin/chauffeur");
        assert_eq!(std::fs::read_link(&link).unwrap(), new.join("chauffeur"));

        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(root.join("gone/chauffeur"), &link).unwrap();
        link_cli(&new, &root).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), new.join("chauffeur"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn leaves_a_source_install_alone() {
        let root = temp("source");
        let bin = fake_bundle(&root, "Chauffeur.app");
        let installed = root.join(".chauffeur/bin");
        std::fs::create_dir_all(&installed).unwrap();
        std::fs::write(installed.join("chauffeur"), "").unwrap();
        std::fs::create_dir_all(root.join(".local/bin")).unwrap();
        let link = root.join(".local/bin/chauffeur");
        std::os::unix::fs::symlink(installed.join("chauffeur"), &link).unwrap();
        link_cli(&bin, &root).unwrap();
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            installed.join("chauffeur")
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn lifts_the_quarantine_flag_from_the_whole_bundle() {
        let root = temp("quarantine");
        let bin = fake_bundle(&root, "Chauffeur.app");
        let file = bin.join("chauffeur");
        let flag = |path: &Path| {
            Command::new("/usr/bin/xattr")
                .args(["-p", "com.apple.quarantine"])
                .arg(path)
                .output()
                .unwrap()
                .status
                .success()
        };
        Command::new("/usr/bin/xattr")
            .args(["-w", "com.apple.quarantine", "0081;00000000;Safari;"])
            .arg(&file)
            .status()
            .unwrap();
        assert!(flag(&file));
        lift_quarantine(&bin);
        assert!(!flag(&file));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn only_a_packaged_app_replaces_a_daemon_of_another_version() {
        assert!(must_replace_daemon(Some("0.1.0"), "0.2.0", true));
        assert!(!must_replace_daemon(Some("0.2.0"), "0.2.0", true));
        assert!(!must_replace_daemon(Some("0.1.0"), "0.2.0", false));
        assert!(!must_replace_daemon(None, "0.2.0", true));
    }
}
