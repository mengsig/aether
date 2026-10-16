//! The daemon's own build, published for engines to launch as the MCP bridge.
//!
//! `aether update` replaces the installed binary while the old daemon runs,
//! and a bridge started from that path would speak the new build's protocol.
//! So each daemon publishes the image it runs as `<dir>/aether-<version>-<id>`
//! and holds `<dir>/<id>.lock` for its whole life. A sweep removes the files
//! of an id only while it holds that lock itself, so a live daemon's copy stays.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, File, TryLockError};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};
use rustix::io::Errno;

use crate::agent_temp::{context, file_type, mode_bits, refused};

const PREFIX: &str = "aether-";
const LOCK_SUFFIX: &str = ".lock";

/// A sweep lists at most this many entries before it removes anything.
const MAX_ENTRIES: usize = 10_000;

/// A sweep can take a new lock in the instant before its owner does; the
/// owner then publishes under another id.
const PUBLISH_ATTEMPTS: usize = 3;

/// A directory opened this way is the directory itself, never a link to one.
const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// `<store directory>/bin`, beside the store of the daemon publishing there.
pub fn default_dir(store: &Path) -> Option<PathBuf> {
    store
        .parent()
        .filter(|parent| parent.is_absolute())
        .map(|parent| parent.join("bin"))
}

/// A published copy, locked while this value lives. Dropping it leaves the
/// files for the next sweep; [`Published::remove`] removes them at once.
#[derive(Debug)]
pub struct Published {
    path: PathBuf,
    lock_path: PathBuf,
    lock: File,
    linked: bool,
}

impl Published {
    /// The executable an engine launches as the bridge.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A hard link to the image, rather than a copy of its bytes.
    pub fn linked(&self) -> bool {
        self.linked
    }

    /// Remove the copy, then its lock: a daemon's clean shutdown.
    pub fn remove(self) -> io::Result<()> {
        remove_if_present(&self.path)?;
        remove_if_present(&self.lock_path)?;
        drop(self.lock);
        Ok(())
    }
}

/// What a sweep did with one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepAction {
    Kept,
    WouldRemove,
    Removed,
}

impl SweepAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kept => "kept",
            Self::WouldRemove => "would_remove",
            Self::Removed => "removed",
        }
    }
}

/// One file a sweep looked at, and why it was kept or removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweptFile {
    pub path: PathBuf,
    pub bytes: u64,
    pub action: SweepAction,
    pub reason: &'static str,
}

/// Publish `image` into the private directory `dir`, made 0700 if missing,
/// under `version` and a fresh id. On Linux `image` is `/proc/self/exe`.
pub fn publish(dir: &Path, image: &Path, version: &str) -> io::Result<Published> {
    if !is_version(version) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{version:?} cannot name a bridge copy"),
        ));
    }
    debug_assert!(dir.is_absolute(), "{} is not absolute", dir.display());
    let dir_fd = make_private_dir(dir)?;
    for _ in 0..PUBLISH_ATTEMPTS {
        let id = ulid::Ulid::generate().to_string();
        let lock_name = format!("{id}{LOCK_SUFFIX}");
        let Some(lock) = take_lock(&dir_fd, dir, &lock_name)? else {
            continue;
        };
        // On failure the lock is released here and the sweep removes what is left.
        let name = format!("{PREFIX}{version}-{id}");
        let linked = place(&dir_fd, dir, image, &name)?;
        debug_assert_eq!(classify(&name), Some((id, Kind::Copy)));
        return Ok(Published {
            path: dir.join(name),
            lock_path: dir.join(lock_name),
            lock,
            linked,
        });
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        format!("storage sweeps in {} took every new lock", dir.display()),
    ))
}

