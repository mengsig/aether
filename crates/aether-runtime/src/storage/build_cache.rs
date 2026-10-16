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
}

/// Bytes, repositories and partitions under `root`. Below the configured
/// root, links are never followed, and files a running build deletes
/// mid-walk are skipped.
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
    let mut budget = MAX_CACHE_ENTRIES;
    for repository in real_dirs(root, &mut report) {
        report.repositories += 1;
        for partition in real_dirs(&repository, &mut report) {
            report.partitions += 1;
            add_tree(partition, &mut report, &mut budget);
        }
    }
    debug_assert!(budget <= MAX_CACHE_ENTRIES, "{budget}");
    Ok(report)
}

/// The directories directly in `dir`, links and files left out.
fn real_dirs(dir: &Path, report: &mut BuildCacheReport) -> Vec<PathBuf> {
    let Some(entries) = report.readable(fs::read_dir(dir)) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| report.readable(entry))
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

/// Add the blocks of everything under `top`, itself included, spending one
/// unit of `budget` per entry.
fn add_tree(top: PathBuf, report: &mut BuildCacheReport, budget: &mut usize) {
    let mut pending = vec![top];
    while let Some(path) = pending.pop() {
        if *budget == 0 {
            report.truncated = true;
            return;
        }
        *budget -= 1;
        let Some(meta) = report.readable(fs::symlink_metadata(&path)) else {
            continue;
        };
        report.bytes = report
            .bytes
            .saturating_add(meta.blocks().saturating_mul(512));
        if !meta.is_dir() {
            continue;
        }
        let Some(entries) = report.readable(fs::read_dir(&path)) else {
            continue;
        };
        pending.extend(
            entries
                .filter_map(|entry| report.readable(entry))
                .map(|entry| entry.path()),
        );
    }
}

impl BuildCacheReport {
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
    fn a_root_that_is_not_a_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cache");
        fs::write(&root, b"file").unwrap();
        assert!(report(&root).is_err());
    }
}
