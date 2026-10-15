//! Size- and age-bounded segments shared safely by writers of one log stream.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use aether_core::disk::DiskFloor;

/// Records the disk refused, kept to write once it takes them: a full disk
/// is when the log matters most and can be written least.
const BACKLOG_BYTES: usize = 4 * 1024 * 1024;

pub(super) struct BoundedLog {
    directory: PathBuf,
    segment_bytes: u64,
    segments: usize,
    retention: Option<Duration>,
    disk_floor: Option<DiskFloor>,
    reported_error: bool,
    /// Bytes accepted but not yet written, oldest first.
    backlog: Vec<u8>,
    backlog_bytes: usize,
    /// Records refused once the backlog was full; said in their place.
    dropped: u64,
    drop_marker_needs_newline: bool,
}

impl BoundedLog {
    #[cfg(test)]
    pub(super) fn new(directory: PathBuf, segment_bytes: u64, segments: usize) -> io::Result<Self> {
        Self::with_policy(directory, segment_bytes, segments, 0, None)
    }

    pub(super) fn with_policy(
        directory: PathBuf,
        segment_bytes: u64,
        segments: usize,
        retention_days: u16,
        disk_floor: Option<DiskFloor>,
    ) -> io::Result<Self> {
        if segment_bytes == 0 || segments == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "log limits must be positive",
            ));
        }
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        let directory = std::fs::canonicalize(directory)?;
        check_directory(&directory)?;
        let writer = Self {
            directory,
            segment_bytes,
            segments,
            retention: (retention_days != 0)
                .then(|| Duration::from_secs(u64::from(retention_days) * 24 * 60 * 60)),
            disk_floor,
            reported_error: false,
            backlog: Vec::new(),
            backlog_bytes: BACKLOG_BYTES,
            dropped: 0,
            drop_marker_needs_newline: false,
        };
        let _lock = writer.lock()?;
        for index in 0..writer.segments {
            private_existing(&writer.path(index))?;
        }
        writer.current()?;
        writer.maintain(SystemTime::now())?;
        Ok(writer)
    }

    fn path(&self, index: usize) -> PathBuf {
        self.directory.join(format!("events.{index}.log"))
    }

    fn lock(&self) -> io::Result<File> {
        let path = self.directory.join("writer.lock");
        let file = open_private_file(
            &path,
            OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true),
        )?;
        fs4::FileExt::lock(&file)?;
        // Closing the handle releases the OS lock, including on errors.
        Ok(file)
    }

    fn current(&self) -> io::Result<File> {
        let path = self.path(0);
        open_private_file(&path, OpenOptions::new().create(true).append(true))
    }

    fn rotate(&self) -> io::Result<()> {
        // Validate the entire set before removing anything. This directory is
        // a versioned, dedicated namespace, never the legacy log directory.
        let present = (0..self.segments)
            .map(|i| private_existing(&self.path(i)))
            .collect::<io::Result<Vec<_>>>()?;
        if present[self.segments - 1] {
            std::fs::remove_file(self.path(self.segments - 1))?;
        }
        for index in (1..self.segments).rev() {
            if present[index - 1] {
                std::fs::rename(self.path(index - 1), self.path(index))?;
            }
        }
        Ok(())
    }

    fn maintain(&self, now: SystemTime) -> io::Result<()> {
        let Some(retention) = self.retention else {
            return Ok(());
        };
        let expired = |time| now.duration_since(time).is_ok_and(|age| age >= retention);
        let file = self.current()?;
        let metadata = file.metadata()?;
        // Birth time bounds a busy segment's age. Filesystems without birth
        // times can still rotate idle segments by their last modification.
        let modified = metadata.modified()?;
        let started = match metadata.created() {
            Ok(created) => created.min(modified),
            Err(error) if error.kind() == io::ErrorKind::Unsupported => modified,
            Err(error) => return Err(error),
        };
        if metadata.len() > 0 && expired(started) {
            drop(file);
            self.rotate()?;
            self.current()?;
        }
        // Validate every candidate before deleting any. Never follow links or
        // infer ownership from an age or a similar-looking legacy filename.
        let stale = (1..self.segments)
            .map(|index| {
                let path = self.path(index);
                match open_private_file(&path, OpenOptions::new().read(true)) {
                    Ok(file) => Ok(expired(file.metadata()?.modified()?).then_some(path)),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(error),
                }
            })
            .collect::<io::Result<Vec<_>>>()?;
        for path in stale.into_iter().flatten() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    fn check_disk_floor(&self) -> io::Result<()> {
        let Some(floor) = self.disk_floor else {
            return Ok(());
        };
        let available = fs4::available_space(&self.directory)?;
        let floor_bytes = floor.bytes(fs4::total_space(&self.directory)?);
        if available < floor_bytes {
            return Err(io::Error::other(format!(
                "log disk space below floor: {available} bytes available, {floor_bytes} required"
            )));
        }
        Ok(())
    }

    fn write_locked(&self, bytes: &[u8]) -> io::Result<usize> {
        self.check_disk_floor()?;
        let mut file = self.current()?;
        let mut size = file.metadata()?.len();
        if size >= self.segment_bytes {
            drop(file);
            self.rotate()?;
            file = self.current()?;
            size = 0;
        }
        let remaining = usize::try_from(self.segment_bytes - size).unwrap_or(usize::MAX);
        file.write(&bytes[..bytes.len().min(remaining)])
    }

    /// Write the backlog out, then the count of any records dropped behind
    /// it, so the file keeps its order.
    fn drain(&mut self) -> io::Result<()> {
        if self.backlog.is_empty() && self.dropped == 0 {
            return Ok(());
        }
        let _lock = self.lock()?;
        self.maintain(SystemTime::now())?;
        self.write_backlog()?;
        if self.dropped > 0 {
            let separator = if self.drop_marker_needs_newline {
                "\n"
            } else {
                ""
            };
            let marker = format!(
                "{separator}{} WARN aether::logging: dropped {} log records the disk would not take\n",
                aether_core::time::now().format("%Y-%m-%dT%H:%M:%S%.6fZ"),
                self.dropped
            );
            self.backlog.extend_from_slice(marker.as_bytes());
            self.dropped = 0;
            self.drop_marker_needs_newline = false;
            self.write_backlog()?;
        }
        Ok(())
    }

    fn write_backlog(&mut self) -> io::Result<()> {
        while !self.backlog.is_empty() {
            let written = match self.write_locked(&self.backlog) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if written == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "log write made no progress",
                ));
            }
            self.backlog.drain(..written);
        }
        Ok(())
    }

    fn report<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        if let Err(error) = &result {
            if !self.reported_error {
                eprintln!(
                    "Aether could not write logs in {}: {error}",
                    self.directory.display()
                );
            }
            self.reported_error = true;
        } else {
            self.reported_error = false;
        }
        result
    }
}

