//! Keyed operations, written before they are sent and removed once answered,
//! per endpoint and store. One left behind is a lost acknowledgement.
use crate::{ClientError, Result};
use aether_core::time::{Timestamp, now};
use aether_protocol::Command;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// Unresolved operations one endpoint may hold before new sends are refused.
pub(crate) const MAX_UNRESOLVED: usize = 1024;
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const SETTLED_LOG: &str = "settled.jsonl";

/// One operation the host has not been seen to answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unresolved {
    pub command: Command,
    /// The host's `submission_horizon` when the command was first sent. Never
    /// rewritten: a later one would vouch for receipts dropped in between.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizon: Option<String>,
    pub sent_at: Timestamp,
    /// Why the host could not say what became of it. Set once the host has
    /// said so and never cleared: only a person settles it from then on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unconfirmed: Option<String>,
}

impl Unresolved {
    /// The command's idempotency key. Every entry has one: nothing unkeyed is
    /// recorded, and one that lost its key does not load.
    pub fn key(&self) -> &str {
        match self.command.keyed_request() {
            Some(Ok(keyed)) => keyed.key,
            _ => "",
        }
    }
}

/// An outbox file that cannot be read back as an operation this client wrote.
/// It never stops a connection; only a person discards it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unreadable {
    /// The file's name without `.json`: what `outbox discard` takes.
    pub key: String,
    pub path: PathBuf,
    /// Always starts with `unreadable:` and says what was wrong.
    pub reason: String,
}

/// Everything in an outbox, readable or not.
#[derive(Debug, Default)]
pub(crate) struct Pending {
    pub(crate) entries: Vec<Unresolved>,
    pub(crate) unreadable: Vec<Unreadable>,
}

/// Exclusive hold on one entry, for as long as it is kept. A send in flight
/// holds its entry so another process adopting the outbox leaves it alone.
#[derive(Debug)]
pub(crate) struct Claim {
    pub(crate) key: String,
    file: std::fs::File,
}

impl Drop for Claim {
    fn drop(&mut self) {
        // A concurrent fork can keep the descriptor alive until the child execs.
        // The claim's lifetime, not that unrelated child's, owns the lock.
        if let Err(error) = self.file.unlock() {
            // Closing remains safe: any inherited descriptors retain the lock
            // until they close, delaying resolution rather than allowing overlap.
            tracing::warn!(key = %self.key, %error,
                "could not unlock outbox claim; closing it and awaiting any inherited descriptors");
        }
    }
}

pub(crate) enum Claimed {
    Held(Claim, Box<Unresolved>),
    /// Another live process holds it, or it was replaced while locking.
    Busy,
    /// It was retired between the listing and now.
    Gone,
}

/// How an unresolved operation left the outbox, for the settled log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Settled {
    /// The host held its receipt.
    Executed,
    /// The host proved it never ran, and it was sent again unchanged.
    Resent,
    /// The host could not say; it now waits for a person.
    Unconfirmed,
    MarkedDelivered,
    SentAgain,
    Discarded,
}

#[derive(Serialize)]
struct SettledLine<'a> {
    at: Timestamp,
    key: &'a str,
    cmd: String,
    settled: Settled,
    #[serde(skip_serializing_if = "str::is_empty")]
    detail: &'a str,
}

#[derive(Debug)]
pub(crate) struct Outbox {
    directory: PathBuf,
}

