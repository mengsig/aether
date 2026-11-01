//! Which partitions a running process points into, whoever started it: an
//! engine's native background job or an orphaned build is not in the store.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Processes read at most; a host with more is not checked at all.
#[cfg(target_os = "linux")]
const MAX_PROCESSES: usize = 65_536;

/// The pids whose environment or working directory lies inside each
/// partition under `root`, or `None` where processes cannot be read, so the
/// caller must treat every partition as possibly in use.
#[cfg(target_os = "linux")]
pub(super) fn using_partitions(root: &Path) -> Option<HashMap<PathBuf, Vec<u32>>> {
    debug_assert!(root.is_absolute(), "{} is not absolute", root.display());
    let resolved = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let roots = [root, resolved.as_path()];
    let mut users: HashMap<PathBuf, Vec<u32>> = HashMap::new();
    let entries = std::fs::read_dir("/proc").ok()?;
    let mut read = 0usize;
    for entry in entries {
        let Some(pid) = entry
            .ok()
            .and_then(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        else {
            continue;
        };
        read += 1;
        if read > MAX_PROCESSES {
            return None;
        }
        for partition in partitions_of(pid, &roots) {
            let pids = users.entry(partition).or_default();
            if !pids.contains(&pid) {
                pids.push(pid);
            }
        }
    }
    debug_assert!(
        users.keys().all(|partition| partition.starts_with(root)),
        "every partition is named under the configured root"
    );
    Some(users)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn using_partitions(_root: &Path) -> Option<HashMap<PathBuf, Vec<u32>>> {
    None
}

/// The partitions `pid`'s environment values and working directory name.
/// A process gone or not readable by this user names none.
#[cfg(target_os = "linux")]
fn partitions_of(pid: u32, roots: &[&Path; 2]) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStrExt as _;
    let proc_dir = PathBuf::from(format!("/proc/{pid}"));
    let mut named = Vec::new();
    if let Ok(environ) = std::fs::read(proc_dir.join("environ")) {
        for pair in environ.split(|&byte| byte == 0) {
            let Some(at) = pair.iter().position(|&byte| byte == b'=') else {
                continue;
            };
            named.push(PathBuf::from(std::ffi::OsStr::from_bytes(&pair[at + 1..])));
        }
    }
    if let Ok(cwd) = std::fs::read_link(proc_dir.join("cwd")) {
        named.push(cwd);
    }
    named
        .iter()
        .filter_map(|path| partition_under(path, roots))
        .collect()
}

/// `<root>/<repository>/<run>` for a path at or below a partition under
/// either name of the root; the partition is always named under the first.
fn partition_under(path: &Path, roots: &[&Path; 2]) -> Option<PathBuf> {
    roots.iter().find_map(|root| {
        let rest = path.strip_prefix(root).ok()?;
        let mut parts = rest.components();
        let (
            Some(std::path::Component::Normal(repository)),
            Some(std::path::Component::Normal(run)),
        ) = (parts.next(), parts.next())
        else {
            return None;
        };
        Some(roots[0].join(repository).join(run))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_path_names_the_partition_it_lies_in() {
        let root = Path::new("/srv/cache");
        let real = Path::new("/data/cache");
        let roots = [root, real];
        let partition = root.join("repo-0123456789abcdef/run_one");
        for path in [
            "/srv/cache/repo-0123456789abcdef/run_one",
            "/srv/cache/repo-0123456789abcdef/run_one/cargo-target/debug",
            "/data/cache/repo-0123456789abcdef/run_one/cargo-target",
        ] {
            assert_eq!(
                partition_under(Path::new(path), &roots).as_ref(),
                Some(&partition),
                "{path}"
            );
        }
        for path in [
            "/srv/cache",
            "/srv/cache/repo-0123456789abcdef",
            "/srv/cache/../cache/repo/run",
            "/srv/cachex/repo/run",
            "/home/user/target",
        ] {
            assert_eq!(partition_under(Path::new(path), &roots), None, "{path}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_child_whose_environment_points_into_a_partition_is_seen() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cache");
        let partition = root.join("repo-0123456789abcdef/run_one");
        std::fs::create_dir_all(&partition).unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .env("CARGO_TARGET_DIR", partition.join("cargo-target"))
            .spawn()
            .unwrap();
        let users = using_partitions(&root).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(users.get(&partition), Some(&vec![child.id()]), "{users:?}");
        assert_eq!(users.len(), 1, "{users:?}");
    }
}
