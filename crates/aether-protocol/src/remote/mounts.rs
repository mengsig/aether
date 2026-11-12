//! Listener path mounts (`RemoteSettings.mounts`): which prefixes of the
//! remote listener forward to which loopback daemon. This is the wire shape
//! and its validation; forwarding belongs to the listener.

use serde::{Deserialize, Serialize};

/// The most mounts one listener carries.
pub const MAX_MOUNTS: usize = 4;
const MAX_PREFIX_SEGMENT: usize = 32;

/// What a mount forwards to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountKind {
    Archon,
}

/// One prefix of the listener forwarded to a loopback target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RemoteMount {
    /// One lowercase path segment: `^/[a-z][a-z0-9-]{0,31}$`.
    pub prefix: String,
    pub kind: MountKind,
    /// `http://127.0.0.1:<port>` or `http://[::1]:<port>`.
    pub target: String,
}

/// Why a set of mounts is refused, naming the first offending one.
pub fn validate_mounts(mounts: &[RemoteMount]) -> Result<(), String> {
    if mounts.len() > MAX_MOUNTS {
        return Err(format!("at most {MAX_MOUNTS} mounts, got {}", mounts.len()));
    }
    let mut seen = std::collections::BTreeSet::new();
    for mount in mounts {
        validate_prefix(&mount.prefix)?;
        validate_target(&mount.target)?;
        if !seen.insert(mount.prefix.as_str()) {
            return Err(format!("mount prefix {} is listed twice", mount.prefix));
        }
    }
    debug_assert_eq!(seen.len(), mounts.len());
    Ok(())
}

fn validate_prefix(prefix: &str) -> Result<(), String> {
    let refused = || {
        format!(
            "mount prefix {prefix:?} must be one lowercase segment such as /archon: \
             a letter, then up to {} letters, digits or dashes",
            MAX_PREFIX_SEGMENT - 1
        )
    };
    let segment = prefix.strip_prefix('/').ok_or_else(refused)?;
    let mut chars = segment.chars();
    let starts_with_letter = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    let rest_allowed = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !starts_with_letter || !rest_allowed || segment.len() > MAX_PREFIX_SEGMENT {
        return Err(refused());
    }
    Ok(())
}

fn validate_target(target: &str) -> Result<(), String> {
    let refused = || {
        format!("mount target {target:?} must be http://127.0.0.1:<port> or http://[::1]:<port>")
    };
    let port = target
        .strip_prefix("http://127.0.0.1:")
        .or_else(|| target.strip_prefix("http://[::1]:"))
        .ok_or_else(refused)?;
    // Digits only: `u16::from_str` would also take a leading `+`.
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
        return Err(refused());
    }
    match port.parse::<u16>() {
        Ok(port) if port > 0 => Ok(()),
        _ => Err(refused()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(prefix: &str, target: &str) -> RemoteMount {
        RemoteMount {
            prefix: prefix.into(),
            kind: MountKind::Archon,
            target: target.into(),
        }
    }

    #[test]
    fn loopback_mounts_with_one_lowercase_segment_are_accepted() {
        let mounts = [
            mount("/archon", "http://127.0.0.1:8378"),
            mount("/a", "http://[::1]:1"),
            mount(&format!("/a{}", "b-9".repeat(10)), "http://127.0.0.1:65535"),
        ];
        assert_eq!(validate_mounts(&mounts), Ok(()));
        assert_eq!(validate_mounts(&[]), Ok(()));
    }

    #[test]
    fn a_prefix_outside_the_pattern_is_refused() {
        let too_long = format!("/a{}", "b".repeat(32));
        for prefix in [
            "",
            "/",
            "archon",
            "/Archon",
            "/1archon",
            "/-x",
            "/arc/hon",
            "/arc.hon",
            "/..",
            "/arc%2e",
            "/arc\\hon",
            "/archon/",
            too_long.as_str(),
        ] {
            let refused = validate_mounts(&[mount(prefix, "http://127.0.0.1:8378")]);
            assert!(refused.is_err(), "{prefix:?} was accepted");
        }
    }

    #[test]
    fn a_target_that_is_not_loopback_http_with_a_port_is_refused() {
        for target in [
            "http://localhost:8378",
            "https://127.0.0.1:8378",
            "http://127.0.0.2:8378",
            "http://0.0.0.0:8378",
            "http://127.0.0.1",
            "http://127.0.0.1:",
            "http://127.0.0.1:0",
            "http://127.0.0.1:65536",
            "http://127.0.0.1:+80",
            "http://127.0.0.1:80/path",
            "http://[::1]:80@example.com",
            "http://[::2]:80",
        ] {
            let refused = validate_mounts(&[mount("/archon", target)]);
            assert!(refused.is_err(), "{target:?} was accepted");
        }
    }

    #[test]
    fn more_than_four_mounts_or_a_repeated_prefix_is_refused() {
        let five: Vec<_> = ["/a", "/b", "/c", "/d", "/e"]
            .iter()
            .map(|prefix| mount(prefix, "http://127.0.0.1:8378"))
            .collect();
        assert!(validate_mounts(&five[..4]).is_ok());
        assert!(validate_mounts(&five).unwrap_err().contains("at most 4"));
        let twice = [
            mount("/archon", "http://127.0.0.1:1"),
            mount("/archon", "http://127.0.0.1:2"),
        ];
        assert!(validate_mounts(&twice).unwrap_err().contains("twice"));
    }
}
