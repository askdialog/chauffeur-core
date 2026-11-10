//! `chauffeur worker`: GCP workers the daemon creates, installs Chauffeur on and forwards the
//! socket of (docs/cloud-workers.md). Creating one shows its estimated hourly cost and asks first;
//! the daemon creates and deletes in the background, this command only follows.
use std::time::Duration;

use anyhow::bail;
use chauffeur_protocol::client::{
    ClientCall, EmptyParams, SubscribeParams, SubscribeResult, UsageResult,
};
use chauffeur_protocol::events::{Event, RegistryEvent};
use chauffeur_protocol::model::{AccountStatus, AgentKind};
use chauffeur_protocol::sidecar::AgentLoginResult;
use chauffeur_protocol::worker::{
    WorkerCreateParams, WorkerInfo, WorkerLoginCodeParams, WorkerLoginParams, WorkerParams,
    WorkerState, WorkersResult,
};
use clap::Subcommand;

use crate::client::Client;

#[derive(Subcommand)]
pub enum Action {
    /// The workers, and what a new one costs.
    List,
    /// Create a VM in GCP with your gcloud, install Chauffeur there and declare it as a transfer
    /// machine.
    Create {
        /// Lowercase letters, digits and dashes; transfers name the worker by it.
        name: String,
        /// e2-standard-2, e2-standard-4 (the default) or e2-standard-8.
        #[arg(long)]
        machine_type: Option<String>,
        /// Create without asking, its cost and the limit of workers included.
        #[arg(long)]
        yes: bool,
        /// Return once started instead of following the creation.
        #[arg(long)]
        no_wait: bool,
    },
    /// Delete a worker's VM and its disk.
    Delete {
        name: String,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        no_wait: bool,
    },
    /// Sign a worker's Claude Code or Codex in from here: open the page this prints (the browser
    /// opens it when this runs in a terminal), paste back the code Claude Code's page shows, and
    /// wait. Without --agent, every agent signed out there. A transfer refused for these logins
    /// resumes once none is missing.
    Login {
        name: String,
        #[arg(long, value_enum)]
        agent: Option<crate::Agent>,
    },
    /// Open a shell on a worker, or run a command there.
    Ssh {
        name: String,
        #[arg(last = true)]
        command: Vec<String>,
    },
}

