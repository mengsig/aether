//! GC has stricter preservation checks than failed-delegation rollback.

use super::*;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::time::SystemTime;

const MAX_ENTRIES: usize = 16_384;
const MAX_DEPTH: usize = 64;

fn run(command: &mut Command, description: &str) -> crate::Result<Vec<u8>> {
    successful(
        run_reader_timeout(
            command,
            None,
            256 * 1024,
            Duration::from_secs(2),
            description,
        )?,
        description,
    )
}

fn git_string(command: &mut Command, description: &str) -> crate::Result<String> {
    output_string(run(command, description)?, description)
}

fn git_input(command: &mut Command, input: String, description: &str) -> crate::Result<String> {
    output_string(
        successful(
            run_reader_timeout(
                command,
                Some(Box::new(io::Cursor::new(input.into_bytes()))),
                256 * 1024,
                Duration::from_secs(2),
                description,
            )?,
            description,
        )?,
        description,
    )
}

pub(crate) struct Candidate {
    pub directory: PathBuf,
    pub bytes: u64,
    pub bytes_complete: bool,
    pub modified: SystemTime,
    permissions: Vec<(File, fs::Permissions)>,
    git_state: Option<GitState>,
}

#[derive(PartialEq, Eq)]
struct GitState {
    references: String,
    source_head: String,
    head: String,
}

