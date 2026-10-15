//! What the agents' temp dirs hold. Reported only; nothing is removed yet.

use std::path::{Path, PathBuf};
use std::{fs, io};

use serde::Serialize;

use super::{MAX_ENTRIES, read_directory};

const POLICY: &str = "reported only; agent temp dirs are not cleaned up yet";

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
    let Some(entries) = read_directory(root)? else {
        return Ok(report);
    };
    let mut budget = MAX_ENTRIES;
    for entry in entries {
        if budget == 0 {
            report.truncated = true;
            break;
        }
        budget -= 1;
        let entry = entry?;
        let is_leaf = entry
            .file_name()
            .to_str()
            .is_some_and(aether_core::agent_temp::is_leaf_name);
        // `file_type` does not follow a symlink, so a linked leaf is skipped.
        if !is_leaf || !entry.file_type()?.is_dir() {
            continue;
        }
        report.dirs += 1;
        add_tree(&entry.path(), &mut report, &mut budget);
    }
    Ok(report)
}

/// Add the file bytes under `leaf` to `report`, spending one unit of
/// `budget` per entry, without following symlinks.
fn add_tree(leaf: &Path, report: &mut AgentTempReport, budget: &mut usize) {
    let mut pending = vec![leaf.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Some(entries) = report.readable(fs::read_dir(&dir)) else {
            continue;
        };
        for entry in entries {
            if *budget == 0 {
                report.truncated = true;
                return;
            }
            *budget -= 1;
            let found = entry.and_then(|entry| Ok((entry.path(), entry.metadata()?)));
            let Some((path, meta)) = report.readable(found) else {
                continue;
            };
            if meta.is_dir() {
                pending.push(path);
            } else if meta.is_file() {
                report.bytes += meta.len();
            }
        }
    }
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
