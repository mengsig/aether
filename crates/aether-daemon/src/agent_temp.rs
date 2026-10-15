//! The root of the agents' temp dirs, made ready before any agent runs.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

/// Resolve the configured root (or `t` beside the store), warn about what
/// would hurt agents using it, and claim it for this instance.
pub(crate) fn prepare(
    configured: Option<&Path>,
    store_path: &Path,
    instance_id: &str,
) -> anyhow::Result<PathBuf> {
    let root = match configured {
        Some(root) => root.to_path_buf(),
        None => aether_core::agent_temp::default_root(store_path).with_context(|| {
            format!(
                "store path {} has no absolute parent for agent temp dirs",
                store_path.display()
            )
        })?,
    };
    for warning in aether_core::agent_temp::warnings(&root) {
        tracing::warn!("{warning}");
    }
    aether_core::agent_temp::prepare_root(&root, instance_id)
        .context("cannot prepare the agents' temp dirs")?;
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
        let root = prepare(None, &store, "instance-a").unwrap();
        assert_eq!(root, dir.path().join("t"));
        assert_eq!(
            std::fs::read_to_string(root.join(aether_core::agent_temp::MARKER)).unwrap(),
            "instance-a"
        );
        assert_eq!(
            prepare(None, &store, "instance-a").unwrap(),
            root,
            "a restart reuses it"
        );
        assert!(prepare(None, Path::new("aether.db"), "instance-a").is_err());
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
        prepare(Some(&claimed), &store, "instance-a").unwrap();
        let error = prepare(Some(&claimed), &store, "instance-b").unwrap_err();
        assert!(
            format!("{error:#}").contains("used by another Aether instance"),
            "{error:#}"
        );
    }
}
