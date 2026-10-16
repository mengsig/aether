//! Per-agent temporary directories.
//!
//! One private root per Aether instance, claimed by a marker holding the
//! store's instance id, holds one 0700 leaf per agent. The leaf is named by
//! the random tail of the agent's id, so it greps against the id. Processes
//! an agent starts get the leaf as `TMPDIR`, `TMP` and `TEMP`.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Component, Path, PathBuf};

use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, Stat};
use rustix::io::Errno;

use crate::id::AgentId;

/// The variables an agent's processes find their temp directory in.
pub const ENV_VARS: [&str; 3] = ["TMPDIR", "TMP", "TEMP"];

/// The root's claim: the id of the instance whose agents own its leaves.
pub const MARKER: &str = ".aether-instance";

/// A leaf is the last 12 characters of the agent's ULID: its random part.
pub const LEAF_LEN: usize = 12;

/// Tools put Unix sockets, capped at 108 bytes, under `TMPDIR`; a leaf path
/// longer than this leaves them too little room.
pub const PATH_BUDGET: usize = 50;

/// An instance id is a short token; anything longer is not a marker we wrote.
const MAX_MARKER_BYTES: u64 = 256;

/// A directory opened this way is the directory itself, never a link to one.
const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// `<store directory>/t`, beside the store whose instance owns it.
pub fn default_root(store: &Path) -> Option<PathBuf> {
    store
        .parent()
        .filter(|parent| parent.is_absolute())
        .map(|parent| parent.join("t"))
}

/// The leaf name of `agent`: uppercase, as the id renders.
pub fn leaf_name(agent: AgentId) -> String {
    let ulid = agent.ulid().to_string();
    let name = ulid[ulid.len() - LEAF_LEN..].to_owned();
    debug_assert!(is_leaf_name(&name), "{name} is not a leaf name");
    name
}

/// Twelve Crockford base32 characters, the only names a leaf can have.
pub fn is_leaf_name(name: &str) -> bool {
    name.len() == LEAF_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b.is_ascii_uppercase() && !b"ILOU".contains(&b)))
}

/// Where `agent`'s directory lives under `root`. Nothing is created.
pub fn leaf_path(root: &Path, agent: AgentId) -> PathBuf {
    debug_assert!(root.is_absolute(), "{} is not absolute", root.display());
    root.join(leaf_name(agent))
}

/// `root` rebuilt from its components. It must be absolute, UTF-8 and plain.
fn normalize_root(root: &Path) -> io::Result<PathBuf> {
    let invalid = |why: &str| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("agents.temp_root {} {why}", root.display()),
        )
    };
    if root.to_str().is_none() {
        return Err(invalid("is not valid UTF-8"));
    }
    if !root.is_absolute() {
        return Err(invalid("is not absolute"));
    }
    if root.components().any(|c| c == Component::ParentDir) {
        return Err(invalid("must not contain `..`"));
    }
    let normalized: PathBuf = root.components().collect();
    if normalized.parent().is_none() {
        return Err(invalid("is the filesystem root"));
    }
    debug_assert!(normalized.is_absolute(), "{}", normalized.display());
    debug_assert!(normalized.as_os_str().len() <= root.as_os_str().len());
    Ok(normalized)
}

/// `root` with every link resolved: its deepest existing ancestor made
/// canonical, and the rest appended. Each directory on it can then be checked.
fn resolve_root(root: &Path) -> io::Result<PathBuf> {
    let root = normalize_root(root)?;
    let existing = deepest_existing(&root)?;
    let resolved =
        fs::canonicalize(existing).map_err(|e| context(e, "cannot resolve", existing))?;
    let rest = root.strip_prefix(existing).map_err(io::Error::other)?;
    // A link may lead to `/` or to a path that is not UTF-8.
    let resolved = normalize_root(&resolved.join(rest))?;
    debug_assert!(resolved.ends_with(rest), "{}", resolved.display());
    Ok(resolved)
}

/// Resolve `root`, create it if missing, refuse it unless it and its
/// ancestors are safe, and claim it for `instance_id`. Returns the real root.
pub fn prepare_root(root: &Path, instance_id: &str) -> io::Result<PathBuf> {
    debug_assert!(!instance_id.trim().is_empty(), "an instance id is required");
    let root = resolve_root(root)?;
    let dir = make_root(&root, rustix::process::geteuid().as_raw())?;
    claim_root(&dir, &root, instance_id)?;
    Ok(root)
}

