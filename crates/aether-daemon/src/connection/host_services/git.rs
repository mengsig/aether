//! A workspace's git state, read by running `git` where the content, `.git` included, is untrusted:
//! no run may start a program the repository names ([`head`], [`work_tree`]).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use aether_protocol::ErrorReply;
use aether_protocol::remote::GitSummary;
use tokio::process::Command;
use tokio::sync::Semaphore;

use super::unavailable;

mod head;
mod work_tree;

/// Git requests that may run at once. Tests share one process and run side
/// by side, which is not the crowd this keeps out.
const MAX_GIT_JOBS: usize = if cfg!(test) { 1024 } else { 8 };
/// Longest line of git's own words kept in an error message.
const MAX_MESSAGE_CHARS: usize = 200;
/// Abbreviated commit length, git's own default.
const ABBREVIATED_COMMIT: usize = 7;

static GIT_JOBS: Semaphore = Semaphore::const_new(MAX_GIT_JOBS);

/// Branch, commit, upstream distance and dirtiness around `dir`, within `timeout` overall. Not
/// being in a repository is `repository: false`; anything else that stops git is an error.
pub(super) async fn summarize(dir: &Path, timeout: Duration) -> Result<GitSummary, ErrorReply> {
    summarize_with(OsStr::new("git"), dir, timeout).await
}

/// [`summarize`] with the git program named, so tests can stand in for it.
async fn summarize_with(
    program: &OsStr,
    dir: &Path,
    timeout: Duration,
) -> Result<GitSummary, ErrorReply> {
    let _slot = GIT_JOBS
        .try_acquire()
        .map_err(|_| unavailable("too many git requests are running; try again"))?;
    let repo = Repo { program, dir };
    match tokio::time::timeout(timeout, repo.read_summary()).await {
        Ok(summary) => summary,
        Err(_) => Err(unavailable("git took too long")),
    }
}

/// A directory to run git in, and the program that is git.
struct Repo<'a> {
    program: &'a OsStr,
    dir: &'a Path,
}

/// The two object name formats git has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectFormat {
    Sha1,
    Sha256,
}

impl ObjectFormat {
    fn hex_len(self) -> usize {
        match self {
            Self::Sha1 => 40,
            Self::Sha256 => 64,
        }
    }
}

/// Where git found the repository around a directory.
#[derive(Debug, PartialEq, Eq)]
struct Layout {
    /// Top of the work tree.
    root: PathBuf,
    /// This work tree's git directory, where its index is.
    git_dir: PathBuf,
    /// The git directory every work tree shares, where the objects are.
    common_dir: PathBuf,
    object_format: ObjectFormat,
}

impl Repo<'_> {
    async fn read_summary(&self) -> Result<GitSummary, ErrorReply> {
        let Some(layout) = self.locate().await? else {
            return Ok(GitSummary::default());
        };
        let head = self.read_head(layout.object_format).await?;
        let settings = self.comparison_settings().await?;
        let changes = self.count_changes(&layout, &head, &settings).await?;
        let abbreviated = |commit: String| commit.chars().take(ABBREVIATED_COMMIT).collect();
        Ok(GitSummary {
            repository: true,
            root: Some(layout.root),
            branch: head.branch,
            head: head.commit.map(abbreviated),
            detached: head.detached,
            dirty: changes.staged + changes.modified + changes.untracked > 0,
            ahead: head.ahead,
            behind: head.behind,
            staged: changes.staged,
            modified: changes.modified,
            untracked: changes.untracked,
        })
    }

    /// A `git` that inherits nothing from the daemon's environment and cannot
    /// page, prompt, take a lock, run a hook or reach a remote.
    fn git(&self) -> Command {
        let mut command = Command::new(self.program);
        command.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        command
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(["--no-pager", "--no-optional-locks"])
            .args(["-c", "core.fsmonitor=false"])
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(["-c", "protocol.allow=never"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        command
    }

    /// Git in the repository as it is, its config read. Only for commands
    /// that touch refs, commits and config: never the index or the work tree.
    fn in_repository(&self) -> Command {
        let mut command = self.git();
        command.arg("-C").arg(self.dir);
        command
    }
}

fn spawn_error(error: std::io::Error) -> ErrorReply {
    if error.kind() == std::io::ErrorKind::NotFound {
        return unavailable("git is not installed or not on the daemon's PATH");
    }
    unavailable(format!("cannot run git: {error}"))
}

struct Finished {
    success: bool,
    stdout: String,
    stderr: String,
}

/// Run `command` to completion; its output is small by construction.
async fn finish(mut command: Command) -> Result<Finished, ErrorReply> {
    let output = command.output().await.map_err(spawn_error)?;
    Ok(Finished {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn first_line(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_MESSAGE_CHARS)
        .collect()
}

fn failed(what: &str, run: &Finished) -> ErrorReply {
    unavailable(format!("git {what} failed: {}", first_line(&run.stderr)))
}

impl Repo<'_> {
    /// Where the repository around the directory is; `None` outside a work
    /// tree, which includes a bare repository and the inside of `.git`.
    async fn locate(&self) -> Result<Option<Layout>, ErrorReply> {
        let mut command = self.in_repository();
        command.args([
            "rev-parse",
            "--is-inside-work-tree",
            "--show-toplevel",
            "--absolute-git-dir",
            "--git-common-dir",
            "--show-object-format",
        ]);
        let run = finish(command).await?;
        if !run.success {
            let outside = run.stderr.contains("not a git repository")
                || run.stderr.contains("must be run in a work tree");
            return if outside {
                Ok(None)
            } else {
                Err(failed("rev-parse", &run))
            };
        }
        let lines: Vec<&str> = run.stdout.lines().collect();
        match lines[..] {
            ["true", root, git_dir, common_dir, format] => {
                self.layout(root, git_dir, common_dir, format).map(Some)
            }
            ["false", ..] => Ok(None),
            _ => Err(unexpected("rev-parse")),
        }
    }

    /// A [`Layout`] from what `rev-parse` printed. Only the common directory
    /// may be relative, to the directory git ran in.
    fn layout(
        &self,
        root: &str,
        git_dir: &str,
        common_dir: &str,
        format: &str,
    ) -> Result<Layout, ErrorReply> {
        let object_format = match format {
            "sha1" => ObjectFormat::Sha1,
            "sha256" => ObjectFormat::Sha256,
            _ => return Err(unexpected("rev-parse")),
        };
        let (root, git_dir) = (PathBuf::from(root), PathBuf::from(git_dir));
        if !root.is_absolute() || !git_dir.is_absolute() || common_dir.is_empty() {
            return Err(unexpected("rev-parse"));
        }
        Ok(Layout {
            root,
            git_dir,
            common_dir: self.dir.join(common_dir),
            object_format,
        })
    }
}

/// An object name exactly as `format` writes one, or an error naming `what`.
fn object_name(text: &str, format: ObjectFormat, what: &str) -> Result<String, ErrorReply> {
    let hex = text.len() == format.hex_len() && text.bytes().all(|b| b.is_ascii_hexdigit());
    if !hex {
        return Err(unexpected(what));
    }
    Ok(text.to_owned())
}

fn unexpected(what: &str) -> ErrorReply {
    unavailable(format!("git {what} answered something unexpected"))
}

#[cfg(test)]
mod hostile_tests;
#[cfg(test)]
mod tests;
