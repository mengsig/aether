//! The Git directory a sandboxed turn needs to write to commit.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// Longer than `git rev-parse` takes on any healthy disk.
const GIT_QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// The repository's shared Git directory when it lies outside `cwd`, as for a
/// linked worktree: a commit there writes the worktree's index and HEAD under
/// it, and its objects and refs. `None` when `cwd` is in no repository, or
/// the directory is inside `cwd` and so already writable.
pub(super) async fn outside_common_dir(cwd: &Path) -> Option<PathBuf> {
    let query = tokio::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(GIT_QUERY_TIMEOUT, query).await {
        Ok(Ok(output)) if output.status.success() => output,
        // Not a repository: there is nothing to commit to.
        Ok(Ok(_)) => return None,
        Ok(Err(error)) => {
            tracing::warn!(cwd = %cwd.display(), %error, "cannot ask git for the workspace's Git directory; a sandboxed commit may fail");
            return None;
        }
        Err(_) => {
            tracing::warn!(cwd = %cwd.display(), "git did not name the workspace's Git directory in time; a sandboxed commit may fail");
            return None;
        }
    };
    let named = String::from_utf8_lossy(&output.stdout);
    resolve_outside(cwd, Path::new(named.trim()))
}

/// `common` resolved, unless it lies inside `cwd`.
fn resolve_outside(cwd: &Path, common: &Path) -> Option<PathBuf> {
    let resolved = |path: &Path| match std::fs::canonicalize(path) {
        Ok(path) => Some(path),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "cannot resolve a path for the sandbox's writable Git directory");
            None
        }
    };
    let common = resolved(common)?;
    let workspace = resolved(cwd)?;
    debug_assert!(common.is_absolute() && workspace.is_absolute());
    (!common.starts_with(&workspace)).then_some(common)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?}");
    }

    #[tokio::test]
    async fn only_a_linked_worktree_names_a_git_directory_outside_it() {
        let root = tempfile::tempdir().expect("tempdir");
        let main = root.path().join("main");
        std::fs::create_dir(&main).expect("main");
        git(&main, &["init", "-q"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "base"]);
        let linked = root.path().join("linked");
        git(
            &main,
            &["worktree", "add", "-q", linked.to_str().expect("utf-8")],
        );

        let common = outside_common_dir(&linked).await;
        assert_eq!(
            common,
            Some(std::fs::canonicalize(main.join(".git")).expect("canonical"))
        );
        assert_eq!(
            outside_common_dir(&main).await,
            None,
            "a plain checkout's Git directory is inside it"
        );
        let plain = root.path().join("plain");
        std::fs::create_dir(&plain).expect("plain");
        assert_eq!(outside_common_dir(&plain).await, None, "no repository");
    }
}
