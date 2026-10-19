//! One daemon per store, and one per socket.
//!
//! Two daemons over the same database would both recover and schedule the
//! same agents. Two daemons on the same socket - which the store lock cannot
//! catch, since they may serve different stores - would race between probing
//! that socket, unlinking it and binding it, and the loser of that race would
//! take the winner's socket away.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::Context;

/// Another live process holds the lock.
#[derive(Debug)]
pub struct LockBusy(String);

impl std::fmt::Display for LockBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LockBusy {}

/// True when a live daemon holds the lock for `socket`. A daemon that is
/// shutting down keeps it until its process ends.
pub fn socket_lock_is_held(socket: &Path) -> anyhow::Result<bool> {
    match InstanceLock::for_socket(socket) {
        Ok(_released_at_once) => Ok(false),
        Err(error) if error.is::<LockBusy>() => Ok(true),
        Err(error) => Err(error),
    }
}

/// An exclusive lock on the file beside the store or the socket, held while
/// this value lives. The OS releases it when the process ends, however it
/// ends, so a daemon killed outright leaves nothing locked.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
    target: PathBuf,
}

impl InstanceLock {
    /// Take the lock for `store_path`, failing at once if a live daemon holds it.
    pub fn for_store(store_path: &Path) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !store_path.as_os_str().is_empty(),
            "the store path is empty"
        );
        Self::take(
            store_path,
            LastComponent::Resolved,
            format!(
                "another aether daemon is already running for {}",
                store_path.display()
            ),
        )
    }

    /// Take the lock for `socket`, failing at once if another daemon holds
    /// it. Taken before the socket is probed and held for the daemon's life,
    /// so probe, unlink and bind are one critical section: without it two
    /// daemons for different stores can both find the socket dead and each
    /// unlink what the other just bound.
    pub fn for_socket(socket: &Path) -> anyhow::Result<Self> {
        anyhow::ensure!(!socket.as_os_str().is_empty(), "the socket path is empty");
        Self::take(
            socket,
            LastComponent::AsWritten,
            format!(
                "another aether daemon is already using the socket {}",
                socket.display()
            ),
        )
    }

    /// Lock `<target>.lock`, reporting `busy` when another process holds it.
    ///
    /// The lock file itself is never removed: unlinking it would let the next
    /// daemon create and lock a fresh inode while this one still holds the
    /// old one, which is exactly the mutual exclusion it exists to provide.
    fn take(target: &Path, last: LastComponent, busy: String) -> anyhow::Result<Self> {
        let dir = parent_dir(target);
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        // Set the mode at creation; an existing parent may be shared.
        builder
            .create(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
        check_shared_parent(dir)?;
        let target = resolved_target(target, dir, last)?;
        check_shared_parent(parent_dir(&target))?;
        let mut path = target.as_os_str().to_owned();
        path.push(".lock");
        let path = PathBuf::from(path);
        let file = open_private_file(&path, PrivateOpen::Create)?
            .with_context(|| format!("opening the daemon lock {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Self {
                _file: file,
                target,
            }),
            Err(TryLockError::WouldBlock) => Err(anyhow::Error::new(LockBusy(busy))),
            Err(TryLockError::Error(e)) => {
                Err(anyhow::Error::new(e).context(format!("locking {}", path.display())))
            }
        }
    }

    /// Harden the locked target before SQLite can create readable sidecars.
    pub(super) fn prepare_store(&self) -> anyhow::Result<PathBuf> {
        // Keep the target identified by this lock even if its configured alias is retargeted.
        drop(open_private_file(&self.target, PrivateOpen::Create)?);
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = self.target.as_os_str().to_owned();
            sidecar.push(suffix);
            drop(open_private_file(
                Path::new(&sidecar),
                PrivateOpen::Existing,
            )?);
        }
        Ok(self.target.clone())
    }

    pub(super) fn target(&self) -> &Path {
        &self.target
    }
}

