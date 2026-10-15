use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read},
    path::{Component, Path},
    time::{Duration, SystemTime},
};

use aether_store::EngineSessionOwnership;
use nix::{
    fcntl::{AtFlags, OFlag, open, openat},
    sys::stat::{FileStat, Mode, SFlag, fstat, fstatat},
    unistd::{UnlinkatFlags, getuid, unlinkat},
};

const HEADER_BYTES: u64 = 64 * 1024;

#[derive(Debug)]
pub(super) struct LogFile {
    bytes: u64,
    blocker: Option<&'static str>,
    unavailable: bool,
    checked: Option<CheckedFile>,
}

#[derive(Debug)]
struct CheckedFile {
    parent_identity: (nix::libc::dev_t, nix::libc::ino_t),
    path: std::path::PathBuf,
    name: OsString,
    stat: FileStat,
}

impl LogFile {
    pub(super) fn inspect(path: &Path, records: &[&EngineSessionOwnership]) -> io::Result<Self> {
        let mut log = Self {
            bytes: 0,
            blocker: None,
            unavailable: false,
            checked: None,
        };
        let result: io::Result<()> = (|| {
            let parent = open_parent(path)?;
            let name = path
                .file_name()
                .ok_or_else(|| io::Error::other("engine log has no filename"))?;
            let stat = fstatat(&parent, Path::new(name), AtFlags::AT_SYMLINK_NOFOLLOW)?;
            if SFlag::from_bits_truncate(stat.st_mode) != SFlag::S_IFREG {
                log.blocker = Some("not a regular file; symlinks are never followed");
                log.unavailable = true;
                return Ok(());
            }
            log.bytes = u64::try_from(stat.st_size)
                .map_err(|_| io::Error::other("engine log has invalid size"))?;
            if stat.st_nlink != 1 || stat.st_uid != getuid().as_raw() || stat.st_mode & 0o022 != 0 {
                log.blocker = Some("unsafe file ownership, permissions or hard links");
                return Ok(());
            }
            let [record] = records else {
                log.blocker = Some("ambiguous or changed ownership");
                return Ok(());
            };
            if !recognized_path(record, path) {
                log.blocker = Some("unrecognized engine log path");
                return Ok(());
            }
            let file: File = openat(
                &parent,
                Path::new(name),
                OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                Mode::empty(),
            )?
            .into();
            if !same_file(&stat, &fstat(&file)?) {
                log.blocker = Some("file changed during inspection");
                return Ok(());
            }
            if !session_matches(file, record, log.bytes)? {
                log.blocker = Some("native session identity missing or mismatched");
                return Ok(());
            }
            let parent_stat = fstat(&parent)?;
            log.checked = Some(CheckedFile {
                parent_identity: (parent_stat.st_dev, parent_stat.st_ino),
                path: path.to_path_buf(),
                name: name.to_owned(),
                stat,
            });
            Ok(())
        })();
        match result {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                log.blocker = Some("recorded log is missing");
            }
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(
                        nix::libc::ELOOP
                            | nix::libc::ENOTDIR
                            | nix::libc::EACCES
                            | nix::libc::EPERM
                    )
                ) =>
            {
                log.blocker = Some("path cannot be safely inspected");
                log.unavailable = true;
            }
            Err(error) => return Err(error),
        }
        Ok(log)
    }

    pub(super) fn bytes(&self) -> u64 {
        self.bytes
    }

    pub(super) fn blocker(&self) -> Option<&'static str> {
        self.blocker
    }

    pub(super) fn unavailable(&self) -> bool {
        self.unavailable
    }

    pub(super) fn identity(&self) -> Option<(nix::libc::dev_t, nix::libc::ino_t)> {
        self.checked
            .as_ref()
            .map(|file| (file.stat.st_dev, file.stat.st_ino))
    }

    pub(super) fn retention_blocker(
        &self,
        record: &EngineSessionOwnership,
        days: u16,
        now: SystemTime,
    ) -> Option<&'static str> {
        let file = self.checked.as_ref()?;
        let modified = if file.stat.st_mtime >= 0 {
            SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(file.stat.st_mtime as u64))
        } else {
            SystemTime::UNIX_EPOCH
                .checked_sub(Duration::from_secs(file.stat.st_mtime.unsigned_abs()))
        }
        .and_then(|time| time.checked_add(Duration::from_nanos(file.stat.st_mtime_nsec as u64)));
        if modified.is_none_or(|time| !old_enough(time, days, now))
            || !old_enough(SystemTime::from(record.created_at), days, now)
        {
            Some("within retention")
        } else {
            None
        }
    }

    pub(super) fn recheck(&self) -> io::Result<Option<&'static str>> {
        let Some(file) = &self.checked else {
            return Ok(Some("file was not safely inspected"));
        };
        Ok(rechecked_parent(file)?
            .is_none()
            .then_some("file or parent changed since inspection"))
    }

    pub(super) fn remove(&self) -> io::Result<()> {
        let file = self
            .checked
            .as_ref()
            .ok_or_else(|| io::Error::other("file was not safely inspected"))?;
        let parent = rechecked_parent(file)?
            .ok_or_else(|| io::Error::other("file or parent changed since inspection"))?;
        unlinkat(&parent, Path::new(&file.name), UnlinkatFlags::NoRemoveDir)?;
        Ok(())
    }
}

