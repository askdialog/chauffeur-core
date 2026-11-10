//! Opening a workspace folder, or one of its worktrees, in an editor, a terminal or the file
//! manager: with `open -a` on macOS, by running the app's command on Linux. The last editor or
//! terminal used becomes the default. A workspace moved to a GCP worker opens there, in Cursor or
//! VS Code over SSH (Remote-SSH), through the worker's entry in Chauffeur's ssh config.

use std::path::{Path, PathBuf};

use chauffeur_protocol::client::{OpenParams, OpenResult};
use chauffeur_protocol::model::{OpenApp, OpenAppKind, WorkspaceState, WorktreeOrigin};

use crate::{Daemon, Error, Result};

const LAST_APP_KEY: &str = "open_app";

struct Known {
    id: &'static str,
    name: &'static str,
    kind: OpenAppKind,
    /// What to look for in the app folders: the bundle on macOS, the commands on Linux (the first
    /// found wins). None for the file manager, which is always there.
    names: &'static [&'static str],
}

const fn known(
    id: &'static str,
    name: &'static str,
    kind: OpenAppKind,
    names: &'static [&'static str],
) -> Known {
    Known {
        id,
        name,
        kind,
        names,
    }
}

/// In the order the menu lists them.
#[cfg(target_os = "macos")]
const KNOWN: &[Known] = &[
    known("cursor", "Cursor", OpenAppKind::Editor, &["Cursor.app"]),
    known(
        "vscode",
        "VS Code",
        OpenAppKind::Editor,
        &["Visual Studio Code.app"],
    ),
    known("zed", "Zed", OpenAppKind::Editor, &["Zed.app"]),
    known(
        "windsurf",
        "Windsurf",
        OpenAppKind::Editor,
        &["Windsurf.app"],
    ),
    known(
        "sublime",
        "Sublime Text",
        OpenAppKind::Editor,
        &["Sublime Text.app"],
    ),
    known("finder", "Finder", OpenAppKind::Files, &[]),
    known(
        "terminal",
        "Terminal",
        OpenAppKind::Terminal,
        &["Terminal.app"],
    ),
    known("iterm", "iTerm", OpenAppKind::Terminal, &["iTerm.app"]),
    known(
        "ghostty",
        "Ghostty",
        OpenAppKind::Terminal,
        &["Ghostty.app"],
    ),
    known("warp", "Warp", OpenAppKind::Terminal, &["Warp.app"]),
];

/// In the order the menu lists them.
#[cfg(not(target_os = "macos"))]
const KNOWN: &[Known] = &[
    known("cursor", "Cursor", OpenAppKind::Editor, &["cursor"]),
    known("vscode", "VS Code", OpenAppKind::Editor, &["code"]),
    // Arch packages Zed as `zeditor`; Zed's own installer names it `zed`.
    known("zed", "Zed", OpenAppKind::Editor, &["zeditor", "zed"]),
    known("windsurf", "Windsurf", OpenAppKind::Editor, &["windsurf"]),
    known("sublime", "Sublime Text", OpenAppKind::Editor, &["subl"]),
    known("files", "Files", OpenAppKind::Files, &[]),
    known("ghostty", "Ghostty", OpenAppKind::Terminal, &["ghostty"]),
    known("kitty", "kitty", OpenAppKind::Terminal, &["kitty"]),
    known(
        "alacritty",
        "Alacritty",
        OpenAppKind::Terminal,
        &["alacritty"],
    ),
    known("wezterm", "WezTerm", OpenAppKind::Terminal, &["wezterm"]),
    known("foot", "foot", OpenAppKind::Terminal, &["foot"]),
    known("konsole", "Konsole", OpenAppKind::Terminal, &["konsole"]),
    known(
        "gnome-terminal",
        "GNOME Terminal",
        OpenAppKind::Terminal,
        &["gnome-terminal"],
    ),
];

/// Tests and the e2e scripts look for apps in these folders (`:`-separated) instead, so that a
/// fake editor stands for the user's.
const APP_DIRS_OVERRIDE: &str = "CHAUFFEUR_APP_DIRS";

fn app_dirs_override() -> Option<Vec<PathBuf>> {
    std::env::var_os(APP_DIRS_OVERRIDE)
        .filter(|dirs| !dirs.is_empty())
        .map(|dirs| std::env::split_paths(&dirs).collect())
}

