//! Executable fixtures that tests can run while other tests spawn processes.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

/// Writes `contents` to `path` as an executable, without this process ever
/// holding a write fd on it: a sibling test's fork inherits any such fd until
/// its exec, and running the script meanwhile fails with ETXTBSY. A rename
/// cannot help - the inherited fd is on the same inode - so a child writes it.
pub(crate) fn write(path: &Path, contents: &str) {
    let status = std::process::Command::new("/bin/sh")
        .args(["-c", r#"printf '%s' "$1" > "$2""#, "sh", contents])
        .arg(path)
        .status()
        .expect("spawn sh to write a script");
    assert!(
        status.success(),
        "writing {} failed: {status}",
        path.display()
    );
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .expect("make the script executable");
}
