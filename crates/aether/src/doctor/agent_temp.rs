use std::path::Path;

use super::{Check, Paths};

/// The root agents' temp dirs go under: one the daemon would refuse fails,
/// and one too long for sockets or held in memory warns.
pub fn check_agent_temp(paths: &Paths, configured: Option<&Path>) -> Check {
    let store = paths.store_path();
    let Some(root) = configured
        .map(Path::to_path_buf)
        .or_else(|| aether_core::agent_temp::default_root(&store))
    else {
        return Check::fail(
            "agent temp",
            format!(
                "{} has no absolute parent to hold agent temp dirs",
                store.display()
            ),
            "set agents.temp_root in config.toml to an absolute path",
        );
    };
    if let Err(error) = aether_core::agent_temp::inspect_root(&root) {
        return Check::fail(
            "agent temp",
            error.to_string(),
            "the daemon refuses to start until agents.temp_root is a private directory you own",
        );
    }
    let warnings = aether_core::agent_temp::warnings(&root);
    if !warnings.is_empty() {
        return Check::warn(
            "agent temp",
            warnings.join("; "),
            "set agents.temp_root in config.toml to a short, disk-backed path",
        );
    }
    Check::ok(
        "agent temp",
        format!("agents' temp dirs go under {}", root.display()),
    )
}
