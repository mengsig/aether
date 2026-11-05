//! `memory_export` and `memory_import` (docs/memory/MEMORY-V2.md §2.3–2.4):
//! a versioned JSON Lines artifact of memories as stored, and an import that
//! inserts what the store lacks in one commit and never overwrites.

use std::collections::{BTreeMap, HashSet};

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::MemoryId;
use aether_core::memory::{MemoryRecord, MemoryScope};
use aether_core::memory_admission::MemoryState;
use aether_core::memory_v2_reports::{
    EXPORT_FORMAT, EXPORT_VERSION, ExportEntry, ExportHeader, ImportConflict, LastImport,
    MAX_EXPORT_ENCODED_BYTES, MAX_EXPORT_RECORDS, MemoryExport, MemoryImport,
};
use aether_core::time::now;
use aether_store::{Commit, MemoryQuery, MemoryStaleMark, Mutation};
use sha2::Digest as _;

use super::{count, require_operator, stale_note};
use crate::{Result, Runtime, RuntimeError};

/// The meta key the latest applied import is kept under.
pub(super) const LAST_IMPORT_KEY: &str = "memory.last_import";

impl Runtime {
    /// Every memory in `scope` (or the store), as stored, as an artifact.
    pub fn export_memories(&self, scope: Option<MemoryScope>, by: &Actor) -> Result<MemoryExport> {
        require_operator(by, "export memories")?;
        // Every write takes this lock, so holding it makes the record, admission
        // and stale reads one snapshot: an acceptance cannot fall between them.
        let _snapshot = self.hold();
        let records = self.exported_records(scope.as_ref())?;
        let ids: Vec<MemoryId> = records.iter().map(|record| record.id).collect();
        let mut admissions = self.store.memory_admissions(&ids)?;
        let mut stale = self.store.memory_stale_marks(&ids)?;
        let header = ExportHeader {
            format: EXPORT_FORMAT.into(),
            version: EXPORT_VERSION,
            exported_at: now(),
            schema: aether_store::SCHEMA_VERSION,
            scope,
            records: count(records.len()),
        };
        let mut artifact = encode_line(&header)?;
        for record in records {
            let entry = ExportEntry {
                admission: admissions.remove(&record.id),
                stale: stale.remove(&record.id).map(stale_note),
                record,
            };
            debug_assert!(entry.record.standing.is_none(), "standing is never stored");
            artifact.push_str(&encode_line(&entry)?);
        }
        debug_assert!(admissions.is_empty() && stale.is_empty());
        fits_one_frame(&artifact, MAX_EXPORT_ENCODED_BYTES)?;
        Ok(MemoryExport {
            digest: digest(&artifact),
            records: header.records,
            artifact,
        })
    }

    /// Current, candidate, proposed, superseded and forgotten alike, ordered
    /// by creation then id. Refused past [`MAX_EXPORT_RECORDS`].
    fn exported_records(&self, scope: Option<&MemoryScope>) -> Result<Vec<MemoryRecord>> {
        let query = |proposed| MemoryQuery {
            scope: scope.cloned(),
            exact_scope: true,
            nested_scopes: true,
            include_superseded: true,
            include_forgotten: true,
            include_candidates: true,
            proposed,
            limit: MAX_EXPORT_RECORDS + 1,
            ..Default::default()
        };
        let mut found: BTreeMap<MemoryId, MemoryRecord> = BTreeMap::new();
        for proposed in [false, true] {
            for record in self.store.memories(&query(proposed))? {
                found.insert(record.id, record);
            }
        }
        if found.len() > MAX_EXPORT_RECORDS {
            return Err(RuntimeError::Invalid(format!(
                "more than {MAX_EXPORT_RECORDS} memories; export one scope at a time"
            )));
        }
        let mut records: Vec<MemoryRecord> = found.into_values().collect();
        records.sort_by_key(|record| (record.created_at, record.id));
        debug_assert!(records.windows(2).all(|w| w[0].id != w[1].id));
        Ok(records)
    }

