//! What the build cache holds. Reported only; nothing is evicted yet.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use serde::Serialize;

const POLICY: &str = "reported only; build cache partitions are not evicted yet";

/// A cache is build output, far more entries than any other category, so its
/// walk gets its own, larger bound.
const MAX_CACHE_ENTRIES: usize = 4_000_000;

/// Directories open at once while walking; deeper ones are left uncounted.
const MAX_DEPTH: usize = 128;

#[derive(Debug, Serialize)]
pub(super) struct BuildCacheReport {
    root: PathBuf,
    policy: &'static str,
    /// Allocated disk blocks, as `du` counts them.
    bytes: u64,
    repositories: usize,
    /// One per repository and run.
    partitions: usize,
    /// Entries that could not be read, so `bytes` misses them.
    unreadable: usize,
    truncated: bool,
    #[serde(skip)]
    budget: usize,
}

/// Bytes, repositories and partitions under `root`. Below the configured
/// root, links are never followed, and files a running build deletes
/// mid-walk are skipped. Every entry read, at any level, spends the budget.
pub(super) fn report(root: &Path) -> io::Result<BuildCacheReport> {
    debug_assert!(root.is_absolute(), "{} is not absolute", root.display());
    let mut report = BuildCacheReport {
        root: root.to_path_buf(),
        policy: POLICY,
        bytes: 0,
        repositories: 0,
        partitions: 0,
        unreadable: 0,
        truncated: false,
        budget: MAX_CACHE_ENTRIES,
    };
    match fs::metadata(root) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err(io::Error::other(format!(
                "{} is not a directory",
                root.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(report),
        Err(error) => return Err(error),
    }
    for repository in report.real_dirs(root) {
        report.repositories += 1;
        for partition in report.real_dirs(&repository) {
            report.partitions += 1;
            report.add_tree(&partition);
        }
    }
    debug_assert!(report.budget <= MAX_CACHE_ENTRIES, "{}", report.budget);
    Ok(report)
}

impl BuildCacheReport {
    /// The directories directly in `dir`, links and files left out.
    fn real_dirs(&mut self, dir: &Path) -> Vec<PathBuf> {
        let Some(entries) = self.readable(fs::read_dir(dir)) else {
            return Vec::new();
        };
        let mut dirs = Vec::new();
        for entry in entries {
            if !self.spend() {
                break;
            }
            let Some(entry) = self.readable(entry) else {
                continue;
            };
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                dirs.push(entry.path());
            }
        }
        dirs
    }

    /// Add the blocks of everything under `top`, itself included. Open
    /// directories are held one per level, so memory follows depth, not width.
    fn add_tree(&mut self, top: &Path) {
        let Some(entries) = self.add_entry(top) else {
            return;
        };
        let mut open = vec![entries];
        while let Some(entries) = open.last_mut() {
            let Some(entry) = entries.next() else {
                open.pop();
                continue;
            };
            if !self.spend() {
                return;
            }
            let Some(entry) = self.readable(entry) else {
                continue;
            };
            let Some(child) = self.add_entry(&entry.path()) else {
                continue;
            };
            if open.len() == MAX_DEPTH {
                self.truncated = true;
                continue;
            }
            open.push(child);
        }
    }

    /// Count `path`'s own blocks; its entries if it is a real directory.
    fn add_entry(&mut self, path: &Path) -> Option<fs::ReadDir> {
        let meta = self.readable(fs::symlink_metadata(path))?;
        self.bytes = self.bytes.saturating_add(meta.blocks().saturating_mul(512));
        if !meta.is_dir() {
            return None;
        }
        self.readable(fs::read_dir(path))
    }

    /// Take one entry from the budget, or mark the report truncated.
    fn spend(&mut self) -> bool {
        if self.budget == 0 {
            self.truncated = true;
            return false;
        }
        self.budget -= 1;
        true
    }

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
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn partitions_are_counted_and_links_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cache");
        let missing = report(&root).unwrap();
        assert_eq!((missing.bytes, missing.partitions), (0, 0));

        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("big"), vec![1u8; 1 << 20]).unwrap();
        let run = root.join("repo-0123/run_one/cargo-target/debug");
        fs::create_dir_all(&run).unwrap();
        fs::write(run.join("artifact"), vec![1u8; 64 << 10]).unwrap();
        fs::create_dir_all(root.join("repo-0123/run_two")).unwrap();
        fs::create_dir_all(root.join("repo-4567")).unwrap();
        std::os::unix::fs::symlink(&outside, run.join("link")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("repo-0123/run_link")).unwrap();
        fs::write(root.join("stray"), b"not a repository").unwrap();

        let seen = report(&root).unwrap();
        assert_eq!(seen.repositories, 2);
        assert_eq!(seen.partitions, 2, "a link is not a partition");
        assert!(seen.bytes >= 64 << 10, "{seen:?}");
        assert!(
            seen.bytes < 1 << 20,
            "the linked file is not counted: {seen:?}"
        );
        assert_eq!((seen.unreadable, seen.truncated), (0, false));
    }

    #[test]
    fn the_budget_bounds_every_level_and_marks_the_report() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cache");
        let partition = root.join("repo-0123/run_one");
        fs::create_dir_all(&partition).unwrap();
        for i in 0..10 {
            fs::write(partition.join(i.to_string()), b"x").unwrap();
        }
        let mut seen = report(&root).unwrap();
        assert_eq!(
            (seen.budget, seen.truncated),
            (MAX_CACHE_ENTRIES - 12, false)
        );

        seen.budget = 5;
        seen.truncated = false;
        seen.add_tree(&partition);
        assert_eq!((seen.budget, seen.truncated), (0, true), "{seen:?}");
        assert!(
            seen.real_dirs(&root).is_empty(),
            "nothing is read past the bound"
        );
    }

    #[test]
    fn a_root_that_is_not_a_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cache");
        fs::write(&root, b"file").unwrap();
        assert!(report(&root).is_err());
    }
}