/// Create `name` and lock it. `None` when a sweep took the new file first.
fn take_lock(dir: &OwnedFd, dir_path: &Path, name: &str) -> io::Result<Option<File>> {
    let path = dir_path.join(name);
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let file = rustix::fs::openat(dir, name, flags, Mode::RUSR | Mode::WUSR)
        .map(File::from)
        .map_err(|e| context(e, "cannot create", &path))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Error(error)) => return Err(context(error, "cannot lock", &path)),
    }
    // A sweep that locked and removed the file first leaves this lock on an unlinked inode.
    let held = rustix::fs::fstat(&file).map_err(|e| context(e, "cannot inspect", &path))?;
    match rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(named) if named.st_dev == held.st_dev && named.st_ino == held.st_ino => Ok(Some(file)),
        Ok(_) | Err(Errno::NOENT) => Ok(None),
        Err(errno) => Err(context(errno, "cannot inspect", &path)),
    }
}

/// Stage the image at `.<name>`, then rename it to `name`, so the bridge
/// path never names a partial copy. Returns whether the image was linked.
fn place(dir: &OwnedFd, dir_path: &Path, image: &Path, name: &str) -> io::Result<bool> {
    debug_assert!(!name.starts_with('.'), "{name} would be hidden");
    let staged = format!(".{name}");
    let linked = cfg!(target_os = "linux") && link(dir, dir_path, image, &staged)?;
    if !linked {
        copy(dir_path, image, &staged)?;
    }
    rustix::fs::renameat(dir, staged.as_str(), dir, name)
        .map_err(|e| context(e, "cannot publish", &dir_path.join(name)))?;
    Ok(linked)
}

/// Linux refuses to open a running executable for writing, so a link to the
/// daemon's own image keeps its build while it runs. An image that cannot be
/// linked, or that someone else could rewrite, is copied instead.
fn link(dir: &OwnedFd, dir_path: &Path, image: &Path, staged: &str) -> io::Result<bool> {
    // Another filesystem, protected hard links or a deleted image: copy it.
    if rustix::fs::linkat(rustix::fs::CWD, image, dir, staged, AtFlags::SYMLINK_FOLLOW).is_err() {
        return Ok(false);
    }
    let path = dir_path.join(staged);
    let stat = rustix::fs::statat(dir, staged, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|e| context(e, "cannot inspect", &path))?;
    let uid = rustix::process::geteuid().as_raw();
    let trusted = file_type(&stat) == FileType::RegularFile
        && (stat.st_uid == uid || stat.st_uid == 0)
        && mode_bits(&stat) & 0o022 == 0;
    if trusted {
        return Ok(true);
    }
    rustix::fs::unlinkat(dir, staged, AtFlags::empty())
        .map_err(|e| context(e, "cannot remove", &path))?;
    Ok(false)
}

/// Copy the image's bytes into a file only its owner may read and run.
fn copy(dir_path: &Path, image: &Path, staged: &str) -> io::Result<()> {
    let path = dir_path.join(staged);
    // The directory is private and the name fresh, so nobody else can plant an entry there.
    fs::copy(image, &path).map_err(|e| context(e, "cannot copy the daemon's build to", &path))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o500))
        .map_err(|e| context(e, "cannot chmod", &path))
}

/// Remove every copy, staged copy and lock of an id whose lock no daemon
/// holds; with `dry_run`, only report them. Anything else is kept.
pub fn sweep(dir: &Path, dry_run: bool) -> io::Result<Vec<SweptFile>> {
    let dir_fd = match open_dir(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        opened => opened?,
    };
    check_private(dir, &dir_fd)?;
    let (ids, mut swept) = list(&dir_fd, dir)?;
    for (id, files) in ids {
        swept.extend(sweep_id(&dir_fd, dir, &id, files, dry_run)?);
    }
    debug_assert!(swept.iter().all(|file| file.path.starts_with(dir)));
    Ok(swept)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Lock,
    Copy,
}

#[derive(Debug)]
struct Entry {
    name: String,
    kind: Kind,
    bytes: u64,
}

type Listing = (BTreeMap<String, Vec<Entry>>, Vec<SweptFile>);

