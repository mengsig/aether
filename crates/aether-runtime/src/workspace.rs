//! Private Git worktrees for child agents and conservative parent integration.
//!
//! Snapshotting deliberately builds a separate index from raw file bytes. Git
//! never writes the source index, and repository clean/smudge filters do not
//! change what the child initially sees.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::ExitStatus;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use aether_core::id::AgentId;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};

use crate::RuntimeError;

pub(crate) mod gc;
pub(crate) mod review;

const MANIFEST: &str = "manifest.json";
const JOURNAL: &str = "integration.json";
const WORKSPACE: &str = "workspace";
const GIT_TIMEOUT: Duration = Duration::from_secs(60);
const GIT_STDOUT_LIMIT: usize = 64 * 1024 * 1024;
const GIT_STDERR_LIMIT: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkspaceManifest {
    pub parent: AgentId,
    pub source: PathBuf,
    pub workspace: PathBuf,
    pub baseline_tree: String,
    pub integrated_tree: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IntegrationReport {
    pub changed_paths: Vec<PathBuf>,
    pub already_integrated: bool,
}

/// What an integration did: landed, or refused before writing anything
/// because the source changed paths the child also changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IntegrationOutcome {
    Landed(IntegrationReport),
    Conflicts(Vec<PathBuf>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    mode: u32,
    oid: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Journal {
    baseline_tree: String,
    final_tree: String,
}

struct TemporaryIndex(PathBuf);

impl Drop for TemporaryIndex {
    fn drop(&mut self) {
        for path in [&self.0, &self.0.with_extension("lock")] {
            if let Err(error) = fs::remove_file(path)
                && error.kind() != io::ErrorKind::NotFound
            {
                tracing::warn!(path = %path.display(), %error, "cannot remove temporary Git index");
            }
        }
    }
}

fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Invalid(message.into())
}

fn io_error(action: &str, path: &Path, error: io::Error) -> RuntimeError {
    invalid(format!("{action} {}: {error}", path.display()))
}

fn git(repo: &Path) -> Command {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .arg("-C")
        .arg(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-c")
        .arg("core.autocrlf=false")
        .process_group(0);
    command
}

fn git_with_ignore_config(repo: &Path) -> Command {
    let mut command = git(repo);
    // Untracked discovery must honor the user's global/system excludes. This
    // command only lists paths; hooks and fsmonitor stay disabled by `git`.
    for key in [
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_SYSTEM",
        "GIT_CONFIG_NOSYSTEM",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        } else {
            command.env_remove(key);
        }
    }
    command
}

fn read_limited(
    mut stream: impl Read,
    limit: usize,
    exceeded: Arc<AtomicBool>,
) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Ok(output);
        }
        let remaining = limit.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
        if count > remaining {
            exceeded.store(true, Ordering::Relaxed);
        }
    }
}

fn kill_git_group(child: &mut std::process::Child) {
    if let Err(error) = killpg(Pid::from_raw(child.id() as i32), Signal::SIGKILL)
        && error != nix::errno::Errno::ESRCH
    {
        tracing::warn!(%error, "cannot stop Git process group");
    }
    let _ = child.kill();
}

struct GitOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_reader(
    command: &mut Command,
    input: Option<Box<dyn Read + Send>>,
    limit: usize,
    description: &str,
) -> crate::Result<GitOutput> {
    run_reader_timeout(command, input, limit, GIT_TIMEOUT, description)
}

fn run_reader_timeout(
    command: &mut Command,
    input: Option<Box<dyn Read + Send>>,
    limit: usize,
    timeout: Duration,
    description: &str,
) -> crate::Result<GitOutput> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| invalid(format!("cannot start git {description}: {error}")))?;
    let writer_input = match input {
        Some(input) => match child.stdin.take() {
            Some(stdin) => Some((input, stdin)),
            None => {
                kill_git_group(&mut child);
                let _ = child.wait();
                return Err(invalid("git stdin unavailable"));
            }
        },
        None => None,
    };
    let exceeded = Arc::new(AtomicBool::new(false));
    let Some(stdout) = child.stdout.take() else {
        kill_git_group(&mut child);
        let _ = child.wait();
        return Err(invalid("git stdout unavailable"));
    };
    let Some(stderr) = child.stderr.take() else {
        kill_git_group(&mut child);
        let _ = child.wait();
        return Err(invalid("git stderr unavailable"));
    };
    let out_exceeded = Arc::clone(&exceeded);
    let err_exceeded = Arc::clone(&exceeded);
    let out_reader = thread::spawn(move || read_limited(stdout, limit, out_exceeded));
    let err_reader = thread::spawn(move || read_limited(stderr, GIT_STDERR_LIMIT, err_exceeded));
    let writer = writer_input
        .map(|(mut input, mut stdin)| thread::spawn(move || io::copy(&mut input, &mut stdin)));
    let started = Instant::now();
    let mut status = None;
    let stopped = loop {
        if exceeded.load(Ordering::Relaxed) || started.elapsed() >= timeout {
            kill_git_group(&mut child);
            if status.is_none() {
                status =
                    Some(child.wait().map_err(|error| {
                        invalid(format!("cannot reap git {description}: {error}"))
                    })?);
            }
            break true;
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(exit)) => status = Some(exit),
                Ok(None) => {}
                Err(error) => {
                    kill_git_group(&mut child);
                    let _ = child.wait();
                    return Err(invalid(format!(
                        "cannot wait for git {description}: {error}"
                    )));
                }
            }
        }
        if status.is_some()
            && out_reader.is_finished()
            && err_reader.is_finished()
            && writer.as_ref().is_none_or(thread::JoinHandle::is_finished)
        {
            break false;
        }
        thread::sleep(Duration::from_millis(1));
    };
    let output = out_reader
        .join()
        .map_err(|_| invalid("Git stdout reader panicked"))?
        .map_err(|error| invalid(format!("cannot read git {description} output: {error}")))?;
    let stderr = err_reader
        .join()
        .map_err(|_| invalid("Git stderr reader panicked"))?
        .map_err(|error| invalid(format!("cannot read git {description} error: {error}")))?;
    let write_result = writer
        .map(|writer| {
            writer
                .join()
                .map_err(|_| invalid("Git input writer panicked"))
        })
        .transpose()?
        .map(|result| result.map(|_| ()))
        .transpose();
    if stopped {
        let reason = if exceeded.load(Ordering::Relaxed) {
            "output limit"
        } else {
            "60 second timeout"
        };
        return Err(invalid(format!("git {description} exceeded {reason}")));
    }
    let status = status.ok_or_else(|| invalid("Git process exit status unavailable"))?;
    if status.success() {
        write_result
            .map_err(|error| invalid(format!("cannot send git {description} input: {error}")))?;
    }
    Ok(GitOutput {
        status,
        stdout: output,
        stderr,
    })
}