pub async fn run(action: Action, client: &mut Client, json: bool) -> anyhow::Result<()> {
    let result: WorkersResult = client.call(ClientCall::WorkerList(EmptyParams {})).await?;
    match action {
        Action::List => crate::print_or(json, &result, || list(&result)),
        Action::Create {
            name,
            machine_type,
            yes,
            no_wait,
        } => {
            // A failed or stopped worker is created again with its own machine type, on its VM.
            let failed = result.workers.iter().find(|w| {
                w.name == name && matches!(w.state, WorkerState::Failed | WorkerState::Stopped)
            });
            let wanted = match (failed, machine_type) {
                (Some(worker), Some(asked)) if asked != worker.machine_type => bail!(
                    "{name} was created as {}; delete it to create it as {asked}",
                    worker.machine_type
                ),
                (Some(worker), _) => worker.machine_type.clone(),
                (None, asked) => asked.unwrap_or_else(|| result.default_machine_type.clone()),
            };
            let Some(offer) = result.machine_types.iter().find(|m| m.name == wanted) else {
                let known: Vec<&str> = result
                    .machine_types
                    .iter()
                    .map(|m| m.name.as_str())
                    .collect();
                bail!("Unknown machine type {wanted}: choose {}", known.join(", "));
            };
            let beyond_limit = failed.is_none() && result.workers.len() as u32 >= result.limit;
            eprintln!(
                "{name}: {} ({} vCPU, {} GB), {} GB disk, in {} ({}): about ${:.2} an hour while it exists \
                 (Paris prices of {}; network and model use not included)",
                offer.name,
                offer.vcpus,
                offer.memory_gb,
                result.disk_gb,
                result.zone,
                result.project,
                offer.hourly_usd,
                result.prices_as_of
            );
            if beyond_limit {
                eprintln!(
                    "{} workers already exist; up to {} are created without asking again.",
                    result.workers.len(),
                    result.limit
                );
            }
            if !yes && !crate::confirm(&format!("Create the worker {name}?"))? {
                bail!("nothing created");
            }
            let started: WorkerInfo = client
                .call(ClientCall::WorkerCreate(WorkerCreateParams {
                    name: name.clone(),
                    machine_type: Some(wanted),
                    beyond_limit,
                }))
                .await?;
            if no_wait {
                crate::print_or(json, &started, || {
                    format!("{name} is being created; `chauffeur worker list` follows it")
                });
                return Ok(());
            }
            let ready = follow(client, &name, started.log.len().saturating_sub(1)).await?;
            match ready {
                Some(worker) if worker.state == WorkerState::Ready => {
                    crate::print_or(json, &worker, || {
                        format!(
                            "{name} is ready: `chauffeur cloud transfer <workspace> --to {name}`. \
                             Sign its agents in once with `chauffeur worker login {name}`."
                        )
                    })
                }
                Some(worker) => bail!(
                    "{}",
                    worker.error.unwrap_or_else(|| "the creation failed".into())
                ),
                None => bail!("the daemon lost track of {name}"),
            }
        }
        Action::Delete { name, yes, no_wait } => {
            let Some(worker) = result.workers.iter().find(|w| w.name == name) else {
                bail!("No worker named {name}");
            };
            if !yes
                && !crate::confirm(&format!(
                    "Delete the worker {name} (the VM {} and its disk)?",
                    worker.instance
                ))?
            {
                bail!("nothing deleted");
            }
            let deleting: WorkerInfo = client
                .call(ClientCall::WorkerDelete(WorkerParams {
                    name: name.clone(),
                }))
                .await?;
            if no_wait {
                crate::print_or(json, &deleting, || {
                    format!("{name} is being deleted; `chauffeur worker list` follows it")
                });
                return Ok(());
            }
            let from = deleting.log.len().saturating_sub(1);
            if let Some(worker) = follow(client, &name, from).await? {
                bail!(
                    "{}",
                    worker.error.unwrap_or_else(|| "the deletion failed".into())
                );
            }
            crate::print_or(json, &serde_json::json!({ "deleted": name }), || {
                format!("{name} is deleted")
            });
        }
        Action::Login { name, agent } => {
            if !result.workers.iter().any(|w| w.name == name) {
                bail!("No worker named {name}");
            }
            login(client, &name, agent.map(AgentKind::from), json).await?;
        }
        Action::Ssh { name, command } => {
            let Some(worker) = result.workers.iter().find(|w| w.name == name) else {
                bail!("No worker named {name}");
            };
            use std::os::unix::process::CommandExt;
            let mut gcloud = std::process::Command::new(
                std::env::var_os("CHAUFFEUR_GCLOUD").unwrap_or_else(|| "gcloud".into()),
            );
            gcloud.args([
                "compute",
                "ssh",
                &worker.instance,
                "--project",
                &worker.project,
                "--zone",
                &worker.zone,
            ]);
            if !command.is_empty() {
                gcloud.arg("--").args(&command);
            }
            bail!("could not run gcloud: {}", gcloud.exec());
        }
    }
    Ok(())
}

fn agent_name(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Claude => "Claude Code",
        AgentKind::Codex => "Codex",
    }
}

/// Signs `agent` in on the worker, or each agent signed out there, one after the other.
async fn login(
    client: &mut Client,
    name: &str,
    agent: Option<AgentKind>,
    json: bool,
) -> anyhow::Result<()> {
    let _: SubscribeResult = client
        .call(ClientCall::EventsSubscribe(SubscribeParams {
            registry: true,
            ..Default::default()
        }))
        .await?;
    let agents = match agent {
        Some(agent) => vec![agent],
        None => {
            let usage: UsageResult = client
                .call(ClientCall::WorkerUsage(WorkerParams { name: name.into() }))
                .await?;
            let out: Vec<AgentKind> = usage
                .agents
                .iter()
                .filter(|u| u.account == AccountStatus::SignedOut)
                .map(|u| u.agent)
                .collect();
            if out.is_empty() {
                crate::print_or(json, &serde_json::json!({ "signed_out": [] }), || {
                    format!("No agent is signed out on {name}")
                });
                return Ok(());
            }
            out
        }
    };
    for agent in agents {
        sign_in(client, name, agent, json).await?;
    }
    Ok(())
}