impl Candidate {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            bytes: 0,
            bytes_complete: false,
            modified: SystemTime::UNIX_EPOCH,
            permissions: vec![],
            git_state: None,
        }
    }

    /// Walk through pinned directory handles. Permission repair must never
    /// chmod a symlink target or a file shared through another hard link.
    pub fn measure(&mut self, dry_run: bool) -> crate::Result<()> {
        let path = self.directory.clone();
        let device = fs::symlink_metadata(&path)
            .map_err(|error| io_error("inspect GC candidate", &path, error))?
            .dev();
        let mut remaining = MAX_ENTRIES;
        self.walk(&path, device, 0, &mut remaining, dry_run)?;
        self.bytes_complete = true;
        Ok(())
    }

    fn walk(
        &mut self,
        path: &Path,
        device: u64,
        depth: usize,
        remaining: &mut usize,
        dry_run: bool,
    ) -> crate::Result<()> {
        if *remaining == 0 || depth > MAX_DEPTH {
            return Err(invalid("workspace GC entry/depth limit reached"));
        }
        *remaining -= 1;
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| io_error("inspect GC entry", path, error))?;
        if metadata.dev() != device {
            return Err(invalid("workspace contains a mount boundary"));
        }
        self.modified = self.modified.max(
            metadata
                .modified()
                .map_err(|error| io_error("read GC entry age", path, error))?,
        );
        if metadata.file_type().is_symlink() {
            self.bytes = self.bytes.saturating_add(metadata.len());
            return Ok(());
        }
        if !metadata.is_dir() && !metadata.is_file() {
            return Err(invalid("workspace contains a special file"));
        }
        let handle = pin(path)?;
        let pinned = handle
            .metadata()
            .map_err(|error| io_error("inspect pinned GC entry", path, error))?;
        if pinned.dev() != metadata.dev() || pinned.ino() != metadata.ino() {
            return Err(invalid("workspace changed during GC inspection"));
        }
        let needed = if metadata.is_dir() { 0o700 } else { 0o600 };
        let permissions = metadata.permissions();
        if permissions.mode() & needed != needed {
            if dry_run {
                return Err(invalid(
                    "workspace needs permission repair; dry run kept it",
                ));
            }
            if metadata.is_file() && metadata.nlink() != 1 {
                return Err(invalid(
                    "workspace permission repair would affect a hard link",
                ));
            }
            let restore_handle = handle
                .try_clone()
                .map_err(|error| io_error("retain GC permission handle", path, error))?;
            set_pinned_permissions(
                &restore_handle,
                fs::Permissions::from_mode(permissions.mode() | needed),
            )?;
            self.permissions.push((restore_handle, permissions));
        }
        if metadata.is_file() {
            self.bytes = self.bytes.saturating_add(metadata.len());
            return Ok(());
        }
        let directory = descriptor_path(&handle);
        for entry in
            fs::read_dir(&directory).map_err(|error| io_error("read GC directory", path, error))?
        {
            let entry = entry.map_err(|error| io_error("read GC directory entry", path, error))?;
            self.walk(&entry.path(), device, depth + 1, remaining, dry_run)?;
        }
        Ok(())
    }

    pub fn restore_permissions(&mut self) -> crate::Result<()> {
        while let Some((handle, permissions)) = self.permissions.last() {
            set_pinned_permissions(handle, permissions.clone())?;
            self.permissions.pop();
        }
        Ok(())
    }

    pub fn inspect(&mut self, root: &Path) -> crate::Result<WorkspaceManifest> {
        self.check_metadata()?;
        let manifest = load_timeout(
            root,
            &self.directory.join(WORKSPACE),
            Duration::from_secs(2),
        )?;
        if manifest.workspace != self.directory.join(WORKSPACE) {
            return Err(invalid("workspace manifest names a different GC candidate"));
        }
        verify_saved_files(&manifest.workspace)?;
        check_status(&manifest.workspace)?;
        let state = git_state(&manifest)?;
        check_commits(&manifest, &self.directory, &state)?;
        self.git_state = Some(state);
        Ok(manifest)
    }

    fn check_metadata(&self) -> crate::Result<()> {
        for entry in fs::read_dir(&self.directory)
            .map_err(|error| io_error("read workspace metadata", &self.directory, error))?
        {
            let entry = entry.map_err(|error| {
                io_error("read workspace metadata entry", &self.directory, error)
            })?;
            if entry.file_name() != OsStr::new(MANIFEST)
                && entry.file_name() != OsStr::new(WORKSPACE)
            {
                return Err(invalid(
                    "workspace has an integration journal or extra metadata",
                ));
            }
        }
        let metadata = fs::symlink_metadata(self.directory.join(MANIFEST))
            .map_err(|error| io_error("inspect GC workspace manifest", &self.directory, error))?;
        if !metadata.is_file() || metadata.len() > 64 * 1024 {
            return Err(invalid(
                "workspace manifest is unsafe or exceeds the GC size limit",
            ));
        }
        let workspace = self.directory.join(WORKSPACE);
        if !fs::symlink_metadata(&workspace)
            .map_err(|error| io_error("inspect GC worktree", &workspace, error))?
            .is_dir()
        {
            return Err(invalid("workspace GC worktree must be a real directory"));
        }
        Ok(())
    }

    pub fn recheck(&self, manifest: &WorkspaceManifest) -> crate::Result<()> {
        self.check_metadata()?;
        let current: WorkspaceManifest = read_json(&self.directory.join(MANIFEST))?;
        if &current != manifest || self.git_state.as_ref() != Some(&git_state(manifest)?) {
            return Err(invalid(
                "workspace manifest, HEAD, or source refs changed during GC",
            ));
        }
        check_status(&manifest.workspace)
    }

    pub fn remove(&mut self, manifest: &WorkspaceManifest) -> crate::Result<()> {
        // Git performs its own final dirty-worktree check. Never use --force:
        // an edit made after our inspection must still prevent removal.
        successful(
            run_reader_timeout(
                git(&manifest.source)
                    .args(["worktree", "remove", "--"])
                    .arg(&manifest.workspace),
                None,
                256 * 1024,
                GIT_TIMEOUT,
                "remove collected workspace",
            )?,
            "remove collected workspace",
        )?;
        // Retain snapshot/integration refs: deleting them could destroy the
        // only copy of an older baseline or an integration recovery target.
        fs::remove_file(self.directory.join(MANIFEST)).map_err(|error| {
            io_error(
                "remove collected workspace manifest",
                &self.directory,
                error,
            )
        })?;
        fs::remove_dir(&self.directory).map_err(|error| {
            io_error(
                "remove collected workspace directory",
                &self.directory,
                error,
            )
        })?;
        self.permissions.clear();
        Ok(())
    }
}

impl Drop for Candidate {
    fn drop(&mut self) {
        if let Err(error) = self.restore_permissions() {
            tracing::error!(%error, "cannot restore workspace permissions after GC");
        }
    }
}

fn descriptor_path(handle: &File) -> PathBuf {
    #[cfg(target_os = "linux")]
    let root = "/proc/self/fd";
    #[cfg(not(target_os = "linux"))]
    let root = "/dev/fd";
    Path::new(root).join(handle.as_raw_fd().to_string())
}

fn pin(path: &Path) -> crate::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    options.custom_flags(nix::libc::O_PATH | nix::libc::O_NOFOLLOW);
    #[cfg(not(target_os = "linux"))]
    options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    options
        .open(path)
        .map_err(|error| io_error("pin workspace GC entry", path, error))
}