/// Every regular file this module names, grouped by id, and the entries kept
/// as unrecognized. The whole listing is read before anything is removed.
fn list(dir: &OwnedFd, dir_path: &Path) -> io::Result<Listing> {
    let mut ids: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    let mut unrecognized = Vec::new();
    let mut count = 0;
    for entry in Dir::read_from(dir).map_err(|e| context(e, "cannot list", dir_path))? {
        let entry = entry.map_err(|e| context(e, "cannot list", dir_path))?;
        let raw = entry.file_name().to_bytes();
        if raw == b"." || raw == b".." {
            continue;
        }
        count += 1;
        if count > MAX_ENTRIES {
            return Err(io::Error::other(format!(
                "{} holds more than {MAX_ENTRIES} entries; nothing was removed",
                dir_path.display()
            )));
        }
        let path = dir_path.join(OsStr::from_bytes(raw));
        let stat = match rustix::fs::statat(dir, entry.file_name(), AtFlags::SYMLINK_NOFOLLOW) {
            // Another sweep removed it after the listing.
            Err(Errno::NOENT) => continue,
            stat => stat.map_err(|e| context(e, "cannot inspect", &path))?,
        };
        let bytes = u64::try_from(stat.st_size).map_err(io::Error::other)?;
        let known = std::str::from_utf8(raw)
            .ok()
            .filter(|_| file_type(&stat) == FileType::RegularFile)
            .and_then(|name| classify(name).map(|(id, kind)| (name, id, kind)));
        match known {
            Some((name, id, kind)) => ids.entry(id).or_default().push(Entry {
                name: name.to_owned(),
                kind,
                bytes,
            }),
            None => unrecognized.push(SweptFile {
                path,
                bytes,
                action: SweepAction::Kept,
                reason: "unrecognized file retained",
            }),
        }
    }
    Ok((ids, unrecognized))
}

/// The id and kind of a name `publish` writes: `<id>.lock`, or the copy
/// `aether-<version>-<id>`, staged under a leading dot.
fn classify(name: &str) -> Option<(String, Kind)> {
    if let Some(id) = name.strip_suffix(LOCK_SUFFIX) {
        return is_id(id).then(|| (id.to_owned(), Kind::Lock));
    }
    let copy = name.strip_prefix('.').unwrap_or(name);
    let (version, id) = copy.strip_prefix(PREFIX)?.rsplit_once('-')?;
    (is_version(version) && is_id(id)).then(|| (id.to_owned(), Kind::Copy))
}

fn is_id(text: &str) -> bool {
    text.len() == 26 && ulid::Ulid::from_string(text).is_ok()
}

/// A version is part of a file name, so it is a short token of its own.
fn is_version(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && !text.starts_with('.')
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+' | b'_'))
}

/// Remove an id's files if its lock is free, holding that lock meanwhile.
/// Copies go before the lock, so no copy is left without one to guard it.
fn sweep_id(
    dir: &OwnedFd,
    dir_path: &Path,
    id: &str,
    mut files: Vec<Entry>,
    dry_run: bool,
) -> io::Result<Vec<SweptFile>> {
    debug_assert!(!files.is_empty(), "an id is listed only with its files");
    debug_assert!(
        files
            .iter()
            .all(|f| classify(&f.name).is_some_and(|(of, _)| of == id))
    );
    let lock = match lock_state(dir, dir_path, id)? {
        LockState::Held => {
            return Ok(decide(
                dir_path,
                files,
                SweepAction::Kept,
                "its daemon is running",
            ));
        }
        LockState::Free(lock) => lock,
    };
    files.sort_by_key(|file| file.kind == Kind::Lock);
    if !dry_run {
        for file in &files {
            remove_at(dir, &file.name, &dir_path.join(&file.name))?;
        }
    }
    drop(lock);
    let action = if dry_run {
        SweepAction::WouldRemove
    } else {
        SweepAction::Removed
    };
    Ok(decide(
        dir_path,
        files,
        action,
        "no running daemon holds its lock",
    ))
}