fn successful(output: GitOutput, description: &str) -> crate::Result<Vec<u8>> {
    if !output.status.success() {
        return Err(invalid(format!(
            "git {description} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

fn run(command: &mut Command, description: &str) -> crate::Result<Vec<u8>> {
    successful(
        run_reader(command, None, GIT_STDOUT_LIMIT, description)?,
        description,
    )
}

fn output_string(output: Vec<u8>, description: &str) -> crate::Result<String> {
    String::from_utf8(output)
        .map(|value| value.trim_end_matches('\n').to_owned())
        .map_err(|_| invalid(format!("git {description} returned non-UTF-8 output")))
}

fn git_string(command: &mut Command, description: &str) -> crate::Result<String> {
    output_string(run(command, description)?, description)
}

fn git_string_timeout(
    command: &mut Command,
    timeout: Duration,
    description: &str,
) -> crate::Result<String> {
    output_string(
        successful(
            run_reader_timeout(command, None, GIT_STDOUT_LIMIT, timeout, description)?,
            description,
        )?,
        description,
    )
}

fn run_with_input(
    command: &mut Command,
    input: &[u8],
    description: &str,
) -> crate::Result<Vec<u8>> {
    successful(
        run_reader(
            command,
            Some(Box::new(io::Cursor::new(input.to_vec()))),
            GIT_STDOUT_LIMIT,
            description,
        )?,
        description,
    )
}

fn real_directory(path: &Path, create: bool) -> crate::Result<PathBuf> {
    if create {
        fs::create_dir_all(path)
            .map_err(|error| io_error("cannot create directory", path, error))?;
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| io_error("cannot inspect directory", path, error))?;
    if !metadata.file_type().is_dir() {
        return Err(invalid(format!(
            "{} must be a real directory",
            path.display()
        )));
    }
    fs::canonicalize(path).map_err(|error| io_error("cannot resolve directory", path, error))
}

fn planned_directory(path: &Path) -> crate::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| invalid(format!("cannot resolve current directory: {error}")))?
            .join(path)
    };
    let mut existing = absolute.as_path();
    while !existing.exists() {
        existing = existing
            .parent()
            .ok_or_else(|| invalid("managed workspace root has no existing ancestor"))?;
    }
    let mut planned = fs::canonicalize(existing)
        .map_err(|error| io_error("cannot resolve managed root ancestor", existing, error))?;
    for component in absolute
        .strip_prefix(existing)
        .map_err(|_| invalid("invalid managed root"))?
        .components()
    {
        let Component::Normal(name) = component else {
            return Err(invalid(
                "managed workspace root contains an unsafe path component",
            ));
        };
        planned.push(name);
    }
    Ok(planned)
}

fn source_root(source: &Path) -> crate::Result<PathBuf> {
    source_root_timeout(source, GIT_TIMEOUT)
}

fn source_root_timeout(source: &Path, timeout: Duration) -> crate::Result<PathBuf> {
    let source = real_directory(source, false)?;
    let top = git_string_timeout(
        git(&source).args(["rev-parse", "--show-toplevel"]),
        timeout,
        "repository root",
    ).map_err(|error| invalid(format!(
        "isolated child workspaces require a Git repository at {}; initialize Git there or choose shared mode: {error}",
        source.display()
    )))?;
    let top = fs::canonicalize(&top)
        .map_err(|error| io_error("cannot resolve Git repository root", Path::new(&top), error))?;
    if top != source {
        return Err(invalid(format!(
            "workspace source {} must be the Git repository root {}",
            source.display(),
            top.display()
        )));
    }
    Ok(source)
}

fn validate_relative(path: &Path) -> crate::Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(invalid(format!("invalid Git path {}", path.display())));
    }
    for component in path.components() {
        match component {
            Component::Normal(name) if name.as_bytes() != b".git" => {}
            _ => return Err(invalid(format!("unsafe Git path {}", path.display()))),
        }
    }
    Ok(())
}

fn reject_symlink_ancestors(root: &Path, path: &Path) -> crate::Result<()> {
    let mut current = root.to_path_buf();
    if let Some(parent) = path.parent() {
        for component in parent.components() {
            let Component::Normal(name) = component else {
                return Err(invalid(format!("unsafe path {}", path.display())));
            };
            current.push(name);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_dir() => {}
                Ok(_) => {
                    return Err(invalid(format!(
                        "{} is not a real directory; Git path {} cannot be traversed safely",
                        current.display(),
                        path.display()
                    )));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Err(error) => {
                    return Err(io_error("cannot inspect path ancestor", &current, error));
                }
            }
        }
    }
    Ok(())
}

fn snapshot_path_accessible(root: &Path, path: &Path) -> crate::Result<bool> {
    let mut current = root.to_path_buf();
    for component in path.parent().into_iter().flat_map(Path::components) {
        let Component::Normal(name) = component else {
            return Err(invalid("unsafe Git path"));
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(invalid(format!(
                    "{} is a symlink ancestor; refusing to read outside the repository",
                    current.display()
                )));
            }
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(io_error(
                    "cannot inspect Git path ancestor",
                    &current,
                    error,
                ));
            }
        }
    }
    Ok(true)
}

fn parse_nul_paths(output: &[u8]) -> crate::Result<Vec<PathBuf>> {
    if !output.is_empty() && !output.ends_with(&[0]) {
        return Err(invalid("incomplete Git path list"));
    }
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            let path = PathBuf::from(OsString::from_vec(path.to_vec()));
            validate_relative(&path)?;
            Ok(path)
        })
        .collect()
}

fn tracked_paths(source: &Path) -> crate::Result<BTreeSet<PathBuf>> {
    let output = run(
        git(source).args(["ls-files", "--stage", "-z"]),
        "tracked files",
    )?;
    let mut paths = BTreeSet::new();
    for record in output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let separator = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| invalid("invalid Git index record"))?;
        let (prefix, path) = (&record[..separator], &record[separator + 1..]);
        let prefix = std::str::from_utf8(prefix).map_err(|_| invalid("invalid Git index mode"))?;
        let mut fields = prefix.split_ascii_whitespace();
        let mode = fields
            .next()
            .ok_or_else(|| invalid("missing Git index mode"))?;
        let _oid = fields
            .next()
            .ok_or_else(|| invalid("missing Git index object"))?;
        let stage = fields
            .next()
            .ok_or_else(|| invalid("missing Git index stage"))?;
        if stage != "0" {
            return Err(invalid(
                "cannot create a child workspace while Git has unmerged files",
            ));
        }
        if !matches!(mode, "100644" | "100755" | "120000") {
            return Err(invalid(format!(
                "unsupported Git index mode {mode}; submodules and special files are not supported"
            )));
        }
        let path = PathBuf::from(OsString::from_vec(path.to_vec()));
        validate_relative(&path)?;
        paths.insert(path);
    }
    Ok(paths)
}

