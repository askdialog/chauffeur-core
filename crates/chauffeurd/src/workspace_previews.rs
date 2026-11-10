//! A workspace's web previews (docs/workspace-development-services.md, decided with Justin
//! 2026-10-27): the ports of its block that answer HTTP, detected, then declared by the user in
//! its development profile by their place in the block; and what each declared port does now,
//! for the phone to list and open while the workspace runs on another machine. Probing only
//! connects to loopback: nothing is started or stopped here.
use std::collections::BTreeSet;
use std::time::Duration;

use chauffeur_protocol::client::ProcessInfo;
use chauffeur_protocol::dev_services::{
    DeclaredPreview, DetectedPreview, Preview, PreviewChoice, PreviewState, Previews,
};
use chauffeur_protocol::events::ProcessKind;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::store::PortBlock;
use crate::workspace_development_profile::{self as profiles, PreviewPort};
use crate::{Daemon, Error, Result};

/// How long a port has to accept a connection.
const CONNECT: Duration = Duration::from_millis(500);
/// How long what listens has to start an HTTP answer.
const ANSWER: Duration = Duration::from_secs(2);

/// Whether something accepts a connection on `port` of this machine's loopback.
async fn accepts(port: u16) -> bool {
    matches!(
        tokio::time::timeout(CONNECT, TcpStream::connect(("127.0.0.1", port))).await,
        Ok(Ok(_))
    )
}

/// Whether what listens on `port` answers HTTP: a database or a debugger port is no preview.
async fn answers_http(port: u16) -> bool {
    let attempt = async {
        let mut stream = tokio::time::timeout(CONNECT, TcpStream::connect(("127.0.0.1", port)))
            .await
            .ok()?
            .ok()?;
        let request = format!("HEAD / HTTP/1.0\r\nHost: localhost:{port}\r\n\r\n");
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut head = [0u8; 5];
        stream.read_exact(&mut head).await.ok()?;
        Some(&head == b"HTTP/")
    };
    matches!(tokio::time::timeout(ANSWER, attempt).await, Ok(Some(true)))
}

/// What a declared port does: something accepts on it, else what the workspace's run scripts
/// say (one running is still starting it; the last one failing is an error).
pub fn state(listening: bool, processes: &[ProcessInfo]) -> PreviewState {
    if listening {
        return PreviewState::Ready;
    }
    let mut runs: Vec<&ProcessInfo> = processes
        .iter()
        .filter(|p| p.kind == ProcessKind::Run)
        .collect();
    if runs.iter().any(|p| p.running) {
        return PreviewState::Starting;
    }
    // Ids are time-ordered (UUID v7): the last one ran last.
    runs.sort_by(|a, b| a.id.cmp(&b.id));
    match runs.last() {
        Some(last) if last.exit_code.is_some_and(|code| code != 0) => PreviewState::Error,
        _ => PreviewState::Stopped,
    }
}

/// The port of `block` at `offset`, when the block has that place.
fn port_at(block: Option<&PortBlock>, offset: u16) -> Option<u16> {
    block
        .filter(|b| offset < b.size)
        .and_then(|b| b.base.checked_add(offset))
}

/// The declared previews of a profile, with their port on this machine.
pub fn declared(block: Option<&PortBlock>, previews: &[PreviewPort]) -> Vec<DeclaredPreview> {
    previews
        .iter()
        .map(|p| DeclaredPreview {
            name: p.name.clone(),
            offset: p.offset,
            port: port_at(block, p.offset),
        })
        .collect()
}

/// `workspace`'s declared previews and what each port does now, on this machine.
pub async fn list(daemon: &Daemon, workspace: &str) -> Result<Previews> {
    let row = daemon.workspaces.row(workspace)?;
    let block = daemon.store.port_block(&row.id)?;
    let profile = profiles::load(&daemon.store, &row.id)?;
    let processes = daemon.procs.list(&row.id);
    let mut previews = Vec::new();
    for declared in profile.iter().flat_map(|p| &p.profile.preview_ports) {
        let Some(port) = port_at(block.as_ref(), declared.offset) else {
            continue;
        };
        previews.push(Preview {
            name: declared.name.clone(),
            offset: declared.offset,
            port,
            state: state(accepts(port).await, &processes),
        });
    }
    Ok(Previews {
        machine: None,
        previews,
    })
}

/// The ports of `workspace`'s block that answer HTTP now, and those declared (listed even when
/// nothing answers, never left out unseen). One not declared is named after the run script
/// listening on it, else `port-<port>`.
pub async fn detect(
    daemon: &Daemon,
    workspace_id: &str,
    declared: &[PreviewPort],
) -> Result<Vec<DetectedPreview>> {
    let Some(block) = daemon.store.port_block(workspace_id)? else {
        return Ok(Vec::new());
    };
    let offsets: Vec<u16> = (0..block.size).collect();
    let answering = futures_util::future::join_all(
        offsets
            .iter()
            .map(|offset| answers_http(block.base + offset)),
    )
    .await;
    let scripts = daemon.procs.running_run_groups(workspace_id);
    let mut names: BTreeSet<String> = declared.iter().map(|p| p.name.clone()).collect();
    let mut found = Vec::new();
    for (offset, listening) in offsets.into_iter().zip(answering) {
        let port = block.base + offset;
        let known = declared.iter().find(|p| p.offset == offset);
        if !listening && known.is_none() {
            continue;
        }
        let name = match known {
            Some(p) => p.name.clone(),
            None => {
                let script = match listener_group(port).await {
                    Some(group) => scripts
                        .iter()
                        .find(|(pgid, _)| *pgid == group)
                        .map(|(_, name)| preview_name(name)),
                    None => None,
                };
                let name = script
                    .filter(|name| !names.contains(name))
                    .unwrap_or_else(|| format!("port-{port}"));
                names.insert(name.clone());
                name
            }
        };
        found.push(DetectedPreview {
            name,
            port,
            offset,
            listening,
            declared: known.is_some(),
        });
    }
    Ok(found)
}

