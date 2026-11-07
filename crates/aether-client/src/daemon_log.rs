//! What the daemon's own log says about why it is slow: the recent
//! `slow status section` warnings, read from a bounded tail of its file.
//! A log that is missing or unreadable just means that signal is not named.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};

/// How much of the end of the log is read, however large the file is.
const TAIL_BYTES: u64 = 256 * 1024;
/// How old a warning may be and still explain a timeout happening now.
const WINDOW: TimeDelta = TimeDelta::minutes(10);
const SLOW_SECTION_MESSAGE: &str = "slow status section";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlowSection {
    pub(crate) name: String,
    pub(crate) elapsed_ms: u64,
}

/// The file the daemon writes now; mirrors the daemon's `logging::log_file`.
fn log_path() -> PathBuf {
    aether_protocol::paths::log_dir()
        .join("aetherd.log")
        .join("segments-v1")
        .join("events.0.log")
}

/// The daemon's own slow queries in the last few minutes, in one phrase;
/// `None` when its log shows none or cannot be read.
pub(crate) fn observe() -> Option<String> {
    let path = log_path();
    let tail = match read_tail(&path) {
        Ok(tail) => tail,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "cannot read the daemon log tail");
            return None;
        }
    };
    slowest_recent(&tail, Utc::now()).map(|section| describe(&section))
}

/// The last [`TAIL_BYTES`] of the file, without the line the cut may have
/// split.
fn read_tail(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes)?;
    assert!(
        bytes.len() as u64 <= TAIL_BYTES,
        "the read is bounded by the tail size"
    );
    let text = String::from_utf8_lossy(&bytes);
    Ok(if start > 0 {
        drop_first_line(&text).to_owned()
    } else {
        text.into_owned()
    })
}

fn drop_first_line(text: &str) -> &str {
    text.split_once('\n').map_or("", |(_, rest)| rest)
}

/// The slowest `slow status section` warning within [`WINDOW`] of `now`.
pub(crate) fn slowest_recent(tail: &str, now: DateTime<Utc>) -> Option<SlowSection> {
    tail.lines()
        .filter_map(|line| parse_slow_section(line, now))
        .max_by_key(|section| section.elapsed_ms)
}

/// One log line as a recent slow section; `None` for any other line, an
/// old one, or one that does not read as `<rfc3339> ... section="x" elapsed_ms=n`.
fn parse_slow_section(line: &str, now: DateTime<Utc>) -> Option<SlowSection> {
    let (stamp, rest) = line.split_once(char::is_whitespace)?;
    let at = DateTime::parse_from_rfc3339(stamp)
        .ok()?
        .with_timezone(&Utc);
    if now.signed_duration_since(at) > WINDOW || !rest.contains(SLOW_SECTION_MESSAGE) {
        return None;
    }
    let field = |name: &str| rest.split_whitespace().find_map(|t| t.strip_prefix(name));
    let name = field("section=")?.trim_matches('"');
    let elapsed_ms = field("elapsed_ms=")?.parse().ok()?;
    if name.is_empty() {
        return None;
    }
    Some(SlowSection {
        name: name.to_owned(),
        elapsed_ms,
    })
}

pub(crate) fn describe(section: &SlowSection) -> String {
    assert!(!section.name.is_empty(), "a section has a name");
    format!(
        "its own queries are slow: status section `{}` took {:.1} s",
        section.name,
        section.elapsed_ms as f64 / 1000.0
    )
}

#[cfg(test)]
#[path = "daemon_log_tests.rs"]
mod tests;
