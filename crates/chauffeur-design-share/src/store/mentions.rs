//! Who is mentioned where (docs/adr/mentions-in-share-comments.md), and the names the service
//! learned of people from their Google sign-in, for the "Shared with" and mention lists. A design
//! share's mentions and a workspace share's live in two tables of the same shape, each tied to its
//! kind of share.

use std::collections::{HashMap, HashSet};

use chauffeur_protocol::design_share::MentionPlace;
use rusqlite::{OptionalExtension, params};

use super::Store;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS people (
    email TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS mentions (
    id TEXT PRIMARY KEY,
    share_id TEXT NOT NULL REFERENCES shares(id),
    comment_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    email TEXT NOT NULL,
    at INTEGER NOT NULL,
    seen_at INTEGER,
    UNIQUE (share_id, comment_id, email)
);
CREATE INDEX IF NOT EXISTS mentions_by_email ON mentions (email, at);
CREATE TABLE IF NOT EXISTS workspace_mentions (
    id TEXT PRIMARY KEY,
    share_id TEXT NOT NULL REFERENCES workspace_shares(id),
    comment_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    email TEXT NOT NULL,
    at INTEGER NOT NULL,
    seen_at INTEGER,
    UNIQUE (share_id, comment_id, email)
);
CREATE INDEX IF NOT EXISTS workspace_mentions_by_email ON workspace_mentions (email, at);
";

/// The table of a place's mentions.
fn table(place: MentionPlace) -> &'static str {
    match place {
        MentionPlace::Design => "mentions",
        MentionPlace::Workspace => "workspace_mentions",
    }
}

const PLACES: [MentionPlace; 2] = [MentionPlace::Design, MentionPlace::Workspace];

/// A mention a comment of a share makes, as the comments say it now.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WantedMention {
    pub comment_id: String,
    pub thread_id: String,
    /// Lowercase.
    pub email: String,
}

/// A mention of someone, as kept.
#[derive(Clone, Debug, PartialEq)]
pub struct MentionRow {
    pub id: String,
    pub place: MentionPlace,
    pub share_id: String,
    pub comment_id: String,
    pub thread_id: String,
    pub at: u64,
    pub seen: bool,
}

impl Store {
    /// Keeps the name someone's Google account gave; true when it is new or changed.
    pub fn remember_name(&self, email: &str, name: &str, now: u64) -> anyhow::Result<bool> {
        let email = email.trim().to_lowercase();
        let name = name.trim();
        if email.is_empty() || name.is_empty() {
            return Ok(false);
        }
        let conn = self.conn.lock().unwrap();
        let known: Option<String> = conn
            .query_row(
                "SELECT name FROM people WHERE email = ?1",
                params![email],
                |row| row.get(0),
            )
            .optional()?;
        if known.as_deref() == Some(name) {
            return Ok(false);
        }
        conn.execute(
            "INSERT INTO people (email, name, updated_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT (email) DO UPDATE SET name = excluded.name, updated_at = excluded.updated_at",
            params![email, name, now as i64],
        )?;
        Ok(true)
    }

