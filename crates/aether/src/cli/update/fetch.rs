//! The requests `aether update` makes: each has a deadline and a size limit,
//! and none is made unless the person ran the command.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, ensure};
use tokio::io::AsyncWriteExt as _;

use super::release::ReleaseSource;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A connection that sends nothing for this long is given up on.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// The whole request for a version or a checksum file.
const TEXT_TIMEOUT: Duration = Duration::from_secs(30);
/// The whole download of one archive, as in `install.sh`.
const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_TEXT_BYTES: u64 = 4 * 1024;
/// The changelog grows with every release; this leaves it room for years.
const MAX_NOTES_BYTES: u64 = 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

/// A client for release downloads. It follows GitHub's redirects to its asset
/// host and uses the proxy the environment names (`https_proxy`).
pub(super) fn client(source: &ReleaseSource) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("aether/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .https_only(source.https_only())
        .build()
        .context("could not set up the HTTP client")
}

/// A small text asset, such as a version or a checksum file.
pub(super) async fn text(client: &reqwest::Client, url: &str) -> anyhow::Result<String> {
    let response = get(client, url, TEXT_TIMEOUT, MAX_TEXT_BYTES).await?;
    read_text(response, url, MAX_TEXT_BYTES).await
}

/// The release notes asset, or `None` when the release has no such file:
/// releases published before it existed do not.
pub(super) async fn notes(client: &reqwest::Client, url: &str) -> anyhow::Result<Option<String>> {
    let response = send(client, url, TEXT_TIMEOUT).await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let response = accepted(response, url, MAX_NOTES_BYTES)?;
    read_text(response, url, MAX_NOTES_BYTES).await.map(Some)
}

async fn read_text(
    mut response: reqwest::Response,
    url: &str,
    max_bytes: u64,
) -> anyhow::Result<String> {
    let interrupted = || format!("the download of {url} was interrupted");
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.with_context(interrupted)? {
        ensure!(
            (body.len() + chunk.len()) as u64 <= max_bytes,
            "{url} is larger than the {max_bytes} bytes a release text file may be"
        );
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).with_context(|| format!("{url} is not UTF-8 text"))
}

/// Download `url` into the new file `destination` and return the SHA-256 of
/// what was written, as lowercase hex.
pub(super) async fn file(
    client: &reqwest::Client,
    url: &str,
    destination: &Path,
) -> anyhow::Result<String> {
    use sha2::Digest as _;

    let mut response = get(client, url, ARCHIVE_TIMEOUT, MAX_ARCHIVE_BYTES).await?;
    let interrupted = || format!("the download of {url} was interrupted");
    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .await
        .with_context(|| format!("could not create {}", destination.display()))?;
    let mut hasher = sha2::Sha256::new();
    let mut written = 0u64;
    while let Some(chunk) = response.chunk().await.with_context(interrupted)? {
        written += chunk.len() as u64;
        ensure!(
            written <= MAX_ARCHIVE_BYTES,
            "{url} is larger than the {MAX_ARCHIVE_BYTES} bytes a release archive may be"
        );
        hasher.update(&chunk);
        output
            .write_all(&chunk)
            .await
            .with_context(|| format!("could not write {}", destination.display()))?;
    }
    ensure!(written > 0, "{url} is empty");
    output
        .sync_all()
        .await
        .with_context(|| format!("could not flush {}", destination.display()))?;
    Ok(hex::encode(hasher.finalize()))
}

/// Start a request that must finish, body included, within `timeout`.
async fn get(
    client: &reqwest::Client,
    url: &str,
    timeout: Duration,
    max_bytes: u64,
) -> anyhow::Result<reqwest::Response> {
    let response = send(client, url, timeout).await?;
    accepted(response, url, max_bytes)
}

async fn send(
    client: &reqwest::Client,
    url: &str,
    timeout: Duration,
) -> anyhow::Result<reqwest::Response> {
    client
        .get(url)
        .timeout(timeout)
        .send()
        .await
        .with_context(|| format!("could not download {url}"))
}

/// The response, if it succeeded and announces no more than `max_bytes`.
fn accepted(
    response: reqwest::Response,
    url: &str,
    max_bytes: u64,
) -> anyhow::Result<reqwest::Response> {
    let status = response.status();
    ensure!(
        status.is_success(),
        "could not download {url}: the server answered {status}"
    );
    if let Some(length) = response.content_length() {
        ensure!(
            length <= max_bytes,
            "{url} is {length} bytes, more than the {max_bytes} allowed"
        );
    }
    Ok(response)
}
