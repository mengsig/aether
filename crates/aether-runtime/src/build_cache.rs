//! Where an agent's builds keep their output: a directory its client names,
//! or a partition of Aether's cache per repository and run. The agents of one
//! run share a partition, so its isolated children build warm; another run is
//! another trust domain and never gets the same directory.

use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Component, Path, PathBuf};

use aether_core::agent::{Agent, AgentConfig, ExecutionBackend};
use aether_core::event::{Actor, OperatorOrigin};
use aether_core::id::RunId;
use sha2::Digest as _;

use crate::{Result, Runtime, RuntimeError};

/// Trailing characters of the repository id kept readable in its directory
/// name: the end of a URL or path is the part that names the project.
const SLUG_LIMIT: usize = 48;

/// The partition the builds of `repository` in `run` use under `root`.
pub(crate) fn partition(root: &Path, repository: &str, run: RunId) -> PathBuf {
    debug_assert!(root.is_absolute(), "{} is not absolute", root.display());
    debug_assert!(!repository.is_empty(), "a partition needs a repository");
    root.join(repository_dir(repository)).join(run.to_string())
}

/// A readable directory name for a repository id that no other id shares:
/// the id made path-safe, then a digest of the id as given.
fn repository_dir(repository: &str) -> String {
    let safe: String = repository
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    // Only ASCII is left, so any byte offset is a character boundary.
    let slug = &safe[safe.len().saturating_sub(SLUG_LIMIT)..];
    let slug = slug.trim_matches('-');
    let digest = hex::encode(sha2::Sha256::digest(repository.as_bytes()));
    let name = if slug.is_empty() {
        digest[..16].to_owned()
    } else {
        format!("{slug}-{}", &digest[..16])
    };
    debug_assert!(
        !name.contains('/') && !name.starts_with(['.', '-']),
        "{name} is not one plain directory name"
    );
    name
}

/// The variables `agent`'s processes get and the directories they name;
/// nothing is created. Empty without a repository workspace or a client's
/// directory, and in a container, which does not mount the cache.
pub(crate) fn exports(runtime: &Runtime, agent: &Agent) -> Vec<(String, PathBuf)> {
    if agent.scope.backend != ExecutionBackend::Host {
        return Vec::new();
    }
    let Some(base) = base_dir(runtime, agent) else {
        return Vec::new();
    };
    debug_assert!(base.is_absolute(), "{} is not absolute", base.display());
    runtime
        .models
        .config()
        .build_cache
        .env
        .iter()
        .map(|var| (var.name.clone(), base.join(&var.dir)))
        .collect()
}

fn base_dir(runtime: &Runtime, agent: &Agent) -> Option<PathBuf> {
    if let Some(dir) = &agent.config.build_cache {
        return Some(dir.clone());
    }
    let root = runtime.config.build_cache_root.as_deref()?;
    let repository = agent.scope.repository.as_deref()?;
    agent.scope.workspace_root.as_ref()?;
    Some(partition(root, repository, agent.run))
}

/// [`exports`], each directory created now because one of the agent's
/// processes is about to start. A directory that cannot be made stops it.
pub(crate) fn prepare(runtime: &Runtime, agent: &Agent) -> Result<Vec<(String, PathBuf)>> {
    let exports = exports(runtime, agent);
    for (name, dir) in &exports {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| {
                RuntimeError::Invalid(format!("build cache {name} {}: {e}", dir.display()))
            })?;
    }
    Ok(exports)
}

/// Settle a new agent's client-named cache: an omitted one is its parent's,
/// a new one is checked. A paired device names no host directory for the
/// daemon to make writable.
pub(crate) fn settle(config: &mut AgentConfig, parent: Option<&Agent>, by: &Actor) -> Result<()> {
    let inherited = parent.and_then(|p| p.config.build_cache.as_ref());
    let Some(dir) = &config.build_cache else {
        config.build_cache = inherited.cloned();
        return Ok(());
    };
    if Some(dir) == inherited {
        return Ok(());
    }
    if matches!(
        by,
        Actor::User {
            origin: Some(OperatorOrigin::Device { .. }),
            ..
        }
    ) {
        return Err(RuntimeError::Invalid(
            "a paired device cannot name a build_cache directory".into(),
        ));
    }
    check_client_dir(dir)
}

/// A client's directory becomes an environment value and a writable sandbox
/// root, so it must name one place: absolute, plain and UTF-8.
fn check_client_dir(dir: &Path) -> Result<()> {
    let shown = dir.display();
    let refuse = |why: &str| {
        Err(RuntimeError::Invalid(format!(
            "build_cache `{shown}` {why}"
        )))
    };
    if dir.to_str().is_none() {
        return refuse("is not valid UTF-8");
    }
    if !dir.is_absolute() {
        return refuse("must be an absolute path");
    }
    if dir.components().any(|c| c == Component::ParentDir) {
        return refuse("must not contain `..`");
    }
    if dir.parent().is_none() {
        return refuse("cannot be the filesystem root");
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_partition_is_per_repository_and_run() {
        let root = Path::new("/srv/cache");
        let (one, two) = (RunId::new(), RunId::new());
        let repo = "github.com/example-org/example-repo";
        let first = partition(root, repo, one);
        assert_eq!(first, partition(root, repo, one));
        assert_ne!(first, partition(root, repo, two), "another run");
        assert_ne!(first, partition(root, "github.com/example-org/other", one));
        assert_eq!(first.parent(), partition(root, repo, two).parent());
        let name = first
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            name.starts_with("github-com-example-org-example-repo-"),
            "{name}"
        );
    }

    #[test]
    fn repository_names_stay_plain_and_distinct() {
        let long = format!("/home/user/{}/project", "x".repeat(200));
        let names = ["a/b", "a-b", "../..", ".", "/srv/ünïcode", &long].map(repository_dir);
        for name in &names {
            assert_eq!(Path::new(name).components().count(), 1, "{name}");
            assert!(!name.starts_with(['.', '-']), "{name}");
            assert!(name.len() <= SLUG_LIMIT + 17, "{name}");
        }
        assert_ne!(names[0], names[1], "same slug, different ids");
        assert!(
            names[5].contains("xxx-project-"),
            "the name keeps the end: {}",
            names[5]
        );
    }

    #[test]
    fn a_client_dir_must_be_one_absolute_plain_path() {
        assert!(check_client_dir(Path::new("/srv/items/one")).is_ok());
        for dir in ["relative/cache", "/srv/../cache", "/", "~/cache"] {
            assert!(check_client_dir(Path::new(dir)).is_err(), "{dir}");
        }
    }
}