/// Replace a daemon PID or startup diagnostic file after validating its descriptor.
pub fn write_private_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    write_private_file_tracked(path, bytes, &mut None)
}

/// Reject unsafe existing PID entries before startup invokes external processes.
/// Publication must still validate its own descriptor after this preflight.
pub(super) fn check_private_file_replacement(path: &Path) -> anyhow::Result<()> {
    let dir = parent_dir(path);
    check_shared_parent(dir)?;
    let path = in_resolved_dir(path, dir)?;
    check_shared_parent(parent_dir(&path))?;
    drop(open_private_file(&path, PrivateOpen::CheckReplacement)?);
    Ok(())
}

/// Retain the published descriptor for identity checks, including after a partial write.
pub(super) fn write_private_file_tracked(
    path: &Path,
    bytes: &[u8],
    published: &mut Option<File>,
) -> anyhow::Result<()> {
    use std::io::Write;
    let dir = parent_dir(path);
    check_shared_parent(dir)?;
    let path = in_resolved_dir(path, dir)?;
    check_shared_parent(parent_dir(&path))?;
    let file = open_private_file(&path, PrivateOpen::Replace)?
        .with_context(|| format!("opening {} for replacement", path.display()))?;
    let file = published.insert(file);
    file.write_all(bytes)
        .with_context(|| format!("writing {}", path.display()))
}

/// Other users must not be able to replace daemon-owned directory entries.
pub(super) fn check_shared_parent(dir: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let canonical = std::fs::canonicalize(dir)
            .with_context(|| format!("resolving the daemon file parent {}", dir.display()))?;
        for ancestor in canonical.ancestors() {
            let metadata = std::fs::metadata(ancestor).with_context(|| {
                format!("checking the daemon file parent {}", ancestor.display())
            })?;
            anyhow::ensure!(
                metadata.uid() == 0 || metadata.uid() == rustix::process::geteuid().as_raw(),
                "daemon file parent {} must be owned by the current user or root",
                ancestor.display()
            );
            anyhow::ensure!(
                metadata.mode() & 0o022 == 0 || metadata.mode() & 0o1000 != 0,
                "daemon file parent {} is writable by other users without the sticky bit",
                ancestor.display()
            );
        }
    }
    Ok(())
}

/// The directory holding `path`; `.` for a bare file name.
fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// Whether the locked path's own last component is stable enough to resolve.
#[derive(Clone, Copy)]
enum LastComponent {
    /// A store file: every path to one database must name one lock, so a
    /// link resolves to the lock of the database it points at.
    Resolved,
    /// A socket: the daemon unlinks that very name and binds a socket over
    /// it, so resolving it would give the next daemon a different lock.
    AsWritten,
}

fn resolved_target(target: &Path, dir: &Path, last: LastComponent) -> anyhow::Result<PathBuf> {
    match last {
        LastComponent::AsWritten => in_resolved_dir(target, dir),
        LastComponent::Resolved => {
            match std::fs::canonicalize(target) {
                Ok(path) => Ok(path),
                // Not created yet: the name is final, only its directory is a link.
                Err(e) if e.kind() == ErrorKind::NotFound => in_resolved_dir(target, dir),
                Err(e) => Err(anyhow::Error::new(e)
                    .context(format!("resolving the path {}", target.display()))),
            }
        }
    }
}

enum PrivateOpen {
    Existing,
    Create,
    Replace,
    CheckReplacement,
}

