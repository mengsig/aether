//! What a release is called and where its assets are: versions, targets,
//! asset names and URLs. `install.sh` follows the same layout.

use std::fmt;
use std::net::IpAddr;

use anyhow::{Context as _, bail, ensure};

/// Where released assets are downloaded from unless overridden.
pub(super) const DEFAULT_BASE_URL: &str = "https://github.com/mengsig/aether/releases";
/// Overrides [`DEFAULT_BASE_URL`], for mirrors and tests. Shared with `install.sh`.
pub(super) const BASE_URL_VARIABLE: &str = "AETHER_INSTALL_BASE_URL";
/// The asset whose whole content is the release's tag.
pub(super) const VERSION_ASSET: &str = "VERSION";

/// A released version: `MAJOR.MINOR.PATCH`, tagged `vMAJOR.MINOR.PATCH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    /// Parse `1.2.3` or `v1.2.3`. Anything else is refused, so a tag built
    /// from the result is exactly the one that was asked for.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let digits = text.strip_prefix('v').unwrap_or(text);
        let parts: Vec<&str> = digits.split('.').collect();
        ensure!(
            parts.len() == 3,
            "`{text}` is not a release version; expected the form v1.2.3"
        );
        let mut numbers = [0u64; 3];
        for (number, part) in numbers.iter_mut().zip(&parts) {
            *number = parse_component(part)
                .with_context(|| format!("`{text}` is not a release version"))?;
        }
        let [major, minor, patch] = numbers;
        Ok(Self {
            major,
            minor,
            patch,
        })
    }

    /// The git tag and release name: `v1.2.3`.
    pub(super) fn tag(&self) -> String {
        format!("v{self}")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// One number of a version: digits only, no sign, no leading zero.
fn parse_component(part: &str) -> anyhow::Result<u64> {
    ensure!(
        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()),
        "`{part}` is not a number"
    );
    ensure!(
        part == "0" || !part.starts_with('0'),
        "`{part}` has a leading zero"
    );
    part.parse()
        .with_context(|| format!("`{part}` is too large"))
}

/// The release target built for an operating system and CPU, as
/// `std::env::consts` names them.
pub(super) fn target_for(os: &str, arch: &str) -> anyhow::Result<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        _ => bail!(
            "no released binary for {os} on {arch}; releases exist for Linux and macOS on \
             x86_64 and aarch64. Build from source instead (docs/install.md)"
        ),
    }
}

/// The archive that holds the binary for `target`.
pub(super) fn archive_name(target: &str) -> String {
    format!("aether-{target}.tar.gz")
}

/// The file that holds an archive's SHA-256.
pub(super) fn checksum_name(archive: &str) -> String {
    format!("{archive}.sha256")
}

/// One release archive: its file name, and where it and its checksum are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Archive {
    pub(super) name: String,
    pub(super) url: String,
    pub(super) checksum_url: String,
}

/// A validated download base: every asset URL is built from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReleaseSource {
    base: String,
    https: bool,
}

impl ReleaseSource {
    /// The base named by [`BASE_URL_VARIABLE`], or GitHub's when it is unset
    /// or empty.
    pub(super) fn from_env() -> anyhow::Result<Self> {
        match std::env::var(BASE_URL_VARIABLE) {
            Ok(base) if !base.is_empty() => {
                Self::new(&base).with_context(|| format!("{BASE_URL_VARIABLE} is not usable"))
            }
            Ok(_) | Err(std::env::VarError::NotPresent) => Self::new(DEFAULT_BASE_URL),
            Err(std::env::VarError::NotUnicode(_)) => {
                bail!("{BASE_URL_VARIABLE} is not valid UTF-8")
            }
        }
    }

    /// Accept `https://`, and `http://` for this machine only: a checksum
    /// fetched over plain HTTP from elsewhere proves nothing.
    pub(super) fn new(base: &str) -> anyhow::Result<Self> {
        let url = reqwest::Url::parse(base).with_context(|| format!("`{base}` is not a URL"))?;
        ensure!(
            url.query().is_none() && url.fragment().is_none(),
            "`{base}` must not carry a query or a fragment"
        );
        ensure!(
            url.username().is_empty() && url.password().is_none(),
            "the download base must not carry credentials"
        );
        let https = match url.scheme() {
            "https" => true,
            "http" if is_this_machine(&url) => false,
            "http" => {
                bail!("`{base}` must use https:// (http:// only for 127.0.0.1, localhost or [::1])")
            }
            other => bail!("`{base}` must use https://, not {other}://"),
        };
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            https,
        })
    }

    /// Whether every request, redirects included, must stay on HTTPS.
    pub(super) fn https_only(&self) -> bool {
        self.https
    }

    /// An asset of the newest published release.
    pub(super) fn latest_url(&self, asset: &str) -> String {
        format!("{}/latest/download/{asset}", self.base)
    }

    /// An asset of one release.
    pub(super) fn tagged_url(&self, version: Version, asset: &str) -> String {
        format!("{}/download/{}/{asset}", self.base, version.tag())
    }

    /// The page a person reads one release's notes on.
    pub(super) fn release_page(&self, version: Version) -> String {
        format!("{}/tag/{}", self.base, version.tag())
    }

    /// The archive of one release for `target`, pinned to that release's tag.
    pub(super) fn archive(&self, version: Version, target: &str) -> Archive {
        let name = archive_name(target);
        Archive {
            url: self.tagged_url(version, &name),
            checksum_url: self.tagged_url(version, &checksum_name(&name)),
            name,
        }
    }
}

fn is_this_machine(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The digest a `.sha256` file publishes for `archive`, lowercased.
///
/// The file is `sha256sum` output: the digest, then the name it belongs to.
/// A file naming another archive is refused rather than trusted.
pub(super) fn published_digest(checksum_file: &str, archive: &str) -> anyhow::Result<String> {
    let line = checksum_file.lines().next().unwrap_or_default();
    let mut fields = line.split_whitespace();
    let digest = fields.next().unwrap_or_default();
    ensure!(
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "the published checksum for {archive} is not a SHA-256 digest"
    );
    if let Some(name) = fields.next() {
        ensure!(
            name.trim_start_matches('*') == archive,
            "the published checksum is for `{name}`, not {archive}"
        );
    }
    Ok(digest.to_ascii_lowercase())
}
