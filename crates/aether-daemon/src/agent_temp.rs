//! The root of the agents' temp dirs, made ready before any agent runs.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context as _;

/// The root of the agents' temp dirs, claimed for this instance. Only a
/// configured root stops startup: when `t` beside the store cannot be
/// prepared, agents keep the daemon's own temp dir, as before this existed.
pub(crate) fn prepare(
    configured: Option<&Path>,
    store_path: &Path,
    instance_id: &str,
) -> anyhow::Result<Option<PathBuf>> {
    debug_assert!(!instance_id.trim().is_empty(), "an instance id is required");
    if let Some(root) = configured {
        let root = claim(root, instance_id).context("cannot prepare agents.temp_root")?;
        return Ok(Some(root));
    }
    let default = aether_core::agent_temp::default_root(store_path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("store path {} has no absolute parent", store_path.display()),
        )
    });
    match default.and_then(|root| claim(&root, instance_id)) {
        Ok(root) => Ok(Some(root)),
        Err(error) => {
            tracing::warn!(
                "agents share the daemon's temp dir: the default agent temp root beside {} \
                 cannot be used ({error}); set agents.temp_root to a private directory you own",
                store_path.display()
            );
            Ok(None)
        }
    }
}

/// Normalize `root`, claim it, and warn about what would hurt agents using it.
fn claim(root: &Path, instance_id: &str) -> io::Result<PathBuf> {
    let root = aether_core::agent_temp::normalize_root(root)?;
    aether_core::agent_temp::prepare_root(&root, instance_id)?;
    for warning in aether_core::agent_temp::warnings(&root) {
        tracing::warn!("{warning}");
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn the_default_root_sits_beside_the_store_and_is_claimed() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("aether.db");
        let root = prepare(None, &store, "instance-a").unwrap().unwrap();
        assert_eq!(root, dir.path().join("t"));
        assert_eq!(
            std::fs::read_to_string(root.join(aether_core::agent_temp::MARKER)).unwrap(),
            "instance-a"
        );
        assert_eq!(
            prepare(None, &store, "instance-a").unwrap(),
            Some(root),
            "a restart reuses it"
        );
        assert_eq!(
            prepare(None, Path::new("aether.db"), "instance-a").unwrap(),
            None
        );
    }

    #[test]
    fn a_default_root_that_cannot_be_prepared_falls_back_to_the_daemon_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        let unwritable = prepare(None, &locked.join("aether.db"), "instance-a");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        // A superuser writes there regardless, and then the root is made.
        if !locked.join("t").exists() {
            assert_eq!(unwritable.unwrap(), None);
        }

        let foreign = dir.path().join("foreign");
        std::fs::create_dir(&foreign).unwrap();
        prepare(Some(&foreign.join("t")), &store_in(&foreign), "instance-b").unwrap();
        let claimed = prepare(None, &store_in(&foreign), "instance-a").unwrap();
        assert_eq!(
            claimed, None,
            "another instance's default root is left alone"
        );

        let error = prepare(
            Some(&foreign.join("t")),
            &store_in(dir.path()),
            "instance-a",
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("used by another Aether instance"),
            "a configured root still refuses startup: {error:#}"
        );
    }

    fn store_in(dir: &Path) -> PathBuf {
        dir.join("aether.db")
    }

    #[test]
    fn an_unsafe_or_foreign_root_refuses_startup() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("aether.db");
        let shared = dir.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        let error = prepare(Some(&shared), &store, "instance-a").unwrap_err();
        assert!(
            format!("{error:#}").contains("writable by group or others"),
            "{error:#}"
        );

        let claimed = dir.path().join("claimed");
        let trailing = PathBuf::from(format!("{}/", claimed.display()));
        assert_eq!(
            prepare(Some(&trailing), &store, "instance-a").unwrap(),
            Some(claimed.clone()),
            "a configured root is normalized"
        );
        let error = prepare(Some(&claimed), &store, "instance-b").unwrap_err();
        assert!(
            format!("{error:#}").contains("used by another Aether instance"),
            "{error:#}"
        );
    }
}
