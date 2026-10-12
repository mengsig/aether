//! What a device may subscribe with: the endpoint, its host and the keys. Endpoints are held to
//! known push services so a device cannot make the daemon POST to any public URL.

use aether_protocol::remote::PushSubscription;
use aws_lc_rs::agreement;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::ece::{AUTH_SECRET_BYTES, PUBLIC_KEY_BYTES, Recipient};
use super::{MAX_ENDPOINT_BYTES, PushError};

/// The push services of Chromium, Firefox, Safari and Edge. A leading `*.`
/// matches any subdomain, never the bare domain.
pub const KNOWN_HOSTS: &[&str] = &[
    "fcm.googleapis.com",
    "jmt17.google.com",
    "*.push.services.mozilla.com",
    "*.push.apple.com",
    "*.notify.windows.com",
];

/// Whether `endpoint` has the shape of one: bounded, https on the default
/// port, a named host, no credentials. A [`HostPolicy`] decides the host.
pub fn check_endpoint(endpoint: &str) -> Result<url::Url, PushError> {
    if endpoint.len() > MAX_ENDPOINT_BYTES {
        return Err(PushError::BadEndpoint);
    }
    let url = url::Url::parse(endpoint).map_err(|_| PushError::BadEndpoint)?;
    let credentials = !url.username().is_empty() || url.password().is_some();
    if url.scheme() != "https" || credentials || url.port().is_some() {
        return Err(PushError::BadEndpoint);
    }
    if !matches!(url.host(), Some(url::Host::Domain(_))) {
        return Err(PushError::BadEndpoint);
    }
    Ok(url)
}

/// The origin a VAPID token is addressed to.
pub(crate) fn audience(endpoint: &url::Url) -> String {
    assert_eq!(endpoint.scheme(), "https", "a checked endpoint is https");
    assert!(
        endpoint.port().is_none(),
        "a checked endpoint uses the default port"
    );
    endpoint.origin().ascii_serialization()
}

/// Which hosts an endpoint may name: the known push services and whatever
/// the operator added in `[remote] push_hosts`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostPolicy {
    extra: Vec<String>,
}

impl HostPolicy {
    /// `extra` holds exact host names or `*.suffix` patterns.
    pub fn new(extra: &[String]) -> Result<Self, String> {
        let mut patterns = Vec::with_capacity(extra.len());
        for raw in extra {
            let pattern = raw.trim().to_ascii_lowercase();
            check_pattern(&pattern).map_err(|why| format!("push host `{raw}` {why}"))?;
            patterns.push(pattern);
        }
        Ok(Self { extra: patterns })
    }

    pub fn allows(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        KNOWN_HOSTS
            .iter()
            .copied()
            .chain(self.extra.iter().map(String::as_str))
            .any(|pattern| matches(pattern, &host))
    }

    /// The endpoint, parsed, when both its shape and its host are acceptable.
    pub fn check(&self, endpoint: &str) -> Result<url::Url, PushError> {
        let url = check_endpoint(endpoint)?;
        let host = url.host_str().ok_or(PushError::BadEndpoint)?;
        if !self.allows(host) {
            return Err(PushError::HostNotAllowed {
                host: host.to_owned(),
            });
        }
        Ok(url)
    }
}

fn check_pattern(pattern: &str) -> Result<(), &'static str> {
    let name = pattern.strip_prefix("*.").unwrap_or(pattern);
    if name.is_empty() || name.len() > 253 {
        return Err("is empty or too long");
    }
    let label = |label: &str| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if !name.split('.').all(label) {
        return Err("is not a host name or a `*.suffix` pattern");
    }
    if !name.contains('.') {
        return Err("must name a domain, not a single label");
    }
    // No top-level domain is numeric; this is an address, which is never allowed.
    if name
        .rsplit('.')
        .next()
        .is_some_and(|last| last.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("is an address, and a push service is named");
    }
    Ok(())
}

fn matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        None => pattern == host,
        Some(suffix) => host
            .strip_suffix(suffix)
            .is_some_and(|rest| rest.len() > 1 && rest.ends_with('.')),
    }
}

