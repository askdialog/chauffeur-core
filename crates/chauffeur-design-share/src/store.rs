//! What the service keeps: shares, their reviewers and their comments, in one SQLite file on the
//! VM's disk. The files of a design are on the disk beside it (files.rs). Workspace shares live
//! in the same file (workspace.rs), and so do the e-mails' records (mail.rs): what was sent, the
//! sign-in links, the addresses that stopped Chauffeur's e-mails.

use std::path::Path;
use std::sync::Mutex;

use chauffeur_protocol::client::{CommentAuthor, DesignComment};
use rusqlite::{Connection, OptionalExtension, params};

use crate::Identity;

mod mail;
mod mentions;
mod workspace;

pub use mail::EmailKind;
pub use mentions::{MentionRow, WantedMention};

pub use workspace::WorkspaceShareRow;

pub struct Store {
    conn: Mutex<Connection>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShareRow {
    pub id: String,
    pub owner: CommentAuthor,
    pub title: String,
    pub created_at: u64,
    pub stopped_at: Option<u64>,
    pub last_push_at: Option<u64>,
    /// Lowercase, in the order given.
    pub reviewers: Vec<String>,
    /// The team domain it is open to, named or not: its owner's Google Workspace domain when
    /// they opened it to their team.
    pub team_domain: Option<String>,
    /// A markdown file's share (story 57): the file's name. None: a design.
    pub document: Option<String>,
}

impl ShareRow {
    pub fn stopped(&self) -> bool {
        self.stopped_at.is_some()
    }

    /// Whether someone may look at the share while it lives: its owner, one named on it, or an
    /// account of its team domain.
    pub fn can_view(&self, who: &Identity) -> bool {
        !self.stopped()
            && (self.owner.email.eq_ignore_ascii_case(&who.email)
                || self.named(&who.email)
                || in_team(self.team_domain.as_deref(), who))
    }

    pub fn named(&self, email: &str) -> bool {
        self.reviewers.iter().any(|r| r.eq_ignore_ascii_case(email))
    }
}

/// Whether an identity's Google Workspace domain is the team domain a share is open to; a
/// personal Google account or a sign-in link never is.
pub fn in_team(team_domain: Option<&str>, who: &Identity) -> bool {
    team_domain.is_some_and(|team| who.hd.as_deref() == Some(team))
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS shares (
    id TEXT PRIMARY KEY,
    owner_email TEXT NOT NULL,
    owner_name TEXT NOT NULL,
    title TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    stopped_at INTEGER,
    last_push_at INTEGER,
    team INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS reviewers (
    share_id TEXT NOT NULL REFERENCES shares(id),
    email TEXT NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (share_id, email)
);
CREATE TABLE IF NOT EXISTS comments (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    share_id TEXT NOT NULL REFERENCES shares(id),
    id TEXT NOT NULL,
    json TEXT NOT NULL,
    UNIQUE (share_id, id)
);
";

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Store> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        Self::with(conn)
    }

    pub fn open_in_memory() -> anyhow::Result<Store> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Self::with(conn)
    }

    fn with(conn: Connection) -> anyhow::Result<Store> {
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(workspace::SCHEMA)?;
        conn.execute_batch(mail::SCHEMA)?;
        conn.execute_batch(mentions::SCHEMA)?;
        let has = |table: &str, column: &str| -> rusqlite::Result<bool> {
            conn.prepare(&format!(
                "SELECT 1 FROM pragma_table_info('{table}') WHERE name = '{column}'"
            ))?
            .exists([])
        };
        if !has("shares", "team")? {
            // A base from before shares were opened to the team.
            conn.execute_batch("ALTER TABLE shares ADD COLUMN team INTEGER NOT NULL DEFAULT 0")?;
        }
        for table in ["shares", "workspace_shares"] {
            if !has(table, "team_domain")? {
                // A base from before a share's team was its owner's domain: `team` is kept in
                // step with it, for `adopt_team_shares`.
                conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN team_domain TEXT"))?;
            }
        }
        if !has("shares", "document")? {
            // A base from before a share could hold a markdown file.
            conn.execute_batch("ALTER TABLE shares ADD COLUMN document TEXT")?;
        }
        Ok(Store {
            conn: Mutex::new(conn),
        })
    }

