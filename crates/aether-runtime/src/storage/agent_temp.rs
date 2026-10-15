//! What the agents' temp dirs hold. Reported only; nothing is removed yet.

use std::ffi::CString;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};
use rustix::io::Errno;

use serde::Serialize;

use super::MAX_ENTRIES;

const POLICY: &str = "reported only; agent temp dirs are not cleaned up yet";

/// A directory opened this way is the directory itself, never a link to one.
const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

#[derive(Debug, Serialize)]
pub(super) struct AgentTempReport {
    root: PathBuf,
    policy: &'static str,
    bytes: u64,
    /// Directories named as an agent's leaf; anything else under the root is
    /// not an agent's and is not counted.
    dirs: usize,
    /// Entries inside a leaf that could not be read, so `bytes` misses them.
    unreadable: usize,
    truncated: bool,
}

/// Bytes and leaf count under `root`, stopping after a fixed number of
/// entries. Agents' processes delete their files mid-walk; those are skipped.
/// The root is opened once and each leaf in it, so a root swapped mid-walk
/// is never left.
pub(super) fn report(root: &Path) -> io::Result<AgentTempReport> {
    debug_assert!(root.is_absolute(), "{} is not absolute", root.display());
    let mut report = AgentTempReport {
        root: root.to_path_buf(),
        policy: POLICY,
        bytes: 0,
        dirs: 0,
        unreadable: 0,
        truncated: false,
    };
    let dir = match rustix::fs::open(root, DIR_FLAGS, Mode::empty()) {
        Ok(dir) => dir,
        Err(Errno::NOENT) => return Ok(report),
        Err(Errno::LOOP | Errno::NOTDIR | Errno::MLINK) => {
            return Err(io::Error::other(format!(
                "{} is not a real directory",
                root.display()
            )));
        }
        Err(errno) => return Err(errno.into()),
    };
    let mut budget = MAX_ENTRIES;
    for entry in Dir::read_from(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == c"." || name == c".." {
            continue;
        }
        if budget == 0 {
            report.truncated = true;
            break;
        }
        budget -= 1;
        if !name
            .to_str()
            .is_ok_and(aether_core::agent_temp::is_leaf_name)
        {
            continue;
        }
        let opened = rustix::fs::openat(&dir, name, DIR_FLAGS, Mode::empty());
        // A link or a file named as a leaf is not an agent's.
        if matches!(opened, Err(Errno::LOOP | Errno::NOTDIR | Errno::MLINK)) {
            continue;
        }
        let Some(leaf) = report.readable(opened.map_err(io::Error::from)) else {
            continue;
        };
        report.dirs += 1;
        add_tree(leaf, &mut report, &mut budget);
    }
    debug_assert!(budget <= MAX_ENTRIES, "{budget}");
    Ok(report)
}

/// A directory still to walk: its name in the already open `parent`.
type Pending = (Rc<OwnedFd>, CString);

/// Add the file bytes under the open `leaf` to `report`, spending one unit
/// of `budget` per entry. Each directory is opened relative to its parent
/// without following links, so a link swapped in mid-walk is never entered.
fn add_tree(leaf: OwnedFd, report: &mut AgentTempReport, budget: &mut usize) {
    let mut pending = Vec::new();
    let mut dir = Rc::new(leaf);
    loop {
        if !add_entries(&dir, &mut pending, report, budget) {
            return;
        }
        dir = loop {
            let Some((parent, name)) = pending.pop() else {
                return;
            };
            let opened = rustix::fs::openat(&*parent, &name, DIR_FLAGS, Mode::empty());
            if let Some(next) = report.readable(opened.map_err(io::Error::from)) {
                break Rc::new(next);
            }
        };
    }
}

/// Count the files directly in `dir` and queue its subdirectories. False
/// once the budget is spent.
fn add_entries(
    dir: &Rc<OwnedFd>,
    pending: &mut Vec<Pending>,
    report: &mut AgentTempReport,
    budget: &mut usize,
) -> bool {
    let Some(entries) = report.readable(Dir::read_from(&**dir).map_err(io::Error::from)) else {
        return true;
    };
    for entry in entries {
        let Some(entry) = report.readable(entry.map_err(io::Error::from)) else {
            continue;
        };
        let name = entry.file_name();
        if name == c"." || name == c".." {
            continue;
        }
        if *budget == 0 {
            report.truncated = true;
            return false;
        }
        *budget -= 1;
        // `d_type` describes the entry itself; only an unknown one needs a stat.
        let kind = match entry.file_type() {
            FileType::Unknown | FileType::RegularFile => {
                let stat = rustix::fs::statat(&**dir, name, AtFlags::SYMLINK_NOFOLLOW);
                let Some(stat) = report.readable(stat.map_err(io::Error::from)) else {
                    continue;
                };
                let kind = FileType::from_raw_mode(stat.st_mode);
                if kind == FileType::RegularFile {
                    report.bytes += u64::try_from(stat.st_size).unwrap_or(0);
                }
                kind
            }
            kind => kind,
        };
        if kind == FileType::Directory {
            pending.push((Rc::clone(dir), name.to_owned()));
        }
    }
    true
}

