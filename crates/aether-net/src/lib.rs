//! How Aether makes an outbound HTTP request it did not have to make.
//!
//! Two callers reach hosts named from outside the daemon's own code: the
//! `web_fetch`/`web_search` tools, where the model or the operator chose the
//! URL, and `aether serve`'s event subscriptions, where the operator did.
//! Both need the same discipline, and having it in one place is the point:
//! the second one to be written diverged from the first, and the divergence
//! was the bug.
//!
//! The discipline is: parse and vet the target, resolve it once, decide on
//! the addresses that came back, and then connect to exactly those - no
//! re-resolution, no redirect, no proxy. Re-resolving would let a short-TTL
//! name answer public for the check and loopback for the request; a redirect
//! would reach an address nothing vetted; a proxy would move resolution off
//! this machine entirely, leaving the check deciding nothing.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    /// The caller's target cannot mean what it says.
    #[error("{0}")]
    Invalid(String),
    /// The target is well-formed and this policy refuses it.
    #[error("{0}")]
    Denied(String),
    #[error("{0}")]
    Internal(String),
}

/// Which addresses a target may resolve to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressPolicy {
    /// Public addresses only. For a URL a model chose: it must not become a
    /// way to reach the daemon's own neighbours.
    PublicOnly,
    /// Any address that resolves. For a target the operator wrote in their
    /// own config file - a self-hosted receiver on the LAN is a legitimate
    /// setup, and the operator could point the daemon anywhere regardless.
    /// Everything else about the check still applies: scheme, pinned
    /// addresses, no redirect, no proxy.
    Configured,
}

/// A target that passed the address check, with the addresses it passed on.
#[derive(Debug, Clone)]
pub struct Vetted {
    pub url: Url,
    /// The host as a resolver spells it: an IPv6 literal without brackets.
    pub host: String,
    pub port: u16,
    pub addrs: Vec<IpAddr>,
}

impl Vetted {
    /// Whether every address this resolved to is on this machine. What makes
    /// plaintext acceptable: nothing leaves the loopback interface.
    pub fn is_loopback(&self) -> bool {
        self.addrs.iter().all(IpAddr::is_loopback)
    }
}

/// Parse and vet a target: only http(s), resolved once, and refused by
/// `policy` on the addresses that came back.
///
/// Returns those addresses, because the check is only worth anything if the
/// connection uses them - see [`pinned_client`].
pub async fn vet(raw: &str, policy: AddressPolicy) -> Result<Vetted, NetError> {
    let url = Url::parse(raw).map_err(|e| NetError::Invalid(format!("url: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(NetError::Invalid(format!(
            "only http and https are fetched, not `{}`",
            url.scheme()
        )));
    }
    let host = url
        .host()
        .ok_or_else(|| NetError::Invalid("url has no host".into()))?;
    let port = url.port_or_known_default().unwrap_or(80);
    // An IP literal is already an address: resolving it would only be a way
    // to get it wrong. `host_str` brackets IPv6, which no resolver accepts.
    let (host_key, addrs) = match host {
        url::Host::Ipv4(v4) => (v4.to_string(), vec![IpAddr::V4(v4)]),
        url::Host::Ipv6(v6) => (v6.to_string(), vec![IpAddr::V6(v6)]),
        url::Host::Domain(name) => {
            let name = name.to_owned();
            let lookup = name.clone();
            let addrs = tokio::task::spawn_blocking(move || {
                use std::net::ToSocketAddrs;
                (lookup.as_str(), port)
                    .to_socket_addrs()
                    .map(|it| it.map(|a| a.ip()).collect::<Vec<_>>())
            })
            .await
            .map_err(|e| NetError::Internal(format!("dns task: {e}")))?
            .map_err(|e| NetError::Invalid(format!("cannot resolve {raw}: {e}")))?;
            (name, addrs)
        }
    };
    if addrs.is_empty() {
        return Err(NetError::Invalid(format!("cannot resolve {raw}")));
    }
    if policy == AddressPolicy::PublicOnly
        && let Some(bad) = addrs.iter().find(|ip| !is_public(ip))
    {
        return Err(NetError::Denied(format!(
            "{raw} resolves to {bad}, which is not a public address"
        )));
    }
    Ok(Vetted {
        url,
        host: host_key,
        port,
        addrs,
    })
}

/// A client pinned to one vetted target: the addresses that were checked are
/// the addresses it connects to, it follows no redirect, and it ignores the
/// environment's proxy settings.
pub fn pinned_client(
    target: &Vetted,
    timeout: Duration,
    user_agent: &str,
) -> Result<reqwest::Client, NetError> {
    let addrs: Vec<SocketAddr> = target
        .addrs
        .iter()
        .map(|ip| SocketAddr::new(*ip, target.port))
        .collect();
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(user_agent.to_owned())
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve_to_addrs(&target.host, &addrs)
        .build()
        .map_err(|e| NetError::Internal(format!("http client: {e}")))
}

