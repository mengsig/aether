use std::path::{Path, PathBuf};

use super::{Check, Paths};

const HINT_ABSOLUTE: &str =
    "add to config.toml an absolute path, e.g. [agents] temp_root = \"/var/tmp/ae\"";
const HINT_PRIVATE: &str =
    "add to config.toml a private directory you own, e.g. [agents] temp_root = \"/var/tmp/ae\"";
const HINT_SHORT: &str =
    "add to config.toml a short, disk-backed path, e.g. [agents] temp_root = \"/var/tmp/ae\"";

/// The root agents' temp dirs go under. A configured root the daemon would
/// refuse fails, since it stops startup; a default one warns, since the daemon
/// then leaves agents its own temp dir. A long or in-memory root warns.
pub fn check_agent_temp(
    paths: &Paths,
    configured: Option<&Path>,
    instance_id: Option<&str>,
) -> Check {
    let store = resolved_store(&paths.store_path());
    let Some(root) = configured
        .map(Path::to_path_buf)
        .or_else(|| aether_core::agent_temp::default_root(&store))
    else {
        return Check::warn(
            "agent temp",
            format!(
                "{} has no parent to hold agent temp dirs; agents use the daemon's temp dir",
                store.display()
            ),
            HINT_ABSOLUTE,
        );
    };
    let root = match (
        aether_core::agent_temp::inspect_root(&root, instance_id),
        configured,
    ) {
        (Ok(root), _) => root,
        (Err(error), Some(_)) => {
            return Check::fail(
                "agent temp",
                error.to_string(),
                "the daemon refuses to start until agents.temp_root is a private directory you own",
            );
        }
        (Err(error), None) => {
            return Check::warn(
                "agent temp",
                format!("{error}; agents use the daemon's temp dir instead"),
                HINT_PRIVATE,
            );
        }
    };
    let warnings = aether_core::agent_temp::warnings(&root);
    if !warnings.is_empty() {
        return Check::warn("agent temp", warnings.join("; "), HINT_SHORT);
    }
    Check::ok(
        "agent temp",
        format!("agents' temp dirs go under {}", root.display()),
    )
}

/// The store as the daemon names it: resolved through links, or, before it
/// exists, in its resolved directory. Lexically absolute if neither exists.
fn resolved_store(store: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(store) {
        return resolved;
    }
    let (Some(dir), Some(name)) = (store.parent(), store.file_name()) else {
        return store.to_path_buf();
    };
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    match std::fs::canonicalize(dir) {
        Ok(dir) => dir.join(name),
        Err(_) => std::path::absolute(store).unwrap_or_else(|_| store.to_path_buf()),
    }
}