impl AgentTempReport {
    /// What a read gave, if anything. A path removed mid-walk is expected;
    /// any other failure is counted so the total is known to be short.
    fn readable<T>(&mut self, read: io::Result<T>) -> Option<T> {
        match read {
            Ok(value) => Some(value),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => {
                self.unreadable += 1;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn bytes_are_counted_under_agent_leaves_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("t");
        assert_eq!(
            report(&root).unwrap().dirs,
            0,
            "a missing root reports nothing"
        );
        let leaf = root.join("0123456789AB");
        fs::create_dir_all(leaf.join("nested")).unwrap();
        fs::write(leaf.join("a"), [0u8; 100]).unwrap();
        fs::write(leaf.join("nested/b"), [0u8; 23]).unwrap();
        fs::create_dir(root.join("0123456789AC")).unwrap();
        fs::write(root.join(aether_core::agent_temp::MARKER), b"instance").unwrap();
        fs::create_dir(root.join("not-a-leaf")).unwrap();
        fs::write(root.join("not-a-leaf/c"), [0u8; 1000]).unwrap();
        std::os::unix::fs::symlink(root.join("not-a-leaf"), root.join("0123456789AD")).unwrap();
        std::os::unix::fs::symlink(root.join("not-a-leaf/c"), leaf.join("link")).unwrap();
        std::os::unix::fs::symlink(root.join("not-a-leaf"), leaf.join("dir-link")).unwrap();

        let counted = report(&root).unwrap();
        assert_eq!(counted.bytes, 123);
        assert_eq!(counted.dirs, 2);
        assert_eq!(counted.unreadable, 0);
        assert!(!counted.truncated);
        let json = serde_json::to_value(&counted).unwrap();
        assert_eq!(json["policy"], POLICY);
        assert_eq!(json["root"], root.to_str().unwrap());
    }

    #[test]
    fn an_unreadable_directory_is_counted_not_fatal() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("0123456789AB/locked");
        fs::create_dir_all(&locked).unwrap();
        fs::write(dir.path().join("0123456789AB/a"), [0u8; 5]).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        // A superuser reads it regardless, and then nothing is unreadable.
        let expected = usize::from(fs::read_dir(&locked).is_err());
        let counted = report(dir.path());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        let counted = counted.unwrap();
        assert_eq!(counted.bytes, 5);
        assert_eq!(counted.unreadable, expected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_root_swapped_for_a_link_mid_walk_never_counts_what_it_points_at() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let (root, decoy, link) = (
            dir.path().join("t"),
            dir.path().join("decoy"),
            dir.path().join("link"),
        );
        for (tree, bytes) in [(&root, 1), (&decoy, 1000)] {
            fs::create_dir_all(tree.join("0123456789AB")).unwrap();
            fs::write(tree.join("0123456789AB/f"), vec![0u8; bytes]).unwrap();
        }
        std::os::unix::fs::symlink(&decoy, &link).unwrap();
        let stop = AtomicBool::new(false);
        let miscounted = std::thread::scope(|scope| {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let flags = rustix::fs::RenameFlags::EXCHANGE;
                    rustix::fs::renameat_with(
                        rustix::fs::CWD,
                        &root,
                        rustix::fs::CWD,
                        &link,
                        flags,
                    )
                    .unwrap();
                }
            });
            // A walk that starts on the link fails; one that starts on the root counts it.
            let miscounted = (0..5000)
                .filter_map(|_| report(&root).ok())
                .find(|counted| counted.bytes != 1);
            stop.store(true, Ordering::Relaxed);
            miscounted
        });
        assert!(miscounted.is_none(), "{miscounted:?}");
    }

    #[test]
    fn a_large_tree_is_reported_partially() {
        let dir = tempfile::tempdir().unwrap();
        let leaf = dir.path().join("0123456789AB");
        fs::create_dir(&leaf).unwrap();
        for i in 0..=MAX_ENTRIES {
            fs::write(leaf.join(i.to_string()), b"x").unwrap();
        }
        let counted = report(dir.path()).unwrap();
        assert!(counted.truncated);
        assert_eq!(counted.bytes, (MAX_ENTRIES - 1) as u64);
    }
}
