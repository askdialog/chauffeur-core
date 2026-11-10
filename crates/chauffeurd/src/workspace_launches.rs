//! Durable ordinary-chat dispatch intents. No absence or sidecar restart proves source stop.
use crate::{Error, Result, store::Store};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub id: String,
    pub workspace: String,
    pub chat: String,
    pub generation: String,
    pub phase: String,
}
fn uncertain() -> Error {
    Error::Conflict("Agent launch evidence changed; retain the workspace fence and reconcile its processes before transferring".into())
}
fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id)
}

/// Called before the request can be written; failures do not authorize dispatch.
pub fn begin(store: &Store, workspace: &str, chat: &str, generation: &str) -> Result<Launch> {
    if !valid_id(generation) {
        return Err(uncertain());
    }
    let launch = Launch {
        id: uuid::Uuid::now_v7().to_string(),
        workspace: workspace.into(),
        chat: chat.into(),
        generation: generation.into(),
        phase: "launching".into(),
    };
    store.durable(|c| {
        let count = c.execute("INSERT INTO workspace_agent_launches(launch_id,workspace_id,chat_id,sidecar_generation,phase)
            SELECT ?1,?2,?3,?4,'launching' FROM chats WHERE id=?3 AND workspace_id=?2", params![launch.id,workspace,chat,generation])?;
        if count != 1 { return Err(uncertain()); }
        Ok(())
    })?;
    Ok(launch)
}

/// Acceptance is not exit. A concurrent stop receipt must never be changed back to active.
pub fn acknowledged(store: &Store, launch: &Launch) -> Result<()> {
    store.durable(|c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let phase: Option<String> = tx.query_row("SELECT phase FROM workspace_agent_launches WHERE launch_id=?1 AND chat_id=?2 AND sidecar_generation=?3",params![launch.id,launch.chat,launch.generation],|r|r.get(0)).optional()?;
        match phase.as_deref() {
            Some("launching") => { tx.execute("UPDATE workspace_agent_launches SET phase='acknowledged' WHERE launch_id=?1",[&launch.id])?; }
            Some("acknowledged" | "stopped") => {}
            _ => return Err(uncertain()),
        }
        tx.commit()?;
        Ok(())
    })
}

/// Receipts belong to the exact private sidecar pipe generation and chat. Validate the whole
/// receipt before changing any row; replay of a known stopped intent is harmless.
pub fn confirmed(store: &Store, chat: &str, generation: &str, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut seen = HashSet::new();
    if ids.len() > 10000
        || !valid_id(generation)
        || ids.iter().any(|id| !valid_id(id) || !seen.insert(id))
    {
        return Err(uncertain());
    }
    store.durable(|c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for id in ids {
            let phase: Option<String> = tx.query_row("SELECT phase FROM workspace_agent_launches WHERE launch_id=?1 AND chat_id=?2 AND sidecar_generation=?3",params![id,chat,generation],|r|r.get(0)).optional()?;
            if !matches!(phase.as_deref(), Some("launching" | "acknowledged" | "stopped")) { return Err(uncertain()); }
        }
        for id in ids { tx.execute("UPDATE workspace_agent_launches SET phase='stopped' WHERE launch_id=?1",[id])?; }
        tx.commit()?;
        Ok(())
    })
}

/// Register only an observed direct child of the exact private sidecar process. A launch may
/// have no registration (lost reply, early exit, legacy sidecar); absence never proves exit.
pub fn register_root(
    store: &Store,
    launch: &Launch,
    parent: &crate::cloud_process::NativeProcessPin,
    pid: u32,
) -> Result<()> {
    let root = crate::cloud_process::inspect_child(parent, pid)?;
    store.durable(|c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let matches = tx.query_row("SELECT count(*) FROM workspace_agent_launches WHERE launch_id=?1 AND workspace_id=?2 AND chat_id=?3 AND sidecar_generation=?4 AND phase IN ('launching','acknowledged','stopped')", params![launch.id,launch.workspace,launch.chat,launch.generation], |r| r.get::<_,u32>(0))?;
        if matches != 1 { return Err(uncertain()); }
        tx.execute("INSERT INTO workspace_agent_roots(launch_id,sidecar_pid,sidecar_stamp,native_pid,native_stamp) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(launch_id) DO NOTHING",params![launch.id,parent.pid,parent.stamp,root.pid,root.stamp])?;
        let same = tx.query_row("SELECT count(*) FROM workspace_agent_roots WHERE launch_id=?1 AND sidecar_pid=?2 AND sidecar_stamp=?3 AND native_pid=?4 AND native_stamp=?5",params![launch.id,parent.pid,parent.stamp,root.pid,root.stamp], |r|r.get::<_,u32>(0))?;
        if same != 1 { return Err(uncertain()); }
        tx.commit()?;
        Ok(())
    })
}