/// Refuse a `root` the daemon would refuse, creating and claiming nothing,
/// and return its real path. A missing root passes if it could be made.
pub fn inspect_root(root: &Path, instance_id: Option<&str>) -> io::Result<PathBuf> {
    let root = resolve_root(root)?;
    let uid = rustix::process::geteuid().as_raw();
    check_ancestors(&root, uid)?;
    let dir = match open_root(&root, uid) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            check_creatable(&root)?;
            return Ok(root);
        }
        opened => opened?,
    };
    let claimed = match read_marker(&dir, &root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(root),
        read => read?,
    };
    match instance_id {
        Some(instance_id) if claimed.trim() != instance_id => {
            Err(claimed_elsewhere(&root, &claimed))
        }
        _ => Ok(root),
    }
}

/// Remove the private files a claim stages its marker in, left behind when
/// a daemon died mid-claim. Call only while nothing else claims `root`.
pub fn remove_staged_markers(root: &Path) -> io::Result<Vec<String>> {
    let dir = open_root(root, rustix::process::geteuid().as_raw())?;
    let mut removed = Vec::new();
    for entry in Dir::read_from(&dir).map_err(|e| context(e, "cannot list", root))? {
        let entry = entry.map_err(|e| context(e, "cannot list", root))?;
        let Some(name) = entry
            .file_name()
            .to_str()
            .ok()
            .filter(|n| is_staged_marker(n))
        else {
            continue;
        };
        let path = root.join(name);
        let stat = rustix::fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| context(e, "cannot inspect", &path))?;
        if file_type(&stat) != FileType::RegularFile {
            continue;
        }
        rustix::fs::unlinkat(&dir, name, AtFlags::empty())
            .map_err(|e| context(e, "cannot remove", &path))?;
        removed.push(name.to_owned());
    }
    debug_assert!(removed.iter().all(|n| is_staged_marker(n)), "{removed:?}");
    Ok(removed)
}

/// `.aether-instance.<ULID>`, the name `publish_marker` stages a claim in.
fn is_staged_marker(name: &str) -> bool {
    name.strip_prefix(MARKER)
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|id| ulid::Ulid::from_string(id).is_ok())
}

/// Create `leaf` if missing (recreating the root first if it went too), and
/// refuse it unless it is our own 0700 directory in a root this instance
/// still holds. Everything is checked again on every call. The root must be
/// the real path `prepare_root` returned.
pub fn ensure_leaf(leaf: &Path, instance_id: &str) -> io::Result<()> {
    let (Some(root), Some(name)) = (leaf.parent(), leaf.file_name()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an agent temp dir needs a root",
        ));
    };
    debug_assert!(
        name.to_str().is_some_and(is_leaf_name),
        "{} is not a leaf",
        leaf.display()
    );
    debug_assert!(!instance_id.trim().is_empty(), "an instance id is required");
    let uid = rustix::process::geteuid().as_raw();
    // The root is already real, so it is made again where it was, never resolved anew.
    let dir = match open_root(root, uid) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => make_root(root, uid)?,
        opened => opened?,
    };
    claim_root(&dir, root, instance_id)?;
    create_leaf(&dir, leaf, name)?;
    let stat = rustix::fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|e| context(e, "cannot inspect", leaf))?;
    check_leaf(leaf, &stat, uid)
}

/// What to tell an operator about `root` before agents use it: a path too
/// long for the sockets tools make under it, or a filesystem held in memory.
pub fn warnings(root: &Path) -> Vec<String> {
    debug_assert!(root.is_absolute(), "warnings judge the resolved root");
    let mut warnings = Vec::new();
    let bytes = root.as_os_str().len() + 1 + LEAF_LEN;
    if bytes > PATH_BUDGET {
        // Claude Code's sandbox points TMPDIR one level deeper, at claude-<uid>.
        let uid = rustix::process::geteuid().as_raw();
        let sandboxed = bytes + format!("/claude-{uid}").len();
        warnings.push(format!(
            "agent temp dirs under {} are {bytes} bytes ({sandboxed} inside Claude Code's \
             claude-{uid} sandbox dir), over the {PATH_BUDGET}-byte budget that leaves Unix \
             sockets room under TMPDIR; set a shorter agents.temp_root",
            root.display()
        ));
    }
    match volatile_filesystem(root) {
        Ok(Some(kind)) => warnings.push(format!(
            "agents.temp_root {} is on {kind}, which holds agent scratch in memory; \
             set agents.temp_root to a disk-backed directory",
            root.display()
        )),
        Ok(None) => {}
        Err(error) => warnings.push(format!(
            "cannot tell which filesystem holds agents.temp_root {}: {error}",
            root.display()
        )),
    }
    warnings
}

