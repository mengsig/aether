//! Changes in the work tree, counted by `git status` over a git directory of the daemon's own, so
//! no filter the repository's config names can run. Git gets its index, objects and files only.

use std::ffi::OsString;
use std::path::Path;

use aether_protocol::ErrorReply;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

use super::head::Head;
use super::{Layout, ObjectFormat, Repo, failed, finish, first_line, spawn_error, unexpected};
use crate::connection::host_services::unavailable;

/// `status` entries read before the counts are left as lower bounds.
pub(super) const MAX_STATUS_ENTRIES: usize = 10_000;
/// Bytes of git's stderr kept to find the line that says what went wrong.
const MAX_MESSAGE_BYTES: usize = 4096;
/// The `core.*` settings carried over because they decide whether a file counts as changed. Each
/// takes a word and names no program.
const COMPARISON_KEYS: [&str; 8] = [
    "filemode",
    "symlinks",
    "ignorecase",
    "autocrlf",
    "eol",
    "trustctime",
    "checkstat",
    "precomposeunicode",
];
/// Longest value any of [`COMPARISON_KEYS`] takes, with room to spare.
const MAX_SETTING_CHARS: usize = 16;

/// Paths that differ, by where.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Changes {
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
}

impl Repo<'_> {
    /// The repository's values for [`COMPARISON_KEYS`], each as `key=value`.
    pub(super) async fn comparison_settings(&self) -> Result<Vec<String>, ErrorReply> {
        let pattern = format!(r"^core\.({})$", COMPARISON_KEYS.join("|"));
        let mut command = self.in_repository();
        command.args(["config", "-z", "--get-regexp", &pattern]);
        let run = finish(command).await?;
        if !run.success {
            // Exit 1 with nothing said is how `git config` says no key matched.
            return match run.stderr.trim().is_empty() {
                true => Ok(Vec::new()),
                false => Err(failed("config", &run)),
            };
        }
        let records = run.stdout.split('\0').filter(|record| !record.is_empty());
        records.map(comparison_setting).collect()
    }

    pub(super) async fn count_changes(
        &self,
        layout: &Layout,
        head: &Head,
        settings: &[String],
    ) -> Result<Changes, ErrorReply> {
        let (commit, format) = (head.commit.clone(), layout.object_format);
        let private = tokio::task::spawn_blocking(move || private_git_dir(commit, format))
            .await
            .map_err(|e| ErrorReply::internal(format!("the git directory job failed: {e}")))?
            .map_err(|e| unavailable(format!("cannot prepare a git directory: {e}")))?;
        let mut command = self.beside_repository(layout, private.path(), settings);
        command.args([
            "status",
            "--porcelain=v2",
            "-z",
            "--no-renames",
            "--untracked-files=normal",
            "--ignore-submodules=all",
        ]);
        read_status(command).await
    }

    /// Git over the repository's work tree, index and objects with `private`
    /// as its git directory: the repository's config is never opened.
    fn beside_repository(&self, layout: &Layout, private: &Path, settings: &[String]) -> Command {
        assert!(
            private.is_absolute() && layout.root.is_absolute(),
            "git is told where things are in full"
        );
        let mut excludes = OsString::from("core.excludesFile=");
        excludes.push(layout.common_dir.join("info/exclude"));
        let mut command = self.git();
        command
            .env("GIT_DIR", private)
            .env("GIT_WORK_TREE", &layout.root)
            .env("GIT_INDEX_FILE", layout.git_dir.join("index"))
            .env("GIT_OBJECT_DIRECTORY", layout.common_dir.join("objects"));
        for setting in settings {
            command.arg("-c").arg(setting);
        }
        command.arg("-c").arg(excludes).arg("-C").arg(&layout.root);
        command
    }
}

/// One `key\nvalue` record of `git config -z` as `key=value`. A key that was
/// not asked for, or a value that is not a plain word, is refused.
fn comparison_setting(record: &str) -> Result<String, ErrorReply> {
    // A key written with no `= value` is a boolean that is true.
    let (key, value) = record.split_once('\n').unwrap_or((record, "true"));
    let asked = key
        .strip_prefix("core.")
        .is_some_and(|name| COMPARISON_KEYS.contains(&name));
    if !asked {
        return Err(unexpected("config"));
    }
    let word = !value.is_empty()
        && value.len() <= MAX_SETTING_CHARS
        && value.bytes().all(|b| b.is_ascii_alphanumeric());
    if !word {
        return Err(unavailable(format!(
            "the repository sets `{key}` to something that is not one word"
        )));
    }
    Ok(format!("{key}={value}"))
}