    /// Insert every entry the store lacks, report the identical ones and
    /// the conflicting ones, and with `dry_run` write nothing.
    pub fn import_memories(
        &self,
        artifact: &str,
        dry_run: bool,
        by: Actor,
    ) -> Result<MemoryImport> {
        require_operator(&by, "import memories")?;
        let entries = parse_artifact(artifact)?;
        let digest = digest(artifact);
        // Compared under the commit lock, so nothing lands between the
        // comparison and the insert.
        let hold = (!dry_run).then(|| self.hold());
        let plan = self.import_plan(entries)?;
        let report = MemoryImport {
            dry_run,
            digest,
            records: count(plan.inserts.len() + plan.unchanged + plan.conflicts.len()),
            inserted: count(plan.inserts.len()),
            unchanged: count(plan.unchanged),
            conflicts: plan.conflicts,
        };
        let Some(hold) = hold else {
            return Ok(report);
        };
        let commit = import_commit(&plan.inserts, &report, by)?;
        self.apply_held(hold, commit)?;
        for entry in &plan.inserts {
            self.memory.index_committed(&entry.record)?;
        }
        Ok(report)
    }

    fn import_plan(&self, entries: Vec<ExportEntry>) -> Result<ImportPlan> {
        let ids: Vec<MemoryId> = entries.iter().map(|entry| entry.record.id).collect();
        let admissions = self.store.memory_admissions(&ids)?;
        let stale = self.store.memory_stale_marks(&ids)?;
        let mut plan = ImportPlan::default();
        for entry in entries {
            let id = entry.record.id;
            let Some(stored) = self.store.memory(id)? else {
                plan.inserts.push(entry);
                continue;
            };
            let reason = if stored != entry.record {
                Some("the stored record differs")
            } else if admissions.get(&id) != entry.admission.as_ref() {
                Some("the stored admission differs")
            } else if stale.get(&id).cloned().map(stale_note) != entry.stale {
                Some("the stored stale mark differs")
            } else {
                None
            };
            match reason {
                Some(reason) => plan.conflicts.push(ImportConflict {
                    memory: id,
                    reason: reason.into(),
                }),
                None => plan.unchanged += 1,
            }
        }
        debug_assert_eq!(
            plan.inserts.len() + plan.unchanged + plan.conflicts.len(),
            ids.len()
        );
        Ok(plan)
    }
}

#[derive(Default)]
struct ImportPlan {
    inserts: Vec<ExportEntry>,
    unchanged: usize,
    conflicts: Vec<ImportConflict>,
}

/// One commit: every insert with its admission row and stale mark, the
/// `memory.imported` event, and the last-import note status reads.
fn import_commit(inserts: &[ExportEntry], report: &MemoryImport, by: Actor) -> Result<Commit> {
    debug_assert!(!report.dry_run);
    let last = LastImport {
        at: now(),
        digest: report.digest.clone(),
        inserted: report.inserted,
        unchanged: report.unchanged,
        conflicts: count(report.conflicts.len()),
    };
    let payload = serde_json::to_value(&last)
        .map_err(|error| RuntimeError::Internal(format!("encode import note: {error}")))?;
    let mut commit = Commit::new()
        .event(EventDraft::new(
            by,
            EventKind::Custom {
                name: "memory.imported".into(),
                payload: payload.clone(),
            },
        ))
        .mutate(Mutation::SetMeta {
            key: LAST_IMPORT_KEY.into(),
            value: payload.to_string(),
        });
    for entry in inserts {
        commit = commit.mutate(Mutation::PutMemory(entry.record.clone()));
        if let Some(admission) = &entry.admission {
            commit = commit.mutate(Mutation::PutMemoryAdmission(admission.clone()));
        }
        if let Some(note) = &entry.stale {
            commit = commit.mutate(Mutation::MarkMemoryStale(MemoryStaleMark {
                memory: entry.record.id,
                reason: note.reason.clone(),
                since: note.since,
                revision: note.revision.clone(),
            }));
        }
    }
    Ok(commit)
}

/// The artifact's entries, every one checked before anything is compared.
fn parse_artifact(artifact: &str) -> Result<Vec<ExportEntry>> {
    let mut lines = artifact
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty());
    let (_, first) = lines
        .next()
        .ok_or_else(|| invalid("the artifact is empty: it has no header".into()))?;
    let header: ExportHeader = serde_json::from_str(first)
        .map_err(|error| invalid(format!("line 1 is not an export header: {error}")))?;
    if header.format != EXPORT_FORMAT {
        return Err(invalid(format!(
            "unknown artifact format `{}`",
            header.format
        )));
    }
    if header.version == 0 || header.version > EXPORT_VERSION {
        return Err(invalid(format!(
            "export version {} is not one this build reads (up to {EXPORT_VERSION})",
            header.version
        )));
    }
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for (index, line) in lines {
        let entry: ExportEntry = serde_json::from_str(line)
            .map_err(|error| invalid(format!("line {} is malformed: {error}", index + 1)))?;
        check_entry(&entry, index + 1)?;
        if !seen.insert(entry.record.id) {
            return Err(invalid(format!(
                "line {}: memory {} appears twice",
                index + 1,
                entry.record.id
            )));
        }
        if entries.len() == MAX_EXPORT_RECORDS {
            return Err(invalid(format!("more than {MAX_EXPORT_RECORDS} memories")));
        }
        entries.push(entry);
    }
    if entries.len() != header.records as usize {
        return Err(invalid(format!(
            "the header counts {} records but the artifact holds {}",
            header.records,
            entries.len()
        )));
    }
    Ok(entries)
}