/// An error from `write_all` means the record waits in the backlog for the
/// next write or flush or, the backlog full, was dropped and will be counted.
impl Write for BoundedLog {
    /// Never queues: an error means none of `bytes` was taken.
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let result = self.drain().and_then(|()| {
            let _lock = self.lock()?;
            self.maintain(SystemTime::now())?;
            self.write_locked(bytes)
        });
        self.report(result)
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut written = 0;
        let result = self.drain().and_then(|()| {
            let _lock = self.lock()?;
            self.maintain(SystemTime::now())?;
            while written < bytes.len() {
                let n = match self.write_locked(&bytes[written..]) {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    result => result?,
                };
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "log write made no progress",
                    ));
                }
                written += n;
            }
            Ok(())
        });
        if result.is_err() {
            let remaining = &bytes[written..];
            // After a gap, refuse later records until the dropped count is written.
            if self.dropped == 0
                && remaining.len() <= self.backlog_bytes.saturating_sub(self.backlog.len())
            {
                self.backlog.extend_from_slice(remaining);
            } else {
                self.dropped += 1;
                self.drop_marker_needs_newline |= written > 0 && bytes[written - 1] != b'\n';
            }
        }
        self.report(result)
    }

    fn flush(&mut self) -> io::Result<()> {
        // Writes go directly to File; only the backlog waits in userspace.
        let result = self.drain();
        self.report(result)
    }
}

fn open_validated_file(path: &Path, options: &mut OpenOptions) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a regular log file: {}", path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "log file is not owned by the current user: {}",
                    path.display()
                ),
            ));
        }
        if metadata.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("log file has multiple hard links: {}", path.display()),
            ));
        }
    }
    Ok(file)
}

fn open_private_file(path: &Path, options: &mut OpenOptions) -> io::Result<File> {
    let file = open_validated_file(path, options)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

/// Inspect the logger's destination without creating it or changing its permissions.
pub(crate) fn check_log_file(path: &Path) -> io::Result<std::fs::Metadata> {
    open_validated_file(path, OpenOptions::new().append(true))?.metadata()
}

fn private_existing(path: &Path) -> io::Result<bool> {
    match open_private_file(path, OpenOptions::new().read(true)) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn check_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // A writable ancestor permits replacing even a private namespace directory.
        for ancestor in directory.ancestors() {
            let metadata = std::fs::metadata(ancestor)?;
            if (metadata.uid() != 0 && metadata.uid() != rustix::process::geteuid().as_raw())
                || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "log directory can be replaced by another user: {}",
                        ancestor.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
