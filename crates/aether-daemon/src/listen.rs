//! The transports the daemon accepts connections on.
//!
//! The Unix socket is authorized by its file mode: reaching it means being
//! this user. A TCP listener has no such boundary, so it carries a token,
//! binds to loopback unless the operator says otherwise, caps how many
//! connections it serves at once and drops the ones that fall silent.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aether_protocol::auth::{MIN_TOKEN_CHARS, constant_time_eq};
use aether_protocol::{ErrorCode, ErrorReply, Hello};
use tokio::net::TcpListener;

/// Connections served at once. A peer that opens more waits in the kernel's
/// backlog rather than costing a task and a pair of file descriptors each.
pub const DEFAULT_MAX_CONNECTIONS: usize = 64;

/// How long a connection may go without sending a frame. A client that
/// subscribes and then listens sends `Ping` to stay under it.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// How long a connection may take to present a hello the daemon accepts.
/// Deliberately far shorter than the idle timeout and independent of it: an
/// un-greeted peer has authorized nothing, so it must not be able to hold a
/// connection slot on the same terms as a client that has.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a peer that presented the wrong token waits for its refusal.
/// A token holder is never delayed; guessing costs a connection slot for
/// this long per attempt.
const BAD_TOKEN_DELAY: Duration = Duration::from_millis(250);

/// The daemon's optional TCP listener.
#[derive(Clone)]
pub struct TcpListen {
    pub addr: SocketAddr,
    /// The shared secret every hello on this transport must present.
    pub token: String,
    /// The environment variable `token` was read from, when it was read from
    /// one - `[listen] token_env` for a config-driven listener. An embedder
    /// that sets `DaemonConfig.tcp` directly and names its own variable here
    /// gets it hidden from tool children the same way; one that only passes
    /// the value has nothing to name and nothing more to hide.
    pub token_env: Option<String>,
    /// Bind outside loopback. Off by default: this transport drives the whole
    /// fleet behind one static string, and Aether terminates no TLS.
    pub allow_remote: bool,
    pub max_connections: usize,
    pub idle_timeout: Duration,
    /// How long a connection may take to present an accepted hello.
    pub handshake_timeout: Duration,
}

/// Manual, so that one `tracing::debug!(?listen)` added later cannot put the
/// fleet's token in a log file.
impl std::fmt::Debug for TcpListen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpListen")
            .field("addr", &self.addr)
            .field("token", &"<redacted>")
            .field("token_env", &self.token_env)
            .field("allow_remote", &self.allow_remote)
            .field("max_connections", &self.max_connections)
            .field("idle_timeout", &self.idle_timeout)
            .field("handshake_timeout", &self.handshake_timeout)
            .finish()
    }
}

impl TcpListen {
    /// The token is trimmed here, once: `check` validates a trimmed token, so
    /// storing the untrimmed one would validate one string and compare
    /// another the moment a caller passes a padded value.
    pub fn new(addr: SocketAddr, token: impl Into<String>) -> Self {
        Self {
            addr,
            token: token.into().trim().to_owned(),
            token_env: None,
            allow_remote: false,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            handshake_timeout: HANDSHAKE_TIMEOUT,
        }
    }
}

/// What a connection must present, and how long it may sit idle.
#[derive(Clone)]
pub struct ConnectionPolicy {
    /// The token every hello must carry. `None` on the Unix socket.
    pub token: Option<Arc<str>>,
    /// How long a greeted connection may go without sending a frame.
    pub idle_timeout: Option<Duration>,
    /// How long an un-greeted connection may take to greet. Always set:
    /// there is no transport on which sitting silent before the hello is a
    /// thing a client legitimately does.
    pub handshake_timeout: Duration,
    /// Set on the remote carrier: the hello must prove a paired device
    /// against this challenge. `None` on the socket and the TCP listener.
    pub device: Option<aether_remote::DeviceGate>,
}

/// Manual, for the same reason as `TcpListen`'s.
impl std::fmt::Debug for ConnectionPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionPolicy")
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("idle_timeout", &self.idle_timeout)
            .field("handshake_timeout", &self.handshake_timeout)
            .field("device", &self.device.is_some())
            .finish()
    }
}

impl Default for ConnectionPolicy {
    fn default() -> Self {
        ConnectionPolicy {
            token: None,
            idle_timeout: None,
            handshake_timeout: HANDSHAKE_TIMEOUT,
            device: None,
        }
    }
}

impl ConnectionPolicy {
    /// Whether this hello may proceed to a welcome.
    pub fn authorize(&self, hello: &Hello) -> Result<(), ErrorReply> {
        let Some(expected) = self.token.as_deref() else {
            return Ok(());
        };
        match hello.token.as_deref() {
            Some(presented) if constant_time_eq(presented, expected) => Ok(()),
            Some(_) => Err(ErrorReply::new(
                ErrorCode::Unauthorized,
                "the token is not this daemon's",
            )),
            None => Err(ErrorReply::new(
                ErrorCode::Unauthorized,
                "this transport needs a token in the hello frame",
            )),
        }
    }

    /// How long to hold a peer that failed to authorize before answering it.
    pub fn refusal_delay(&self) -> Duration {
        if self.token.is_some() || self.device.is_some() {
            BAD_TOKEN_DELAY
        } else {
            Duration::ZERO
        }
    }
}