fn check_entry(entry: &ExportEntry, line: usize) -> Result<()> {
    let record = &entry.record;
    if record.standing.is_some() {
        return Err(invalid(format!(
            "line {line}: a record carries `standing`, which is never stored"
        )));
    }
    if let Some(admission) = &entry.admission {
        if admission.memory != record.id {
            return Err(invalid(format!(
                "line {line}: the admission names memory {}, not {}",
                admission.memory, record.id
            )));
        }
        if !matches!(
            admission.state,
            MemoryState::Candidate | MemoryState::Active
        ) {
            return Err(invalid(format!(
                "line {line}: admission state `{}` is never stored",
                admission.state.as_str()
            )));
        }
    }
    // The store refuses these on write; refusing here names the line.
    if (record.accepted_at.is_some() && record.proposed_at.is_none())
        || (record.is_proposed() && record.superseded_by.is_some())
    {
        return Err(invalid(format!(
            "line {line}: memory {} has an inconsistent proposal state",
            record.id
        )));
    }
    Ok(())
}

fn encode_line<T: serde::Serialize>(value: &T) -> Result<String> {
    let mut line = serde_json::to_string(value)
        .map_err(|error| RuntimeError::Internal(format!("encode export line: {error}")))?;
    debug_assert!(!line.contains('\n'), "JSON Lines keeps a value on one line");
    line.push('\n');
    Ok(line)
}

/// Refuses an artifact whose escaped JSON string exceeds `limit`, so a reply
/// is never built that the transport would drop as too large.
fn fits_one_frame(artifact: &str, limit: usize) -> Result<()> {
    let encoded = escaped_len(artifact);
    debug_assert!(
        encoded >= artifact.len() + 2,
        "escaping only grows a string"
    );
    if encoded <= limit {
        return Ok(());
    }
    Err(RuntimeError::Invalid(format!(
        "the export is {encoded} bytes on the wire, over the {limit} one frame carries; \
         export one scope at a time"
    )))
}

/// The length `serde_json` gives `text` as a JSON string, quotes included,
/// counted without building it.
fn escaped_len(text: &str) -> usize {
    let extra: usize = text
        .bytes()
        .map(|byte| match byte {
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 0x08 | 0x0c => 1,
            0x00..=0x1f => 5,
            _ => 0,
        })
        .sum();
    text.len() + extra + 2
}

fn digest(artifact: &str) -> String {
    hex::encode(sha2::Sha256::digest(artifact.as_bytes()))
}

fn invalid(message: String) -> RuntimeError {
    RuntimeError::Invalid(format!("memory import: {message}"))
}

#[cfg(test)]
mod tests {
    use super::{escaped_len, fits_one_frame};

    #[test]
    fn escaped_len_is_what_serde_json_writes() {
        for text in [
            "",
            "plain",
            "{\"a\":\"b\\\\c\"}\n",
            "tab\tbell\u{7}nul\u{0}",
            "é ✓ \u{1f600}\r\u{8}\u{c}",
        ] {
            let written = serde_json::to_string(text).expect("encode");
            assert_eq!(escaped_len(text), written.len(), "{text:?}");
        }
    }

    #[test]
    fn an_artifact_past_one_frame_is_refused_with_its_size() {
        let artifact = "{\"record\":1}\n".repeat(4);
        let encoded = escaped_len(&artifact);
        assert!(fits_one_frame(&artifact, encoded).is_ok());
        let refused = fits_one_frame(&artifact, encoded - 1).expect_err("one byte over");
        assert!(
            refused.to_string().contains(&encoded.to_string()),
            "{refused}"
        );
    }
}
