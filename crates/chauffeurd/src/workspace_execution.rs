//! Source-host admission barrier. A drained barrier does not prove existing processes stopped.
//! Trusted callers must verify controller identity/state before install or cancellation release.
use crate::{
    Error, Result,
    cloud_workspace_handoff::{Phase, State},
    store::Store,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Clone)]
pub struct ExecutionGate {
    store: Store,
    inner: Arc<Inner>,
}
struct Inner {
    generation: String,
    stopping: Mutex<HashSet<String>>,
    active: Mutex<HashMap<String, usize>>,
    changed: Notify,
}
pub struct Admission {
    inner: Arc<Inner>,
    workspace: String,
}
fn blocked() -> Error {
    Error::Conflict("Workspace execution is fenced for handoff; reconcile or cancel the current transfer before running locally".into())
}
fn not_committed() -> Error {
    Error::Conflict(
        "No restoration here matches this committed transfer; restore its snapshot on this machine before activating it".into(),
    )
}
fn moved() -> Error {
    Error::Conflict(
        "This workspace moved to another machine with a committed transfer; it is read-only here, work on it there".into(),
    )
}
fn unavailable() -> Error {
    Error::Unavailable("Workspace execution admission cannot be verified".into())
}
fn transfer_matches(fence: &State, state: &State) -> bool {
    fence.binding == state.binding
        && fence
            .transfer
            .as_ref()
            .zip(state.transfer.as_ref())
            .is_some_and(|(a, b)| {
                a.operation_id == b.operation_id
                    && a.source_epoch == b.source_epoch
                    && a.source_peer_id == b.source_peer_id
                    && a.target_peer_id == b.target_peer_id
            })
}
fn validate_fence(state: &State) -> Result<()> {
    state.validate()?;
    if state.transfer.as_ref().map(|t| t.phase) != Some(Phase::Stopping) {
        return Err(blocked());
    }
    Ok(())
}
impl ExecutionGate {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            inner: Arc::new(Inner {
                generation: uuid::Uuid::now_v7().to_string(),
                stopping: Mutex::default(),
                active: Mutex::new(HashMap::new()),
                changed: Notify::new(),
            }),
        }
    }
    fn persisted(&self, workspace: &str) -> Result<Option<State>> {
        self.store
            .workspace_fence(workspace)?
            .map(|raw| {
                if raw.len() > 8192 {
                    return Err(unavailable());
                }
                let state: State = serde_json::from_str(&raw)?;
                state.validate()?;
                if !matches!(
                    state.transfer.as_ref().map(|t| t.phase),
                    Some(Phase::Stopping | Phase::Cancelled | Phase::Committed)
                ) {
                    return Err(blocked());
                }
                if state.binding.workspace_id != workspace {
                    return Err(unavailable());
                }
                Ok(state)
            })
            .transpose()
    }
    /// Held through sending/registering the launch, not through the lifetime of its process.
    /// Every clone in one daemon shares the barrier. A restarted daemon rereads the fence.
    pub fn admit(&self, workspace: &str) -> Result<Admission> {
        let mut active = self.inner.active.lock().map_err(|_| unavailable())?;
        if self.store.with(|c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM workspace_restores WHERE workspace_id=?1 AND activated IS NULL)",
                [workspace],
                |r| r.get::<_, bool>(0),
            )
        })? {
            return Err(Error::Conflict("This restored workspace is quarantined; verify the destination and commit ownership before running it".into()));
        }
        match self
            .persisted(workspace)?
            .and_then(|s| s.transfer.map(|t| t.phase))
        {
            Some(Phase::Stopping) => return Err(blocked()),
            Some(Phase::Committed) => return Err(moved()),
            _ => {}
        }
        *active.entry(workspace.to_owned()).or_default() += 1;
        Ok(Admission {
            inner: self.inner.clone(),
            workspace: workspace.to_owned(),
        })
    }
    /// Persists before acknowledging. No public RPC or ordinary shutdown invokes this.
    /// This blocks NEW admitted work; it neither interrupts nor declares existing work stopped.
    pub fn install(&self, stopping: &State) -> Result<()> {
        validate_fence(stopping)?;
        let _active = self.inner.active.lock().map_err(|_| unavailable())?;
        let workspace = &stopping.binding.workspace_id;
        let expected = self.store.workspace_fence(workspace)?;
        if let Some(current) = self.persisted(workspace)? {
            let transfer = current.transfer.as_ref().ok_or_else(blocked)?;
            if transfer.phase == Phase::Committed {
                return Err(moved());
            }
            if transfer.phase == Phase::Stopping {
                if current != *stopping {
                    self.ensure_preparation_settled(workspace)?;
                }
                return if transfer_matches(&current, stopping) {
                    Ok(())
                } else {
                    Err(blocked())
                };
            }
            let next = stopping.transfer.as_ref().ok_or_else(blocked)?;
            // Keep the cancelled generation/revision so delayed old stop messages cannot
            // refence a source which was explicitly resumed. A fresh operation must advance.
            if current.binding != stopping.binding
                || current.epoch != stopping.epoch
                || current.execution_peer_id != stopping.execution_peer_id
                || stopping.revision <= current.revision
                || transfer.operation_id == next.operation_id
            {
                return Err(blocked());
            }
        }
        self.ensure_stop_settled(workspace)?;
        let raw = serde_json::to_string(stopping)?;
        if !self
            .store
            .workspace_fence_cas(workspace, expected.as_deref(), Some(&raw))?
        {
            return Err(blocked());
        }
        self.inner.changed.notify_waiters();
        Ok(())
    }
    /// Wait for pre-fence admission calls to finish. Timeout/cancellation leaves the fence.
    /// Lost sidecar responses, old agents and detached children still require reconciliation.
    pub async fn wait_for_admissions(&self, stopping: &State) -> Result<()> {
        validate_fence(stopping)?;
        let workspace = &stopping.binding.workspace_id;
        loop {
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let active = self.inner.active.lock().map_err(|_| unavailable())?;
                let current = self.persisted(workspace)?.ok_or_else(blocked)?;
                if current.transfer.as_ref().map(|t| t.phase) != Some(Phase::Stopping)
                    || !transfer_matches(&current, stopping)
                {
                    return Err(blocked());
                }
                if active.get(workspace).copied().unwrap_or(0) == 0 {
                    return Ok(());
                }
            }
            changed.await;
        }
    }
    /// Acquire only after draining admissions. The durable pending marker precedes every stop
    /// dispatch. Reopening a daemon cannot settle lost handles or late sidecar calls by absence.
    pub(crate) fn begin_stop_attempt(&self, stopping: &State) -> Result<StopAttempt> {
        validate_fence(stopping)?;
        let active = self.inner.active.lock().map_err(|_| unavailable())?;
        let workspace = &stopping.binding.workspace_id;
        let current = self.persisted(workspace)?.ok_or_else(blocked)?;
        if current.transfer.as_ref().map(|t| t.phase) != Some(Phase::Stopping)
            || !transfer_matches(&current, stopping)
            || active.get(workspace).copied().unwrap_or(0) != 0
        {
            return Err(blocked());
        }
        let mut busy = self.inner.stopping.lock().map_err(|_| unavailable())?;
        if busy.contains(workspace) {
            return Err(blocked());
        }
        let fence = serde_json::to_string(&current)?;
        self.store.durable(|c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current: Option<String> = tx.query_row("SELECT execution_fence FROM workspaces WHERE id=?1", [workspace], |r| r.get(0))?;
            if current.as_deref() != Some(fence.as_str()) || tx.query_row("SELECT EXISTS(SELECT 1 FROM workspace_preparation_attempts WHERE workspace_id=?1 AND settled!=1)", [workspace], |r| r.get::<_,bool>(0))? {
                return Err(blocked());
            }
            let previous: Option<(String,String,u8)> = tx.query_row("SELECT fence,daemon_generation,settled FROM workspace_stop_attempts WHERE workspace_id=?1", [workspace], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            if let Some((old_fence,generation,settled)) = previous
                && (settled > 1 || (settled == 0 && (old_fence != fence || generation != self.inner.generation))) {
                return Err(Error::Conflict("A source stop was interrupted; reconcile its retained process evidence before retrying or cancelling".into()));
            }
            tx.execute("INSERT INTO workspace_stop_attempts(workspace_id,fence,daemon_generation,settled) VALUES(?1,?2,?3,0) ON CONFLICT(workspace_id) DO UPDATE SET fence=excluded.fence,daemon_generation=excluded.daemon_generation,settled=0", params![workspace,fence,self.inner.generation])?;
            tx.commit()?;
            Ok(())
        })?;
        busy.insert(workspace.to_owned());
        Ok(StopAttempt {
            gate: self.clone(),
            workspace: workspace.to_owned(),
            fence,
        })
    }

    /// The exact stopping fence is installed, its registered stop attempt settled, and no
    /// admission or stop task is in flight. This is not proof that unregistered writers stopped.
    pub fn require_settled_stop(&self, stopping: &State) -> Result<()> {
        validate_fence(stopping)?;
        let active = self.inner.active.lock().map_err(|_| unavailable())?;
        self.require_settled_stop_locked(stopping, &active)
    }

    fn require_settled_stop_locked(
        &self,
        stopping: &State,
        active: &HashMap<String, usize>,
    ) -> Result<()> {
        let workspace = &stopping.binding.workspace_id;
        let current = self.persisted(workspace)?.ok_or_else(blocked)?;
        if current != *stopping || active.get(workspace).copied().unwrap_or(0) != 0 {
            return Err(blocked());
        }
        if self
            .inner
            .stopping
            .lock()
            .map_err(|_| unavailable())?
            .contains(workspace)
        {
            return Err(blocked());
        }
        let fence = serde_json::to_string(&current)?;
        let attempt: Option<(String, u8)> = self.store.with(|c| {
            c.query_row(
                "SELECT fence, settled FROM workspace_stop_attempts WHERE workspace_id=?1",
                [workspace],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })?;
        match attempt {
            Some((stored, 1)) if stored == fence => Ok(()),
            _ => Err(Error::Conflict(
                "The workspace's registered stop has not settled; stop its agents, scripts and terminals first".into(),
            )),
        }
    }

    /// Settles the preparation attempts an earlier daemon generation left pending on
    /// `workspace`: that process is gone, so none of their work can still act. Called only for
    /// an explicit resume or cancellation of the transfer they belong to; this generation's
    /// pending attempts are left alone, since a task of this daemon may still run them.
    pub(crate) fn settle_stale_preparations(&self, workspace: &str) -> Result<()> {
        let _active = self.inner.active.lock().map_err(|_| unavailable())?;
        self.store.durable(|c| {
            c.execute(
                "UPDATE workspace_preparation_attempts SET settled=1 WHERE workspace_id=?1 AND daemon_generation!=?2 AND settled!=1",
                params![workspace, self.inner.generation],
            )?;
            Ok(())
        })?;
        self.inner.changed.notify_waiters();
        Ok(())
    }

    /// Owns source preparation after the exact registered stop settles. This guard is neither
    /// an admission nor a stop attempt, so capture can still require the same settled stop.
    /// Pending attempts, including abandoned attempts in this daemon, need explicit recovery.
    pub(crate) fn begin_preparation(&self, stopping: &State) -> Result<PreparationAttempt> {
        validate_fence(stopping)?;
        let active = self.inner.active.lock().map_err(|_| unavailable())?;
        self.require_settled_stop_locked(stopping, &active)?;
        let workspace = &stopping.binding.workspace_id;
        let fence = serde_json::to_string(stopping)?;
        let attempt_id = uuid::Uuid::now_v7().to_string();
        self.store.durable(|c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current: Option<String> = tx.query_row("SELECT execution_fence FROM workspaces WHERE id=?1", [workspace], |r| r.get(0))?;
            let settled: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM workspace_stop_attempts WHERE workspace_id=?1 AND fence=?2 AND settled=1)", params![workspace,fence], |r|r.get(0))?;
            let pending: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM workspace_preparation_attempts WHERE workspace_id=?1 AND settled!=1)", [workspace], |r|r.get(0))?;
            if current.as_deref() != Some(fence.as_str()) || !settled || pending {
                return Err(blocked());
            }
            tx.execute("INSERT INTO workspace_preparation_attempts(attempt_id,workspace_id,fence,daemon_generation,settled) VALUES(?1,?2,?3,?4,0)", params![attempt_id,workspace,fence,self.inner.generation])?;
            tx.commit()?;
            Ok(())
        })?;
        Ok(PreparationAttempt {
            gate: self.clone(),
            workspace: workspace.clone(),
            fence,
            attempt_id,
        })
    }

    // Called with the admission mutex held. A pending/malformed record or an in-flight
    // task prevents cancellation from reopening execution while old stop requests may act.
    fn ensure_stop_settled(&self, workspace: &str) -> Result<()> {
        self.ensure_preparation_settled(workspace)?;
        if self
            .inner
            .stopping
            .lock()
            .map_err(|_| unavailable())?
            .contains(workspace)
        {
            return Err(blocked());
        }
        let settled: Option<u8> = self.store.with(|c| {
            c.query_row(
                "SELECT settled FROM workspace_stop_attempts WHERE workspace_id=?1",
                [workspace],
                |r| r.get(0),
            )
            .optional()
        })?;
        if settled.is_some_and(|value| value != 1) {
            return Err(blocked());
        }
        Ok(())
    }

    fn ensure_preparation_settled(&self, workspace: &str) -> Result<()> {
        if self.store.with(|c| c.query_row("SELECT EXISTS(SELECT 1 FROM workspace_preparation_attempts WHERE workspace_id=?1 AND settled!=1)", [workspace], |r|r.get::<_,bool>(0)))? {
            return Err(blocked());
        }
        Ok(())
    }

    /// Only the exact pre-commit cancellation can resume this source. A forward commit,
    /// heartbeat or missing controller cannot unlock it. Return support is separate work.
    pub fn release_cancelled(&self, cancelled: &State) -> Result<()> {
        cancelled.validate()?;
        if cancelled.transfer.as_ref().map(|t| t.phase) != Some(Phase::Cancelled) {
            return Err(blocked());
        }
        let _active = self.inner.active.lock().map_err(|_| unavailable())?;
        let workspace = &cancelled.binding.workspace_id;
        self.ensure_stop_settled(workspace)?;
        let raw = self.store.workspace_fence(workspace)?.ok_or_else(blocked)?;
        let current = self.persisted(workspace)?.ok_or_else(blocked)?;
        if !transfer_matches(&current, cancelled) {
            return Err(blocked());
        }
        if current.transfer.as_ref().map(|t| t.phase) == Some(Phase::Committed) {
            return Err(moved());
        }
        if current.transfer.as_ref().map(|t| t.phase) == Some(Phase::Cancelled) {
            return if current == *cancelled {
                Ok(())
            } else {
                Err(blocked())
            };
        }
        if cancelled.revision <= current.revision {
            return Err(blocked());
        }
        let released = serde_json::to_string(cancelled)?;
        if !self
            .store
            .workspace_fence_cas(workspace, Some(&raw), Some(&released))?
        {
            return Err(blocked());
        }
        self.inner.changed.notify_waiters();
        Ok(())
    }

    /// Source side of a committed transfer: the stopped workspace keeps its fence for good,
    /// replaced by the committed state, so it reads as moved and runs nothing here. Only a later
    /// transfer back can bring it home. The stop and every preparation must have settled.
    pub fn retire_committed(&self, committed: &State) -> Result<()> {
        committed.validate()?;
        if committed.transfer.as_ref().map(|t| t.phase) != Some(Phase::Committed) {
            return Err(blocked());
        }
        let _active = self.inner.active.lock().map_err(|_| unavailable())?;
        let workspace = &committed.binding.workspace_id;
        self.ensure_stop_settled(workspace)?;
        let raw = self.store.workspace_fence(workspace)?.ok_or_else(blocked)?;
        let current = self.persisted(workspace)?.ok_or_else(blocked)?;
        if !transfer_matches(&current, committed) {
            return Err(blocked());
        }
        match current.transfer.as_ref().map(|t| t.phase) {
            Some(Phase::Committed) if current == *committed => return Ok(()),
            Some(Phase::Stopping) if committed.revision > current.revision => {}
            _ => return Err(blocked()),
        }
        let retired = serde_json::to_string(committed)?;
        if !self
            .store
            .workspace_fence_cas(workspace, Some(&raw), Some(&retired))?
        {
            return Err(blocked());
        }
        self.inner.changed.notify_waiters();
        Ok(())
    }

    /// Destination side of a committed transfer: the workspace this daemon restored for that
    /// operation, under the receipt the journal committed, leaves its quarantine and the source's
    /// fence it arrived with, so it can run here. Its private clones and native homes stay.
    /// Trusted callers authenticate the `Committed` state and the local peer first.
    pub fn activate_restored(&self, committed: &State, local_peer: &str) -> Result<()> {
        committed.validate()?;
        let t = committed.transfer.as_ref().ok_or_else(blocked)?;
        if t.phase != Phase::Committed
            || t.target_peer_id != local_peer
            || committed.execution_peer_id != local_peer
        {
            return Err(not_committed());
        }
        let _active = self.inner.active.lock().map_err(|_| unavailable())?;
        let workspace = &committed.binding.workspace_id;
        let activated = serde_json::to_string(committed)?;
        self.store.durable(|c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let row: Option<(String, String, Option<String>, Option<String>)> = tx
                .query_row(
                    "SELECT operation_id,snapshot_digest,receipt,activated FROM workspace_restores WHERE workspace_id=?1",
                    [workspace],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let Some((operation, snapshot, Some(receipt), previous)) = row else {
                return Err(not_committed());
            };
            let restored: crate::cloud_workspace_restore::Restored =
                serde_json::from_str(&receipt).map_err(|_| unavailable())?;
            let digest = URL_SAFE_NO_PAD.encode(Sha256::digest(serde_json::to_vec(&restored.receipt)?));
            if operation != t.operation_id
                || t.snapshot_digest.as_deref() != Some(snapshot.as_str())
                || restored.receipt.operation_id != t.operation_id
                || restored.receipt.snapshot_digest != snapshot
                || restored.receipt.source_epoch != t.source_epoch
                || restored.receipt.target_peer_id != local_peer
                || restored.receipt.workspace_id != *workspace
                || restored.digest != digest
                || t.destination_receipt_digest.as_deref() != Some(digest.as_str())
            {
                return Err(not_committed());
            }
            if let Some(previous) = previous {
                return if previous == activated { Ok(()) } else { Err(not_committed()) };
            }
            let fence: Option<String> = tx.query_row(
                "SELECT execution_fence FROM workspaces WHERE id=?1",
                [workspace],
                |r| r.get(0),
            )?;
            let arrived: State = serde_json::from_str(fence.as_deref().ok_or_else(unavailable)?)
                .map_err(|_| unavailable())?;
            if arrived.transfer.as_ref().map(|a| a.phase) != Some(Phase::Stopping)
                || !transfer_matches(&arrived, committed)
            {
                return Err(not_committed());
            }
            if tx.execute(
                "UPDATE workspaces SET execution_fence=NULL WHERE id=?1 AND execution_fence=?2
                 AND NOT EXISTS(SELECT 1 FROM workspace_preparation_attempts WHERE workspace_id=?1 AND settled!=1)",
                params![workspace, fence],
            )? != 1
                || tx.execute(
                    "UPDATE workspace_restores SET activated=?2 WHERE workspace_id=?1 AND activated IS NULL",
                    params![workspace, activated],
                )? != 1
            {
                return Err(not_committed());
            }
            tx.commit()?;
            Ok(())
        })?;
        self.inner.changed.notify_waiters();
        Ok(())
    }
}
/// Retained pending evidence survives Drop and panic. Only the coordinator which has awaited
/// all preparation work may explicitly settle this exact attempt; no automatic retry adopts it.
pub(crate) struct PreparationAttempt {
    gate: ExecutionGate,
    workspace: String,
    fence: String,
    attempt_id: String,
}
impl PreparationAttempt {
    pub(crate) fn id(&self) -> &str {
        &self.attempt_id
    }
    pub(crate) fn finish(self) -> Result<()> {
        let _active = self.gate.inner.active.lock().map_err(|_| unavailable())?;
        self.gate.store.durable(|c| {
            let changed = c.execute("UPDATE workspace_preparation_attempts SET settled=1 WHERE attempt_id=?1 AND workspace_id=?2 AND fence=?3 AND daemon_generation=?4 AND settled=0 AND EXISTS(SELECT 1 FROM workspaces WHERE id=?2 AND execution_fence=?3)", params![self.attempt_id,self.workspace,self.fence,self.gate.inner.generation])?;
            if changed != 1 { return Err(blocked()); }
            Ok(())
        })?;
        self.gate.inner.changed.notify_waiters();
        Ok(())
    }
}
/// A registered-stop operation guard, not a source quiescence receipt. Dropping it never
/// clears its durable pending marker. The coordinator owns it through all asynchronous calls.
pub(crate) struct StopAttempt {
    gate: ExecutionGate,
    workspace: String,
    fence: String,
}
impl StopAttempt {
    pub(crate) fn finish(self) -> Result<()> {
        let _active = self.gate.inner.active.lock().map_err(|_| unavailable())?;
        let current = self.gate.persisted(&self.workspace)?.ok_or_else(blocked)?;
        if serde_json::to_string(&current)? != self.fence {
            return Err(blocked());
        }
        self.gate.store.durable(|c| {
            let changed = c.execute("UPDATE workspace_stop_attempts SET settled=1 WHERE workspace_id=?1 AND fence=?2 AND daemon_generation=?3 AND settled=0", params![self.workspace,self.fence,self.gate.inner.generation])?;
            if changed != 1 { return Err(blocked()); }
            Ok(())
        })
    }
}
impl Drop for StopAttempt {
    fn drop(&mut self) {
        if let Ok(mut busy) = self.gate.inner.stopping.lock() {
            busy.remove(&self.workspace);
        }
        self.gate.inner.changed.notify_waiters();
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        if let Ok(mut active) = self.inner.active.lock() {
            if let Some(count) = active.get_mut(&self.workspace) {
                *count -= 1;
                if *count == 0 {
                    active.remove(&self.workspace);
                }
            }
            drop(active);
            self.inner.changed.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_workspace_handoff::{Binding, Transfer};
    use crate::store::{WorkspaceRow, now};
    use chauffeur_protocol::model::WorkspaceState;
    fn peer(n: char) -> String {
        format!("12D3KooW{}", n.to_string().repeat(44))
    }
    pub(super) fn stopping(workspace: &str) -> State {
        State {
            version: 1,
            binding: Binding {
                workspace_id: workspace.into(),
                owner_peer_id: peer('1'),
                controller_peer_id: peer('2'),
            },
            revision: 3,
            epoch: 1,
            execution_peer_id: peer('1'),
            transfer: Some(Transfer {
                operation_id: uuid::Uuid::now_v7().to_string(),
                source_peer_id: peer('1'),
                target_peer_id: peer('3'),
                source_epoch: 1,
                phase: Phase::Stopping,
                source_stop_digest: None,
                snapshot_digest: None,
                destination_receipt_digest: None,
            }),
        }
    }
    fn fixture() -> (tempfile::TempDir, Store, State) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("state.sqlite")).unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        store
            .insert_workspace(&WorkspaceRow {
                id: id.clone(),
                slug: id.clone(),
                path: root.path().join("workspace").to_string_lossy().into(),
                branch: "fixture".into(),
                parent_id: None,
                state: WorkspaceState::Ready,
                tickets: vec![],
                created_at: now(),
                archived_at: None,
                last_error: None,
                attention_at: None,
                seen_at: None,
                explore: false,
                title: None,
                auto_title: false,
                title_source: None,
                pinned: false,
                position: 0.0,
                section_id: None,
                house_position: None,
            })
            .unwrap();
        (root, store, stopping(&id))
    }
    #[tokio::test]
    async fn fence_blocks_new_admission_and_survives_wait_timeout_and_store_reopen() {
        let (root, store, state) = fixture();
        let gate = ExecutionGate::new(store.clone());
        let admitted = gate.admit(&state.binding.workspace_id).unwrap();
        gate.install(&state).unwrap();
        assert!(gate.admit(&state.binding.workspace_id).is_err());
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                gate.wait_for_admissions(&state)
            )
            .await
            .is_err()
        );
        assert!(gate.admit(&state.binding.workspace_id).is_err());
        let reopened = ExecutionGate::new(Store::open(&root.path().join("state.sqlite")).unwrap());
        assert!(reopened.admit(&state.binding.workspace_id).is_err());
        drop(admitted);
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            gate.wait_for_admissions(&state),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(gate.admit(&state.binding.workspace_id).is_err());
    }
    #[tokio::test]
    async fn only_exact_cancellation_releases_and_invalidates_waiters() {
        let (_root, store, state) = fixture();
        let gate = ExecutionGate::new(store);
        let _admitted = gate.admit(&state.binding.workspace_id).unwrap();
        gate.install(&state).unwrap();
        gate.install(&state).unwrap();
        let mut wrong = state.clone();
        wrong.transfer.as_mut().unwrap().operation_id = uuid::Uuid::now_v7().to_string();
        assert!(gate.install(&wrong).is_err());
        wrong.transfer.as_mut().unwrap().phase = Phase::Cancelled;
        assert!(gate.release_cancelled(&wrong).is_err());
        let mut cancelled = state.clone();
        cancelled.revision += 1;
        cancelled.transfer.as_mut().unwrap().phase = Phase::Cancelled;
        assert!(gate.release_cancelled(&state).is_err());
        gate.release_cancelled(&cancelled).unwrap();
        gate.release_cancelled(&cancelled).unwrap();
        assert!(
            gate.install(&state).is_err(),
            "A delayed stopping message must not undo cancellation"
        );
        let mut fresh = state.clone();
        fresh.revision = cancelled.revision + 2;
        fresh.transfer.as_mut().unwrap().operation_id = uuid::Uuid::now_v7().to_string();
        gate.install(&fresh).unwrap();
        let mut fresh_cancel = fresh.clone();
        fresh_cancel.revision += 1;
        fresh_cancel.transfer.as_mut().unwrap().phase = Phase::Cancelled;
        gate.release_cancelled(&fresh_cancel).unwrap();
        assert!(gate.wait_for_admissions(&state).await.is_err());
        assert!(gate.admit(&state.binding.workspace_id).is_ok());
    }
    #[test]
    fn corrupt_fence_or_missing_workspace_never_means_unfenced() {
        let (_root, store, state) = fixture();
        let gate = ExecutionGate::new(store.clone());
        assert!(gate.admit("missing-workspace").is_err());
        store
            .workspace_fence_cas(&state.binding.workspace_id, None, Some("invalid JSON"))
            .unwrap();
        assert!(gate.admit(&state.binding.workspace_id).is_err());
        assert!(gate.install(&state).is_err());
        let mut cancelled = state.clone();
        cancelled.transfer.as_mut().unwrap().phase = Phase::Cancelled;
        assert!(gate.release_cancelled(&cancelled).is_err());
        assert_eq!(
            store.workspace_fence(&state.binding.workspace_id).unwrap(),
            Some("invalid JSON".into())
        );
    }
    fn cancellation(state: &State) -> State {
        let mut value = state.clone();
        value.revision += 1;
        value.transfer.as_mut().unwrap().phase = Phase::Cancelled;
        value
    }
    #[tokio::test]
    async fn cancellation_cannot_overtake_an_inflight_or_abandoned_registered_stop() {
        let (_root, store, state) = fixture();
        let gate = ExecutionGate::new(store);
        let admitted = gate.admit(&state.binding.workspace_id).unwrap();
        gate.install(&state).unwrap();
        assert!(
            gate.begin_stop_attempt(&state).is_err(),
            "drain before stop dispatch"
        );
        drop(admitted);
        gate.wait_for_admissions(&state).await.unwrap();
        let first = gate.begin_stop_attempt(&state).unwrap();
        assert!(
            gate.begin_stop_attempt(&state).is_err(),
            "one attempt owns the stop calls"
        );
        assert!(gate.release_cancelled(&cancellation(&state)).is_err());
        drop(first);
        assert!(
            gate.release_cancelled(&cancellation(&state)).is_err(),
            "drop is not settlement"
        );
        gate.begin_stop_attempt(&state).unwrap().finish().unwrap();
        gate.release_cancelled(&cancellation(&state)).unwrap();
        assert!(gate.admit(&state.binding.workspace_id).is_ok());
    }
    #[test]
    fn daemon_reopen_cannot_forget_a_pending_stop_or_lose_a_settled_one() {
        let (root, store, state) = fixture();
        let gate = ExecutionGate::new(store);
        gate.install(&state).unwrap();
        drop(gate.begin_stop_attempt(&state).unwrap());
        let reopened = ExecutionGate::new(Store::open(&root.path().join("state.sqlite")).unwrap());
        assert!(
            reopened.begin_stop_attempt(&state).is_err(),
            "old handles cannot be reconciled by a new daemon generation"
        );
        assert!(reopened.release_cancelled(&cancellation(&state)).is_err());
        assert!(reopened.admit(&state.binding.workspace_id).is_err());
        gate.begin_stop_attempt(&state).unwrap().finish().unwrap();
        reopened.release_cancelled(&cancellation(&state)).unwrap();
        assert!(reopened.admit(&state.binding.workspace_id).is_ok());
    }
    #[test]
    fn stop_settlement_cannot_overwrite_changed_operation_evidence() {
        let (_root, store, state) = fixture();
        let gate = ExecutionGate::new(store.clone());
        gate.install(&state).unwrap();
        let attempt = gate.begin_stop_attempt(&state).unwrap();
        store
            .with(|c| {
                c.execute(
                    "UPDATE workspace_stop_attempts SET daemon_generation='changed'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(attempt.finish().is_err());
        assert!(gate.release_cancelled(&cancellation(&state)).is_err());
        assert!(gate.admit(&state.binding.workspace_id).is_err());
    }

    #[test]
    fn only_an_earlier_daemon_s_pending_preparation_is_settled_on_request() {
        let (root, store, state) = fixture();
        let gate = ExecutionGate::new(store);
        settled(&gate, &state);
        // Left pending by this daemon: one of its tasks may still run it.
        drop(gate.begin_preparation(&state).unwrap());
        gate.settle_stale_preparations(&state.binding.workspace_id)
            .unwrap();
        assert!(gate.begin_preparation(&state).is_err());
        assert!(gate.release_cancelled(&cancellation(&state)).is_err());
        // The next daemon: that process is gone, so a resume or cancellation settles it.
        let reopened = ExecutionGate::new(Store::open(&root.path().join("state.sqlite")).unwrap());
        assert!(reopened.begin_preparation(&state).is_err());
        reopened
            .settle_stale_preparations(&state.binding.workspace_id)
            .unwrap();
        reopened
            .begin_preparation(&state)
            .unwrap()
            .finish()
            .unwrap();
        reopened.release_cancelled(&cancellation(&state)).unwrap();
        assert!(reopened.admit(&state.binding.workspace_id).is_ok());
    }

    fn settled(gate: &ExecutionGate, state: &State) {
        gate.install(state).unwrap();
        gate.begin_stop_attempt(state).unwrap().finish().unwrap();
    }

    #[test]
    fn preparation_owns_the_settled_fence_without_preventing_capture_checks() {
        let (_root, store, state) = fixture();
        let gate = ExecutionGate::new(store.clone());
        gate.install(&state).unwrap();
        assert!(gate.begin_preparation(&state).is_err());
        let stop = gate.begin_stop_attempt(&state).unwrap();
        assert!(gate.begin_preparation(&state).is_err());
        stop.finish().unwrap();
        let attempt = gate.begin_preparation(&state).unwrap();
        gate.require_settled_stop(&state).unwrap();
        gate.install(&state).unwrap();
        let mut newer = state.clone();
        newer.revision += 1;
        assert!(gate.install(&newer).is_err());
        assert!(gate.admit(&state.binding.workspace_id).is_err());
        assert!(gate.begin_preparation(&state).is_err());
        assert!(gate.begin_stop_attempt(&state).is_err());
        assert!(gate.release_cancelled(&cancellation(&state)).is_err());
        let raw = serde_json::to_string(&state).unwrap();
        let cancelled = serde_json::to_string(&cancellation(&state)).unwrap();
        assert!(
            !store
                .workspace_fence_cas(&state.binding.workspace_id, Some(&raw), Some(&cancelled))
                .unwrap()
        );
        attempt.finish().unwrap();
        gate.release_cancelled(&cancellation(&state)).unwrap();
        assert!(gate.admit(&state.binding.workspace_id).is_ok());
        let history: i64 = store
            .with(|c| {
                c.query_row(
                    "SELECT count(*) FROM workspace_preparation_attempts WHERE settled=1",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(history, 1);
    }

    #[test]
    fn preparation_refuses_inexact_fences_and_cannot_settle_substituted_evidence() {
        for field in ["attempt_id", "daemon_generation", "fence"] {
            let (_root, store, state) = fixture();
            let gate = ExecutionGate::new(store.clone());
            settled(&gate, &state);
            let mut wrong = state.clone();
            wrong.revision += 1;
            assert!(gate.begin_preparation(&wrong).is_err());
            wrong = state.clone();
            wrong.transfer.as_mut().unwrap().operation_id = uuid::Uuid::now_v7().to_string();
            assert!(gate.begin_preparation(&wrong).is_err());
            let attempt = gate.begin_preparation(&state).unwrap();
            // Names come only from this fixed fixture list; no input reaches SQL syntax.
            store
                .with(|c| {
                    c.execute(
                        &format!("UPDATE workspace_preparation_attempts SET {field}='changed'"),
                        [],
                    )?;
                    Ok(())
                })
                .unwrap();
            assert!(attempt.finish().is_err());
            assert!(gate.begin_preparation(&state).is_err());
            assert!(gate.release_cancelled(&cancellation(&state)).is_err());
        }
    }

    #[test]
    fn dropped_or_panicking_preparation_cannot_be_adopted_even_by_the_same_daemon() {
        for panic in [false, true] {
            let (root, store, state) = fixture();
            let gate = ExecutionGate::new(store.clone());
            settled(&gate, &state);
            let attempt = gate.begin_preparation(&state).unwrap();
            if panic {
                assert!(
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                        let _owned = attempt;
                        panic!("fixture interrupted preparation");
                    }))
                    .is_err()
                );
            } else {
                drop(attempt);
            }
            let reopened =
                ExecutionGate::new(Store::open(&root.path().join("state.sqlite")).unwrap());
            for current in [&gate, &reopened] {
                assert!(current.begin_preparation(&state).is_err());
                assert!(current.begin_stop_attempt(&state).is_err());
                assert!(current.release_cancelled(&cancellation(&state)).is_err());
                current.require_settled_stop(&state).unwrap();
            }
        }
    }

    #[test]
    fn stale_preparation_cannot_settle_a_new_attempt_or_operation() {
        let (_root, store, state) = fixture();
        let gate = ExecutionGate::new(store.clone());
        settled(&gate, &state);
        let stale = gate.begin_preparation(&state).unwrap();
        // Simulate explicit reconciliation; the old owned handle must remain powerless.
        store
            .with(|c| {
                c.execute("UPDATE workspace_preparation_attempts SET settled=1", [])?;
                Ok(())
            })
            .unwrap();
        gate.release_cancelled(&cancellation(&state)).unwrap();
        let mut next = state.clone();
        next.revision += 3;
        next.transfer.as_mut().unwrap().operation_id = uuid::Uuid::now_v7().to_string();
        settled(&gate, &next);
        let current = gate.begin_preparation(&next).unwrap();
        assert!(stale.finish().is_err());
        assert!(gate.release_cancelled(&cancellation(&next)).is_err());
        current.finish().unwrap();
        gate.release_cancelled(&cancellation(&next)).unwrap();
        let history: i64 = store
            .with(|c| {
                c.query_row(
                    "SELECT count(*) FROM workspace_preparation_attempts",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(history, 2);
    }

    #[test]
    fn preparation_and_cancellation_have_one_winner_even_across_gate_generations() {
        for _ in 0..12 {
            let (_root, store, state) = fixture();
            let gate = ExecutionGate::new(store.clone());
            settled(&gate, &state);
            let other = ExecutionGate::new(store);
            let barrier = std::sync::Barrier::new(2);
            let (prepared, cancelled) = std::thread::scope(|scope| {
                let a = scope.spawn(|| {
                    barrier.wait();
                    gate.begin_preparation(&state)
                });
                let b = scope.spawn(|| {
                    barrier.wait();
                    other.release_cancelled(&cancellation(&state))
                });
                (a.join().unwrap(), b.join().unwrap())
            });
            assert_ne!(prepared.is_ok(), cancelled.is_ok());
            if let Ok(attempt) = prepared {
                gate.require_settled_stop(&state).unwrap();
                attempt.finish().unwrap();
                other.release_cancelled(&cancellation(&state)).unwrap();
            }
            assert!(other.admit(&state.binding.workspace_id).is_ok());
        }
    }

    #[test]
    fn preparation_and_a_new_stop_attempt_cannot_both_acquire_the_workspace() {
        for _ in 0..12 {
            let (_root, store, state) = fixture();
            let gate = ExecutionGate::new(store.clone());
            settled(&gate, &state);
            let other = ExecutionGate::new(store);
            let barrier = std::sync::Barrier::new(2);
            let (prepared, stopped) = std::thread::scope(|scope| {
                let a = scope.spawn(|| {
                    barrier.wait();
                    gate.begin_preparation(&state)
                });
                let b = scope.spawn(|| {
                    barrier.wait();
                    other.begin_stop_attempt(&state)
                });
                (a.join().unwrap(), b.join().unwrap())
            });
            assert_ne!(prepared.is_ok(), stopped.is_ok());
            if let Ok(attempt) = prepared {
                attempt.finish().unwrap();
            }
            if let Ok(attempt) = stopped {
                attempt.finish().unwrap();
            }
            gate.require_settled_stop(&state).unwrap();
        }
    }
}
