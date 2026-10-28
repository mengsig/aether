//! Version/capability preflight shared by every engine adapter's `detect`.
//!
//! Each adapter shells out to its CLI's `--version` and, for Codex's
//! app-server, probes one more capability. Before this module every failure
//! mode there - the program absent, a non-zero exit, output with no
//! recognizable version, a version below what the adapter's `args()` needs,
//! or (codex-app) a missing subcommand - collapsed to the same `None`, with
//! no reason recorded anywhere. [`DetectError`] keeps the reason; [`probe_version`]
//! is the one place that runs `<program> --version` and turns its outcome
//! into one of them.

use std::path::Path;
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Bound on CLI output kept in an error: enough to show what went wrong,
/// never enough to flood a log with a runaway or garbled process.
const MAX_OUTPUT_CHARS: usize = 500;

fn bounded(text: &str) -> String {
    let trimmed = text.trim();
    let mut chars = trimmed.chars();
    let head: String = chars.by_ref().take(MAX_OUTPUT_CHARS).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// A CLI's `major.minor.patch` version, comparable so a minimum can be
/// expressed as a value instead of re-parsed inline at each use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EngineVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl EngineVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parse the bare `major.minor.patch` version out of `--version` output,
    /// wherever it falls among the whitespace-separated tokens. The two
    /// supported CLIs disagree on where it sits - `claude --version` prints
    /// it first (`"2.1.269 (Claude Code)"`), `codex --version` last
    /// (`"codex-cli 0.154.0"`) - so this scans by shape instead of position,
    /// which reads either without assuming one.
    pub fn parse(text: &str) -> Option<Self> {
        text.split_whitespace().find_map(Self::parse_token)
    }

    fn parse_token(token: &str) -> Option<Self> {
        // A trailing `-prerelease`/`+build` tag (`2.1.269-beta.1`,
        // `0.154.0+build.5`) does not change what the release can do, and a
        // dev/prerelease build is realistically what a CLI auto-updater
        // ships between numbered releases; treat it as its base version
        // rather than rejecting the whole CLI as unparsable over it.
        let release = token.split(['-', '+']).next().unwrap_or(token);
        let mut parts = release.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        // A fourth dotted part (e.g. a build id) is not a plain release
        // version; leave it to the next token rather than truncating it.
        if parts.next().is_some() {
            return None;
        }
        Some(Self::new(major, minor, patch))
    }
}

impl std::fmt::Display for EngineVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Why an engine CLI could not be used, in enough detail to act on. Every
/// variant names the program; `aether doctor` and the daemon log are both
/// expected to print the value's [`Display`] verbatim.
#[derive(Debug, Clone, PartialEq)]
pub enum DetectError {
    /// The CLI started but did not finish its preflight within the deadline.
    TimedOut { program: String, probe: String },
    /// The OS could not start it - not on `PATH`, or another spawn failure.
    NotInstalled { program: String, os_error: String },
    /// The probe's scratch state directory could not be created, so the CLI
    /// was not run: a temp-storage problem, not the CLI's.
    ScratchUnavailable { program: String, os_error: String },
    /// It started but exited non-zero.
    ExitFailure {
        program: String,
        status: String,
        stderr: String,
    },
    /// It exited zero but nothing in its output looked like a version.
    UnparsableVersion { program: String, output: String },
    /// It reported a version below what this adapter's `args()` requires.
    TooOld {
        program: String,
        detected: EngineVersion,
        minimum: EngineVersion,
    },
    /// The version gate passed but a protocol/subcommand capability this
    /// adapter depends on did not answer (codex-app's `app-server --help`).
    MissingCapability {
        program: String,
        capability: String,
        detail: String,
    },
}