fn open_private_file(path: &Path, mode: PrivateOpen) -> anyhow::Result<Option<File>> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(matches!(mode, PrivateOpen::Create | PrivateOpen::Replace))
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Never follow a replacement symlink or block while opening a FIFO.
        options.mode(0o600).custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error)
            if matches!(mode, PrivateOpen::Existing | PrivateOpen::CheckReplacement)
                && error.kind() == ErrorKind::NotFound =>
        {
            return Ok(None);
        }
        Err(error) => {
            return Err(anyhow::Error::new(error).context(format!(
                "opening the private daemon file {}",
                path.display()
            )));
        }
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("checking the daemon file {}", path.display()))?;
    anyhow::ensure!(
        metadata.is_file(),
        "daemon file {} must be a regular file",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        anyhow::ensure!(
            metadata.uid() == rustix::process::geteuid().as_raw(),
            "daemon file {} must be owned by the current user",
            path.display()
        );
        if matches!(mode, PrivateOpen::Replace | PrivateOpen::CheckReplacement) {
            anyhow::ensure!(
                metadata.nlink() == 1,
                "daemon file {} has multiple hard links",
                path.display()
            );
        }
        if !matches!(mode, PrivateOpen::CheckReplacement) {
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("hardening the daemon file {}", path.display()))?;
        }
    }
    // Opening must not truncate a symlink target, foreign file, or nonregular file.
    if matches!(mode, PrivateOpen::Replace) {
        file.set_len(0)
            .with_context(|| format!("replacing {}", path.display()))?;
    }
    Ok(Some(file))
}

