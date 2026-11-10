//! Imported development declarations are retained provenance, never local execution authority.
use std::collections::BTreeSet;

use anyhow::ensure;
use rusqlite::params;

use crate::workspace_development_profile::{self, StoredProfile};
use crate::{Result, store::Store};

const MAX_PROFILES: usize = 32;
const MAX_BYTES: usize = 1024 * 1024;

pub(crate) fn validate(profiles: &[StoredProfile], workspace: &str) -> anyhow::Result<()> {
    ensure!(
        profiles.len() <= MAX_PROFILES,
        "Too many development profiles to transfer"
    );
    let mut seen = BTreeSet::new();
    let mut bytes = 0usize;
    for profile in profiles {
        profile.validate(workspace)?;
        let encoded = serde_json::to_vec(profile)?;
        bytes = bytes
            .checked_add(encoded.len())
            .ok_or_else(|| anyhow::anyhow!("Development profiles exceed the transfer limit"))?;
        ensure!(
            bytes <= MAX_BYTES,
            "Development profiles exceed the transfer limit"
        );
        ensure!(seen.insert(encoded), "Duplicate development profile");
    }
    Ok(())
}

pub(crate) fn capture(store: &Store, workspace: &str) -> Result<Vec<StoredProfile>> {
    let mut profiles = store.with(|c| {
        // Check stored byte lengths before SQLite materializes a potentially corrupt payload.
        let mut query = c.prepare("SELECT ordinal, CASE WHEN length(CAST(profile AS BLOB)) <= ?2 THEN profile END FROM workspace_imported_development_profiles WHERE workspace_id=?1 ORDER BY ordinal LIMIT 33")?;
        let mut rows = query.query(params![workspace, MAX_BYTES as i64])?;
        let mut profiles = Vec::new();
        let mut bytes = 0usize;
        while let Some(row) = rows.next()? {
            let ordinal: i64 = row.get(0)?;
            let raw: Option<String> = row.get(1)?;
            if ordinal != profiles.len() as i64 || profiles.len() == MAX_PROFILES {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let raw = raw.ok_or(rusqlite::Error::InvalidQuery)?;
            bytes = bytes.checked_add(raw.len()).ok_or(rusqlite::Error::InvalidQuery)?;
            if bytes > MAX_BYTES {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let profile = serde_json::from_str(&raw).map_err(|error| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error)))?;
            profiles.push(profile);
        }
        Ok(profiles)
    })?;
    if let Some(current) = workspace_development_profile::load(store, workspace)?
        && !profiles.contains(&current)
    {
        profiles.push(current);
    }
    validate(&profiles, workspace)?;
    Ok(profiles)
}

pub(crate) fn install(
    tx: &rusqlite::Transaction<'_>,
    workspace: &str,
    profiles: &[StoredProfile],
) -> anyhow::Result<()> {
    validate(profiles, workspace)?;
    for (ordinal, profile) in profiles.iter().enumerate() {
        tx.execute("INSERT INTO workspace_imported_development_profiles(workspace_id,ordinal,profile) VALUES(?1,?2,?3)", params![workspace, ordinal as i64, serde_json::to_string(profile)?])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_imports_are_refused_instead_of_becoming_an_empty_profile_list() {
        let store = Store::open_in_memory().unwrap();
        let workspace = uuid::Uuid::now_v7().to_string();
        assert!(capture(&store, &workspace).unwrap().is_empty());
        for (ordinal, raw) in [
            (0, "broken".to_owned()),
            (1, "{}".to_owned()),
            (0, "x".repeat(MAX_BYTES + 1)),
        ] {
            store
                .with(|c| {
                    c.execute("DELETE FROM workspace_imported_development_profiles", [])?;
                    c.execute(
                        "INSERT INTO workspace_imported_development_profiles VALUES(?1,?2,?3)",
                        params![workspace, ordinal, raw],
                    )?;
                    Ok(())
                })
                .unwrap();
            assert!(capture(&store, &workspace).is_err());
        }
    }
}
