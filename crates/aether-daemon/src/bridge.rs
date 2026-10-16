//! The executable engines launch as the MCP bridge: this daemon's own build,
//! whatever has replaced the installed binary since it started.

use std::io;
use std::path::{Path, PathBuf};

use aether_core::bridge_bin::{self, Published};
use anyhow::Context as _;

/// The bridge command, and the published copy it names when there is one.
#[derive(Debug)]
pub(crate) struct Bridge {
    command: PathBuf,
    published: Option<Published>,
}

impl Bridge {
    /// Publish this process's image beside the store. Without a copy the
    /// bridge still names the running image where the platform can.
    pub(crate) fn prepare(store_path: &Path) -> Option<Self> {
        match publish(store_path) {
            Ok(published) => {
                tracing::info!(
                    path = %published.path().display(),
                    linked = published.linked(),
                    "MCP bridge: this daemon's own build"
                );
                Some(Self {
                    command: published.path().to_path_buf(),
                    published: Some(published),
                })
            }
            Err(error) => {
                let command = fallback_command()?;
                tracing::warn!(
                    %error,
                    path = %command.display(),
                    "cannot publish this daemon's build for the MCP bridge; launching the fallback path"
                );
                Some(Self {
                    command,
                    published: None,
                })
            }
        }
    }

    pub(crate) fn command(&self) -> &Path {
        &self.command
    }

    /// Remove the published copy on a clean shutdown, once no turn runs.
    pub(crate) fn remove(self) -> anyhow::Result<()> {
        let Some(published) = self.published else {
            return Ok(());
        };
        let path = published.path().to_path_buf();
        published
            .remove()
            .with_context(|| format!("removing the bridge copy {}", path.display()))
    }
}

fn publish(store_path: &Path) -> io::Result<Published> {
    let dir = bridge_bin::default_dir(store_path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("store path {} has no absolute parent", store_path.display()),
        )
    })?;
    bridge_bin::publish(&dir, &running_image()?, env!("CARGO_PKG_VERSION"))
}

/// The image this process runs, even after its path was replaced.
#[cfg(target_os = "linux")]
fn running_image() -> io::Result<PathBuf> {
    Ok(PathBuf::from("/proc/self/exe"))
}

#[cfg(not(target_os = "linux"))]
fn running_image() -> io::Result<PathBuf> {
    std::env::current_exe()
}

/// The engine resolves the command, and its `/proc/self/exe` is the engine,
/// so name this daemon's process instead.
#[cfg(target_os = "linux")]
fn fallback_command() -> Option<PathBuf> {
    Some(PathBuf::from(format!("/proc/{}/exe", std::process::id())))
}

#[cfg(not(target_os = "linux"))]
fn fallback_command() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bridge_is_this_build_published_beside_the_store_and_removed_at_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::fs::canonicalize(dir.path()).unwrap().join("aether.db");
        let bridge = Bridge::prepare(&store).unwrap();
        let command = bridge.command().to_path_buf();
        assert_eq!(
            command.parent(),
            store.parent().map(|p| p.join("bin")).as_deref()
        );
        let name = command.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with(&format!("aether-{}-", env!("CARGO_PKG_VERSION"))),
            "{name}"
        );
        assert_eq!(
            std::fs::metadata(&command).unwrap().len(),
            std::fs::metadata(std::env::current_exe().unwrap())
                .unwrap()
                .len()
        );
        bridge.remove().unwrap();
        assert!(!command.exists());
    }

    #[test]
    fn a_store_whose_bin_dir_cannot_be_used_falls_back_to_the_running_image() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::fs::canonicalize(dir.path()).unwrap().join("aether.db");
        std::fs::write(store.with_file_name("bin"), "not a directory").unwrap();
        let bridge = Bridge::prepare(&store).unwrap();
        assert_eq!(bridge.command(), fallback_command().unwrap());
        if cfg!(target_os = "linux") {
            assert_eq!(
                std::fs::canonicalize(bridge.command()).unwrap(),
                std::fs::canonicalize("/proc/self/exe").unwrap(),
                "the daemon's own pid names the daemon's image"
            );
        }
        bridge.remove().unwrap();
    }
}
