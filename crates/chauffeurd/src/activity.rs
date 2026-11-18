//! The user's activity over a time window, every workspace included, archived ones too
//! (`activity.list`, docs/adr/activity-for-recaps.md): the workspaces and chats the user wrote
//! in, their messages and where each chat stood. An agent asked "what did I do since yesterday"
//! reads it rather than guessing from folders and git logs.
//!
//! Only what the user typed counts, as for Find routines: not what an agent sent (delegated
//! chats, child workspaces, delegated answers), nor guests' suggestions, routines or reviews.

use std::collections::HashMap;

use chauffeur_protocol::activity::{
    ActivityChat, ActivityMessage, ActivityParams, ActivityResult, ActivityWorkspace,
};
use chauffeur_protocol::model::WorktreeOrigin;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

use crate::find_routines::{clean, cut};
use crate::store::{EVENT_TYPE, Store, now};
use crate::{Error, Result};

const MESSAGE_CHARS: usize = 800;
const ANSWER_CHARS: usize = 2_000;

/// A user message as the event log keeps it.
struct Written {
    at: String,
    workspace_id: String,
    chat_id: String,
    text: String,
}

impl Store {
    /// The user's own messages in `[since, until)`, oldest first.
    fn written_between(&self, since: &str, until: &str) -> Result<Vec<Written>> {
        self.read(|c| {
            c.prepare(
                "SELECT e.ts, e.workspace_id, e.chat_id, json_extract(e.payload, '$.event.event.text')
                 FROM events e
                 JOIN chats c ON c.id = e.chat_id
                 JOIN workspaces w ON w.id = e.workspace_id
                 WHERE e.ts >= ?1 AND e.ts < ?2 AND e.registry = 0
                   AND json_extract(e.payload, '$.event.scope') = 'chat'
                   AND json_extract(e.payload, '$.event.event.type') = 'user_message'
                   AND json_extract(e.payload, '$.event.event.delegated_answer') IS NULL
                   AND json_extract(e.payload, '$.event.event.suggested_by') IS NULL
                   AND c.parent_chat IS NULL AND w.parent_id IS NULL
                   AND NOT EXISTS (SELECT 1 FROM daily_reviews r WHERE r.workspace_id = w.id)
                   AND NOT EXISTS (SELECT 1 FROM weekly_reviews r WHERE r.workspace_id = w.id)
                   AND NOT EXISTS (SELECT 1 FROM internal_workspaces r WHERE r.workspace_id = w.id)
                   AND NOT EXISTS (SELECT 1 FROM routines r WHERE r.workspace_id = w.id)
                 ORDER BY e.seq",
            )?
            .query_map(params![since, until], |r| {
                Ok(Written {
                    at: r.get(0)?,
                    workspace_id: r.get(1)?,
                    chat_id: r.get(2)?,
                    text: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                })
            })?
            .collect()
        })
    }

    /// The agent's last text of a chat in `[since, until)`, a subagent's left out.
    fn last_answer(
        &self,
        chat: &str,
        since: &str,
        until: &str,
    ) -> Result<Option<(String, String)>> {
        self.read(|c| {
            c.query_row(
                &format!(
                    "SELECT ts, json_extract(payload, '$.event.event.text') FROM events
                     WHERE chat_id = ?1 AND {EVENT_TYPE} = 'assistant_text'
                       AND ts >= ?2 AND ts < ?3
                       AND json_extract(payload, '$.event.event.parent_tool_use_id') IS NULL
                     ORDER BY seq DESC LIMIT 1"
                ),
                params![chat, since, until],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    ))
                },
            )
            .optional()
        })
    }
}

/// An instant as the event log writes it (UTC, milliseconds), so that it compares as a string.
fn instant(name: &str, value: &str) -> Result<String> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| {
        Error::BadRequest(format!(
            "{name} must be an RFC 3339 instant such as 2026-11-10T09:00:00+01:00, not {value:?}"
        ))
    })?;
    Ok(parsed
        .with_timezone(&Utc)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

pub fn list(store: &Store, params: ActivityParams) -> Result<ActivityResult> {
    let since = instant("since", &params.since)?;
    let until = match &params.until {
        Some(until) => instant("until", until)?,
        None => now(),
    };
    if since >= until {
        return Err(Error::BadRequest(format!(
            "since ({since}) must come before until ({until})"
        )));
    }
    let mut workspaces: Vec<ActivityWorkspace> = Vec::new();
    // Where each workspace and chat sits in `workspaces`, and those that are gone.
    let mut workspace_at: HashMap<String, Option<usize>> = HashMap::new();
    let mut chat_at: HashMap<String, Option<(usize, usize)>> = HashMap::new();
    for written in store.written_between(&since, &until)? {
        let w = match workspace_at.get(&written.workspace_id) {
            Some(at) => *at,
            None => {
                let at = match store.find_workspace(&written.workspace_id)? {
                    Some(row) => {
                        let mut repos: Vec<String> = store
                            .worktrees_of(&row.id)?
                            .into_iter()
                            .filter(|tree| tree.origin == WorktreeOrigin::Workspace)
                            .map(|tree| tree.repo)
                            .collect();
                        repos.sort();
                        repos.dedup();
                        workspaces.push(ActivityWorkspace {
                            name: row.title.clone().unwrap_or_else(|| row.slug.clone()),
                            id: row.id,
                            branch: row.branch,
                            path: row.path,
                            repos,
                            tickets: row.tickets,
                            archived_at: row.archived_at,
                            chats: Vec::new(),
                        });
                        Some(workspaces.len() - 1)
                    }
                    None => None,
                };
                workspace_at.insert(written.workspace_id.clone(), at);
                at
            }
        };
        let Some(w) = w else { continue };
        let at = match chat_at.get(&written.chat_id) {
            Some(at) => *at,
            None => {
                let at = match store.chat(&written.chat_id)? {
                    Some(chat) => {
                        let chats = &mut workspaces[w].chats;
                        chats.push(ActivityChat {
                            last_answer: store.last_answer(&chat.id, &since, &until)?.map(
                                |(at, text)| ActivityMessage {
                                    at,
                                    text: cut(text.trim(), ANSWER_CHARS),
                                },
                            ),
                            id: chat.id,
                            title: chat.title,
                            agent: chat.agent,
                            messages: Vec::new(),
                        });
                        Some((w, chats.len() - 1))
                    }
                    None => None,
                };
                chat_at.insert(written.chat_id.clone(), at);
                at
            }
        };
        let Some((w, c)) = at else { continue };
        workspaces[w].chats[c].messages.push(ActivityMessage {
            at: written.at,
            text: cut(&clean(&written.text), MESSAGE_CHARS),
        });
    }
    Ok(ActivityResult {
        since,
        until,
        workspaces,
    })
}