    /// Shares opened to the team before a share named its team domain get `domain`, the one
    /// every account then belonged to; returns how many.
    pub fn adopt_team_shares(&self, domain: &str) -> anyhow::Result<usize> {
        let conn = self.conn.lock().unwrap();
        let mut adopted = 0;
        for table in ["shares", "workspace_shares"] {
            adopted += conn.execute(
                &format!(
                    "UPDATE {table} SET team_domain = ?1 WHERE team = 1 AND team_domain IS NULL"
                ),
                params![domain],
            )?;
        }
        Ok(adopted)
    }

    pub fn create_share(
        &self,
        id: &str,
        owner: &CommentAuthor,
        title: &str,
        reviewers: &[String],
        team_domain: Option<&str>,
        now: u64,
    ) -> anyhow::Result<ShareRow> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO shares (id, owner_email, owner_name, title, created_at, team, team_domain) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, owner.email.to_lowercase(), owner.name, title, now as i64, team_domain.is_some(), team_domain],
        )?;
        Self::set_reviewers(&tx, id, reviewers)?;
        tx.commit()?;
        drop(conn);
        Ok(self.share(id)?.expect("just created"))
    }

    /// Makes a new share hold the markdown file `name` instead of a design.
    pub fn set_document(&self, id: &str, name: &str) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE shares SET document = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(())
    }

    fn set_reviewers(
        tx: &rusqlite::Transaction,
        id: &str,
        reviewers: &[String],
    ) -> rusqlite::Result<()> {
        tx.execute("DELETE FROM reviewers WHERE share_id = ?1", params![id])?;
        let mut position = 0;
        for email in reviewers {
            let email = email.trim().to_lowercase();
            if email.is_empty() {
                continue;
            }
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO reviewers (share_id, email, position) VALUES (?1, ?2, ?3)",
                params![id, email, position],
            )?;
            position += inserted as i64;
        }
        Ok(())
    }

    pub fn share(&self, id: &str) -> anyhow::Result<Option<ShareRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                &format!("SELECT {SHARE_COLUMNS} FROM shares WHERE id = ?1"),
                params![id],
                share_row,
            )
            .optional()?;
        let Some(mut share) = row else {
            return Ok(None);
        };
        share.reviewers = reviewers_of(&conn, id)?;
        Ok(Some(share))
    }

    /// The live shares of others that someone may look at: named on them, or open to their team
    /// domain; the most recently changed first.
    pub fn shares_for(&self, who: &Identity) -> anyhow::Result<Vec<ShareRow>> {
        let email = who.email.trim().to_lowercase();
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(&format!(
            "SELECT {SHARE_COLUMNS} FROM shares s WHERE s.stopped_at IS NULL AND s.owner_email != ?1 \
             AND (s.team_domain = ?2 OR EXISTS (SELECT 1 FROM reviewers r WHERE r.share_id = s.id AND r.email = ?1)) \
             ORDER BY COALESCE(s.last_push_at, s.created_at) DESC, s.created_at DESC"
        ))?;
        let mut shares: Vec<ShareRow> = statement
            .query_map(params![email, who.hd], share_row)?
            .collect::<Result<_, _>>()?;
        for share in &mut shares {
            share.reviewers = reviewers_of(&conn, &share.id)?;
        }
        Ok(shares)
    }

    /// Changes what is given: `team_domain` `Some(None)` closes the share to its team.
    pub fn update_share(
        &self,
        id: &str,
        title: Option<&str>,
        reviewers: Option<&[String]>,
        team_domain: Option<Option<&str>>,
    ) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        if let Some(title) = title {
            tx.execute(
                "UPDATE shares SET title = ?2 WHERE id = ?1",
                params![id, title],
            )?;
        }
        if let Some(team_domain) = team_domain {
            tx.execute(
                "UPDATE shares SET team = ?2, team_domain = ?3 WHERE id = ?1",
                params![id, team_domain.is_some(), team_domain],
            )?;
        }
        if let Some(reviewers) = reviewers {
            Self::set_reviewers(&tx, id, reviewers)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn stop_share(&self, id: &str, now: u64) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE shares SET stopped_at = ?2 WHERE id = ?1 AND stopped_at IS NULL",
            params![id, now as i64],
        )?;
        Ok(())
    }

    pub fn touch_push(&self, id: &str, now: u64) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE shares SET last_push_at = ?2 WHERE id = ?1",
            params![id, now as i64],
        )?;
        Ok(())
    }

    /// The e-mails an owner named on earlier shares, most recent first, for the app to recall.
    pub fn past_reviewers(&self, owner_email: &str) -> anyhow::Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT r.email FROM reviewers r JOIN shares s ON s.id = r.share_id \
             WHERE s.owner_email = ?1 ORDER BY s.created_at DESC, r.position",
        )?;
        let mut seen = std::collections::HashSet::new();
        let emails = statement
            .query_map(params![owner_email.to_lowercase()], |row| {
                row.get::<_, String>(0)
            })?
            .filter_map(|r| r.ok())
            .filter(|email| seen.insert(email.clone()))
            .collect();
        Ok(emails)
    }

    pub fn comments(&self, share_id: &str) -> anyhow::Result<Vec<DesignComment>> {
        let conn = self.conn.lock().unwrap();
        let mut statement =
            conn.prepare("SELECT json FROM comments WHERE share_id = ?1 ORDER BY seq")?;
        let comments = statement
            .query_map(params![share_id], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .filter_map(|json| serde_json::from_str(&json).ok())
            .collect();
        Ok(comments)
    }

    /// The daemon's whole list takes the place of the service's, in its order.
    pub fn replace_comments(
        &self,
        share_id: &str,
        comments: &[DesignComment],
    ) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM comments WHERE share_id = ?1",
            params![share_id],
        )?;
        for comment in comments {
            tx.execute(
                "INSERT INTO comments (share_id, id, json) VALUES (?1, ?2, ?3)",
                params![share_id, comment.id, serde_json::to_string(comment)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_comment(&self, share_id: &str, comment: &DesignComment) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO comments (share_id, id, json) VALUES (?1, ?2, ?3) \
             ON CONFLICT (share_id, id) DO UPDATE SET json = excluded.json",
            params![share_id, comment.id, serde_json::to_string(comment)?],
        )?;
        Ok(())
    }

    pub fn delete_comments(&self, share_id: &str, ids: &[String]) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        for id in ids {
            conn.execute(
                "DELETE FROM comments WHERE share_id = ?1 AND id = ?2",
                params![share_id, id],
            )?;
        }
        Ok(())
    }
}

