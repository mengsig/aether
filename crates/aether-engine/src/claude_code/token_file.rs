//! Reads the operator's long-lived Claude Code OAuth token file.
//! Errors name the path and the problem, never the file's content.

use std::borrow::Cow;
use std::ffi::OsString;
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::Path;

use aether_core::redact::{MIN_KNOWN_SECRET_CHARS, Redactor};

use crate::{EngineError, EngineEvent};

pub(super) const TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";
/// A real token is far below this; a larger file is not a token file.
const MAX_BYTES: u64 = 16 * 1024;

fn refuse(path: &Path, problem: impl std::fmt::Display) -> EngineError {
    EngineError::Unsupported(format!(
        "claude_oauth_token_file {} {problem}",
        path.display()
    ))
}

/// One spawn's token. Not `Debug`, so it never reaches a log by accident.
pub(super) struct Token(String);

impl Token {
    /// Read at every spawn so a rotated file takes effect without a restart.
    pub(super) fn read(path: &Path) -> Result<Self, EngineError> {
        debug_assert!(
            !path.as_os_str().is_empty(),
            "the config rejects an empty path"
        );
        let token = read_token(path)?;
        debug_assert!(token.len() >= MIN_KNOWN_SECRET_CHARS && !token.contains('\n'));
        Ok(Self(token))
    }

    /// The environment entry that hands the token to the child.
    pub(super) fn environment(&self) -> (OsString, OsString) {
        (TOKEN_ENV.into(), self.0.as_str().into())
    }

    /// `base` that also masks the token, for everything the child prints.
    pub(super) fn redactor(&self, base: Redactor) -> Result<Redactor, EngineError> {
        base.with_known_secret(&self.0)
            .map_err(|_| EngineError::Unsupported("the Claude OAuth token cannot be masked".into()))
    }

    /// `line` with the token masked, however JSON escapes it.
    pub(super) fn mask_line<'a>(&self, line: &'a str) -> Cow<'a, str> {
        super::token_mask::mask_line(&self.0, line)
    }

    /// `events` with the token masked, even where deltas split it.
    pub(super) fn mask_events(
        &self,
        held: &mut super::token_mask::HeldDeltas,
        events: Vec<EngineEvent>,
    ) -> Vec<EngineEvent> {
        held.mask(&self.0, events)
    }
}

fn read_token(path: &Path) -> Result<String, EngineError> {
    // O_NOFOLLOW refuses a symlink; O_NONBLOCK keeps a FIFO from stalling the open.
    let flags = nix::fcntl::OFlag::O_NOFOLLOW | nix::fcntl::OFlag::O_NONBLOCK;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits())
        .open(path)
        .map_err(|e| refuse(path, format!("cannot be opened as a regular file: {e}")))?;
    let meta = file
        .metadata()
        .map_err(|e| refuse(path, format!("cannot be inspected: {e}")))?;
    check_metadata(path, &meta)?;
    let mut raw = String::new();
    file.take(MAX_BYTES)
        .read_to_string(&mut raw)
        .map_err(|e| refuse(path, format!("cannot be read as UTF-8 text: {e}")))?;
    parse_token(path, &raw)
}

fn check_metadata(path: &Path, meta: &std::fs::Metadata) -> Result<(), EngineError> {
    if !meta.is_file() {
        return Err(refuse(path, "must be a regular file"));
    }
    if meta.len() > MAX_BYTES {
        return Err(refuse(path, format!("is larger than {MAX_BYTES} bytes")));
    }
    let euid = rustix::process::geteuid().as_raw();
    if meta.uid() != euid {
        return Err(refuse(
            path,
            format!("must be owned by uid {euid} (found {})", meta.uid()),
        ));
    }
    let mode = meta.mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(refuse(
            path,
            format!("must have mode 0600 (found {mode:04o})"),
        ));
    }
    Ok(())
}

fn parse_token(path: &Path, raw: &str) -> Result<String, EngineError> {
    let token = raw.trim();
    if token.is_empty() {
        return Err(refuse(path, "is empty"));
    }
    if token.contains(['\n', '\r']) {
        return Err(refuse(path, "must hold the token on a single line"));
    }
    if token.chars().count() < MIN_KNOWN_SECRET_CHARS {
        return Err(refuse(
            path,
            format!("holds fewer than {MIN_KNOWN_SECRET_CHARS} characters"),
        ));
    }
    // Characters a JSON writer never escapes, so masking knows its spellings.
    if !token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_.~+=".contains(c))
    {
        return Err(refuse(path, "holds characters a token never has"));
    }
    Ok(token.to_owned())
}
