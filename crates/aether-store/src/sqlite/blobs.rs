use super::{SqliteStore, memory_key};
use crate::{Result, Store, StoreError};
use aether_core::{artifact::Artifact, id::ArtifactId, time::now};
use rusqlite::{OptionalExtension, params};
use std::io::Write as _;
use std::path::PathBuf;

impl SqliteStore {
    pub(super) fn evict_blob(&self, id: ArtifactId) -> Result<()> {
        let mut conn = self.conn.lock();
        // Mark durably before unlink, so interrupted deletion remains eligible for retry.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let body: Option<String> = tx
            .query_row(
                "SELECT body FROM artifacts WHERE id = ?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(body) = body else {
            return Err(StoreError::Corrupt(format!(
                "artifact {id} is not recorded"
            )));
        };
        let mut artifact: Artifact = serde_json::from_str(&body)?;
        if artifact.evicted_at.is_none() {
            artifact.evicted_at = Some(now());
            tx.execute(
                "UPDATE artifacts SET evicted = 1, body = ?2 WHERE id = ?1",
                params![id.to_string(), serde_json::to_string(&artifact)?],
            )?;
        }
        let Some(path) = artifact.path.clone().or_else(|| self.artifact_path(id)) else {
            tx.commit()?;
            return Ok(());
        };
        tx.commit()?;
        // Exclude new references between the final path check and unlink.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if live_path_reference(&tx, &path)? {
            tx.execute("UPDATE artifacts SET unlink_pending = 0 WHERE id = ?1 OR json_extract(body, '$.path') = ?2", params![id.to_string(), path.to_string_lossy()])?;
            tx.commit()?;
            return Ok(());
        }
        if let Some(key) = memory_key(&path) {
            self.mem_artifacts.lock().remove(key);
        } else {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        tx.execute(
            "UPDATE artifacts SET unlink_pending = 0 WHERE id = ?1 OR json_extract(body, '$.path') = ?2",
            params![id.to_string(), path.to_string_lossy()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(super) fn remove_unreferenced_blob(&self, id: ArtifactId) -> Result<()> {
        let path = self
            .artifact_path(id)
            .unwrap_or_else(|| PathBuf::from(format!("memory://{id}")));
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let referenced: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM artifacts WHERE evicted = 0 AND id = ?1)",
            [id.to_string()],
            |r| r.get(0),
        )?;
        if let Some(dir) = &self.staging_dir {
            let staged = dir.join(id.to_string());
            if !live_explicit_path_reference(&tx, &path)?
                && !live_explicit_path_reference(&tx, &staged)?
            {
                match std::fs::remove_file(staged) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        if referenced || live_path_reference(&tx, &path)? {
            tx.commit()?;
            return Ok(());
        }
        if let Some(key) = memory_key(&path) {
            self.mem_artifacts.lock().remove(key);
        } else {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub(super) fn retry_evicted_blobs(&self) -> Result<()> {
        let Some(dir) = &self.artifact_dir else {
            return Ok(());
        };
        let mut conn = self.conn.lock();
        // Exclude other connections' record commits until all unlink decisions finish.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut after = String::new();
        loop {
            let records: Vec<Artifact> = Self::read_many(
                &tx,
                "SELECT body FROM artifacts WHERE unlink_pending = 1 AND id > ?1 ORDER BY id LIMIT 1000",
                &[&after],
            )?;
            if records.is_empty() {
                break;
            }
            for artifact in records {
                after = artifact.id.to_string();
                let path = artifact.path.or_else(|| self.artifact_path(artifact.id));
                if let Some(path) = path {
                    // Only retry managed payloads; external paths do not belong to startup cleanup.
                    let managed = path.parent() == Some(dir.as_path())
                        && path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.parse::<ArtifactId>().is_ok());
                    if managed {
                        if live_path_reference(&tx, &path)? {
                            continue;
                        }
                        match std::fs::remove_file(&path) {
                            Ok(()) => {}
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => {
                                tracing::warn!(artifact = %artifact.id, path = %path.display(), %error,
                                    "evicted artifact unlink failed; will retry on next startup");
                                continue;
                            }
                        }
                    }
                }
                tx.execute(
                    "UPDATE artifacts SET unlink_pending = 0 WHERE id = ?1",
                    [artifact.id.to_string()],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub(super) fn write_blob(&self, id: ArtifactId, bytes: &[u8]) -> Result<PathBuf> {
        match self.artifact_path(id) {
            Some(path) => {
                let parent = path.parent().ok_or_else(|| {
                    StoreError::Corrupt(format!("artifact path has no parent: {}", path.display()))
                })?;
                // A unique file prevents overlapping writes from sharing a
                // staging path, and cannot follow a stale symlink there.
                let mut temp = tempfile::NamedTempFile::new_in(parent)?;
                temp.write_all(bytes)?;
                temp.persist(&path).map_err(|error| error.error)?;
                Ok(path)
            }
            None => {
                let key = format!("memory://{id}");
                self.mem_artifacts
                    .lock()
                    .insert(key.clone(), bytes.to_vec());
                Ok(PathBuf::from(key))
            }
        }
    }

    pub(super) fn read_blob(&self, id: ArtifactId) -> Result<Option<Vec<u8>>> {
        // The record's own path, because a deduplicated record points at the
        // bytes another record stored.
        let artifact = self.artifact(id)?;
        let live = artifact
            .as_ref()
            .is_some_and(|a| a.evicted_at.is_none() && a.path.is_some());
        let recorded = artifact.and_then(|a| a.path);
        let Some(path) = recorded.or_else(|| self.artifact_path(id)) else {
            return Ok(None);
        };
        if let Some(key) = memory_key(&path) {
            return Ok(self.mem_artifacts.lock().get(key).cloned());
        }
        use std::io::Read;
        let Some(mut file) = self.open_blob_file(&path, live)? else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    pub(super) fn import_blob(
        &self,
        id: ArtifactId,
        source: &std::path::Path,
    ) -> Result<crate::ImportedArtifact> {
        use sha2::{Digest, Sha256};
        use std::io::{Read, Write};
        let mut input = std::fs::File::open(source)?;
        let copy = |output: &mut std::fs::File| -> Result<_> {
            let mut hash = Sha256::new();
            let mut size_bytes = 0;
            let mut buffer = [0; 64 * 1024];
            loop {
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                output.write_all(&buffer[..count])?;
                hash.update(&buffer[..count]);
                size_bytes += count as u64;
            }
            output.sync_all()?;
            // Hash the copied snapshot, not another read of the mutable source.
            let digest = hex::encode(hash.finalize());
            let shared = if let Some(twin) = self.artifact_by_digest(&digest)?
                && let Some(path) = twin.path
                && self.artifact_bytes_exist(twin.id)?
            {
                Some((path, twin.id))
            } else {
                None
            };
            Ok((size_bytes, digest, shared))
        };
        let mut memory_temp = None;
        let (publication, (size_bytes, digest, shared)) = if self.staging_dir.is_some() {
            let (publication, copied) = self.write_staged_blob(id, copy)?;
            (Some(publication), copied)
        } else {
            let mut temp = tempfile::NamedTempFile::new()?;
            let mut copy = copy;
            let copied = copy(temp.as_file_mut())?;
            memory_temp = Some(temp);
            (None, copied)
        };
        if let Some((path, twin)) = shared {
            if publication.is_some() {
                self.remove_unreferenced_blob(id)?;
            }
            return Ok(crate::ImportedArtifact {
                path,
                size_bytes,
                digest,
                shared_with: Some(twin),
                publication: None,
            });
        }
        let publication = match publication {
            Some(publication) => publication,
            None => self.stage_artifact_bytes(
                id,
                &std::fs::read(memory_temp.ok_or(StoreError::NoArtifactDir)?.path())?,
            )?,
        };
        let path = publication.path.clone();
        Ok(crate::ImportedArtifact {
            path,
            size_bytes,
            digest,
            shared_with: None,
            publication: Some(publication),
        })
    }

    pub(super) fn read_blob_range(
        &self,
        id: ArtifactId,
        offset: u64,
        limit: usize,
    ) -> Result<Option<Vec<u8>>> {
        use std::io::{Read, Seek, SeekFrom};
        let artifact = self.artifact(id)?;
        let live = artifact
            .as_ref()
            .is_some_and(|a| a.evicted_at.is_none() && a.path.is_some());
        let recorded = artifact.and_then(|a| a.path);
        let Some(path) = recorded.or_else(|| self.artifact_path(id)) else {
            return Ok(None);
        };
        if let Some(key) = memory_key(&path) {
            return Ok(self.mem_artifacts.lock().get(key).map(|bytes| {
                let start = usize::try_from(offset)
                    .unwrap_or(usize::MAX)
                    .min(bytes.len());
                bytes[start..start.saturating_add(limit).min(bytes.len())].to_vec()
            }));
        }
        let Some(mut file) = self.open_blob_file(&path, live)? else {
            return Ok(None);
        };
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.take(limit as u64).read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    pub(super) fn blob_exists(&self, id: ArtifactId) -> Result<bool> {
        let artifact = self.artifact(id)?;
        let live = artifact
            .as_ref()
            .is_some_and(|a| a.evicted_at.is_none() && a.path.is_some());
        let recorded = artifact.and_then(|a| a.path);
        let Some(path) = recorded.or_else(|| self.artifact_path(id)) else {
            return Ok(false);
        };
        match memory_key(&path) {
            Some(key) => Ok(self.mem_artifacts.lock().contains_key(key)),
            None => Ok(self.open_blob_file(&path, live)?.is_some()),
        }
    }
}

pub(super) fn live_path_reference(
    conn: &rusqlite::Connection,
    path: &std::path::Path,
) -> Result<bool> {
    // Separate predicates keep the path expression index and primary key usable.
    let owner = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.parse::<ArtifactId>().ok());
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM artifacts WHERE evicted = 0 AND json_extract(body, '$.path') = ?1)
         OR EXISTS(SELECT 1 FROM artifacts WHERE evicted = 0 AND id = ?2 AND json_extract(body, '$.path') IS NULL)",
        params![path.to_string_lossy(), owner.map(|id| id.to_string())],
        |row| row.get(0),
    )?)
}

pub(super) fn live_explicit_path_reference(
    conn: &rusqlite::Connection,
    path: &std::path::Path,
) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM artifacts WHERE evicted = 0 AND json_extract(body, '$.path') = ?1)",
        [path.to_string_lossy()],
        |row| row.get(0),
    )?)
}

impl SqliteStore {
    /// Read cleanup journals and staged names without retrying unlink or promotion.
    pub fn pending_artifact_cleanup(
        &self,
    ) -> Result<aether_core::retention::PendingArtifactCleanup> {
        let conn = self.read_conn()?;
        let mut stmt =
            conn.prepare("SELECT id FROM artifacts WHERE unlink_pending=1 ORDER BY id")?;
        let ids = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut report = aether_core::retention::PendingArtifactCleanup::default();
        for id in ids {
            report.pending_unlinks.push(
                id?.parse().map_err(|e| {
                    StoreError::Corrupt(format!("invalid pending artifact id: {e}"))
                })?,
            );
        }
        if let Some(dir) = &self.staging_dir {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                if entry.file_type()?.is_file()
                    && let Some(id) = entry
                        .file_name()
                        .to_str()
                        .and_then(|name| name.parse::<ArtifactId>().ok())
                {
                    report.staged_publications.push(id);
                }
            }
        }
        report.staged_publications.sort();
        Ok(report)
    }
}