const SHARE_COLUMNS: &str = "id, owner_email, owner_name, title, created_at, stopped_at, last_push_at, team_domain, document";

/// A row of `SHARE_COLUMNS`, its reviewers still to read.
fn share_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ShareRow> {
    Ok(ShareRow {
        id: row.get(0)?,
        owner: CommentAuthor {
            email: row.get(1)?,
            name: row.get(2)?,
        },
        title: row.get(3)?,
        created_at: row.get::<_, i64>(4)? as u64,
        stopped_at: row.get::<_, Option<i64>>(5)?.map(|v| v as u64),
        last_push_at: row.get::<_, Option<i64>>(6)?.map(|v| v as u64),
        reviewers: Vec::new(),
        team_domain: row.get(7)?,
        document: row.get(8)?,
    })
}

fn reviewers_of(conn: &Connection, share_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement =
        conn.prepare("SELECT email FROM reviewers WHERE share_id = ?1 ORDER BY position")?;
    statement
        .query_map(params![share_id], |row| row.get(0))?
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn carol() -> CommentAuthor {
        CommentAuthor {
            name: "Carol".into(),
            email: "Carol@example.com".into(),
        }
    }

    /// Someone signed in with a Google account of `hd`, or none.
    fn who(email: &str, hd: Option<&str>) -> Identity {
        Identity::new(email, None).in_workspace(hd)
    }

    fn comment(id: &str, text: &str) -> DesignComment {
        DesignComment {
            id: id.into(),
            screen: "a.html".into(),
            quote: None,
            pin: None,
            author: carol(),
            at: 1,
            text: text.into(),
            reply_to: None,
            resolved: None,
            sent: None,
            mentions: Vec::new(),
        }
    }

    #[test]
    fn a_share_keeps_its_reviewers_lowercase_in_order_and_once() {
        let store = Store::open_in_memory().unwrap();
        let share = store
            .create_share(
                "s1",
                &carol(),
                "CTA",
                &[
                    "Alice@example.com".into(),
                    " ".into(),
                    "bob@example.com".into(),
                    "alice@example.com".into(),
                ],
                None,
                10,
            )
            .unwrap();
        assert_eq!(share.owner.email, "carol@example.com");
        assert_eq!(share.reviewers, ["alice@example.com", "bob@example.com"]);
        assert!(share.can_view(&who("ALICE@example.com", None)));
        assert!(share.can_view(&who("carol@example.com", None)));
        assert!(!share.can_view(&who("dave@example.com", Some("example.com"))));
        store
            .update_share(
                "s1",
                Some("CTA v2"),
                Some(&["dave@example.com".into()]),
                None,
            )
            .unwrap();
        let share = store.share("s1").unwrap().unwrap();
        assert_eq!(
            (share.title.as_str(), share.reviewers.as_slice()),
            ("CTA v2", &["dave@example.com".to_string()][..])
        );
        store.stop_share("s1", 20).unwrap();
        let share = store.share("s1").unwrap().unwrap();
        assert!(share.stopped() && !share.can_view(&who("dave@example.com", None)));
        assert_eq!(store.share("nope").unwrap(), None);
    }

    #[test]
    fn past_reviewers_are_recalled_most_recent_first_without_repeats() {
        let store = Store::open_in_memory().unwrap();
        store
            .create_share("s1", &carol(), "a", &["alice@example.com".into()], None, 1)
            .unwrap();
        store
            .create_share(
                "s2",
                &carol(),
                "b",
                &["bob@example.com".into(), "alice@example.com".into()],
                None,
                2,
            )
            .unwrap();
        assert_eq!(
            store.past_reviewers("carol@example.com").unwrap(),
            ["bob@example.com", "alice@example.com"]
        );
        assert!(
            store
                .past_reviewers("alice@example.com")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn comments_are_replaced_whole_by_the_daemon_and_changed_one_by_one_by_reviewers() {
        let store = Store::open_in_memory().unwrap();
        store
            .create_share("s1", &carol(), "a", &[], None, 1)
            .unwrap();
        store
            .replace_comments("s1", &[comment("c2", "second"), comment("c1", "first")])
            .unwrap();
        assert_eq!(
            store
                .comments("s1")
                .unwrap()
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["c2", "c1"]
        );
        store
            .upsert_comment("s1", &comment("c1", "edited"))
            .unwrap();
        store.upsert_comment("s1", &comment("c3", "third")).unwrap();
        let comments = store.comments("s1").unwrap();
        assert_eq!(
            comments.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            ["second", "edited", "third"]
        );
        store
            .delete_comments("s1", &["c2".into(), "c3".into()])
            .unwrap();
        assert_eq!(store.comments("s1").unwrap().len(), 1);
        store.replace_comments("s1", &[]).unwrap();
        assert!(store.comments("s1").unwrap().is_empty());
    }

    #[test]
    fn a_team_share_opens_to_accounts_of_its_team_domain_and_lists_beside_those_naming_them() {
        let store = Store::open_in_memory().unwrap();
        let alice = CommentAuthor {
            name: "Alice".into(),
            email: "alice@example.com".into(),
        };
        store
            .create_share("named", &carol(), "a", &["bob@example.com".into()], None, 1)
            .unwrap();
        store
            .create_share("team", &alice, "b", &[], Some("example.com"), 2)
            .unwrap();
        store
            .create_share(
                "private",
                &alice,
                "c",
                &["dave@example.com".into()],
                None,
                3,
            )
            .unwrap();
        store
            .create_share(
                "mine",
                &CommentAuthor {
                    name: "Bob".into(),
                    email: "bob@example.com".into(),
                },
                "d",
                &[],
                Some("example.com"),
                4,
            )
            .unwrap();
        store
            .create_share("stopped", &alice, "e", &[], Some("example.com"), 5)
            .unwrap();
        store.stop_share("stopped", 6).unwrap();
        store
            .create_share("other", &alice, "f", &[], Some("other.com"), 7)
            .unwrap();
        store.touch_push("named", 10).unwrap();

        let bob = who("bob@example.com", Some("example.com"));
        let team = store.share("team").unwrap().unwrap();
        assert!(team.can_view(&bob));
        // The same address without its Workspace's word (a sign-in link, a personal account)
        // is not of the team.
        assert!(!team.can_view(&who("bob@example.com", None)));
        assert!(!team.can_view(&who("eve@other.com", Some("other.com"))));
        assert!(!store.share("private").unwrap().unwrap().can_view(&bob));
        // Others' live shares only, the most recently changed first.
        let listed: Vec<String> = store
            .shares_for(&who("Bob@example.com", Some("example.com")))
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(listed, ["named", "team"]);
        assert_eq!(
            store
                .shares_for(&who("bob@example.com", None))
                .unwrap()
                .len(),
            1
        );

        store.update_share("team", None, None, Some(None)).unwrap();
        let team = store.share("team").unwrap().unwrap();
        assert!(team.team_domain.is_none() && !team.can_view(&bob));
        assert_eq!(store.shares_for(&bob).unwrap().len(), 1);
    }

    #[test]
    fn a_base_from_before_team_domains_gives_its_team_shares_the_domain_adopted() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE shares (id TEXT PRIMARY KEY, owner_email TEXT NOT NULL, owner_name TEXT NOT NULL, \
             title TEXT NOT NULL, created_at INTEGER NOT NULL, stopped_at INTEGER, last_push_at INTEGER); \
             INSERT INTO shares (id, owner_email, owner_name, title, created_at) VALUES ('s1', 'carol@example.com', 'Carol', 'a', 1);",
        )
        .unwrap();
        let store = Store::with(conn).unwrap();
        assert!(store.share("s1").unwrap().unwrap().team_domain.is_none());
        store
            .conn
            .lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO shares (id, owner_email, owner_name, title, created_at, team) VALUES ('s2', 'carol@example.com', 'Carol', 'b', 2, 1); \
                 INSERT INTO workspace_shares (id, owner_email, owner_name, title, created_at, team) VALUES ('w1', 'carol@example.com', 'Carol', 'c', 3, 1);",
            )
            .unwrap();
        assert_eq!(store.adopt_team_shares("example.com").unwrap(), 2);
        assert_eq!(
            store.share("s2").unwrap().unwrap().team_domain.as_deref(),
            Some("example.com")
        );
        assert_eq!(
            store
                .workspace_share("w1")
                .unwrap()
                .unwrap()
                .team_domain
                .as_deref(),
            Some("example.com")
        );
        assert!(store.share("s1").unwrap().unwrap().team_domain.is_none());
        // Closed since, a share is not adopted again at the next start.
        store.update_share("s2", None, None, Some(None)).unwrap();
        assert_eq!(store.adopt_team_shares("example.com").unwrap(), 0);
    }
}
