//! Scoped filesystem resolution. Every path a tool touches goes through here.

use std::path::{Component, Path, PathBuf};

use crate::tool::{ToolContext, ToolError};

const MAX_SYMLINK_REDIRECTS: usize = 40;

pub(crate) const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) fn open_parent(path: &Path, create: bool) -> Result<std::fs::File, ToolError> {
    let parent = path
        .parent()
        .filter(|_| path.is_absolute())
        .ok_or_else(|| {
            ToolError::InvalidInput(format!("{} must be an absolute file path", path.display()))
        })?;
    open_directory(parent, create)
}

pub(crate) fn open_directory(path: &Path, create: bool) -> Result<std::fs::File, ToolError> {
    use nix::fcntl::{open, openat};
    use nix::sys::stat::{Mode, mkdirat};

    if !path.is_absolute() {
        return Err(ToolError::InvalidInput(format!(
            "{} must be an absolute directory path",
            path.display()
        )));
    }
    let flags = directory_search_flags();
    let mut dir: std::fs::File = open("/", flags, Mode::empty())
        .map_err(std::io::Error::from)?
        .into();
    for component in path.components() {
        let name = match component {
            Component::RootDir => continue,
            Component::Normal(name) => Path::new(name),
            _ => {
                return Err(ToolError::InvalidInput(
                    "file path is not normalized".into(),
                ));
            }
        };
        let next = match openat(&dir, name, flags, Mode::empty()) {
            Err(nix::errno::Errno::ENOENT) if create => {
                match mkdirat(&dir, name, Mode::from_bits_truncate(0o777)) {
                    Ok(()) | Err(nix::errno::Errno::EEXIST) => {}
                    Err(error) => return Err(std::io::Error::from(error).into()),
                }
                openat(&dir, name, flags, Mode::empty())
            }
            result => result,
        }
        .map_err(std::io::Error::from)?;
        dir = next.into();
    }
    Ok(dir)
}

pub(crate) fn directory_search_flags() -> nix::fcntl::OFlag {
    use nix::fcntl::OFlag;

    #[cfg(target_os = "linux")]
    let access = OFlag::O_PATH;
    #[cfg(target_os = "macos")]
    let access = OFlag::O_SEARCH;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let access = OFlag::O_RDONLY;
    // File access requires directory search permission, not permission to list its contents.
    access | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC
}

