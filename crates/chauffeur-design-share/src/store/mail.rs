//! What the service keeps of its e-mails (mail.rs, sign_in.rs): each one sent, for the limits;
//! the sign-in links, by the hash of their token only; the addresses that stopped Chauffeur's
//! e-mails.

use rusqlite::{OptionalExtension, params};

use super::Store;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS emails_sent (
    kind TEXT NOT NULL,
    owner_email TEXT,
    address TEXT NOT NULL,
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS emails_sent_owner ON emails_sent (kind, owner_email, at);
CREATE INDEX IF NOT EXISTS emails_sent_address ON emails_sent (kind, address, at);
CREATE TABLE IF NOT EXISTS sign_in_links (
    token_hash TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    used_at INTEGER
);
CREATE TABLE IF NOT EXISTS blocked_addresses (
    email TEXT PRIMARY KEY,
    at INTEGER NOT NULL
);
";

/// What an e-mail of the service was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmailKind {
    /// An owner's share named the address.
    Invitation,
    /// The address asked for a sign-in link.
    SignInLink,
}

impl EmailKind {
    fn name(self) -> &'static str {
        match self {
            EmailKind::Invitation => "invitation",
            EmailKind::SignInLink => "sign_in_link",
        }
    }
}

impl Store {
    /// Records an e-mail handed to the mailer; `owner` is the share's owner for an invitation.
    pub fn log_email(
        &self,
        kind: EmailKind,
        owner: Option<&str>,
        address: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO emails_sent (kind, owner_email, address, at) VALUES (?1, ?2, ?3, ?4)",
            params![kind.name(), owner, address, now as i64],
        )?;
        Ok(())
    }

    /// The invitations an owner's shares sent since `since`.
    pub fn invitations_since(&self, owner: &str, since: u64) -> anyhow::Result<u32> {
        Ok(self.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM emails_sent WHERE kind = ?1 AND owner_email = ?2 AND at > ?3",
            params![EmailKind::Invitation.name(), owner, since as i64],
            |row| row.get(0),
        )?)
    }

    /// The sign-in links sent to an address since `since`.
    pub fn sign_in_links_since(&self, address: &str, since: u64) -> anyhow::Result<u32> {
        Ok(self.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM emails_sent WHERE kind = ?1 AND address = ?2 AND at > ?3",
            params![EmailKind::SignInLink.name(), address, since as i64],
            |row| row.get(0),
        )?)
    }

    pub fn block_address(&self, email: &str, now: u64) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO blocked_addresses (email, at) VALUES (?1, ?2)",
            params![email, now as i64],
        )?;
        Ok(())
    }

    /// Whether the address stopped Chauffeur's e-mails.
    pub fn blocked(&self, email: &str) -> anyhow::Result<bool> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .prepare("SELECT 1 FROM blocked_addresses WHERE email = ?1")?
            .exists(params![email])?)
    }

    pub fn add_sign_in_link(&self, token_hash: &str, email: &str, now: u64) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO sign_in_links (token_hash, email, created_at) VALUES (?1, ?2, ?3)",
            params![token_hash, email, now as i64],
        )?;
        Ok(())
    }

    /// Uses a sign-in link made after `made_after` and never used: its e-mail, once; none for a
    /// link unknown, used or too old.
    pub fn use_sign_in_link(
        &self,
        token_hash: &str,
        made_after: u64,
        now: u64,
    ) -> anyhow::Result<Option<String>> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let email: Option<String> = tx
            .query_row(
                "SELECT email FROM sign_in_links WHERE token_hash = ?1 AND used_at IS NULL AND created_at > ?2",
                params![token_hash, made_after as i64],
                |row| row.get(0),
            )
            .optional()?;
        if email.is_some() {
            tx.execute(
                "UPDATE sign_in_links SET used_at = ?2 WHERE token_hash = ?1",
                params![token_hash, now as i64],
            )?;
        }
        tx.commit()?;
        Ok(email)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sign_in_link_signs_in_once_and_only_while_it_is_fresh() {
        let store = Store::open_in_memory().unwrap();
        store
            .add_sign_in_link("h1", "bob@gmail.com", 1_000)
            .unwrap();
        store
            .add_sign_in_link("h2", "bob@gmail.com", 1_000)
            .unwrap();
        assert_eq!(store.use_sign_in_link("h1", 1_000, 2_000).unwrap(), None);
        assert_eq!(
            store.use_sign_in_link("h1", 999, 2_000).unwrap().as_deref(),
            Some("bob@gmail.com")
        );
        assert_eq!(store.use_sign_in_link("h1", 999, 2_001).unwrap(), None);
        assert_eq!(store.use_sign_in_link("nope", 0, 2_000).unwrap(), None);
    }

    #[test]
    fn e_mails_are_counted_by_owner_for_invitations_and_by_address_for_links() {
        let store = Store::open_in_memory().unwrap();
        for at in [10, 20, 30] {
            store
                .log_email(EmailKind::Invitation, Some("carol@x.com"), "a@y.com", at)
                .unwrap();
        }
        store
            .log_email(EmailKind::SignInLink, None, "a@y.com", 25)
            .unwrap();
        assert_eq!(store.invitations_since("carol@x.com", 15).unwrap(), 2);
        assert_eq!(store.invitations_since("dave@x.com", 0).unwrap(), 0);
        assert_eq!(store.sign_in_links_since("a@y.com", 0).unwrap(), 1);
        assert_eq!(store.sign_in_links_since("a@y.com", 25).unwrap(), 0);
    }

    #[test]
    fn a_blocked_address_stays_blocked() {
        let store = Store::open_in_memory().unwrap();
        assert!(!store.blocked("a@y.com").unwrap());
        store.block_address("a@y.com", 1).unwrap();
        store.block_address("a@y.com", 2).unwrap();
        assert!(store.blocked("a@y.com").unwrap());
        assert!(!store.blocked("b@y.com").unwrap());
    }
}