fn decide(
    dir_path: &Path,
    files: Vec<Entry>,
    action: SweepAction,
    reason: &'static str,
) -> Vec<SweptFile> {
    files
        .into_iter()
        .map(|file| SweptFile {
            path: dir_path.join(file.name),
            bytes: file.bytes,
            action,
            reason,
        })
        .collect()
}

#[derive(Debug)]
enum LockState {
    Held,
    /// Taken by this sweep until dropped, or `None` when the lock is gone.
    Free(Option<File>),
}

fn lock_state(dir: &OwnedFd, dir_path: &Path, id: &str) -> io::Result<LockState> {
    let name = format!("{id}{LOCK_SUFFIX}");
    let path = dir_path.join(&name);
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let file = match rustix::fs::openat(dir, name.as_str(), flags, Mode::empty()) {
        Ok(fd) => File::from(fd),
        // Publication locks before it copies, and removal unlocks last, so these are orphans.
        Err(Errno::NOENT) => return Ok(LockState::Free(None)),
        Err(errno) => return Err(context(errno, "cannot open", &path)),
    };
    match file.try_lock() {
        Ok(()) => Ok(LockState::Free(Some(file))),
        Err(TryLockError::WouldBlock) => Ok(LockState::Held),
        Err(TryLockError::Error(error)) => Err(context(error, "cannot lock", &path)),
    }
}

/// Create `dir` 0700 if missing and open it, refusing a link, or a directory
/// someone else owns or may enter.
fn make_private_dir(dir: &Path) -> io::Result<OwnedFd> {
    let created = match rustix::fs::mkdir(dir, Mode::RWXU) {
        Ok(()) => true,
        Err(Errno::EXIST) => false,
        Err(errno) => return Err(context(errno, "cannot create", dir)),
    };
    let dir_fd = open_dir(dir)?;
    if created {
        // The umask may have narrowed the mode below what the owner needs.
        rustix::fs::fchmod(&dir_fd, Mode::RWXU).map_err(|e| context(e, "cannot chmod", dir))?;
    }
    check_private(dir, &dir_fd)?;
    Ok(dir_fd)
}

fn open_dir(dir: &Path) -> io::Result<OwnedFd> {
    rustix::fs::open(dir, DIR_FLAGS, Mode::empty()).map_err(|errno| match errno {
        // What O_NOFOLLOW and O_DIRECTORY report for a link or a non-directory.
        Errno::LOOP | Errno::NOTDIR | Errno::MLINK => {
            refused(format!("{} is not a real directory", dir.display()))
        }
        errno => context(errno, "cannot open", dir),
    })
}

/// Engines run what is in here, so only its owner may change or even list it.
fn check_private(dir: &Path, dir_fd: &OwnedFd) -> io::Result<()> {
    let stat = rustix::fs::fstat(dir_fd).map_err(|e| context(e, "cannot inspect", dir))?;
    let uid = rustix::process::geteuid().as_raw();
    if stat.st_uid != uid {
        return Err(refused(format!(
            "{} is owned by uid {}, not {uid}; remove it so Aether can recreate it",
            dir.display(),
            stat.st_uid
        )));
    }
    let mode = mode_bits(&stat);
    if mode & 0o077 != 0 || mode & 0o700 != 0o700 {
        return Err(refused(format!(
            "{} has mode {mode:o}, not 700; run `chmod 700` on it",
            dir.display()
        )));
    }
    Ok(())
}

fn remove_at(dir: &OwnedFd, name: &str, path: &Path) -> io::Result<()> {
    match rustix::fs::unlinkat(dir, name, AtFlags::empty()) {
        // A concurrent sweep removed it first.
        Ok(()) | Err(Errno::NOENT) => Ok(()),
        Err(errno) => Err(context(errno, "cannot remove", path)),
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            Err(context(error, "cannot remove", path))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "bridge_bin_tests.rs"]
mod tests;
