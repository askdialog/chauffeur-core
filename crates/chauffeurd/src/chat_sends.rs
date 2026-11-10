//! Durable receipts for client-identified sends. An uncertain handoff is never replayed:
//! a retry either confirms acceptance or asks the user to inspect the conversation.

use chauffeur_protocol::client::SendParams;
use chauffeur_protocol::model::ChatInfo;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Daemon, Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub id: String,
    pub fingerprint: String,
    pub accepted: bool,
}

pub async fn send(daemon: &Daemon, params: &SendParams) -> Result<ChatInfo> {
    let Some(id) = params.client_id.as_deref() else {
        return daemon
            .chats
            .send_with(
                &params.chat,
                &params.text,
                params.images.as_deref().unwrap_or_default(),
                params.mode.unwrap_or_default(),
            )
            .await;
    };
    uuid::Uuid::parse_str(id)
        .map_err(|_| Error::BadRequest("The send identifier must be a UUID".into()))?;
    let chat = daemon.chats.resolve(&params.chat)?;
    let _admission = daemon.execution.admit(&chat.workspace_id)?;
    let payload = serde_json::to_vec(&(
        &params.text,
        &params.images,
        params.mode.unwrap_or_default(),
    ))?;
    let fingerprint = hex::encode(Sha256::digest(payload));
    if let Some(receipt) = claim(&daemon.store, &chat.id, id, &fingerprint)? {
        if receipt.fingerprint != fingerprint {
            return Err(Error::Conflict(
                "This send identifier belongs to a different message; create a new send".into(),
            ));
        }
        if !receipt.accepted {
            return Err(Error::Conflict("This message's confirmation is unknown; inspect the conversation before sending a new message".into()));
        }
        return daemon.chats.resolve(&chat.id);
    }
    let info = daemon
        .chats
        .send_with(
            &chat.id,
            &params.text,
            params.images.as_deref().unwrap_or_default(),
            params.mode.unwrap_or_default(),
        )
        .await?;
    daemon.store.durable(|connection| {
        connection.execute(
            "UPDATE chat_send_receipts SET accepted=1 WHERE chat_id=?1 AND id=?2",
            params![chat.id, id],
        )?;
        Ok(())
    })?;
    Ok(info)
}

fn claim(
    store: &crate::store::Store,
    chat: &str,
    id: &str,
    fingerprint: &str,
) -> Result<Option<Receipt>> {
    store.durable(|connection| {
        let existing = connection.query_row(
            "SELECT id,fingerprint,accepted FROM chat_send_receipts WHERE chat_id=?1 AND id=?2",
            params![chat, id], |row| Ok(Receipt { id: row.get(0)?, fingerprint: row.get(1)?, accepted: row.get(2)? }),
        ).optional()?;
        if existing.is_none() {
            connection.execute("INSERT INTO chat_send_receipts(chat_id,id,fingerprint,accepted) VALUES(?1,?2,?3,0)", params![chat,id,fingerprint])?;
        }
        Ok(existing)
    })
}

pub fn receipts(store: &crate::store::Store, chat: &str) -> Result<Vec<Receipt>> {
    store.with(|connection| {
        let mut query = connection.prepare(
            "SELECT id,fingerprint,accepted FROM chat_send_receipts WHERE chat_id=?1 ORDER BY id",
        )?;
        query
            .query_map([chat], |row| {
                Ok(Receipt {
                    id: row.get(0)?,
                    fingerprint: row.get(1)?,
                    accepted: row.get(2)?,
                })
            })?
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claim_is_durable_and_an_uncertain_send_is_never_claimed_again() {
        let store = crate::store::Store::open_in_memory().unwrap();
        assert!(claim(&store, "chat", "send", "content").unwrap().is_none());
        let receipt = claim(&store, "chat", "send", "changed").unwrap().unwrap();
        assert_eq!(receipt.fingerprint, "content");
        assert!(!receipt.accepted);
        assert_eq!(receipts(&store, "chat").unwrap(), vec![receipt]);
        assert!(
            claim(&store, "another-chat", "send", "content")
                .unwrap()
                .is_none()
        );
    }
}
