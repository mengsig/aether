//! Per-agent temporary directories.
//!
//! One private root per Aether instance, claimed by a marker holding the
//! store's instance id, holds one 0700 leaf per agent. The leaf is named by
//! the random tail of the agent's id, so it greps against the id. Processes
//! an agent starts get the leaf as `TMPDIR`, `TMP` and `TEMP`.

use std::fs;
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

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

/// Create `root` if missing, refuse it unless it is private to us, and claim
/// it for `instance_id`. A root another instance claimed is refused.
pub fn prepare_root(root: &Path, instance_id: &str) -> io::Result<()> {
    debug_assert!(!instance_id.trim().is_empty(), "an instance id is required");
    if !root.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("agents.temp_root {} is not absolute", root.display()),
        ));
    }
    if let Some(parent) = root.parent() {
        fs::create_dir_all(parent).map_err(|e| context(e, "cannot create", parent))?;
    }
    create_private_dir(root).map_err(|e| context(e, "cannot create agents.temp_root", root))?;
    let meta = fs::symlink_metadata(root).map_err(|e| context(e, "cannot inspect", root))?;
    check_root(root, &meta, rustix::process::geteuid().as_raw())?;
    claim_root(root, instance_id)
}

/// Refuse an existing `root` that is not private to us, creating and claiming
/// nothing. A missing root passes: the daemon creates it.
pub fn inspect_root(root: &Path) -> io::Result<()> {
    match fs::symlink_metadata(root) {
        Ok(meta) => check_root(root, &meta, rustix::process::geteuid().as_raw()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(context(error, "cannot inspect", root)),
    }
}

/// Create `leaf` if missing (recreating the root first if it went too), and
/// refuse it unless it is a real 0700 directory we own.
pub fn ensure_leaf(leaf: &Path, instance_id: &str) -> io::Result<()> {
    let root = leaf.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "an agent temp dir needs a root",
        )
    })?;
    debug_assert!(
        leaf.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_leaf_name),
        "{} is not a leaf",
        leaf.display()
    );
    let created = match create_private_dir(leaf) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            prepare_root(root, instance_id)?;
            create_private_dir(leaf)
        }
        created => created,
    };
    created.map_err(|e| context(e, "cannot create agent temp dir", leaf))?;
    let meta = fs::symlink_metadata(leaf).map_err(|e| context(e, "cannot inspect", leaf))?;
    check_leaf(leaf, &meta, rustix::process::geteuid().as_raw())
}

/// What to tell an operator about `root` before agents use it: a path too
/// long for the sockets tools make under it, or a filesystem held in memory.
pub fn warnings(root: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    let bytes = root.as_os_str().len() + 1 + LEAF_LEN;
    if bytes > PATH_BUDGET {
        warnings.push(format!(
            "agent temp dirs under {} are {bytes} bytes, over the {PATH_BUDGET}-byte budget \
             that leaves Unix sockets room under TMPDIR; set a shorter agents.temp_root",
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

/// Make `path` as a 0700 directory. One that already exists is left as it is
/// for the caller to inspect; the mode is set only on what this call made.
fn create_private_dir(path: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        // The umask may have narrowed the mode below what the owner needs.
        Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o700)),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

/// A root is shared by every agent; anyone else able to write in it could
/// plant or swap their directories.
fn check_root(root: &Path, meta: &fs::Metadata, uid: u32) -> io::Result<()> {
    let fix = "point agents.temp_root at a private directory you own";
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(refused(format!(
            "agents.temp_root {} is not a real directory; {fix}",
            root.display()
        )));
    }
    if meta.uid() != uid {
        return Err(refused(format!(
            "agents.temp_root {} is owned by uid {}, not {uid}; {fix}",
            root.display(),
            meta.uid()
        )));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(refused(format!(
            "agents.temp_root {} is writable by group or others (mode {:o}); run `chmod go-w` \
             on it or {fix}",
            root.display(),
            meta.mode() & 0o7777
        )));
    }
    Ok(())
}

/// A leaf is never adopted: anything but our own 0700 directory is refused.
fn check_leaf(leaf: &Path, meta: &fs::Metadata, uid: u32) -> io::Result<()> {
    let fix = "remove it so Aether can recreate it";
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(refused(format!(
            "agent temp dir {} is not a real directory; {fix}",
            leaf.display()
        )));
    }
    if meta.uid() != uid {
        return Err(refused(format!(
            "agent temp dir {} is owned by uid {}, not {uid}; {fix}",
            leaf.display(),
            meta.uid()
        )));
    }
    if meta.mode() & 0o777 != 0o700 {
        return Err(refused(format!(
            "agent temp dir {} has mode {:o}, not 700; {fix}",
            leaf.display(),
            meta.mode() & 0o777
        )));
    }
    Ok(())
}

/// Write the marker if absent; refuse a root another instance claimed.
fn claim_root(root: &Path, instance_id: &str) -> io::Result<()> {
    let marker = root.join(MARKER);
    match crate::private_file::create_new(&marker) {
        Ok(mut file) => {
            file.write_all(instance_id.as_bytes())?;
            return file.sync_all();
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(context(error, "cannot write", &marker)),
    }
    let claimed = read_marker(&marker)?;
    if claimed.trim() == instance_id {
        return Ok(());
    }
    Err(refused(format!(
        "agents.temp_root {} is used by another Aether instance ({:?} in {}); give each \
         instance its own agents.temp_root, or remove the marker if that instance is gone",
        root.display(),
        claimed.trim(),
        marker.display()
    )))
}

fn read_marker(marker: &Path) -> io::Result<String> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(marker)
        .map_err(|e| context(e, "cannot read", marker))?;
    if !file.metadata()?.is_file() {
        return Err(refused(format!(
            "{} is not a regular file",
            marker.display()
        )));
    }
    let mut claimed = String::new();
    file.take(MAX_MARKER_BYTES)
        .read_to_string(&mut claimed)
        .map_err(|e| context(e, "cannot read", marker))?;
    Ok(claimed)
}

fn refused(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

fn context(error: io::Error, what: &str, path: &Path) -> io::Error {
    io::Error::new(error.kind(), format!("{what} {}: {error}", path.display()))
}

#[cfg(test)]
#[path = "agent_temp_tests.rs"]
mod tests;
