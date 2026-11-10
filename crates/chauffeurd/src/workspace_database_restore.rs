//! Destination database effects stay inside an exact completed workspace quarantine.
//! Dropped/failed attempts remain pending; neither retries nor successful effects release it.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    Error, Result, cloud_workspace_archive, cloud_workspace_handoff::State,
    cloud_workspace_restore::Restored, store::Store,
};

const MAX_QUARANTINE_RECEIPT: i64 = 64 * 1024;
const MAX_RECORD: usize = 512 * 1024;

fn changed() -> Error {
    Error::Conflict("Database restoration is uncertain or its workspace quarantine changed; retain all data and reconcile the destination before retrying".into())
}
fn bounded(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_RECORD
}
fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 8192
        && !value.contains('\0')
        && !value.split('/').any(|p| matches!(p, "." | ".."))
}

#[derive(Serialize)]
struct Quarantine {
    published: State,
    target: String,
    operation: String,
    snapshot: String,
    native: String,
    receipt: String,
    workspace_path: String,
}
struct RecordedQuarantine {
    operation: Option<String>,
    snapshot: Option<String>,
    native: Option<String>,
    receipt: Option<String>,
}
fn quarantine(conn: &Connection, published: &State, target: &str) -> Result<String> {
    let context = cloud_workspace_archive::transfer_context(published).map_err(|_| changed())?;
    if target != context.target_peer_id {
        return Err(changed());
    }
    let workspace = &context.workspace_id;
    let row = conn
        .query_row(
            "SELECT CASE WHEN length(CAST(operation_id AS BLOB))<=36 THEN operation_id END,
            CASE WHEN length(CAST(snapshot_digest AS BLOB))=43 THEN snapshot_digest END,
            CASE WHEN length(CAST(native_root AS BLOB))<=8192 THEN native_root END,
            CASE WHEN length(CAST(receipt AS BLOB))<=?2 THEN receipt END
         FROM workspace_restores WHERE workspace_id=?1",
            params![workspace, MAX_QUARANTINE_RECEIPT],
            |r| {
                Ok(RecordedQuarantine {
                    operation: r.get(0)?,
                    snapshot: r.get(1)?,
                    native: r.get(2)?,
                    receipt: r.get(3)?,
                })
            },
        )
        .optional()?;
    let Some(RecordedQuarantine {
        operation: Some(operation),
        snapshot: Some(snapshot),
        native: Some(native),
        receipt: Some(raw),
    }) = row
    else {
        return Err(changed());
    };
    let state: Option<(bool,Option<String>)> = conn.query_row(
        "SELECT state='ready',CASE WHEN length(CAST(path AS BLOB))<=8192 THEN path END FROM workspaces WHERE id=?1",
        [workspace], |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional()?;
    let Some((state, Some(workspace_path))) = state else {
        return Err(changed());
    };
    let restored: Restored = serde_json::from_str(&raw).map_err(|_| changed())?;
    let receipt = &restored.receipt;
    if !state
        || operation != context.operation_id
        || snapshot != context.snapshot_digest
        || !path(&native)
        || !path(&workspace_path)
        || receipt.version != 1
        || receipt.workspace_id != *workspace
        || receipt.operation_id != operation
        || receipt.source_epoch != context.source_epoch
        || receipt.target_peer_id != target
        || receipt.snapshot_digest != snapshot
        || receipt.native_root != native
        || receipt.workspace_path != workspace_path
        || restored.digest != URL_SAFE_NO_PAD.encode(Sha256::digest(serde_json::to_vec(receipt)?))
        || serde_json::to_string(&restored)? != raw
    {
        return Err(changed());
    }
    Ok(serde_json::to_string(&Quarantine {
        published: published.clone(),
        target: target.into(),
        operation,
        snapshot,
        native,
        receipt: raw,
        workspace_path,
    })?)
}

pub(crate) struct Attempt {
    store: Store,
    id: String,
    published: State,
    target: String,
    export: String,
    quarantine: String,
    plan: String,
}
impl Attempt {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    fn check(&self, conn: &Connection) -> Result<()> {
        let workspace = &self.published.binding.workspace_id;
        if quarantine(conn, &self.published, &self.target)? != self.quarantine {
            return Err(changed());
        }
        let row: Option<(Option<String>, Option<String>, bool)> = conn
            .query_row(
                "SELECT CASE WHEN length(CAST(quarantine AS BLOB))<=?3 THEN quarantine END,
                CASE WHEN length(CAST(plan AS BLOB))<=?3 THEN plan END, receipt IS NULL
             FROM workspace_database_restores WHERE attempt_id=?1 AND workspace_id=?2
                AND operation_id=?4 AND export_name=?5",
                params![
                    self.id,
                    workspace,
                    MAX_RECORD as i64,
                    self.published
                        .transfer
                        .as_ref()
                        .ok_or_else(changed)?
                        .operation_id,
                    self.export
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if row != Some((Some(self.quarantine.clone()), Some(self.plan.clone()), true)) {
            return Err(changed());
        }
        Ok(())
    }
    pub(crate) fn require_quarantined(&self) -> Result<()> {
        self.store.durable(|c| {
            let tx = c.transaction()?;
            self.check(&tx)?;
            tx.commit()?;
            Ok(())
        })
    }
    /// The receipt records completed effects only; the workspace remains quarantined.
    pub(crate) fn finish(self, receipt: &str) -> Result<()> {
        if !bounded(receipt) {
            return Err(changed());
        }
        self.store.durable(|c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            self.check(&tx)?;
            let changed_rows = tx.execute("UPDATE workspace_database_restores SET receipt=?2 WHERE attempt_id=?1 AND receipt IS NULL",params![self.id,receipt])?;
            if changed_rows != 1 { return Err(changed()); }
            tx.commit()?;
            Ok(())
        })
    }
}

/// The caller authenticates the published journal and pinned local destination peer. Reserve
/// before any database effect; no existing attempt, completed or pending, is adopted or rerun.
pub(crate) fn reserve(
    store: &Store,
    published: &State,
    target: &str,
    export: &str,
    plan: &str,
) -> Result<Attempt> {
    if !name(export) || !bounded(plan) {
        return Err(changed());
    }
    store.durable(|c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let selected = quarantine(&tx,published,target)?;
        let workspace = &published.binding.workspace_id;
        let existing: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM workspace_database_restores WHERE workspace_id=?1 AND (export_name=?2 OR receipt IS NULL))",params![workspace,export],|r|r.get(0))?;
        if existing { return Err(changed()); }
        let id = uuid::Uuid::now_v7().to_string();
        let operation = &published.transfer.as_ref().ok_or_else(changed)?.operation_id;
        tx.execute("INSERT INTO workspace_database_restores(attempt_id,workspace_id,operation_id,export_name,quarantine,plan) VALUES(?1,?2,?3,?4,?5,?6)",params![id,workspace,operation,export,selected,plan])?;
        tx.commit()?;
        Ok(Attempt {store:store.clone(),id,published:published.clone(),target:target.into(),export:export.into(),quarantine:selected,plan:plan.into()})
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cloud_workspace_handoff::{Binding, Phase, Transfer},
        cloud_workspace_restore::Receipt,
        workspace_execution::ExecutionGate,
    };
    use std::collections::BTreeMap;

    fn fixture() -> (Store, State, String) {
        let peer = |n: char| format!("12D3KooW{}", n.to_string().repeat(44));
        let hash = |n: u8| URL_SAFE_NO_PAD.encode([n; 32]);
        let state = State {
            version: 1,
            binding: Binding {
                workspace_id: uuid::Uuid::now_v7().to_string(),
                owner_peer_id: peer('1'),
                controller_peer_id: peer('2'),
            },
            revision: 5,
            epoch: 1,
            execution_peer_id: peer('1'),
            transfer: Some(Transfer {
                operation_id: uuid::Uuid::now_v7().to_string(),
                source_peer_id: peer('1'),
                target_peer_id: peer('3'),
                source_epoch: 1,
                phase: Phase::SnapshotReady,
                source_stop_digest: Some(hash(1)),
                snapshot_digest: Some(hash(2)),
                destination_receipt_digest: None,
            }),
        };
        let target = peer('3');
        let t = state.transfer.as_ref().unwrap();
        let receipt = Receipt {
            version: 1,
            workspace_id: state.binding.workspace_id.clone(),
            operation_id: t.operation_id.clone(),
            source_epoch: 1,
            target_peer_id: target.clone(),
            snapshot_digest: t.snapshot_digest.clone().unwrap(),
            workspace_path: "/fixture/workspace".into(),
            native_root: "/fixture/native".into(),
            blocked_chats: BTreeMap::new(),
        };
        let restored = Restored {
            digest: URL_SAFE_NO_PAD.encode(Sha256::digest(serde_json::to_vec(&receipt).unwrap())),
            receipt,
            kept_copy: None,
            returned: false,
        };
        let store = Store::open_in_memory().unwrap();
        store.with(|c| {
            c.execute("INSERT INTO workspaces(id,slug,path,branch,state,created_at) VALUES(?1,?1,'/fixture/workspace','fixture','ready','fixture')",[&state.binding.workspace_id])?;
            c.execute("INSERT INTO workspace_restores(workspace_id,operation_id,snapshot_digest,native_root,receipt) VALUES(?1,?2,?3,'/fixture/native',?4)",params![state.binding.workspace_id,t.operation_id,t.snapshot_digest,serde_json::to_string(&restored).unwrap()])?;
            Ok(())
        }).unwrap();
        (store, state, target)
    }
    fn count(store: &Store) -> i64 {
        store
            .with(|c| {
                c.query_row(
                    "SELECT count(*) FROM workspace_database_restores",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap()
    }

    #[test]
    fn completed_database_effects_never_release_the_workspace_or_allow_the_same_export_again() {
        let (store, state, target) = fixture();
        let attempt = reserve(&store, &state, &target, "main", "{\"plan\":1}").unwrap();
        let id = attempt.id.clone();
        attempt.require_quarantined().unwrap();
        assert!(reserve(&store, &state, &target, "different", "plan").is_err());
        attempt.finish("{\"completed\":true}").unwrap();
        let receipt: String = store
            .with(|c| {
                c.query_row(
                    "SELECT receipt FROM workspace_database_restores WHERE attempt_id=?1",
                    [id],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(receipt, "{\"completed\":true}");
        assert!(
            ExecutionGate::new(store.clone())
                .admit(&state.binding.workspace_id)
                .is_err()
        );
        assert!(reserve(&store, &state, &target, "main", "plan").is_err());
        assert!(reserve(&store, &state, &target, "different", "plan").is_ok());
    }

    #[test]
    fn incomplete_corrupt_oversized_and_changed_quarantines_refuse_before_reservation() {
        let mutations = [
            "DELETE FROM workspace_restores",
            "UPDATE workspace_restores SET receipt=NULL",
            "UPDATE workspace_restores SET receipt='{}'",
            "UPDATE workspace_restores SET operation_id='changed'",
            "UPDATE workspace_restores SET snapshot_digest='changed'",
            "UPDATE workspace_restores SET native_root='/different'",
            "UPDATE workspaces SET path='/different'",
            "UPDATE workspaces SET state='creating'",
        ];
        for mutation in mutations {
            let (store, state, target) = fixture();
            store.with(|c| c.execute_batch(mutation)).unwrap();
            assert!(
                reserve(&store, &state, &target, "main", "plan").is_err(),
                "{mutation}"
            );
            assert_eq!(count(&store), 0);
        }
        let (store, state, target) = fixture();
        store
            .with(|c| {
                c.execute(
                    "UPDATE workspace_restores SET receipt=?1",
                    ["é".repeat(MAX_QUARANTINE_RECEIPT as usize / 2 + 1)],
                )
            })
            .unwrap();
        assert!(reserve(&store, &state, &target, "main", "plan").is_err());
        assert_eq!(count(&store), 0);
    }

    #[test]
    fn validly_hashed_receipts_still_need_the_exact_epoch_target_and_source_binding() {
        for mutation in 0..5 {
            let (store, state, target) = fixture();
            let raw: String = store
                .with(|c| c.query_row("SELECT receipt FROM workspace_restores", [], |r| r.get(0)))
                .unwrap();
            let mut restored: Restored = serde_json::from_str(&raw).unwrap();
            match mutation {
                0 => restored.receipt.source_epoch += 1,
                1 => restored.receipt.target_peer_id = state.binding.owner_peer_id.clone(),
                2 => restored.receipt.workspace_id = uuid::Uuid::now_v7().to_string(),
                3 => restored.receipt.version = 2,
                _ => restored.digest = "incorrect".into(),
            }
            if mutation != 4 {
                restored.digest = URL_SAFE_NO_PAD.encode(Sha256::digest(
                    serde_json::to_vec(&restored.receipt).unwrap(),
                ));
            }
            store
                .with(|c| {
                    c.execute(
                        "UPDATE workspace_restores SET receipt=?1",
                        [serde_json::to_string(&restored).unwrap()],
                    )
                })
                .unwrap();
            assert!(reserve(&store, &state, &target, "main", "plan").is_err());
        }
        let (store, state, target) = fixture();
        assert!(reserve(&store, &state, &state.binding.owner_peer_id, "main", "plan").is_err());
        let mut stopping = state.clone();
        let t = stopping.transfer.as_mut().unwrap();
        t.phase = Phase::Stopping;
        t.source_stop_digest = None;
        t.snapshot_digest = None;
        assert!(reserve(&store, &stopping, &target, "main", "plan").is_err());
    }

    #[test]
    fn pending_attempts_survive_lost_ownership_and_block_both_retries_and_other_exports() {
        let (store, state, target) = fixture();
        drop(reserve(&store, &state, &target, "main", "plan").unwrap());
        let reopened = store.clone();
        for name in ["main", "other"] {
            assert!(reserve(&reopened, &state, &target, name, "plan").is_err());
        }
        let pending: i64 = store
            .with(|c| {
                c.query_row(
                    "SELECT count(*) FROM workspace_database_restores WHERE receipt IS NULL",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(pending, 1);
    }

    #[test]
    fn changed_or_replaced_attempts_cannot_be_finished_by_stale_owned_guards() {
        for mutation in [
            "UPDATE workspace_database_restores SET plan='changed'",
            "UPDATE workspace_database_restores SET quarantine='changed'",
            "UPDATE workspace_database_restores SET export_name='other'",
            "UPDATE workspace_restores SET receipt=NULL",
        ] {
            let (store, state, target) = fixture();
            let attempt = reserve(&store, &state, &target, "main", "plan").unwrap();
            store.with(|c| c.execute_batch(mutation)).unwrap();
            assert!(attempt.require_quarantined().is_err());
            assert!(attempt.finish("done").is_err());
        }
        let (store, state, target) = fixture();
        let old = reserve(&store, &state, &target, "main", "plan").unwrap();
        store
            .with(|c| c.execute("DELETE FROM workspace_database_restores", []))
            .unwrap();
        let current = reserve(&store, &state, &target, "main", "new-plan").unwrap();
        assert!(old.finish("old-completion").is_err());
        current.require_quarantined().unwrap();
    }

    #[test]
    fn concurrent_reservations_have_one_winner_and_records_have_bounded_sizes() {
        let (store, state, target) = fixture();
        assert!(reserve(&store, &state, &target, "../escape", "plan").is_err());
        assert!(reserve(&store, &state, &target, "main", &"x".repeat(MAX_RECORD + 1)).is_err());
        std::thread::scope(|scope| {
            let a = scope.spawn(|| reserve(&store, &state, &target, "first", "plan"));
            let b = scope.spawn(|| reserve(&store, &state, &target, "second", "plan"));
            assert_ne!(a.join().unwrap().is_ok(), b.join().unwrap().is_ok());
        });
        assert_eq!(count(&store), 1);
        let (store, state, target) = fixture();
        let attempt = reserve(&store, &state, &target, "main", "plan").unwrap();
        assert!(attempt.finish(&"x".repeat(MAX_RECORD + 1)).is_err());
        assert!(reserve(&store, &state, &target, "main", "plan").is_err());
    }
}
