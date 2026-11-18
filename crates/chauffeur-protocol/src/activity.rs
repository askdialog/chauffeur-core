//! The user's activity over a time window, every workspace included (`activity.list`,
//! `chauffeur activity`): what an agent reads to recap the user's work ("what did I do since
//! yesterday morning"). Pull requests and tickets' states stay with `gh` and Linear.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::{AgentKind, TicketRef};

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ActivityParams {
    /// RFC 3339: the start of the window.
    pub since: String,
    /// RFC 3339: its end, excluded; now when absent.
    #[serde(default)]
    pub until: Option<String>,
}

/// The workspaces the user wrote in during the window, by their first message in it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ActivityResult {
    pub since: String,
    pub until: String,
    pub workspaces: Vec<ActivityWorkspace>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ActivityWorkspace {
    pub id: String,
    /// The name shown: its title, else its slug.
    pub name: String,
    pub branch: String,
    pub path: String,
    pub repos: Vec<String>,
    pub tickets: Vec<TicketRef>,
    /// RFC 3339, when it was archived.
    pub archived_at: Option<String>,
    pub chats: Vec<ActivityChat>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ActivityChat {
    pub id: String,
    pub title: Option<String>,
    pub agent: AgentKind,
    /// The user's messages in the window, oldest first, each cut to 800 characters, pasted text
    /// marked rather than repeated.
    pub messages: Vec<ActivityMessage>,
    /// The agent's last text in the window, cut to 2,000 characters: where the chat stood.
    pub last_answer: Option<ActivityMessage>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ActivityMessage {
    /// RFC 3339.
    pub at: String,
    pub text: String,
}
