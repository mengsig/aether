//! `complete_path`: one directory listing per request, filtered by the last
//! typed segment, for choosing a workspace and for `@` mentions.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use aether_protocol::remote::{PathCompletion, PathCompletionKind, PathCompletions};
use aether_protocol::{ErrorReply, Reply};
use ignore::WalkBuilder;

use super::{check_absolute, check_text, effective_limit, home_dir, on_blocking_pool, workspaces};

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;
/// Directory entries one request may look at; a bigger directory is cut off.
const SCAN_ENTRIES: usize = 50_000;
/// How long one request may read a directory before it is cut off.
const SCAN_TIME: Duration = Duration::from_secs(1);

pub(super) async fn complete_path(
    kind: PathCompletionKind,
    prefix: String,
    workspace: Option<PathBuf>,
    limit: Option<u32>,
) -> Result<Reply, ErrorReply> {
    check_text("prefix", &prefix)?;
    let limit = effective_limit(limit, DEFAULT_LIMIT, MAX_LIMIT)?;
    let completions = match kind {
        PathCompletionKind::Directory => {
            let target = directory_target(&prefix, home_dir().as_deref())?;
            on_blocking_pool("path completion", move || {
                complete_directories(&target, limit)
            })
            .await?
        }
        PathCompletionKind::WorkspaceFile => {
            let workspace = workspace.ok_or_else(|| {
                ErrorReply::invalid("`workspace` is required for `workspace_file`")
            })?;
            let workspace = check_absolute("workspace", &workspace)?;
            on_blocking_pool("path completion", move || {
                complete_workspace_files(&workspace, &prefix, limit)
            })
            .await??
        }
    };
    Ok(Reply::PathCompletions(completions))
}

/// The directory to list and the name fragment its entries must start with.
#[derive(Debug, PartialEq, Eq)]
struct DirectoryTarget {
    parent: PathBuf,
    segment: String,
}

/// `text` split after its last `/`: the directory part as typed, and the
/// fragment being completed.
fn split_prefix(text: &str) -> (&str, &str) {
    match text.rfind('/') {
        Some(at) => text.split_at(at + 1),
        None => ("", text),
    }
}

/// `.` and `..` folded lexically, the way a shell's `cd` reads them; symlinks
/// are not resolved.
fn fold(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Where a `directory` prefix points: absolute, or `~` for the daemon user's
/// home. An empty prefix or a bare `~` means the home directory.
fn directory_target(prefix: &str, home: Option<&Path>) -> Result<DirectoryTarget, ErrorReply> {
    let text = if prefix.is_empty() || prefix == "~" {
        "~/"
    } else {
        prefix
    };
    let (dir, segment) = split_prefix(text);
    let parent = if let Some(rest) = dir.strip_prefix("~/") {
        let home = home.ok_or_else(|| ErrorReply::invalid("the daemon has no home directory"))?;
        home.join(rest)
    } else if dir.starts_with('/') {
        PathBuf::from(dir)
    } else {
        return Err(ErrorReply::invalid(
            "`prefix` must be an absolute path or start with `~/`",
        ));
    };
    assert!(parent.is_absolute(), "a target parent is always absolute");
    Ok(DirectoryTarget {
        parent: fold(&parent),
        segment: segment.to_owned(),
    })
}

/// Which entry names a typed fragment accepts: case-insensitive unless it has an uppercase letter,
/// and hidden names only for a fragment starting with `.`.
struct NameMatcher {
    fragment: Vec<char>,
    fold_case: bool,
    show_hidden: bool,
}

impl NameMatcher {
    fn new(segment: &str) -> Self {
        let fold_case = !segment.chars().any(char::is_uppercase);
        let fragment = if fold_case {
            segment.chars().flat_map(char::to_lowercase).collect()
        } else {
            segment.chars().collect()
        };
        Self {
            fragment,
            fold_case,
            show_hidden: segment.starts_with('.'),
        }
    }

    fn accepts(&self, name: &str) -> bool {
        if name.starts_with('.') && !self.show_hidden {
            return false;
        }
        if self.fold_case {
            let mut folded = name.chars().flat_map(char::to_lowercase);
            self.fragment
                .iter()
                .all(|want| folded.next() == Some(*want))
        } else {
            let mut chars = name.chars();
            self.fragment.iter().all(|want| chars.next() == Some(*want))
        }
    }
}

/// How much of a directory one request has read, against [`SCAN_ENTRIES`] and
/// [`SCAN_TIME`].
struct Scan {
    started: Instant,
    entries: usize,
    cut: bool,
}

impl Scan {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            entries: 0,
            cut: false,
        }
    }

    /// Account for one more entry; `false` once the budget is spent.
    fn next_entry(&mut self) -> bool {
        let out_of_time = self.entries.is_multiple_of(64) && self.started.elapsed() > SCAN_TIME;
        if self.entries >= SCAN_ENTRIES || out_of_time {
            self.cut = true;
            return false;
        }
        self.entries += 1;
        true
    }
}