/// `tmpfs` or `ramfs` when `path`, or its nearest existing ancestor, is on one.
#[cfg(target_os = "linux")]
fn volatile_filesystem(path: &Path) -> io::Result<Option<&'static str>> {
    const TMPFS_MAGIC: u64 = 0x0102_1994;
    const RAMFS_MAGIC: u64 = 0x8584_58f6;
    let probe = path
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no existing ancestor"))?;
    // The field's width and sign vary by architecture; both magics fit either way.
    #[allow(clippy::unnecessary_cast)]
    let kind = rustix::fs::statfs(probe)?.f_type as u64;
    Ok(match kind {
        TMPFS_MAGIC => Some("tmpfs"),
        RAMFS_MAGIC => Some("ramfs"),
        _ => None,
    })
}

#[cfg(not(target_os = "linux"))]
fn volatile_filesystem(_path: &Path) -> io::Result<Option<&'static str>> {
    Ok(None)
}

/// `path` itself or its deepest ancestor that exists, or may: one that
/// cannot be inspected is returned so that the caller's own access reports why.
fn deepest_existing(path: &Path) -> io::Result<&Path> {
    path.ancestors()
        .find(|dir| !matches!(fs::symlink_metadata(dir), Err(e) if e.kind() == io::ErrorKind::NotFound))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no ancestor of {} exists", path.display()),
            )
        })
}

/// Make the real `root` a 0700 directory if missing, creating its missing
/// ancestors private, and open it. Its ancestors are checked first.
fn make_root(root: &Path, uid: u32) -> io::Result<OwnedFd> {
    check_ancestors(root, uid)?;
    if let Some(parent) = root.parent() {
        // Ancestors made here are private, so the check above still holds.
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|e| context(e, "cannot create", parent))?;
    }
    create_root(root, uid)
}

/// Whoever can rename an ancestor of the real `root` can swap the root, so
/// each one that exists must be a directory that is ours or root's.
fn check_ancestors(root: &Path, uid: u32) -> io::Result<()> {
    debug_assert!(root.is_absolute(), "{} is not absolute", root.display());
    for dir in root.ancestors().skip(1) {
        let stat = match rustix::fs::lstat(dir) {
            Err(Errno::NOENT) => continue,
            stat => stat.map_err(|e| context(e, "cannot inspect", dir))?,
        };
        check_ancestor(root, dir, &stat, uid)?;
    }
    Ok(())
}

/// One `dir` above `root`: a shared one needs the sticky bit, so that only
/// an entry's owner can rename it. A link means the root is no longer real.
fn check_ancestor(root: &Path, dir: &Path, stat: &Stat, uid: u32) -> io::Result<()> {
    let fix = "point agents.temp_root at a directory under parents only you or root can change";
    if file_type(stat) != FileType::Directory {
        return Err(refused(format!(
            "agents.temp_root {}: its parent {} is not a real directory; {fix}",
            root.display(),
            dir.display()
        )));
    }
    if stat.st_uid != uid && stat.st_uid != 0 {
        return Err(refused(format!(
            "agents.temp_root {}: its parent {} is owned by uid {}, not {uid} or root; {fix}",
            root.display(),
            dir.display(),
            stat.st_uid
        )));
    }
    let mode = mode_bits(stat);
    if mode & 0o022 != 0 && mode & 0o1000 == 0 {
        return Err(refused(format!(
            "agents.temp_root {}: its parent {} is writable by group or others without the \
             sticky bit (mode {mode:o}); {fix}",
            root.display(),
            dir.display()
        )));
    }
    Ok(())
}

