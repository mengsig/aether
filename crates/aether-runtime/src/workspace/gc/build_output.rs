//! Declared build output (`target/`, `node_modules/`, ...) that Git reports
//! as wholly ignored. GC sizes it without the entry cap and may delete it.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildOutputAction {
    Kept,
    WouldPrune,
    Pruned,
}

#[derive(Debug)]
pub(crate) struct BuildOutput {
    pub path: PathBuf,
    /// Allocated disk blocks, as `du` counts them.
    pub bytes: u64,
    pub action: BuildOutputAction,
    device: u64,
    inode: u64,
}

impl BuildOutput {
    /// Size a directory iteratively without following symlinks; a deep tree
    /// costs time, not stack. The newest modification time feeds the grace check.
    pub fn measure(path: PathBuf, device: u64) -> crate::Result<(Self, SystemTime)> {
        let root = fs::symlink_metadata(&path)
            .map_err(|error| io_error("inspect build output", &path, error))?;
        if !root.is_dir() || root.dev() != device {
            return Err(invalid(format!(
                "build output {} is not a directory on the workspace filesystem",
                path.display()
            )));
        }
        let mut bytes = 0_u64;
        let mut modified = SystemTime::UNIX_EPOCH;
        let mut pending = vec![path.clone()];
        while let Some(entry) = pending.pop() {
            let metadata = fs::symlink_metadata(&entry)
                .map_err(|error| io_error("inspect build output entry", &entry, error))?;
            if metadata.dev() != device {
                return Err(invalid("workspace build output contains a mount boundary"));
            }
            bytes = bytes.saturating_add(metadata.blocks().saturating_mul(512));
            modified = modified.max(
                metadata
                    .modified()
                    .map_err(|error| io_error("read build output age", &entry, error))?,
            );
            if !metadata.is_dir() {
                continue;
            }
            for child in fs::read_dir(&entry)
                .map_err(|error| io_error("read build output directory", &entry, error))?
            {
                let child =
                    child.map_err(|error| io_error("read build output entry", &entry, error))?;
                pending.push(child.path());
            }
        }
        let output = Self {
            path,
            bytes,
            action: BuildOutputAction::Kept,
            device: root.dev(),
            inode: root.ino(),
        };
        Ok((output, modified))
    }

    /// Delete through directory handles pinned from the candidate without
    /// following symlinks, so a swapped ancestor or replaced directory is refused.
    pub fn prune(&mut self, candidate: &Path) -> crate::Result<()> {
        debug_assert_eq!(self.action, BuildOutputAction::Kept, "pruned twice");
        debug_assert!(
            self.path.starts_with(candidate),
            "build output is in its candidate"
        );
        let parent = pin_beneath(candidate, &self.path)?;
        let name = self
            .path
            .file_name()
            .ok_or_else(|| invalid("build output has no name"))?;
        let root = self.pin_directory(&parent, name, &self.path)?;
        let pinned = root
            .metadata()
            .map_err(|error| io_error("inspect pinned build output", &self.path, error))?;
        if pinned.ino() != self.inode {
            return Err(invalid(format!(
                "build output {} changed during GC",
                self.path.display()
            )));
        }
        self.remove_tree(&parent, root)?;
        self.action = BuildOutputAction::Pruned;
        Ok(())
    }

    /// Depth-first and iterative: each frame holds the handle its entries are
    /// unlinked through, and is removed from its parent's handle once empty.
    fn remove_tree(&self, parent: &File, root: File) -> crate::Result<()> {
        let mut stack = vec![Frame::open(root, self.path.clone())?];
        while let Some(frame) = stack.last_mut() {
            if let Some(child) = frame.subdirectories.pop() {
                let path = frame.path.join(&child);
                let handle = self.pin_directory(&frame.handle, &child, &path)?;
                stack.push(Frame::open(handle, path)?);
                continue;
            }
            let path = std::mem::take(&mut frame.path);
            stack.pop();
            let holder = stack.last().map_or(parent, |frame| &frame.handle);
            let name = path
                .file_name()
                .ok_or_else(|| invalid("build output entry has no name"))?;
            fs::remove_dir(descriptor_path(holder).join(name))
                .map_err(|error| io_error(PRUNE_FAILED, &path, error))?;
        }
        Ok(())
    }

    fn pin_directory(&self, holder: &File, name: &OsStr, path: &Path) -> crate::Result<File> {
        let handle = pin(&descriptor_path(holder).join(name))?;
        let metadata = handle
            .metadata()
            .map_err(|error| io_error("inspect pinned build output", path, error))?;
        if !metadata.is_dir() {
            return Err(invalid(format!(
                "build output {} changed during GC",
                path.display()
            )));
        }
        if metadata.dev() != self.device {
            return Err(invalid("workspace build output contains a mount boundary"));
        }
        Ok(handle)
    }
}

const PRUNE_FAILED: &str = "prune build output (it may be partly removed)";

/// A pinned directory being emptied: its files are already unlinked.
struct Frame {
    handle: File,
    path: PathBuf,
    subdirectories: Vec<OsString>,
}

impl Frame {
    fn open(handle: File, path: PathBuf) -> crate::Result<Self> {
        let directory = descriptor_path(&handle);
        let mut subdirectories = Vec::new();
        for entry in
            fs::read_dir(&directory).map_err(|error| io_error(PRUNE_FAILED, &path, error))?
        {
            let entry = entry.map_err(|error| io_error(PRUNE_FAILED, &path, error))?;
            let kind = entry
                .file_type()
                .map_err(|error| io_error(PRUNE_FAILED, &path, error))?;
            if kind.is_dir() {
                subdirectories.push(entry.file_name());
                continue;
            }
            fs::remove_file(directory.join(entry.file_name()))
                .map_err(|error| io_error(PRUNE_FAILED, &path.join(entry.file_name()), error))?;
        }
        Ok(Self {
            handle,
            path,
            subdirectories,
        })
    }
}

/// One porcelain v1 `-z` status record: its two status letters and path.
pub(super) struct StatusRecord<'a> {
    pub code: [u8; 2],
    pub path: &'a [u8],
}

pub(super) fn status_records(output: &[u8]) -> crate::Result<Vec<StatusRecord<'_>>> {
    let mut records = Vec::new();
    let mut fields = output.split(|byte| *byte == 0).filter(|f| !f.is_empty());
    while let Some(field) = fields.next() {
        let [x, y, b' ', path @ ..] = field else {
            return Err(invalid("invalid GC workspace status record"));
        };
        if path.is_empty() {
            return Err(invalid("GC workspace status record has no path"));
        }
        // A rename or copy carries its origin as the next field.
        if [*x, *y].iter().any(|code| matches!(code, b'R' | b'C')) && fields.next().is_none() {
            return Err(invalid("GC workspace status rename has no origin"));
        }
        records.push(StatusRecord {
            code: [*x, *y],
            path,
        });
    }
    Ok(records)
}

/// Directories Git lists as ignored in their entirety, relative to the
/// worktree. A directory holding a tracked file is listed by its parts.
pub(super) fn ignored_directories(output: &[u8]) -> crate::Result<BTreeSet<PathBuf>> {
    let mut directories = BTreeSet::new();
    for record in status_records(output)? {
        if record.code != *b"!!" {
            continue;
        }
        if let Some(directory) = record.path.strip_suffix(b"/") {
            directories.insert(PathBuf::from(OsStr::from_bytes(directory)));
        }
    }
    Ok(directories)
}