/// Explicit read-only OS reconciliation, usable after reopening the store. Never signal a
/// historical PID. A still-present sidecar could dispatch a delayed request: both its exact
/// identity and the registered native root must be absent before recording a root stop.
/// Missing registrations remain unresolved. This is not complete workspace quiescence.
pub fn reconcile_roots(store: &Store, workspace: &str) -> Result<Vec<Launch>> {
    use crate::cloud_process::{NativeProcessPin, is_stopped};
    for launch in unresolved(store, workspace)? {
        let pins = store.with(|c| c.query_row("SELECT sidecar_pid,sidecar_stamp,native_pid,native_stamp FROM workspace_agent_roots WHERE launch_id=?1", [&launch.id], |r| Ok((NativeProcessPin{pid:r.get(0)?,stamp:r.get(1)?}, NativeProcessPin{pid:r.get(2)?,stamp:r.get(3)?}))).optional())?;
        let Some((parent, root)) = pins else { continue };
        if parent.stamp.is_empty() || root.stamp.is_empty() || parent.pid == root.pid {
            return Err(uncertain());
        }
        if !is_stopped(&parent)? || !is_stopped(&root)? {
            continue;
        }
        store.durable(|c| {
            // Preserve exact evidence under concurrent registration/acknowledgement.
            let count = c.execute("UPDATE workspace_agent_launches SET phase='stopped' WHERE launch_id=?1 AND workspace_id=?2 AND chat_id=?3 AND sidecar_generation=?4 AND phase IN ('launching','acknowledged','stopped') AND EXISTS(SELECT 1 FROM workspace_agent_roots WHERE launch_id=?1 AND sidecar_pid=?5 AND sidecar_stamp=?6 AND native_pid=?7 AND native_stamp=?8)", params![launch.id,launch.workspace,launch.chat,launch.generation,parent.pid,parent.stamp,root.pid,root.stamp])?;
            if count != 1 { return Err(uncertain()); }
            Ok(())
        })?;
    }
    unresolved(store, workspace)
}

