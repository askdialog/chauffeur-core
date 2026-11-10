//! What the service keeps of workspace shares (story 53): the shares and their guests, and the
//! hosted copy their daemons push: the chats' list and stored events, the notes, the suggestions.
//! Stopping a share removes the copy at once; the share's row and guests stay, for the link to
//! say it was stopped.

use chauffeur_protocol::client::CommentAuthor;
use chauffeur_protocol::events::EventEnvelope;
use chauffeur_protocol::workspace_share::{
    Guest, GuestRole, ShareComment, SharedChat, SharedNote, Suggestion,
};
use rusqlite::{Connection, OptionalExtension, params};

use super::{Store, in_team};
use crate::Identity;

/// The chat a stored suggestion or comment is on.
#[derive(serde::Deserialize)]
struct ChatOf {
    chat: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceShareRow {
    pub id: String,
    pub owner: CommentAuthor,
    pub title: String,
    pub created_at: u64,
    pub stopped_at: Option<u64>,
    pub last_push_at: Option<u64>,
    /// Lowercase, in the order given.
    pub guests: Vec<Guest>,
    /// The team domain it is open to, as viewers unless named with another role: its owner's
    /// Google Workspace domain when they opened it to their team.
    pub team_domain: Option<String>,
}

impl WorkspaceShareRow {
    pub fn stopped(&self) -> bool {
        self.stopped_at.is_some()
    }

