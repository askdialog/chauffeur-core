//! Durable controller-side handoff metadata, not an execution lease or a process fence.
//! Trusted callers must authenticate actors and validate signed receipts and full artifacts
//! before recording their digests. No RPC, host launch guard or automatic failover is installed.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result, cloud_vault::CloudVaultHeads};
const MAX: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub workspace_id: String,
    pub owner_peer_id: String,
    pub controller_peer_id: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Preparing,
    Stopping,
    Stopped,
    SnapshotReady,
    Restored,
    Committed,
    Cancelled,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Transfer {
    pub operation_id: String,
    pub source_peer_id: String,
    pub target_peer_id: String,
    pub source_epoch: u64,
    pub phase: Phase,
    pub source_stop_digest: Option<String>,
    pub snapshot_digest: Option<String>,
    pub destination_receipt_digest: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub version: u8,
    pub binding: Binding,
    pub revision: u64,
    pub epoch: u64,
    pub execution_peer_id: String,
    pub transfer: Option<Transfer>,
}
impl State {
    pub(crate) fn validate(&self) -> Result<()> {
        check(self)
    }
    /// Cancelling departures advances the journal without changing the stay established by a
    /// commit. In-flight transfers and a later ownership epoch never continue that stay.
    pub(crate) fn continues_execution_of(&self, committed: &State) -> bool {
        committed.transfer.as_ref().map(|t| t.phase) == Some(Phase::Committed)
            && (self == committed
                || (self.binding == committed.binding
                    && self.epoch == committed.epoch
                    && self.execution_peer_id == committed.execution_peer_id
                    && self.revision > committed.revision
                    && self.transfer.as_ref().is_some_and(|t| {
                        t.phase == Phase::Cancelled
                            && t.source_epoch == committed.epoch
                            && t.source_peer_id == committed.execution_peer_id
                    })))
    }
    /// Planning metadata only: a real host must also enforce its durable launch fence,
    /// authenticated ownership and complete context. Missing/unreadable state is not local.
    pub fn recorded_execution_peer(&self) -> Option<&str> {
        match self.transfer.as_ref().map(|t| t.phase) {
            None | Some(Phase::Preparing | Phase::Committed | Phase::Cancelled) => {
                Some(&self.execution_peer_id)
            }
            _ => None,
        }
    }
}
#[derive(Clone, Debug)]
pub enum Action {
    BeginStop,
    SourceStopped {
        peer_id: String,
        receipt_digest: String,
    },
    SnapshotPublished {
        peer_id: String,
        snapshot_digest: String,
    },
    DestinationRestored {
        peer_id: String,
        snapshot_digest: String,
        receipt_digest: String,
    },
    Commit {
        snapshot_digest: String,
        destination_receipt_digest: String,
    },
    Cancel,
}
fn conflict() -> Error {
    Error::Conflict(
        "Workspace handoff changed; inspect retained state before another action".into(),
    )
}
fn invalid() -> Error {
    Error::BadRequest("Invalid workspace handoff binding, evidence or transition".into())
}
pub(crate) fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}
pub(crate) fn peer(value: &str) -> bool {
    value.starts_with("12D3KooW")
        && value.len() == 52
        && value
            .bytes()
            .all(|b| b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz".contains(&b))
}
pub(crate) fn digest(value: &str) -> bool {
    value.len() == 43
        && URL_SAFE_NO_PAD
            .decode(value)
            .is_ok_and(|b| b.len() == 32 && URL_SAFE_NO_PAD.encode(b) == value)
}
fn binding(value: &Binding) -> Result<()> {
    if !uuid(&value.workspace_id)
        || !peer(&value.owner_peer_id)
        || !peer(&value.controller_peer_id)
        || value.owner_peer_id == value.controller_peer_id
    {
        return Err(invalid());
    }
    Ok(())
}
fn check(s: &State) -> Result<()> {
    binding(&s.binding)?;
    if s.version != 1
        || s.revision == 0
        || s.revision > MAX
        || s.epoch == 0
        || s.epoch > MAX
        || !peer(&s.execution_peer_id)
        || s.execution_peer_id == s.binding.controller_peer_id
    {
        return Err(invalid());
    }
    if let Some(t) = &s.transfer {
        if !uuid(&t.operation_id)
            || !peer(&t.source_peer_id)
            || !peer(&t.target_peer_id)
            || t.source_peer_id == t.target_peer_id
            || t.source_peer_id == s.binding.controller_peer_id
            || t.target_peer_id == s.binding.controller_peer_id
            || t.source_epoch == 0
            || t.source_epoch >= MAX
        {
            return Err(invalid());
        }
        let evidence = [
            &t.source_stop_digest,
            &t.snapshot_digest,
            &t.destination_receipt_digest,
        ];
        if evidence
            .iter()
            .any(|d| d.as_ref().is_some_and(|v| !digest(v)))
        {
            return Err(invalid());
        }
        let mask = evidence.map(|v| v.is_some());
        let valid = match t.phase {
            Phase::Preparing | Phase::Stopping => mask == [false, false, false],
            Phase::Stopped => mask == [true, false, false],
            Phase::SnapshotReady => mask == [true, true, false],
            Phase::Restored | Phase::Committed => mask == [true, true, true],
            Phase::Cancelled => matches!(
                mask,
                [false, false, false]
                    | [true, false, false]
                    | [true, true, false]
                    | [true, true, true]
            ),
        };
        let (holder, epoch) = if t.phase == Phase::Committed {
            (&t.target_peer_id, t.source_epoch + 1)
        } else {
            (&t.source_peer_id, t.source_epoch)
        };
        if !valid || &s.execution_peer_id != holder || s.epoch != epoch {
            return Err(invalid());
        }
    } else if s.epoch != 1 || s.revision != 1 {
        return Err(invalid());
    }
    Ok(())
}
pub(crate) fn load(conn: &Connection, expected: &Binding) -> Result<Option<State>> {
    binding(expected)?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT state FROM workspace_handoffs WHERE workspace_id=?1",
            [&expected.workspace_id],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|raw| {
        if raw.len() > 16 * 1024 { return Err(invalid()); }
        let state: State = serde_json::from_str(&raw)?; check(&state)?;
        if &state.binding != expected { return Err(conflict()); }
        let latest: Option<i64> = conn.query_row("SELECT MAX(revision) FROM workspace_handoff_operations WHERE workspace_id=?1", [&state.binding.workspace_id], |r| r.get(0))?;
        if latest != state.transfer.as_ref().map(|_| state.revision as i64) { return Err(Error::Unavailable("Workspace handoff revision history differs; explicit recovery required".into())); }
        if let Some(t) = &state.transfer {
            let history: Option<String> = conn.query_row(
                "SELECT state FROM workspace_handoff_operations WHERE operation_id=?1 AND workspace_id=?2 AND source_epoch=?3",
                params![t.operation_id, state.binding.workspace_id, t.source_epoch as i64], |r| r.get(0)).optional()?;
            let history = history.filter(|raw| raw.len() <= 16 * 1024).map(|raw| serde_json::from_str::<State>(&raw)).transpose()?;
            if history.as_ref() != Some(&state) { return Err(Error::Unavailable("Workspace handoff history missing or inconsistent; explicit recovery required".into())); }

        }
        Ok(state)
    }).transpose()
}
pub(crate) fn require_root(conn: &Connection) -> Result<()> {
    let pinned: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM endpoint_root)", [], |r| {
        r.get(0)
    })?;
    if !pinned {
        return Err(Error::Unavailable(
            "Protected endpoint root missing; explicit recovery required".into(),
        ));
    }
    Ok(())
}
fn save(conn: &Connection, s: &State) -> Result<()> {
    check(s)?;
    let raw = serde_json::to_string(s)?;
    conn.execute(
        "UPDATE workspace_handoffs SET state=?2 WHERE workspace_id=?1",
        params![s.binding.workspace_id, raw],
    )?;
    if let Some(t) = &s.transfer
        && conn.execute(
            "UPDATE workspace_handoff_operations SET state=?2, revision=?3 WHERE operation_id=?1",
            params![t.operation_id, raw, s.revision as i64],
        )? != 1
    {
        return Err(conflict());
    }
    Ok(())
}

