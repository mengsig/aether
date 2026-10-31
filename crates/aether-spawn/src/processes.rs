//! Lists the processes under `/proc` one at a time, as a sweep visits them.

use std::io;
use std::mem::MaybeUninit;
use std::path::Path;

use rustix::fs::{Mode, OFlags, RawDir, SeekFrom};

/// Room for any one `/proc` entry; a read fills it with those after it too.
const ENTRY_ROOM: usize = 256;

/// Visit the pid of each process listed under `proc_root`. The listing is
/// read afresh past each pid, so a child forked meanwhile is still visited,
/// as its pid lies past its parent's; one given a lower pid, as the pid
/// counter wraps around, is missed.
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
    let mut position = 0;
    loop {
        let (pid, next) = {
            let mut entries = RawDir::new(&listing, &mut buffer);
            let Some(entry) = entries.next() else {
                return Ok(());
            };
            let entry = entry?;
            let name = entry.file_name().to_str().ok();
            let pid = name.and_then(|name| name.parse::<u32>().ok());
            (pid, entry.next_entry_cookie())
        };
        assert_ne!(next, position, "a read of the listing did not advance");
        rustix::fs::seek(&listing, SeekFrom::Start(next))?;
        position = next;
        if let Some(pid) = pid.filter(|pid| *pid > 0) {
            visit(pid)?;
        }
    }
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
    /// lies ahead of where the listing has got to.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_process_forked_during_the_listing_is_visited() {
        let own = std::process::id();
        let mut forked = None;
        visit_processes(Path::new("/proc"), |pid| {
            if pid == own {
                forked = Some(std::process::Command::new("sleep").arg("5").spawn()?);
            }
            if let Some(child) = forked.as_mut().filter(|child| child.id() == pid) {
                child.kill()?;
            }
            Ok(())
        })
        .unwrap();
        let mut child = forked.expect("this process is listed");
        let killed = child.wait().unwrap();
        assert!(
            !killed.success(),
            "the child forked mid-listing was never visited"
        );
    }
}