impl std::fmt::Display for DetectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimedOut { program, probe } => write!(
                f,
                "{program} {probe} timed out after {} seconds; check that the CLI responds before retrying",
                PROBE_TIMEOUT.as_secs()
            ),
            Self::NotInstalled { program, os_error } => write!(
                f,
                "{program} is not installed or not on PATH ({os_error}); install {program}"
            ),
            Self::ScratchUnavailable { program, os_error } => write!(
                f,
                "could not create a scratch directory to probe {program} in ({os_error}); point TMPDIR at a writable directory with free space"
            ),
            Self::ExitFailure {
                program,
                status,
                stderr,
            } => write!(
                f,
                "{program} --version exited {status}: {stderr}; reinstall {program}"
            ),
            Self::UnparsableVersion { program, output } => write!(
                f,
                "{program} --version printed no recognizable version ({output}); reinstall {program}"
            ),
            Self::TooOld {
                program,
                detected,
                minimum,
            } => write!(
                f,
                "{program} {detected} is older than the {minimum} Aether requires; upgrade {program} to >= {minimum}"
            ),
            Self::MissingCapability {
                program,
                capability,
                detail,
            } => write!(
                f,
                "{program} does not support {capability} ({detail}); upgrade {program}"
            ),
        }
    }
}

impl std::error::Error for DetectError {}

/// Why `kind`'s CLI is no longer an executable on this process's `PATH`, if
/// it is not: a cheap check that a CLI probed usable earlier is still there.
/// An engine with no CLI of its own has nothing to lose, so it is present.
pub fn missing_cli(kind: crate::EngineKind) -> Option<DetectError> {
    let program = if kind == crate::EngineKind::CLAUDE_CODE {
        "claude"
    } else if kind == crate::EngineKind::CODEX {
        "codex"
    } else {
        return None;
    };
    if crate::offline::executable_on_path(program, std::env::var_os("PATH").as_deref()) {
        return None;
    }
    Some(DetectError::NotInstalled {
        program: program.to_owned(),
        os_error: "no executable of that name in any PATH directory".to_owned(),
    })
}

/// The variable naming the directory Codex keeps its state in. Codex
/// creates files there on any start, `--version` included.
pub(crate) const CODEX_STATE_ENV: &str = "CODEX_HOME";

/// Run `<program> --version` (optionally searching `path` instead of the
/// process's own `PATH`, for deterministic tests) and parse its version.
/// `state_env` names the variable of a CLI that writes state on every start;
/// the probe points it at a scratch directory removed afterwards.
pub(crate) async fn probe_version(
    program: &str,
    path: Option<&Path>,
    state_env: Option<&str>,
) -> Result<EngineVersion, DetectError> {
    probe_version_with_args(program, &["--version"], path, state_env).await
}

