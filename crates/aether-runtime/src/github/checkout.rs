//! Managed, per-thread Git checkouts for automatic GitHub dispatch.
//! The checkout root lives beside the daemon database, not inside whichever
//! project happened to start the daemon.

use crate::{Runtime, RuntimeError};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, process::Command};

const CLONE_TIMEOUT: Duration = Duration::from_secs(90);
const GIT_TIMEOUT: Duration = Duration::from_secs(10);
const REAP_TIMEOUT: Duration = Duration::from_secs(5);
const STDERR_LIMIT: u64 = 8192;

struct ProcessGroup(Option<Pid>);

impl ProcessGroup {
    fn disarm(&mut self) {
        self.0 = None;
    }

    fn stop(&mut self) {
        if let Some(pid) = self.0.take()
            && let Err(error) = killpg(pid, Signal::SIGKILL)
            && error != nix::errno::Errno::ESRCH
        {
            tracing::warn!(%error, "cannot stop GitHub clone process group");
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.stop();
    }
}

// A poll can be cancelled while clone is in flight. Keep the child and its
// incomplete directory together so cancellation cannot leave a live Git
// helper writing into a checkout that a later poll might install.
struct CloneGuard {
    child: tokio::process::Child,
    group: ProcessGroup,
    temporary: PathBuf,
}

impl Drop for CloneGuard {
    fn drop(&mut self) {
        if self.group.0.is_none() {
            return;
        }
        match self.child.try_wait() {
            Ok(Some(_)) => {
                self.group.disarm();
                remove_temporary(&self.temporary);
                return;
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "cannot inspect cancelled GitHub clone"),
        }
        self.group.stop();
        let started = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    remove_temporary(&self.temporary);
                    return;
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%error, path = %self.temporary.display(), "cannot reap cancelled GitHub clone; incomplete checkout retained");
                    return;
                }
            }
            if started.elapsed() >= REAP_TIMEOUT {
                tracing::warn!(path = %self.temporary.display(), "cancelled GitHub clone did not exit; incomplete checkout retained");
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn failure(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Invalid(message.into())
}

fn directory(path: &Path) -> crate::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(failure(format!(
            "GitHub checkout path {} is not a real directory",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(path).map_err(|e| {
                failure(format!(
                    "cannot create GitHub checkout directory {}: {e}",
                    path.display()
                ))
            })?;
            directory(path)
        }
        Err(error) => Err(failure(format!(
            "cannot inspect GitHub checkout path {}: {error}",
            path.display()
        ))),
    }
}

fn existing_directory(path: &Path) -> crate::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(failure(format!(
            "GitHub checkout path {} is not a real directory",
            path.display()
        ))),
        Err(error) => Err(failure(format!(
            "cannot inspect GitHub checkout path {}: {error}",
            path.display()
        ))),
    }
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

fn remove_temporary(path: &Path) {
    if let Err(error) = std::fs::remove_dir_all(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %error, "cannot remove incomplete GitHub checkout");
    }
}

async fn stop_clone(
    group: &mut ProcessGroup,
    child: &mut tokio::process::Child,
    temporary: &Path,
) -> crate::Result<()> {
    if child
        .try_wait()
        .map_err(|error| failure(format!("cannot inspect GitHub clone exit: {error}")))?
        .is_some()
    {
        group.disarm();
        remove_temporary(temporary);
        return Ok(());
    }
    group.stop();
    match tokio::time::timeout(REAP_TIMEOUT, child.wait()).await {
        Ok(Ok(_)) => {
            remove_temporary(temporary);
            Ok(())
        }
        Ok(Err(error)) => Err(failure(format!(
            "GitHub clone stopped but could not be reaped: {error}; incomplete checkout left at {}",
            temporary.display()
        ))),
        Err(_) => Err(failure(format!(
            "GitHub clone did not exit after being stopped; incomplete checkout left at {}",
            temporary.display()
        ))),
    }
}