/// Decode a subscription's keys: a P-256 point and a 16-byte secret.
pub fn recipient(subscription: &PushSubscription) -> Result<Recipient, PushError> {
    let decode = |text: &str| URL_SAFE_NO_PAD.decode(text).map_err(|_| PushError::BadKeys);
    let p256dh: [u8; PUBLIC_KEY_BYTES] = decode(&subscription.p256dh)?
        .try_into()
        .map_err(|_| PushError::BadKeys)?;
    let auth: [u8; AUTH_SECRET_BYTES] = decode(&subscription.auth)?
        .try_into()
        .map_err(|_| PushError::BadKeys)?;
    if p256dh[0] != 0x04 {
        return Err(PushError::BadKeys);
    }
    let unparsed = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, p256dh);
    agreement::ParsedPublicKey::try_from(&unparsed).map_err(|_| PushError::BadKeys)?;
    Ok(Recipient { p256dh, auth })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_endpoints_are_stored() {
        check_endpoint("https://fcm.googleapis.com/fcm/send/abc").expect("a push service");
        for bad in [
            "http://push.example/x",
            "ftp://push.example/x",
            "https://user@push.example/x",
            "https://:secret@push.example/x",
            "https://push.example:8443/x",
            "https://93.184.216.34/x",
            "https://[2606:2800:220:1::1]/x",
            "not a url",
            "",
        ] {
            assert_eq!(check_endpoint(bad), Err(PushError::BadEndpoint), "{bad}");
        }
        let long = format!("https://push.example/{}", "x".repeat(MAX_ENDPOINT_BYTES));
        assert_eq!(check_endpoint(&long), Err(PushError::BadEndpoint));
    }

    #[test]
    fn the_known_push_services_are_allowed_and_nothing_else() {
        let policy = HostPolicy::default();
        for endpoint in [
            "https://fcm.googleapis.com/fcm/send/abc",
            "https://updates.push.services.mozilla.com/wpush/v2/abc",
            "https://web.push.apple.com/abc",
            "https://wns2-par02p.notify.windows.com/w/?token=abc",
            "https://FCM.GoogleAPIs.com./wp/abc",
        ] {
            policy.check(endpoint).expect(endpoint);
        }
        for (endpoint, host) in [
            ("https://example.com/hook", "example.com"),
            (
                "https://fcm.googleapis.com.evil.example/x",
                "fcm.googleapis.com.evil.example",
            ),
            ("https://push.apple.com/x", "push.apple.com"),
            ("https://evilpush.apple.com/x", "evilpush.apple.com"),
            ("https://notify.windows.com/x", "notify.windows.com"),
        ] {
            assert_eq!(
                policy.check(endpoint),
                Err(PushError::HostNotAllowed { host: host.into() }),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn an_operator_extends_the_host_list() {
        let extra = [
            "Push.Example.org".to_owned(),
            "*.push.example.net".to_owned(),
        ];
        let policy = HostPolicy::new(&extra).expect("valid patterns");
        assert!(policy.allows("push.example.org"));
        assert!(policy.allows("eu.push.example.net"));
        assert!(
            !policy.allows("push.example.net"),
            "a wildcard needs a subdomain"
        );
        assert!(!policy.allows("other.example.org"));
        for bad in [
            "",
            "*",
            "*.",
            "localhost",
            "a b.example",
            "https://x.example",
            "-x.example",
            "10.0.0.1",
            "*.0.1",
        ] {
            HostPolicy::new(&[bad.to_owned()]).expect_err(bad);
        }
    }

    #[test]
    fn subscription_keys_must_be_a_point_and_a_secret() {
        let agent = crate::push::testing::UserAgent::generate();
        let good = agent.subscription("https://fcm.googleapis.com/fcm/send/abc");
        let decoded = recipient(&good).expect("a real subscription");
        assert_eq!(decoded, agent.recipient());

        let off_curve = URL_SAFE_NO_PAD.encode([&[0x04u8][..], &[0x01; 64]].concat());
        let short_auth = URL_SAFE_NO_PAD.encode([0u8; 15]);
        let padded = format!("{}=", good.auth);
        let cases = [
            (off_curve.as_str(), good.auth.as_str()),
            ("", good.auth.as_str()),
            ("not base64!", good.auth.as_str()),
            (good.p256dh.as_str(), short_auth.as_str()),
            (good.p256dh.as_str(), padded.as_str()),
        ];
        for (p256dh, auth) in cases {
            let bad = PushSubscription {
                endpoint: good.endpoint.clone(),
                p256dh: p256dh.into(),
                auth: auth.into(),
            };
            assert_eq!(
                recipient(&bad),
                Err(PushError::BadKeys),
                "{p256dh} / {auth}"
            );
        }
    }
}
