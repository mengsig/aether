//! Reads the operator's long-lived Claude Code OAuth token file.
//! Errors name the path and the problem, never the file's content.

use std::ffi::OsString;
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::Path;

use crate::EngineError;

pub(super) const TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";
/// A real token is far below this; a larger file is not a token file.
const MAX_BYTES: u64 = 16 * 1024;

fn refuse(path: &Path, problem: impl std::fmt::Display) -> EngineError {
    EngineError::Unsupported(format!(
        "claude_oauth_token_file {} {problem}",
        path.display()
    ))
}

/// The environment entry that hands the token in `path` to one child.
/// Read at every spawn so a rotated file takes effect without a restart.
pub(super) fn environment(path: &Path) -> Result<(OsString, OsString), EngineError> {
    debug_assert!(
        !path.as_os_str().is_empty(),
        "the config rejects an empty path"
    );
    let token = read_token(path)?;
    debug_assert!(!token.is_empty() && !token.contains('\n'));
    Ok((TOKEN_ENV.into(), token.into()))
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
    Ok(token.to_owned())
}
