//! Where Aether keeps its files. One place, so the daemon, the client and
//! the CLI never disagree.
//!
//! Environment overrides: `AETHER_CONFIG_DIR`, `AETHER_DATA_DIR`,
//! `AETHER_SOCKET`.

use std::path::{Path, PathBuf};

/// Configuration: `providers.toml`, `config.toml`, `credentials.json`.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AETHER_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("aether")
}

/// Durable state: the SQLite store, artifacts, logs.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AETHER_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("aether")
}

/// The store's filename inside the data directory. Named, so a caller with
/// its own data directory (a test, `aether doctor`) does not type it again.
pub const STORE_FILE_NAME: &str = "aether.db";

pub fn store_path() -> PathBuf {
    data_dir().join(STORE_FILE_NAME)
}

/// Where the daemon keeps the images sent with messages: beside its store,
/// resolved as the daemon resolves it.
pub fn attachment_dir_for(store: &Path) -> PathBuf {
    store.with_extension("attachments")
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// The daemon's Unix socket. Prefers the runtime dir so a reboot clears it.
pub fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("AETHER_SOCKET") {
        return PathBuf::from(path);
    }
    if let Some(runtime) = dirs::runtime_dir() {
        return runtime.join("aether").join("aetherd.sock");
    }
    data_dir().join("aetherd.sock")
}

/// Where the daemon records its pid while running.
pub fn pid_path() -> PathBuf {
    pid_path_for(&socket_path())
}

/// The pid file beside a given socket. A daemon on its own socket must never
/// write the default socket's pid file.
pub fn pid_path_for(socket: &Path) -> PathBuf {
    socket.with_extension("pid")
}

/// Where a daemon started in the background leaves the failure that stopped
/// it. A detached daemon has no terminal, so a failure that kills it before
/// it listens would otherwise be lost; the client that started it clears
/// this file before spawning, then reads it back and reports it.
pub fn startup_log_path_for(socket: &Path) -> PathBuf {
    socket.with_extension("startup.log")
}
