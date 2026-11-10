//! SQL and connection management. JSON inputs use stdin so passwords never enter process arguments.
use crate::client::Client;
use chauffeur_protocol::{client::ClientCall, database::*};
use clap::Subcommand;
use serde_json::Value;
use std::io::Read;

#[derive(Subcommand)]
pub enum Action {
    /// List global connections and those of this workspace (all when omitted).
    List {
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Save DatabaseSaveParams JSON from stdin. Omit id to create; password null keeps it.
    Save,
    /// Test DatabaseSaveParams JSON from stdin without saving it.
    Test,
    /// Execute one SQL statement read from stdin.
    Query {
        connection: String,
        workspace: String,
        #[arg(long)]
        request: Option<String>,
    },
    /// List tables and columns.
    Schema {
        connection: String,
        workspace: String,
    },
    /// Remove saved credentials and history, leaving the database untouched.
    Delete {
        connection: String,
        workspace: String,
    },
    /// Show the last 100 SQL statements.
    History {
        connection: String,
        workspace: String,
    },
    /// Apply DatabaseApplyParams JSON from stdin atomically.
    Apply,
    /// Cancel a running request by the id supplied to query or apply.
    Cancel { request: String },
}

fn stdin() -> anyhow::Result<String> {
    let mut text = String::new();
    std::io::stdin()
        .take(8 * 1024 * 1024 + 1)
        .read_to_string(&mut text)?;
    anyhow::ensure!(text.len() <= 8 * 1024 * 1024, "Database input exceeds 8 MB");
    Ok(text)
}

pub async fn run(client: &mut Client, action: Action) -> anyhow::Result<()> {
    let call = match action {
        Action::List { workspace } => ClientCall::DatabaseList(DatabaseListParams { workspace }),
        Action::Save => ClientCall::DatabaseSave(serde_json::from_str(&stdin()?)?),
        Action::Test => ClientCall::DatabaseTest(serde_json::from_str(&stdin()?)?),
        Action::Query {
            connection,
            workspace,
            request,
        } => ClientCall::DatabaseQuery(DatabaseQueryParams {
            connection,
            workspace,
            request: request.unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
            sql: stdin()?,
        }),
        Action::Schema {
            connection,
            workspace,
        } => ClientCall::DatabaseSchema(DatabaseParams {
            connection,
            workspace,
        }),
        Action::Delete {
            connection,
            workspace,
        } => ClientCall::DatabaseDelete(DatabaseParams {
            connection,
            workspace,
        }),
        Action::History {
            connection,
            workspace,
        } => ClientCall::DatabaseHistory(DatabaseParams {
            connection,
            workspace,
        }),
        Action::Apply => ClientCall::DatabaseApply(serde_json::from_str(&stdin()?)?),
        Action::Cancel { request } => ClientCall::DatabaseCancel(DatabaseCancelParams { request }),
    };
    let result: Value = client.call(call).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
