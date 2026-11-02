//! Lists the processes under `/proc` one at a time, as a sweep visits them.

use std::io;
use std::mem::MaybeUninit;
use std::path::Path;

use rustix::fs::{Mode, OFlags, RawDir, SeekFrom};

/// Room for any one `/proc` entry; a read fills it with those after it too.
const ENTRY_ROOM: usize = 256;

/// Visit the pid of each process listed under `proc_root`. Each read starts
/// again at the entry just visited, so what follows it is listed as it is
/// after the visit: a child forked meanwhile is still visited, as its pid lies
/// past its parent's; one given a lower pid, as the pid counter wraps, is missed.
pub fn visit_processes(
    proc_root: &Path,
    mut visit: impl FnMut(u32) -> io::Result<()>,
) -> io::Result<()> {
    let listing = rustix::fs::open(
        proc_root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut buffer = [MaybeUninit::<u8>::uninit(); ENTRY_ROOM];
    let mut handled = Handled::default();
    loop {
        rustix::fs::seek(&listing, SeekFrom::Start(handled.start))?;
        let Some(pid) = next_entry(&listing, &mut buffer, &mut handled)? else {
            return Ok(());
        };
        if let Some(pid) = pid.filter(|pid| *pid > 0) {
            visit(pid)?;
        }
    }
}

/// The entry last handled: where it starts and its name, once there is one.
#[derive(Default)]
struct Handled {
    start: u64,
    name: Vec<u8>,
}

/// Read from `handled.start`, step past the entry handled there if it is still
/// listed, and make the next entry the handled one. `Some` holds its pid, if
/// its name is one; `None` ends the listing.
fn next_entry(
    listing: &rustix::fd::OwnedFd,
    buffer: &mut [MaybeUninit<u8>],
    handled: &mut Handled,
) -> io::Result<Option<Option<u32>>> {
    let mut entries = RawDir::new(listing, buffer);
    let Some(first) = entries.next().transpose()? else {
        return Ok(None);
    };
    let (entry, start) =
        if !handled.name.is_empty() && first.file_name().to_bytes() == handled.name.as_slice() {
            let after = first.next_entry_cookie();
            assert_ne!(
                after, handled.start,
                "a read of the listing did not advance"
            );
            let Some(next) = entries.next().transpose()? else {
                return Ok(None);
            };
            (next, after)
        } else {
            (first, handled.start)
        };
    let name = entry.file_name().to_bytes();
    assert!(!name.is_empty(), "a listed entry has a name");
    handled.start = start;
    handled.name.clear();
    handled.name.extend_from_slice(name);
    let pid = std::str::from_utf8(name)
        .ok()
        .and_then(|name| name.parse().ok());
    Ok(Some(pid))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn every_numbered_entry_is_visited_once() {
        let root = tempfile::tempdir().unwrap();
        for name in ["7", "12", "self", "0", "300"] {
            std::fs::create_dir(root.path().join(name)).unwrap();
        }
        std::fs::write(root.path().join("uptime"), "1").unwrap();
        let mut visited = Vec::new();
        visit_processes(root.path(), |pid| {
            visited.push(pid);
            Ok(())
        })
        .unwrap();
        visited.sort_unstable();
        assert_eq!(visited, vec![7, 12, 300]);
    }

    #[test]
    fn a_visit_that_fails_ends_the_listing_with_its_error() {
        let root = tempfile::tempdir().unwrap();
        for name in ["1", "2", "3"] {
            std::fs::create_dir(root.path().join(name)).unwrap();
        }
        let mut visits = 0;
        let error = visit_processes(root.path(), |_| {
            visits += 1;
            Err(io::Error::other("stop"))
        })
        .unwrap_err();
        assert_eq!((visits, error.to_string().as_str()), (1, "stop"));
        assert!(visit_processes(&root.path().join("gone"), |_| Ok(())).is_err());
    }

    /// A process forked while the listing is visited is visited too: its pid
    /// lies ahead of where the listing has got to. The visit is recorded, not
    /// raced against the child's own exit, so a stalled host cannot fail it.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_process_forked_during_the_listing_is_visited() {
        let own = std::process::id();
        let mut forked = None;
        let mut child_visits = 0;
        let listed = visit_processes(Path::new("/proc"), |pid| {
            if pid == own {
                assert!(forked.is_none(), "this process was listed twice");
                forked = Some(std::process::Command::new("sleep").arg("300").spawn()?);
            }
            if forked.as_ref().is_some_and(|child| child.id() == pid) {
                child_visits += 1;
            }
            Ok(())
        });
        let mut child = forked.expect("this process is listed");
        child.kill().unwrap();
        assert!(
            !child.wait().unwrap().success(),
            "the child outlived its kill"
        );
        listed.unwrap();
        assert_eq!(
            child_visits, 1,
            "the child forked mid-listing was not visited exactly once"
        );
    }
}
