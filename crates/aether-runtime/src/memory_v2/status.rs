//! `memory_migration_status`: where the store stands on Memory 2.0's
//! migrations, what it holds, and the backups a rollback would restore.

use std::path::Path;

use aether_core::memory_v2_reports::{
    LastImport, MemoryCounts, MemoryMigration, MemoryMigrationStatus, MigrationBackup,
};

use super::transfer::LAST_IMPORT_KEY;
use crate::storage::parse_backup_name;
use crate::{Result, Runtime, RuntimeError};

/// The migrations Memory 2.0 added, oldest first.
const MEMORY_MIGRATIONS: [(u32, &str); 3] = [
    (30, "memory admission"),
    (31, "stale marks and the holdout log"),
    (32, "memory 2.0 protocol (no schema change)"),
];

impl Runtime {
    /// Readable by every role: counts and backup file names, no host path.
    pub fn memory_migration_status(&self) -> Result<MemoryMigrationStatus> {
        let stamp = self
            .store
            .meta("schema_version")?
            .ok_or_else(|| RuntimeError::Internal("the store has no schema stamp".into()))?;
        let schema_version: u32 = stamp.parse().map_err(|_| {
            RuntimeError::Internal(format!("schema stamp `{stamp}` is not a number"))
        })?;
        let counts = self.store.memory_v2_counts()?;
        debug_assert!(counts.admitted + counts.implicit == counts.total);
        let wire = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
        let last_import = match self.store.meta(LAST_IMPORT_KEY)? {
            Some(note) => Some(serde_json::from_str::<LastImport>(&note).map_err(|error| {
                RuntimeError::Internal(format!("the last-import note does not decode: {error}"))
            })?),
            None => None,
        };
        let backups = match &self.config.storage_paths {
            Some(paths) => match paths.store.parent() {
                Some(root) => migration_backups(&root.join("migration-backups"))?,
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        Ok(MemoryMigrationStatus {
            schema_version,
            migrations: MEMORY_MIGRATIONS
                .iter()
                .map(|(version, name)| MemoryMigration {
                    version: *version,
                    name: (*name).into(),
                    applied: *version <= schema_version,
                })
                .collect(),
            memories: MemoryCounts {
                total: wire(counts.total),
                admitted: wire(counts.admitted),
                implicit: wire(counts.implicit),
                candidates: wire(counts.candidates),
                stale: wire(counts.stale),
                held_out: wire(counts.held_out),
            },
            backups,
            last_import,
        })
    }
}

/// The pre-migration backups in `dir`, newest first; none when it is absent.
fn migration_backups(dir: &Path) -> Result<Vec<MigrationBackup>> {
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(RuntimeError::Internal(format!(
                "list migration backups: {error}"
            )));
        }
    };
    let io =
        |error: std::io::Error| RuntimeError::Internal(format!("read a migration backup: {error}"));
    let mut backups = Vec::new();
    for entry in listing {
        let entry = entry.map_err(io)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((from_version, to_version, id)) = parse_backup_name(&name) else {
            continue;
        };
        let at = chrono::DateTime::from_timestamp_millis(id.timestamp_ms() as i64)
            .ok_or_else(|| RuntimeError::Internal(format!("backup {name} has no valid time")))?;
        backups.push(MigrationBackup {
            name,
            from_version,
            to_version,
            bytes: entry.metadata().map_err(io)?.len(),
            at,
        });
    }
    backups.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.name.cmp(&a.name)));
    debug_assert!(backups.iter().all(|backup| !backup.name.contains('/')));
    Ok(backups)
}