impl Outbox {
    pub(crate) async fn open(directory: PathBuf) -> Result<Self> {
        tokio::fs::create_dir_all(&directory).await?;
        use std::os::unix::fs::PermissionsExt as _;
        tokio::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).await?;
        Ok(Self { directory })
    }

    fn path(&self, key: &str) -> PathBuf {
        self.directory.join(format!(
            "{}.json",
            hex::encode(Sha256::digest(key.as_bytes()))
        ))
    }

    /// Write `command` down before it is first sent, with the horizon it is
    /// sent under.
    pub(crate) async fn record(&self, command: &Command, horizon: Option<&str>) -> Result<Claim> {
        let keyed = command
            .keyed_request()
            .ok_or_else(|| ClientError::InvalidUsage("outbox command has no key".into()))??;
        let key = keyed.key.to_owned();
        if let Some(existing) = self.read(&self.path(&key)).await? {
            return Err(if existing.command == *command {
                ClientError::DeliveryUnconfirmed(format!(
                    "operation {key} is already unresolved; it is resolved on the next \
                     connection, or settle it with `aether outbox`"
                ))
            } else {
                ClientError::InvalidUsage(
                    "outbox key was reused with different command fields".into(),
                )
            });
        }
        if self.entries().await?.len() >= MAX_UNRESOLVED {
            return Err(ClientError::DeliveryUnconfirmed(format!(
                "not sent: {MAX_UNRESOLVED} operations are already unresolved for this host; \
                 settle them with `aether outbox` before sending more"
            )));
        }
        let entry = Unresolved {
            command: command.clone(),
            horizon: horizon.map(str::to_owned),
            sent_at: now(),
            unconfirmed: None,
        };
        self.publish(key, &serde_json::to_vec(&entry)?).await
    }

    async fn staged(&self, bytes: &[u8]) -> Result<(PathBuf, std::fs::File)> {
        debug_assert!(!bytes.is_empty());
        let temporary = self
            .directory
            .join(format!(".{:032x}.writing", rand::random::<u128>()));
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
        Ok((temporary, file.into_std().await))
    }

    async fn sync_directory(&self) -> Result<()> {
        tokio::fs::File::open(&self.directory)
            .await?
            .sync_all()
            .await?;
        Ok(())
    }

    /// Publish `bytes` as the entry for `key`, locked from the moment it
    /// exists. The lock lives as long as the returned claim.
    async fn publish(&self, key: String, bytes: &[u8]) -> Result<Claim> {
        debug_assert!(!key.is_empty());
        let (temporary, file) = self.staged(bytes).await?;
        file.try_lock()
            .map_err(|error| std::io::Error::other(format!("cannot lock a new entry: {error}")))?;
        // Held as a claim from here, so every early return below unlocks it.
        let claim = Claim { key, file };
        // A hard link, not a rename: it fails where an entry already stands.
        let linked = tokio::fs::hard_link(&temporary, self.path(&claim.key)).await;
        tokio::fs::remove_file(&temporary).await?;
        match linked {
            Ok(()) => {
                self.sync_directory().await?;
                Ok(claim)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(ClientError::DeliveryUnconfirmed(format!(
                    "operation {} was recorded concurrently; it is resolved on the next \
                     connection",
                    claim.key
                )))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Note that the host could not say what became of `entry`. Everything
    /// else in it, its horizon above all, stays as first written.
    pub(crate) async fn mark_unconfirmed(
        &self,
        claim: &Claim,
        entry: &Unresolved,
        reason: &str,
    ) -> Result<()> {
        assert_eq!(claim.key, entry.key(), "the claim is on this entry");
        assert!(!reason.is_empty(), "an unconfirmed entry says why");
        let marked = Unresolved {
            unconfirmed: Some(reason.to_owned()),
            ..entry.clone()
        };
        let (temporary, _) = self.staged(&serde_json::to_vec(&marked)?).await?;
        tokio::fs::rename(&temporary, self.path(&claim.key)).await?;
        self.sync_directory().await
    }

    pub(crate) async fn acknowledge(&self, claim: &Claim) -> Result<()> {
        match tokio::fs::remove_file(self.path(&claim.key)).await {
            Ok(()) => self.sync_directory().await,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Append one line to the record of how operations left this outbox.
    pub(crate) async fn log(
        &self,
        entry: &Unresolved,
        settled: Settled,
        detail: &str,
    ) -> Result<()> {
        self.log_line(entry.key(), &entry.command.name(), settled, detail)
            .await
    }

    async fn log_line(&self, key: &str, cmd: &str, settled: Settled, detail: &str) -> Result<()> {
        let mut line = serde_json::to_vec(&SettledLine {
            at: now(),
            key,
            cmd: cmd.to_owned(),
            settled,
            detail,
        })?;
        line.push(b'\n');
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(self.directory.join(SETTLED_LOG))
            .await?;
        file.write_all(&line).await?;
        file.sync_all().await?;
        Ok(())
    }

    async fn entries(&self) -> Result<Vec<PathBuf>> {
        let mut directory = tokio::fs::read_dir(&self.directory).await?;
        let mut paths = Vec::new();
        while let Some(entry) = directory.next_entry().await? {
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                paths.push(entry.path());
            }
        }
        Ok(paths)
    }

    /// For a caller that can only fail: any fault names the file.
    async fn read(&self, path: &Path) -> Result<Option<Unresolved>> {
        self.read_entry(path).await.map_err(|what| {
            ClientError::Malformed(format!("remote outbox entry {}: {what}", path.display()))
        })
    }

    /// `Err` says what is wrong with this one file and nothing else.
    async fn read_entry(&self, path: &Path) -> std::result::Result<Option<Unresolved>, String> {
        let metadata = match tokio::fs::metadata(path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot be examined: {error}")),
        };
        if !metadata.is_file() || metadata.len() > MAX_ENTRY_BYTES {
            return Err("not an operation this client wrote".into());
        }
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot be read: {error}")),
        };
        let raw: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("cannot be decoded: {error}"))?;
        // Before horizons an entry was the bare command, which nothing vouches for.
        let entry = if raw.get("cmd").is_some() {
            let modified = metadata
                .modified()
                .map_err(|error| format!("has no modification time: {error}"))?;
            Unresolved {
                command: serde_json::from_value(raw)
                    .map_err(|error| format!("holds an unknown command: {error}"))?,
                horizon: None,
                sent_at: modified.into(),
                unconfirmed: None,
            }
        } else {
            serde_json::from_value(raw).map_err(|error| format!("cannot be decoded: {error}"))?
        };
        if entry.key().is_empty() {
            return Err("its operation has no idempotency key".into());
        }
        if path != self.path(entry.key()) {
            return Err("its filename does not match its key".into());
        }
        Ok(Some(entry))
    }

    /// Every operation, in the order they were first sent, and every file
    /// that is not one. A bad file never fails the listing.
    pub(crate) async fn pending(&self) -> Result<Pending> {
        let mut pending = Pending::default();
        for path in self.entries().await? {
            match self.read_entry(&path).await {
                Ok(Some(entry)) => pending.entries.push(entry),
                // One retired between the listing and the read is simply gone.
                Ok(None) => {}
                Err(what) => pending.unreadable.push(Unreadable {
                    key: path
                        .file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    reason: format!("unreadable: {what} ({})", path.display()),
                    path,
                }),
            }
        }
        pending
            .entries
            .sort_by(|a, b| (a.sent_at, a.key()).cmp(&(b.sent_at, b.key())));
        pending.unreadable.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(pending)
    }

    /// Take the entry under `key` for as long as the claim is kept, reading
    /// it again once it is held so it is what is on disk now.
    pub(crate) async fn claim(&self, key: &str) -> Result<Claimed> {
        use std::os::unix::fs::MetadataExt as _;
        let path = self.path(key);
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Claimed::Gone);
            }
            Err(error) => return Err(error.into()),
        };
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(Claimed::Busy),
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        // Held as a claim from here, so every early return below unlocks it.
        let claim = Claim {
            key: key.to_owned(),
            file,
        };
        let replaced = match std::fs::metadata(&path) {
            Ok(now) => now.ino() != claim.file.metadata()?.ino(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Claimed::Gone),
            Err(error) => return Err(error.into()),
        };
        if replaced {
            return Ok(Claimed::Busy);
        }
        match self.read(&path).await? {
            Some(entry) => Ok(Claimed::Held(claim, Box::new(entry))),
            None => Ok(Claimed::Gone),
        }
    }

    /// Remove a file [`Self::pending`] could not read, and say so in the log.
    pub(crate) async fn discard_unreadable(&self, bad: &Unreadable) -> Result<()> {
        assert!(
            bad.path.starts_with(&self.directory)
                && bad.path.extension().is_some_and(|e| e == "json"),
            "only an outbox entry file is discarded"
        );
        // The name alone made it an entry, so it may be a directory; only an empty one is removed.
        let removed = match tokio::fs::symlink_metadata(&bad.path).await {
            Ok(found) if found.is_dir() => tokio::fs::remove_dir(&bad.path).await,
            Ok(_) => tokio::fs::remove_file(&bad.path).await,
            Err(error) => Err(error),
        };
        match removed {
            Ok(()) => self.sync_directory().await?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                let path = bad.path.display();
                return Err(std::io::Error::new(error.kind(), format!("{path}: {error}")).into());
            }
        }
        self.log_line(&bad.key, "unreadable", Settled::Discarded, &bad.reason)
            .await
    }
}

#[cfg(test)]
#[path = "outbox_tests.rs"]
mod tests;