fn reject_sparse(source: &Path) -> crate::Result<()> {
    let output = run_reader(
        git(source).args(["config", "--bool", "--get", "core.sparseCheckout"]),
        None,
        256,
        "check sparse checkout",
    )?;
    if output.status.success() && output.stdout.starts_with(b"true") {
        return Err(invalid(
            "sparse Git checkouts are not supported for child workspaces",
        ));
    }
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(invalid(format!(
            "cannot check sparse checkout: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let output = run(git(source).args(["ls-files", "-v", "-z"]), "sparse entries")?;
    if output
        .split(|byte| *byte == 0)
        .any(|record| record.starts_with(b"S "))
    {
        return Err(invalid("skip-worktree Git entries are not supported"));
    }
    Ok(())
}

fn hash_path(source: &Path, path: &Path, mode: u32) -> crate::Result<String> {
    let absolute = source.join(path);
    let mut command = git(source);
    command.args(["hash-object", "-w", "--stdin"]);
    let input: Box<dyn Read + Send> = if mode == 0o120000 {
        let target = fs::read_link(&absolute)
            .map_err(|error| io_error("cannot read symlink", &absolute, error))?;
        Box::new(io::Cursor::new(target.as_os_str().as_bytes().to_vec()))
    } else {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
            .open(&absolute)
            .map_err(|error| io_error("cannot open regular file safely", &absolute, error))?;
        if !file
            .metadata()
            .map_err(|error| io_error("cannot inspect open file", &absolute, error))?
            .file_type()
            .is_file()
        {
            return Err(invalid(format!(
                "{} is not a regular file",
                absolute.display()
            )));
        }
        Box::new(file)
    };
    output_string(
        successful(
            run_reader(&mut command, Some(input), 256, "object hashing")?,
            "object hashing",
        )?,
        "object hashing",
    )
}

fn snapshot(source: &Path, managed_root: &Path) -> crate::Result<String> {
    reject_sparse(source)?;
    let tracked = tracked_paths(source)?;
    let mut paths = tracked.clone();
    paths.extend(parse_nul_paths(&run(
        git_with_ignore_config(source).args(["ls-files", "--others", "--exclude-standard", "-z"]),
        "untracked files",
    )?)?);
    let index = TemporaryIndex(managed_root.join(format!(".index-{}", ulid::Ulid::generate())));
    run(
        git(source)
            .env("GIT_INDEX_FILE", &index.0)
            .args(["read-tree", "--empty"]),
        "initialize private index",
    )?;
    let mut records = Vec::new();
    for path in paths {
        if !snapshot_path_accessible(source, &path)? {
            continue;
        }
        let absolute = source.join(&path);
        let metadata = match fs::symlink_metadata(&absolute) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_error("cannot inspect source entry", &absolute, error)),
        };
        if metadata.file_type().is_dir() && tracked.contains(&path) {
            continue;
        }
        let mode = if metadata.file_type().is_symlink() {
            0o120000
        } else if metadata.file_type().is_file() {
            if metadata.permissions().mode() & 0o111 != 0 {
                0o100755
            } else {
                0o100644
            }
        } else {
            return Err(invalid(format!(
                "unsupported source entry {}; regular files and symlinks are required",
                absolute.display()
            )));
        };
        let oid = hash_path(source, &path, mode)?;
        records.extend_from_slice(format!("{mode:o} {oid}\t").as_bytes());
        records.extend_from_slice(path.as_os_str().as_bytes());
        records.push(0);
    }
    run_with_input(
        git(source)
            .env("GIT_INDEX_FILE", &index.0)
            .args(["update-index", "-z", "--index-info"]),
        &records,
        "populate private index",
    )?;
    git_string(
        git(source)
            .env("GIT_INDEX_FILE", &index.0)
            .args(["write-tree"]),
        "write private tree",
    )
}

fn stable_snapshot(source: &Path, managed_root: &Path) -> crate::Result<String> {
    for _ in 0..2 {
        let first = snapshot(source, managed_root)?;
        let second = snapshot(source, managed_root)?;
        if first == second {
            return Ok(first);
        }
    }
    Err(invalid(format!(
        "{} changed while preparing a Git snapshot; retry when edits have stopped",
        source.display()
    )))
}

fn oid(tree: &str) -> crate::Result<()> {
    if (tree.len() != 40 && tree.len() != 64) || !tree.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(invalid("invalid Git tree object ID in workspace metadata"));
    }
    Ok(())
}

fn tree_entries(repo: &Path, tree: &str) -> crate::Result<BTreeMap<PathBuf, Entry>> {
    tree_entries_timeout(repo, tree, GIT_TIMEOUT)
}

fn tree_entries_timeout(
    repo: &Path,
    tree: &str,
    timeout: Duration,
) -> crate::Result<BTreeMap<PathBuf, Entry>> {
    oid(tree)?;
    let output = successful(
        run_reader_timeout(
            git(repo).args(["ls-tree", "-r", "-z", "--full-tree", tree]),
            None,
            GIT_STDOUT_LIMIT,
            timeout,
            "list snapshot tree",
        )?,
        "list snapshot tree",
    )?;
    let mut entries = BTreeMap::new();
    for record in output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let separator = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| invalid("invalid Git tree entry"))?;
        let (prefix, path) = (&record[..separator], &record[separator + 1..]);
        let prefix = std::str::from_utf8(prefix).map_err(|_| invalid("invalid Git tree mode"))?;
        let mut fields = prefix.split_ascii_whitespace();
        let mode = u32::from_str_radix(
            fields
                .next()
                .ok_or_else(|| invalid("missing Git tree mode"))?,
            8,
        )
        .map_err(|_| invalid("invalid Git tree mode"))?;
        let kind = fields
            .next()
            .ok_or_else(|| invalid("missing Git tree kind"))?;
        let oid = fields
            .next()
            .ok_or_else(|| invalid("missing Git tree object"))?;
        if kind != "blob" || !matches!(mode, 0o100644 | 0o100755 | 0o120000) {
            return Err(invalid(
                "submodules and special Git tree entries are not supported",
            ));
        }
        let path = PathBuf::from(OsString::from_vec(path.to_vec()));
        validate_relative(&path)?;
        entries.insert(
            path,
            Entry {
                mode,
                oid: oid.to_owned(),
            },
        );
    }
    Ok(entries)
}

fn create_commit(
    source: &Path,
    tree: &str,
    message: &str,
    parent: Option<&str>,
) -> crate::Result<String> {
    let mut command = git(source);
    command
        .env("GIT_AUTHOR_NAME", "Aether")
        .env("GIT_AUTHOR_EMAIL", "aether@local.invalid")
        .env("GIT_COMMITTER_NAME", "Aether")
        .env("GIT_COMMITTER_EMAIL", "aether@local.invalid")
        .args(["commit-tree", tree]);
    if let Some(parent) = parent {
        command.args(["-p", parent]);
    }
    git_string(
        command.args(["-m", message]),
        "create workspace snapshot commit",
    )
}

fn current_head(source: &Path) -> crate::Result<Option<String>> {
    let output = run_reader(
        git(source).args(["rev-parse", "--verify", "HEAD^{commit}"]),
        None,
        256,
        "inspect source HEAD",
    )?;
    if output.status.success() {
        Ok(Some(output_string(output.stdout, "source HEAD")?))
    } else {
        Ok(None)
    }
}

fn sync_directory(path: &Path) -> crate::Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error("cannot sync directory", path, error))
}

fn atomic_json<T: Serialize>(path: &Path, value: &T) -> crate::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("metadata path has no parent"))?;
    let temporary = parent.join(format!(".metadata-{}", ulid::Ulid::generate()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| io_error("cannot create metadata", &temporary, error))?;
        serde_json::to_writer(&mut file, value)
            .map_err(|error| invalid(format!("cannot encode workspace metadata: {error}")))?;
        file.sync_all()
            .map_err(|error| io_error("cannot sync metadata", &temporary, error))?;
        fs::rename(&temporary, path)
            .map_err(|error| io_error("cannot install metadata", path, error))?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> crate::Result<T> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| io_error("cannot inspect metadata", path, error))?;
    if !metadata.file_type().is_file() {
        return Err(invalid(format!(
            "unsafe workspace metadata at {}",
            path.display()
        )));
    }
    let file = File::open(path).map_err(|error| io_error("cannot read metadata", path, error))?;
    serde_json::from_reader(file).map_err(|error| {
        invalid(format!(
            "invalid workspace metadata {}: {error}",
            path.display()
        ))
    })
}

