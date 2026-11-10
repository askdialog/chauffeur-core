//! This Mac's Claude Code configuration (`CLAUDE.md`, settings, skills, agents, commands, hooks,
//! installed plugins and user MCP servers), handed to a GCP worker on each send so that its
//! agents work there as here (docs/cloud-workers.md). Never the login, the transcripts, the
//! history or the caches. The archive goes once per change: the worker keeps the digest of the
//! one it installed. Its contents are never logged.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, bail, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chauffeur_protocol::client::EmptyParams;
use chauffeur_protocol::worker::{
    WorkerClaudeConfigChunk, WorkerClaudeConfigInstallParams, WorkerClaudeConfigInstalled,
    WorkerClaudeConfigStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::worker_credentials::{private_dir_all, replace_private};
use crate::{Daemon, Error, Result};

/// The files of the configuration folder copied, when present.
const FILES: &[&str] = &[
    "CLAUDE.md",
    "settings.json",
    "plugins/installed_plugins.json",
    "plugins/known_marketplaces.json",
];
/// The folders copied whole, when present: one deleted here goes there too.
const DIRS: &[&str] = &[
    "skills",
    "agents",
    "commands",
    "hooks",
    "plugins/cache",
    "plugins/marketplaces",
];
/// Files naming this Mac's paths, which are rewritten to the worker's.
const REWRITTEN: &[&str] = &[
    "settings.json",
    "plugins/installed_plugins.json",
    "plugins/known_marketplaces.json",
];
/// The archive's own entry: this Mac's paths and its user MCP servers.
const META: &str = ".chauffeur-claude-config.json";
/// The largest chunk a driver sends.
pub(crate) const CHUNK: usize = 8 << 20;
/// Held while this daemon sends a configuration: two sends of a batch would write the worker's
/// one archive over each other; the second finds it installed instead.
pub(crate) static SENDING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Where Claude Code keeps its configuration on a machine, as its agents resolve it:
/// `CLAUDE_CONFIG_DIR` when set, else `~/.claude`; its state file (`.claude.json`, where user
/// MCP servers live) is inside that folder when it is set, else in the home folder.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Locations {
    home: PathBuf,
    config: PathBuf,
    state: PathBuf,
}

