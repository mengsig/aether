//! Finding the newest protocol both this build and a daemon speak, shared by
//! every transport that can open a fresh connection per attempt, and refusing
//! a command the version agreed on cannot carry.

use std::future::Future;

use aether_protocol::compat::{MIN_PROTOCOL_VERSION, served_window};
use aether_protocol::{Command, ErrorCode, ErrorReply, Introduced, PROTOCOL_VERSION};

use crate::{Client, ClientError, Result};

/// Connect at the newest version both sides speak. `attempt` opens a fresh
/// connection and greets at the version it is given; each refusal names the
/// window the daemon serves, so versions it cannot speak are skipped.
pub(crate) async fn newest_common<F, Fut>(mut attempt: F) -> Result<Client>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = Result<Client>>,
{
    let mut version = PROTOCOL_VERSION;
    let mut refusals: Vec<String> = Vec::new();
    loop {
        let refusal = match attempt(version).await {
            Err(ClientError::Incompatible(refusal)) => refusal,
            result => return result,
        };
        let next = next_version(version, &refusal);
        if refusals.is_empty() {
            refusals.push(refusal);
        } else {
            refusals.push(format!("protocol {version} also refused: {refusal}"));
        }
        match next {
            Some(lower) => {
                debug_assert!(lower < version, "each attempt asks for an older version");
                version = lower;
            }
            None => break,
        }
    }
    debug_assert!(!refusals.is_empty(), "the loop ends only after a refusal");
    Err(ClientError::Incompatible(refusals.join("; ")))
}

/// The next version to ask for after `tried` was refused, or `None` when no
/// version this build speaks is left that the daemon might serve.
fn next_version(tried: u32, refusal: &str) -> Option<u32> {
    let below = tried.checked_sub(1)?;
    let candidate = match served_window(refusal) {
        Some(window) if below < *window.start() => return None,
        Some(window) => below.min(*window.end()),
        None => below,
    };
    (candidate >= MIN_PROTOCOL_VERSION).then_some(candidate)
}

impl Client {
    /// Refuse a command the daemon on this connection cannot decode, before
    /// it is written: an older daemon would fail to parse it instead.
    pub(crate) fn check_introduced(&self, command: &Command) -> Result<()> {
        let Some(welcome) = self.welcome.as_ref() else {
            return Ok(());
        };
        let message = match command.introduced() {
            Introduced::Version(needed) if welcome.protocol_version >= needed => return Ok(()),
            Introduced::Version(needed) => format!(
                "`{}` needs protocol {needed} and this connection speaks protocol {}",
                command.name(),
                welcome.protocol_version
            ),
            Introduced::Feature(feature) if welcome.features.iter().any(|f| f == feature) => {
                return Ok(());
            }
            Introduced::Feature(feature) => format!(
                "`{}` needs a daemon that announces `{feature}`, and this one does not",
                command.name()
            ),
        };
        Err(ClientError::Protocol(ErrorReply::new(
            ErrorCode::Unimplemented,
            format!(
                "{message}: the daemon is older than this client. Restart it from this build \
                 (`aether stop`, then start it again), or update Aether on the host. \
                 Nothing was sent"
            ),
        )))
    }
}

#[cfg(test)]
#[path = "negotiation_tests.rs"]
mod tests;
