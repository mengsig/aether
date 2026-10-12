use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use super::MIN_IDLE_TIMEOUT_SECS;

/// The fixed half of remote control: where the listener binds, and its limits.
/// What is enabled is changed live with `aether remote`. See docs/remote.md.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteSection {
    /// `host:port` the listener binds once enabled.
    pub address: String,
    /// Bind outside loopback, acknowledging that Aether terminates no TLS.
    pub allow_remote: bool,
    /// Connections served at once; more wait in the kernel's backlog.
    pub max_connections: usize,
    /// How long a WebSocket may go without sending a frame.
    pub idle_timeout_secs: u64,
    /// Let a push notification carry a short preview of what an agent wrote.
    pub push_previews: bool,
    /// The VAPID contact, `mailto:` or `https:`. Empty derives it from the
    /// first public origin.
    pub push_subject: String,
    /// Push services a device may subscribe with, beyond the browsers' own:
    /// exact host names or `*.suffix`. The daemon checks them at start.
    pub push_hosts: Vec<String>,
}

/// Safe rather than convenient: loopback only, and no content in a payload
/// that crosses a push service.
impl Default for RemoteSection {
    fn default() -> Self {
        Self {
            address: "127.0.0.1:8377".into(),
            allow_remote: false,
            max_connections: 32,
            idle_timeout_secs: 300,
            push_previews: false,
            push_subject: String::new(),
            push_hosts: Vec::new(),
        }
    }
}

impl RemoteSection {
    /// Where the listener binds, once every value here means what it says.
    pub fn address(&self) -> Result<SocketAddr, String> {
        let address = self.address.trim();
        let parsed: SocketAddr = address.parse().map_err(|_| {
            format!("[remote] address `{address}` is not a host:port, e.g. \"127.0.0.1:8377\"")
        })?;
        if self.max_connections == 0 {
            return Err("[remote] max_connections must be at least 1".into());
        }
        if self.idle_timeout_secs < MIN_IDLE_TIMEOUT_SECS {
            return Err(format!(
                "[remote] idle_timeout_secs must be at least {MIN_IDLE_TIMEOUT_SECS}; below that \
                 a client's keepalive cannot hold a connection open"
            ));
        }
        if !parsed.ip().is_loopback() && !self.allow_remote {
            return Err(format!(
                "[remote] address `{address}` is not loopback; the listener speaks plaintext HTTP, \
                 so front it with `tailscale serve` or set allow_remote = true to mean it"
            ));
        }
        self.push_subject()?;
        Ok(parsed)
    }

    /// The configured VAPID contact, when one is set.
    pub fn push_subject(&self) -> Result<Option<&str>, String> {
        let subject = self.push_subject.trim();
        if subject.is_empty() {
            return Ok(None);
        }
        if !(subject.starts_with("mailto:") || subject.starts_with("https://")) {
            return Err("[remote] push_subject must be a mailto: or https:// URL".into());
        }
        Ok(Some(subject))
    }
}

#[cfg(test)]
mod tests {
    use super::RemoteSection;
    use crate::config::AetherConfig;

    #[test]
    fn the_defaults_bind_loopback_and_send_no_previews() {
        let config: AetherConfig = toml::from_str("").expect("empty config");
        let remote = config.remote;
        let address = remote.address().expect("the default is valid");
        assert!(address.ip().is_loopback());
        assert_eq!(address.port(), 8377);
        assert!(!remote.allow_remote);
        assert!(!remote.push_previews);
        assert_eq!(remote.push_subject(), Ok(None));
        assert!(remote.push_hosts.is_empty());
    }

    #[test]
    fn a_bind_outside_loopback_must_be_meant() {
        let mut remote = RemoteSection {
            address: "0.0.0.0:8377".into(),
            ..Default::default()
        };
        let refused = remote.address().expect_err("plaintext on the network");
        assert!(refused.contains("allow_remote"), "{refused}");
        remote.allow_remote = true;
        remote.address().expect("acknowledged");
    }

    #[test]
    fn values_that_cannot_mean_what_they_say_are_refused() {
        for (key, value) in [
            ("address", "\"box.local\""),
            ("max_connections", "0"),
            ("idle_timeout_secs", "1"),
            ("push_subject", "\"ops@example.com\""),
        ] {
            let text = format!("[remote]\n{key} = {value}\n");
            let config: AetherConfig = toml::from_str(&text).expect("well-formed");
            let refused = config.remote.address().expect_err(key);
            assert!(refused.contains("[remote]"), "{refused}");
        }
    }

    #[test]
    fn the_template_documents_the_defaults() {
        let template = super::super::CONFIG_TEMPLATE;
        let uncommented = template.replace("# [remote]", "[remote]");
        for key in [
            "address = \"127.0.0.1:8377\"",
            "push_previews = false",
            "push_hosts = []",
            "idle_timeout_secs = 300",
        ] {
            assert!(uncommented.contains(key), "{key}");
        }
    }
}