/// A git directory holding a config of ours and a `HEAD` at `commit`, or on
/// a branch with no commit when there is none.
fn private_git_dir(
    commit: Option<String>,
    format: ObjectFormat,
) -> std::io::Result<tempfile::TempDir> {
    let config = match format {
        ObjectFormat::Sha1 => "[core]\n\trepositoryformatversion = 0\n\tbare = false\n",
        ObjectFormat::Sha256 => {
            "[core]\n\trepositoryformatversion = 1\n\tbare = false\n\
             [extensions]\n\tobjectformat = sha256\n"
        }
    };
    let head = match commit {
        Some(commit) => {
            assert_eq!(commit.len(), format.hex_len(), "a checked object name");
            format!("{commit}\n")
        }
        None => "ref: refs/heads/unborn\n".to_owned(),
    };
    let dir = tempfile::Builder::new().prefix("aether-git-").tempdir()?;
    std::fs::create_dir(dir.path().join("refs"))?;
    std::fs::write(dir.path().join("config"), config)?;
    std::fs::write(dir.path().join("HEAD"), head)?;
    Ok(dir)
}

/// What `git status --porcelain=v2 -z` said, counted.
async fn read_status(mut command: Command) -> Result<Changes, ErrorReply> {
    let mut child = command.spawn().map_err(spawn_error)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ErrorReply::internal("git status has no stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ErrorReply::internal("git status has no stderr"))?;
    let (parsed, errors) = tokio::join!(
        parse_status(stdout, &mut child),
        read_head(stderr, MAX_MESSAGE_BYTES)
    );
    let parser = parsed?;
    let status = child
        .wait()
        .await
        .map_err(|e| unavailable(format!("cannot wait for git status: {e}")))?;
    if !status.success() && !parser.is_full() {
        return Err(unavailable(format!(
            "git status failed: {}",
            first_line(&errors?)
        )));
    }
    Ok(parser.changes)
}

/// Read records until the output ends or [`MAX_STATUS_ENTRIES`] have been
/// counted, in which case `git` is stopped: the counts are then lower bounds.
async fn parse_status(
    stdout: tokio::process::ChildStdout,
    child: &mut tokio::process::Child,
) -> Result<StatusParser, ErrorReply> {
    let mut parser = StatusParser::default();
    let mut records = BufReader::new(stdout);
    let mut raw = Vec::new();
    while !parser.is_full() {
        raw.clear();
        let read = records
            .read_until(0, &mut raw)
            .await
            .map_err(|e| unavailable(format!("cannot read git status: {e}")))?;
        if read == 0 {
            return Ok(parser);
        }
        parser.entry(&String::from_utf8_lossy(&raw));
    }
    child
        .start_kill()
        .map_err(|e| unavailable(format!("cannot stop git status: {e}")))?;
    Ok(parser)
}

/// The first `keep` bytes of `stream`, reading it to the end so the writer
/// never blocks on a full pipe.
async fn read_head(
    mut stream: tokio::process::ChildStderr,
    keep: usize,
) -> Result<String, ErrorReply> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|e| unavailable(format!("cannot read git errors: {e}")))?;
        if read == 0 {
            return Ok(String::from_utf8_lossy(&head).into_owned());
        }
        let room = keep.saturating_sub(head.len());
        head.extend_from_slice(&chunk[..read.min(room)]);
    }
}

/// Accumulates the records of one `status --porcelain=v2 -z`.
#[derive(Default)]
pub(super) struct StatusParser {
    pub changes: Changes,
    entries: usize,
}

impl StatusParser {
    fn is_full(&self) -> bool {
        self.entries >= MAX_STATUS_ENTRIES
    }

    pub(super) fn entry(&mut self, record: &str) {
        let record = record.trim_end_matches('\0');
        if record.starts_with("# ") {
            return;
        }
        self.entries += 1;
        let mut fields = record.split(' ');
        match fields.next() {
            Some("?") => self.changes.untracked += 1,
            Some("1") => self.changed(fields.next().unwrap_or("..")),
            Some("u") => self.changes.modified += 1,
            _ => {}
        }
    }

    /// The `XY` of an ordinary entry: `X` is the index side, `Y` the work tree's.
    fn changed(&mut self, xy: &str) {
        let mut sides = xy.chars();
        if sides.next().is_some_and(|x| x != '.') {
            self.changes.staged += 1;
        }
        if sides.next().is_some_and(|y| y != '.') {
            self.changes.modified += 1;
        }
    }
}
