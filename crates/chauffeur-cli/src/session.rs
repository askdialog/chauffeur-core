//! `chauffeur session`: the logins a workspace's repos and the user declare, checked and renewed
//! (docs/adr/declared-sessions.md). Inside a workspace, it is that workspace by default.
use crate::client::Client;
use chauffeur_protocol::client::{
    ClientCall, LoginSessionInfo, LoginSessionLogin, LoginSessionParams, LoginSessionState,
    LoginSessionsResult, WorkspaceParams,
};
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Action {
    /// List the workspace's sessions, checked again when their last check is a minute old.
    Ls {
        #[arg(long, env = "CHAUFFEUR_WORKSPACE_ID")]
        workspace: String,
    },
    /// Run a session's check now; exits 1 when it does not hold.
    Check {
        name: String,
        #[arg(long, env = "CHAUFFEUR_WORKSPACE_ID")]
        workspace: String,
    },
    /// Start a session's login, and print the link it gives to open. Refused to agents.
    Login {
        name: String,
        #[arg(long, env = "CHAUFFEUR_WORKSPACE_ID")]
        workspace: String,
    },
}

fn line(session: &LoginSessionInfo) -> String {
    let state = match session.state {
        LoginSessionState::Holds => match &session.expires_at {
            Some(end) => format!("holds until {end}"),
            None => "holds".to_string(),
        },
        LoginSessionState::Expired => "expired".to_string(),
        LoginSessionState::Unchecked => "not checked".to_string(),
    };
    let login = if session.logging_in {
        ", login running"
    } else {
        ""
    };
    let from = session.repo.as_deref().unwrap_or("your settings");
    format!(
        "{}  {}  {state}{login}  ({from})",
        session.name, session.label
    )
}

fn print_or<T: serde::Serialize>(json: bool, value: &T, text: impl FnOnce() -> String) {
    match json {
        true => println!(
            "{}",
            serde_json::to_string_pretty(value).expect("results serialize")
        ),
        false => println!("{}", crate::render::sanitize_terminal(&text())),
    }
}

pub async fn run(client: &mut Client, action: Action, json: bool) -> anyhow::Result<()> {
    match action {
        Action::Ls { workspace } => {
            let result: LoginSessionsResult = client
                .call(ClientCall::SessionList(WorkspaceParams { workspace }))
                .await?;
            print_or(json, &result, || match result.sessions.is_empty() {
                true => "No session declared.".to_string(),
                false => result
                    .sessions
                    .iter()
                    .map(line)
                    .collect::<Vec<_>>()
                    .join("\n"),
            });
        }
        Action::Check { name, workspace } => {
            let session: LoginSessionInfo = client
                .call(ClientCall::SessionCheck(LoginSessionParams {
                    workspace,
                    name,
                }))
                .await?;
            print_or(json, &session, || line(&session));
            if session.state != LoginSessionState::Holds {
                std::process::exit(1);
            }
        }
        Action::Login { name, workspace } => {
            let result: LoginSessionLogin = client
                .call(ClientCall::SessionLogin(LoginSessionParams {
                    workspace,
                    name,
                }))
                .await?;
            print_or(json, &result, || {
                let mut text = line(&result.session);
                if !result.output.is_empty() {
                    text.push_str("\n\n");
                    text.push_str(&result.output);
                }
                text
            });
        }
    }
    Ok(())
}