fn workspace_directory(root: &Path, workspace: &Path) -> crate::Result<PathBuf> {
    let root = real_directory(root, false)?;
    let workspace = real_directory(workspace, false)?;
    let parent = workspace
        .parent()
        .ok_or_else(|| invalid("workspace has no metadata directory"))?;
    if workspace.file_name() != Some(OsStr::new(WORKSPACE))
        || parent.parent() != Some(root.as_path())
    {
        return Err(invalid(format!(
            "{} is outside the private managed workspace root {}",
            workspace.display(),
            root.display()
        )));
    }
    let metadata = fs::symlink_metadata(parent)
        .map_err(|error| io_error("cannot inspect workspace directory", parent, error))?;
    if !metadata.file_type().is_dir() {
        return Err(invalid(
            "workspace metadata directory is a symlink or special file",
        ));
    }
    Ok(parent.to_path_buf())
}

pub(crate) fn create(
    managed_root: &Path,
    source: &Path,
    parent: AgentId,
) -> crate::Result<WorkspaceManifest> {
    let source = source_root(source)?;
    let planned = planned_directory(managed_root)?;
    if planned.starts_with(&source) {
        return Err(invalid(format!(
            "managed workspace root {} must be outside the source repository {}",
            planned.display(),
            source.display()
        )));
    }
    let root = real_directory(managed_root, true)?;
    if root.starts_with(&source) {
        return Err(invalid(
            "managed workspace root resolved inside the source repository",
        ));
    }
    let baseline_tree = stable_snapshot(&source, &root)?;
    let id = ulid::Ulid::generate().to_string();
    let directory = root.join(&id);
    fs::create_dir(&directory).map_err(|error| {
        io_error(
            "cannot create private workspace directory",
            &directory,
            error,
        )
    })?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error("cannot protect workspace directory", &directory, error))?;
    let workspace = directory.join(WORKSPACE);
    let reference = format!("refs/aether/workspaces/{id}");
    let result = (|| {
        let head = current_head(&source)?;
        let commit = create_commit(
            &source,
            &baseline_tree,
            "Aether child workspace snapshot",
            head.as_deref(),
        )?;
        run(
            git(&source).args(["update-ref", &reference, &commit]),
            "protect workspace snapshot from garbage collection",
        )?;
        run(
            git(&source)
                .args(["worktree", "add", "--detach", "--no-checkout", "--"])
                .arg(&workspace)
                .arg(&commit),
            "create detached child worktree",
        )?;
        run(
            git(&workspace).args(["read-tree", &commit]),
            "initialize child worktree index",
        )?;
        for (path, entry) in tree_entries(&source, &baseline_tree)? {
            write_entry(&source, &workspace, &path, &entry)?;
        }
        if stable_snapshot(&source, &root)? != baseline_tree {
            return Err(invalid(
                "source changed while creating the child worktree; retry",
            ));
        }
        let manifest = WorkspaceManifest {
            parent,
            source: source.clone(),
            workspace: workspace.clone(),
            baseline_tree: baseline_tree.clone(),
            integrated_tree: None,
        };
        atomic_json(&directory.join(MANIFEST), &manifest)?;
        Ok(manifest)
    })();
    if result.is_err() {
        let _ = run(
            git(&source)
                .args(["worktree", "remove", "--force", "--"])
                .arg(&workspace),
            "remove incomplete child worktree",
        );
        let _ = run(
            git(&source).args(["update-ref", "-d", &reference]),
            "remove incomplete snapshot reference",
        );
        let _ = fs::remove_dir_all(&directory);
    }
    result
}

pub(crate) fn load(managed_root: &Path, workspace: &Path) -> crate::Result<WorkspaceManifest> {
    load_timeout(managed_root, workspace, GIT_TIMEOUT)
}

fn load_timeout(
    managed_root: &Path,
    workspace: &Path,
    timeout: Duration,
) -> crate::Result<WorkspaceManifest> {
    let directory = workspace_directory(managed_root, workspace)?;
    let manifest: WorkspaceManifest = read_json(&directory.join(MANIFEST))?;
    let actual_workspace = fs::canonicalize(workspace)
        .map_err(|error| io_error("cannot resolve workspace", workspace, error))?;
    if manifest.workspace != actual_workspace {
        return Err(invalid("workspace manifest names a different worktree"));
    }
    oid(&manifest.baseline_tree)?;
    if let Some(integrated) = &manifest.integrated_tree {
        oid(integrated)?;
    }
    let source = source_root_timeout(&manifest.source, timeout)?;
    if source != manifest.source {
        return Err(invalid("workspace manifest source is not canonical"));
    }
    let top = git_string_timeout(
        git(&actual_workspace).args(["rev-parse", "--show-toplevel"]),
        timeout,
        "verify child worktree",
    )?;
    if fs::canonicalize(&top)
        .map_err(|error| io_error("cannot resolve worktree", Path::new(&top), error))?
        != actual_workspace
    {
        return Err(invalid("workspace path is not the Git worktree root"));
    }
    let source_common = git_string_timeout(
        git(&source).args(["rev-parse", "--path-format=absolute", "--git-common-dir"]),
        timeout,
        "source Git directory",
    )?;
    let workspace_common = git_string_timeout(
        git(&actual_workspace).args(["rev-parse", "--path-format=absolute", "--git-common-dir"]),
        timeout,
        "worktree Git directory",
    )?;
    if fs::canonicalize(&source_common).map_err(|error| {
        io_error(
            "cannot resolve Git directory",
            Path::new(&source_common),
            error,
        )
    })? != fs::canonicalize(&workspace_common).map_err(|error| {
        io_error(
            "cannot resolve Git directory",
            Path::new(&workspace_common),
            error,
        )
    })? {
        return Err(invalid("workspace belongs to a different Git repository"));
    }
    tree_entries_timeout(&source, &manifest.baseline_tree, timeout)?;
    Ok(manifest)
}

/// Abandon a freshly created workspace if durable child creation failed.
/// Refuse to remove any worktree that has acquired edits or extra files.
pub(crate) fn discard(managed_root: &Path, manifest: &WorkspaceManifest) -> crate::Result<()> {
    let trusted = load(managed_root, &manifest.workspace)?;
    if trusted.parent != manifest.parent
        || trusted.source != manifest.source
        || trusted.baseline_tree != manifest.baseline_tree
        || trusted.integrated_tree.is_some()
    {
        return Err(invalid("workspace is not an unused child worktree"));
    }
    let root = real_directory(managed_root, false)?;
    let directory = trusted
        .workspace
        .parent()
        .ok_or_else(|| invalid("workspace has no metadata directory"))?;
    if directory.join(JOURNAL).exists() {
        return Err(invalid(
            "workspace has a pending integration; refusing to discard it",
        ));
    }
    if stable_snapshot(&trusted.workspace, &root)? != trusted.baseline_tree {
        return Err(invalid("child worktree has edits; refusing to discard it"));
    }
    let baseline = tree_entries(&trusted.source, &trusted.baseline_tree)?;
    reject_extra_workspace_files(&trusted.workspace, &trusted.workspace, &baseline)?;
    run(
        git(&trusted.source)
            .args(["worktree", "remove", "--"])
            .arg(&trusted.workspace),
        "remove unused child worktree",
    )?;
    let id = directory
        .file_name()
        .ok_or_else(|| invalid("workspace has no identifier"))?
        .to_string_lossy();
    run(
        git(&trusted.source).args(["update-ref", "-d", &format!("refs/aether/workspaces/{id}")]),
        "remove unused workspace snapshot reference",
    )?;
    fs::remove_dir_all(directory)
        .map_err(|error| io_error("cannot remove unused workspace metadata", directory, error))?;
    sync_directory(&root)
}