fn set_pinned_permissions(handle: &File, permissions: fs::Permissions) -> crate::Result<()> {
    // O_PATH handles cannot use fchmod; the descriptor link still names the
    // pinned inode if its original path is replaced with a symlink.
    fs::set_permissions(descriptor_path(handle), permissions)
        .map_err(|error| invalid(format!("set pinned workspace permissions: {error}")))
}

fn check_status(workspace: &Path) -> crate::Result<()> {
    // Git's final worktree removal check honors these flags too, so saved
    // bytes at inspection time cannot make their later deletion safe.
    let entries = run(
        git(workspace).args(["ls-files", "-v", "-z"]),
        "check GC workspace index flags",
    )?;
    if entries.split(|byte| *byte == 0).any(|entry| {
        entry
            .first()
            .is_some_and(|tag| tag.is_ascii_lowercase() || *tag == b'S')
    }) {
        return Err(invalid(
            "workspace index has assume-unchanged or skip-worktree entries",
        ));
    }
    let status = run(
        git(workspace).args([
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignored=matching",
            "--ignore-submodules=none",
        ]),
        "check GC workspace status",
    )?;
    if !status.is_empty() {
        return Err(invalid("workspace has dirty, untracked, or ignored files"));
    }
    Ok(())
}

fn git_state(manifest: &WorkspaceManifest) -> crate::Result<GitState> {
    let symbolic = run_reader_timeout(
        git(&manifest.workspace).args(["symbolic-ref", "-q", "HEAD"]),
        None,
        4096,
        Duration::from_secs(2),
        "check GC detached HEAD",
    )?;
    if symbolic.status.success() {
        return Err(invalid(
            "workspace HEAD is attached to a branch; keeping its commits",
        ));
    }
    if symbolic.status.code() != Some(1) {
        successful(symbolic, "check GC detached HEAD")?;
    }
    Ok(GitState {
        references: git_string(
            git(&manifest.source).args(["for-each-ref", "--format=%(refname) %(objectname)"]),
            "list GC source refs",
        )?,
        source_head: git_string(
            git(&manifest.source).args(["rev-parse", "--verify", "HEAD^{commit}"]),
            "read GC source HEAD",
        )?,
        head: git_string(
            git(&manifest.workspace).args(["rev-parse", "HEAD"]),
            "read GC workspace HEAD",
        )?,
    })
}

fn check_commits(
    manifest: &WorkspaceManifest,
    directory: &Path,
    state: &GitState,
) -> crate::Result<()> {
    let id = directory
        .file_name()
        .ok_or_else(|| invalid("workspace has no identifier"))?
        .to_string_lossy();
    let snapshot_ref = format!("refs/aether/workspaces/{id}");
    let integration_ref = format!("refs/aether/integrations/{id}");
    // --all also includes every worktree HEAD, including the candidate's
    // own detached HEAD. Enumerate actual refs instead, plus source HEAD.
    let mut saved = BTreeSet::new();
    for reference in state.references.lines() {
        let (name, object) = reference
            .split_once(' ')
            .ok_or_else(|| invalid("invalid GC source ref record"))?;
        if name != snapshot_ref && name != integration_ref {
            oid(object)?;
            saved.insert(object.to_owned());
        }
    }
    saved.insert(state.source_head.clone());
    let input = format!(
        "{}\n{}",
        state.head,
        saved
            .iter()
            .map(|object| format!("^{object}\n"))
            .collect::<String>()
    );
    let missing = git_input(
        git(&manifest.source).args(["rev-list", "--max-count=2", "--stdin"]),
        input,
        "check GC unique commits",
    )?;
    if missing.is_empty() {
        return Ok(());
    }
    let snapshot = git_string(
        git(&manifest.source).args(["rev-parse", "--verify", &snapshot_ref]),
        "read GC snapshot commit",
    )?;
    if missing != snapshot {
        return Err(invalid(
            "workspace has commits missing from other source refs",
        ));
    }
    let snapshot_tree = git_string(
        git(&manifest.source).args(["show", "-s", "--format=%T", &snapshot]),
        "read GC snapshot tree",
    )?;
    if snapshot_tree != manifest.baseline_tree {
        return Err(invalid("workspace snapshot ref differs from its baseline"));
    }
    // The synthetic snapshot commit is expected to be private. Its contents
    // must nevertheless be saved elsewhere before that worktree can go.
    if saved.is_empty() {
        return Err(invalid("workspace baseline has no other source ref"));
    }
    let saved_trees = git_input(
        git(&manifest.source).args(["log", "--stdin", "--max-count=1024", "--format=%T"]),
        saved.iter().map(|object| format!("{object}\n")).collect(),
        "check GC saved baseline",
    )?;
    if !saved_trees.lines().any(|tree| tree == snapshot_tree) {
        return Err(invalid(
            "workspace baseline is not saved in another source ref",
        ));
    }
    Ok(())
}