pub fn read(store: &CloudVaultHeads, expected: &Binding) -> Result<Option<State>> {
    let conn = store.connect()?;
    let tx = conn.unchecked_transaction()?;
    require_root(&tx)?;
    let state = load(&tx, expected)?;
    tx.commit()?;
    Ok(state)
}

/// Retained operation metadata supports reconciliation/cleanup after a later transfer.
pub fn read_operation(
    store: &CloudVaultHeads,
    expected: &Binding,
    operation_id: &str,
) -> Result<Option<State>> {
    binding(expected)?;
    if !uuid(operation_id) {
        return Err(invalid());
    }
    let conn = store.connect()?;
    require_root(&conn)?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT state FROM workspace_handoff_operations WHERE operation_id=?1",
            [operation_id],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|raw| {
        if raw.len() > 16 * 1024 {
            return Err(invalid());
        }
        let state: State = serde_json::from_str(&raw)?;
        check(&state)?;
        if &state.binding != expected
            || state.transfer.as_ref().map(|t| t.operation_id.as_str()) != Some(operation_id)
        {
            return Err(conflict());
        }
        Ok(state)
    })
    .transpose()
}

/// Explicit one-time registration by the trusted controller, never read-time fallback.
/// A missing row is not inferred from a cloud listing, backup or heartbeat.
pub fn initialize(
    store: &CloudVaultHeads,
    expected: Binding,
    source_peer_id: String,
) -> Result<State> {
    binding(&expected)?;
    let state = State {
        version: 1,
        binding: expected,
        revision: 1,
        epoch: 1,
        execution_peer_id: source_peer_id,
        transfer: None,
    };
    check(&state)?;
    let mut conn = store.connect()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    require_root(&tx)?;
    let history: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspace_handoff_operations WHERE workspace_id=?1)",
        [&state.binding.workspace_id],
        |r| r.get(0),
    )?;
    if load(&tx, &state.binding)?.is_some() || history {
        return Err(conflict());
    }
    tx.execute(
        "INSERT INTO workspace_handoffs(workspace_id,state) VALUES(?1,?2)",
        params![state.binding.workspace_id, serde_json::to_string(&state)?],
    )?;
    tx.commit()?;
    Ok(state)
}
/// A stale revision returns None; it never retries a mutation automatically.
pub fn prepare(
    store: &CloudVaultHeads,
    expected: &Binding,
    revision: u64,
    operation_id: String,
    target_peer_id: String,
) -> Result<Option<State>> {
    let mut conn = store.connect()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    require_root(&tx)?;
    let mut state = load(&tx, expected)?.ok_or_else(conflict)?;
    if state.revision != revision {
        return Ok(None);
    }
    if state.revision >= MAX
        || state.epoch >= MAX
        || state
            .transfer
            .as_ref()
            .is_some_and(|t| !matches!(t.phase, Phase::Committed | Phase::Cancelled))
    {
        return Err(conflict());
    }
    if !uuid(&operation_id) || !peer(&target_peer_id) || target_peer_id == state.execution_peer_id {
        return Err(invalid());
    }
    let used: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspace_handoff_operations WHERE operation_id=?1)",
        [&operation_id],
        |r| r.get(0),
    )?;
    if used {
        return Err(conflict());
    }

    state.transfer = Some(Transfer {
        operation_id,
        source_peer_id: state.execution_peer_id.clone(),
        target_peer_id,
        source_epoch: state.epoch,
        phase: Phase::Preparing,
        source_stop_digest: None,
        snapshot_digest: None,
        destination_receipt_digest: None,
    });
    tx.execute("INSERT INTO workspace_handoff_operations(operation_id,workspace_id,source_epoch,revision,state) VALUES(?1,?2,?3,?4,?5)", params![state.transfer.as_ref().ok_or_else(conflict)?.operation_id, expected.workspace_id, state.epoch as i64, state.revision as i64, serde_json::to_string(&state)?])?;
    state.revision += 1;
    save(&tx, &state)?;
    tx.commit()?;
    Ok(Some(state))
}
/// Digests identify already verified, operation-bound evidence. They are not signatures
/// or proof of process termination by themselves; no public method accepts remote claims.
pub fn advance(
    store: &CloudVaultHeads,
    expected: &Binding,
    revision: u64,
    operation_id: &str,
    source_epoch: u64,
    action: Action,
) -> Result<Option<State>> {
    let mut conn = store.connect()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    require_root(&tx)?;
    let mut state = load(&tx, expected)?.ok_or_else(conflict)?;
    if state.revision != revision {
        return Ok(None);
    }
    let t = state.transfer.as_mut().ok_or_else(conflict)?;
    if state.revision >= MAX || t.operation_id != operation_id || t.source_epoch != source_epoch {
        return Err(conflict());
    }
    match action {
        Action::BeginStop if t.phase == Phase::Preparing => t.phase = Phase::Stopping,
        Action::SourceStopped {
            peer_id,
            receipt_digest,
        } if t.phase == Phase::Stopping
            && peer_id == t.source_peer_id
            && digest(&receipt_digest) =>
        {
            t.source_stop_digest = Some(receipt_digest);
            t.phase = Phase::Stopped;
        }
        Action::SnapshotPublished {
            peer_id,
            snapshot_digest,
        } if t.phase == Phase::Stopped
            && peer_id == t.source_peer_id
            && digest(&snapshot_digest) =>
        {
            t.snapshot_digest = Some(snapshot_digest);
            t.phase = Phase::SnapshotReady;
        }
        Action::DestinationRestored {
            peer_id,
            snapshot_digest,
            receipt_digest,
        } if t.phase == Phase::SnapshotReady
            && peer_id == t.target_peer_id
            && t.snapshot_digest.as_ref() == Some(&snapshot_digest)
            && digest(&receipt_digest) =>
        {
            t.destination_receipt_digest = Some(receipt_digest);
            t.phase = Phase::Restored;
        }
        Action::Commit {
            snapshot_digest,
            destination_receipt_digest,
        } if t.phase == Phase::Restored
            && t.snapshot_digest.as_ref() == Some(&snapshot_digest)
            && t.destination_receipt_digest.as_ref() == Some(&destination_receipt_digest) =>
        {
            state.execution_peer_id = t.target_peer_id.clone();
            state.epoch = t.source_epoch + 1;
            t.phase = Phase::Committed;
        }
        Action::Cancel if !matches!(t.phase, Phase::Committed | Phase::Cancelled) => {
            t.phase = Phase::Cancelled
        }
        _ => return Err(invalid()),
    }
    state.revision += 1;
    save(&tx, &state)?;
    tx.commit()?;
    Ok(Some(state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    fn peer_id(c: char) -> String {
        format!("12D3KooW{}", c.to_string().repeat(44))
    }
    fn scope() -> Binding {
        Binding {
            workspace_id: uuid::Uuid::now_v7().to_string(),
            owner_peer_id: peer_id('1'),
            controller_peer_id: peer_id('2'),
        }
    }
    fn hash(n: u8) -> String {
        URL_SAFE_NO_PAD.encode([n; 32])
    }
    fn fixture() -> (tempfile::TempDir, CloudVaultHeads, State) {
        let root = tempfile::tempdir().unwrap();
        let store = CloudVaultHeads::new(root.path());
        store.pin_root(&hash(0)).unwrap();
        let state = initialize(&store, scope(), peer_id('1')).unwrap();
        (root, store, state)
    }
    fn start(store: &CloudVaultHeads, s: &State, target: char) -> State {
        prepare(
            store,
            &s.binding,
            s.revision,
            uuid::Uuid::now_v7().to_string(),
            peer_id(target),
        )
        .unwrap()
        .unwrap()
    }
    fn step(store: &CloudVaultHeads, s: &State, action: Action) -> State {
        let t = s.transfer.as_ref().unwrap();
        advance(
            store,
            &s.binding,
            s.revision,
            &t.operation_id,
            t.source_epoch,
            action,
        )
        .unwrap()
        .unwrap()
    }
    fn stop(store: &CloudVaultHeads, s: &State) -> State {
        let s = step(store, s, Action::BeginStop);
        step(
            store,
            &s,
            Action::SourceStopped {
                peer_id: s.execution_peer_id.clone(),
                receipt_digest: hash(1),
            },
        )
    }
    fn ready(store: &CloudVaultHeads, s: &State) -> State {
        let s = stop(store, s);
        let s = step(
            store,
            &s,
            Action::SnapshotPublished {
                peer_id: s.execution_peer_id.clone(),
                snapshot_digest: hash(2),
            },
        );
        step(
            store,
            &s,
            Action::DestinationRestored {
                peer_id: s.transfer.as_ref().unwrap().target_peer_id.clone(),
                snapshot_digest: hash(2),
                receipt_digest: hash(3),
            },
        )
    }
    fn commit() -> Action {
        Action::Commit {
            snapshot_digest: hash(2),
            destination_receipt_digest: hash(3),
        }
    }
    #[test]
    fn cancelled_departures_continue_the_same_stay_but_other_ownership_does_not() {
        let (_root, store, initial) = fixture();
        let prepared = start(&store, &initial, '3');
        let restored = ready(&store, &prepared);
        let committed = step(&store, &restored, commit());
        assert!(committed.continues_execution_of(&committed));
        let mut current = committed.clone();
        for target in ['1', '4'] {
            let prepared = start(&store, &current, target);
            assert!(!prepared.continues_execution_of(&committed));
            let stopped = stop(&store, &prepared);
            assert!(!stopped.continues_execution_of(&committed));
            current = step(&store, &stopped, Action::Cancel);
            assert!(current.continues_execution_of(&committed));
        }
        for changed in [
            State {
                epoch: current.epoch + 1,
                ..current.clone()
            },
            State {
                execution_peer_id: peer_id('4'),
                ..current.clone()
            },
            State {
                binding: scope(),
                ..current.clone()
            },
            State {
                revision: committed.revision,
                ..current.clone()
            },
        ] {
            assert!(!changed.continues_execution_of(&committed));
        }
        let prepared = start(&store, &current, '1');
        let restored = ready(&store, &prepared);
        let returned = step(&store, &restored, commit());
        assert!(!returned.continues_execution_of(&committed));
    }
    fn rejected(store: &CloudVaultHeads, s: &State, action: Action) {
        let t = s.transfer.as_ref().unwrap();
        assert!(
            advance(
                store,
                &s.binding,
                s.revision,
                &t.operation_id,
                t.source_epoch,
                action
            )
            .is_err()
        );
        assert_eq!(read(store, &s.binding).unwrap(), Some(s.clone()));
    }
    #[test]
    fn durable_ordered_handoff_retains_identity_and_old_operation_after_return_begins() {
        let (root, store, initial) = fixture();
        let prepared = start(&store, &initial, '3');
        assert_eq!(
            prepared.recorded_execution_peer(),
            Some(peer_id('1').as_str())
        );
        let stopping = step(&store, &prepared, Action::BeginStop);
        assert_eq!(stopping.recorded_execution_peer(), None);
        let reopened = CloudVaultHeads::new(root.path());
        assert_eq!(
            read(&reopened, &stopping.binding).unwrap(),
            Some(stopping.clone())
        );
        let stopped = step(
            &reopened,
            &stopping,
            Action::SourceStopped {
                peer_id: peer_id('1'),
                receipt_digest: hash(1),
            },
        );
        let published = step(
            &reopened,
            &stopped,
            Action::SnapshotPublished {
                peer_id: peer_id('1'),
                snapshot_digest: hash(2),
            },
        );
        let restored = step(
            &reopened,
            &published,
            Action::DestinationRestored {
                peer_id: peer_id('3'),
                snapshot_digest: hash(2),
                receipt_digest: hash(3),
            },
        );
        assert_eq!(restored.recorded_execution_peer(), None);
        let committed = step(&reopened, &restored, commit());
        assert_eq!(committed.execution_peer_id, peer_id('3'));
        assert_eq!(committed.epoch, 2);
        rejected(&reopened, &committed, Action::Cancel);
        let returning = start(&reopened, &committed, '1');
        assert_eq!(returning.transfer.as_ref().unwrap().source_epoch, 2);
        assert!(
            returning
                .transfer
                .as_ref()
                .unwrap()
                .source_stop_digest
                .is_none()
        );
        assert_eq!(
            read_operation(
                &reopened,
                &initial.binding,
                &committed.transfer.as_ref().unwrap().operation_id
            )
            .unwrap(),
            Some(committed)
        );
    }
    #[test]
    fn no_phase_skips_or_wrong_actor_snapshot_or_restoration_receipt_can_commit() {
        let (_root, store, initial) = fixture();
        let s = start(&store, &initial, '3');
        rejected(&store, &s, commit());
        rejected(
            &store,
            &s,
            Action::SnapshotPublished {
                peer_id: peer_id('1'),
                snapshot_digest: hash(2),
            },
        );
        let s = step(&store, &s, Action::BeginStop);
        rejected(
            &store,
            &s,
            Action::SourceStopped {
                peer_id: peer_id('3'),
                receipt_digest: hash(1),
            },
        );
        rejected(
            &store,
            &s,
            Action::SourceStopped {
                peer_id: peer_id('1'),
                receipt_digest: "not-a-digest".into(),
            },
        );
        let s = step(
            &store,
            &s,
            Action::SourceStopped {
                peer_id: peer_id('1'),
                receipt_digest: hash(1),
            },
        );
        rejected(&store, &s, commit());
        let s = step(
            &store,
            &s,
            Action::SnapshotPublished {
                peer_id: peer_id('1'),
                snapshot_digest: hash(2),
            },
        );
        rejected(
            &store,
            &s,
            Action::DestinationRestored {
                peer_id: peer_id('1'),
                snapshot_digest: hash(2),
                receipt_digest: hash(3),
            },
        );
        rejected(
            &store,
            &s,
            Action::DestinationRestored {
                peer_id: peer_id('3'),
                snapshot_digest: hash(9),
                receipt_digest: hash(3),
            },
        );
        let s = step(
            &store,
            &s,
            Action::DestinationRestored {
                peer_id: peer_id('3'),
                snapshot_digest: hash(2),
                receipt_digest: hash(3),
            },
        );
        rejected(
            &store,
            &s,
            Action::Commit {
                snapshot_digest: hash(9),
                destination_receipt_digest: hash(3),
            },
        );
        rejected(
            &store,
            &s,
            Action::Commit {
                snapshot_digest: hash(2),
                destination_receipt_digest: hash(9),
            },
        );
    }
    #[test]
    fn lost_commit_response_is_observed_without_replaying_and_old_epoch_is_rejected() {
        let (_root, store, initial) = fixture();
        let prepared = start(&store, &initial, '3');
        let restored = ready(&store, &prepared);
        let t = restored.transfer.as_ref().unwrap();
        let committed = step(&store, &restored, commit());
        assert!(
            advance(
                &store,
                &initial.binding,
                restored.revision,
                &t.operation_id,
                t.source_epoch,
                commit()
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(
            read(&store, &initial.binding).unwrap(),
            Some(committed.clone())
        );
        let returning = start(&store, &committed, '1');
        assert!(
            advance(
                &store,
                &initial.binding,
                returning.revision,
                &t.operation_id,
                t.source_epoch,
                Action::BeginStop
            )
            .is_err()
        );
        let current = returning.transfer.as_ref().unwrap();
        assert!(
            advance(
                &store,
                &initial.binding,
                returning.revision,
                &current.operation_id,
                1,
                Action::BeginStop
            )
            .is_err()
        );
        assert_eq!(read(&store, &initial.binding).unwrap(), Some(returning));
    }
    #[test]
    fn cancellation_before_commit_restores_source_metadata_and_keeps_snapshot_for_cleanup() {
        let (_root, store, initial) = fixture();
        let s = start(&store, &initial, '3');
        let s = ready(&store, &s);
        let cancelled = step(&store, &s, Action::Cancel);
        assert_eq!(cancelled.epoch, 1);
        assert_eq!(
            cancelled.recorded_execution_peer(),
            Some(peer_id('1').as_str())
        );
        assert_eq!(
            cancelled.transfer.as_ref().unwrap().snapshot_digest,
            Some(hash(2))
        );
        rejected(&store, &cancelled, commit());
        assert!(
            prepare(
                &store,
                &initial.binding,
                cancelled.revision,
                cancelled.transfer.as_ref().unwrap().operation_id.clone(),
                peer_id('4')
            )
            .is_err()
        );
        assert_eq!(read(&store, &initial.binding).unwrap(), Some(cancelled));
    }
    #[test]
    fn concurrent_preparations_have_one_winner_and_distinct_workspaces_do_not_share_progress() {
        let (root, store, initial) = fixture();
        let barrier = Arc::new(Barrier::new(3));
        let workers: Vec<_> = ['3', '4']
            .into_iter()
            .map(|target| {
                let home = root.path().to_owned();
                let barrier = barrier.clone();
                let scope = initial.binding.clone();
                std::thread::spawn(move || {
                    let store = CloudVaultHeads::new(&home);
                    barrier.wait();
                    prepare(
                        &store,
                        &scope,
                        1,
                        uuid::Uuid::now_v7().to_string(),
                        peer_id(target),
                    )
                    .unwrap()
                    .is_some()
                })
            })
            .collect();
        barrier.wait();
        assert_eq!(
            workers
                .into_iter()
                .map(|w| w.join().unwrap() as usize)
                .sum::<usize>(),
            1
        );
        let first = read(&store, &initial.binding).unwrap().unwrap();
        let second = initialize(&store, scope(), peer_id('1')).unwrap();
        let next = start(&store, &second, '3');
        let cancelled = step(&store, &next, Action::Cancel);
        assert_eq!(read(&store, &initial.binding).unwrap(), Some(first));
        assert_eq!(read(&store, &second.binding).unwrap(), Some(cancelled));
    }
    #[test]
    fn missing_root_or_history_never_reinitializes_ownership() {
        let root = tempfile::tempdir().unwrap();
        let store = CloudVaultHeads::new(root.path());
        let binding = scope();
        assert!(initialize(&store, binding.clone(), peer_id('1')).is_err());
        store.pin_root(&hash(0)).unwrap();
        let initial = initialize(&store, binding.clone(), peer_id('1')).unwrap();
        let s = start(&store, &initial, '3');
        store
            .connect()
            .unwrap()
            .execute("DELETE FROM workspace_handoffs", [])
            .unwrap();
        assert!(read(&store, &binding).unwrap().is_none());
        assert!(initialize(&store, binding.clone(), peer_id('1')).is_err());
        assert_eq!(
            read_operation(&store, &binding, &s.transfer.unwrap().operation_id)
                .unwrap()
                .unwrap()
                .revision,
            2
        );
        store
            .connect()
            .unwrap()
            .execute("DELETE FROM endpoint_root", [])
            .unwrap();
        assert!(read(&store, &binding).is_err());
        assert!(store.root_state().unwrap().1);
        assert!(store.pin_root(&hash(0)).is_err());
    }
    #[test]
    fn restoring_an_old_current_row_cannot_hide_retained_transfer_progress() {
        let (_root, store, initial) = fixture();
        let prepared = start(&store, &initial, '3');
        let stopped = stop(&store, &prepared);
        store
            .connect()
            .unwrap()
            .execute(
                "UPDATE workspace_handoffs SET state=?1",
                [serde_json::to_string(&initial).unwrap()],
            )
            .unwrap();
        assert!(read(&store, &initial.binding).is_err());
        assert!(initialize(&store, initial.binding.clone(), peer_id('1')).is_err());
        assert_eq!(
            read_operation(
                &store,
                &initial.binding,
                &stopped.transfer.as_ref().unwrap().operation_id
            )
            .unwrap(),
            Some(stopped)
        );
    }

    #[test]
    fn refuses_changed_identity_and_history_and_controller_as_execution_target() {
        let (_root, store, initial) = fixture();
        assert!(
            prepare(
                &store,
                &initial.binding,
                1,
                uuid::Uuid::now_v7().to_string(),
                initial.binding.controller_peer_id.clone()
            )
            .is_err()
        );
        assert_eq!(
            read(&store, &initial.binding).unwrap(),
            Some(initial.clone())
        );
        let s = start(&store, &initial, '3');
        let mut changed = initial.binding.clone();
        changed.owner_peer_id = peer_id('5');
        assert!(read(&store, &changed).is_err());
        let mut changed = initial.binding.clone();
        changed.controller_peer_id = peer_id('5');
        assert!(read(&store, &changed).is_err());
        store
            .connect()
            .unwrap()
            .execute("UPDATE workspace_handoff_operations SET state='{}'", [])
            .unwrap();
        assert!(read(&store, &s.binding).is_err());
        assert!(
            advance(
                &store,
                &s.binding,
                s.revision,
                &s.transfer.unwrap().operation_id,
                1,
                Action::BeginStop
            )
            .is_err()
        );
    }
}