fn clone_failure(status: Option<std::process::ExitStatus>, stderr: &[u8]) -> RuntimeError {
    let mut detail =
        aether_core::text::strip_controls(&String::from_utf8_lossy(stderr)).into_owned();
    for name in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(token) = std::env::var(name)
            && !token.is_empty()
        {
            detail = detail.replace(&token, "[redacted]");
        }
    }
    let detail = detail.trim();
    let message = match status {
        Some(status) => format!("GitHub checkout clone failed with {status}"),
        None => "GitHub checkout clone failed".into(),
    };
    if detail.is_empty() {
        failure(format!(
            "{message}; verify git, gh auth and repository access"
        ))
    } else {
        failure(format!("{message}: {detail}"))
    }
}

fn git() -> Command {
    let mut command = Command::new("git");
    // A daemon started from a Git hook must not let inherited repository
    // selectors redirect clone or origin verification into another tree.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    aether_spawn::detach_session(command.as_std_mut());
    command
}

async fn check_existing(path: &Path, remote: &str) -> crate::Result<()> {
    existing_directory(path)?;
    let git_dir = path.join(".git");
    let metadata = std::fs::symlink_metadata(&git_dir).map_err(|_| {
        failure(format!(
            "GitHub checkout {} has no .git directory; move it aside before retrying",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_dir() {
        return Err(failure(format!(
            "GitHub checkout {} has an unsafe .git path",
            path.display()
        )));
    }
    let mut command = git();
    command
        .stdout(Stdio::piped())
        .arg("-C")
        .arg(path)
        .args(["remote", "get-url", "origin"]);
    let mut child = command
        .spawn()
        .map_err(|e| failure(format!("cannot check GitHub checkout origin: {e}")))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| failure("git stdout unavailable"))?;
    let operation = async {
        let mut output = Vec::new();
        (&mut stdout)
            .take(1025)
            .read_to_end(&mut output)
            .await
            .map_err(|e| failure(format!("cannot read GitHub checkout origin: {e}")))?;
        if output.len() > 1024 {
            return Err(failure("GitHub checkout origin exceeds output limit"));
        }
        let status = child
            .wait()
            .await
            .map_err(|e| failure(format!("cannot wait for GitHub checkout origin: {e}")))?;
        Ok::<_, RuntimeError>((status, output))
    };
    let (status, output) = tokio::time::timeout(GIT_TIMEOUT, operation)
        .await
        .map_err(|_| failure("checking GitHub checkout origin timed out"))??;
    if !status.success() {
        return Err(failure(format!(
            "cannot verify GitHub checkout origin at {}",
            path.display()
        )));
    }
    if String::from_utf8_lossy(&output).trim() != remote {
        return Err(failure(format!(
            "GitHub checkout {} has a different origin; move it aside before retrying",
            path.display()
        )));
    }
    Ok(())
}

fn checkout_path(
    runtime: &Runtime,
    repository: &str,
    number: u64,
    generation: u64,
    create: bool,
) -> crate::Result<PathBuf> {
    let root = runtime
        .config
        .github_checkout_root
        .as_ref()
        .ok_or_else(|| failure("automatic GitHub dispatch needs a managed checkout root"))?;
    let (owner, name) = repository
        .split_once('/')
        .ok_or_else(|| failure("invalid configured GitHub repository"))?;
    if number == 0 || !safe_component(owner) || !safe_component(name) {
        return Err(failure("invalid GitHub thread checkout identifier"));
    }
    let ensure = if create {
        directory
    } else {
        existing_directory
    };
    ensure(root)?;
    let root = std::fs::canonicalize(root)
        .map_err(|e| failure(format!("cannot resolve GitHub checkout root: {e}")))?;
    let owner_dir = root.join(owner);
    ensure(&owner_dir)?;
    let repository_dir = owner_dir.join(name);
    ensure(&repository_dir)?;
    let thread_dir = repository_dir.join(number.to_string());
    ensure(&thread_dir)?;
    Ok(thread_dir.join(format!("g{generation}")))
}

pub(super) async fn verify(
    runtime: &Runtime,
    repository: &str,
    number: u64,
    generation: u64,
) -> crate::Result<PathBuf> {
    let target = checkout_path(runtime, repository, number, generation, false)?;
    let remote = format!("https://github.com/{repository}.git");
    check_existing(&target, &remote).await?;
    Ok(target)
}

pub(super) async fn prepare(
    runtime: &Runtime,
    repository: &str,
    number: u64,
    generation: u64,
) -> crate::Result<PathBuf> {
    let target = checkout_path(runtime, repository, number, generation, true)?;
    let repository_dir = target
        .parent()
        .ok_or_else(|| failure("GitHub checkout has no thread directory"))?;
    let remote = format!("https://github.com/{repository}.git");
    match std::fs::symlink_metadata(&target) {
        Ok(_) => {
            check_existing(&target, &remote).await?;
            return Ok(target);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(failure(format!("cannot inspect GitHub checkout: {error}"))),
    }
    let temporary = repository_dir.join(format!(".clone-{}", ulid::Ulid::generate()));
    let mut command = git();
    command.stderr(Stdio::piped());
    command
        .args([
            "-c",
            "credential.helper=",
            "-c",
            "credential.helper=!gh auth git-credential",
            "-c",
            "protocol.file.allow=never",
            "clone",
            "--quiet",
            "--depth",
            "1",
            "--single-branch",
            "--",
            &remote,
        ])
        .arg(&temporary);
    let child = command
        .spawn()
        .map_err(|error| failure(format!("cannot start GitHub checkout clone: {error}")))?;
    let mut clone = CloneGuard {
        group: ProcessGroup(child.id().map(|id| Pid::from_raw(id as i32))),
        child,
        temporary: temporary.clone(),
    };
    let mut stderr = clone
        .child
        .stderr
        .take()
        .ok_or_else(|| failure("git stderr unavailable"))?;
    let operation = async {
        let mut output = Vec::new();
        (&mut stderr)
            .take(STDERR_LIMIT + 1)
            .read_to_end(&mut output)
            .await
            .map_err(|e| failure(format!("cannot read GitHub checkout clone error: {e}")))?;
        if output.len() as u64 > STDERR_LIMIT {
            return Err(failure("GitHub checkout clone error exceeds output limit"));
        }
        let status = clone
            .child
            .wait()
            .await
            .map_err(|e| failure(format!("cannot wait for GitHub clone: {e}")))?;
        Ok::<_, RuntimeError>((status, output))
    };
    let result = tokio::time::timeout(CLONE_TIMEOUT, operation).await;
    let (status, stderr) = match result {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            stop_clone(&mut clone.group, &mut clone.child, &temporary).await?;
            return Err(error);
        }
        Err(_) => {
            stop_clone(&mut clone.group, &mut clone.child, &temporary).await?;
            return Err(failure("GitHub checkout clone timed out after 90 seconds"));
        }
    };
    clone.group.disarm();
    if !status.success() {
        remove_temporary(&temporary);
        return Err(clone_failure(Some(status), &stderr));
    }
    match std::fs::rename(&temporary, &target) {
        Ok(()) => Ok(target),
        Err(error) => {
            remove_temporary(&temporary);
            if std::fs::symlink_metadata(&target).is_ok() {
                check_existing(&target, &remote).await?;
                Ok(target)
            } else {
                Err(failure(format!("cannot install GitHub checkout: {error}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_clone_reaps_and_removes_its_incomplete_directory() {
        let root = tempfile::tempdir().unwrap();
        let temporary = root.path().join(".clone-interrupted");
        std::fs::create_dir(&temporary).unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "exec /bin/sleep 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .process_group(0);
        let child = command.spawn().unwrap();
        let clone = CloneGuard {
            group: ProcessGroup(child.id().map(|id| Pid::from_raw(id as i32))),
            child,
            temporary: temporary.clone(),
        };
        drop(clone);
        assert!(!temporary.exists());
    }
}
