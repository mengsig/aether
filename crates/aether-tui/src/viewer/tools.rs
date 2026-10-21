//! Optional document tools, run as argv-only child processes with a deadline.
use anyhow::{Context, Result, bail, ensure};
use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use std::{
    ffi::OsStr,
    fs::File,
    io::Read,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const MAX_STDOUT_BYTES: u64 = 1024 * 1024;
const MAX_STDERR_BYTES: u64 = 2048;
const CHROMIUM: &[&str] = &[
    "chromium",
    "chromium-browser",
    "google-chrome",
    "google-chrome-stable",
];

/// Absolute paths of the installed renderers, found once on `PATH`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tools {
    pub pdftoppm: Option<PathBuf>,
    pub pdfinfo: Option<PathBuf>,
    pub pdftotext: Option<PathBuf>,
    pub mutool: Option<PathBuf>,
    pub chromium: Option<PathBuf>,
}

impl Tools {
    pub fn detect() -> Self {
        let path = std::env::var_os("PATH").unwrap_or_default();
        Self::search(&path)
    }

    pub fn search(path: &OsStr) -> Self {
        let dirs: Vec<PathBuf> = std::env::split_paths(path)
            .filter(|dir| dir.is_absolute())
            .collect();
        let find = |name: &str| {
            dirs.iter()
                .map(|dir| dir.join(name))
                .find(|candidate| is_executable(candidate))
        };
        Self {
            pdftoppm: find("pdftoppm"),
            pdfinfo: find("pdfinfo"),
            pdftotext: find("pdftotext"),
            mutool: find("mutool"),
            chromium: CHROMIUM.iter().find_map(|name| find(name)),
        }
    }
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Run `command` in `dir` with stdin closed; stdout is returned, bounded to 1 MiB.
/// The whole process group is killed at the deadline and after exit.
pub(super) fn run(mut command: Command, dir: &Path, timeout: Duration) -> Result<Vec<u8>> {
    let program = Path::new(command.get_program());
    ensure!(program.is_absolute(), "tool path must be absolute");
    ensure!(dir.is_dir(), "tool working directory is missing");
    let name = program
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stdout_path = dir.join(".stdout");
    let stderr_path = dir.join(".stderr");
    let mut child = command
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(File::create(&stdout_path).context("create tool stdout")?)
        .stderr(File::create(&stderr_path).context("create tool stderr")?)
        .process_group(0)
        .spawn()
        .with_context(|| format!("start {name}"))?;
    let group = Pid::from_raw(i32::try_from(child.id()).context("child pid")?);
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().context("wait for tool")? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    kill_group(group)?;
    let Some(status) = status else {
        child.wait().context("reap timed out tool")?;
        bail!("{name} timed out after {} s", timeout.as_secs());
    };
    if !status.success() {
        let stderr = read_bounded(&stderr_path, MAX_STDERR_BYTES)?;
        let detail = crate::ui::cells::text::clean(String::from_utf8_lossy(&stderr).trim());
        bail!("{name} failed ({status}): {detail}");
    }
    read_bounded(&stdout_path, MAX_STDOUT_BYTES)
}

/// Stragglers such as browser helpers must not outlive the render.
fn kill_group(group: Pid) -> Result<()> {
    match killpg(group, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        // macOS reports EPERM for a group whose members are all zombies.
        Err(Errno::EPERM) if cfg!(target_os = "macos") => Ok(()),
        Err(error) => Err(error).context("stop tool process group"),
    }
}

/// Read at most `limit` bytes; longer files are cut rather than refused.
pub(super) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(limit)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {}", path.display()))?;
    debug_assert!(bytes.len() as u64 <= limit);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn search_finds_only_executables_on_absolute_path_entries() {
        let dir = tempfile::tempdir().unwrap();
        script(dir.path(), "pdftoppm", "exit 0");
        std::fs::write(dir.path().join("pdftotext"), "not executable").unwrap();
        script(dir.path(), "chromium-browser", "exit 0");
        let path = std::env::join_paths([Path::new("relative"), dir.path()]).unwrap();
        let tools = Tools::search(&path);
        assert_eq!(tools.pdftoppm, Some(dir.path().join("pdftoppm")));
        assert_eq!(tools.pdftotext, None);
        assert_eq!(tools.chromium, Some(dir.path().join("chromium-browser")));
        assert_eq!(Tools::search(OsStr::new("relative")), Tools::default());
    }

    #[test]
    fn arguments_are_not_shell_interpreted_and_output_is_returned() {
        let dir = tempfile::tempdir().unwrap();
        let echo = script(dir.path(), "echo-args", r#"printf '%s|' "$@""#);
        let mut command = Command::new(echo);
        command.args(["$(touch pwned)", "a b"]);
        let out = run(command, dir.path(), Duration::from_secs(5)).unwrap();
        assert_eq!(out, b"$(touch pwned)|a b|");
        assert!(!dir.path().join("pwned").exists());
    }

    #[test]
    fn failures_carry_stderr_and_hung_tools_are_killed_at_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let failing = script(dir.path(), "fails", "echo broken page >&2; exit 3");
        let error = run(Command::new(failing), dir.path(), Duration::from_secs(5)).unwrap_err();
        assert!(format!("{error:#}").contains("broken page"), "{error:#}");
        let hung = script(dir.path(), "hangs", "sleep 30 & sleep 30");
        let started = Instant::now();
        let error = run(Command::new(hung), dir.path(), Duration::from_millis(200)).unwrap_err();
        assert!(format!("{error:#}").contains("timed out"), "{error:#}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