fn rechecked_parent(file: &CheckedFile) -> io::Result<Option<File>> {
    let result: io::Result<Option<File>> = (|| {
        let parent = open_parent(&file.path)?;
        let current = fstat(&parent)?;
        let stat = fstatat(&parent, Path::new(&file.name), AtFlags::AT_SYMLINK_NOFOLLOW)?;
        if file.parent_identity == (current.st_dev, current.st_ino) && same_file(&file.stat, &stat)
        {
            Ok(Some(parent))
        } else {
            Ok(None)
        }
    })();
    match result {
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                || matches!(
                    error.raw_os_error(),
                    Some(
                        nix::libc::ELOOP
                            | nix::libc::ENOTDIR
                            | nix::libc::EACCES
                            | nix::libc::EPERM
                    )
                ) =>
        {
            Ok(None)
        }
        result => result,
    }
}

fn same_file(a: &FileStat, b: &FileStat) -> bool {
    a.st_dev == b.st_dev
        && a.st_ino == b.st_ino
        && a.st_size == b.st_size
        && a.st_mode == b.st_mode
        && a.st_uid == b.st_uid
        && a.st_nlink == b.st_nlink
        && a.st_mtime == b.st_mtime
        && a.st_mtime_nsec == b.st_mtime_nsec
        && a.st_ctime == b.st_ctime
        && a.st_ctime_nsec == b.st_ctime_nsec
}

fn open_parent(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(io::Error::other("engine log path must be absolute"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("engine log has no parent"))?;
    #[cfg(target_os = "linux")]
    let access = OFlag::O_PATH;
    #[cfg(not(target_os = "linux"))]
    let access = OFlag::O_RDONLY;
    let flags = access | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let mut dir: File = open("/", flags, Mode::empty())?.into();
    for component in parent.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                dir = openat(&dir, Path::new(name), flags, Mode::empty())?.into()
            }
            _ => return Err(io::Error::other("engine log path must be normalized")),
        }
    }
    Ok(dir)
}

pub(super) fn recognized_path(record: &EngineSessionOwnership, path: &Path) -> bool {
    let id = &record.session_id;
    if !path.is_absolute()
        || id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    match record.engine.as_str() {
        "claude-code" => {
            name == format!("{id}.jsonl")
                && path
                    .parent()
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    == Some(std::ffi::OsStr::new("projects"))
        }
        "codex" => name.starts_with("rollout-") && name.ends_with(&format!("-{id}.jsonl")),
        _ => false,
    }
}

fn session_matches(file: File, record: &EngineSessionOwnership, bytes: u64) -> io::Result<bool> {
    let mut header = Vec::new();
    file.take(HEADER_BYTES).read_to_end(&mut header)?;
    let complete = header.len() as u64 == bytes;
    let mut lines = header.split(|byte| *byte == b'\n').peekable();
    while let Some(line) = lines.next() {
        if line.is_empty() {
            continue;
        }
        if lines.peek().is_none() && !complete {
            return Ok(false);
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            return Ok(false);
        };
        if record.engine == "codex" {
            return Ok(value["type"] == "session_meta"
                && value["payload"]["id"].as_str() == Some(record.session_id.as_str()));
        }
        if let Some(id) = value["sessionId"].as_str() {
            return Ok(id == record.session_id
                && matches!(
                    value["type"].as_str(),
                    Some("user" | "assistant" | "system" | "progress")
                ));
        }
    }
    Ok(false)
}

pub(super) fn old_enough(time: SystemTime, days: u16, now: SystemTime) -> bool {
    now.duration_since(time)
        .is_ok_and(|age| age >= Duration::from_secs(u64::from(days) * 86400))
}