/// Whether this listener may be opened at all. Checked before anything is
/// bound, so a refused configuration never holds a port even briefly.
pub fn check(listen: &TcpListen) -> anyhow::Result<()> {
    // Not trimmed: what is validated here must be the string `authorize`
    // compares, or a padded token passes a check on a value no client can
    // present. `TcpListen::new` trims once, at construction.
    let token = listen.token.as_str();
    if token.chars().count() < MIN_TOKEN_CHARS {
        anyhow::bail!(
            "the TCP listener needs a token of at least {MIN_TOKEN_CHARS} characters; \
             this one has {}",
            token.chars().count()
        );
    }
    if listen.max_connections == 0 {
        anyhow::bail!("the TCP listener needs room for at least one connection");
    }
    if !listen.addr.ip().is_loopback() && !listen.allow_remote {
        anyhow::bail!(
            "{} is not a loopback address, and this transport can create agents that run \
             commands on this machine; tunnel it over SSH, or pass --allow-remote to accept \
             plaintext on the network",
            listen.addr
        );
    }
    Ok(())
}

/// Bind the TCP listener. Returns it and the address it actually bound,
/// which a caller that asked for port 0 needs.
pub async fn bind_tcp(listen: &TcpListen) -> anyhow::Result<(TcpListener, SocketAddr)> {
    check(listen)?;
    let listener = TcpListener::bind(listen.addr).await?;
    let local = listener.local_addr()?;
    if !local.ip().is_loopback() {
        tracing::warn!(
            address = %local,
            "daemon: listening outside loopback in plaintext - Aether terminates no TLS, so the \
             token and every frame cross the network in the clear unless something in front of \
             it does"
        );
    }
    Ok((listener, local))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello_with(token: Option<&str>) -> Hello {
        let hello = Hello::new("t", "0");
        match token {
            Some(t) => hello.with_token(t),
            None => hello,
        }
    }

    const GOOD: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn the_socket_asks_for_no_token() {
        let open = ConnectionPolicy::default();
        assert!(open.authorize(&hello_with(None)).is_ok());
        assert!(open.authorize(&hello_with(Some("anything"))).is_ok());
        assert_eq!(open.refusal_delay(), Duration::ZERO);
    }

    #[test]
    fn a_tokened_transport_takes_that_token_and_no_other() {
        let policy = ConnectionPolicy {
            token: Some(GOOD.into()),
            ..Default::default()
        };
        assert!(policy.authorize(&hello_with(Some(GOOD))).is_ok());
        for wrong in [None, Some(""), Some("0123456789abcdef0123456789abcdeg")] {
            let refused = policy
                .authorize(&hello_with(wrong))
                .expect_err("refused without the token");
            assert_eq!(refused.code, ErrorCode::Unauthorized);
        }
        assert!(policy.refusal_delay() > Duration::ZERO);
    }

    /// The `Debug` impls are hand-written so a `tracing::debug!(?listen)`
    /// cannot print the fleet's token. A derive would, and nothing else
    /// would notice.
    #[test]
    fn debug_prints_no_token() {
        let listen = TcpListen::new("127.0.0.1:7777".parse().expect("addr"), GOOD);
        let printed = format!("{listen:?}");
        assert!(!printed.contains(GOOD), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");

        let policy = ConnectionPolicy {
            token: Some(GOOD.into()),
            ..Default::default()
        };
        let printed = format!("{policy:?}");
        assert!(!printed.contains(GOOD), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");

        // The client's own frame carries the same secret on the way in.
        let printed = format!("{:?}", hello_with(Some(GOOD)));
        assert!(!printed.contains(GOOD), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
    }

    /// A refusal says the token was wrong without repeating what was sent:
    /// the message reaches a peer that has not authorized, and a log.
    #[test]
    fn a_refusal_never_echoes_the_token() {
        let policy = ConnectionPolicy {
            token: Some(GOOD.into()),
            ..Default::default()
        };
        let presented = "0123456789abcdef0123456789abcdeg";
        let refused = policy
            .authorize(&hello_with(Some(presented)))
            .expect_err("refused");
        assert!(!refused.message.contains(presented), "{}", refused.message);
        assert!(!refused.message.contains(GOOD), "{}", refused.message);
    }

    #[test]
    fn a_short_token_is_refused_before_anything_is_bound() {
        let addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
        let refused = check(&TcpListen::new(addr, "hunter2")).expect_err("a weak token");
        assert!(
            refused.to_string().contains(&MIN_TOKEN_CHARS.to_string()),
            "{refused}"
        );
    }

    #[tokio::test]
    async fn loopback_binds_and_reports_its_port() {
        let addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
        let (_listener, local) = bind_tcp(&TcpListen::new(addr, GOOD)).await.expect("bind");
        assert!(local.ip().is_loopback());
        assert_ne!(local.port(), 0, "the bound port is reported back");
    }

    /// A padded token used to pass a check on its trimmed length and then be
    /// compared with the padding, so a client presenting the token the
    /// operator configured was refused.
    #[test]
    fn the_token_that_is_validated_is_the_token_that_is_compared() {
        let addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
        let listen = TcpListen::new(addr, format!("  {GOOD}  "));
        assert_eq!(listen.token, GOOD, "the token is stored trimmed");
        check(&listen).expect("a padded configuration is still this token");
        let policy = ConnectionPolicy {
            token: Some(listen.token.as_str().into()),
            ..Default::default()
        };
        assert!(policy.authorize(&hello_with(Some(GOOD))).is_ok());
    }

    /// The guard is checked, not the bind: a test must not hold a port on
    /// every interface of the machine it runs on.
    #[test]
    fn a_non_loopback_bind_needs_the_operator_to_mean_it() {
        let addr: SocketAddr = "0.0.0.0:7777".parse().expect("addr");
        let refused = check(&TcpListen::new(addr, GOOD)).expect_err("no --allow-remote");
        assert!(refused.to_string().contains("allow-remote"), "{refused}");
        let mut meant = TcpListen::new(addr, GOOD);
        meant.allow_remote = true;
        check(&meant).expect("acknowledged plaintext is allowed");
    }
}
