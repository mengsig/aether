//! The agent each running TUI views, published beside the daemon socket so a
//! CLI command on the same machine can name it `@viewed`. Of several live
//! TUIs, the one whose view changed last wins.
//!
//! Each TUI owns two files in `<socket>.viewed/`: `<id>.lock`, locked
//! exclusively for as long as it runs, and `<id>.json`, what it views. The OS
//! drops the lock however the TUI ends. Readers probe it with a shared lock,
//! which only a running TUI blocks, so readers cleaning up a stale record
//! never look live to each other. No TUI touches another's files.

use std::fs::{File, TryLockError};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use aether_core::id::AgentId;
use aether_core::time::Timestamp;
use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// A record is two short fields; anything longer is not one.
const MAX_RECORD_BYTES: u64 = 4096;

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    agent: AgentId,
    changed_at: Timestamp,
}

/// `<socket file name>.viewed`, beside the daemon socket.
pub fn viewed_dir(daemon_socket: &Path) -> PathBuf {
    let mut name = daemon_socket.file_name().unwrap_or_default().to_owned();
    name.push(".viewed");
    daemon_socket.with_file_name(name)
}

/// The agent the live TUI whose view changed last views, if any TUI does.
pub fn read(dir: &Path) -> anyhow::Result<Option<AgentId>> {
    if !check_dir(dir)? {
        return Ok(None);
    }
    let mut newest: Option<(Timestamp, String, AgentId)> = None;
    for entry in std::fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))? {
        let entry = entry.with_context(|| format!("listing {}", dir.display()))?;
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".json")) else {
            continue;
        };
        if ulid::Ulid::from_string(id).is_err() {
            continue;
        }
        let Some(record) = live_record(dir, id)? else {
            continue;
        };
        let key = (record.changed_at, id.to_owned());
        if newest
            .as_ref()
            .is_none_or(|(at, seen, _)| key > (*at, seen.clone()))
        {
            newest = Some((key.0, key.1, record.agent));
        }
    }
    Ok(newest.map(|(_, _, agent)| agent))
}

/// The record of publisher `id` while it runs; a stale one is removed.
fn live_record(dir: &Path, id: &str) -> anyhow::Result<Option<Record>> {
    let (lock_path, record_path) = (lock_path(dir, id), record_path(dir, id));
    let lock = match open_lock(&lock_path, false) {
        Ok(lock) => Some(lock),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("opening {}", lock_path.display()));
        }
    };
    let live = match lock.as_ref().map(File::try_lock_shared) {
        None | Some(Ok(())) => false,
        Some(Err(TryLockError::WouldBlock)) => true,
        Some(Err(TryLockError::Error(error))) => {
            return Err(error).with_context(|| format!("locking {}", lock_path.display()));
        }
    };
    if live {
        return read_record(&record_path);
    }
    // The publisher is gone and ids are never reused: only readers, which
    // all reach this same verdict, touch its files now.
    remove_if_present(&record_path)?;
    if lock.is_some() {
        remove_if_present(&lock_path)?;
    }
    Ok(None)
}

/// Keeps one TUI's record in step with what it views.
#[derive(Debug)]
pub struct Publisher {
    dir: PathBuf,
    slot: Option<Slot>,
    published: Option<AgentId>,
}

/// This publisher's id and its lock, held until it finishes.
#[derive(Debug)]
struct Slot {
    id: String,
    _lock: File,
}

impl Publisher {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            slot: None,
            published: None,
        }
    }

    /// Publish `viewed` if it changed since the last call. A failure is not
    /// retried until the view changes again, so it is reported once.
    pub fn sync(&mut self, viewed: Option<AgentId>) -> anyhow::Result<()> {
        if viewed == self.published {
            return Ok(());
        }
        self.published = viewed;
        match viewed {
            Some(agent) => self.publish(agent),
            None => self.withdraw(),
        }
    }

    /// Remove this TUI's files as it exits; another TUI's stay.
    pub fn finish(mut self) -> anyhow::Result<()> {
        self.withdraw()?;
        let Some(slot) = self.slot.take() else {
            return Ok(());
        };
        remove_if_present(&lock_path(&self.dir, &slot.id))
    }

    fn publish(&mut self, agent: AgentId) -> anyhow::Result<()> {
        if self.slot.is_none() {
            self.slot = Some(Slot::claim(&self.dir)?);
        }
        let slot = self.slot.as_ref().context("the slot was just claimed")?;
        let record = Record {
            agent,
            changed_at: aether_core::time::now(),
        };
        let bytes = serde_json::to_vec(&record)?;
        debug_assert!(
            bytes.len() as u64 <= MAX_RECORD_BYTES,
            "a record reads back"
        );
        let path = record_path(&self.dir, &slot.id);
        aether_core::private_file::write(&path, &bytes)
            .with_context(|| format!("publishing the viewed agent to {}", path.display()))
    }

    fn withdraw(&self) -> anyhow::Result<()> {
        match &self.slot {
            Some(slot) => remove_if_present(&record_path(&self.dir, &slot.id)),
            None => Ok(()),
        }
    }
}

impl Slot {
    /// A fresh id, locked before its record can exist, so no reader ever
    /// finds the record of a live publisher unlocked.
    fn claim(dir: &Path) -> anyhow::Result<Self> {
        ensure_dir(dir)?;
        let id = ulid::Ulid::generate().to_string();
        let path = lock_path(dir, &id);
        let lock =
            open_lock(&path, true).with_context(|| format!("creating {}", path.display()))?;
        match lock.try_lock() {
            Ok(()) => Ok(Self { id, _lock: lock }),
            Err(error) => Err(io::Error::from(error))
                .with_context(|| format!("locking the new {}", path.display())),
        }
    }
}

fn lock_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.lock"))
}

fn record_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn open_lock(path: &Path, create: bool) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(create).create_new(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        ));
    }
    Ok(file)
}

fn read_record(path: &Path) -> anyhow::Result<Option<Record>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", path.display()));
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_RECORD_BYTES,
        "{} is too large to be a viewed-agent record",
        path.display()
    );
    let record = serde_json::from_slice(&bytes)
        .with_context(|| format!("{} is not a viewed-agent record", path.display()))?;
    Ok(Some(record))
}

fn remove_if_present(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("removing {}", path.display()))
        }
        _ => Ok(()),
    }
}

fn ensure_dir(dir: &Path) -> anyhow::Result<()> {
    if check_dir(dir)? {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    match builder.create(dir) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
            Err(error).with_context(|| format!("creating {}", dir.display()))
        }
        // Another TUI may have created it first; it must still be ours alone.
        _ => check_dir(dir).map(|_| ()),
    }
}

/// Whether `dir` exists, refusing a link, a file, or a directory that is
/// not this user's alone.
fn check_dir(dir: &Path) -> anyhow::Result<bool> {
    let meta = match std::fs::symlink_metadata(dir) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("inspecting {}", dir.display()));
        }
    };
    anyhow::ensure!(meta.is_dir(), "{} is not a directory", dir.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        anyhow::ensure!(
            meta.uid() == nix::unistd::getuid().as_raw() && meta.permissions().mode() & 0o077 == 0,
            "{} is not this user's alone; refusing to use it",
            dir.display()
        );
    }
    Ok(true)
}

#[cfg(test)]
#[path = "viewed_tests.rs"]
mod tests;