/// An entry name that can be sent: UTF-8, since a lossy spelling could name
/// another file, and free of control characters.
fn listable(name: &OsStr) -> Option<&str> {
    name.to_str()
        .filter(|name| !name.chars().any(char::is_control))
}

/// Sorted by path, a directory's trailing `/` aside, and cut to `limit`;
/// `truncated` when more matched or the scan was cut short.
fn finish(mut found: Vec<PathCompletion>, limit: usize, scan: &Scan) -> PathCompletions {
    assert!(limit > 0, "a limit is validated before it is used");
    found.sort_by(|a, b| {
        a.path
            .trim_end_matches('/')
            .cmp(b.path.trim_end_matches('/'))
    });
    let truncated = found.len() > limit || scan.cut;
    found.truncate(limit);
    assert!(found.len() <= limit, "a completion never exceeds its limit");
    PathCompletions {
        entries: found,
        truncated,
    }
}

fn complete_directories(target: &DirectoryTarget, limit: usize) -> PathCompletions {
    // A parent that is missing or unreadable has nothing to offer; the typed
    // path is checked when it is used.
    let Ok(entries) = std::fs::read_dir(&target.parent) else {
        return PathCompletions::default();
    };
    let matcher = NameMatcher::new(&target.segment);
    let mut scan = Scan::new();
    let mut found = Vec::new();
    for entry in entries {
        if !scan.next_entry() {
            break;
        }
        let Ok(entry) = entry else { continue };
        let file_name = entry.file_name();
        let Some(name) = listable(&file_name) else {
            continue;
        };
        if matcher.accepts(name) && is_directory(&entry) {
            let path = target.parent.join(name);
            found.push(PathCompletion {
                path: format!("{}/", path.display()),
                is_dir: true,
            });
        }
    }
    finish(found, limit, &scan)
}

/// A directory, or a symlink to one: somewhere to go, listed and not resolved.
fn is_directory(entry: &std::fs::DirEntry) -> bool {
    match entry.file_type() {
        Ok(kind) if kind.is_dir() => true,
        Ok(kind) if kind.is_symlink() => {
            std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir())
        }
        _ => false,
    }
}

fn complete_workspace_files(
    workspace: &Path,
    prefix: &str,
    limit: usize,
) -> Result<PathCompletions, ErrorReply> {
    let root = workspaces::resolve_directory(workspace)
        .map_err(|unusable| ErrorReply::invalid(format!("`workspace` {}", unusable.reason)))?;
    let (dir, segment) = split_prefix(prefix);
    let Some(target) = directory_inside(&root, dir) else {
        return Ok(PathCompletions::default());
    };
    let matcher = NameMatcher::new(segment);
    let mut scan = Scan::new();
    let mut found = Vec::new();
    for item in git_aware_walk(&target) {
        if !scan.next_entry() {
            break;
        }
        // A directory that cannot be read yields an error and no entries.
        let Ok(entry) = item else { continue };
        if entry.depth() == 0 {
            continue;
        }
        let Some(name) = listable(entry.file_name()) else {
            continue;
        };
        if name == ".git" || !matcher.accepts(name) {
            continue;
        }
        let Some(is_dir) = kind_inside(&root, &entry) else {
            continue;
        };
        let slash = if is_dir { "/" } else { "" };
        found.push(PathCompletion {
            path: format!("{dir}{name}{slash}"),
            is_dir,
        });
    }
    Ok(finish(found, limit, &scan))
}

/// `target`'s direct children, minus what git ignores, with symlinks left alone.
fn git_aware_walk(target: &Path) -> ignore::Walk {
    WalkBuilder::new(target)
        .max_depth(Some(1))
        .hidden(false)
        .ignore(false)
        .parents(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .follow_links(false)
        .build()
}

/// The real directory `dir` names below `root`; `None` when it would leave
/// `root`, by `..`, an absolute path or a symlink, or does not exist.
fn directory_inside(root: &Path, dir: &str) -> Option<PathBuf> {
    let mut target = root.to_path_buf();
    for part in Path::new(dir).components() {
        match part {
            Component::Normal(name) => target.push(name),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if target == root {
        return Some(target);
    }
    let real = std::fs::canonicalize(&target).ok()?;
    real.starts_with(root).then_some(real)
}

/// Whether an entry is a directory (`true`) or a file (`false`); `None` for a special file, a
/// dangling symlink or one whose target is outside `root`.
fn kind_inside(root: &Path, entry: &ignore::DirEntry) -> Option<bool> {
    if entry.path_is_symlink() {
        let real = std::fs::canonicalize(entry.path()).ok()?;
        if !real.starts_with(root) {
            return None;
        }
        return real
            .is_dir()
            .then_some(true)
            .or_else(|| real.is_file().then_some(false));
    }
    let kind = entry.file_type()?;
    if kind.is_dir() {
        Some(true)
    } else {
        kind.is_file().then_some(false)
    }
}

#[cfg(test)]
mod tests;
