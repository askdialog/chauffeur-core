//! Comments that do not mention their share's owner, waiting for the quiet that tells the owner
//! of them in one notification (docs/adr/untagged-comments-digest.md). A design share's and a
//! workspace share's live in one table, told apart by `place`.

use chauffeur_protocol::design_share::MentionPlace;
use rusqlite::params;

use super::Store;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS pending_digests (
    place TEXT NOT NULL,
    share_id TEXT NOT NULL,
    comment_id TEXT NOT NULL,
    author TEXT NOT NULL,
    at INTEGER NOT NULL,
    notified_at INTEGER,
    PRIMARY KEY (place, share_id, comment_id)
);
";

fn place_name(place: MentionPlace) -> &'static str {
    match place {
        MentionPlace::Design => "design",
        MentionPlace::Workspace => "workspace",
    }
}

/// A comment that should reach the owner once its share has been quiet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WantedDigest {
    pub comment_id: String,
    /// The author's name, as the notification shows it.
    pub author: String,
}

/// A share whose comments wait for its owner, quiet for long enough.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DueDigest {
    pub place: MentionPlace,
    pub share_id: String,
    pub count: u32,
    /// Each author once, in the order of their first comment.
    pub authors: Vec<String>,
}

impl Store {
    /// Makes a share's waiting comments those `wanted`: a new one waits from `now`, one no longer
    /// wanted (deleted, answered by the owner) is dropped, the others keep their time, told or not.
    pub fn sync_digests(
        &self,
        place: MentionPlace,
        share_id: &str,
        wanted: &[WantedDigest],
        now: u64,
    ) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let place = place_name(place);
        let kept: Vec<String> = {
            let mut statement = tx.prepare(
                "SELECT comment_id FROM pending_digests WHERE place = ?1 AND share_id = ?2",
            )?;
            statement
                .query_map(params![place, share_id], |row| row.get(0))?
                .collect::<Result<_, _>>()?
        };
        for comment_id in &kept {
            if !wanted.iter().any(|w| &w.comment_id == comment_id) {
                tx.execute(
                    "DELETE FROM pending_digests WHERE place = ?1 AND share_id = ?2 AND comment_id = ?3",
                    params![place, share_id, comment_id],
                )?;
            }
        }
        for comment in wanted.iter().filter(|w| !kept.contains(&w.comment_id)) {
            tx.execute(
                "INSERT INTO pending_digests (place, share_id, comment_id, author, at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    place,
                    share_id,
                    comment.comment_id,
                    comment.author,
                    now as i64
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Forgets a share's waiting comments (it was stopped).
    pub fn drop_digests(&self, place: MentionPlace, share_id: &str) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "DELETE FROM pending_digests WHERE place = ?1 AND share_id = ?2",
            params![place_name(place), share_id],
        )?;
        Ok(())
    }

    /// The shares with comments not yet told whose latest arrived `quiet_ms` ago or more, the
    /// longest waiting first.
    pub fn due_digests(&self, now: u64, quiet_ms: u64) -> anyhow::Result<Vec<DueDigest>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT place, share_id FROM pending_digests WHERE notified_at IS NULL \
             GROUP BY place, share_id HAVING MAX(at) <= ?1 ORDER BY MIN(at)",
        )?;
        let shares: Vec<(String, String)> = statement
            .query_map(params![now.saturating_sub(quiet_ms) as i64], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<Result<_, _>>()?;
        let mut authors = conn.prepare(
            "SELECT author FROM pending_digests \
             WHERE place = ?1 AND share_id = ?2 AND notified_at IS NULL ORDER BY at, rowid",
        )?;
        let mut due = Vec::new();
        for (place, share_id) in shares {
            let names: Vec<String> = authors
                .query_map(params![place, share_id], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            let mut each: Vec<String> = Vec::new();
            for name in &names {
                if !each.contains(name) {
                    each.push(name.clone());
                }
            }
            due.push(DueDigest {
                place: if place == "design" {
                    MentionPlace::Design
                } else {
                    MentionPlace::Workspace
                },
                share_id,
                count: names.len() as u32,
                authors: each,
            });
        }
        Ok(due)
    }

    /// The share's waiting comments were told to its owner.
    pub fn digested(&self, place: MentionPlace, share_id: &str, now: u64) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE pending_digests SET notified_at = ?3 \
             WHERE place = ?1 AND share_id = ?2 AND notified_at IS NULL",
            params![place_name(place), share_id, now as i64],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted(id: &str, author: &str) -> WantedDigest {
        WantedDigest {
            comment_id: id.into(),
            author: author.into(),
        }
    }

    #[test]
    fn a_share_is_due_once_its_latest_comment_is_old_enough_and_only_until_it_is_told() {
        let store = Store::open_in_memory().unwrap();
        let place = MentionPlace::Design;
        store
            .sync_digests(place, "s1", &[wanted("c1", "Alice")], 1_000)
            .unwrap();
        store
            .sync_digests(
                place,
                "s1",
                &[
                    wanted("c1", "Alice"),
                    wanted("c2", "Bob"),
                    wanted("c3", "Alice"),
                ],
                5_000,
            )
            .unwrap();
        // c1 is old enough, but c2 and c3 restart the quiet.
        assert!(store.due_digests(6_000, 2_000).unwrap().is_empty());
        let due = store.due_digests(7_000, 2_000).unwrap();
        assert_eq!(
            due,
            [DueDigest {
                place,
                share_id: "s1".into(),
                count: 3,
                authors: vec!["Alice".into(), "Bob".into()],
            }]
        );
        store.digested(place, "s1", 7_000).unwrap();
        assert!(store.due_digests(99_000, 2_000).unwrap().is_empty());
        // Syncing the same comments again does not make them wait again.
        store
            .sync_digests(
                place,
                "s1",
                &[
                    wanted("c1", "Alice"),
                    wanted("c2", "Bob"),
                    wanted("c3", "Alice"),
                ],
                9_000,
            )
            .unwrap();
        assert!(store.due_digests(99_000, 2_000).unwrap().is_empty());
    }

    #[test]
    fn a_comment_no_longer_wanted_stops_waiting() {
        let store = Store::open_in_memory().unwrap();
        let place = MentionPlace::Workspace;
        store
            .sync_digests(place, "w1", &[wanted("c1", "Alice")], 1_000)
            .unwrap();
        store.sync_digests(place, "w1", &[], 2_000).unwrap();
        assert!(store.due_digests(99_000, 0).unwrap().is_empty());
        store
            .sync_digests(place, "w1", &[wanted("c2", "Bob")], 3_000)
            .unwrap();
        store.drop_digests(place, "w1").unwrap();
        assert!(store.due_digests(99_000, 0).unwrap().is_empty());
    }
}
