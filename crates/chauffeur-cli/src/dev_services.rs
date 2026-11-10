//! `chauffeur services`: a workspace's development services. Shown as declared; `detect` lists the
//! containers that work or write in it; `save` declares the chosen ones, as data that follows the
//! workspace to another machine (`--data`, a PostgreSQL server) or a cache rebuilt there
//! (`--cache`); and its web previews, the ports of its block that answer HTTP (`--preview`), which
//! the phone opens while the workspace runs on another machine.
use chauffeur_protocol::client::{ClientCall, WorkspaceParams};
use chauffeur_protocol::dev_services::{
    DevServiceChoice, DevServices, DevServicesDetected, DevServicesSaveParams, PreviewChoice,
};
use clap::Subcommand;

use crate::client::Client;

#[derive(Subcommand)]
pub enum Action {
    /// The containers that work or write in the workspace, and what each could be declared as.
    Detect,
    /// Declare these services, as detected now; the others are left out.
    Save {
        /// A PostgreSQL server whose databases follow the workspace (repeatable).
        #[arg(long)]
        data: Vec<String>,
        /// A service rebuilt on another machine (repeatable).
        #[arg(long)]
        cache: Vec<String>,
        /// A web preview, `<name>=<port>` with a port of the workspace's (repeatable); they replace
        /// those declared, which stay when none is given.
        #[arg(long, value_parser = preview, conflicts_with = "no_preview")]
        preview: Vec<PreviewChoice>,
        /// Declare no web preview.
        #[arg(long)]
        no_preview: bool,
    },
}

fn preview(value: &str) -> Result<PreviewChoice, String> {
    let (name, port) = value
        .split_once('=')
        .ok_or("write a preview as <name>=<port>, e.g. web=41000")?;
    let port = port
        .parse()
        .map_err(|_| format!("{port} is not a port; write a preview as <name>=<port>"))?;
    Ok(PreviewChoice {
        name: name.to_owned(),
        port,
    })
}

pub async fn run(
    client: &mut Client,
    workspace: String,
    action: Option<Action>,
    json: bool,
) -> anyhow::Result<()> {
    match action {
        None => {
            let declared: DevServices = client
                .call(ClientCall::ServicesGet(WorkspaceParams { workspace }))
                .await?;
            super::print_or(json, &declared, || shown(&declared));
        }
        Some(Action::Detect) => {
            let detected: DevServicesDetected = client
                .call(ClientCall::ServicesDetect(WorkspaceParams { workspace }))
                .await?;
            super::print_or(json, &detected, || {
                let mut lines = Vec::new();
                if detected.services.is_empty() {
                    lines.push("No container works or writes in this workspace.".into());
                }
                for s in &detected.services {
                    let state = if s.running { "running" } else { "stopped" };
                    let from = s.compose.as_ref().map_or_else(
                        || "no compose file".into(),
                        |c| format!("{} → {}", c.file, c.service),
                    );
                    let kind = if s.data { "data" } else { "cache" };
                    lines.push(format!(
                        "{}  {} ({}, {state}; {from})  suggested: {kind}",
                        s.name, s.image, s.container
                    ));
                    if let Some(pg) = &s.postgres {
                        lines.push(format!(
                            "  PostgreSQL as {}: {}",
                            pg.username,
                            if pg.databases.is_empty() {
                                "databases read once it runs".into()
                            } else {
                                pg.databases.join(", ")
                            }
                        ));
                    }
                    if let Some(problem) = &s.problem {
                        lines.push(format!("  ! {problem}"));
                    }
                }
                if !detected.services.is_empty() {
                    lines.push(
                        "Declare them with `chauffeur services <workspace> save --data <name> --cache <name>`."
                            .into(),
                    );
                }
                if detected.previews.is_empty() {
                    lines.push("No port of the workspace answers HTTP.".into());
                } else {
                    lines.push("Web previews:".into());
                }
                for p in &detected.previews {
                    let state = if p.listening {
                        "answers HTTP"
                    } else {
                        "nothing answers"
                    };
                    let declared = if p.declared { ", declared" } else { "" };
                    lines.push(format!("  {}  port {} ({state}{declared})", p.name, p.port));
                }
                if detected.previews.iter().any(|p| !p.declared) {
                    lines.push(
                        "Declare them with `chauffeur services <workspace> save --preview <name>=<port>`."
                            .into(),
                    );
                }
                lines.join("\n")
            });
        }
        Some(Action::Save {
            data,
            cache,
            preview,
            no_preview,
        }) => {
            let current: DevServices = client
                .call(ClientCall::ServicesGet(WorkspaceParams {
                    workspace: workspace.clone(),
                }))
                .await?;
            let services = data
                .into_iter()
                .map(|name| DevServiceChoice { name, data: true })
                .chain(
                    cache
                        .into_iter()
                        .map(|name| DevServiceChoice { name, data: false }),
                )
                .collect();
            let saved: DevServices = client
                .call(ClientCall::ServicesSave(DevServicesSaveParams {
                    workspace,
                    services,
                    revision: current.revision,
                    previews: (no_preview || !preview.is_empty()).then_some(preview),
                }))
                .await?;
            super::print_or(json, &saved, || shown(&saved));
        }
    }
    Ok(())
}

fn shown(declared: &DevServices) -> String {
    let mut lines = vec![services_shown(declared)];
    for p in &declared.previews {
        let port = p
            .port
            .map_or_else(|| "no such port here".into(), |port| format!("port {port}"));
        lines.push(format!("preview {}  {port}", p.name));
    }
    lines.join("\n")
}

fn services_shown(declared: &DevServices) -> String {
    if declared.services.is_empty() {
        return "No development service is declared; `chauffeur services <workspace> detect` lists the candidates.".into();
    }
    declared
        .services
        .iter()
        .map(|s| {
            let what = if s.data {
                format!("data: {}", s.databases.join(", "))
            } else {
                "cache".into()
            };
            let from = s
                .compose
                .as_ref()
                .map_or_else(String::new, |c| format!(" ({} → {})", c.file, c.service));
            format!("{}{from}  {what}", s.name)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