async fn probe_version_with_args(
    program: &str,
    args: &[&str],
    path: Option<&Path>,
    state_env: Option<&str>,
) -> Result<EngineVersion, DetectError> {
    let output = run_probe(program, args, path, state_env)
        .await?
        .ok_or_else(|| DetectError::TimedOut {
            program: program.to_owned(),
            probe: "--version".into(),
        })?
        .map_err(|e| DetectError::NotInstalled {
            program: program.to_owned(),
            os_error: e.to_string(),
        })?;
    if !output.status.success() {
        return Err(DetectError::ExitFailure {
            program: program.to_owned(),
            status: output.status.to_string(),
            stderr: bounded(&String::from_utf8_lossy(&output.stderr)),
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    EngineVersion::parse(&text).ok_or_else(|| DetectError::UnparsableVersion {
        program: program.to_owned(),
        output: bounded(&text),
    })
}

/// Run `<program> <capability_args>` (optionally on `path`) and report
/// whether it answered - the structural check for a subcommand an older CLI
/// may not have (codex-app's `app-server --help`).
pub(crate) async fn probe_capability(
    program: &str,
    capability_args: &[&str],
    capability: &str,
    path: Option<&Path>,
    state_env: Option<&str>,
) -> Result<(), DetectError> {
    let output = run_probe(program, capability_args, path, state_env)
        .await?
        .ok_or_else(|| DetectError::TimedOut {
            program: program.to_owned(),
            probe: capability_args.join(" "),
        })?
        .map_err(|e| DetectError::MissingCapability {
            program: program.to_owned(),
            capability: capability.to_owned(),
            detail: e.to_string(),
        })?;
    if !output.status.success() {
        return Err(DetectError::MissingCapability {
            program: program.to_owned(),
            capability: capability.to_owned(),
            detail: bounded(&String::from_utf8_lossy(&output.stderr)),
        });
    }
    Ok(())
}

/// Run one probe with no stdin, its output captured; `None` once it outlives
/// [`PROBE_TIMEOUT`], when it is killed. Spawned like an engine, so a daemon
/// killed mid-probe does not leave it behind, and its group ends with it.
async fn run_probe(
    program: &str,
    args: &[&str],
    path: Option<&Path>,
    state_env: Option<&str>,
) -> Result<Option<std::io::Result<std::process::Output>>, DetectError> {
    use std::process::Stdio;

    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    // Removed when the probe ends, whatever it wrote there.
    let _scratch = match state_env {
        Some(name) => Some(scratch_state_dir(&mut command, name).map_err(|e| {
            DetectError::ScratchUnavailable {
                program: program.to_owned(),
                os_error: e.to_string(),
            }
        })?),
        None => None,
    };
    let (child, watch) = match crate::lifeline::spawn(command) {
        Ok(spawned) => spawned,
        Err(error) => return Ok(Some(Err(error))),
    };
    // Whatever the probe started, a wrapper's own CLI included, goes with it,
    // also when this future is dropped mid-probe.
    let _group_end = watch.ending_on_drop();
    let output = tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output())
        .await
        .ok();
    Ok(output)
}

/// Point `name` at a fresh private directory for `command`'s run.
fn scratch_state_dir(
    command: &mut tokio::process::Command,
    name: &str,
) -> std::io::Result<tempfile::TempDir> {
    assert!(!name.is_empty(), "a state variable needs a name");
    let dir = tempfile::Builder::new()
        .prefix("aether-engine-probe-")
        .tempdir()?;
    debug_assert!(
        dir.path().is_dir(),
        "the CLI requires the directory to exist"
    );
    command.env(name, dir.path());
    Ok(dir)
}

#[cfg(test)]
#[path = "detect_process_tests.rs"]
mod process_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_version_first_shape() {
        assert_eq!(
            EngineVersion::parse("2.1.269 (Claude Code)"),
            Some(EngineVersion::new(2, 1, 269))
        );
    }

    #[test]
    fn parses_the_version_last_shape() {
        assert_eq!(
            EngineVersion::parse("codex-cli 0.154.0"),
            Some(EngineVersion::new(0, 154, 0))
        );
    }

    #[test]
    fn rejects_output_with_no_version_shaped_token() {
        assert_eq!(EngineVersion::parse("not a version"), None);
        assert_eq!(EngineVersion::parse(""), None);
    }

    /// A dev/prerelease build (a CLI auto-updater's realistic in-between
    /// state) parses to its base release rather than being unparsable.
    #[test]
    fn parses_a_prerelease_or_build_suffixed_version() {
        assert_eq!(
            EngineVersion::parse("2.1.269-beta.1 (Claude Code)"),
            Some(EngineVersion::new(2, 1, 269))
        );
        assert_eq!(
            EngineVersion::parse("codex-cli 0.155.0-alpha.1"),
            Some(EngineVersion::new(0, 155, 0))
        );
        assert_eq!(
            EngineVersion::parse("1.2.3+build.7"),
            Some(EngineVersion::new(1, 2, 3))
        );
    }

    #[test]
    fn bounds_long_output_with_an_ellipsis() {
        let long = "x".repeat(MAX_OUTPUT_CHARS + 50);
        let shown = bounded(&long);
        assert_eq!(shown.chars().count(), MAX_OUTPUT_CHARS + 1);
        assert!(shown.ends_with('…'));

        let short = "short output";
        assert_eq!(bounded(short), short);
    }

    #[test]
    fn versions_compare_numerically_not_lexically() {
        assert!(EngineVersion::new(2, 1, 9) < EngineVersion::new(2, 1, 10));
        assert!(EngineVersion::new(0, 9, 0) < EngineVersion::new(0, 10, 0));
    }

    #[test]
    fn error_messages_name_the_remedy() {
        let too_old = DetectError::TooOld {
            program: "claude".into(),
            detected: EngineVersion::new(2, 0, 0),
            minimum: EngineVersion::new(2, 1, 219),
        }
        .to_string();
        assert!(
            too_old.contains("upgrade claude to >= 2.1.219"),
            "{too_old}"
        );

        let missing = DetectError::MissingCapability {
            program: "codex".into(),
            capability: "app-server".into(),
            detail: "exit status: 2".into(),
        }
        .to_string();
        assert!(missing.contains("app-server"), "{missing}");
        assert!(missing.contains("upgrade codex"), "{missing}");
    }
}
