//! Explicit daemon destinations, including SSH profiles with no credentials.

use crate::{Client, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MAX_REMOTE_NAME_CHARS: usize = 32;
pub const BRIDGE_COMMAND: [&str; 3] = ["aether", "bridge", "--stdio"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshRemote {
    #[serde(skip)]
    name: String,
    #[serde(rename = "ssh")]
    target: String,
    #[serde(default = "default_executable")]
    executable: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    socket: Option<String>,
}

fn default_executable() -> String {
    "aether".into()
}

impl SshRemote {
    pub fn new(name: &str, target: &str) -> std::result::Result<Self, String> {
        Self::configured(name, target, "aether", None)
    }

    pub fn configured(
        name: &str,
        target: &str,
        executable: &str,
        socket: Option<&str>,
    ) -> std::result::Result<Self, String> {
        if name.is_empty()
            || name.len() > MAX_REMOTE_NAME_CHARS
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        {
            return Err(format!(
                "a remote name is 1 to {MAX_REMOTE_NAME_CHARS} characters of a-z, 0-9, - and _"
            ));
        }
        if target.is_empty()
            || target.starts_with('-')
            || target.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(
                "invalid ssh destination: empty, option-like or containing whitespace".into(),
            );
        }
        for value in std::iter::once(executable).chain(socket) {
            if value.is_empty() || value.chars().any(char::is_control) {
                return Err("remote executable and socket must be nonempty paths without control characters".into());
            }
        }
        Ok(Self {
            name: name.into(),
            target: target.into(),
            executable: executable.into(),
            socket: socket.map(str::to_owned),
        })
    }

    pub fn validated(self, name: &str) -> std::result::Result<Self, String> {
        Self::configured(name, &self.target, &self.executable, self.socket.as_deref())
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn command(&self) -> Vec<String> {
        let mut remote = vec![quote(&self.executable)];
        if let Some(socket) = &self.socket {
            remote.extend(["--socket".into(), quote(socket)]);
        }
        remote.extend(["--local".into(), "bridge".into(), "--stdio".into()]);
        vec![
            "ssh".into(),
            "-T".into(),
            "-oBatchMode=yes".into(),
            "-oServerAliveInterval=15".into(),
            "-oServerAliveCountMax=3".into(),
            "-oConnectTimeout=10".into(),
            "--".into(),
            self.target.clone(),
            remote.join(" "),
        ]
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Socket(PathBuf),
    Ssh(SshRemote),
}

impl From<PathBuf> for Endpoint {
    fn from(path: PathBuf) -> Self {
        Self::Socket(path)
    }
}
impl From<&PathBuf> for Endpoint {
    fn from(path: &PathBuf) -> Self {
        Self::Socket(path.clone())
    }
}
impl From<&Path> for Endpoint {
    fn from(path: &Path) -> Self {
        Self::Socket(path.to_owned())
    }
}
impl From<&Endpoint> for Endpoint {
    fn from(endpoint: &Endpoint) -> Self {
        endpoint.clone()
    }
}
impl Endpoint {
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Ssh(_))
    }
    pub fn local_path(&self) -> Option<&Path> {
        match self {
            Self::Socket(path) => Some(path),
            Self::Ssh(_) => None,
        }
    }
    pub fn command_hint(&self, explicit: bool) -> String {
        match self {
            Self::Socket(path) => aether_protocol::command_hint::command_hint(path, explicit),
            Self::Ssh(remote) => format!("aether --remote {}", remote.name()),
        }
    }
    pub fn identity(&self) -> String {
        match self {
            Self::Socket(path) => format!("socket:{}", path.display()),
            Self::Ssh(remote) => format!(
                "ssh:{:?}",
                (
                    &remote.name,
                    &remote.target,
                    &remote.executable,
                    &remote.socket
                )
            ),
        }
    }
    pub fn state_dir(&self, instance_id: &str) -> PathBuf {
        use sha2::{Digest as _, Sha256};
        let identity = self.identity();
        debug_assert!(!identity.is_empty());
        debug_assert!(!instance_id.is_empty());
        let mut hash = Sha256::new();
        hash.update((identity.len() as u64).to_be_bytes());
        hash.update(identity.as_bytes());
        hash.update(instance_id.as_bytes());
        let digest = hash.finalize();
        aether_protocol::paths::config_dir()
            .join("endpoints")
            .join(hex::encode(digest))
    }
    pub fn display(&self) -> String {
        match self {
            Self::Socket(path) => path.display().to_string(),
            Self::Ssh(remote) => format!("{} ({})", remote.name, remote.target),
        }
    }
}

impl Client {
    pub async fn connect_endpoint(endpoint: &Endpoint) -> Result<Client> {
        match endpoint {
            Endpoint::Socket(path) => Client::connect(path).await,
            Endpoint::Ssh(remote) => crate::ssh::connect(remote).await,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    #[test]
    fn shell_paths_are_quoted_and_ssh_policy_is_preserved() {
        let remote = SshRemote::configured("box", "me@box", "/aether's $bin", Some("/a b/socket"))
            .expect("profile");
        let command = remote.command();
        assert_eq!(&command[6..8], &["--", "me@box"]);
        assert!(
            command
                .last()
                .expect("remote command")
                .contains("'/aether'\\''s $bin' --socket '/a b/socket'")
        );
        assert!(command.contains(&"-oBatchMode=yes".into()));
        assert!(
            !command
                .iter()
                .any(|arg| arg.contains("StrictHostKeyChecking"))
        );
    }
    #[test]
    fn malformed_names_and_targets_are_refused() {
        for target in ["-oProxyCommand=sh", "", "host name", "host\n"] {
            assert!(SshRemote::new("box", target).is_err());
        }
        for name in ["", "Box", "a b", &"x".repeat(33)] {
            assert!(SshRemote::new(name, "host").is_err());
        }
    }

    #[test]
    fn state_is_separate_for_profiles_host_paths_and_daemon_instances() {
        let first =
            Endpoint::Ssh(SshRemote::configured("box", "host:x", "y", None).expect("profile"));
        let second =
            Endpoint::Ssh(SshRemote::configured("box", "host", "x:y", None).expect("profile"));
        assert_ne!(first.state_dir("instance"), second.state_dir("instance"));
        assert_ne!(first.state_dir("old"), first.state_dir("new"));
    }
}