/// Whether this address is out on the internet rather than on this machine
/// or one of its networks.
pub fn is_public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || v4.is_unspecified()
                || o[0] == 0
                // Reserved for future use (240.0.0.0/4).
                || o[0] >= 240
                // IETF protocol assignments (192.0.0.0/24).
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                // Deprecated 6to4 relay anycast.
                || (o[0] == 192 && o[1] == 88 && o[2] == 99)
                // Carrier-grade NAT and the benchmarking range.
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || (o[0] == 198 && (18..20).contains(&o[1])))
        }
        IpAddr::V6(v6) => {
            let seg = v6.segments();
            // Both of these carry an IPv4 address a gateway will translate
            // to, so the embedded address is what actually gets reached.
            let embedded = v6
                .to_ipv4_mapped()
                .or_else(|| nat64_embedded(v6))
                .or_else(|| sixtofour_embedded(v6));
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Unique local and link-local.
                || (seg[0] & 0xfe00) == 0xfc00
                || (seg[0] & 0xffc0) == 0xfe80
                // Deprecated, documentation and translation-only ranges are not destinations.
                || (seg[0] & 0xffc0) == 0xfec0
                || seg[..6] == [0; 6]
                || seg[..3] == [0x64, 0xff9b, 1]
                || seg[..2] == [0x2001, 0]
                || seg[..2] == [0x2001, 0xdb8]
                || seg[..4] == [0x100, 0, 0, 0]
                || seg[..6] == [0, 0, 0, 0, 0xffff, 0]
                || embedded.is_some_and(|v4| !is_public(&IpAddr::V4(v4))))
        }
    }
}

fn nat64_embedded(v6: &std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let seg = v6.segments();
    let is_nat64 = seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2..6].iter().all(|s| *s == 0);
    is_nat64.then(|| std::net::Ipv4Addr::from(((seg[6] as u32) << 16) | seg[7] as u32))
}

fn sixtofour_embedded(v6: &std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let seg = v6.segments();
    (seg[0] == 0x2002).then(|| std::net::Ipv4Addr::from(((seg[1] as u32) << 16) | seg[2] as u32))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[tokio::test]
    async fn only_http_targets_are_vetted_at_all() {
        for bad in ["file:///etc/passwd", "ftp://example.com/x", "not a url"] {
            assert!(
                vet(bad, AddressPolicy::Configured).await.is_err(),
                "{bad} must not be reachable"
            );
        }
    }

    #[tokio::test]
    async fn a_private_address_is_refused_for_a_chosen_url_and_allowed_for_a_configured_one() {
        for private in [
            "http://127.0.0.1:8080/",
            "http://10.0.0.1/",
            "http://[::1]/",
        ] {
            let refused = vet(private, AddressPolicy::PublicOnly)
                .await
                .expect_err(private);
            assert!(
                matches!(refused, NetError::Denied(_)),
                "{private} must be refused as a private address, not as unresolvable: {refused:?}"
            );
            vet(private, AddressPolicy::Configured)
                .await
                .expect("the operator may configure their own LAN");
        }
    }

    #[tokio::test]
    async fn an_ip_literal_is_pinned_to_itself() {
        let target = vet("http://127.0.0.1:8080/x", AddressPolicy::Configured)
            .await
            .expect("vetted");
        assert_eq!(
            target.addrs,
            vec!["127.0.0.1".parse::<IpAddr>().expect("ip")]
        );
        assert_eq!(target.port, 8080);
        assert!(target.is_loopback());
        pinned_client(&target, Duration::from_secs(1), "aether-test").expect("client");
    }

    #[test]
    fn addresses_that_reach_this_machine_or_its_networks_are_not_public() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            // Multicast, reserved, and the IETF protocol block.
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "192.0.0.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            // NAT64 and 6to4 wrappers around a private IPv4 address: a
            // gateway translates these straight through.
            "64:ff9b::7f00:1",
            "64:ff9b::a00:1",
            "2002:7f00:1::1",
            "2002:c0a8:101::1",
            "ff02::1",
            "192.88.99.1",
            "::10.0.0.1",
            "::8.8.8.8",
            "fec0::1",
            "febf:ffff::1",
            "feff:ffff::1",
            "64:ff9b:1::a00:1",
            "64:ff9b:1::808:808",
            "2001:0:4136:e378:8000:63bf:f5ff:fffe",
            "2001:db8::1",
            "100::1",
            "::ffff:0:10.0.0.1",
            "::ffff:0:8.8.8.8",
        ] {
            let parsed: IpAddr = ip.parse().expect(ip);
            assert!(!is_public(&parsed), "{ip} must not count as public");
        }
        for ip in [
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700:4700::1111",
            // The same wrappers around a public IPv4 address are public.
            "64:ff9b::5db8:d822",
            "2002:5db8:d822::1",
        ] {
            let parsed: IpAddr = ip.parse().expect(ip);
            assert!(is_public(&parsed), "{ip} is public");
        }
    }
}