/// The process group of what listens on `port`, from `lsof`; none when it cannot tell.
async fn listener_group(port: u16) -> Option<i32> {
    let output = tokio::time::timeout(
        ANSWER,
        tokio::process::Command::new("lsof")
            .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fg"])
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix('g')?.parse().ok())
}

/// A preview name a profile accepts, from a run script's name.
fn preview_name(raw: &str) -> String {
    let name: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .take(80)
        .collect();
    if name.is_empty() { "web".into() } else { name }
}

/// The previews chosen, each by its port on this machine, as the profile keeps them.
pub fn chosen(
    daemon: &Daemon,
    workspace_id: &str,
    choices: &[PreviewChoice],
) -> Result<Vec<PreviewPort>> {
    let block = daemon
        .store
        .port_block(workspace_id)?
        .ok_or_else(|| Error::Conflict("The workspace has no ports for a preview".into()))?;
    let mut names = BTreeSet::new();
    let mut offsets = BTreeSet::new();
    let mut previews = Vec::new();
    for choice in choices {
        let valid = !choice.name.is_empty()
            && choice.name.len() <= 80
            && choice
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b));
        if !valid {
            return Err(Error::BadRequest(format!(
                "Name the preview on port {} with letters, digits, `_` or `-` only",
                choice.port
            )));
        }
        let offset = choice
            .port
            .checked_sub(block.base)
            .filter(|offset| *offset < block.size)
            .ok_or_else(|| {
                Error::BadRequest(format!(
                    "Port {} is not one of the workspace's ({}–{}); choose one of them",
                    choice.port,
                    block.base,
                    block.base + block.size - 1
                ))
            })?;
        if !names.insert(choice.name.as_str()) || !offsets.insert(offset) {
            return Err(Error::BadRequest(format!(
                "{} or port {} is chosen twice",
                choice.name, choice.port
            )));
        }
        previews.push(PreviewPort {
            name: choice.name.clone(),
            offset,
        });
    }
    Ok(previews)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, running: bool, exit_code: Option<i32>) -> ProcessInfo {
        ProcessInfo {
            id: id.into(),
            workspace_id: "w".into(),
            kind: ProcessKind::Run,
            repo: Some("shop".into()),
            name: "dev".into(),
            command: "bun run dev".into(),
            running,
            exit_code,
            tail: Vec::new(),
        }
    }

    #[test]
    fn a_preview_is_ready_when_its_port_accepts_else_told_by_the_run_scripts() {
        assert_eq!(state(true, &[]), PreviewState::Ready);
        assert_eq!(state(false, &[]), PreviewState::Stopped);
        assert_eq!(
            state(false, &[run("1", false, Some(1)), run("2", true, None)]),
            PreviewState::Starting
        );
        assert_eq!(
            state(false, &[run("1", false, Some(0)), run("2", false, Some(1))]),
            PreviewState::Error
        );
        // Stopped by the user (a signal, no code), after an earlier failure.
        assert_eq!(
            state(false, &[run("1", false, Some(1)), run("2", false, None)]),
            PreviewState::Stopped
        );
        let mut setup = run("3", true, None);
        setup.kind = ProcessKind::Setup;
        assert_eq!(state(false, &[setup]), PreviewState::Stopped);
    }

    #[test]
    fn a_declared_preview_names_its_port_on_this_machine_only_within_the_block() {
        let block = PortBlock {
            owner: "w".into(),
            workspace_id: "w".into(),
            base: 41000,
            size: 10,
        };
        let previews = [
            PreviewPort {
                name: "web".into(),
                offset: 0,
            },
            PreviewPort {
                name: "api".into(),
                offset: 12,
            },
        ];
        let shown = declared(Some(&block), &previews);
        assert_eq!(shown[0].port, Some(41000));
        assert_eq!(shown[1].port, None);
        assert_eq!(declared(None, &previews)[0].port, None);
    }

    #[test]
    fn a_run_script_name_becomes_a_preview_name() {
        assert_eq!(preview_name("dev"), "dev");
        assert_eq!(preview_name("shop:dev server"), "shop-dev-server");
        assert_eq!(preview_name(""), "web");
    }

    #[tokio::test]
    async fn only_a_port_that_answers_http_is_a_preview() {
        let web = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let web_port = web.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = web.accept().await {
                let mut request = [0u8; 256];
                let _ = stream.read(&mut request).await;
                let _ = stream.write_all(b"HTTP/1.0 200 OK\r\n\r\n").await;
            }
        });
        let raw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let raw_port = raw.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = raw.accept().await {
                let _ = stream.write_all(b"-ERR not http\r\n").await;
            }
        });
        assert!(answers_http(web_port).await);
        assert!(!answers_http(raw_port).await);
        assert!(accepts(raw_port).await);
        drop(TcpStream::connect(("127.0.0.1", web_port)).await);
    }
}