/// A missing root is made by the daemon, which needs to write and search its
/// nearest existing ancestor.
fn check_creatable(root: &Path) -> io::Result<()> {
    let existing = deepest_existing(root)?;
    rustix::fs::accessat(
        rustix::fs::CWD,
        existing,
        rustix::fs::Access::WRITE_OK | rustix::fs::Access::EXEC_OK,
        AtFlags::EACCESS,
    )
    .map_err(|e| context(e, "cannot create agents.temp_root under", existing))
}

/// Make `root` a 0700 directory if missing, then open and check it as
/// `open_root` does. The mode is set only on what this call made.
fn create_root(root: &Path, uid: u32) -> io::Result<OwnedFd> {
    let created = match rustix::fs::mkdirat(rustix::fs::CWD, root, Mode::RWXU) {
        Ok(()) => true,
        Err(Errno::EXIST) => false,
        Err(errno) => return Err(context(errno, "cannot create agents.temp_root", root)),
    };
    let dir = open_dir(root)?;
    if created {
        // The umask may have narrowed the mode below what the owner needs.
        rustix::fs::fchmod(&dir, Mode::RWXU).map_err(|e| context(e, "cannot chmod", root))?;
    }
    let stat = rustix::fs::fstat(&dir).map_err(|e| context(e, "cannot inspect", root))?;
    check_root(root, &stat, uid)?;
    Ok(dir)
}

/// Open `root` and refuse it unless it is private to us. A missing root is
/// reported as `NotFound`.
fn open_root(root: &Path, uid: u32) -> io::Result<OwnedFd> {
    let dir = open_dir(root)?;
    let stat = rustix::fs::fstat(&dir).map_err(|e| context(e, "cannot inspect", root))?;
    check_root(root, &stat, uid)?;
    Ok(dir)
}

/// Open `root` itself, never a link in its place.
fn open_dir(root: &Path) -> io::Result<OwnedFd> {
    rustix::fs::open(root, DIR_FLAGS, Mode::empty()).map_err(|errno| match errno {
        // What O_NOFOLLOW and O_DIRECTORY report for a link or a non-directory.
        Errno::LOOP | Errno::NOTDIR | Errno::MLINK => not_a_directory(root),
        errno => context(errno, "cannot open agents.temp_root", root),
    })
}

/// Make `name` a 0700 directory in the root `dir`. One that already exists
/// is left as it is for the caller to inspect.
fn create_leaf(dir: &OwnedFd, leaf: &Path, name: &OsStr) -> io::Result<()> {
    match rustix::fs::mkdirat(dir, name, Mode::RWXU) {
        Ok(()) => {}
        Err(Errno::EXIST) => return Ok(()),
        Err(errno) => return Err(context(errno, "cannot create agent temp dir", leaf)),
    }
    let created = rustix::fs::openat(dir, name, DIR_FLAGS, Mode::empty())
        .map_err(|e| context(e, "cannot open", leaf))?;
    // The umask may have narrowed the mode below what the owner needs.
    rustix::fs::fchmod(&created, Mode::RWXU).map_err(|e| context(e, "cannot chmod", leaf))
}

/// A root is shared by every agent; anyone else able to write in it could
/// plant or swap their directories, and an owner unable to could make none.
fn check_root(root: &Path, stat: &Stat, uid: u32) -> io::Result<()> {
    let fix = "point agents.temp_root at a private directory you own";
    if file_type(stat) != FileType::Directory {
        return Err(not_a_directory(root));
    }
    if stat.st_uid != uid {
        return Err(refused(format!(
            "agents.temp_root {} is owned by uid {}, not {uid}; {fix}",
            root.display(),
            stat.st_uid
        )));
    }
    let mode = mode_bits(stat);
    if mode & 0o022 != 0 {
        return Err(refused(format!(
            "agents.temp_root {} is writable by group or others (mode {mode:o}); run `chmod go-w` \
             on it or {fix}",
            root.display()
        )));
    }
    if mode & 0o700 != 0o700 {
        return Err(refused(format!(
            "agents.temp_root {} is not readable, writable and searchable by its owner \
             (mode {mode:o}); run `chmod u+rwx` on it or {fix}",
            root.display()
        )));
    }
    Ok(())
}