pub(crate) fn open_regular_at(
    parent: &std::fs::File,
    path: &Path,
    flags: nix::fcntl::OFlag,
) -> Result<std::fs::File, ToolError> {
    use nix::fcntl::{AtFlags, OFlag, openat};
    use nix::sys::stat::{Mode, SFlag, fstatat};

    let name = path.file_name().ok_or_else(|| not_regular(path))?;
    let name = Path::new(name);
    let stat = fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(std::io::Error::from)?;
    if SFlag::from_bits_truncate(stat.st_mode) != SFlag::S_IFREG {
        return Err(not_regular(path));
    }
    // Component handles pin the directory; nonblocking/no-follow also protect the final open.
    let file: std::fs::File = openat(
        parent,
        name,
        flags | OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?
    .into();
    if !file.metadata()?.is_file() {
        return Err(not_regular(path));
    }
    Ok(file)
}

pub(crate) fn root_name(path: &Path) -> Result<&std::ffi::OsStr, ToolError> {
    match path.components().next_back() {
        Some(Component::Normal(name)) => Ok(name),
        Some(Component::RootDir) => Ok(std::ffi::OsStr::new(".")),
        _ => Err(ToolError::InvalidInput(
            "file path is not normalized".into(),
        )),
    }
}

pub(crate) fn file_type(path: &Path) -> Result<nix::sys::stat::SFlag, ToolError> {
    use nix::fcntl::AtFlags;
    use nix::sys::stat::{SFlag, fstatat};

    let name = root_name(path)?;
    let parent = if path.parent().is_some() {
        open_parent(path, false)?
    } else {
        open_directory(path, false)?
    };
    let stat = fstatat(&parent, Path::new(name), AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(std::io::Error::from)?;
    Ok(SFlag::from_bits_truncate(stat.st_mode))
}

fn not_regular(path: &Path) -> ToolError {
    ToolError::InvalidInput(format!("{} is not a regular file", path.display()))
}

pub(crate) fn read_regular_file(path: &Path) -> Result<Vec<u8>, ToolError> {
    let parent = open_parent(path, false)?;
    read_regular_file_at(&parent, path)
}

pub(crate) fn read_regular_file_at(
    parent: &std::fs::File,
    path: &Path,
) -> Result<Vec<u8>, ToolError> {
    let file = open_regular_at(parent, path, nix::fcntl::OFlag::O_RDONLY)?;
    if file.metadata()?.len() > MAX_FILE_BYTES {
        return Err(file_too_large(path));
    }
    read_bounded(file, path)
}

fn read_bounded(reader: impl std::io::Read, path: &Path) -> Result<Vec<u8>, ToolError> {
    use std::io::Read;

    // The limit also covers growth after stat; never allocate from an untrusted file size.
    let mut bytes = Vec::new();
    reader.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(file_too_large(path));
    }
    Ok(bytes)
}

fn file_too_large(path: &Path) -> ToolError {
    ToolError::InvalidInput(format!(
        "{} exceeds the file-tool size limit of {} MiB; use grep or bash with sed -n for larger files",
        path.display(),
        MAX_FILE_BYTES / (1024 * 1024)
    ))
}

/// Replace the resolved target inode; other hard links retain their old contents.
/// In-scope symlink aliases are resolved by the caller before opening any components.
/// Replacement fails if the original ownership cannot be preserved.
pub(crate) fn write_regular_file(path: &Path, bytes: &[u8]) -> Result<bool, ToolError> {
    use std::io::Write;
    let parent = open_parent(path, true)?;
    replace_regular_at(&parent, path, |file| file.write_all(bytes))
}

fn replace_regular_at(
    parent: &std::fs::File,
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<bool, ToolError> {
    use nix::fcntl::{OFlag, openat, renameat};
    use nix::sys::stat::Mode;
    use nix::unistd::{UnlinkatFlags, unlinkat};

    let metadata = match open_regular_at(parent, path, OFlag::O_WRONLY) {
        Ok(file) => Some(file.metadata()?),
        Err(ToolError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let temporary = format!(".aether-write-{:032x}", rand::random::<u128>());
    let temporary = Path::new(&temporary);
    let mode = if metadata.is_some() { 0o600 } else { 0o666 };
    let mut file: std::fs::File = openat(
        parent,
        temporary,
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(mode),
    )
    .map_err(std::io::Error::from)?
    .into();
    let result = (|| -> std::io::Result<()> {
        write(&mut file)?;
        if let Some(metadata) = &metadata {
            use nix::unistd::{Gid, Uid, fchown};
            use std::os::unix::fs::MetadataExt;
            // Refuse replacement if ownership cannot be preserved. Chown can clear mode bits.
            fchown(
                &file,
                Some(Uid::from_raw(metadata.uid())),
                Some(Gid::from_raw(metadata.gid())),
            )
            .map_err(std::io::Error::from)?;
            file.set_permissions(metadata.permissions())?;
        }
        file.sync_all()?;
        renameat(
            parent,
            temporary,
            parent,
            Path::new(path.file_name().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "path names no file")
            })?),
        )
        .map_err(std::io::Error::from)?;
        Ok(())
    })();
    if let Err(error) = result {
        if let Err(cleanup) = unlinkat(parent, temporary, UnlinkatFlags::NoRemoveDir) {
            return Err(std::io::Error::new(
                error.kind(),
                format!("{error}; removing temporary file failed: {cleanup}"),
            )
            .into());
        }
        return Err(error.into());
    }
    Ok(metadata.is_some())
}

/// Resolve `input` against the context and prove it lies inside the scope.
///
/// Symlinks in the existing part of the path are resolved before the check,
/// so a link that escapes the workspace is rejected. Components that do not
/// exist yet (a file about to be written) are appended lexically.
pub fn resolve_path(ctx: &ToolContext, input: impl AsRef<Path>) -> Result<PathBuf, ToolError> {
    let input = input.as_ref();
    if input.as_os_str().is_empty() {
        return Err(ToolError::InvalidInput("path is empty".into()));
    }
    let joined = if input.is_absolute() {
        input.to_path_buf()
    } else {
        ctx.cwd.join(input)
    };
    let resolved = canonicalize_lenient(&joined, MAX_SYMLINK_REDIRECTS)?;
    let roots = allowed_roots(ctx)?;
    if roots.iter().any(|root| resolved.starts_with(root)) {
        Ok(resolved)
    } else {
        Err(ToolError::OutOfScope(resolved))
    }
}

/// Lexical resolution without touching the filesystem, for resource keys.
pub fn lexical_resolve(ctx: &ToolContext, input: impl AsRef<Path>) -> PathBuf {
    let input = input.as_ref();
    let joined = if input.is_absolute() {
        input.to_path_buf()
    } else {
        ctx.cwd.join(input)
    };
    normalize_lexically(&joined)
}

/// Keep the declared path for resource rules and add its filesystem target for leases.
/// Unresolvable paths keep their declared key; file execution reports the resolution error.
pub fn resource_paths(ctx: &ToolContext, input: impl AsRef<Path>) -> Vec<PathBuf> {
    let input = input.as_ref();
    let mut paths = vec![lexical_resolve(ctx, input)];
    let joined = if input.is_absolute() {
        input.to_path_buf()
    } else {
        ctx.cwd.join(input)
    };
    if let Ok(resolved) = canonicalize_lenient(&joined, MAX_SYMLINK_REDIRECTS)
        && resolved != paths[0]
    {
        paths.push(resolved);
    }
    paths
}

/// The directories an agent may touch: workspace root plus allowed paths,
/// or only the cwd when no workspace root is configured.
fn allowed_roots(ctx: &ToolContext) -> Result<Vec<PathBuf>, ToolError> {
    let mut roots = Vec::new();
    match &ctx.scope.workspace_root {
        Some(root) => {
            roots.push(std::fs::canonicalize(root).map_err(|e| {
                ToolError::Internal(format!(
                    "workspace root {} unavailable: {e}",
                    root.display()
                ))
            })?);
            for extra in &ctx.scope.allowed_paths {
                // A missing allowed path grants nothing; it is not an error.
                if let Ok(p) = std::fs::canonicalize(extra) {
                    roots.push(p);
                }
            }
        }
        None => roots.push(std::fs::canonicalize(&ctx.cwd).map_err(|e| {
            ToolError::Internal(format!("cwd {} unavailable: {e}", ctx.cwd.display()))
        })?),
    }
    // Shell logs live here, and the tool results point the agent at them.
    if let Some(Ok(p)) = ctx.temp_dir.as_ref().map(std::fs::canonicalize) {
        roots.push(p);
    }
    Ok(roots)
}

/// Canonicalize the longest existing prefix, then append the rest lexically.
fn canonicalize_lenient(path: &Path, remaining_links: usize) -> Result<PathBuf, ToolError> {
    let comps: Vec<Component<'_>> = path.components().collect();
    let mut prefix_len = comps.len();
    let mut base = None;
    while prefix_len > 0 {
        let candidate: PathBuf = comps[..prefix_len].iter().collect();
        match std::fs::canonicalize(&candidate) {
            Ok(c) => {
                base = Some(c);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // A dangling link exists even though canonicalize reports NotFound.
                // Follow its target before appending components or checking scope.
                match std::fs::read_link(&candidate) {
                    Ok(target) => {
                        if remaining_links == 0 {
                            return Err(ToolError::InvalidInput(format!(
                                "too many symlinks resolving {}",
                                path.display()
                            )));
                        }
                        let mut redirected =
                            candidate.parent().unwrap_or(Path::new("/")).join(target);
                        redirected.extend(comps[prefix_len..].iter().map(|comp| comp.as_os_str()));
                        return canonicalize_lenient(&redirected, remaining_links - 1);
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
                        ) =>
                    {
                        prefix_len -= 1;
                    }
                    Err(e) => return Err(ToolError::Io(e)),
                }
            }
            Err(e) => return Err(ToolError::Io(e)),
        }
    }
    let mut out =
        base.ok_or_else(|| ToolError::InvalidInput(format!("cannot resolve {}", path.display())))?;
    for comp in &comps[prefix_len..] {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(n) => out.push(n),
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    // Removing a missing component with `..` can expose an existing symlink.
    // Resolve the normalized path again before treating it as a final target.
    if out != path {
        return canonicalize_lenient(&out, remaining_links);
    }
    Ok(out)
}

pub(crate) fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ctx_in;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn file_access_does_not_require_readable_parent_directories() {
        use std::os::unix::fs::PermissionsExt;
        // Root bypasses the permission check this regression exercises.
        if nix::unistd::geteuid().is_root() {
            eprintln!("SKIPPED: root bypasses directory permission checks");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().canonicalize().unwrap().join("searchable");
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("file");
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o300)).unwrap();
        let read = read_regular_file(&path);
        let write = write_regular_file(&path, b"new");
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(read.unwrap(), b"old");
        assert!(write.unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn failed_replacement_preserves_contents_and_removes_temporary() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("file");
        std::fs::write(&path, b"original").unwrap();
        let parent = open_parent(&path, false).unwrap();
        let result = replace_regular_at(&parent, &path, |file| {
            file.write_all(b"partial")?;
            Err(std::io::Error::from_raw_os_error(nix::libc::ENOSPC))
        });
        assert!(
            matches!(result, Err(ToolError::Io(ref error)) if error.raw_os_error() == Some(nix::libc::ENOSPC))
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn replacement_preserves_mode_and_leaves_other_hard_links_unchanged() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("file");
        let alias = dir.path().join("alias");
        std::fs::write(&path, b"original").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        std::fs::hard_link(&path, &alias).unwrap();
        if nix::unistd::geteuid().is_root() {
            nix::unistd::fchown(
                std::fs::File::open(&path).unwrap(),
                Some(nix::unistd::Uid::from_raw(65534)),
                Some(nix::unistd::Gid::from_raw(65534)),
            )
            .unwrap();
        } else if let Some(group) = nix::unistd::getgroups()
            .unwrap()
            .into_iter()
            .find(|group| *group != nix::unistd::getegid())
        {
            nix::unistd::fchown(std::fs::File::open(&path).unwrap(), None, Some(group)).unwrap();
        } else {
            eprintln!(
                "SKIPPED: differing ownership requires root or a supplementary group; same-owner replacement still checked"
            );
        }
        use std::os::unix::fs::MetadataExt;
        let original = std::fs::metadata(&path).unwrap();
        assert!(write_regular_file(&path, b"replacement").unwrap());
        let replaced = std::fs::metadata(&path).unwrap();
        assert_eq!(
            (replaced.uid(), replaced.gid()),
            (original.uid(), original.gid())
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        assert_eq!(std::fs::read(&alias).unwrap(), b"original");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn directory_symlink_swap_after_resolution_cannot_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/file"), b"inside").unwrap();
        std::fs::write(outside.path().join("file"), b"secret").unwrap();
        let path = resolve_path(&ctx_in(dir.path()), "sub/file").unwrap();
        let new_path = resolve_path(&ctx_in(dir.path()), "sub/new/created").unwrap();
        std::fs::rename(dir.path().join("sub"), dir.path().join("moved")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("sub")).unwrap();
        assert!(read_regular_file(&path).is_err());
        assert!(write_regular_file(&path, b"escape").is_err());
        assert!(write_regular_file(&new_path, b"escape").is_err());
        assert_eq!(
            std::fs::read(outside.path().join("file")).unwrap(),
            b"secret"
        );
        assert!(!outside.path().join("new").exists());
    }

    #[test]
    fn pinned_parent_does_not_follow_a_later_symlink_swap() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let path = dir.path().canonicalize().unwrap().join("sub/file");
        let parent = open_parent(&path, false).unwrap();
        std::fs::rename(dir.path().join("sub"), dir.path().join("moved")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("sub")).unwrap();
        use std::io::Write;
        replace_regular_at(&parent, &path, |file| file.write_all(b"inside")).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("moved/file")).unwrap(),
            b"inside"
        );
        assert!(!outside.path().join("file").exists());
    }

    #[test]
    fn bounded_read_covers_growth_and_accepts_exact_limit() {
        use std::io::Read;
        let path = Path::new("growing");
        assert_eq!(
            read_bounded(std::io::repeat(b'x').take(MAX_FILE_BYTES), path)
                .unwrap()
                .len() as u64,
            MAX_FILE_BYTES
        );
        assert!(
            matches!(read_bounded(std::io::repeat(b'x'), path), Err(ToolError::InvalidInput(ref error)) if error.contains("size limit"))
        );
    }

    #[test]
    fn relative_paths_resolve_inside_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let p = resolve_path(&ctx, "src/new.rs").unwrap();
        assert_eq!(p, dir.path().canonicalize().unwrap().join("src/new.rs"));
    }

    #[test]
    fn dotdot_escape_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        assert!(matches!(
            resolve_path(&ctx, "../outside.txt"),
            Err(ToolError::OutOfScope(_))
        ));
        assert!(matches!(
            resolve_path(&ctx, "a/b/../../../x"),
            Err(ToolError::OutOfScope(_))
        ));
    }

    #[test]
    fn absolute_path_outside_is_rejected_inside_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        assert!(matches!(
            resolve_path(&ctx, "/etc/passwd"),
            Err(ToolError::OutOfScope(_))
        ));
        let inside = dir.path().join("ok.txt");
        assert!(resolve_path(&ctx, &inside).is_ok());
    }

    #[test]
    fn symlink_escape_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        let ctx = ctx_in(dir.path());
        assert!(matches!(
            resolve_path(&ctx, "link/secret"),
            Err(ToolError::OutOfScope(_))
        ));
        assert!(matches!(
            resolve_path(&ctx, "link/new_file"),
            Err(ToolError::OutOfScope(_))
        ));
    }

    #[test]
    fn dangling_symlinks_resolve_to_their_missing_targets() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        std::os::unix::fs::symlink("missing/file", dir.path().join("alias")).unwrap();
        assert_eq!(
            resolve_path(&ctx, "alias").unwrap(),
            dir.path().canonicalize().unwrap().join("missing/file")
        );
        std::os::unix::fs::symlink("alias", dir.path().join("chain")).unwrap();
        assert_eq!(
            resolve_path(&ctx, "chain").unwrap(),
            dir.path().canonicalize().unwrap().join("missing/file")
        );
        std::os::unix::fs::symlink("missing", dir.path().join("parent_alias")).unwrap();
        assert_eq!(
            resolve_path(&ctx, "parent_alias/new/file").unwrap(),
            dir.path().canonicalize().unwrap().join("missing/new/file")
        );
    }

    #[test]
    fn dangling_symlinks_cannot_escape_scope() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        std::os::unix::fs::symlink(outside.path().join("new/file"), dir.path().join("alias"))
            .unwrap();
        assert!(matches!(
            resolve_path(&ctx, "alias"),
            Err(ToolError::OutOfScope(_))
        ));
        std::os::unix::fs::symlink(outside.path().join("new"), dir.path().join("parent_alias"))
            .unwrap();
        assert!(matches!(
            resolve_path(&ctx, "parent_alias/file"),
            Err(ToolError::OutOfScope(_))
        ));
    }

    #[test]
    fn missing_parent_traversal_cannot_hide_an_escaping_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        std::fs::write(outside.path().join("file"), "outside").unwrap();
        std::os::unix::fs::symlink(outside.path().join("file"), dir.path().join("alias")).unwrap();
        assert!(matches!(
            resolve_path(&ctx, "missing/../alias"),
            Err(ToolError::OutOfScope(_))
        ));
    }

    #[test]
    fn symlink_resolution_cycles_fail_visibly() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        std::os::unix::fs::symlink("loop", dir.path().join("loop")).unwrap();
        assert!(matches!(resolve_path(&ctx, "loop"), Err(ToolError::Io(_))));
        std::os::unix::fs::symlink("missing/../indirect", dir.path().join("indirect")).unwrap();
        assert!(matches!(
            resolve_path(&ctx, "indirect"),
            Err(ToolError::InvalidInput(_))
        ));
    }

    #[test]
    fn resource_paths_preserve_declared_aliases_and_resolution_failures() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let ctx = ctx_in(&root);
        std::os::unix::fs::symlink("missing/file", dir.path().join("alias")).unwrap();
        assert_eq!(
            resource_paths(&ctx, "alias"),
            vec![root.join("alias"), root.join("missing/file")]
        );
        assert_eq!(resource_paths(&ctx, "new"), vec![root.join("new")]);
        std::os::unix::fs::symlink("loop", dir.path().join("loop")).unwrap();
        assert_eq!(resource_paths(&ctx, "loop"), vec![root.join("loop")]);
        assert!(resolve_path(&ctx, "loop").is_err());
    }

    #[test]
    fn the_agent_temp_dir_is_in_scope_with_or_without_a_workspace_root() {
        let work = tempfile::tempdir().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("output.log");
        std::fs::write(&log, "x").unwrap();
        let mut ctx = ctx_in(work.path());
        assert!(resolve_path(&ctx, &log).is_err());
        ctx.temp_dir = Some(temp.path().to_path_buf());
        assert!(resolve_path(&ctx, &log).is_ok());
        ctx.scope.workspace_root = None;
        assert!(resolve_path(&ctx, &log).is_ok());
    }

    #[test]
    fn allowed_paths_extend_scope() {
        let dir = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let mut ctx = ctx_in(dir.path());
        ctx.scope.allowed_paths.push(extra.path().to_path_buf());
        assert!(resolve_path(&ctx, extra.path().join("f")).is_ok());
    }

    #[test]
    fn no_workspace_root_limits_to_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_in(dir.path());
        ctx.scope.workspace_root = None;
        assert!(resolve_path(&ctx, "inside").is_ok());
        assert!(matches!(
            resolve_path(&ctx, "/tmp"),
            Err(ToolError::OutOfScope(_))
        ));
    }

    #[test]
    fn empty_path_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        assert!(matches!(
            resolve_path(&ctx, ""),
            Err(ToolError::InvalidInput(_))
        ));
    }

    #[test]
    fn lexical_resolution_normalizes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        assert_eq!(lexical_resolve(&ctx, "a/./b/../c"), dir.path().join("a/c"));
    }
}
