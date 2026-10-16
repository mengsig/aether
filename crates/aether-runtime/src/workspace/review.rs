//! Read a workspace's changes for an operator, and abandon a managed
//! worktree the operator decided to throw away.

use super::*;
use aether_core::workspace_review::{
    COMMIT_LIMIT, CappedPatch, ChangedFile, FILE_LIMIT, FileStatus, PATCH_LIMIT, WorkspaceCommit,
    cap_patch, cap_summary,
};

#[cfg(test)]
mod tests;

const HEAD_TIMEOUT: Duration = Duration::from_secs(5);

/// Files, line counts and a capped patch between two Git tree-ishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeDiff {
    pub files: Vec<ChangedFile>,
    pub files_truncated: bool,
    pub insertions: u64,
    pub deletions: u64,
    pub patch: CappedPatch,
}

fn object_id(value: &str) -> crate::Result<()> {
    if (value.len() != 40 && value.len() != 64) || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid(format!("{value:?} is not a Git object ID")));
    }
    Ok(())
}

/// The commit `dir` has checked out; `None` outside a repository or on an
/// unborn branch.
pub(crate) fn head(dir: &Path) -> crate::Result<Option<String>> {
    let output = run_reader_timeout(
        git(dir).args(["rev-parse", "--verify", "-q", "HEAD^{commit}"]),
        None,
        256,
        HEAD_TIMEOUT,
        "read HEAD",
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let head = output_string(output.stdout, "HEAD")?;
    object_id(&head)?;
    Ok(Some(head))
}

/// The managed workspace's directory name, which names its refs.
fn workspace_id(manifest: &WorkspaceManifest) -> crate::Result<String> {
    let id = manifest
        .workspace
        .parent()
        .and_then(Path::file_name)
        .ok_or_else(|| invalid("workspace has no identifier"))?
        .to_string_lossy()
        .into_owned();
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(invalid(format!("unexpected workspace identifier {id:?}")));
    }
    Ok(id)
}

/// The commit a managed workspace started from.
pub(crate) fn snapshot_commit(manifest: &WorkspaceManifest) -> crate::Result<String> {
    let reference = format!(
        "refs/aether/workspaces/{}^{{commit}}",
        workspace_id(manifest)?
    );
    let commit = git_string(
        git(&manifest.workspace).args(["rev-parse", "--verify", &reference]),
        "resolve workspace snapshot",
    )?;
    object_id(&commit)?;
    Ok(commit)
}

/// The tree a managed workspace holds now, committed or not. `manifest`
/// must come from [`load`].
pub(crate) fn current_tree(
    managed_root: &Path,
    manifest: &WorkspaceManifest,
) -> crate::Result<String> {
    let root = real_directory(managed_root, false)?;
    stable_snapshot(&manifest.workspace, &root)
}

fn diff_command(repo: &Path) -> Command {
    let mut command = git(repo);
    command.args([
        "diff",
        "--no-renames",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
    ]);
    command
}

fn nul_records(output: &[u8]) -> impl Iterator<Item = &[u8]> {
    output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
}

/// A path as reply text; one that is not UTF-8 is sent as `hex:` bytes.
pub(crate) fn path_text(path: &Path) -> String {
    path.to_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("hex:{}", hex::encode(path.as_os_str().as_bytes())))
}

fn bytes_path(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes.to_vec()))
}

/// `--name-status -z` output: a status letter, then the path.
fn parse_name_status(output: &[u8]) -> crate::Result<Vec<(PathBuf, FileStatus)>> {
    let records: Vec<_> = nul_records(output).collect();
    if records.len() % 2 != 0 {
        return Err(invalid("git diff listed a status without a path"));
    }
    records
        .chunks(2)
        .map(|pair| {
            let status = match pair[0] {
                b"A" => FileStatus::Added,
                b"D" => FileStatus::Deleted,
                b"M" | b"T" => FileStatus::Modified,
                other => {
                    return Err(invalid(format!(
                        "unexpected git diff status {:?}",
                        String::from_utf8_lossy(other)
                    )));
                }
            };
            Ok((bytes_path(pair[1]), status))
        })
        .collect()
}

/// `--numstat -z` output: `added<TAB>deleted<TAB>path`, `-` for binary.
fn parse_numstat(output: &[u8]) -> crate::Result<BTreeMap<PathBuf, Option<(u64, u64)>>> {
    let mut counts = BTreeMap::new();
    for record in nul_records(output) {
        let mut fields = record.splitn(3, |byte| *byte == b'\t');
        let (Some(added), Some(deleted), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err(invalid("git diff returned a malformed line count"));
        };
        let lines = match (added, deleted) {
            (b"-", b"-") => None,
            _ => Some((count(added)?, count(deleted)?)),
        };
        counts.insert(bytes_path(path), lines);
    }
    Ok(counts)
}

fn count(field: &[u8]) -> crate::Result<u64> {
    std::str::from_utf8(field)
        .ok()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| invalid("git diff returned a malformed line count"))
}

