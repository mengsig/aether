//! Filesystem evidence bracketing the expensive Git preservation probes.

use super::*;

#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    mode: u32,
    kind: fs::FileType,
    links: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl Stamp {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            kind: metadata.file_type(),
            links: metadata.nlink(),
            length: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

pub(super) struct Snapshot {
    directory: PathBuf,
    manifest: WorkspaceManifest,
    entries: BTreeMap<PathBuf, Stamp>,
    /// Build output directories: their own stamp is kept, their contents not.
    skipped: BTreeSet<PathBuf>,
    /// The source checkout was missing at capture and must stay missing.
    source_gone: bool,
}

impl Snapshot {
    pub fn capture(
        directory: &Path,
        manifest: &WorkspaceManifest,
        skipped: &BTreeSet<PathBuf>,
        source_gone: bool,
    ) -> crate::Result<Self> {
        if manifest.workspace != directory.join(WORKSPACE) || !manifest.source.is_absolute() {
            return Err(invalid("workspace manifest names a different GC candidate"));
        }
        debug_assert!(
            skipped
                .iter()
                .all(|path| path.starts_with(&manifest.workspace)),
            "only workspace build output is skipped"
        );
        let mut snapshot = Self {
            directory: directory.to_path_buf(),
            manifest: manifest.clone(),
            entries: BTreeMap::new(),
            skipped: skipped.clone(),
            source_gone,
        };
        snapshot.walk(directory, 0, false)?;
        let workspace = snapshot.git_directory(&manifest.workspace)?;
        let common = snapshot.common_directory(&workspace)?;
        // Without its checkout, the source's Git state is the common directory.
        let source = if source_gone {
            common.clone()
        } else {
            snapshot.record(&manifest.source)?;
            let source = snapshot.git_directory(&manifest.source)?;
            if common != snapshot.common_directory(&source)? {
                return Err(invalid("workspace belongs to a different Git repository"));
            }
            source
        };
        snapshot.walk(&common, 0, true)?;
        for git_directory in [&source, &workspace] {
            if git_directory != &common {
                snapshot.walk(git_directory, 0, true)?;
            }
        }
        snapshot.check_reference_targets(&[&common, &source, &workspace])?;
        // Alternate object databases can change the meaning/availability of
        // the saved history without changing this repository's refs.
        let objects = common.join("objects");
        snapshot.record_directory(&objects)?;
        let info = objects.join("info");
        if info
            .try_exists()
            .map_err(|error| io_error("inspect Git object info", &info, error))?
        {
            snapshot.walk(&info, 0, true)?;
            if info
                .join("alternates")
                .try_exists()
                .map_err(|error| io_error("inspect Git alternates", &info, error))?
            {
                return Err(invalid(
                    "workspace GC does not support alternate Git object databases",
                ));
            }
        }
        snapshot.recheck(manifest)?;
        Ok(snapshot)
    }

    pub fn recheck(&self, manifest: &WorkspaceManifest) -> crate::Result<()> {
        if manifest != &self.manifest {
            return Err(invalid("workspace manifest differs from the GC snapshot"));
        }
        if self.source_gone && !source_missing(&manifest.source)? {
            return Err(invalid("workspace source reappeared during GC"));
        }
        let external = self
            .entries
            .iter()
            .filter(|(path, _)| !path.starts_with(&self.directory));
        let files = self
            .entries
            .iter()
            .filter(|(path, stamp)| path.starts_with(&self.directory) && !stamp.kind.is_dir());
        // Check candidate directories last, children before parents, so new
        // ignored files and journals are checked closest to Git removal.
        let directories = self
            .entries
            .iter()
            .rev()
            .filter(|(path, stamp)| path.starts_with(&self.directory) && stamp.kind.is_dir());
        for (path, expected) in external.chain(files).chain(directories) {
            let current = inspect_metadata(path)?;
            if Stamp::from_metadata(&current) != *expected {
                return Err(invalid(format!(
                    "workspace or Git metadata changed during GC: {}",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    fn check_reference_targets(&self, directories: &[&PathBuf]) -> crate::Result<()> {
        for (path, stamp) in &self.entries {
            if !stamp.kind.is_file()
                || !directories.iter().any(|directory| {
                    path == &directory.join("HEAD") || path.starts_with(directory.join("refs"))
                })
            {
                continue;
            }
            let reference = read_pointer(path)?;
            if let Some(target) = reference.strip_prefix("ref:")
                && !target.trim().starts_with("refs/")
            {
                // Git also accepts cross-worktree symbolic refs. Their
                // targets live outside the captured administration dirs.
                return Err(invalid(
                    "workspace GC does not support cross-worktree symbolic refs",
                ));
            }
        }
        Ok(())
    }

    fn record(&mut self, path: &Path) -> crate::Result<fs::Metadata> {
        let metadata = inspect_metadata(path)?;
        let stamp = Stamp::from_metadata(&metadata);
        if let Some(previous) = self.entries.get(path) {
            if previous != &stamp {
                return Err(invalid("workspace changed during GC snapshot"));
            }
        } else {
            if self.entries.len() >= MAX_ENTRIES {
                return Err(invalid("workspace GC snapshot entry limit reached"));
            }
            self.entries.insert(path.to_path_buf(), stamp);
        }
        Ok(metadata)
    }

    fn record_directory(&mut self, path: &Path) -> crate::Result<()> {
        if !self.record(path)?.is_dir() {
            return Err(invalid("workspace GC requires real Git directories"));
        }
        Ok(())
    }

    fn walk(&mut self, path: &Path, depth: usize, git_metadata: bool) -> crate::Result<()> {
        if depth > MAX_DEPTH {
            return Err(invalid("workspace GC snapshot depth limit reached"));
        }
        let metadata = self.record(path)?;
        if metadata.file_type().is_symlink() && !git_metadata {
            return Ok(());
        }
        if metadata.is_file() || (metadata.is_dir() && self.skipped.contains(path)) {
            return Ok(());
        }
        if !metadata.is_dir() {
            return Err(invalid(
                "workspace GC cannot snapshot special or symlinked Git metadata",
            ));
        }
        let handle = pin(path)?;
        let pinned = handle
            .metadata()
            .map_err(|error| io_error("inspect GC snapshot directory", path, error))?;
        if Stamp::from_metadata(&pinned) != Stamp::from_metadata(&metadata) {
            return Err(invalid("workspace changed during GC snapshot"));
        }
        for entry in fs::read_dir(descriptor_path(&handle))
            .map_err(|error| io_error("read GC snapshot directory", path, error))?
        {
            let entry = entry.map_err(|error| io_error("read GC snapshot entry", path, error))?;
            let name = entry.file_name();
            if git_metadata && depth == 0 {
                if name == "reftable" {
                    return Err(invalid("workspace GC requires file-backed Git refs"));
                }
                // Objects are immutable Git data; reflogs and hooks do not
                // establish reachability. Visit only the two relevant
                // worktree administration directories separately.
                if ["objects", "logs", "hooks", "worktrees"]
                    .iter()
                    .any(|skip| name == *skip)
                {
                    continue;
                }
            }
            let child = path.join(name);
            let child_metadata = inspect_metadata(&child)?;
            if child_metadata.dev() != metadata.dev() {
                return Err(invalid("workspace GC snapshot contains a mount boundary"));
            }
            self.walk(&child, depth + 1, git_metadata)?;
        }
        self.record(path)?;
        Ok(())
    }

    fn git_directory(&mut self, workspace: &Path) -> crate::Result<PathBuf> {
        let dot_git = workspace.join(".git");
        let metadata = self.record(&dot_git)?;
        if metadata.is_dir() {
            return resolve_directory(&dot_git);
        }
        let content = read_pointer(&dot_git)?;
        let directory = content
            .strip_prefix("gitdir: ")
            .ok_or_else(|| invalid("workspace GC requires a standard Git directory pointer"))?;
        if Path::new(directory)
            .components()
            .any(|part| part == Component::ParentDir)
        {
            return Err(invalid(
                "workspace GC does not support relative parent Git directory pointers",
            ));
        }
        resolve_directory(&workspace.join(directory))
    }

    fn common_directory(&mut self, git_directory: &Path) -> crate::Result<PathBuf> {
        self.record_directory(git_directory)?;
        let pointer = git_directory.join("commondir");
        match fs::symlink_metadata(&pointer) {
            Ok(_) => {
                self.record(&pointer)?;
                let target = read_pointer(&pointer)?;
                // Only the standard linked-worktree parent traversal is
                // supported; arbitrary traversals can hide symlink aliases
                // which disappear when the path is normalized.
                if target != "../.."
                    && Path::new(&target)
                        .components()
                        .any(|part| part == Component::ParentDir)
                {
                    return Err(invalid(
                        "workspace GC requires a standard Git common directory pointer",
                    ));
                }
                resolve_directory(&git_directory.join(target))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Ok(git_directory.to_path_buf())
            }
            Err(error) => Err(io_error("inspect Git common directory", &pointer, error)),
        }
    }
}

fn inspect_metadata(path: &Path) -> crate::Result<fs::Metadata> {
    fs::symlink_metadata(path).map_err(|error| io_error("inspect GC snapshot entry", path, error))
}

fn read_pointer(path: &Path) -> crate::Result<String> {
    let metadata = inspect_metadata(path)?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return Err(invalid("unsafe or oversized Git directory pointer"));
    }
    let mut bytes = Vec::new();
    OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)
        .and_then(|file| file.take(4097).read_to_end(&mut bytes))
        .map_err(|error| io_error("read Git directory pointer", path, error))?;
    if bytes.len() > 4096 {
        return Err(invalid("oversized Git directory pointer"));
    }
    let content =
        String::from_utf8(bytes).map_err(|_| invalid("non-UTF-8 Git directory pointer"))?;
    let content = content.strip_suffix('\n').unwrap_or(&content);
    if content.is_empty() || content.contains(['\n', '\r', '\0']) {
        return Err(invalid("unsupported Git directory pointer"));
    }
    Ok(content.to_owned())
}

fn resolve_directory(path: &Path) -> crate::Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    let resolved = fs::canonicalize(path)
        .map_err(|error| io_error("resolve GC Git directory", path, error))?;
    if resolved != normalized || !inspect_metadata(&resolved)?.is_dir() {
        return Err(invalid(
            "workspace GC does not support symlinked Git directories",
        ));
    }
    Ok(resolved)
}

/// A missing source is not read: the worktree's own view includes the
/// repository's common configuration.
pub(super) fn check_configuration(
    manifest: &WorkspaceManifest,
    source_gone: bool,
) -> crate::Result<()> {
    let source = (!source_gone).then_some(&manifest.source);
    for workspace in source.into_iter().chain([&manifest.workspace]) {
        let config = run(
            git(workspace).args(["config", "--no-includes", "--null", "--list"]),
            "check GC Git configuration",
        )?;
        for entry in config
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
        {
            let key_end = entry
                .iter()
                .position(|byte| *byte == b'\n')
                .unwrap_or(entry.len());
            let key = entry[..key_end].to_ascii_lowercase();
            if key == b"include.path" || key.starts_with(b"includeif.") {
                return Err(invalid(
                    "workspace GC does not support Git configuration includes",
                ));
            }
            if key == b"extensions.refstorage" && entry.get(key_end + 1..) != Some(b"files") {
                return Err(invalid("workspace GC requires file-backed Git refs"));
            }
        }
    }
    Ok(())
}