/// Where apps are installed.
#[cfg(target_os = "macos")]
fn app_dirs(_daemon: &Daemon) -> Vec<PathBuf> {
    if let Some(dirs) = app_dirs_override() {
        return dirs;
    }
    let mut dirs = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.insert(1, PathBuf::from(home).join("Applications"));
    }
    dirs
}

/// Where apps are installed: the user's PATH.
#[cfg(not(target_os = "macos"))]
fn app_dirs(daemon: &Daemon) -> Vec<PathBuf> {
    if let Some(dirs) = app_dirs_override() {
        return dirs;
    }
    daemon
        .shell_env
        .vars()
        .get("PATH")
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn is_app(path: &Path) -> bool {
    path.is_dir()
}

#[cfg(not(target_os = "macos"))]
fn is_app(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// The known apps found in `dirs`, with their path (none for the file manager).
fn installed(dirs: &[PathBuf]) -> Vec<(&'static Known, Option<PathBuf>)> {
    KNOWN
        .iter()
        .filter_map(|known| {
            if known.names.is_empty() {
                return Some((known, None));
            }
            known
                .names
                .iter()
                .flat_map(|name| dirs.iter().map(move |dir| dir.join(name)))
                .find(|path| is_app(path))
                .map(|path| (known, Some(path)))
        })
        .collect()
}

fn default_id(found: &[(&'static Known, Option<PathBuf>)], last: Option<&str>) -> &'static str {
    if let Some(last) = last
        && let Some((known, _)) = found.iter().find(|(k, _)| k.id == last)
    {
        return known.id;
    }
    found
        .iter()
        .find(|(k, _)| k.kind == OpenAppKind::Editor)
        .or_else(|| found.iter().find(|(k, _)| k.kind == OpenAppKind::Files))
        .map(|(k, _)| k.id)
        .unwrap_or("finder")
}

fn describe(known: &Known, default: &str) -> OpenApp {
    OpenApp {
        id: known.id.into(),
        name: known.name.into(),
        kind: known.kind,
        is_default: known.id == default,
    }
}

pub fn apps(daemon: &Daemon) -> Result<Vec<OpenApp>> {
    let found = installed(&app_dirs(daemon));
    let last = daemon.store.kv_get(LAST_APP_KEY)?;
    let default = default_id(&found, last.as_deref());
    Ok(found.iter().map(|(k, _)| describe(k, default)).collect())
}

/// The program and arguments that open `folder` in an app found at `path`.
#[cfg(target_os = "macos")]
fn command(known: &Known, path: Option<&Path>, folder: &Path) -> (String, Vec<String>) {
    let folder = folder.to_string_lossy().into_owned();
    let args = match (known.id, path) {
        (_, None) => vec![folder],
        // Ghostty takes the folder as an option, not as a document to open.
        ("ghostty", Some(bundle)) => vec![
            "-na".into(),
            bundle.to_string_lossy().into_owned(),
            "--args".into(),
            format!("--working-directory={folder}"),
        ],
        (_, Some(bundle)) => vec!["-a".into(), bundle.to_string_lossy().into_owned(), folder],
    };
    ("/usr/bin/open".into(), args)
}

/// The program and arguments that open `folder` in an app found at `path`.
#[cfg(not(target_os = "macos"))]
fn command(known: &Known, path: Option<&Path>, folder: &Path) -> (String, Vec<String>) {
    let folder = folder.to_string_lossy().into_owned();
    let Some(path) = path else {
        return ("xdg-open".into(), vec![folder]);
    };
    let args = match known.id {
        "ghostty" | "foot" | "gnome-terminal" => vec![format!("--working-directory={folder}")],
        "alacritty" => vec!["--working-directory".into(), folder],
        "kitty" => vec!["--directory".into(), folder],
        "wezterm" => vec!["start".into(), "--cwd".into(), folder],
        "konsole" => vec!["--workdir".into(), folder],
        _ => vec![folder],
    };
    (path.to_string_lossy().into_owned(), args)
}

/// Runs the opening command and waits for `open`, which returns at once.
#[cfg(target_os = "macos")]
async fn launch(_daemon: &Daemon, program: &str, args: &[String]) -> Result<()> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await?;
    if !output.status.success() {
        return Err(Error::Unavailable(format!(
            "open failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Starts the app detached, in the user's environment and graphical session: a terminal runs
/// until it is closed, and must outlive the daemon.
#[cfg(not(target_os = "macos"))]
async fn launch(daemon: &Daemon, program: &str, args: &[String]) -> Result<()> {
    let mut env = daemon.shell_env.vars();
    for (key, value) in desktop_session(&env) {
        env.entry(key).or_insert(value);
    }
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .env_clear()
        .envs(env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| Error::Unavailable(format!("could not start {program}: {e}")))?;
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
}

/// The graphical session's variables `env` lacks. A daemon started by systemd at login may not
/// have them, unless the compositor exported them: they are then guessed from the sockets of the
/// session's Wayland compositor and X server.
#[cfg(not(target_os = "macos"))]
pub(crate) fn desktop_session(
    env: &std::collections::BTreeMap<String, String>,
) -> Vec<(String, String)> {
    const SESSION: &[&str] = &[
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "XAUTHORITY",
        "XDG_RUNTIME_DIR",
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
        "DBUS_SESSION_BUS_ADDRESS",
    ];
    let mut vars: Vec<(String, String)> = SESSION
        .iter()
        .filter(|key| !env.contains_key(**key))
        .filter_map(|key| Some((key.to_string(), std::env::var(key).ok()?)))
        .collect();
    let has = |key: &str, vars: &[(String, String)]| {
        env.contains_key(key) || vars.iter().any(|(k, _)| k == key)
    };
    let runtime = env
        .get("XDG_RUNTIME_DIR")
        .cloned()
        .or_else(|| std::env::var("XDG_RUNTIME_DIR").ok())
        // SAFETY: getuid cannot fail.
        .unwrap_or_else(|| format!("/run/user/{}", unsafe { libc::getuid() }));
    if !has("XDG_RUNTIME_DIR", &vars) {
        vars.push(("XDG_RUNTIME_DIR".into(), runtime.clone()));
    }
    if !has("WAYLAND_DISPLAY", &vars)
        && let Some(socket) = lowest_socket(Path::new(&runtime), "wayland-")
    {
        vars.push(("WAYLAND_DISPLAY".into(), socket));
    }
    if !has("DISPLAY", &vars)
        && let Some(number) = lowest_socket(Path::new("/tmp/.X11-unix"), "X")
    {
        vars.push(("DISPLAY".into(), format!(":{}", &number[1..])));
    }
    vars
}

/// The name of the lowest-numbered `<prefix><n>` entry of `dir` (`wayland-0`, `X1`…), skipping
/// lock files.
#[cfg(not(target_os = "macos"))]
fn lowest_socket(dir: &Path, prefix: &str) -> Option<String> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let number: u32 = name.strip_prefix(prefix)?.parse().ok()?;
            Some((number, name))
        })
        .min()
        .map(|(_, name)| name)
}

pub async fn open(daemon: &Daemon, params: &OpenParams) -> Result<OpenResult> {
    let workspace = daemon.workspaces.get(&params.workspace)?;
    if matches!(
        workspace.state,
        WorkspaceState::Archived | WorkspaceState::Archiving
    ) {
        return Err(Error::Conflict(format!("{} is archived", workspace.slug)));
    }
    if let Some(machine) = crate::cloud_transfer_driver::moved_to(daemon, &workspace.id) {
        return open_remote(daemon, &workspace, &machine, params).await;
    }
    let design = match &params.design_chat {
        Some(_) if params.repo.is_some() => {
            return Err(Error::BadRequest(
                "open either a repo or a design chat's folder".into(),
            ));
        }
        Some(chat) => {
            let chat = daemon.chats.resolve(chat)?;
            if chat.kind != chauffeur_protocol::model::ChatKind::Design
                || chat.workspace_id != workspace.id
            {
                return Err(Error::BadRequest(format!(
                    "{} is not a design chat of {}",
                    chat.id, workspace.slug
                )));
            }
            Some(
                crate::design::design_dir(&workspace.path, &chat.id).map_err(|error| {
                    Error::Internal(anyhow::anyhow!("the design folder: {error}"))
                })?,
            )
        }
        None => None,
    };
    let shell_folder = match &params.terminal {
        Some(_) if params.repo.is_some() || design.is_some() => {
            return Err(Error::BadRequest(
                "open either a repo, a design chat's folder or a terminal's folder".into(),
            ));
        }
        Some(terminal) => {
            if !daemon
                .terminals
                .list(&workspace.id)
                .iter()
                .any(|t| t.id == *terminal)
            {
                return Err(Error::not_found(format!(
                    "terminal {terminal} in {}",
                    workspace.slug
                )));
            }
            Some(daemon.terminals.cwd(terminal)?)
        }
        None => None,
    };
    let folder = match (&params.repo, shell_folder) {
        _ if design.is_some() => PathBuf::from(design.clone().unwrap_or_default()),
        (_, Some(folder)) => folder,
        (None, None) => PathBuf::from(&workspace.path),
        (Some(repo), None) => workspace
            .worktrees
            .iter()
            .filter(|w| w.repo == *repo && !w.removed)
            .min_by_key(|w| w.origin != WorktreeOrigin::Workspace)
            .map(|w| PathBuf::from(&w.path))
            .ok_or_else(|| Error::not_found(format!("{repo} in {}", workspace.slug)))?,
    };
    let found = installed(&app_dirs(daemon));
    let last = daemon.store.kv_get(LAST_APP_KEY)?;
    let id = params.app.clone().unwrap_or_else(|| {
        let id = default_id(&found, last.as_deref());
        // A design folder holds screens: the default terminal gives way to an editor.
        match found.iter().find(|(k, _)| k.id == id) {
            Some((known, _)) if design.is_some() && known.kind == OpenAppKind::Terminal => {
                default_id(&found, None).to_string()
            }
            _ => id.to_string(),
        }
    });
    let Some((known, path)) = found.iter().find(|(k, _)| k.id == id) else {
        let ids: Vec<&str> = found.iter().map(|(k, _)| k.id).collect();
        return Err(Error::BadRequest(format!(
            "{id} is not installed; apps: {}",
            ids.join(", ")
        )));
    };
    if design.is_some() && known.kind == OpenAppKind::Terminal {
        return Err(Error::BadRequest(
            "a design chat's folder opens in an editor or the file manager, not a terminal".into(),
        ));
    }
    let (program, args) = command(known, path.as_deref(), &folder);
    launch(daemon, &program, &args).await?;
    // Revealing a folder in the file manager is a detour: the default stays the editor or terminal.
    let default = if known.kind == OpenAppKind::Files {
        default_id(&found, last.as_deref())
    } else {
        daemon.store.kv_set(LAST_APP_KEY, known.id)?;
        known.id
    };
    Ok(OpenResult {
        app: describe(known, default),
        path: folder.to_string_lossy().into_owned(),
    })
}

/// The editors that open a folder on another machine over SSH.
const REMOTE_EDITORS: &[&str] = &["cursor", "vscode"];

/// Opens a workspace moved to a GCP worker where it runs: `cursor` or `code --remote
/// ssh-remote+<host> <folder>`. A workspace moved to a machine declared by hand is refused, and so
/// is one on a worker the editors cannot reach yet, saying what to do.
async fn open_remote(
    daemon: &Daemon,
    workspace: &chauffeur_protocol::model::WorkspaceInfo,
    machine: &str,
    params: &OpenParams,
) -> Result<OpenResult> {
    let host = match crate::workers::remote_host(daemon, machine) {
        None => {
            return Err(Error::Conflict(format!(
                "{} runs on {machine}, a machine declared by hand: only a GCP worker's workspaces open in this machine's editors; open it on {machine}",
                workspace.slug
            )));
        }
        Some(crate::workers::RemoteHost::Unavailable(what)) => {
            return Err(Error::Conflict(format!(
                "{} does not open on {machine} yet: {what}",
                workspace.slug
            )));
        }
        Some(crate::workers::RemoteHost::Host(host)) => host,
    };
    let root = crate::cloud_transfer_driver::moved_path(daemon, &workspace.id);
    let found = installed(&app_dirs(daemon));
    let last = daemon.store.kv_get(LAST_APP_KEY)?;
    let (known, path, folder) = plan_remote(
        &RemoteAsk {
            slug: &workspace.slug,
            machine,
            path: &workspace.path,
            worktrees: &workspace.worktrees,
            root: root.as_deref(),
            params,
            last: last.as_deref(),
        },
        &found,
    )?;
    let (program, args) = remote_command(known, path, &host, &folder);
    launch_cli(daemon, &program, &args).await?;
    daemon.store.kv_set(LAST_APP_KEY, known.id)?;
    Ok(OpenResult {
        app: describe(known, known.id),
        path: folder.to_string_lossy().into_owned(),
    })
}

/// A workspace on a worker to open, and what is known of it here.
struct RemoteAsk<'a> {
    slug: &'a str,
    machine: &'a str,
    /// Its folder here, its worktrees' paths below it.
    path: &'a str,
    worktrees: &'a [chauffeur_protocol::model::WorktreeInfo],
    /// Its folder there, as the transfer that moved it reported.
    root: Option<&'a str>,
    params: &'a OpenParams,
    last: Option<&'a str>,
}

/// The editor a workspace on a worker opens with, found at which path, and its folder there.
fn plan_remote<'f>(
    ask: &RemoteAsk,
    found: &'f [(&'static Known, Option<PathBuf>)],
) -> Result<(&'static Known, &'f Path, PathBuf)> {
    let (slug, machine) = (ask.slug, ask.machine);
    if ask.params.design_chat.is_some() {
        return Err(Error::BadRequest(format!(
            "{slug} runs on {machine}: its design chats open in the app; open its folder or a repo in an editor"
        )));
    }
    if ask.params.terminal.is_some() {
        return Err(Error::BadRequest(format!(
            "{slug} runs on {machine}: its terminals' folders are there; open its folder or a repo in an editor"
        )));
    }
    let root = PathBuf::from(ask.root.ok_or_else(|| {
        Error::Conflict(format!(
            "Where {slug} is on {machine} is not known here: bring it back and transfer it again to open it in an editor"
        ))
    })?);
    let folder = match &ask.params.repo {
        None => root,
        Some(repo) => {
            let worktree = ask
                .worktrees
                .iter()
                .filter(|w| w.repo == *repo && !w.removed)
                .min_by_key(|w| w.origin != WorktreeOrigin::Workspace)
                .ok_or_else(|| Error::not_found(format!("{repo} in {slug}")))?;
            let inside = Path::new(&worktree.path)
                .strip_prefix(ask.path)
                .map_err(|_| {
                    Error::BadRequest(format!(
                        "{repo}'s worktree is outside {slug}'s folder; open the workspace instead"
                    ))
                })?;
            root.join(inside)
        }
    };
    let editors: Vec<(&'static Known, &'f Path)> = found
        .iter()
        .filter(|(k, _)| REMOTE_EDITORS.contains(&k.id))
        .filter_map(|(k, path)| Some((*k, path.as_deref()?)))
        .collect();
    let id = match ask.params.app.as_deref() {
        Some(id) => id,
        None => ask
            .last
            .filter(|last| editors.iter().any(|(k, _)| k.id == *last))
            .or_else(|| editors.first().map(|(k, _)| k.id))
            .ok_or_else(|| {
                Error::Unavailable(format!(
                    "Install Cursor or VS Code (with Remote-SSH) to open {slug} on {machine}"
                ))
            })?,
    };
    if let Some(&(known, path)) = editors.iter().find(|(k, _)| k.id == id) {
        return Ok((known, path, folder));
    }
    match KNOWN.iter().find(|k| k.id == id) {
        Some(known) if REMOTE_EDITORS.contains(&id) => Err(Error::Unavailable(format!(
            "{} is not installed here: install it with Remote-SSH, or open {slug} with {}",
            known.name,
            if id == "cursor" { "VS Code" } else { "Cursor" }
        ))),
        _ => Err(Error::BadRequest(format!(
            "Only Cursor and VS Code open {slug} on {machine} (Remote-SSH)"
        ))),
    }
}

/// The editor's own command line, inside its bundle on macOS, and its arguments.
fn remote_command(known: &Known, path: &Path, host: &str, folder: &Path) -> (String, Vec<String>) {
    let cli = if cfg!(target_os = "macos") {
        let name = if known.id == "vscode" {
            "code"
        } else {
            known.id
        };
        path.join("Contents/Resources/app/bin").join(name)
    } else {
        path.to_path_buf()
    };
    (
        cli.to_string_lossy().into_owned(),
        vec![
            "--remote".into(),
            format!("ssh-remote+{host}"),
            folder.to_string_lossy().into_owned(),
        ],
    )
}

/// Starts an editor's command line detached, in the user's environment: it hands the folder to
/// the editor and may stay as long as the editor does.
#[cfg(target_os = "macos")]
async fn launch_cli(daemon: &Daemon, program: &str, args: &[String]) -> Result<()> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .env_clear()
        .envs(daemon.shell_env.vars())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| Error::Unavailable(format!("could not start {program}: {e}")))?;
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn launch_cli(daemon: &Daemon, program: &str, args: &[String]) -> Result<()> {
    launch(daemon, program, args).await
}

/// Opens a file in the editor last used (the first one installed otherwise), else in the
/// system's text editor.
pub async fn open_file(daemon: &Daemon, file: &Path) -> Result<()> {
    let found = installed(&app_dirs(daemon));
    let last = daemon.store.kv_get(LAST_APP_KEY)?;
    let editor = found
        .iter()
        .find(|(k, _)| Some(k.id) == last.as_deref() && k.kind == OpenAppKind::Editor)
        .or_else(|| found.iter().find(|(k, _)| k.kind == OpenAppKind::Editor));
    let (program, args) = match editor {
        Some((known, path)) => command(known, path.as_deref(), file),
        #[cfg(target_os = "macos")]
        None => (
            "/usr/bin/open".into(),
            vec!["-t".into(), file.to_string_lossy().into_owned()],
        ),
        #[cfg(not(target_os = "macos"))]
        None => ("xdg-open".into(), vec![file.to_string_lossy().into_owned()]),
    };
    launch(daemon, &program, &args).await
}

/// Opens a file in the system's default app for its type (`files.open` checks the type first).
pub async fn open_default(daemon: &Daemon, file: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let program = "/usr/bin/open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    launch(daemon, program, &[file.to_string_lossy().into_owned()]).await
}

/// Shows a file selected in Finder (`open -R`, which runs nothing whatever its type); on Linux,
/// opens the folder holding it, file managers having no common way to select a file. A folder
/// shows its contents: Finder opens the folder holding its first visible entry (a plain `open`
/// would launch a folder that is an app bundle).
pub async fn reveal(daemon: &Daemon, file: &Path) -> Result<()> {
    let (program, args) = reveal_command(&reveal_target(file));
    launch(daemon, program, &args).await
}

fn reveal_target(path: &Path) -> PathBuf {
    if !path.is_dir() {
        return path.to_path_buf();
    }
    let first = std::fs::read_dir(path).ok().and_then(|entries| {
        entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
            .map(|entry| entry.path())
            .min()
    });
    first.unwrap_or_else(|| path.to_path_buf())
}

fn reveal_command(file: &Path) -> (&'static str, Vec<String>) {
    #[cfg(target_os = "macos")]
    return (
        "/usr/bin/open",
        vec!["-R".into(), file.to_string_lossy().into_owned()],
    );
    #[cfg(not(target_os = "macos"))]
    return (
        "xdg-open",
        vec![file.parent().unwrap_or(file).to_string_lossy().into_owned()],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktrees() -> Vec<chauffeur_protocol::model::WorktreeInfo> {
        [
            ("alpha", "/Users/me/chauffeur/workspaces/moving/alpha"),
            ("beta", "/Users/me/clones/beta"),
        ]
        .iter()
        .map(|(repo, path)| chauffeur_protocol::model::WorktreeInfo {
            id: format!("wt-{repo}"),
            repo: (*repo).into(),
            path: (*path).into(),
            branch: "jc/moving".into(),
            base_commit: None,
            base_branch: "main".into(),
            origin: WorktreeOrigin::Workspace,
            group: None,
            removed: false,
        })
        .collect()
    }

    fn params(repo: Option<&str>, app: Option<&str>, design: Option<&str>) -> OpenParams {
        OpenParams {
            workspace: "moving".into(),
            repo: repo.map(Into::into),
            app: app.map(Into::into),
            design_chat: design.map(Into::into),
            terminal: None,
        }
    }

    #[test]
    fn a_workspace_on_a_worker_opens_its_folder_or_a_repo_there_and_refuses_what_it_cannot() {
        let cursor = KNOWN.iter().find(|k| k.id == "cursor").unwrap();
        let finder = KNOWN.iter().find(|k| k.kind == OpenAppKind::Files).unwrap();
        let found = vec![
            (cursor, Some(PathBuf::from("/Applications/Cursor.app"))),
            (finder, None),
        ];
        let worktrees = worktrees();
        let there = Some("/home/me/chauffeur/workspaces/moving");
        let plan =
            |p: &OpenParams, root: Option<&str>, found: &[(&'static Known, Option<PathBuf>)]| {
                plan_remote(
                    &RemoteAsk {
                        slug: "moving",
                        machine: "w1",
                        path: "/Users/me/chauffeur/workspaces/moving",
                        worktrees: &worktrees,
                        root,
                        params: p,
                        last: None,
                    },
                    found,
                )
                .map(|(known, _, folder)| (known.id, folder))
                .map_err(|error| error.to_string())
            };
        assert_eq!(
            plan(&params(None, None, None), there, &found),
            Ok((
                "cursor",
                PathBuf::from("/home/me/chauffeur/workspaces/moving")
            ))
        );
        assert_eq!(
            plan(&params(Some("alpha"), None, None), there, &found),
            Ok((
                "cursor",
                PathBuf::from("/home/me/chauffeur/workspaces/moving/alpha")
            ))
        );
        let refused = |p: OpenParams,
                       root: Option<&str>,
                       found: &[(&'static Known, Option<PathBuf>)],
                       says: &str| {
            let error = plan(&p, root, found).unwrap_err();
            assert!(error.contains(says), "{p:?}: {error}");
        };
        refused(
            params(None, None, Some("chat-1")),
            there,
            &found,
            "design chats open in the app",
        );
        refused(
            OpenParams {
                terminal: Some("t1".into()),
                ..params(None, None, None)
            },
            there,
            &found,
            "its terminals' folders are there",
        );
        refused(
            params(Some("beta"), None, None),
            there,
            &found,
            "beta's worktree is outside moving's folder",
        );
        refused(
            params(None, Some("vscode"), None),
            there,
            &found,
            "VS Code is not installed here",
        );
        refused(
            params(None, Some("finder"), None),
            there,
            &found,
            "Only Cursor and VS Code open moving on w1",
        );
        refused(
            params(None, None, None),
            None,
            &found,
            "Where moving is on w1 is not known here",
        );
        refused(
            params(None, None, None),
            there,
            &found[1..],
            "Install Cursor or VS Code",
        );
    }

    #[test]
    fn a_workspace_on_a_worker_opens_with_the_editors_own_command_over_ssh() {
        let known = |id: &str| KNOWN.iter().find(|k| k.id == id).unwrap();
        let folder = Path::new("/home/me/chauffeur/workspaces/moving");
        let (program, args) = remote_command(
            known("vscode"),
            Path::new("/Applications/Visual Studio Code.app"),
            "chauffeur-worker-w1",
            folder,
        );
        let expected = if cfg!(target_os = "macos") {
            "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"
        } else {
            "/Applications/Visual Studio Code.app"
        };
        assert_eq!(program, expected);
        assert_eq!(
            args,
            [
                "--remote",
                "ssh-remote+chauffeur-worker-w1",
                "/home/me/chauffeur/workspaces/moving"
            ]
        );
        let (program, _) = remote_command(
            known("cursor"),
            Path::new("/Applications/Cursor.app"),
            "chauffeur-worker-w1",
            folder,
        );
        if cfg!(target_os = "macos") {
            assert_eq!(
                program,
                "/Applications/Cursor.app/Contents/Resources/app/bin/cursor"
            );
        }
    }

    #[test]
    fn revealing_a_folder_selects_its_first_visible_entry() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("aime");
        std::fs::create_dir_all(folder.join("previews")).unwrap();
        std::fs::write(folder.join(".DS_Store"), "").unwrap();
        std::fs::write(folder.join("recipe.json"), "").unwrap();
        assert_eq!(reveal_target(&folder), folder.join("previews"));
        let empty = dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(reveal_target(&empty), empty);
        assert_eq!(
            reveal_target(&folder.join("recipe.json")),
            folder.join("recipe.json")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn revealing_a_file_selects_it_and_runs_nothing() {
        let (program, args) = reveal_command(Path::new("/ws/xyz-1/.chauffeur/hero.png"));
        #[cfg(target_os = "macos")]
        assert_eq!(
            (program, args),
            (
                "/usr/bin/open",
                vec!["-R".into(), "/ws/xyz-1/.chauffeur/hero.png".into()]
            )
        );
        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            (program, args),
            ("xdg-open", vec!["/ws/xyz-1/.chauffeur".to_string()])
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apps_are_found_in_the_app_folders_finder_always() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Cursor.app")).unwrap();
        std::fs::create_dir_all(dir.path().join("iTerm.app")).unwrap();
        let found = installed(&[dir.path().to_path_buf()]);
        let ids: Vec<&str> = found.iter().map(|(k, _)| k.id).collect();
        assert_eq!(ids, ["cursor", "finder", "iterm"]);
        assert_eq!(default_id(&found, None), "cursor");
        assert_eq!(default_id(&found, Some("iterm")), "iterm");
        // A default that was uninstalled falls back to the first editor.
        assert_eq!(default_id(&found, Some("zed")), "cursor");
        assert_eq!(default_id(&installed(&[]), None), "finder");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn each_app_gets_the_folder_its_way() {
        let folder = Path::new("/ws/xyz-1");
        let by_id = |id: &str| KNOWN.iter().find(|k| k.id == id).unwrap();
        let open = |args: &[&str]| {
            (
                "/usr/bin/open".to_string(),
                args.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
            )
        };
        assert_eq!(command(by_id("finder"), None, folder), open(&["/ws/xyz-1"]));
        assert_eq!(
            command(
                by_id("cursor"),
                Some(Path::new("/Applications/Cursor.app")),
                folder
            ),
            open(&["-a", "/Applications/Cursor.app", "/ws/xyz-1"])
        );
        assert_eq!(
            command(
                by_id("ghostty"),
                Some(Path::new("/Applications/Ghostty.app")),
                folder
            ),
            open(&[
                "-na",
                "/Applications/Ghostty.app",
                "--args",
                "--working-directory=/ws/xyz-1"
            ])
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn apps_are_found_on_the_path_the_file_manager_always() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        for (name, mode) in [("zeditor", 0o755), ("kitty", 0o755), ("cursor", 0o644)] {
            let path = dir.path().join(name);
            std::fs::write(&path, "").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let found = installed(&[dir.path().to_path_buf()]);
        let ids: Vec<&str> = found.iter().map(|(k, _)| k.id).collect();
        // `cursor` is not executable.
        assert_eq!(ids, ["zed", "files", "kitty"]);
        assert_eq!(
            found[0].1.as_deref(),
            Some(dir.path().join("zeditor").as_path())
        );
        assert_eq!(default_id(&found, None), "zed");
        assert_eq!(default_id(&installed(&[]), None), "files");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn each_app_gets_the_folder_its_way() {
        let folder = Path::new("/ws/xyz-1");
        let by_id = |id: &str| KNOWN.iter().find(|k| k.id == id).unwrap();
        let run = |program: &str, args: &[&str]| {
            (
                program.to_string(),
                args.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            command(by_id("files"), None, folder),
            run("xdg-open", &["/ws/xyz-1"])
        );
        assert_eq!(
            command(by_id("cursor"), Some(Path::new("/usr/bin/cursor")), folder),
            run("/usr/bin/cursor", &["/ws/xyz-1"])
        );
        assert_eq!(
            command(
                by_id("ghostty"),
                Some(Path::new("/usr/bin/ghostty")),
                folder
            ),
            run("/usr/bin/ghostty", &["--working-directory=/ws/xyz-1"])
        );
        assert_eq!(
            command(
                by_id("wezterm"),
                Some(Path::new("/usr/bin/wezterm")),
                folder
            ),
            run("/usr/bin/wezterm", &["start", "--cwd", "/ws/xyz-1"])
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_graphical_session_is_found_from_its_sockets() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["wayland-1", "wayland-1.lock", "wayland-0", "bus"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        assert_eq!(
            lowest_socket(dir.path(), "wayland-").as_deref(),
            Some("wayland-0")
        );
        assert_eq!(lowest_socket(dir.path(), "X"), None);
    }
}