impl Locations {
    pub(crate) fn of(vars: &BTreeMap<String, String>) -> Option<Locations> {
        let set = |name: &str| vars.get(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = set("HOME").or_else(|| std::env::var_os("HOME").map(PathBuf::from))?;
        Some(match set("CLAUDE_CONFIG_DIR") {
            Some(config) => Locations {
                state: config.join(".claude.json"),
                config,
                home,
            },
            None => Locations {
                config: home.join(".claude"),
                state: home.join(".claude.json"),
                home,
            },
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Meta {
    home: String,
    config: String,
    mcp_servers: Map<String, Value>,
}

/// An archive of this Mac's configuration, in a temporary file the driver sends from.
pub(crate) struct Archive {
    pub(crate) file: tempfile::NamedTempFile,
    pub(crate) digest: String,
    pub(crate) bytes: u64,
}

/// Packs this Mac's configuration into a temporary file under `dir`; none when it has none.
/// The entries are sorted and their times and owners left out, so an unchanged configuration
/// gives the same digest.
pub(crate) fn pack(at: &Locations, dir: &Path) -> anyhow::Result<Option<Archive>> {
    let mcp_servers = match fs::read_to_string(&at.state) {
        Ok(text) => match serde_json::from_str::<Value>(&text)
            .context("reading Claude Code's state file")?
            .get("mcpServers")
        {
            Some(Value::Object(servers)) => servers.clone(),
            _ => Map::new(),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(error) => return Err(error).context("reading Claude Code's state file"),
    };
    let file = tempfile::NamedTempFile::new_in(dir)?;
    let mut tar = tar::Builder::new(zstd::Encoder::new(file.reopen()?, 3)?);
    let meta = serde_json::to_vec(&Meta {
        home: text(&at.home)?.to_owned(),
        config: text(&at.config)?.to_owned(),
        mcp_servers: mcp_servers.clone(),
    })?;
    tar.append_data(&mut header(meta.len() as u64, 0o600), META, meta.as_slice())?;
    let mut copied = 0;
    for name in FILES {
        // Followed: a dotfiles setup links them.
        let path = at.config.join(name);
        if fs::metadata(&path).is_ok_and(|m| m.is_file()) {
            append_file(&mut tar, &path, name)?;
            copied += 1;
        }
    }
    for name in DIRS {
        let root = at.config.join(name);
        if !fs::metadata(&root).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        copied += 1;
        for entry in walkdir::WalkDir::new(&root).sort_by_file_name() {
            let entry = entry?;
            let relative = entry.path().strip_prefix(&at.config)?;
            let name = text(relative)?;
            let kind = entry.file_type();
            if kind.is_dir() {
                let mut dir = header(0, 0o755);
                dir.set_entry_type(tar::EntryType::Directory);
                tar.append_data(&mut dir, name, std::io::empty())?;
            } else if kind.is_file() {
                append_file(&mut tar, entry.path(), name)?;
            } else if kind.is_symlink() {
                // A link out of its folder (a plugin's absolute one) would not hold there.
                let target = fs::read_link(entry.path())?;
                if within(Path::new(name), &target, Path::new(name_root(name))) {
                    let mut link = header(0, 0o777);
                    link.set_entry_type(tar::EntryType::Symlink);
                    tar.append_link(&mut link, name, &target)?;
                }
            }
        }
    }
    if copied == 0 && mcp_servers.is_empty() {
        return Ok(None);
    }
    tar.into_inner()?.finish()?;
    let (digest, bytes) = digest(file.reopen()?)?;
    Ok(Some(Archive {
        file,
        digest,
        bytes,
    }))
}

/// The SHA-256 of what `reader` gives, base64url, and its length.
fn digest(mut reader: impl Read) -> std::io::Result<(String, u64)> {
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    let mut bytes = 0;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok((URL_SAFE_NO_PAD.encode(hash.finalize()), bytes));
        }
        hash.update(&buffer[..read]);
        bytes += read as u64;
    }
}

fn text(path: &Path) -> anyhow::Result<&str> {
    path.to_str().context("a Claude Code path is not UTF-8")
}

fn header(size: u64, mode: u32) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_size(size);
    header.set_mode(mode);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header
}

fn append_file<W: Write>(tar: &mut tar::Builder<W>, path: &Path, name: &str) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let file = fs::File::open(path).with_context(|| format!("reading {name}"))?;
    let meta = file.metadata()?;
    let mode = if meta.permissions().mode() & 0o111 != 0 {
        0o755
    } else {
        0o644
    };
    tar.append_data(&mut header(meta.len(), mode), name, file)?;
    Ok(())
}

/// The copied folder an archive path belongs to (`plugins/cache` for `plugins/cache/x/y`).
fn name_root(name: &str) -> &str {
    DIRS.iter()
        .find(|dir| name == **dir || name.starts_with(&format!("{dir}/")))
        .copied()
        .unwrap_or(name)
}

/// Whether the link at `link`, pointing at `target`, stays inside `root` (all three relative to
/// the configuration folder), read without following anything.
fn within(link: &Path, target: &Path, root: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut resolved: Vec<Component> = link
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .collect();
    for component in target.components() {
        match component {
            Component::Normal(_) => resolved.push(component),
            Component::CurDir => {}
            Component::ParentDir => {
                if resolved.pop().is_none() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    resolved.iter().collect::<PathBuf>().starts_with(root)
}

/// Where a worker keeps what it received and the digest it installed.
fn state_dir(daemon: &Daemon) -> PathBuf {
    daemon.paths.home.join("claude-config")
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 43
        && digest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// On the worker: the digest of the configuration it installed last.
pub(crate) fn status(daemon: &Daemon) -> WorkerClaudeConfigStatus {
    WorkerClaudeConfigStatus {
        digest: fs::read_to_string(state_dir(daemon).join("installed"))
            .ok()
            .map(|digest| digest.trim().to_owned())
            .filter(|digest| valid_digest(digest)),
    }
}

/// On the worker: a chunk of the archive, appended at its end; offset 0 starts it again.
pub(crate) fn write(daemon: &Daemon, chunk: WorkerClaudeConfigChunk) -> Result<EmptyParams> {
    let bad =
        |what: &str| Error::BadRequest(format!("Invalid Claude Code configuration chunk: {what}"));
    if !valid_digest(&chunk.digest) {
        return Err(bad("digest"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(&chunk.bytes)
        .map_err(|_| bad("encoding"))?;
    if bytes.is_empty() || bytes.len() > CHUNK {
        return Err(bad("size"));
    }
    let dir = state_dir(daemon);
    private_dir_all(&dir)?;
    let path = dir.join("incoming");
    let mut file = if chunk.offset == 0 {
        fs::File::create(&path)?
    } else {
        let file = fs::OpenOptions::new().append(true).open(&path)?;
        if file.metadata()?.len() != chunk.offset {
            return Err(bad("offset"));
        }
        file
    };
    file.write_all(&bytes)?;
    Ok(EmptyParams {})
}

/// On the worker: checks the archive written whole, then puts the configuration in place and
/// records its digest. What is left out (an MCP server that cannot run here) is a warning.
pub(crate) async fn install(
    daemon: &Daemon,
    params: WorkerClaudeConfigInstallParams,
) -> Result<WorkerClaudeConfigInstalled> {
    if !valid_digest(&params.digest) {
        return Err(Error::BadRequest(
            "Invalid Claude Code configuration digest".into(),
        ));
    }
    let vars = daemon.shell_env.vars();
    let at = Locations::of(&vars).ok_or_else(|| {
        Error::Unavailable("This machine has no home folder for Claude Code".into())
    })?;
    let dir = state_dir(daemon);
    let machine = crate::cloud_transfer::host_name();
    let warnings = tokio::task::spawn_blocking(move || {
        let incoming = dir.join("incoming");
        let result = (|| {
            let file = fs::File::open(&incoming).context("no archive was written")?;
            let (digest, bytes) = digest(file)?;
            ensure!(
                bytes == params.bytes && digest == params.digest,
                "the archive received is not the one sent"
            );
            let warnings = put_in_place(&incoming, &at, &vars, &machine)?;
            fs::write(dir.join("installed"), &params.digest)?;
            anyhow::Ok(warnings)
        })();
        let _ = fs::remove_file(&incoming);
        result
    })
    .await
    .map_err(|error| Error::Internal(error.into()))??;
    Ok(WorkerClaudeConfigInstalled { warnings })
}

/// Unpacks `archive` beside the configuration folder `at` names, then replaces each copied item
/// there: a folder whole, a file by a rename.
fn put_in_place(
    archive: &Path,
    at: &Locations,
    vars: &BTreeMap<String, String>,
    machine: &str,
) -> anyhow::Result<Vec<String>> {
    private_dir_all(&at.config)?;
    let staging = at
        .config
        .join(format!(".chauffeur-staging-{}", uuid::Uuid::now_v7()));
    let result = (|| {
        let meta = unpack(archive, &staging)?;
        for name in REWRITTEN {
            let path = staging.join(name);
            if path.exists() {
                let text = fs::read_to_string(&path)?;
                fs::write(&path, rewrite(&text, &meta, at)?)?;
            }
        }
        for name in FILES.iter().chain(DIRS) {
            let staged = staging.join(name);
            if fs::symlink_metadata(&staged).is_err() {
                continue;
            }
            let target = at.config.join(name);
            fs::create_dir_all(target.parent().context("a copied item has a parent")?)?;
            if staged.is_dir() && fs::symlink_metadata(&target).is_ok() {
                let old = at
                    .config
                    .join(format!(".chauffeur-old-{}", uuid::Uuid::now_v7()));
                fs::rename(&target, &old)?;
                fs::rename(&staged, &target)?;
                fs::remove_dir_all(&old)?;
            } else {
                fs::rename(&staged, &target)?;
            }
        }
        let settings = at.config.join("settings.json");
        if settings.exists() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&settings, fs::Permissions::from_mode(0o600))?;
        }
        merge_mcp_servers(&meta, at, vars, machine)
    })();
    let _ = fs::remove_dir_all(&staging);
    result
}

/// Unpacks the archive into `staging`, refusing anything outside the copied items: an absolute
/// path, a `..`, an entry under a link, a link leaving its folder, a device or a duplicate.
fn unpack(archive: &Path, staging: &Path) -> anyhow::Result<Meta> {
    use std::os::unix::fs::PermissionsExt;
    let mut tar = tar::Archive::new(zstd::Decoder::new(fs::File::open(archive)?)?);
    let mut meta = None;
    let mut links: HashSet<PathBuf> = HashSet::new();
    fs::create_dir(staging)?;
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        ensure!(
            path.components().all(|c| matches!(c, Component::Normal(_))),
            "unsafe path in the Claude Code configuration: {}",
            path.display()
        );
        let name = text(&path)?.to_owned();
        if name == META {
            let mut bytes = Vec::new();
            entry.take(16 << 20).read_to_end(&mut bytes)?;
            meta = Some(serde_json::from_slice::<Meta>(&bytes)?);
            continue;
        }
        let root = name_root(&name);
        ensure!(
            FILES.contains(&name.as_str()) || DIRS.contains(&root),
            "unexpected entry in the Claude Code configuration: {name}"
        );
        ensure!(
            !path.ancestors().skip(1).any(|a| links.contains(a)),
            "an entry under a link in the Claude Code configuration: {name}"
        );
        let target = staging.join(&path);
        ensure!(
            fs::symlink_metadata(&target).is_err(),
            "a duplicate entry in the Claude Code configuration: {name}"
        );
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            fs::create_dir(&target)?;
        } else if kind.is_file() {
            let mut file = fs::File::create(&target)?;
            std::io::copy(&mut entry, &mut file)?;
            let mode = if entry.header().mode()? & 0o111 != 0 {
                0o755
            } else {
                0o644
            };
            fs::set_permissions(&target, fs::Permissions::from_mode(mode))?;
        } else if kind.is_symlink() {
            let link = entry.link_name()?.context("a link without a target")?;
            ensure!(
                within(&path, &link, Path::new(root)),
                "a link leaving its folder in the Claude Code configuration: {name}"
            );
            std::os::unix::fs::symlink(&link, &target)?;
            links.insert(path);
        } else {
            bail!("unsupported entry in the Claude Code configuration: {name}");
        }
    }
    meta.context("the Claude Code configuration has no description")
}

/// `text` with the Mac's configuration folder, then its home folder, made the worker's.
fn rewrite(text: &str, meta: &Meta, at: &Locations) -> anyhow::Result<String> {
    let config = replace_path(text, &meta.config, self::text(&at.config)?);
    Ok(replace_path(&config, &meta.home, self::text(&at.home)?))
}

/// Every `from` that is a whole path (not followed by more of a name) replaced by `to`.
fn replace_path(text: &str, from: &str, to: &str) -> String {
    if from.is_empty() || from == to {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(from) {
        let after = &rest[at + from.len()..];
        let whole = !after
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-'));
        out.push_str(&rest[..at]);
        out.push_str(if whole { to } else { from });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Adds the Mac's user MCP servers to the worker's state file, its other keys and servers kept,
/// except those that cannot run here: one served on the Mac's own address, or whose command
/// this machine does not have.
fn merge_mcp_servers(
    meta: &Meta,
    at: &Locations,
    vars: &BTreeMap<String, String>,
    machine: &str,
) -> anyhow::Result<Vec<String>> {
    if meta.mcp_servers.is_empty() {
        return Ok(Vec::new());
    }
    let mut warnings = Vec::new();
    let mut kept = Map::new();
    for (name, server) in &meta.mcp_servers {
        let server: Value = serde_json::from_str(&rewrite(&server.to_string(), meta, at)?)?;
        if let Some(url) = server.get("url").and_then(Value::as_str)
            && loopback(url)
        {
            warnings.push(format!(
                "Claude Code's MCP server {name} was not copied to {machine}: it is served on this Mac's own address"
            ));
            continue;
        }
        if let Some(command) = server.get("command").and_then(Value::as_str)
            && !resolves(command, vars)
        {
            warnings.push(format!(
                "Claude Code's MCP server {name} was not copied to {machine}: {machine} has no {command}"
            ));
            continue;
        }
        kept.insert(name.clone(), server);
    }
    let mut state = match fs::read_to_string(&at.state) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .context("reading Claude Code's state file there")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(error) => return Err(error).context("reading Claude Code's state file there"),
    };
    let object = state
        .as_object_mut()
        .context("Claude Code's state file there is not an object")?;
    let servers = object
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()));
    if !servers.is_object() {
        *servers = Value::Object(Map::new());
    }
    servers
        .as_object_mut()
        .expect("made an object above")
        .extend(kept);
    replace_private(&at.state, &serde_json::to_string_pretty(&state)?)?;
    Ok(warnings)
}

/// Whether an MCP server's URL is this machine's own address, which on a worker is not the Mac.
fn loopback(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if let Some(bracketed) = host.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or_default()
    } else {
        host.split(':').next().unwrap_or_default()
    };
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Whether `command` runs here: a path that exists, or a name on the login shell's PATH.
fn resolves(command: &str, vars: &BTreeMap<String, String>) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let executable = |path: &Path| {
        fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if command.contains('/') {
        return executable(Path::new(command));
    }
    vars.get("PATH")
        .map(String::as_str)
        .unwrap_or_default()
        .split(':')
        .filter(|dir| !dir.is_empty())
        .any(|dir| executable(&Path::new(dir).join(command)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn locations(home: &Path) -> Locations {
        Locations {
            home: home.to_owned(),
            config: home.join(".claude"),
            state: home.join(".claude.json"),
        }
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// A Mac's configuration: what is copied, and what never is.
    fn mac(home: &Path) -> Locations {
        let at = locations(home);
        let c = &at.config;
        let h = home.display();
        write(&c.join("CLAUDE.md"), "# rules");
        write(
            &c.join("settings.json"),
            &format!("{{\"hooks\":\"bash {h}/.claude/hooks/a.sh\",\"other\":\"{h}2\"}}"),
        );
        write(&c.join("skills/review/SKILL.md"), "skill");
        write(&c.join("hooks/a.sh"), "#!/bin/sh");
        write(
            &c.join("plugins/installed_plugins.json"),
            &format!("{{\"installPath\":\"{h}/.claude/plugins/cache/p\"}}"),
        );
        write(&c.join("plugins/cache/p/plugin.json"), "{}");
        std::os::unix::fs::symlink("plugin.json", c.join("plugins/cache/p/link.json")).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", c.join("plugins/cache/p/out.json")).unwrap();
        write(&c.join("plugins/plugin-catalog-cache.json"), "{}");
        write(&c.join(".credentials.json"), "secret");
        write(&c.join("projects/x/chat.jsonl"), "transcript");
        write(&c.join("history.jsonl"), "history");
        write(&c.join("settings.json.bak"), "old");
        write(
            &at.state,
            &format!(
                "{{\"oauthAccount\":{{\"email\":\"me\"}},\"mcpServers\":{{\
                 \"remote\":{{\"type\":\"http\",\"url\":\"https://mcp.example.com/mcp\"}},\
                 \"local\":{{\"type\":\"http\",\"url\":\"http://127.0.0.1:8787/mcp\"}},\
                 \"tool\":{{\"command\":\"{h}/bin/tool\"}},\
                 \"shell\":{{\"command\":\"sh\"}},\
                 \"absent\":{{\"command\":\"no-such-command-here\"}}}}}}"
            ),
        );
        at
    }

    fn installed(home: &Path) -> (tempfile::TempDir, Locations) {
        let mac_home = tempfile::tempdir().unwrap();
        let mac_at = mac(mac_home.path());
        let archive = pack(&mac_at, mac_home.path()).unwrap().unwrap();
        let at = locations(home);
        write(&home.join("bin/tool"), "#!/bin/sh");
        fs::set_permissions(home.join("bin/tool"), fs::Permissions::from_mode(0o755)).unwrap();
        write(
            &at.state,
            "{\"oauthAccount\":{\"email\":\"worker\"},\"mcpServers\":{\"own\":{\"command\":\"sh\"}}}",
        );
        let vars = BTreeMap::from([("PATH".to_string(), "/bin:/usr/bin".to_string())]);
        let warnings = put_in_place(archive.file.path(), &at, &vars, "w1").unwrap();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("local") && w.contains("own address"))
        );
        assert!(warnings.iter().any(|w| w.contains("absent")));
        (mac_home, at)
    }

    #[test]
    fn the_worker_gets_the_configuration_but_never_the_login_transcripts_or_caches() {
        let home = tempfile::tempdir().unwrap();
        let (_mac, at) = installed(home.path());
        let c = &at.config;
        assert_eq!(fs::read_to_string(c.join("CLAUDE.md")).unwrap(), "# rules");
        assert_eq!(
            fs::read_to_string(c.join("skills/review/SKILL.md")).unwrap(),
            "skill"
        );
        assert!(c.join("plugins/cache/p/plugin.json").exists());
        assert_eq!(
            fs::read_link(c.join("plugins/cache/p/link.json")).unwrap(),
            Path::new("plugin.json")
        );
        for never in [
            "plugins/cache/p/out.json",
            "plugins/plugin-catalog-cache.json",
            ".credentials.json",
            "projects",
            "history.jsonl",
            "settings.json.bak",
        ] {
            assert!(fs::symlink_metadata(c.join(never)).is_err(), "{never}");
        }
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&c.join("settings.json")), 0o600);
        assert_eq!(mode(c), 0o700);
        assert_eq!(mode(&c.join("hooks/a.sh")), 0o644);
    }

    #[test]
    fn the_mac_s_paths_become_the_worker_s_in_settings_plugins_and_mcp_servers_only_as_whole_paths()
    {
        let home = tempfile::tempdir().unwrap();
        let (mac, at) = installed(home.path());
        let worker = home.path().display().to_string();
        let mac = mac.path().display().to_string();
        let settings = fs::read_to_string(at.config.join("settings.json")).unwrap();
        assert!(
            settings.contains(&format!("bash {worker}/.claude/hooks/a.sh")),
            "{settings}"
        );
        // Another folder whose name starts like the home is not the home.
        assert!(settings.contains(&format!("{mac}2")), "{settings}");
        let plugins = fs::read_to_string(at.config.join("plugins/installed_plugins.json")).unwrap();
        assert!(
            plugins.contains(&format!("{worker}/.claude/plugins/cache/p")),
            "{plugins}"
        );
        let state: Value = serde_json::from_str(&fs::read_to_string(&at.state).unwrap()).unwrap();
        assert_eq!(
            state["mcpServers"]["tool"]["command"],
            format!("{worker}/bin/tool")
        );
    }

    #[test]
    fn the_mac_s_mcp_servers_join_the_worker_s_state_file_without_touching_its_other_keys() {
        let home = tempfile::tempdir().unwrap();
        let (_mac, at) = installed(home.path());
        let state: Value = serde_json::from_str(&fs::read_to_string(&at.state).unwrap()).unwrap();
        assert_eq!(state["oauthAccount"]["email"], "worker");
        let servers = state["mcpServers"].as_object().unwrap();
        let mut names: Vec<_> = servers.keys().map(String::as_str).collect();
        names.sort();
        assert_eq!(names, ["own", "remote", "shell", "tool"]);
        assert_eq!(
            fs::metadata(&at.state).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn an_unchanged_configuration_packs_to_the_same_digest_and_a_change_to_another() {
        let mac_home = tempfile::tempdir().unwrap();
        let at = mac(mac_home.path());
        let first = pack(&at, mac_home.path()).unwrap().unwrap();
        // Times are left out: touching a file changes nothing.
        fs::write(at.config.join("CLAUDE.md"), "# rules").unwrap();
        let again = pack(&at, mac_home.path()).unwrap().unwrap();
        assert_eq!(first.digest, again.digest);
        fs::write(at.config.join("skills/review/SKILL.md"), "changed").unwrap();
        assert_ne!(
            first.digest,
            pack(&at, mac_home.path()).unwrap().unwrap().digest
        );
    }

    #[test]
    fn a_mac_without_claude_code_configuration_packs_nothing() {
        let home = tempfile::tempdir().unwrap();
        assert!(
            pack(&locations(home.path()), home.path())
                .unwrap()
                .is_none()
        );
    }

    fn archive_of(entries: impl FnOnce(&mut tar::Builder<Vec<u8>>)) -> tempfile::NamedTempFile {
        let mut tar = tar::Builder::new(Vec::new());
        let meta = br#"{"home":"/m","config":"/m/.claude","mcp_servers":{}}"#;
        tar.append_data(&mut header(meta.len() as u64, 0o600), META, &meta[..])
            .unwrap();
        entries(&mut tar);
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut encoder = zstd::Encoder::new(file.reopen().unwrap(), 3).unwrap();
        encoder.write_all(&tar.into_inner().unwrap()).unwrap();
        encoder.finish().unwrap();
        file
    }

    fn raw_path(header: &mut tar::Header, path: &str) {
        let name = &mut header.as_old_mut().name;
        name.fill(0);
        name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_cksum();
    }

    #[test]
    fn archive_entries_reaching_out_of_the_copied_items_are_refused() {
        type Entries = Box<dyn FnOnce(&mut tar::Builder<Vec<u8>>)>;
        let cases: Vec<(&str, Entries)> = vec![
            (
                "a parent folder",
                Box::new(|tar| {
                    let mut h = header(1, 0o644);
                    raw_path(&mut h, "skills/../../escaped");
                    tar.append(&h, &b"x"[..]).unwrap();
                }),
            ),
            (
                "an absolute path",
                Box::new(|tar| {
                    let mut h = header(1, 0o644);
                    raw_path(&mut h, "/tmp/escaped");
                    tar.append(&h, &b"x"[..]).unwrap();
                }),
            ),
            (
                "an item not copied",
                Box::new(|tar| {
                    tar.append_data(&mut header(1, 0o644), ".credentials.json", &b"x"[..])
                        .unwrap();
                }),
            ),
            (
                "a link out of its folder",
                Box::new(|tar| {
                    let mut h = header(0, 0o777);
                    h.set_entry_type(tar::EntryType::Symlink);
                    tar.append_link(&mut h, "skills/out", "../../..").unwrap();
                }),
            ),
            (
                "an entry under a link",
                Box::new(|tar| {
                    let mut h = header(0, 0o777);
                    h.set_entry_type(tar::EntryType::Symlink);
                    tar.append_link(&mut h, "skills/dir", "here").unwrap();
                    tar.append_data(&mut header(1, 0o644), "skills/dir/file", &b"x"[..])
                        .unwrap();
                }),
            ),
        ];
        for (case, entries) in cases {
            let root = tempfile::tempdir().unwrap();
            let archive = archive_of(entries);
            let staging = root.path().join("staging");
            assert!(unpack(archive.path(), &staging).is_err(), "{case}");
            assert!(!root.path().join("escaped").exists(), "{case}");
        }
    }

    #[test]
    fn a_loopback_url_is_told_apart_from_a_remote_one() {
        for url in [
            "http://127.0.0.1:8787/mcp",
            "http://localhost/mcp",
            "http://user@LOCALHOST:1/x",
            "http://[::1]:9/mcp",
            "http://app.localhost/mcp",
        ] {
            assert!(loopback(url), "{url}");
        }
        for url in ["https://mcp.example.com/mcp", "https://127.example.com/x"] {
            assert!(!loopback(url), "{url}");
        }
    }

    #[test]
    fn locations_follow_claude_config_dir_and_its_state_file_inside_it() {
        let vars = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>()
        };
        let plain = Locations::of(&vars(&[("HOME", "/h")])).unwrap();
        assert_eq!(plain.config, Path::new("/h/.claude"));
        assert_eq!(plain.state, Path::new("/h/.claude.json"));
        let set = Locations::of(&vars(&[("HOME", "/h"), ("CLAUDE_CONFIG_DIR", "/c")])).unwrap();
        assert_eq!(set.config, Path::new("/c"));
        assert_eq!(set.state, Path::new("/c/.claude.json"));
    }
}
