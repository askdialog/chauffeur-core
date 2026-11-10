//! The personal Linear API key: whether one is stored (for `chauffeur doctor`), and reading one
//! from the terminal without echoing it. The daemon saves it (`settings.set`): in the macOS
//! Keychain; on Linux in the Secret Service (GNOME Keyring, KWallet…), else in
//! `~/.config/chauffeur/linear-api-key`, readable by the user only. Never on a command line.

use std::process::{Command, Stdio};

/// The Keychain service, or the Secret Service attribute `service`.
const SERVICE: &str = "chauffeur-linear";

/// Whether a key is stored.
#[cfg(target_os = "macos")]
pub fn stored() -> bool {
    quiet(Command::new("security").args(["find-generic-password", "-s", SERVICE]))
}

/// Whether a key is stored.
#[cfg(not(target_os = "macos"))]
pub fn stored() -> bool {
    Command::new("secret-tool")
        .args(["lookup", "service", SERVICE])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|output| output.status.success() && !output.stdout.trim_ascii().is_empty())
        || key_file().is_some_and(|path| path.is_file())
}

/// Where the key goes on Linux when no Secret Service runs; the daemon reads it there.
#[cfg(not(target_os = "macos"))]
fn key_file() -> Option<std::path::PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".config"))
        })?;
    Some(config.join("chauffeur/linear-api-key"))
}

/// Reads a line from the terminal without echoing it (or from standard input when piped).
pub fn read_hidden(prompt: &str) -> anyhow::Result<String> {
    use std::io::{BufRead, Write};
    print!("{prompt}");
    std::io::stdout().flush()?;
    let fd = libc::STDIN_FILENO;
    let mut term = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr fills `term` when it returns 0; stdin may not be a terminal.
    let saved = (unsafe { libc::tcgetattr(fd, term.as_mut_ptr()) } == 0)
        .then(|| unsafe { term.assume_init() });
    if let Some(saved) = saved {
        let mut quiet = saved;
        quiet.c_lflag &= !libc::ECHO;
        // SAFETY: a termios read from this terminal, with echo off.
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) };
    }
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    if let Some(saved) = saved {
        // SAFETY: restores the settings read above.
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &saved) };
        println!();
    }
    read?;
    Ok(line.trim().to_string())
}

#[cfg(target_os = "macos")]
fn quiet(command: &mut Command) -> bool {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