async fn sign_in(
    client: &mut Client,
    name: &str,
    agent: AgentKind,
    json: bool,
) -> anyhow::Result<()> {
    use std::io::IsTerminal;
    let what = agent_name(agent);
    let started: AgentLoginResult = client
        .call(ClientCall::WorkerLogin(WorkerLoginParams {
            name: name.into(),
            agent,
        }))
        .await?;
    if json {
        println!("{}", serde_json::to_string(&started)?);
    } else {
        println!("Sign {what} in on {name}: open\n{}", started.url);
        if let Some(code) = &started.user_code {
            println!("and enter the code {code}");
            println!(
                "(If OpenAI refuses device code sign-in, turn it on in ChatGPT → Settings → Security, then sign in again.)"
            );
        }
    }
    if std::io::stdout().is_terminal() {
        open_in_browser(&started.url);
    }
    if started.asks_code {
        if !json {
            eprint!("Paste the code the page shows: ");
        }
        let line = tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).map(|_| line)
        })
        .await??;
        if line.trim().is_empty() {
            let _: serde_json::Value = client
                .call(ClientCall::WorkerLoginCancel(WorkerLoginParams {
                    name: name.into(),
                    agent,
                }))
                .await?;
            bail!("No code given: the login of {what} on {name} is cancelled");
        }
        let _: serde_json::Value = client
            .call(ClientCall::WorkerLoginCode(WorkerLoginCodeParams {
                name: name.into(),
                agent,
                code: line.trim().into(),
            }))
            .await?;
    }
    let wait = async {
        loop {
            let notification = client.next_event().await?;
            if let Event::Registry {
                event:
                    RegistryEvent::LoginEnded {
                        agent: ended,
                        worker: Some(worker),
                        success,
                        error,
                    },
            } = notification.envelope.event
                && ended == agent
                && worker == name
            {
                return Ok::<_, anyhow::Error>((success, error));
            }
        }
    };
    let (success, error) = tokio::time::timeout(Duration::from_secs(11 * 60), wait)
        .await
        .map_err(|_| anyhow::anyhow!("the login did not complete in 10 minutes"))??;
    if !success {
        bail!(
            "{what} is not signed in on {name}: {}",
            error.as_deref().unwrap_or("the login did not complete")
        );
    }
    if !json {
        println!("{what} is signed in on {name}.");
    }
    Ok(())
}

fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn list(result: &WorkersResult) -> String {
    let mut lines: Vec<String> = result
        .workers
        .iter()
        .map(|w| {
            let state = match w.state {
                WorkerState::Ready if w.connected => "ready".to_owned(),
                WorkerState::Ready => match &w.error {
                    Some(why) => format!("ready, reconnecting: {why}"),
                    None => "ready, reconnecting".to_owned(),
                },
                WorkerState::Creating => "creating".to_owned(),
                WorkerState::Installing => "installing".to_owned(),
                WorkerState::Deleting => "deleting".to_owned(),
                WorkerState::Stopping => "stopping".to_owned(),
                WorkerState::Stopped => "stopped: only its disk costs".to_owned(),
                WorkerState::Failed => format!("failed: {}", w.error.as_deref().unwrap_or("")),
            };
            format!(
                "{}  {}  ${:.2}/h  {}",
                w.name, w.machine_type, w.hourly_usd, state
            )
        })
        .collect();
    if lines.is_empty() {
        lines.push("No worker.".into());
    }
    let offers: Vec<String> = result
        .machine_types
        .iter()
        .map(|m| format!("{} ${:.2}/h", m.name, m.hourly_usd))
        .collect();
    lines.push(format!(
        "A new one, in {} ({}): {} (Paris prices of {}).",
        result.zone,
        result.project,
        offers.join(", "),
        result.prices_as_of
    ));
    lines.join("\n")
}

/// Prints each step of a worker's creation or deletion, from its log line `from`, until it
/// settles; returns it then, or none once it is gone.
async fn follow(
    client: &mut Client,
    name: &str,
    from: usize,
) -> anyhow::Result<Option<WorkerInfo>> {
    let mut printed = from;
    loop {
        let result: WorkersResult = client.call(ClientCall::WorkerList(EmptyParams {})).await?;
        let Some(worker) = result.workers.into_iter().find(|w| w.name == name) else {
            return Ok(None);
        };
        for line in &worker.log[printed.min(worker.log.len())..] {
            eprintln!("· {line}");
        }
        printed = worker.log.len();
        match worker.state {
            WorkerState::Ready | WorkerState::Failed => return Ok(Some(worker)),
            _ => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
}