/// Unresolved registered intents only. An empty list does not prove coverage of legacy agents,
/// detached children, services or all workspace writers, and cannot issue a source-stop receipt.
pub fn unresolved(store: &Store, workspace: &str) -> Result<Vec<Launch>> {
    let launches = store.with(|c| c.prepare("SELECT launch_id,workspace_id,chat_id,sidecar_generation,phase FROM workspace_agent_launches WHERE workspace_id=?1 AND phase!='stopped' ORDER BY launch_id")?.query_map([workspace], |r| Ok(Launch {id:r.get(0)?,workspace:r.get(1)?,chat:r.get(2)?,generation:r.get(3)?,phase:r.get(4)?}))?.collect::<rusqlite::Result<Vec<_>>>())?;
    if launches.iter().any(|l| {
        !valid_id(&l.id)
            || !valid_id(&l.generation)
            || !matches!(l.phase.as_str(), "launching" | "acknowledged")
    }) {
        return Err(uncertain());
    }
    Ok(launches)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Store) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("state.sqlite")).unwrap();
        store.with(|c| {
            c.execute("INSERT INTO workspaces(id,slug,path,branch,state,created_at) VALUES('ws','ws','/unused','main','ready','now')", [])?;
            c.execute("INSERT INTO chats(id,workspace_id,agent,permission_mode,status,created_at) VALUES('chat','ws','claude','default','idle','now')", [])?;
            Ok(())
        }).unwrap();
        (root, store)
    }
    #[test]
    fn dispatch_intents_survive_reopen_and_only_exact_stop_receipts_settle_them() {
        let (root, store) = fixture();
        let generation = uuid::Uuid::now_v7().to_string();
        let first = begin(&store, "ws", "chat", &generation).unwrap();
        let reopened = Store::open(&root.path().join("state.sqlite")).unwrap();
        assert_eq!(unresolved(&reopened, "ws").unwrap(), vec![first.clone()]);
        acknowledged(&store, &first).unwrap();
        assert_eq!(
            unresolved(&reopened, "ws").unwrap()[0].phase,
            "acknowledged"
        );
        let next = begin(&store, "ws", "chat", &generation).unwrap();
        confirmed(&store, "chat", &generation, std::slice::from_ref(&first.id)).unwrap();
        // Late acceptance and repeated receipts cannot reactivate the old launch or settle a new one.
        acknowledged(&store, &first).unwrap();
        confirmed(&store, "chat", &generation, std::slice::from_ref(&first.id)).unwrap();
        assert_eq!(unresolved(&reopened, "ws").unwrap(), vec![next]);
    }
    #[test]
    fn wrong_generation_chat_unknown_and_duplicate_receipts_never_partially_settle_intents() {
        let (_root, store) = fixture();
        let generation = uuid::Uuid::now_v7().to_string();
        let launch = begin(&store, "ws", "chat", &generation).unwrap();
        assert!(
            confirmed(
                &store,
                "chat",
                &uuid::Uuid::now_v7().to_string(),
                std::slice::from_ref(&launch.id)
            )
            .is_err()
        );
        assert!(
            confirmed(
                &store,
                "other",
                &generation,
                std::slice::from_ref(&launch.id)
            )
            .is_err()
        );
        assert!(
            confirmed(
                &store,
                "chat",
                &generation,
                &[launch.id.clone(), uuid::Uuid::now_v7().to_string()]
            )
            .is_err()
        );
        assert!(
            confirmed(
                &store,
                "chat",
                &generation,
                &[launch.id.clone(), launch.id.clone()]
            )
            .is_err()
        );
        assert_eq!(unresolved(&store, "ws").unwrap(), vec![launch]);
        assert!(begin(&store, "missing", "chat", &generation).is_err());
        assert!(begin(&store, "ws", "missing", &generation).is_err());
        assert!(begin(&store, "ws", "chat", "invalid").is_err());
        store
            .with(|c| {
                c.execute(
                    "UPDATE workspace_agent_launches SET sidecar_generation='corrupt'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(unresolved(&store, "ws").is_err());
    }
    #[test]
    fn concurrent_acceptance_and_stop_confirmation_cannot_reactivate_a_stopped_launch() {
        let (root, store) = fixture();
        let other = Store::open(&root.path().join("state.sqlite")).unwrap();
        let generation = uuid::Uuid::now_v7().to_string();
        let launch = begin(&store, "ws", "chat", &generation).unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| acknowledged(&store, &launch).unwrap());
            scope.spawn(|| {
                confirmed(
                    &other,
                    "chat",
                    &generation,
                    std::slice::from_ref(&launch.id),
                )
                .unwrap()
            });
        });
        assert!(unresolved(&store, "ws").unwrap().is_empty());
    }
    #[test]
    fn reopened_journal_requires_both_the_registered_root_and_its_sidecar_to_disappear() {
        use std::io::{BufRead, Write};
        use std::process::{Command, Stdio};
        let (folder, store) = fixture();
        let generation = uuid::Uuid::now_v7().to_string();
        let launch = begin(&store, "ws", "chat", &generation).unwrap();
        let unknown = begin(&store, "ws", "chat", &generation).unwrap();
        // The disposable parent reaps its child on command but remains alive afterwards.
        let mut parent = Command::new("python3").arg("-u").arg("-c").arg("import subprocess,sys; p=subprocess.Popen(['sleep','30']); print(p.pid,flush=True); sys.stdin.readline(); p.terminate(); p.wait(); print('stopped',flush=True); sys.stdin.read()")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut output = std::io::BufReader::new(parent.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let pid: u32 = line.trim().parse().unwrap();
        let parent_pin = crate::cloud_process::inspect(parent.id()).unwrap().unwrap();
        assert!(register_root(&store, &launch, &parent_pin, std::process::id()).is_err());
        let mut wrong = launch.clone();
        wrong.generation = uuid::Uuid::now_v7().to_string();
        assert!(register_root(&store, &wrong, &parent_pin, pid).is_err());
        register_root(&store, &launch, &parent_pin, pid).unwrap();
        register_root(&store, &launch, &parent_pin, pid).unwrap();
        let reopened = Store::open(&folder.path().join("state.sqlite")).unwrap();
        assert_eq!(reconcile_roots(&reopened, "ws").unwrap().len(), 2);
        writeln!(parent.stdin.as_mut().unwrap(), "stop").unwrap();
        line.clear();
        output.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "stopped");
        assert_eq!(
            reconcile_roots(&reopened, "ws").unwrap().len(),
            2,
            "a live sidecar might still dispatch"
        );
        parent.kill().unwrap();
        parent.wait().unwrap();
        assert_eq!(reconcile_roots(&reopened, "ws").unwrap(), vec![unknown]);
        acknowledged(&store, &launch).unwrap();
        assert_eq!(unresolved(&store, "ws").unwrap().len(), 1);
    }

    #[test]
    fn a_dead_sidecar_does_not_prove_its_orphaned_native_root_stopped() {
        use std::io::BufRead;
        use std::process::{Command, Stdio};
        let (_folder, store) = fixture();
        let launch = begin(&store, "ws", "chat", &uuid::Uuid::now_v7().to_string()).unwrap();
        // The root expires itself, even if an assertion fails; no historical PID is signalled.
        let mut parent = Command::new("python3").arg("-u").arg("-c").arg("import subprocess,sys; p=subprocess.Popen(['sleep','2']); print(p.pid,flush=True); sys.stdin.read()")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        std::io::BufReader::new(parent.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let pin = crate::cloud_process::inspect(parent.id()).unwrap().unwrap();
        register_root(&store, &launch, &pin, line.trim().parse().unwrap()).unwrap();
        parent.kill().unwrap();
        parent.wait().unwrap();
        assert_eq!(reconcile_roots(&store, "ws").unwrap(), vec![launch]);
        // No need to wait for this orphan's init reaper: live/zombie both remain conservative.
    }
}
