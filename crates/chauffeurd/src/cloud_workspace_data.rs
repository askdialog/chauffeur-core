//! Retained logical exports travel as verified data, never as permission to restore or execute.
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, ensure};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::{
    Daemon, Error, Result,
    cloud_workspace_handoff::{Phase, State},
    cloud_workspace_snapshot as snapshot,
    workspace_development_profile::{self as profiles, ProfileHead, StoredProfile},
    workspace_postgres_export::{self as export, ExportReceipt},
};

const MAX_EXPORTS: usize = 32;
const MAX_RECEIPT: i64 = 512 * 1024;
pub(crate) const MAX_DATA_BYTES: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentData {
    pub source_profile: Option<ProfileHead>,
    pub exports: Vec<PackagedExport>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackagedExport {
    pub path: String,
    pub receipt: ExportReceipt,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RetainedExport {
    pub root: PathBuf,
    pub receipt: ExportReceipt,
}
fn refused() -> Error {
    Error::Unavailable("Workspace database exports are missing or changed; retain the source and complete its declared exports before packaging".into())
}
fn receipt_valid(receipt: &ExportReceipt, workspace: &str) -> anyhow::Result<()> {
    receipt.source.validate()?;
    receipt.profile.validate(workspace)?;
    ensure!(
        receipt.version == 1
            && receipt.source.binding.workspace_id == workspace
            && receipt.source.transfer.as_ref().map(|t| t.phase) == Some(Phase::Stopping),
        "Database export source changed"
    );
    let declaration = receipt
        .profile
        .profile
        .postgres_exports
        .iter()
        .find(|d| d.name == receipt.export_name)
        .context("Database export declaration missing")?;
    let service = receipt
        .profile
        .profile
        .services
        .iter()
        .find(|s| s.name == declaration.service)
        .context("Database export service missing")?;
    ensure!(
        service.docker.container == receipt.container
            && receipt.file.path == "database.dump"
            && (5..=16 * 1024 * 1024 * 1024).contains(&receipt.file.size)
            && receipt.file.sha256.len() == 64
            && receipt
                .file
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Database export reference changed"
    );
    ensure!(
        serde_json::to_vec(receipt)?.len() <= MAX_RECEIPT as usize,
        "Database export receipt too large"
    );
    Ok(())
}

/// Structural custody only. The current source requires fresh exact-fence exports; imports retain
/// earlier source snapshots and never become current host or database execution authority.
pub(crate) fn validate(
    data: &DevelopmentData,
    declared: &[StoredProfile],
    source: &State,
) -> anyhow::Result<()> {
    source.validate()?;
    ensure!(
        source.transfer.as_ref().map(|t| t.phase) == Some(Phase::Stopping),
        "Package source is not stopping"
    );
    crate::cloud_workspace_profile::validate(declared, &source.binding.workspace_id)?;
    ensure!(
        data.exports.len() <= MAX_EXPORTS && !data.exports.is_empty(),
        "Invalid database export count"
    );
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for (index, entry) in data.exports.iter().enumerate() {
        ensure!(
            entry.path == format!("database_exports/{index}"),
            "Database export path changed"
        );
        receipt_valid(&entry.receipt, &source.binding.workspace_id)?;
        // Only the exports this source stop took travel (decided with Justin 2026-10-26).
        ensure!(
            entry.receipt.source == *source
                && data.source_profile.as_ref() == Some(&entry.receipt.profile.head),
            "A database export another stop took is in the package"
        );
        ensure!(
            declared.contains(&entry.receipt.profile),
            "Database export profile is not packaged"
        );
        ensure!(
            seen.insert(serde_json::to_vec(&entry.receipt)?),
            "Duplicate database export receipt"
        );
        total = total
            .checked_add(entry.receipt.file.size)
            .context("Database export size overflow")?;
        ensure!(
            total <= MAX_DATA_BYTES,
            "Database exports exceed the package limit"
        );
    }
    if let Some(head) = &data.source_profile {
        let profile = declared
            .iter()
            .find(|p| p.head == *head)
            .context("Current source profile missing")?;
        for export in &profile.profile.postgres_exports {
            ensure!(
                data.exports
                    .iter()
                    .any(|entry| entry.receipt.profile == *profile
                        && entry.receipt.export_name == export.name
                        && entry.receipt.source == *source),
                "Current source database export is stale or missing"
            );
        }
    }
    Ok(())
}

fn metadata(head: Option<ProfileHead>, retained: &[RetainedExport]) -> DevelopmentData {
    DevelopmentData {
        source_profile: head,
        exports: retained
            .iter()
            .enumerate()
            .map(|(index, entry)| PackagedExport {
                path: format!("database_exports/{index}"),
                receipt: entry.receipt.clone(),
            })
            .collect(),
    }
}
/// Short metadata inventory for capture tripwires; payload hashing belongs to copy/final verify.
pub(crate) fn inventory(
    daemon: &Daemon,
    stopping: &State,
    declared: &[StoredProfile],
) -> Result<Vec<RetainedExport>> {
    daemon.execution.require_settled_stop(stopping)?;
    let workspace = &stopping.binding.workspace_id;
    let home = daemon.paths.home.canonicalize()?;
    let current = profiles::load(&daemon.store, workspace)?;
    inventory_in(&daemon.store, &home, stopping, declared, current)
}

fn inventory_in(
    store: &crate::store::Store,
    home: &Path,
    stopping: &State,
    declared: &[StoredProfile],
    current: Option<StoredProfile>,
) -> Result<Vec<RetainedExport>> {
    let workspace = &stopping.binding.workspace_id;
    if current.as_ref().is_some_and(|p| !declared.contains(p)) {
        return Err(refused());
    }
    let fence = serde_json::to_string(stopping)?;
    let locals=store.with(|c| {
        let pending:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM workspace_database_exports e LEFT JOIN workspace_preparation_attempts p USING(attempt_id) WHERE e.workspace_id=?1 AND (e.receipt IS NULL OR p.attempt_id IS NULL OR p.workspace_id!=e.workspace_id OR p.settled!=1))",[workspace],|r|r.get(0))?;
        if pending {return Err(rusqlite::Error::InvalidQuery);}
        let mut q=c.prepare("SELECT CASE WHEN length(CAST(e.attempt_id AS BLOB))=36 THEN e.attempt_id END,CASE WHEN length(CAST(e.receipt AS BLOB))<=?3 THEN e.receipt END FROM workspace_database_exports e JOIN workspace_preparation_attempts p USING(attempt_id) WHERE e.workspace_id=?1 AND p.workspace_id=?1 AND p.fence=?2 AND p.settled=1 ORDER BY e.attempt_id LIMIT 33")?;
        let locals=q.query_map(params![workspace,fence,MAX_RECEIPT],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(locals)
    }).map_err(|_|refused())?;
    let mut retained = Vec::new();
    let mut seen = BTreeSet::new();
    // Exports earlier transfers brought here stay here: only this stop's travel (decided with
    // Justin 2026-10-26).
    // The exports the latest complete source stop proved are the ones carried; an export taken
    // again after a restart supersedes the earlier one of the same name.
    let proved: Option<BTreeSet<(String, String)>> =
        crate::workspace_source_stop::latest_receipt(store, stopping)
            .map_err(|_| refused())?
            .map(|(_, receipt)| {
                receipt
                    .databases
                    .iter()
                    .flat_map(|db| &db.exports)
                    .map(|e| (e.export_name.clone(), e.file.sha256.clone()))
                    .collect()
            });
    let mut local_names = BTreeSet::new();
    let locals = match &proved {
        // Later exports of the same name and bytes supersede earlier ones (ordered by attempt).
        Some(_) => {
            let mut last = std::collections::BTreeMap::new();
            for (id, raw) in locals {
                let name = raw
                    .as_deref()
                    .and_then(|raw| serde_json::from_str::<ExportReceipt>(raw).ok())
                    .map(|receipt| receipt.export_name);
                last.insert(
                    name.unwrap_or_else(|| id.clone().unwrap_or_default()),
                    (id, raw),
                );
            }
            last.into_values().collect()
        }
        None => locals,
    };
    for (id, raw) in locals {
        let id = id.ok_or_else(refused)?;
        if !crate::cloud_workspace_handoff::uuid(&id) {
            return Err(refused());
        }
        let receipt: ExportReceipt =
            serde_json::from_str(&raw.ok_or_else(refused)?).map_err(|_| refused())?;
        receipt_valid(&receipt, workspace).map_err(|_| refused())?;
        if proved.as_ref().is_some_and(|proved| {
            !proved.contains(&(receipt.export_name.clone(), receipt.file.sha256.clone()))
        }) {
            continue;
        }
        if receipt.source != *stopping
            || current.as_ref() != Some(&receipt.profile)
            || !local_names.insert(receipt.export_name.clone())
        {
            return Err(refused());
        }
        let root = home.join(format!("workspace-data-export-{id}"));
        if seen.insert(serde_json::to_vec(&receipt)?) {
            retained.push(RetainedExport { root, receipt });
        }
    }
    let data = metadata(current.as_ref().map(|p| p.head.clone()), &retained);
    if retained.is_empty() {
        if current
            .as_ref()
            .is_some_and(|p| !p.profile.postgres_exports.is_empty())
        {
            return Err(refused());
        }
    } else {
        validate(&data, declared, stopping).map_err(|_| refused())?;
    }
    // Imported old bytes cannot satisfy a current declaration even if its profile was recreated.
    if let Some(profile) = current
        && profile
            .profile
            .postgres_exports
            .iter()
            .any(|d| !local_names.contains(&d.name))
    {
        return Err(refused());
    }
    Ok(retained)
}

pub(crate) fn capture(
    daemon: &Daemon,
    stopping: &State,
    package: &Path,
    declared: &[StoredProfile],
) -> Result<Option<DevelopmentData>> {
    let retained = inventory(daemon, stopping, declared)?;
    if retained.is_empty() {
        return Ok(None);
    }
    let head = profiles::load(&daemon.store, &stopping.binding.workspace_id)?.map(|p| p.head);
    let data = metadata(head, &retained);
    snapshot::private_dir(&package.join("database_exports"))?;
    for (entry, source) in data.exports.iter().zip(&retained) {
        copy(&source.root, &package.join(&entry.path), &entry.receipt)?;
    }
    verify(package, &data, declared, stopping)?;
    Ok(Some(data))
}

pub(crate) fn verify(
    package: &Path,
    data: &DevelopmentData,
    declared: &[StoredProfile],
    source: &State,
) -> anyhow::Result<()> {
    validate(data, declared, source)?;
    for entry in &data.exports {
        let root = package.join(&entry.path);
        ensure!(
            export::verify(&root)? == entry.receipt,
            "Packaged database export changed"
        );
        let file = snapshot::open_file(&root, "manifest.json")?
            .context("Database export manifest missing")?;
        let mut bytes = Vec::new();
        file.take(MAX_RECEIPT as u64 + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes == serde_json::to_vec(&entry.receipt)?,
            "Database export manifest is not canonical"
        );
    }
    let root = package.join("database_exports");
    ensure!(
        fs::symlink_metadata(&root)?.is_dir()
            && root.canonicalize()? == package.canonicalize()?.join("database_exports"),
        "Unsafe package database export root"
    );
    ensure!(
        fs::read_dir(root)?.count() == data.exports.len(),
        "Unexpected packaged database export entry"
    );
    Ok(())
}

/// Copies only authenticated dump bytes and canonical metadata into a new private directory.
/// Partial output remains owned by the caller; neither the source nor an existing path is removed.
pub(crate) fn copy(
    root: &Path,
    destination: &Path,
    expected: &ExportReceipt,
) -> anyhow::Result<()> {
    ensure!(
        export::verify(root)? == *expected,
        "Retained database export changed"
    );
    ensure!(
        destination.is_absolute(),
        "Unsafe database export copy path"
    );
    let root = root.canonicalize()?;
    let parent = destination
        .parent()
        .context("Database export copy parent missing")?
        .canonicalize()?;
    let destination = parent.join(
        destination
            .file_name()
            .context("Database export copy name missing")?,
    );
    ensure!(
        !destination.starts_with(&root),
        "Keep database copies outside their source export"
    );
    snapshot::private_dir(&destination)?;
    let source =
        snapshot::open_file(&root, "database.dump")?.context("Database export dump missing")?;
    let mut output = snapshot::private_file(&destination.join("database.dump"))?;
    let (hash, bytes) = snapshot::copy_hash(source, Some(&mut output), expected.file.size)?;
    ensure!(
        hash == expected.file.sha256 && bytes == expected.file.size,
        "Database export changed while copying"
    );
    output.sync_all()?;
    let mut manifest = snapshot::private_file(&destination.join("manifest.json"))?;
    manifest.write_all(&serde_json::to_vec(expected)?)?;
    manifest.sync_all()?;
    snapshot::sync_directories(&destination)?;
    fs::File::open(parent)?.sync_all()?;
    ensure!(
        export::verify(&destination)? == *expected,
        "Copied database export changed"
    );
    ensure!(
        export::verify(&root)? == *expected,
        "Retained database export changed during copy"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cloud_workspace_handoff::{Binding, Transfer},
        cloud_workspace_package::FileEntry,
        store::Store,
        workspace_development_profile::{
            Disposition, DockerBinding, MeaningfulKind, PostgresExport, Profile, Service,
        },
        workspace_service_stop::{SelectedService, ServiceMount},
    };
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};

    struct Fixture {
        _temp: tempfile::TempDir,
        home: PathBuf,
        store: Store,
        state: State,
        profile: StoredProfile,
        retained: RetainedExport,
    }
    fn fixture() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let store = Store::open_in_memory().unwrap();
        let workspace = uuid::Uuid::now_v7().to_string();
        store.with(|c|c.execute("INSERT INTO workspaces(id,slug,path,branch,state,created_at) VALUES(?1,?1,'/fixture','fixture','ready','fixture')",[&workspace])).unwrap();
        let mounts = vec![ServiceMount {
            kind: "volume".into(),
            name: "data".into(),
            source: "/data".into(),
            destination: "/data".into(),
            driver: "local".into(),
            mode: "".into(),
            writable: true,
            propagation: "".into(),
        }];
        let container = SelectedService {
            id: "a".repeat(64),
            image: format!("sha256:{}", "b".repeat(64)),
            created: "2026-10-23T00:00:00Z".into(),
            mounts_digest: URL_SAFE_NO_PAD
                .encode(Sha256::digest(serde_json::to_vec(&mounts).unwrap())),
            mounts,
        };
        let profile = profiles::save_fixture(
            &store,
            Profile {
                version: 2,
                workspace_id: workspace.clone(),
                linux_setup: vec![],
                linux_startup: vec![],
                preview_ports: vec![],
                services: vec![Service {
                    name: "pg".into(),
                    required: true,
                    docker: DockerBinding {
                        socket: "/docker.sock".into(),
                        engine_id: "engine".into(),
                        container: container.clone(),
                    },
                    runtime_memory: Disposition::Reconstructible,
                    writable_layer: Disposition::Reconstructible,
                    mounts: std::collections::BTreeMap::from([(
                        "/data".into(),
                        Disposition::Preserve {
                            data_kind: MeaningfulKind::Database,
                            reason: "Keep records".into(),
                        },
                    )]),
                    compose: None,
                    image: None,
                }],
                postgres_exports: vec![PostgresExport {
                    name: "main".into(),
                    service: "pg".into(),
                    database: "app".into(),
                    username: "owner".into(),
                    socket_dir: "/run/postgresql".into(),
                    port: 5432,
                }],
            },
            None,
        )
        .unwrap();
        let peer = |n: char| format!("12D3KooW{}", n.to_string().repeat(44));
        let state = State {
            version: 1,
            binding: Binding {
                workspace_id: workspace,
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
        };
        let id = uuid::Uuid::now_v7().to_string();
        let root = home.join(format!("workspace-data-export-{id}"));
        snapshot::private_dir(&root).unwrap();
        let bytes = b"PGDMPfixture logical database bytes";
        snapshot::private_file(&root.join("database.dump"))
            .unwrap()
            .write_all(bytes)
            .unwrap();
        let receipt = ExportReceipt {
            version: 1,
            source: state.clone(),
            profile: profile.clone(),
            export_name: "main".into(),
            container,
            file: FileEntry {
                path: "database.dump".into(),
                sha256: hex::encode(Sha256::digest(bytes)),
                size: bytes.len() as u64,
            },
        };
        snapshot::private_file(&root.join("manifest.json"))
            .unwrap()
            .write_all(&serde_json::to_vec(&receipt).unwrap())
            .unwrap();
        store.with(|c| {
            c.execute("INSERT INTO workspace_preparation_attempts VALUES(?1,?2,?3,'fixture',1)",params![id,state.binding.workspace_id,serde_json::to_string(&state).unwrap()])?;
            c.execute("INSERT INTO workspace_database_exports(attempt_id,workspace_id,plan,receipt) VALUES(?1,?2,'ignored',?3)",params![id,state.binding.workspace_id,serde_json::to_string(&receipt).unwrap()])?;
            Ok(())
        }).unwrap();
        Fixture {
            _temp: temp,
            home,
            store,
            state,
            profile,
            retained: RetainedExport { root, receipt },
        }
    }
    fn inventory_fixture(f: &Fixture, current: bool) -> Result<Vec<RetainedExport>> {
        inventory_in(
            &f.store,
            &f.home,
            &f.state,
            std::slice::from_ref(&f.profile),
            current.then(|| f.profile.clone()),
        )
    }

    #[test]
    fn canonical_manifest_and_dump_bytes_are_both_covered_by_the_package_reference() {
        let f = fixture();
        let retained = inventory_fixture(&f, true).unwrap();
        assert_eq!(retained, vec![f.retained.clone()]);
        let package = f.home.join("package");
        snapshot::private_dir(&package).unwrap();
        snapshot::private_dir(&package.join("database_exports")).unwrap();
        let data = metadata(Some(f.profile.head.clone()), &retained);
        let alias = f.home.join("source-alias");
        std::os::unix::fs::symlink(&f.retained.root, &alias).unwrap();
        assert!(
            copy(
                &f.retained.root,
                &alias.join("nested-copy"),
                &f.retained.receipt
            )
            .is_err()
        );
        assert!(!f.retained.root.join("nested-copy").exists());
        copy(
            &f.retained.root,
            &package.join(&data.exports[0].path),
            &f.retained.receipt,
        )
        .unwrap();
        verify(&package, &data, std::slice::from_ref(&f.profile), &f.state).unwrap();
        assert!(
            copy(
                &f.retained.root,
                &package.join(&data.exports[0].path),
                &f.retained.receipt
            )
            .is_err()
        );
        let manifest = package.join("database_exports/0/manifest.json");
        fs::write(
            &manifest,
            format!(" {}", serde_json::to_string(&f.retained.receipt).unwrap()),
        )
        .unwrap();
        assert!(export::verify(&package.join("database_exports/0")).is_ok());
        assert!(verify(&package, &data, std::slice::from_ref(&f.profile), &f.state).is_err());
        fs::write(&manifest, serde_json::to_vec(&f.retained.receipt).unwrap()).unwrap();
        fs::write(
            package.join("database_exports/0/database.dump"),
            b"PGDMPchanged logical database bytes",
        )
        .unwrap();
        assert!(verify(&package, &data, std::slice::from_ref(&f.profile), &f.state).is_err());
        assert_eq!(
            export::verify(&f.retained.root).unwrap(),
            f.retained.receipt
        );
    }

    #[test]
    fn uncertain_missing_duplicate_and_stale_local_exports_cannot_be_dropped() {
        for mutation in [
            "UPDATE workspace_database_exports SET receipt=NULL",
            "UPDATE workspace_preparation_attempts SET settled=0",
            "DELETE FROM workspace_preparation_attempts",
            "UPDATE workspace_database_exports SET receipt='{}'",
            "DELETE FROM workspace_database_exports",
        ] {
            let f = fixture();
            f.store.with(|c| c.execute_batch(mutation)).unwrap();
            assert!(inventory_fixture(&f, true).is_err(), "{mutation}");
        }
        let f = fixture();
        f.store
            .with(|c| {
                c.execute(
                    "UPDATE workspace_database_exports SET receipt=?1",
                    ["é".repeat(MAX_RECEIPT as usize / 2 + 1)],
                )
            })
            .unwrap();
        assert!(inventory_fixture(&f, true).is_err());
        let f = fixture();
        let mut newer = f.profile.profile.clone();
        newer.linux_setup.push("new setup".into());
        let newer = profiles::save_fixture(&f.store, newer, Some(&f.profile.head)).unwrap();
        assert!(
            inventory_in(
                &f.store,
                &f.home,
                &f.state,
                &[f.profile.clone(), newer.clone()],
                Some(newer)
            )
            .is_err()
        );
        let f = fixture();
        let second = uuid::Uuid::now_v7().to_string();
        f.store.with(|c| {
            c.execute("INSERT INTO workspace_preparation_attempts SELECT ?1,workspace_id,fence,daemon_generation,settled FROM workspace_preparation_attempts",[&second])?;
            c.execute("INSERT INTO workspace_database_exports SELECT ?1,workspace_id,plan,receipt FROM workspace_database_exports",[&second])?;Ok(())
        }).unwrap();
        assert!(inventory_fixture(&f, true).is_err());
    }

    #[test]
    fn exports_earlier_transfers_brought_stay_here_and_cannot_replace_a_fresh_current_dump() {
        let f = fixture();
        let mut imported = f.retained.receipt.clone();
        imported.source.transfer.as_mut().unwrap().operation_id = uuid::Uuid::now_v7().to_string();
        f.store
            .with(|c| {
                c.execute("DELETE FROM workspace_database_exports", [])?;
                c.execute(
                    "INSERT INTO workspace_imported_database_exports VALUES(?1,0,?2,?3)",
                    params![
                        f.state.binding.workspace_id,
                        f.retained.root.to_str().unwrap(),
                        serde_json::to_string(&imported).unwrap()
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        // Not carried: only this stop's exports travel.
        assert!(inventory_fixture(&f, false).unwrap().is_empty());
        // Nor can it stand in for the export the current declaration needs.
        assert!(inventory_fixture(&f, true).is_err());
        let earlier = RetainedExport {
            root: f.retained.root.clone(),
            receipt: imported,
        };
        assert!(
            validate(
                &metadata(Some(f.profile.head.clone()), &[earlier]),
                std::slice::from_ref(&f.profile),
                &f.state,
            )
            .is_err(),
            "a package carrying another stop's export is refused"
        );
    }

    #[test]
    fn metadata_refuses_path_scope_count_and_total_size_substitutions() {
        let f = fixture();
        let data = metadata(
            Some(f.profile.head.clone()),
            std::slice::from_ref(&f.retained),
        );
        for mutation in 0..6 {
            let mut invalid = data.clone();
            match mutation {
                0 => invalid.exports[0].path = "../outside".into(),
                1 => invalid.exports.push(invalid.exports[0].clone()),
                2 => {
                    invalid.exports[0].receipt.source.binding.workspace_id =
                        uuid::Uuid::now_v7().to_string()
                }
                3 => invalid.source_profile.as_mut().unwrap().revision += 1,
                4 => invalid.exports.clear(),
                _ => invalid.source_profile = None,
            }
            assert!(validate(&invalid, std::slice::from_ref(&f.profile), &f.state).is_err());
        }
        let mut large = data;
        large.exports[0].receipt.file.size = MAX_DATA_BYTES;
        let mut other = large.exports[0].clone();
        other.path = "database_exports/1".into();
        other.receipt.source.transfer.as_mut().unwrap().operation_id =
            uuid::Uuid::now_v7().to_string();
        large.exports.push(other);
        assert!(validate(&large, std::slice::from_ref(&f.profile), &f.state).is_err());
    }
}
