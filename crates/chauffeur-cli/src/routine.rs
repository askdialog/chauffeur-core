//! `chauffeur routine`: prompts the daemon runs by itself at set times (docs/adr/routines.md).
//! Inside a routine's workspace, the routine is that workspace's by default.
use crate::client::Client;
use chauffeur_protocol::client::{ClientCall, EmptyParams};
use chauffeur_protocol::routines::*;
use clap::Subcommand;
use std::io::Read;

#[derive(Subcommand)]
pub enum Action {
    /// List the routines, with their state and next run.
    Ls,
    /// Make a draft routine from a chat: a workspace of its own, whose refining chat drafts the
    /// routine's file from the conversation.
    New {
        /// The chat (id, or a workspace meaning its first chat).
        #[arg(long = "from")]
        chat: String,
        /// Its name; by default the chat's title.
        #[arg(long)]
        name: Option<String>,
    },
    /// Show a routine and its current file.
    Show {
        /// Routine id or name, or its workspace.
        #[arg(env = "CHAUFFEUR_WORKSPACE_ID")]
        routine: String,
    },
    /// Save a new version of a routine's file, read from a file or stdin (`-`).
    Edit {
        #[arg(env = "CHAUFFEUR_WORKSPACE_ID")]
        routine: String,
        #[arg(long)]
        file: String,
    },
    /// Run a routine now, as a new chat of its workspace.
    Run {
        #[arg(env = "CHAUFFEUR_WORKSPACE_ID")]
        routine: String,
    },
    /// Make a routine run on its schedule.
    Activate {
        #[arg(env = "CHAUFFEUR_WORKSPACE_ID")]
        routine: String,
    },
    /// Stop a routine running on its schedule; Run still works.
    Pause {
        #[arg(env = "CHAUFFEUR_WORKSPACE_ID")]
        routine: String,
    },
    /// Delete a routine; its workspace and its runs' chats stay.
    Rm { routine: String },
    /// A routine's versions and runs, newest first, with each run's last message.
    History {
        #[arg(env = "CHAUFFEUR_WORKSPACE_ID")]
        routine: String,
    },
    /// Find routines: read your own messages of the last days, every workspace included, and
    /// propose routines from the requests that come back or name a time window. Runs in the
    /// background; `chauffeur routine suggestions` shows what it proposes.
    Find {
        /// How many days back to read.
        #[arg(long, default_value_t = 14)]
        days: u32,
        /// The agent that reads them.
        #[arg(long, value_enum, default_value = "claude")]
        agent: crate::Agent,
        /// Its model; its default otherwise.
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        effort: Option<String>,
    },
    /// The routines Find routines proposes, waiting for you, and whether it runs.
    Suggestions,
    /// Act on a suggestion: make it a draft routine, keep it for later, or ignore it for good.
    Suggestion {
        id: String,
        #[arg(value_enum)]
        action: SuggestionAction,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum SuggestionAction {
    Create,
    Later,
    Ignore,
}

fn read_file(path: &str) -> anyhow::Result<String> {
    if path == "-" {
        let mut text = String::new();
        std::io::stdin()
            .take(1024 * 1024)
            .read_to_string(&mut text)?;
        return Ok(text);
    }
    std::fs::read_to_string(path).map_err(|error| anyhow::anyhow!("could not read {path}: {error}"))
}

fn state(routine: &RoutineInfo) -> String {
    let mut text = routine.state.as_str().to_string();
    if routine.pending {
        text.push_str(", a missed run is due");
    }
    if let Some(next) = &routine.next_run_at {
        text.push_str(&format!(", next run {}", local(next)));
    }
    text
}

fn local(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%a %Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| rfc3339.to_string())
}

fn line(routine: &RoutineInfo) -> String {
    format!(
        "{}  {}  v{}  ({})",
        routine.spec.name,
        routine.id,
        routine.version,
        state(routine)
    )
}

fn details(routine: &RoutineInfo) -> String {
    let mut out = line(routine);
    out.push_str(&format!("\n  workspace {}", routine.workspace_id));
    if let Some(run) = &routine.last_run {
        out.push_str(&format!(
            "\n  last run {} ({}), chat {}",
            local(&run.started_at),
            run.status.map(|s| s.as_str()).unwrap_or("deleted"),
            run.chat_id
        ));
    }
    if let Some(error) = &routine.error {
        out.push_str(&format!("\n  last scheduled run failed: {error}"));
    }
    out.push_str(&format!("\n\n{}", routine.file.trim_end()));
    out
}

fn run_line(run: &RoutineRun) -> String {
    let how = match run.trigger {
        RoutineTrigger::Manual => "by hand",
        RoutineTrigger::Schedule => "on schedule",
    };
    format!(
        "run {} {how}, v{}, {}{}  chat {}",
        local(&run.started_at),
        run.version,
        run.status.map(|s| s.as_str()).unwrap_or("deleted"),
        if run.closed { ", closed" } else { "" },
        run.chat_id
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

fn history_text(result: &RoutineHistory) -> String {
    let mut out = Vec::new();
    for run in &result.runs {
        out.push(run_line(run));
        if let Some(text) = &run.last_text {
            out.extend(text.lines().map(|l| format!("    {l}")));
        }
    }
    if result.runs.is_empty() {
        out.push("no runs yet".into());
    }
    out.push(String::new());
    for version in &result.versions {
        out.push(format!(
            "version {} of {}",
            version.version,
            local(&version.created_at)
        ));
    }
    out.join("\n")
}

fn find_text(find: &RoutineFindInfo) -> String {
    let mut out = Vec::new();
    if let Some(run) = &find.running {
        out.push(format!(
            "running since {}: {} reads {} of your messages of the last {} days",
            local(&run.started_at),
            run.agent.as_str(),
            run.messages,
            run.days
        ));
    } else if let Some(error) = &find.last_error {
        out.push(format!("the last analysis failed: {error}"));
    }
    for suggestion in &find.suggestions {
        out.push(format!(
            "{}  {}  [{}]",
            suggestion.spec.name,
            suggestion.id,
            suggestion.state.as_str()
        ));
        if !suggestion.why.is_empty() {
            out.push(format!("  {}", suggestion.why));
        }
        for request in &suggestion.requests {
            out.push(format!(
                "  - {} in {}: {}",
                local(&request.at),
                request.workspace,
                request.text.lines().next().unwrap_or_default()
            ));
        }
    }
    if find.suggestions.is_empty() && find.running.is_none() {
        out.push("no suggestion waiting — `chauffeur routine find` looks for some".into());
    }
    out.join("\n")
}

pub async fn run(client: &mut Client, action: Action, json: bool) -> anyhow::Result<()> {
    let call = match action {
        Action::Find {
            days,
            agent,
            model,
            effort,
        } => {
            let result: RoutineFindInfo = client
                .call(ClientCall::RoutineFind(RoutineFindParams {
                    days,
                    agent: agent.into(),
                    model,
                    effort,
                }))
                .await?;
            print_or(json, &result, || find_text(&result));
            return Ok(());
        }
        Action::Suggestions => {
            let result: RoutineFindInfo = client
                .call(ClientCall::RoutineSuggestions(EmptyParams {}))
                .await?;
            print_or(json, &result, || find_text(&result));
            return Ok(());
        }
        Action::Suggestion {
            id,
            action: SuggestionAction::Create,
        } => ClientCall::RoutineSuggestionCreate(RoutineSuggestionParams { suggestion: id }),
        Action::Suggestion { id, action } => {
            let state = match action {
                SuggestionAction::Later => RoutineSuggestionState::Later,
                _ => RoutineSuggestionState::Ignored,
            };
            let result: RoutineFindInfo = client
                .call(ClientCall::RoutineSuggestionSet(
                    RoutineSuggestionSetParams {
                        suggestion: id,
                        state,
                    },
                ))
                .await?;
            print_or(json, &result, || find_text(&result));
            return Ok(());
        }
        Action::Ls => {
            let result: RoutinesResult =
                client.call(ClientCall::RoutineList(EmptyParams {})).await?;
            print_or(json, &result, || match result.routines.is_empty() {
                true => {
                    "no routines — make one from a chat with `chauffeur routine new --from <chat>`"
                        .into()
                }
                false => result
                    .routines
                    .iter()
                    .map(line)
                    .collect::<Vec<_>>()
                    .join("\n"),
            });
            return Ok(());
        }
        Action::History { routine } => {
            let result: RoutineHistory = client
                .call(ClientCall::RoutineHistory(RoutineParams { routine }))
                .await?;
            print_or(json, &result, || history_text(&result));
            return Ok(());
        }
        Action::Run { routine } => {
            let result: RoutineRun = client
                .call(ClientCall::RoutineRun(RoutineParams { routine }))
                .await?;
            print_or(json, &result, || run_line(&result));
            return Ok(());
        }
        Action::Rm { routine } => {
            let result: RoutinesResult = client
                .call(ClientCall::RoutineDelete(RoutineParams { routine }))
                .await?;
            print_or(json, &result, || "deleted; its workspace stays".into());
            return Ok(());
        }
        Action::Show { routine } => {
            let result: RoutineInfo = client
                .call(ClientCall::RoutineGet(RoutineParams { routine }))
                .await?;
            print_or(json, &result, || details(&result));
            return Ok(());
        }
        Action::New { chat, name } => ClientCall::RoutineCreate(RoutineCreateParams { chat, name }),
        Action::Edit { routine, file } => ClientCall::RoutineEdit(RoutineEditParams {
            routine,
            file: read_file(&file)?,
        }),
        Action::Activate { routine } => ClientCall::RoutineSetActive(RoutineSetActiveParams {
            routine,
            active: true,
        }),
        Action::Pause { routine } => ClientCall::RoutineSetActive(RoutineSetActiveParams {
            routine,
            active: false,
        }),
    };
    let result: RoutineInfo = client.call(call).await?;
    print_or(json, &result, || line(&result));
    Ok(())
}