/// The patch from `from` to `to`, written to a private file by Git so a
/// huge one is measured without being held in memory.
fn capped_patch(repo: &Path, from: &str, to: &str) -> crate::Result<CappedPatch> {
    let file = tempfile::Builder::new()
        .prefix("aether-patch-")
        .tempfile()
        .map_err(|error| invalid(format!("cannot create a patch file: {error}")))?;
    let mut output = OsString::from("--output=");
    output.push(file.path());
    run(
        diff_command(repo).arg(output).args([from, to]),
        "write patch",
    )?;
    let total = file
        .as_file()
        .metadata()
        .map_err(|error| io_error("cannot measure patch", file.path(), error))?
        .len();
    let mut shown = Vec::with_capacity(PATCH_LIMIT.min(total as usize));
    file.as_file()
        .take(PATCH_LIMIT as u64)
        .read_to_end(&mut shown)
        .map_err(|error| io_error("cannot read patch", file.path(), error))?;
    Ok(cap_patch(&shown, total))
}

/// What changed between two tree-ishes of `repo`.
pub(crate) fn tree_diff(repo: &Path, from: &str, to: &str) -> crate::Result<TreeDiff> {
    object_id(from)?;
    object_id(to)?;
    let statuses = parse_name_status(&run(
        diff_command(repo).args(["--name-status", "-z", from, to]),
        "list changed files",
    )?)?;
    let counts = parse_numstat(&run(
        diff_command(repo).args(["--numstat", "-z", from, to]),
        "count changed lines",
    )?)?;
    let files_truncated = statuses.len() > FILE_LIMIT;
    let (mut insertions, mut deletions) = (0, 0);
    let mut files = Vec::with_capacity(statuses.len().min(FILE_LIMIT));
    for (path, status) in statuses {
        let lines = counts.get(&path).copied().flatten();
        if let Some((added, removed)) = lines {
            insertions += added;
            deletions += removed;
        }
        if files.len() < FILE_LIMIT {
            files.push(ChangedFile {
                path: path_text(&path),
                status,
                insertions: lines.map(|(added, _)| added),
                deletions: lines.map(|(_, removed)| removed),
            });
        }
    }
    Ok(TreeDiff {
        files,
        files_truncated,
        insertions,
        deletions,
        patch: capped_patch(repo, from, to)?,
    })
}

fn parse_commit(record: &[u8]) -> crate::Result<WorkspaceCommit> {
    let text = std::str::from_utf8(record).map_err(|_| invalid("git log returned non-UTF-8"))?;
    let mut fields = text.splitn(4, '\u{1f}');
    let (Some(id), Some(author), Some(at), Some(summary)) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(invalid("git log returned a malformed commit"));
    };
    object_id(id)?;
    let at = at
        .parse::<i64>()
        .ok()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .ok_or_else(|| invalid("git log returned a malformed commit time"))?;
    Ok(WorkspaceCommit {
        id: id.to_owned(),
        author: author.to_owned(),
        at,
        summary: cap_summary(summary),
        turn: None,
    })
}

/// Commits reachable from `to` but not from `from`, newest first, at most
/// `limit` of them.
pub(crate) fn log(
    repo: &Path,
    from: &str,
    to: &str,
    limit: usize,
) -> crate::Result<Vec<WorkspaceCommit>> {
    object_id(from)?;
    object_id(to)?;
    debug_assert!(limit > 0 && limit <= COMMIT_LIMIT + 1);
    let output = run(
        git(repo).args([
            "log",
            "-z",
            "--no-color",
            "--no-show-signature",
            "--format=%H%x1f%an%x1f%at%x1f%s",
            &format!("--max-count={limit}"),
            &format!("{from}..{to}"),
        ]),
        "list commits",
    )?;
    nul_records(&output).map(parse_commit).collect()
}

/// Remove a managed worktree, its refs and its metadata, whatever it holds:
/// the operator chose to throw the work away. Refused mid-integration.
pub(crate) fn abandon(managed_root: &Path, manifest: &WorkspaceManifest) -> crate::Result<()> {
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
        .ok_or_else(|| invalid("workspace has no metadata directory"))?;
    if directory.join(JOURNAL).exists() {
        return Err(invalid(
            "workspace has an unfinished integration; integrate it again to finish before discarding",
        ));
    }
    let id = workspace_id(&trusted)?;
    run(
        git(&trusted.source)
            .args(["worktree", "remove", "--force", "--"])
            .arg(&trusted.workspace),
        "remove child worktree",
    )?;
    for reference in [
        format!("refs/aether/workspaces/{id}"),
        format!("refs/aether/integrations/{id}"),
    ] {
        run(
            git(&trusted.source).args(["update-ref", "-d", &reference]),
            "remove workspace reference",
        )?;
    }
    fs::remove_dir_all(directory)
        .map_err(|error| io_error("cannot remove workspace metadata", directory, error))?;
    sync_directory(&root)
}
