//! Bounded evidence of the latest daemon panic or fatal return, beside its store.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::time::{Timestamp, now};

pub const MAX_RECORD_BYTES: usize = 32 * 1024;
const MAX_MESSAGE_BYTES: usize = 4096;
const MAX_LOCATION_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrashKind {
    Panic,
    FatalError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrashRecord {
    version: u8,
    pub kind: CrashKind,
    pub at: Timestamp,
    pub message: String,
    pub location: Option<String>,
    pub acknowledged: bool,
}

impl CrashRecord {
    pub fn summary(&self) -> String {
        let kind = match self.kind {
            CrashKind::Panic => "panic",
            CrashKind::FatalError => "fatal error",
        };
        format!(
            "Last daemon {kind} at {} ({}): {}. Run `aether doctor` and `aether diagnose`.",
            self.at.to_rfc3339(),
            self.location.as_deref().unwrap_or("location unavailable"),
            self.message,
        )
    }

    fn validate(&self) -> io::Result<()> {
        if self.version != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported crash record version",
            ));
        }
        if self.message.len() > MAX_MESSAGE_BYTES
            || self
                .location
                .as_ref()
                .is_some_and(|s| s.len() > MAX_LOCATION_BYTES)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "crash record fields exceed their limits",
            ));
        }
        Ok(())
    }
}

/// Marker name for a resolved database path; I/O resolves existing store aliases.
pub fn path_for(store: &Path) -> PathBuf {
    let mut path = store.as_os_str().to_owned();
    path.push(".crash.json");
    PathBuf::from(path)
}

fn marker_path(store: &Path) -> io::Result<PathBuf> {
    match std::fs::canonicalize(store) {
        Ok(resolved) => Ok(path_for(&resolved)),
        // A fatal start may happen before the database exists.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path_for(store)),
        Err(error) => Err(error),
    }
}

/// Replace the previous record; successful exits leave evidence untouched.
pub fn record(
    store: &Path,
    kind: CrashKind,
    message: &str,
    location: Option<&str>,
) -> io::Result<()> {
    let record = CrashRecord {
        version: 1,
        kind,
        at: now(),
        message: bounded(message, MAX_MESSAGE_BYTES).to_owned(),
        location: location.map(|s| bounded(s, MAX_LOCATION_BYTES).to_owned()),
        acknowledged: false,
    };
    let path = marker_path(store)?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        let mut directory = std::fs::DirBuilder::new();
        directory.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directory.mode(0o700);
        }
        directory.create(parent)?;
    }
    write(&path, &record)
}

fn bounded(value: &str, limit: usize) -> &str {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    debug_assert!(end <= limit);
    debug_assert!(value.is_char_boundary(end));
    &value[..end]
}

fn write(path: &Path, record: &CrashRecord) -> io::Result<()> {
    record.validate()?;
    let bytes = serde_json::to_vec(record)?;
    debug_assert!(bytes.len() <= MAX_RECORD_BYTES);
    debug_assert!(!bytes.is_empty());
    crate::private_file::write(path, &bytes)
}

/// Refuse links and special files, and never allocate from an untrusted file size.
pub fn read(store: &Path) -> io::Result<Option<CrashRecord>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    let file = match options.open(marker_path(store)?) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "crash evidence is not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "crash evidence exceeds its size limit",
        ));
    }
    let record: CrashRecord = serde_json::from_slice(&bytes)?;
    record.validate()?;
    Ok(Some(record))
}

/// Called under the daemon's store lock, before starting its background tasks.
pub fn acknowledge_startup(store: &Path) -> io::Result<Option<CrashRecord>> {
    let Some(mut record) = read(store)? else {
        return Ok(None);
    };
    if record.acknowledged {
        return Ok(None);
    }
    record.acknowledged = true;
    write(&marker_path(store)?, &record)?;
    Ok(Some(record))
}

/// A failure no daemon start has followed yet; unreadable evidence also counts.
pub fn problem(store: &Path) -> Option<String> {
    match read(store) {
        Ok(Some(record)) if record.acknowledged => None,
        Ok(record) => record.map(|record| record.summary()),
        Err(error) => Some(unreadable(store, &error)),
    }
}

/// Evidence a later daemon start already acknowledged: history, not a problem.
pub fn past_note(store: &Path) -> Option<String> {
    let record = read(store).ok().flatten().filter(|r| r.acknowledged)?;
    let kind = match record.kind {
        CrashKind::Panic => "panic",
        CrashKind::FatalError => "fatal error",
    };
    Some(format!(
        "daemon {kind} at {} ({}): {}; the daemon has started since, so no action is needed",
        record.at.to_rfc3339(),
        record.location.as_deref().unwrap_or("location unavailable"),
        record.message,
    ))
}

fn unreadable(store: &Path, error: &io::Error) -> String {
    format!(
        "Cannot read daemon crash evidence for {}: {error}. Run `aether doctor` and `aether diagnose`.",
        store.display()
    )
}

#[cfg(test)]
mod tests;