    /// What someone may do on the share while it lives; none when they may not look. The owner
    /// reads as a viewer: they send from the app.
    pub fn role_of(&self, who: &Identity) -> Option<GuestRole> {
        if self.stopped() {
            return None;
        }
        if self.owner.email.eq_ignore_ascii_case(&who.email) {
            return Some(GuestRole::Viewer);
        }
        if let Some(guest) = self
            .guests
            .iter()
            .find(|g| g.email.eq_ignore_ascii_case(&who.email))
        {
            return Some(guest.role);
        }
        in_team(self.team_domain.as_deref(), who).then_some(GuestRole::Viewer)
    }
}

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS workspace_shares (
    id TEXT PRIMARY KEY,
    owner_email TEXT NOT NULL,
    owner_name TEXT NOT NULL,
    title TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    stopped_at INTEGER,
    last_push_at INTEGER,
    team INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS workspace_guests (
    share_id TEXT NOT NULL REFERENCES workspace_shares(id),
    email TEXT NOT NULL,
    role TEXT NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (share_id, email)
);
CREATE TABLE IF NOT EXISTS workspace_chats (
    share_id TEXT PRIMARY KEY REFERENCES workspace_shares(id),
    json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS workspace_events (
    share_id TEXT NOT NULL REFERENCES workspace_shares(id),
    chat_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    json TEXT NOT NULL,
    UNIQUE (share_id, chat_id, seq)
);
CREATE TABLE IF NOT EXISTS workspace_notes (
    share_id TEXT NOT NULL REFERENCES workspace_shares(id),
    path TEXT NOT NULL,
    text TEXT NOT NULL,
    modified_ms INTEGER NOT NULL,
    PRIMARY KEY (share_id, path)
);
CREATE TABLE IF NOT EXISTS workspace_suggestions (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    share_id TEXT NOT NULL REFERENCES workspace_shares(id),
    id TEXT NOT NULL,
    json TEXT NOT NULL,
    UNIQUE (share_id, id)
);
CREATE TABLE IF NOT EXISTS workspace_comments (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    share_id TEXT NOT NULL REFERENCES workspace_shares(id),
    id TEXT NOT NULL,
    json TEXT NOT NULL,
    UNIQUE (share_id, id)
);
";

const SHARE_COLUMNS: &str =
    "id, owner_email, owner_name, title, created_at, stopped_at, last_push_at, team_domain";

fn role_name(role: GuestRole) -> &'static str {
    match role {
        GuestRole::Viewer => "viewer",
        GuestRole::Suggest => "suggest",
    }
}

fn role_of_name(name: &str) -> GuestRole {
    match name {
        "suggest" => GuestRole::Suggest,
        _ => GuestRole::Viewer,
    }
}

impl Store {
    pub fn create_workspace_share(
        &self,
        id: &str,
        owner: &CommentAuthor,
        title: &str,
        guests: &[Guest],
        team_domain: Option<&str>,
        now: u64,
    ) -> anyhow::Result<WorkspaceShareRow> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO workspace_shares (id, owner_email, owner_name, title, created_at, team, team_domain) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, owner.email.to_lowercase(), owner.name, title, now as i64, team_domain.is_some(), team_domain],
        )?;
        set_guests(&tx, id, guests)?;
        tx.commit()?;
        drop(conn);
        Ok(self.workspace_share(id)?.expect("just created"))
    }

    pub fn workspace_share(&self, id: &str) -> anyhow::Result<Option<WorkspaceShareRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                &format!("SELECT {SHARE_COLUMNS} FROM workspace_shares WHERE id = ?1"),
                params![id],
                |row| {
                    Ok(WorkspaceShareRow {
                        id: row.get(0)?,
                        owner: CommentAuthor {
                            email: row.get(1)?,
                            name: row.get(2)?,
                        },
                        title: row.get(3)?,
                        created_at: row.get::<_, i64>(4)? as u64,
                        stopped_at: row.get::<_, Option<i64>>(5)?.map(|v| v as u64),
                        last_push_at: row.get::<_, Option<i64>>(6)?.map(|v| v as u64),
                        guests: Vec::new(),
                        team_domain: row.get(7)?,
                    })
                },
            )
            .optional()?;
        let Some(mut share) = row else {
            return Ok(None);
        };
        share.guests = guests_of(&conn, id)?;
        Ok(Some(share))
    }

    /// Changes what is given: `team_domain` `Some(None)` closes the share to its team.
    pub fn update_workspace_share(
        &self,
        id: &str,
        title: Option<&str>,
        guests: Option<&[Guest]>,
        team_domain: Option<Option<&str>>,
    ) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        if let Some(title) = title {
            tx.execute(
                "UPDATE workspace_shares SET title = ?2 WHERE id = ?1",
                params![id, title],
            )?;
        }
        if let Some(team_domain) = team_domain {
            tx.execute(
                "UPDATE workspace_shares SET team = ?2, team_domain = ?3 WHERE id = ?1",
                params![id, team_domain.is_some(), team_domain],
            )?;
        }
        if let Some(guests) = guests {
            set_guests(&tx, id, guests)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Stops a share and removes its hosted copy: chats, events, notes, suggestions, comments.
    pub fn stop_workspace_share(&self, id: &str, now: u64) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE workspace_shares SET stopped_at = ?2 WHERE id = ?1 AND stopped_at IS NULL",
            params![id, now as i64],
        )?;
        for table in [
            "workspace_chats",
            "workspace_events",
            "workspace_notes",
            "workspace_suggestions",
            "workspace_comments",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE share_id = ?1"),
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn touch_workspace_push(&self, id: &str, now: u64) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE workspace_shares SET last_push_at = ?2 WHERE id = ?1",
            params![id, now as i64],
        )?;
        Ok(())
    }

    pub fn workspace_chats(&self, id: &str) -> anyhow::Result<Vec<SharedChat>> {
        let json: Option<String> = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT json FROM workspace_chats WHERE share_id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(json
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default())
    }

    /// The daemon's list takes the place of the service's; the events, suggestions and comments
    /// of chats it no longer lists go with them. Whether suggestions or comments went.
    pub fn set_workspace_chats(&self, id: &str, chats: &[SharedChat]) -> anyhow::Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO workspace_chats (share_id, json) VALUES (?1, ?2) \
             ON CONFLICT (share_id) DO UPDATE SET json = excluded.json",
            params![id, serde_json::to_string(chats)?],
        )?;
        let listed: Vec<&str> = chats.iter().map(|c| c.id.as_str()).collect();
        let stored: Vec<String> = tx
            .prepare("SELECT DISTINCT chat_id FROM workspace_events WHERE share_id = ?1")?
            .query_map(params![id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        for chat in stored.iter().filter(|c| !listed.contains(&c.as_str())) {
            tx.execute(
                "DELETE FROM workspace_events WHERE share_id = ?1 AND chat_id = ?2",
                params![id, chat],
            )?;
        }
        let mut dropped = 0;
        for table in ["workspace_suggestions", "workspace_comments"] {
            let unlisted: Vec<String> = tx
                .prepare(&format!("SELECT id, json FROM {table} WHERE share_id = ?1"))?
                .query_map(params![id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .filter_map(|row| row.ok())
                .filter(|(_, json)| {
                    serde_json::from_str::<ChatOf>(json)
                        .is_ok_and(|of| !listed.contains(&of.chat.as_str()))
                })
                .map(|(row_id, _)| row_id)
                .collect();
            for row_id in unlisted {
                dropped += tx.execute(
                    &format!("DELETE FROM {table} WHERE share_id = ?1 AND id = ?2"),
                    params![id, row_id],
                )?;
            }
        }
        tx.commit()?;
        Ok(dropped > 0)
    }

    /// A chat's stored events, in their order.
    pub fn chat_events(&self, id: &str, chat: &str) -> anyhow::Result<Vec<EventEnvelope>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT json FROM workspace_events WHERE share_id = ?1 AND chat_id = ?2 ORDER BY seq",
        )?;
        let events = statement
            .query_map(params![id, chat], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .filter_map(|json| serde_json::from_str(&json).ok())
            .collect();
        Ok(events)
    }

    /// A chat's whole history from the daemon, in place of what was kept.
    pub fn replace_chat_events(
        &self,
        id: &str,
        chat: &str,
        events: &[EventEnvelope],
    ) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM workspace_events WHERE share_id = ?1 AND chat_id = ?2",
            params![id, chat],
        )?;
        insert_events(&tx, id, chat, events)?;
        tx.commit()?;
        Ok(())
    }

    /// Keeps the events that carry a `seq`; an event kept already stays as it was. Returns how
    /// many were kept.
    pub fn add_chat_events(
        &self,
        id: &str,
        chat: &str,
        events: &[EventEnvelope],
    ) -> anyhow::Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let kept = insert_events(&tx, id, chat, events)?;
        tx.commit()?;
        Ok(kept)
    }

    /// The notes, by path.
    pub fn workspace_notes(&self, id: &str) -> anyhow::Result<Vec<SharedNote>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT path, text, modified_ms FROM workspace_notes WHERE share_id = ?1 ORDER BY path",
        )?;
        let notes = statement
            .query_map(params![id], |row| {
                Ok(SharedNote {
                    path: row.get(0)?,
                    text: row.get(1)?,
                    modified_ms: row.get::<_, i64>(2)? as u64,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(notes)
    }

    pub fn change_workspace_notes(
        &self,
        id: &str,
        changed: &[SharedNote],
        removed: &[String],
    ) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for note in changed {
            tx.execute(
                "INSERT INTO workspace_notes (share_id, path, text, modified_ms) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT (share_id, path) DO UPDATE SET text = excluded.text, modified_ms = excluded.modified_ms",
                params![id, note.path, note.text, note.modified_ms as i64],
            )?;
        }
        for path in removed {
            tx.execute(
                "DELETE FROM workspace_notes WHERE share_id = ?1 AND path = ?2",
                params![id, path],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The suggestions, in the order they were first kept.
    pub fn workspace_suggestions(&self, id: &str) -> anyhow::Result<Vec<Suggestion>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn
            .prepare("SELECT json FROM workspace_suggestions WHERE share_id = ?1 ORDER BY seq")?;
        let suggestions = statement
            .query_map(params![id], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .filter_map(|json| serde_json::from_str(&json).ok())
            .collect();
        Ok(suggestions)
    }

    /// Keeps a suggestion by its id, new or changed.
    pub fn upsert_suggestion(&self, id: &str, suggestion: &Suggestion) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO workspace_suggestions (share_id, id, json) VALUES (?1, ?2, ?3) \
             ON CONFLICT (share_id, id) DO UPDATE SET json = excluded.json",
            params![id, suggestion.id, serde_json::to_string(suggestion)?],
        )?;
        Ok(())
    }

    /// The whole list in place of what was kept, in its order.
    pub fn replace_suggestions(&self, id: &str, suggestions: &[Suggestion]) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM workspace_suggestions WHERE share_id = ?1",
            params![id],
        )?;
        for suggestion in suggestions {
            tx.execute(
                "INSERT OR IGNORE INTO workspace_suggestions (share_id, id, json) VALUES (?1, ?2, ?3)",
                params![id, suggestion.id, serde_json::to_string(suggestion)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The comments, oldest first.
    pub fn workspace_comments(&self, id: &str) -> anyhow::Result<Vec<ShareComment>> {
        let conn = self.conn.lock().unwrap();
        let mut statement =
            conn.prepare("SELECT json FROM workspace_comments WHERE share_id = ?1 ORDER BY seq")?;
        let comments = statement
            .query_map(params![id], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .filter_map(|json| serde_json::from_str(&json).ok())
            .collect();
        Ok(comments)
    }

    pub fn add_comment(&self, id: &str, comment: &ShareComment) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO workspace_comments (share_id, id, json) VALUES (?1, ?2, ?3)",
            params![id, comment.id, serde_json::to_string(comment)?],
        )?;
        Ok(())
    }

    /// How many of these comments there were.
    pub fn remove_comments(&self, id: &str, comments: &[String]) -> anyhow::Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut removed = 0;
        for comment in comments {
            removed += tx.execute(
                "DELETE FROM workspace_comments WHERE share_id = ?1 AND id = ?2",
                params![id, comment],
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }
}

fn set_guests(tx: &rusqlite::Transaction, id: &str, guests: &[Guest]) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM workspace_guests WHERE share_id = ?1",
        params![id],
    )?;
    let mut position = 0;
    for guest in guests {
        let email = guest.email.trim().to_lowercase();
        if email.is_empty() {
            continue;
        }
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO workspace_guests (share_id, email, role, position) VALUES (?1, ?2, ?3, ?4)",
            params![id, email, role_name(guest.role), position],
        )?;
        position += inserted as i64;
    }
    Ok(())
}

fn guests_of(conn: &Connection, id: &str) -> rusqlite::Result<Vec<Guest>> {
    let mut statement = conn.prepare(
        "SELECT email, role FROM workspace_guests WHERE share_id = ?1 ORDER BY position",
    )?;
    statement
        .query_map(params![id], |row| {
            Ok(Guest {
                email: row.get(0)?,
                role: role_of_name(&row.get::<_, String>(1)?),
            })
        })?
        .collect()
}

fn insert_events(
    tx: &rusqlite::Transaction,
    id: &str,
    chat: &str,
    events: &[EventEnvelope],
) -> anyhow::Result<usize> {
    let mut kept = 0;
    for event in events {
        let Some(seq) = event.seq else { continue };
        kept += tx.execute(
            "INSERT OR IGNORE INTO workspace_events (share_id, chat_id, seq, json) VALUES (?1, ?2, ?3, ?4)",
            params![id, chat, seq, serde_json::to_string(event)?],
        )?;
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::events::{ChatEvent, Event};
    use chauffeur_protocol::model::{AgentKind, ChatKind, ChatStatus};
    use chauffeur_protocol::workspace_share::SuggestionState;

    fn carol() -> CommentAuthor {
        CommentAuthor {
            name: "Carol".into(),
            email: "Carol@example.com".into(),
        }
    }

    fn guest(email: &str, role: GuestRole) -> Guest {
        Guest {
            email: email.into(),
            role,
        }
    }

    fn chat(id: &str) -> SharedChat {
        SharedChat {
            id: id.into(),
            title: None,
            kind: ChatKind::Chat,
            agent: AgentKind::Claude,
            status: ChatStatus::Idle,
            created_at: "2026-10-29T10:00:00Z".into(),
        }
    }

    fn event(chat: &str, seq: Option<i64>, text: &str) -> EventEnvelope {
        EventEnvelope {
            seq,
            ts: "2026-10-29T10:00:00Z".into(),
            event: Event::Chat {
                workspace_id: "w".into(),
                chat_id: chat.into(),
                event: ChatEvent::AssistantText {
                    message_id: text.into(),
                    text: text.into(),
                    parent_tool_use_id: None,
                },
            },
        }
    }

    #[test]
    fn a_share_s_roles_go_to_its_guests_the_team_and_its_owner_while_it_lives() {
        let store = Store::open_in_memory().unwrap();
        let share = store
            .create_workspace_share(
                "w1",
                &carol(),
                "Architecture",
                &[
                    guest("Alice@example.com", GuestRole::Suggest),
                    guest(" ", GuestRole::Viewer),
                    guest("bob@example.com", GuestRole::Viewer),
                    guest("alice@example.com", GuestRole::Viewer),
                ],
                None,
                10,
            )
            .unwrap();
        assert_eq!(share.owner.email, "carol@example.com");
        assert_eq!(
            share.guests,
            [
                guest("alice@example.com", GuestRole::Suggest),
                guest("bob@example.com", GuestRole::Viewer)
            ]
        );
        let who = |email: &str, hd: Option<&str>| Identity::new(email, None).in_workspace(hd);
        let dave = who("dave@example.com", Some("example.com"));
        assert_eq!(
            share.role_of(&who("ALICE@example.com", None)),
            Some(GuestRole::Suggest)
        );
        assert_eq!(
            share.role_of(&who("bob@example.com", None)),
            Some(GuestRole::Viewer)
        );
        assert_eq!(
            share.role_of(&who("carol@example.com", None)),
            Some(GuestRole::Viewer)
        );
        assert_eq!(share.role_of(&dave), None);

        store
            .update_workspace_share("w1", None, None, Some(Some("example.com")))
            .unwrap();
        let share = store.workspace_share("w1").unwrap().unwrap();
        assert_eq!(share.role_of(&dave), Some(GuestRole::Viewer));
        assert_eq!(share.role_of(&who("dave@example.com", None)), None);
        assert_eq!(
            share.role_of(&who("alice@example.com", Some("example.com"))),
            Some(GuestRole::Suggest),
            "a named guest keeps their role on a team share"
        );

        store.stop_workspace_share("w1", 20).unwrap();
        let share = store.workspace_share("w1").unwrap().unwrap();
        assert!(share.stopped() && share.role_of(&who("carol@example.com", None)).is_none());
        assert_eq!(store.workspace_share("nope").unwrap(), None);
    }

    #[test]
    fn a_chat_no_longer_listed_takes_its_events_away() {
        let store = Store::open_in_memory().unwrap();
        store
            .create_workspace_share("w1", &carol(), "a", &[], None, 1)
            .unwrap();
        store
            .set_workspace_chats("w1", &[chat("c1"), chat("c2")])
            .unwrap();
        store
            .replace_chat_events(
                "w1",
                "c1",
                &[event("c1", Some(2), "b"), event("c1", Some(1), "a")],
            )
            .unwrap();
        let kept = store
            .add_chat_events(
                "w1",
                "c1",
                &[
                    event("c1", Some(2), "again"),
                    event("c1", None, "typing"),
                    event("c1", Some(3), "c"),
                ],
            )
            .unwrap();
        assert_eq!(kept, 1, "an event kept already, or one without seq, is not");
        store
            .add_chat_events("w1", "c2", &[event("c2", Some(4), "x")])
            .unwrap();
        let seqs = |chat: &str| -> Vec<Option<i64>> {
            store
                .chat_events("w1", chat)
                .unwrap()
                .iter()
                .map(|e| e.seq)
                .collect()
        };
        assert_eq!(seqs("c1"), [Some(1), Some(2), Some(3)]);

        store.set_workspace_chats("w1", &[chat("c1")]).unwrap();
        assert_eq!(store.workspace_chats("w1").unwrap(), [chat("c1")]);
        assert!(seqs("c2").is_empty());
        assert_eq!(seqs("c1").len(), 3);

        store
            .replace_chat_events("w1", "c1", &[event("c1", Some(9), "z")])
            .unwrap();
        assert_eq!(seqs("c1"), [Some(9)]);
    }

    #[test]
    fn a_chat_no_longer_listed_takes_its_suggestions_and_comments_away() {
        let store = Store::open_in_memory().unwrap();
        store
            .create_workspace_share("w1", &carol(), "a", &[], None, 1)
            .unwrap();
        assert!(
            !store
                .set_workspace_chats("w1", &[chat("c1"), chat("c2")])
                .unwrap()
        );
        for (id, chat) in [("1", "c1"), ("2", "c2")] {
            store
                .upsert_suggestion(
                    "w1",
                    &Suggestion {
                        id: format!("s{id}"),
                        chat: chat.into(),
                        author: carol(),
                        text: "go".into(),
                        at: 1,
                        state: SuggestionState::Pending,
                        decided_at: None,
                    },
                )
                .unwrap();
            store
                .add_comment(
                    "w1",
                    &ShareComment {
                        id: format!("k{id}"),
                        chat: chat.into(),
                        message: "m1".into(),
                        quote: "a".into(),
                        author: carol(),
                        text: "why?".into(),
                        at: 1,
                        reply_to: None,
                        mentions: Vec::new(),
                    },
                )
                .unwrap();
        }

        assert!(store.set_workspace_chats("w1", &[chat("c1")]).unwrap());
        let suggestions = store.workspace_suggestions("w1").unwrap();
        assert_eq!(
            suggestions
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["s1"]
        );
        let comments = store.workspace_comments("w1").unwrap();
        assert_eq!(
            comments.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["k1"]
        );
        assert!(
            !store.set_workspace_chats("w1", &[chat("c1")]).unwrap(),
            "nothing more to drop"
        );
    }

    #[test]
    fn stopping_a_share_removes_its_hosted_copy() {
        let store = Store::open_in_memory().unwrap();
        store
            .create_workspace_share("w1", &carol(), "a", &[], None, 1)
            .unwrap();
        store.set_workspace_chats("w1", &[chat("c1")]).unwrap();
        store
            .add_chat_events("w1", "c1", &[event("c1", Some(1), "a")])
            .unwrap();
        let note = SharedNote {
            path: "plan.md".into(),
            text: "# Plan".into(),
            modified_ms: 5,
        };
        store
            .change_workspace_notes("w1", std::slice::from_ref(&note), &[])
            .unwrap();
        store
            .upsert_suggestion(
                "w1",
                &Suggestion {
                    id: "s1".into(),
                    chat: "c1".into(),
                    author: carol(),
                    text: "go".into(),
                    at: 1,
                    state: SuggestionState::Pending,
                    decided_at: None,
                },
            )
            .unwrap();
        store
            .add_comment(
                "w1",
                &ShareComment {
                    id: "k1".into(),
                    chat: "c1".into(),
                    message: "m1".into(),
                    quote: "a".into(),
                    author: carol(),
                    text: "why?".into(),
                    at: 1,
                    reply_to: None,
                    mentions: Vec::new(),
                },
            )
            .unwrap();
        assert_eq!(store.workspace_notes("w1").unwrap(), [note]);

        store.stop_workspace_share("w1", 2).unwrap();
        assert!(store.workspace_chats("w1").unwrap().is_empty());
        assert!(store.chat_events("w1", "c1").unwrap().is_empty());
        assert!(store.workspace_notes("w1").unwrap().is_empty());
        assert!(store.workspace_suggestions("w1").unwrap().is_empty());
        assert!(store.workspace_comments("w1").unwrap().is_empty());
    }

    #[test]
    fn notes_change_and_go_by_path() {
        let store = Store::open_in_memory().unwrap();
        store
            .create_workspace_share("w1", &carol(), "a", &[], None, 1)
            .unwrap();
        let note = |path: &str, text: &str| SharedNote {
            path: path.into(),
            text: text.into(),
            modified_ms: 1,
        };
        store
            .change_workspace_notes("w1", &[note("b.md", "b"), note("a.md", "a")], &[])
            .unwrap();
        store
            .change_workspace_notes("w1", &[note("b.md", "b2")], &["a.md".into()])
            .unwrap();
        assert_eq!(store.workspace_notes("w1").unwrap(), [note("b.md", "b2")]);
    }
}
