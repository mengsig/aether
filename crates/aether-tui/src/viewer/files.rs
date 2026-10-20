use anyhow::{Context, Result, ensure};
use nix::{
    fcntl::{OFlag, open, openat},
    sys::stat::Mode,
};
use std::{
    fs::File,
    path::{Component, Path},
};

/// Open a resolved path without following a symlink introduced after resolution.
pub(crate) fn open_canonical(path: &Path) -> Result<File> {
    ensure!(path.is_absolute(), "preview needs a resolved absolute path");
    let mut parts = path.components().peekable();
    ensure!(
        parts.next() == Some(Component::RootDir),
        "preview needs a local root"
    );
    let directory = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW;
    let mut fd = open(Path::new("/"), directory, Mode::empty()).context("open local root")?;
    while let Some(component) = parts.next() {
        ensure!(
            matches!(component, Component::Normal(_)),
            "preview path must be canonical"
        );
        let flags = if parts.peek().is_none() {
            OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK
        } else {
            directory
        };
        fd = openat(&fd, Path::new(component.as_os_str()), flags, Mode::empty())
            .with_context(|| format!("open resolved file {}", path.display()))?;
    }
    let file = File::from(fd);
    ensure!(file.metadata()?.is_file(), "preview needs a regular file");
    Ok(file)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn late_symlinks_and_nonregular_paths_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("file.txt"), "safe").unwrap();
        assert!(open_canonical(&root.join("file.txt")).is_ok());
        symlink(root.join("file.txt"), root.join("link.txt")).unwrap();
        symlink(&root, root.join("alias")).unwrap();
        assert!(open_canonical(&root.join("link.txt")).is_err());
        assert!(open_canonical(&root.join("alias/file.txt")).is_err());
        assert!(open_canonical(&root).is_err());
        assert!(open_canonical(Path::new("relative.txt")).is_err());
        nix::unistd::mkfifo(&root.join("fifo"), Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
        assert!(open_canonical(&root.join("fifo")).is_err());
    }
}