fn reject_extra_workspace_files(
    root: &Path,
    directory: &Path,
    baseline: &BTreeMap<PathBuf, Entry>,
) -> crate::Result<()> {
    for item in fs::read_dir(directory)
        .map_err(|error| io_error("cannot inspect child worktree", directory, error))?
    {
        let item =
            item.map_err(|error| io_error("cannot inspect child worktree", directory, error))?;
        let path = item.path();
        if directory == root && item.file_name() == OsStr::new(".git") {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| invalid("invalid child worktree entry"))?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| io_error("cannot inspect child worktree entry", &path, error))?;
        if metadata.file_type().is_dir() {
            reject_extra_workspace_files(root, &path, baseline)?;
        } else if !baseline.contains_key(relative) {
            return Err(invalid(format!(
                "child worktree contains extra file {}; refusing to discard it",
                path.display()
            )));
        }
    }
    Ok(())
}

fn ensure_parent_directories(root: &Path, path: &Path) -> crate::Result<()> {
    let mut current = root.to_path_buf();
    if let Some(parent) = path.parent() {
        for component in parent.components() {
            let Component::Normal(name) = component else {
                return Err(invalid("unsafe workspace path"));
            };
            current.push(name);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_dir() => {}
                Ok(_) => {
                    return Err(invalid(format!(
                        "{} blocks a workspace path",
                        current.display()
                    )));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&current)
                        .map_err(|error| io_error("cannot create directory", &current, error))?
                }
                Err(error) => return Err(io_error("cannot inspect directory", &current, error)),
            }
        }
    }
    Ok(())
}

fn blob_bytes(repo: &Path, entry: &Entry) -> crate::Result<Vec<u8>> {
    let output = successful(
        run_reader(
            git(repo).args(["cat-file", "blob", &entry.oid]),
            None,
            16 * 1024,
            "read symlink Git blob",
        )?,
        "read symlink Git blob",
    )?;
    if output.contains(&0) {
        return Err(invalid("Git symlink target contains a NUL byte"));
    }
    Ok(output)
}