/// `target`'s own name inside `dir`, with `dir`'s links resolved.
fn in_resolved_dir(target: &Path, dir: &Path) -> anyhow::Result<PathBuf> {
    let name = target
        .file_name()
        .with_context(|| format!("the path {} names no file", target.display()))?;
    Ok(std::fs::canonicalize(dir)
        .with_context(|| format!("resolving {}", dir.display()))?
        .join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_socket_lock_is_seen_and_a_released_one_is_not() {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("aether.sock");
        assert!(!socket_lock_is_held(&socket).unwrap());
        let held = InstanceLock::for_socket(&socket).unwrap();
        assert!(socket_lock_is_held(&socket).unwrap());
        drop(held);
        assert!(!socket_lock_is_held(&socket).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn replacement_preflight_validates_without_creating_or_changing_entries() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let pid = root.path().join("aether.pid");
        check_private_file_replacement(&pid).unwrap();
        assert!(!pid.exists());
        std::fs::write(&pid, b"keep bytes").unwrap();
        std::fs::set_permissions(&pid, std::fs::Permissions::from_mode(0o644)).unwrap();
        check_private_file_replacement(&pid).unwrap();
        assert_eq!(std::fs::read(&pid).unwrap(), b"keep bytes");
        assert_eq!(
            std::fs::metadata(&pid).unwrap().permissions().mode() & 0o777,
            0o644
        );
        let alias = root.path().join("alias");
        std::fs::hard_link(&pid, &alias).unwrap();
        assert!(check_private_file_replacement(&alias).is_err());
        std::fs::remove_file(&alias).unwrap();
        symlink(&pid, &alias).unwrap();
        assert!(check_private_file_replacement(&alias).is_err());
        std::fs::remove_file(&alias).unwrap();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &alias,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o644),
            0,
        )
        .unwrap();
        assert!(check_private_file_replacement(&alias).is_err());
        assert_eq!(std::fs::read(&pid).unwrap(), b"keep bytes");
        assert_eq!(
            std::fs::metadata(&pid).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unsafe_ancestor_is_rejected_for_socket_store_and_private_outputs() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        let child = root.path().join("safe");
        std::fs::create_dir(&child).unwrap();
        std::fs::set_permissions(&child, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = child.join("aether.pid");
        std::fs::write(&output, b"keep").unwrap();
        std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(InstanceLock::for_socket(&child.join("aether.sock")).is_err());
        assert!(InstanceLock::for_store(&child.join("aether.db")).is_err());
        assert!(write_private_file(&output, b"replace").is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"keep");
        assert_eq!(
            std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(
            std::fs::metadata(&child).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(std::fs::read_dir(&child).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn private_outputs_replace_only_owned_regular_files_with_one_link() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o1777)).unwrap();
        for name in ["aether.sock.pid", "aether.sock.startup.log"] {
            let path = root.path().join(name);
            write_private_file(&path, b"new output").unwrap();
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            write_private_file(&path, b"short").unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"short");
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(
            std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o7777,
            0o1777
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_outputs_reject_symlink_hardlink_fifo_and_unsafe_parent_before_changes() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o1777)).unwrap();
        let victim = root.path().join("keep");
        std::fs::write(&victim, b"keep bytes").unwrap();
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).unwrap();
        for name in ["aether.sock.pid", "aether.sock.startup.log"] {
            let path = root.path().join(name);
            std::os::unix::fs::symlink(&victim, &path).unwrap();
            assert!(write_private_file(&path, b"replace").is_err());
            std::fs::remove_file(&path).unwrap();
            std::fs::hard_link(&victim, &path).unwrap();
            assert!(write_private_file(&path, b"replace").is_err());
            assert_eq!(std::fs::read(&victim).unwrap(), b"keep bytes");
            assert_eq!(
                std::fs::metadata(&victim).unwrap().permissions().mode() & 0o777,
                0o644
            );
            std::fs::remove_file(&path).unwrap();
            rustix::fs::mknodat(
                rustix::fs::CWD,
                &path,
                rustix::fs::FileType::Fifo,
                rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
                0,
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(write_private_file(&path, b"replace").is_err());
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o644
            );
            std::fs::remove_file(path).unwrap();
        }
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(write_private_file(&victim, b"replace").is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep bytes");
        assert_eq!(
            std::fs::metadata(victim).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn a_second_daemon_cannot_take_a_held_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("nested").join("aether.db");
        let first = InstanceLock::for_store(&store).expect("the first daemon");
        let nested = std::fs::canonicalize(dir.path().join("nested")).expect("created");
        assert!(nested.join("aether.db.lock").exists());

        let refused = InstanceLock::for_store(&store).expect_err("the store is taken");
        assert_eq!(
            refused.to_string(),
            format!(
                "another aether daemon is already running for {}",
                store.display()
            )
        );
        let other = dir.path().join("other.db");
        InstanceLock::for_store(&other).expect("a different store is free");

        drop(first);
        InstanceLock::for_store(&store).expect("free again once the first has gone");
    }

    /// Two stores sharing one socket: the store lock lets both through, so
    /// the socket lock is the only thing between them and the bind race.
    #[test]
    fn a_second_daemon_cannot_take_a_held_socket_whatever_its_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("run").join("aetherd.sock");
        let _first_store = InstanceLock::for_store(&dir.path().join("one.db")).expect("one");
        let first = InstanceLock::for_socket(&socket).expect("the first daemon");
        InstanceLock::for_store(&dir.path().join("two.db")).expect("another store is free");

        let refused = InstanceLock::for_socket(&socket).expect_err("the socket is taken");
        assert_eq!(
            refused.to_string(),
            format!(
                "another aether daemon is already using the socket {}",
                socket.display()
            )
        );
        // The lock is beside the socket, and is not the socket.
        let lock = std::fs::canonicalize(dir.path().join("run"))
            .expect("created")
            .join("aetherd.sock.lock");
        assert!(lock.exists() && !socket.exists(), "{}", lock.display());

        drop(first);
        InstanceLock::for_socket(&socket).expect("free again once the first has gone");
    }

    /// A bare relative socket name resolves to `.`, the process's own
    /// working directory - hardening must skip it, not chmod the cwd.
    #[test]
    fn a_bare_socket_name_has_no_directory_of_its_own_to_harden() {
        assert_eq!(parent_dir(Path::new("aetherd.sock")), Path::new("."));
    }

    #[cfg(unix)]
    #[test]
    fn a_new_shared_store_and_socket_parent_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("state");
        let _store = InstanceLock::for_store(&parent.join("aether.db")).unwrap();
        let _socket = InstanceLock::for_socket(&parent.join("aetherd.sock")).unwrap();
        assert_eq!(
            std::fs::metadata(parent).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[cfg(unix)]
    #[test]
    fn fresh_sqlite_files_are_private_in_an_existing_public_parent() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = directory.path().join("aether.db");
        let lock = InstanceLock::for_store(&path).unwrap();
        let prepared = lock.prepare_store().unwrap();
        let _store = aether_store::SqliteStore::open(&prepared).unwrap();
        for name in ["aether.db", "aether.db-wal", "aether.db-shm"] {
            assert_eq!(
                std::fs::metadata(directory.path().join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600,
                "{name}"
            );
        }
        assert_eq!(
            std::fs::metadata(directory.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_sqlite_files_and_journal_are_hardened_without_truncation() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("aether.db");
        let _store = aether_store::SqliteStore::open(&path).unwrap();
        std::fs::write(
            directory.path().join("aether.db-journal"),
            b"journal evidence",
        )
        .unwrap();
        let names = [
            "aether.db",
            "aether.db-wal",
            "aether.db-shm",
            "aether.db-journal",
        ];
        let before: Vec<_> = names
            .iter()
            .map(|name| {
                let path = directory.path().join(name);
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
                std::fs::read(path).unwrap()
            })
            .collect();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let lock = InstanceLock::for_store(&path).unwrap();
        lock.prepare_store().unwrap();
        for (name, bytes) in names.iter().zip(before) {
            let path = directory.path().join(name);
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600,
                "{name}"
            );
            assert_eq!(std::fs::read(path).unwrap(), bytes, "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn preparation_uses_the_locked_database_after_an_alias_is_retargeted() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("real.db");
        let other = directory.path().join("other.db");
        let alias = directory.path().join("alias.db");
        let original = aether_store::SqliteStore::open(&real).unwrap();
        drop(original);
        std::fs::write(&other, b"unrelated evidence").unwrap();
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o644)).unwrap();
        symlink(&real, &alias).unwrap();
        let lock = InstanceLock::for_store(&alias).unwrap();
        std::fs::remove_file(&alias).unwrap();
        symlink(&other, &alias).unwrap();
        let prepared = lock.prepare_store().unwrap();
        assert_eq!(prepared, real);
        let _store = aether_store::SqliteStore::open(prepared).unwrap();
        for name in ["real.db", "real.db-wal", "real.db-shm"] {
            assert_eq!(
                std::fs::metadata(directory.path().join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600,
                "{name}"
            );
        }
        assert!(!directory.path().join("alias.db-wal").exists());
        assert_eq!(std::fs::read(&other).unwrap(), b"unrelated evidence");
        assert_eq!(
            std::fs::metadata(other).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[cfg(unix)]
    #[test]
    fn preparing_a_store_rejects_sidecar_symlinks_without_chmodding_the_target() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = tempfile::tempdir().unwrap();
        let unrelated = directory.path().join("evidence");
        std::fs::write(&unrelated, b"keep").unwrap();
        std::fs::set_permissions(&unrelated, std::fs::Permissions::from_mode(0o644)).unwrap();
        symlink(&unrelated, directory.path().join("aether.db-wal")).unwrap();
        let lock = InstanceLock::for_store(&directory.path().join("aether.db")).unwrap();
        let error = lock.prepare_store().unwrap_err();
        assert!(error.to_string().contains("aether.db-wal"), "{error:#}");
        assert_eq!(
            std::fs::metadata(&unrelated).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(std::fs::read(unrelated).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn a_socket_lock_rejects_symlinks_and_fifos_without_chmodding_other_files() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("aetherd.sock");
        let lock = directory.path().join("aetherd.sock.lock");
        let unrelated = directory.path().join("evidence");
        std::fs::write(&unrelated, b"keep").unwrap();
        std::fs::set_permissions(&unrelated, std::fs::Permissions::from_mode(0o644)).unwrap();
        symlink(&unrelated, &lock).unwrap();
        InstanceLock::for_socket(&socket).unwrap_err();
        assert_eq!(
            std::fs::metadata(&unrelated).unwrap().permissions().mode() & 0o777,
            0o644
        );
        std::fs::remove_file(&lock).unwrap();
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &lock,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        let error = InstanceLock::for_socket(&socket).unwrap_err();
        assert!(error.to_string().contains("regular file"), "{error:#}");
        assert_eq!(std::fs::read(unrelated).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn taking_a_socket_lock_preserves_existing_parent_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o1777))
            .expect("shared directory");
        let _lock =
            InstanceLock::for_socket(&dir.path().join("aetherd.sock")).expect("socket lock");
        assert_eq!(
            std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o7777,
            0o1777,
            "an existing shared socket parent must not be chmodded"
        );
        assert_eq!(
            std::fs::metadata(dir.path().join("aetherd.sock.lock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "another user must not be able to read and flock this lock"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unsafe_shared_socket_parent_is_rejected_without_chmod() {
        use std::os::unix::fs::PermissionsExt;

        for mode in [0o777, 0o775] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(mode)).unwrap();
            InstanceLock::for_socket(&dir.path().join("aetherd.sock"))
                .expect_err("other users could replace the daemon's entries");
            assert_eq!(
                std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o7777,
                mode
            );
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        }
    }

    /// A not-yet-existing socket dir must never sit world-traversable, even
    /// briefly: bind is later, and store open/recovery run before it.
    #[cfg(unix)]
    #[test]
    fn taking_the_socket_lock_hardens_a_freshly_created_socket_dir() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("run").join("aetherd.sock");
        let _lock = InstanceLock::for_socket(&socket).expect("the first daemon");

        let mode = std::fs::metadata(dir.path().join("run"))
            .expect("created")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "{mode:o}");
    }

    /// The daemon unlinks the socket's own name and binds over it, so what
    /// that name pointed at before must not decide which lock it takes: a
    /// configured path that starts as a link would otherwise lock one file
    /// and leave the next daemon locking another.
    #[cfg(unix)]
    #[test]
    fn a_socket_keeps_one_lock_across_the_unlink_and_the_bind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let configured = dir.path().join("aetherd.sock");
        let elsewhere = dir.path().join("elsewhere.sock");
        std::fs::write(&elsewhere, b"").expect("something for the link to point at");
        std::os::unix::fs::symlink(&elsewhere, &configured).expect("the link");

        let first = InstanceLock::for_socket(&configured).expect("the first daemon");
        // What the daemon does next: the link goes, its socket takes the name.
        std::fs::remove_file(&configured).expect("the link goes");
        drop(std::os::unix::net::UnixListener::bind(&configured).expect("bound"));

        let refused = InstanceLock::for_socket(&configured).expect_err("the socket is taken");
        assert!(
            refused.to_string().contains("already using the socket"),
            "{refused:#}"
        );
        drop(first);
        InstanceLock::for_socket(&configured).expect("free again once the first has gone");
    }

    #[cfg(unix)]
    #[test]
    fn every_path_to_one_store_names_the_same_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real");
        std::fs::create_dir(&real).expect("real dir");
        std::fs::write(real.join("aether.db"), b"").expect("the store file");
        let by_file = dir.path().join("link.db");
        std::os::unix::fs::symlink(real.join("aether.db"), &by_file).expect("file link");
        let by_dir = dir.path().join("linked");
        std::os::unix::fs::symlink(&real, &by_dir).expect("dir link");

        let _first = InstanceLock::for_store(&real.join("aether.db")).expect("the first daemon");
        for other in [by_file, by_dir.join("aether.db")] {
            let refused = InstanceLock::for_store(&other).expect_err("the same database");
            assert!(
                refused.to_string().contains("already running"),
                "{refused:#}"
            );
        }
        // A store not created yet still resolves through a linked directory.
        let _fresh = InstanceLock::for_store(&real.join("fresh.db")).expect("a new store");
        let refused =
            InstanceLock::for_store(&by_dir.join("fresh.db")).expect_err("the same store");
        assert!(
            refused.to_string().contains("already running"),
            "{refused:#}"
        );
    }
}