fn verify_saved_files(workspace: &Path) -> crate::Result<()> {
    let head = git_string(
        git(workspace).args(["rev-parse", "HEAD"]),
        "read GC saved HEAD",
    )?;
    let entries = tree_entries_timeout(workspace, &head, Duration::from_secs(2))?;
    let input: Vec<u8> = entries
        .values()
        .flat_map(|entry| entry.oid.bytes().chain(std::iter::once(b'\n')))
        .collect();
    let output = successful(
        run_reader_timeout(
            git(workspace).args(["cat-file", "--batch"]),
            Some(Box::new(io::Cursor::new(input))),
            64 * 1024 * 1024,
            Duration::from_secs(2),
            "read GC saved file contents",
        )?,
        "read GC saved file contents",
    )?;
    let mut remaining = output.as_slice();
    for (relative, entry) in entries {
        let end = remaining
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(|| invalid("invalid GC Git object header"))?;
        let header = std::str::from_utf8(&remaining[..end])
            .map_err(|_| invalid("invalid GC Git object header encoding"))?;
        let size: usize = header
            .strip_prefix(&format!("{} blob ", entry.oid))
            .ok_or_else(|| invalid("unexpected GC Git object"))?
            .parse()
            .map_err(|_| invalid("invalid GC Git object length"))?;
        remaining = &remaining[end + 1..];
        let content = remaining
            .get(..size)
            .ok_or_else(|| invalid("truncated GC Git object"))?;
        if remaining.get(size) != Some(&b'\n') {
            return Err(invalid("invalid GC Git object terminator"));
        }
        reject_symlink_ancestors(workspace, &relative)?;
        let path = workspace.join(relative);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| io_error("inspect saved GC file", &path, error))?;
        let matches = if entry.mode == 0o120000 {
            metadata.file_type().is_symlink()
                && fs::read_link(&path)
                    .map_err(|error| io_error("read saved GC symlink", &path, error))?
                    .as_os_str()
                    .as_bytes()
                    == content
        } else if metadata.is_file() {
            let executable = metadata.permissions().mode() & 0o111 != 0;
            let mut file = OpenOptions::new()
                .read(true)
                .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
                .open(&path)
                .map_err(|error| io_error("read saved GC file", &path, error))?;
            executable == (entry.mode == 0o100755) && file_matches(&mut file, content)?
        } else {
            false
        };
        if !matches {
            return Err(invalid(format!(
                "workspace has unsaved file bytes at {}",
                path.display()
            )));
        }
        remaining = &remaining[size + 1..];
    }
    if !remaining.is_empty() {
        return Err(invalid("unexpected trailing GC Git object output"));
    }
    Ok(())
}

fn file_matches(file: &mut File, mut content: &[u8]) -> crate::Result<bool> {
    let mut buffer = [0_u8; 8192];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| invalid(format!("read GC workspace contents: {error}")))?;
        if count == 0 {
            return Ok(content.is_empty());
        }
        if content.get(..count) != Some(&buffer[..count]) {
            return Ok(false);
        }
        content = &content[count..];
    }
}

pub(crate) fn integration_lock(root: &Path, dry_run: bool) -> crate::Result<Option<File>> {
    let path = root.join(".integration.lock");
    if dry_run
        && !path
            .try_exists()
            .map_err(|error| io_error("inspect workspace integration lock", &path, error))?
    {
        return Ok(None);
    }
    let lock = OpenOptions::new()
        .write(true)
        .create(!dry_run)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(&path)
        .map_err(|error| io_error("open workspace GC integration lock", &path, error))?;
    let started = Instant::now();
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(fs::TryLockError::WouldBlock) if started.elapsed() < Duration::from_millis(100) => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return Err(invalid(format!("workspace integration is busy: {error}"))),
        }
    }
    Ok(Some(lock))
}