fn stream_blob(repo: &Path, entry: &Entry, file: &File) -> crate::Result<()> {
    let mut command = git(repo);
    let output = file
        .try_clone()
        .map_err(|error| invalid(format!("cannot clone output file for Git blob: {error}")))?;
    let mut child = command
        .args(["cat-file", "blob", &entry.oid])
        .stdin(Stdio::null())
        .stdout(Stdio::from(output))
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| invalid(format!("cannot start Git blob read: {error}")))?;
    let exceeded = Arc::new(AtomicBool::new(false));
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| invalid("git stderr unavailable"))?;
    let reader = thread::spawn({
        let exceeded = Arc::clone(&exceeded);
        move || read_limited(stderr, GIT_STDERR_LIMIT, exceeded)
    });
    let started = Instant::now();
    let mut status = None;
    while status.is_none() || !reader.is_finished() {
        if exceeded.load(Ordering::Relaxed) || started.elapsed() >= GIT_TIMEOUT {
            kill_git_group(&mut child);
            if status.is_none() {
                let _ = child.wait();
            }
            return Err(invalid(
                "Git blob read exceeded output limit or 60 second timeout",
            ));
        }
        if status.is_none() {
            status = child
                .try_wait()
                .map_err(|error| invalid(format!("cannot wait for Git blob read: {error}")))?;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let stderr = reader
        .join()
        .map_err(|_| invalid("Git stderr reader panicked"))?
        .map_err(|error| invalid(format!("cannot read Git blob error: {error}")))?;
    if !status
        .ok_or_else(|| invalid("Git blob exit status unavailable"))?
        .success()
    {
        return Err(invalid(format!(
            "cannot read Git blob {}: {}",
            entry.oid,
            String::from_utf8_lossy(&stderr).trim()
        )));
    }
    Ok(())
}

// None means the target is new: it is created with the default mode and the umask applies.
fn regular_file_mode(target: &Path, executable: bool) -> crate::Result<Option<u32>> {
    let existing = match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_file() => {
            Some(metadata.permissions().mode() & 0o777)
        }
        Ok(metadata) if metadata.file_type().is_symlink() || metadata.file_type().is_dir() => None,
        Ok(_) => {
            return Err(invalid(format!(
                "unsupported target entry at {}",
                target.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(io_error("cannot inspect target permissions", target, error)),
    };
    let Some(mode) = existing else {
        return Ok(None);
    };
    if executable {
        // Keep the parent's read/write access. Content replacement does not
        // reinstate setuid, setgid, or sticky bits from the old file.
        // Existing execute access is preserved. Only a newly executable file
        // gains owner execution and execution for classes with read access.
        if mode & 0o111 != 0 {
            Ok(Some(mode))
        } else {
            Ok(Some(mode | 0o100 | ((mode & 0o044) >> 2)))
        }
    } else {
        Ok(Some(mode & !0o111))
    }
}

fn write_entry(repo: &Path, root: &Path, path: &Path, entry: &Entry) -> crate::Result<()> {
    ensure_parent_directories(root, path)?;
    let target = root.join(path);
    let temporary = target.with_file_name(format!(".aether-{}", ulid::Ulid::generate()));
    let result = (|| {
        if entry.mode == 0o120000 {
            let bytes = blob_bytes(repo, entry)?;
            let link = OsString::from_vec(bytes);
            std::os::unix::fs::symlink(&link, &temporary)
                .map_err(|error| io_error("cannot create symlink", &temporary, error))?;
        } else {
            let executable = entry.mode == 0o100755;
            let existing_mode = regular_file_mode(&target, executable)?;
            let create_mode = if executable { 0o755 } else { 0o644 };
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(existing_mode.map_or(create_mode, |_| 0o600))
                .open(&temporary)
                .map_err(|error| io_error("cannot create workspace file", &temporary, error))?;
            stream_blob(repo, entry, &file)?;
            if let Some(mode) = existing_mode {
                file.set_permissions(fs::Permissions::from_mode(mode))
                    .map_err(|error| {
                        io_error("cannot set workspace file mode", &temporary, error)
                    })?;
            }
            file.sync_all()
                .map_err(|error| io_error("cannot sync workspace file", &temporary, error))?;
        }
        fs::rename(&temporary, &target)
            .map_err(|error| io_error("cannot install workspace file", &target, error))?;
        sync_directory(
            target
                .parent()
                .ok_or_else(|| invalid("workspace file has no parent"))?,
        )
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn changed_paths(
    baseline: &BTreeMap<PathBuf, Entry>,
    final_tree: &BTreeMap<PathBuf, Entry>,
) -> Vec<PathBuf> {
    baseline
        .keys()
        .chain(final_tree.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| baseline.get(*path) != final_tree.get(*path))
        .cloned()
        .collect()
}

fn tree_conflict(entries: &BTreeMap<PathBuf, Entry>) -> Option<PathBuf> {
    for path in entries.keys() {
        let mut parent = path.parent();
        while let Some(ancestor) = parent {
            if entries.contains_key(ancestor) {
                return Some(path.clone());
            }
            parent = ancestor.parent();
        }
    }
    None
}

fn inspect_actual_target(
    source: &Path,
    path: &Path,
    current: &BTreeMap<PathBuf, Entry>,
    final_tree: &BTreeMap<PathBuf, Entry>,
    changed: &BTreeSet<PathBuf>,
) -> crate::Result<()> {
    let mut relative = PathBuf::new();
    for component in path.parent().into_iter().flat_map(Path::components) {
        let Component::Normal(name) = component else {
            return Err(invalid("unsafe parent path"));
        };
        relative.push(name);
        let absolute = source.join(&relative);
        match fs::symlink_metadata(&absolute) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) if changed.contains(&relative) && !final_tree.contains_key(&relative) => {
                // The existing file or symlink is removed before this child
                // path is created. Never follow it during preflight.
                return Ok(());
            }
            Ok(_) => {
                return Err(invalid(format!(
                    "parent entry {} blocks child path {}",
                    absolute.display(),
                    path.display()
                )));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("cannot inspect parent path", &absolute, error)),
        }
    }
    let absolute = source.join(path);
    match fs::symlink_metadata(&absolute) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            if final_tree.contains_key(path) {
                inspect_replaced_directory(source, path, final_tree, changed)?;
            }
        }
        Ok(metadata) if metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            if !current.contains_key(path) {
                return Err(invalid(format!(
                    "parent has an ignored or newly created file at {}; integration would overwrite it",
                    absolute.display()
                )));
            }
        }
        Ok(_) => {
            return Err(invalid(format!(
                "unsupported parent entry at {}",
                absolute.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error("cannot inspect parent entry", &absolute, error)),
    }
    Ok(())
}

fn inspect_replaced_directory(
    source: &Path,
    relative: &Path,
    final_tree: &BTreeMap<PathBuf, Entry>,
    changed: &BTreeSet<PathBuf>,
) -> crate::Result<()> {
    let directory = source.join(relative);
    for item in fs::read_dir(&directory)
        .map_err(|error| io_error("cannot inspect parent directory", &directory, error))?
    {
        let item =
            item.map_err(|error| io_error("cannot inspect parent directory", &directory, error))?;
        let child = relative.join(item.file_name());
        let metadata = fs::symlink_metadata(item.path())
            .map_err(|error| io_error("cannot inspect parent entry", &item.path(), error))?;
        if metadata.file_type().is_dir() {
            inspect_replaced_directory(source, &child, final_tree, changed)?;
        } else if !changed.contains(&child) || final_tree.contains_key(&child) {
            return Err(invalid(format!(
                "parent entry {} blocks child replacement of {}",
                item.path().display(),
                directory.display()
            )));
        }
    }
    Ok(())
}

fn remove_empty_directory_tree(path: &Path) -> crate::Result<()> {
    for item in
        fs::read_dir(path).map_err(|error| io_error("cannot inspect directory", path, error))?
    {
        let item = item.map_err(|error| io_error("cannot inspect directory", path, error))?;
        let metadata = fs::symlink_metadata(item.path())
            .map_err(|error| io_error("cannot inspect directory", &item.path(), error))?;
        if metadata.file_type().is_dir() {
            remove_empty_directory_tree(&item.path())?;
        } else {
            return Err(invalid(format!(
                "parent entry {} blocks integration",
                item.path().display()
            )));
        }
    }
    fs::remove_dir(path).map_err(|error| io_error("cannot remove empty directory", path, error))
}

fn apply_changes(
    source: &Path,
    final_tree: &BTreeMap<PathBuf, Entry>,
    current: &BTreeMap<PathBuf, Entry>,
    changed: &[PathBuf],
) -> crate::Result<()> {
    for path in changed {
        if final_tree.contains_key(path) || current.get(path) == final_tree.get(path) {
            continue;
        }
        reject_symlink_ancestors(source, path)?;
        let target = source.join(path);
        fs::remove_file(&target)
            .map_err(|error| io_error("cannot remove parent file", &target, error))?;
        sync_directory(
            target
                .parent()
                .ok_or_else(|| invalid("parent file has no directory"))?,
        )?;
    }
    for path in changed {
        if current.get(path) == final_tree.get(path) {
            continue;
        }
        let Some(entry) = final_tree.get(path) else {
            continue;
        };
        reject_symlink_ancestors(source, path)?;
        let target = source.join(path);
        if fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.file_type().is_dir()) {
            remove_empty_directory_tree(&target)?;
        }
        write_entry(source, source, path, entry)?;
    }
    Ok(())
}

/// Serialize every change to managed worktrees; released when dropped.
fn lock_integrations(root: &Path) -> crate::Result<File> {
    let lock_path = root.join(".integration.lock");
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|error| io_error("cannot open integration lock", &lock_path, error))?;
    lock.lock()
        .map_err(|error| io_error("cannot lock workspace integrations", &lock_path, error))?;
    Ok(lock)
}

pub(crate) fn integrate(
    managed_root: &Path,
    manifest: &WorkspaceManifest,
) -> crate::Result<IntegrationOutcome> {
    let trusted = load(managed_root, &manifest.workspace)?;
    if trusted.parent != manifest.parent
        || trusted.source != manifest.source
        || trusted.baseline_tree != manifest.baseline_tree
    {
        return Err(invalid(
            "workspace manifest differs from the private managed record",
        ));
    }
    let root = real_directory(managed_root, false)?;
    let _lock = lock_integrations(&root)?;
    let directory = trusted
        .workspace
        .parent()
        .ok_or_else(|| invalid("workspace has no metadata directory"))?
        .to_path_buf();
    let journal_path = directory.join(JOURNAL);
    let existing_journal = journal_path.exists();
    let final_tree_id = if existing_journal {
        let journal: Journal = read_json(&journal_path)?;
        if journal.baseline_tree != trusted.baseline_tree {
            return Err(invalid(
                "workspace integration journal has a different baseline",
            ));
        }
        journal.final_tree
    } else {
        stable_snapshot(&trusted.workspace, &root)?
    };
    let baseline = tree_entries(&trusted.source, &trusted.baseline_tree)?;
    let final_tree = tree_entries(&trusted.source, &final_tree_id)?;
    let changed = changed_paths(&baseline, &final_tree);
    let current_tree_id = stable_snapshot(&trusted.source, &root)?;
    let current = tree_entries(&trusted.source, &current_tree_id)?;
    let conflicts: Vec<PathBuf> = changed
        .iter()
        .filter(|path| {
            let actual = current.get(*path);
            actual != baseline.get(*path) && actual != final_tree.get(*path)
        })
        .cloned()
        .collect();
    if !conflicts.is_empty() {
        return Ok(IntegrationOutcome::Conflicts(conflicts));
    }
    let mut desired = current.clone();
    for path in &changed {
        if let Some(entry) = final_tree.get(path) {
            desired.insert(path.clone(), entry.clone());
        } else {
            desired.remove(path);
        }
    }
    if let Some(path) = tree_conflict(&desired) {
        return Err(invalid(format!(
            "parent entry at {} conflicts with a child file/directory change",
            path.display()
        )));
    }
    let changed_set = changed.iter().cloned().collect();
    for path in &changed {
        inspect_actual_target(&trusted.source, path, &current, &final_tree, &changed_set)?;
    }
    if stable_snapshot(&trusted.source, &root)? != current_tree_id {
        return Err(invalid("parent changed during integration checks; retry"));
    }
    let already_integrated = changed
        .iter()
        .all(|path| current.get(path) == final_tree.get(path));
    if !existing_journal {
        // The durable target and journal precede the first parent write.
        let id = directory
            .file_name()
            .ok_or_else(|| invalid("workspace has no identifier"))?
            .to_string_lossy();
        let head = current_head(&trusted.workspace)?;
        let commit = create_commit(
            &trusted.source,
            &final_tree_id,
            "Aether child integration target",
            head.as_deref(),
        )?;
        run(
            git(&trusted.source).args([
                "update-ref",
                &format!("refs/aether/integrations/{id}"),
                &commit,
            ]),
            "protect child integration tree from garbage collection",
        )?;
        atomic_json(
            &journal_path,
            &Journal {
                baseline_tree: trusted.baseline_tree.clone(),
                final_tree: final_tree_id.clone(),
            },
        )?;
    }
    apply_changes(&trusted.source, &final_tree, &current, &changed)?;
    let mut updated = trusted;
    updated.integrated_tree = Some(final_tree_id);
    atomic_json(&directory.join(MANIFEST), &updated)?;
    fs::remove_file(&journal_path)
        .map_err(|error| io_error("cannot clear integration journal", &journal_path, error))?;
    sync_directory(&directory)?;
    Ok(IntegrationOutcome::Landed(IntegrationReport {
        changed_paths: changed,
        already_integrated,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process_umask() -> u32 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o777)
            .open(&path)
            .unwrap();
        0o777 & !(fs::metadata(&path).unwrap().permissions().mode() & 0o777)
    }

    fn git_ok(repo: &Path, args: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn landed(outcome: crate::Result<IntegrationOutcome>) -> IntegrationReport {
        match outcome.unwrap() {
            IntegrationOutcome::Landed(report) => report,
            IntegrationOutcome::Conflicts(paths) => panic!("unexpected conflicts: {paths:?}"),
        }
    }

    fn repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let managed = temp.path().join("managed");
        fs::create_dir(&source).unwrap();
        git_ok(&source, &["init", "-q"]);
        git_ok(&source, &["config", "user.name", "Test User"]);
        git_ok(&source, &["config", "user.email", "test@example.invalid"]);
        fs::write(source.join("tracked.txt"), b"initial\n").unwrap();
        fs::write(source.join("removed.txt"), b"remove me\n").unwrap();
        git_ok(&source, &["add", "."]);
        git_ok(&source, &["commit", "-qm", "initial"]);
        (temp, source, managed)
    }

    #[test]
    fn create_preserves_source_index_and_captures_raw_worktree() {
        let (_temp, source, managed) = repo();
        fs::write(source.join("tracked.txt"), b"staged\n").unwrap();
        git_ok(&source, &["add", "tracked.txt"]);
        fs::write(source.join("tracked.txt"), b"unstaged\0binary\n").unwrap();
        fs::remove_file(source.join("removed.txt")).unwrap();
        fs::write(source.join("script.sh"), b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(source.join("script.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("tracked.txt", source.join("link")).unwrap();
        let before_index = git_ok(&source, &["ls-files", "--stage", "-z"]);
        let before_status = git_ok(&source, &["status", "--porcelain=v1", "-z"]);
        let before_head = git_ok(&source, &["rev-parse", "HEAD"]);

        let manifest = create(&managed, &source, AgentId::new()).unwrap();
        assert_eq!(
            fs::read(manifest.workspace.join("tracked.txt")).unwrap(),
            b"unstaged\0binary\n"
        );
        assert!(!manifest.workspace.join("removed.txt").exists());
        assert_eq!(
            fs::read_link(manifest.workspace.join("link")).unwrap(),
            Path::new("tracked.txt")
        );
        assert_eq!(
            fs::metadata(manifest.workspace.join("script.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755 & !process_umask()
        );
        assert_eq!(
            before_index,
            git_ok(&source, &["ls-files", "--stage", "-z"])
        );
        assert_eq!(
            before_status,
            git_ok(&source, &["status", "--porcelain=v1", "-z"])
        );
        assert_eq!(before_head, git_ok(&source, &["rev-parse", "HEAD"]));
        let child_head = output_string(
            git_ok(&manifest.workspace, &["rev-parse", "HEAD"]),
            "child HEAD",
        )
        .unwrap();
        let source_head = output_string(before_head, "source HEAD").unwrap();
        git_ok(
            &source,
            &["merge-base", "--is-ancestor", &source_head, &child_head],
        );
        assert_eq!(
            load(&managed, &manifest.workspace).unwrap().baseline_tree,
            manifest.baseline_tree
        );
    }

    #[test]
    fn integration_checks_all_conflicts_before_writing_and_retries() {
        let (_temp, source, managed) = repo();
        fs::write(source.join("second.txt"), b"old\n").unwrap();
        git_ok(&source, &["add", "second.txt"]);
        git_ok(&source, &["commit", "-qm", "second"]);
        let manifest = create(&managed, &source, AgentId::new()).unwrap();
        fs::write(manifest.workspace.join("tracked.txt"), b"child one\n").unwrap();
        fs::write(manifest.workspace.join("second.txt"), b"child two\n").unwrap();
        fs::write(source.join("second.txt"), b"parent edit\n").unwrap();
        fs::write(source.join("tracked.txt"), b"parent edit\n").unwrap();
        let outcome = integrate(&managed, &manifest).unwrap();
        assert_eq!(
            outcome,
            IntegrationOutcome::Conflicts(vec![
                PathBuf::from("second.txt"),
                PathBuf::from("tracked.txt")
            ]),
            "every conflicting path is listed, not only the first"
        );
        fs::write(source.join("tracked.txt"), b"initial\n").unwrap();
        assert_eq!(fs::read(source.join("tracked.txt")).unwrap(), b"initial\n");
        assert!(!manifest.workspace.parent().unwrap().join(JOURNAL).exists());

        fs::write(source.join("second.txt"), b"old\n").unwrap();
        let first = landed(integrate(&managed, &manifest));
        assert!(!first.already_integrated);
        assert_eq!(
            first.changed_paths,
            vec![PathBuf::from("second.txt"), PathBuf::from("tracked.txt")]
        );
        assert_eq!(
            fs::read(source.join("tracked.txt")).unwrap(),
            b"child one\n"
        );
        assert_eq!(fs::read(source.join("second.txt")).unwrap(), b"child two\n");
        assert_eq!(
            fs::read(manifest.workspace.join("tracked.txt")).unwrap(),
            b"child one\n"
        );
        assert!(landed(integrate(&managed, &manifest)).already_integrated);
    }

    #[test]
    fn integration_preserves_existing_modes_and_creates_new_files_under_umask() {
        let (_temp, source, managed) = repo();
        for name in ["tracked.txt", "removed.txt"] {
            fs::set_permissions(source.join(name), fs::Permissions::from_mode(0o600)).unwrap();
        }
        let manifest = create(&managed, &source, AgentId::new()).unwrap();
        fs::write(manifest.workspace.join("tracked.txt"), b"private edit\n").unwrap();
        fs::write(
            manifest.workspace.join("removed.txt"),
            b"private executable\n",
        )
        .unwrap();
        fs::set_permissions(
            manifest.workspace.join("removed.txt"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::write(manifest.workspace.join("new.txt"), b"new private file\n").unwrap();
        fs::write(manifest.workspace.join("new.sh"), b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(
            manifest.workspace.join("new.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        landed(integrate(&managed, &manifest));
        let umask = process_umask();
        for (name, expected) in [
            ("tracked.txt", 0o600),
            ("removed.txt", 0o700),
            ("new.txt", 0o644 & !umask),
            ("new.sh", 0o755 & !umask),
        ] {
            let mode = fs::metadata(source.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, expected, "{name}");
        }
    }

    #[test]
    fn integration_clears_special_bits_on_replaced_file() {
        let (_temp, source, managed) = repo();
        fs::set_permissions(
            source.join("tracked.txt"),
            fs::Permissions::from_mode(0o4640),
        )
        .unwrap();
        assert_ne!(
            fs::metadata(source.join("tracked.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o4000,
            0
        );
        let manifest = create(&managed, &source, AgentId::new()).unwrap();
        fs::write(manifest.workspace.join("tracked.txt"), b"new contents\n").unwrap();

        landed(integrate(&managed, &manifest));
        assert_eq!(
            fs::metadata(source.join("tracked.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o640
        );
    }

    #[test]
    fn integration_keeps_existing_executable_access_on_content_edit() {
        let (_temp, source, managed) = repo();
        fs::set_permissions(
            source.join("tracked.txt"),
            fs::Permissions::from_mode(0o744),
        )
        .unwrap();
        let manifest = create(&managed, &source, AgentId::new()).unwrap();
        fs::write(manifest.workspace.join("tracked.txt"), b"new contents\n").unwrap();

        landed(integrate(&managed, &manifest));
        assert_eq!(
            fs::metadata(source.join("tracked.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o744
        );
    }

    #[test]
    fn globally_ignored_file_is_absent_from_child_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();
        let excludes = temp.path().join("global-excludes");
        fs::write(&excludes, b"private.key\n").unwrap();
        fs::write(
            home.join(".gitconfig"),
            format!("[core]\n\texcludesFile = {}\n", excludes.display()),
        )
        .unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "workspace::tests::globally_ignored_file_subprocess_case",
                "--nocapture",
            ])
            .env("HOME", &home)
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_NOSYSTEM")
            .env("AETHER_GLOBAL_IGNORE_CASE", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "subprocess did not run the ignore fixture"
        );
    }

    #[test]
    fn globally_ignored_file_subprocess_case() {
        if std::env::var_os("AETHER_GLOBAL_IGNORE_CASE").is_none() {
            return;
        }
        let (_temp, source, managed) = repo();
        fs::write(source.join("private.key"), b"private contents").unwrap();
        let manifest = create(&managed, &source, AgentId::new()).unwrap();
        assert!(source.join("private.key").exists());
        assert!(!manifest.workspace.join("private.key").exists());
    }

    #[test]
    fn integration_handles_file_directory_replacements_and_ignored_collision() {
        let (_temp, source, managed) = repo();
        fs::create_dir(source.join("folder")).unwrap();
        fs::write(source.join("folder/old"), b"old").unwrap();
        fs::write(source.join("plain"), b"plain").unwrap();
        fs::write(source.join(".gitignore"), b"ignored\n").unwrap();
        git_ok(&source, &["add", "."]);
        git_ok(&source, &["commit", "-qm", "shapes"]);
        let manifest = create(&managed, &source, AgentId::new()).unwrap();
        fs::remove_file(manifest.workspace.join("folder/old")).unwrap();
        fs::remove_dir(manifest.workspace.join("folder")).unwrap();
        fs::write(manifest.workspace.join("folder"), b"now a file").unwrap();
        fs::remove_file(manifest.workspace.join("plain")).unwrap();
        fs::create_dir(manifest.workspace.join("plain")).unwrap();
        fs::write(manifest.workspace.join("plain/new"), b"nested").unwrap();
        fs::write(manifest.workspace.join("ignored"), b"child").unwrap();
        git_ok(&manifest.workspace, &["add", "-f", "ignored"]);
        fs::write(source.join("ignored"), b"parent private").unwrap();
        let error = integrate(&managed, &manifest).unwrap_err().to_string();
        assert!(error.contains("ignored"), "{error}");
        assert_eq!(fs::read(source.join("folder/old")).unwrap(), b"old");
        fs::remove_file(source.join("ignored")).unwrap();
        landed(integrate(&managed, &manifest));
        assert_eq!(fs::read(source.join("folder")).unwrap(), b"now a file");
        assert_eq!(fs::read(source.join("plain/new")).unwrap(), b"nested");
        assert_eq!(fs::read(source.join("ignored")).unwrap(), b"child");
    }

    #[test]
    fn integration_journal_finishes_partial_parent_write() {
        let (_temp, source, managed) = repo();
        fs::write(source.join("second.txt"), b"old\n").unwrap();
        git_ok(&source, &["add", "second.txt"]);
        git_ok(&source, &["commit", "-qm", "second"]);
        let manifest = create(&managed, &source, AgentId::new()).unwrap();
        fs::write(manifest.workspace.join("tracked.txt"), b"new one\n").unwrap();
        fs::write(manifest.workspace.join("second.txt"), b"new two\n").unwrap();
        let target = stable_snapshot(&manifest.workspace, &managed).unwrap();
        atomic_json(
            &manifest.workspace.parent().unwrap().join(JOURNAL),
            &Journal {
                baseline_tree: manifest.baseline_tree.clone(),
                final_tree: target.clone(),
            },
        )
        .unwrap();
        fs::write(source.join("second.txt"), b"new two\n").unwrap();
        let report = landed(integrate(&managed, &manifest));
        assert!(!report.already_integrated);
        assert_eq!(fs::read(source.join("tracked.txt")).unwrap(), b"new one\n");
        assert!(!manifest.workspace.parent().unwrap().join(JOURNAL).exists());
        assert_eq!(
            load(&managed, &manifest.workspace)
                .unwrap()
                .integrated_tree
                .as_deref(),
            Some(target.as_str())
        );
    }

    #[test]
    fn source_symlink_ancestor_is_rejected_without_reading_outside() {
        let (temp, source, managed) = repo();
        fs::create_dir(source.join("folder")).unwrap();
        fs::write(source.join("folder/file"), b"inside").unwrap();
        git_ok(&source, &["add", "folder/file"]);
        git_ok(&source, &["commit", "-qm", "folder"]);
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("file"), b"secret").unwrap();
        fs::remove_file(source.join("folder/file")).unwrap();
        fs::remove_dir(source.join("folder")).unwrap();
        std::os::unix::fs::symlink(&outside, source.join("folder")).unwrap();
        let error = create(&managed, &source, AgentId::new())
            .unwrap_err()
            .to_string();
        assert!(error.contains("symlink ancestor"), "{error}");
    }

    #[test]
    fn discard_refuses_dirty_worktree_and_removes_unused_one() {
        let (_temp, source, managed) = repo();
        let manifest = create(&managed, &source, AgentId::new()).unwrap();
        fs::write(manifest.workspace.join("extra"), b"edit").unwrap();
        assert!(discard(&managed, &manifest).is_err());
        fs::remove_file(manifest.workspace.join("extra")).unwrap();
        discard(&managed, &manifest).unwrap();
        assert!(!manifest.workspace.exists());
        assert!(!manifest.workspace.parent().unwrap().exists());
    }

    #[test]
    fn managed_root_inside_source_is_rejected_without_creating_it() {
        let (_temp, source, _managed) = repo();
        let nested = source.join("generated/workspaces");
        assert!(create(&nested, &source, AgentId::new()).is_err());
        assert!(!source.join("generated").exists());
    }
}