    /// The names known of these e-mails (lowercase), by e-mail.
    pub fn names(&self, emails: &[String]) -> anyhow::Result<HashMap<String, String>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare("SELECT name FROM people WHERE email = ?1")?;
        let mut names = HashMap::new();
        for email in emails {
            let email = email.to_lowercase();
            if let Some(name) = statement
                .query_row(params![email], |row| row.get::<_, String>(0))
                .optional()?
            {
                names.insert(email, name);
            }
        }
        Ok(names)
    }

    /// Makes a share's mentions those `wanted`: new ones kept unread from `now`, those no longer
    /// wanted removed, the others left as they are (read or not). Returns the e-mails whose
    /// mentions changed.
    pub fn sync_mentions(
        &self,
        place: MentionPlace,
        share_id: &str,
        wanted: &[WantedMention],
        now: u64,
    ) -> anyhow::Result<HashSet<String>> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let table = table(place);
        let kept: Vec<(String, String, String)> = {
            let mut statement = tx.prepare(&format!(
                "SELECT id, comment_id, email FROM {table} WHERE share_id = ?1"
            ))?;
            statement
                .query_map(params![share_id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .collect::<Result<_, _>>()?
        };
        let wanted_pairs: HashSet<(&str, &str)> = wanted
            .iter()
            .map(|w| (w.comment_id.as_str(), w.email.as_str()))
            .collect();
        let mut changed = HashSet::new();
        for (id, comment_id, email) in &kept {
            if !wanted_pairs.contains(&(comment_id.as_str(), email.as_str())) {
                tx.execute(&format!("DELETE FROM {table} WHERE id = ?1"), params![id])?;
                changed.insert(email.clone());
            }
        }
        let kept_pairs: HashSet<(&str, &str)> = kept
            .iter()
            .map(|(_, comment_id, email)| (comment_id.as_str(), email.as_str()))
            .collect();
        for mention in wanted {
            if kept_pairs.contains(&(mention.comment_id.as_str(), mention.email.as_str())) {
                continue;
            }
            let inserted = tx.execute(
                &format!(
                    "INSERT OR IGNORE INTO {table} (id, share_id, comment_id, thread_id, email, at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
                ),
                params![
                    crate::random_hex(8),
                    share_id,
                    mention.comment_id,
                    mention.thread_id,
                    mention.email,
                    now as i64
                ],
            )?;
            if inserted > 0 {
                changed.insert(mention.email.clone());
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Someone opened a thread: their mentions in it are read. True when one was not.
    pub fn see_thread(
        &self,
        place: MentionPlace,
        share_id: &str,
        thread_id: &str,
        email: &str,
        now: u64,
    ) -> anyhow::Result<bool> {
        let seen = self.conn.lock().unwrap().execute(
            &format!(
                "UPDATE {} SET seen_at = ?4 \
                 WHERE share_id = ?1 AND thread_id = ?2 AND email = ?3 AND seen_at IS NULL",
                table(place)
            ),
            params![share_id, thread_id, email.to_lowercase(), now as i64],
        )?;
        Ok(seen > 0)
    }

    /// Someone's mention read, by its id, wherever it was made; only theirs. True when it was not.
    pub fn see_mention(&self, id: &str, email: &str, now: u64) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let mut seen = 0;
        for place in PLACES {
            seen += conn.execute(
                &format!(
                    "UPDATE {} SET seen_at = ?3 WHERE id = ?1 AND email = ?2 AND seen_at IS NULL",
                    table(place)
                ),
                params![id, email.to_lowercase(), now as i64],
            )?;
        }
        Ok(seen > 0)
    }

    /// The comments of a share that mention someone and that they have not opened.
    pub fn unread_comments(
        &self,
        place: MentionPlace,
        share_id: &str,
        email: &str,
    ) -> anyhow::Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(&format!(
            "SELECT comment_id FROM {} WHERE share_id = ?1 AND email = ?2 AND seen_at IS NULL \
             ORDER BY at",
            table(place)
        ))?;
        let ids = statement
            .query_map(params![share_id, email.to_lowercase()], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(ids)
    }

    /// Someone's mentions, designs' and workspaces', newest first: every unread one and the
    /// latest read ones, `limit` of each kind at most per place.
    pub fn mentions_of(&self, email: &str, limit: usize) -> anyhow::Result<Vec<MentionRow>> {
        let conn = self.conn.lock().unwrap();
        let mut rows = Vec::new();
        for (place, condition) in PLACES
            .into_iter()
            .flat_map(|place| [(place, "seen_at IS NULL"), (place, "seen_at IS NOT NULL")])
        {
            let mut statement = conn.prepare(&format!(
                "SELECT id, share_id, comment_id, thread_id, at, seen_at FROM {} \
                 WHERE email = ?1 AND {condition} ORDER BY at DESC, rowid DESC LIMIT ?2",
                table(place)
            ))?;
            let found =
                statement.query_map(params![email.to_lowercase(), limit as i64], |row| {
                    Ok(MentionRow {
                        id: row.get(0)?,
                        place,
                        share_id: row.get(1)?,
                        comment_id: row.get(2)?,
                        thread_id: row.get(3)?,
                        at: row.get::<_, i64>(4)? as u64,
                        seen: row.get::<_, Option<i64>>(5)?.is_some(),
                    })
                })?;
            for row in found {
                rows.push(row?);
            }
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.at));
        Ok(rows)
    }

    /// A stopped share's mentions go; returns whose they were.
    pub fn drop_mentions(
        &self,
        place: MentionPlace,
        share_id: &str,
    ) -> anyhow::Result<HashSet<String>> {
        let conn = self.conn.lock().unwrap();
        let table = table(place);
        let emails: HashSet<String> = conn
            .prepare(&format!(
                "SELECT DISTINCT email FROM {table} WHERE share_id = ?1"
            ))?
            .query_map(params![share_id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        conn.execute(
            &format!("DELETE FROM {table} WHERE share_id = ?1"),
            params![share_id],
        )?;
        Ok(emails)
    }
}

#[cfg(test)]
mod tests {
    use chauffeur_protocol::client::CommentAuthor;

    use super::*;

    fn store() -> Store {
        let store = Store::open_in_memory().unwrap();
        store
            .create_share(
                "s1",
                &CommentAuthor {
                    name: "Carol".into(),
                    email: "carol@example.com".into(),
                },
                "a",
                &["alice@example.com".into()],
                None,
                1,
            )
            .unwrap();
        store
    }

    fn wanted(comment: &str, thread: &str, email: &str) -> WantedMention {
        WantedMention {
            comment_id: comment.into(),
            thread_id: thread.into(),
            email: email.into(),
        }
    }

    #[test]
    fn a_google_name_is_kept_and_told_only_when_it_changes() {
        let store = store();
        assert!(
            store
                .remember_name("Alice@example.com", "Alice Martin", 1)
                .unwrap()
        );
        assert!(
            !store
                .remember_name("alice@example.com", "Alice Martin", 2)
                .unwrap()
        );
        assert!(!store.remember_name("bob@example.com", " ", 2).unwrap());
        let names = store
            .names(&["ALICE@example.com".into(), "bob@example.com".into()])
            .unwrap();
        assert_eq!(names.len(), 1);
        assert_eq!(names["alice@example.com"], "Alice Martin");
    }

    #[test]
    fn mentions_follow_the_comments_and_keep_whether_they_were_read() {
        let store = store();
        let changed = store
            .sync_mentions(
                MentionPlace::Design,
                "s1",
                &[
                    wanted("c1", "c1", "alice@example.com"),
                    wanted("c2", "c1", "carol@example.com"),
                ],
                10,
            )
            .unwrap();
        assert_eq!(changed.len(), 2);
        assert_eq!(
            store
                .unread_comments(MentionPlace::Design, "s1", "alice@example.com")
                .unwrap(),
            ["c1"]
        );
        // The same again changes nothing; opening the thread reads both of its comments' ones.
        assert!(
            store
                .sync_mentions(
                    MentionPlace::Design,
                    "s1",
                    &[
                        wanted("c1", "c1", "alice@example.com"),
                        wanted("c2", "c1", "carol@example.com"),
                    ],
                    11,
                )
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .see_thread(MentionPlace::Design, "s1", "c1", "Alice@example.com", 12)
                .unwrap()
        );
        assert!(
            !store
                .see_thread(MentionPlace::Design, "s1", "c1", "alice@example.com", 13)
                .unwrap()
        );
        assert!(
            store
                .unread_comments(MentionPlace::Design, "s1", "alice@example.com")
                .unwrap()
                .is_empty()
        );
        let alice = store.mentions_of("alice@example.com", 50).unwrap();
        assert!(alice.len() == 1 && alice[0].seen && alice[0].at == 10);
        // Edited out: gone; Carol's read by its id, and only by her.
        let changed = store
            .sync_mentions(
                MentionPlace::Design,
                "s1",
                &[wanted("c2", "c1", "carol@example.com")],
                14,
            )
            .unwrap();
        assert_eq!(changed, HashSet::from(["alice@example.com".to_string()]));
        let carol = store.mentions_of("carol@example.com", 50).unwrap();
        assert!(
            !store
                .see_mention(&carol[0].id, "alice@example.com", 15)
                .unwrap()
        );
        assert!(
            store
                .see_mention(&carol[0].id, "carol@example.com", 15)
                .unwrap()
        );
        assert_eq!(
            store.drop_mentions(MentionPlace::Design, "s1").unwrap(),
            HashSet::from(["carol@example.com".to_string()])
        );
        assert!(
            store
                .mentions_of("carol@example.com", 50)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_workspace_share_s_mentions_are_kept_apart_and_listed_with_the_designs() {
        let store = store();
        store
            .create_workspace_share(
                "w1",
                &CommentAuthor {
                    name: "Carol".into(),
                    email: "carol@example.com".into(),
                },
                "Architecture",
                &[],
                None,
                1,
            )
            .unwrap();
        store
            .sync_mentions(
                MentionPlace::Design,
                "s1",
                &[wanted("c1", "c1", "alice@example.com")],
                10,
            )
            .unwrap();
        store
            .sync_mentions(
                MentionPlace::Workspace,
                "w1",
                &[wanted("k2", "k1", "alice@example.com")],
                20,
            )
            .unwrap();
        let rows = store.mentions_of("alice@example.com", 50).unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| (r.place, r.comment_id.as_str()))
                .collect::<Vec<_>>(),
            [
                (MentionPlace::Workspace, "k2"),
                (MentionPlace::Design, "c1")
            ]
        );
        // A thread of one place is not one of the other's.
        assert!(
            !store
                .see_thread(MentionPlace::Design, "w1", "k1", "alice@example.com", 21)
                .unwrap()
        );
        assert_eq!(
            store
                .unread_comments(MentionPlace::Workspace, "w1", "alice@example.com")
                .unwrap(),
            ["k2"]
        );
        // Read by its id from the app, wherever it was made.
        assert!(
            store
                .see_mention(&rows[0].id, "alice@example.com", 22)
                .unwrap()
        );
        assert!(
            store
                .unread_comments(MentionPlace::Workspace, "w1", "alice@example.com")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .drop_mentions(MentionPlace::Workspace, "w1")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(store.mentions_of("alice@example.com", 50).unwrap().len(), 1);
    }

    #[test]
    fn someone_s_mentions_are_every_unread_one_then_the_latest_read_newest_first() {
        let store = store();
        let all: Vec<WantedMention> = (0..5)
            .map(|i| wanted(&format!("c{i}"), &format!("c{i}"), "alice@example.com"))
            .collect();
        // One comment more each time, each mention from its own time.
        for i in 0..all.len() {
            store
                .sync_mentions(MentionPlace::Design, "s1", &all[..=i], 100 + i as u64)
                .unwrap();
        }
        store
            .see_thread(MentionPlace::Design, "s1", "c0", "alice@example.com", 200)
            .unwrap();
        store
            .see_thread(MentionPlace::Design, "s1", "c1", "alice@example.com", 200)
            .unwrap();
        let rows = store.mentions_of("alice@example.com", 1).unwrap();
        // One of each kind: the newest unread, the newest read.
        assert_eq!(
            rows.iter()
                .map(|r| (r.comment_id.as_str(), r.seen))
                .collect::<Vec<_>>(),
            [("c4", false), ("c1", true)]
        );
    }
}
