//! The git branch of each workspace on screen, for the status line and the
//! board inspector. Read on tree changes and the status tick, never in render.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::App;

impl App {
    /// Re-read the branch of the cwd and of every agent's workspace.
    pub(super) fn refresh_branches(&mut self) {
        if self.options.is_remote() {
            return;
        }
        let paths: BTreeSet<PathBuf> = self
            .options
            .cwd
            .iter()
            .cloned()
            .chain(
                self.tree
                    .nodes
                    .values()
                    .filter_map(|node| node.agent.scope.workspace_root.clone()),
            )
            .collect();
        let branches: HashMap<PathBuf, Option<String>> = paths
            .into_iter()
            .map(|path| {
                let branch = branch_of(&path);
                (path, branch)
            })
            .collect();
        if branches != self.branches {
            self.branches = branches;
            self.dirty = true;
        }
    }

    /// The branch checked out at `path`, when it is on one.
    pub fn branch_at(&self, path: &Path) -> Option<&str> {
        self.branches.get(path)?.as_deref()
    }
}

/// `main` for a checkout of `main`; `None` when detached or outside a repository.
pub(crate) fn branch_of(dir: &Path) -> Option<String> {
    let dot_git = dir
        .ancestors()
        .map(|d| d.join(".git"))
        .find(|p| p.exists())?;
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        // A worktree or submodule: `.git` is a file naming the real git directory.
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let target = PathBuf::from(text.strip_prefix("gitdir:")?.trim());
        dot_git.parent()?.join(target)
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::branch_of;

    #[test]
    fn branches_come_from_head_in_checkouts_and_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/tui/main\n").unwrap();
        assert_eq!(
            branch_of(&repo.join("src/deep")).as_deref(),
            Some("tui/main")
        );

        let real = dir.path().join("gitdirs/wt");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("HEAD"), "ref: refs/heads/feature\n").unwrap();
        let worktree = dir.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", real.display()),
        )
        .unwrap();
        assert_eq!(branch_of(&worktree).as_deref(), Some("feature"));

        std::fs::write(real.join("HEAD"), "0123456789abcdef\n").unwrap();
        assert_eq!(branch_of(&worktree), None, "detached");
    }
}