/// A leaf is never adopted: anything but our own 0700 directory is refused.
fn check_leaf(leaf: &Path, stat: &Stat, uid: u32) -> io::Result<()> {
    let fix = "remove it so Aether can recreate it";
    if file_type(stat) != FileType::Directory {
        return Err(refused(format!(
            "agent temp dir {} is not a real directory; {fix}",
            leaf.display()
        )));
    }
    if stat.st_uid != uid {
        return Err(refused(format!(
            "agent temp dir {} is owned by uid {}, not {uid}; {fix}",
            leaf.display(),
            stat.st_uid
        )));
    }
    if mode_bits(stat) & 0o777 != 0o700 {
        return Err(refused(format!(
            "agent temp dir {} has mode {:o}, not 700; {fix}",
            leaf.display(),
            mode_bits(stat) & 0o777
        )));
    }
    Ok(())
}

/// Publish the marker if absent; refuse a root another instance claimed.
fn claim_root(dir: &OwnedFd, root: &Path, instance_id: &str) -> io::Result<()> {
    let claimed = match read_marker(dir, root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            publish_marker(dir, root, instance_id)?
        }
        read => read?,
    };
    if claimed.trim() == instance_id {
        return Ok(());
    }
    Err(claimed_elsewhere(root, &claimed))
}

/// Write the claim to a private file of our own and link it in as the
/// marker, so no reader sees it half written. Returns the claim that landed.
fn publish_marker(dir: &OwnedFd, root: &Path, instance_id: &str) -> io::Result<String> {
    let staged = format!("{MARKER}.{}", ulid::Ulid::generate());
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let fd = rustix::fs::openat(dir, staged.as_str(), flags, Mode::RUSR | Mode::WUSR)
        .map_err(|e| context(e, "cannot create", &root.join(&staged)))?;
    let mut file = fs::File::from(fd);
    let published = file
        .write_all(instance_id.as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            rustix::fs::linkat(dir, staged.as_str(), dir, MARKER, AtFlags::empty())
                .map_err(io::Error::from)
        });
    rustix::fs::unlinkat(dir, staged.as_str(), AtFlags::empty())
        .map_err(|e| context(e, "cannot remove", &root.join(&staged)))?;
    match published {
        Ok(()) => Ok(instance_id.to_owned()),
        // Another claim landed first; it decides, whoever made it.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => read_marker(dir, root),
        Err(error) => Err(context(error, "cannot write", &root.join(MARKER))),
    }
}

/// The claim in the marker of the root `dir`. Opening never blocks, so a
/// FIFO planted as the marker is refused rather than waited on.
fn read_marker(dir: &OwnedFd, root: &Path) -> io::Result<String> {
    let marker = root.join(MARKER);
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let fd = rustix::fs::openat(dir, MARKER, flags, Mode::empty())
        .map_err(|e| context(e, "cannot read", &marker))?;
    let file = fs::File::from(fd);
    if !file.metadata()?.is_file() {
        return Err(refused(format!(
            "{} is not a regular file",
            marker.display()
        )));
    }
    let mut claimed = String::new();
    file.take(MAX_MARKER_BYTES)
        .read_to_string(&mut claimed)
        .map_err(|e| context(e, "cannot read", &marker))?;
    Ok(claimed)
}

fn claimed_elsewhere(root: &Path, claimed: &str) -> io::Error {
    refused(format!(
        "agents.temp_root {} is used by another Aether instance ({:?} in {}); give each \
         instance its own agents.temp_root, or remove the marker if that instance is gone",
        root.display(),
        claimed.trim(),
        root.join(MARKER).display()
    ))
}

fn not_a_directory(root: &Path) -> io::Error {
    refused(format!(
        "agents.temp_root {} is not a real directory; point agents.temp_root at a private \
         directory you own",
        root.display()
    ))
}

pub(crate) fn file_type(stat: &Stat) -> FileType {
    FileType::from_raw_mode(stat.st_mode)
}

/// The permission bits, sticky bit included.
pub(crate) fn mode_bits(stat: &Stat) -> u32 {
    // `st_mode` is narrower than u32 on some platforms.
    #[allow(clippy::unnecessary_cast)]
    let mode = stat.st_mode as u32;
    mode & 0o7777
}

pub(crate) fn refused(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

pub(crate) fn context(error: impl Into<io::Error>, what: &str, path: &Path) -> io::Error {
    let error = error.into();
    io::Error::new(error.kind(), format!("{what} {}: {error}", path.display()))
}

#[cfg(test)]
#[path = "agent_temp_tests.rs"]
mod tests;
