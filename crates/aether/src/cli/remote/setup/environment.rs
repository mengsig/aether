//! The machine around the wizard: its platform, the clock, the network and
//! the Ctrl+C key. A trait so the wizard can run without any of them.

use std::time::Duration;

use aether_protocol::remote::Health;

use super::platform::Platform;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Why `GET /api/health` did not give a healthy answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ProbeError {
    /// The name did not resolve, the connection or the TLS handshake failed.
    Unreachable(String),
    /// Something answered, but not with the daemon's health.
    Status(u16),
    Malformed(String),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(why) => write!(f, "could not connect: {why}"),
            Self::Status(code) => write!(f, "answered HTTP {code}"),
            Self::Malformed(why) => write!(f, "answered something that is not Aether: {why}"),
        }
    }
}

pub(super) trait Environment {
    fn platform(&self) -> Platform;

    async fn sleep(&self, duration: Duration);

    /// `GET {origin}/api/health`, over TLS, without a proxy.
    async fn health(&self, origin: &str) -> Result<Health, ProbeError>;

    /// Resolves when the person presses Ctrl+C.
    async fn interrupted(&self);
}

#[derive(Debug)]
pub(super) struct System {
    platform: Platform,
}

impl System {
    pub(super) fn new() -> Self {
        let proc_version = std::fs::read_to_string("/proc/version").ok();
        let os_release = std::fs::read_to_string("/etc/os-release").ok();
        Self {
            platform: Platform::detect(
                std::env::consts::OS,
                proc_version.as_deref(),
                os_release.as_deref(),
            ),
        }
    }
}

impl Environment for System {
    fn platform(&self) -> Platform {
        self.platform.clone()
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    async fn health(&self, origin: &str) -> Result<Health, ProbeError> {
        assert!(
            origin.starts_with("https://"),
            "{origin} is not an https origin"
        );
        assert!(!origin.ends_with('/'), "{origin} has a trailing slash");
        let client = reqwest::Client::builder()
            .timeout(PROBE_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|e| ProbeError::Unreachable(e.to_string()))?;
        let response = client
            .get(format!("{origin}/api/health"))
            .send()
            .await
            .map_err(|e| ProbeError::Unreachable(error_chain(&e)))?;
        if !response.status().is_success() {
            return Err(ProbeError::Status(response.status().as_u16()));
        }
        response
            .json::<Health>()
            .await
            .map_err(|e| ProbeError::Malformed(e.to_string()))
    }

    async fn interrupted(&self) {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// An error with its causes: reqwest's top line alone is "error sending request".
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
