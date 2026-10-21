//! Removal uses open directory handles; symlinks are unlinked, never entered.

use std::os::unix::ffi::OsStrExt as _;

use super::*;

/// Remove only a leaf Aether created for this exact agent and instance.
/// The caller must reserve admission and prove the agent is permanently final.
pub fn collect_leaf(
    root: &Path,
    agent: AgentId,
    instance_id: &str,
    dry_run: bool,
) -> io::Result<&'static str> {
    if !root.is_absolute() || instance_id.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid agent temp root or instance",
        ));
    }
    check_ancestors(root, rustix::process::geteuid().as_raw())?;
    let parent = match open_root(root, rustix::process::geteuid().as_raw()) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok("missing"),
        opened => opened?,
    };
    if read_marker(&parent, root)? != instance_id {
        return Ok("foreign instance");
    }
    let name = leaf_name(agent);
    let leaf = match rustix::fs::openat(&parent, name.as_str(), DIR_FLAGS, Mode::empty()) {
        Ok(leaf) => leaf,
        Err(Errno::NOENT) => return Ok("missing"),
        Err(Errno::LOOP | Errno::NOTDIR | Errno::MLINK) => return Ok("not a real directory"),
        Err(error) => return Err(error.into()),
    };
    check_leaf(
        &root.join(&name),
        &rustix::fs::fstat(&leaf)?,
        rustix::process::geteuid().as_raw(),
    )?;
    let claim = match read_marker(&leaf, &root.join(&name)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok("ownership unproven"),
        read => read?,
    };
    if claim != agent.to_string() {
        return Ok("different agent owner");
    }
    debug_assert!(is_leaf_name(&name));
    debug_assert_eq!(root.join(&name).parent(), Some(root));
    remove_contents(&leaf, 0, &mut 10_000, true)?;
    if dry_run {
        return Ok("would_remove");
    }
    remove_contents(&leaf, 0, &mut 10_000, false)?;
    same_directory(&parent, OsStr::new(&name), &leaf)?;
    rustix::fs::unlinkat(&leaf, MARKER, AtFlags::empty())?;
    rustix::fs::unlinkat(&parent, name.as_str(), AtFlags::REMOVEDIR)?;
    Ok("removed")
}

fn same_directory(parent: &OwnedFd, name: &OsStr, opened: &OwnedFd) -> io::Result<()> {
    let named = rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?;
    let opened = rustix::fs::fstat(opened)?;
    if file_type(&named) != FileType::Directory
        || named.st_dev != opened.st_dev
        || named.st_ino != opened.st_ino
    {
        return Err(io::Error::other("agent temp directory identity changed"));
    }
    Ok(())
}

fn remove_contents(
    dir: &OwnedFd,
    depth: usize,
    budget: &mut usize,
    dry_run: bool,
) -> io::Result<()> {
    if depth >= 64 {
        return Err(io::Error::other(
            "agent temp directory depth exceeds cleanup limit",
        ));
    }
    debug_assert!(depth < 64);
    debug_assert!(*budget <= 10_000);
    for entry in Dir::read_from(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == c"." || name == c".." || (depth == 0 && name == c".aether-instance") {
            continue;
        }
        if *budget == 0 {
            return Err(io::Error::other(
                "agent temp directory exceeds cleanup entry limit",
            ));
        }
        *budget -= 1;
        let stat = rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)?;
        if file_type(&stat) != FileType::Directory {
            if !dry_run {
                rustix::fs::unlinkat(dir, name, AtFlags::empty())?;
            }
            continue;
        }
        let child = rustix::fs::openat(dir, name, DIR_FLAGS, Mode::empty())?;
        if rustix::fs::fstat(&child)?.st_dev != rustix::fs::fstat(dir)?.st_dev {
            return Err(io::Error::other(
                "agent temp cleanup refuses a different filesystem",
            ));
        }
        remove_contents(&child, depth + 1, budget, dry_run)?;
        same_directory(dir, OsStr::from_bytes(name.to_bytes()), &child)?;
        if !dry_run {
            rustix::fs::unlinkat(dir, name, AtFlags::REMOVEDIR)?;
        }
    }
    Ok(())
}
