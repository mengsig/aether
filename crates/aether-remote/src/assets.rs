//! The web app, compiled into the binary by `build.rs` from `web/`.

pub mod selection;

use std::collections::BTreeMap;
use std::sync::LazyLock;

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// The document every app route falls back to.
pub const INDEX: &str = "/index.html";

/// The default: a cached copy is only ever used after its `ETag` matched.
pub const CACHE_CONTROL: &str = "no-cache";

/// For a URL that names the content it expects (`?v=<hash>`): the bytes
/// behind that URL can never change, so nothing needs revalidating.
pub const CACHE_CONTROL_IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// The query parameter carrying an asset's content hash.
pub const VERSION_PARAMETER: &str = "v";

/// Path of the generated table of every asset's content hash.
pub const MANIFEST: &str = "/asset-manifest.json";

/// Scripts a browser must always revalidate: a stale service worker would
/// pin the whole app to an old release.
const SERVICE_WORKERS: &[&str] = &["/sw.js", "/service-worker.js"];

/// One embedded file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    /// The URL path, starting with `/`.
    pub path: &'static str,
    pub bytes: &'static [u8],
    pub content_type: &'static str,
    /// Lowercase hex SHA-256 of `bytes`.
    pub hash: &'static str,
}

/// Each asset's content hash, in table order. Hashed once, on first use.
static HASHES: LazyLock<Vec<String>> = LazyLock::new(|| {
    use sha2::Digest as _;
    ASSETS
        .iter()
        .map(|(_, bytes)| hex::encode(sha2::Sha256::digest(bytes)))
        .collect()
});

/// `{"<path>": "<hash>"}` for every embedded file, so a service worker can
/// precache `path?v=hash` without a build step. Body and its own hash.
static MANIFEST_BODY: LazyLock<Option<(Vec<u8>, String)>> = LazyLock::new(|| {
    use sha2::Digest as _;
    let table: BTreeMap<&str, &str> = ASSETS
        .iter()
        .zip(HASHES.iter())
        .map(|((path, _), hash)| (*path, hash.as_str()))
        .collect();
    let body = serde_json::to_vec(&table)
        .inspect_err(|e| tracing::error!(error = %e, "could not encode the asset manifest"))
        .ok()?;
    let hash = hex::encode(sha2::Sha256::digest(&body));
    Some((body, hash))
});

/// The asset served at exactly `path`.
pub fn get(path: &str) -> Option<Asset> {
    debug_assert!(
        ASSETS.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "build.rs emits the table sorted and without duplicates"
    );
    if path == MANIFEST {
        return manifest();
    }
    let index = ASSETS
        .binary_search_by(|(known, _)| (*known).cmp(path))
        .ok()?;
    let (path, bytes) = ASSETS[index];
    Some(Asset {
        path,
        bytes,
        content_type: content_type(path),
        hash: HASHES.get(index)?,
    })
}

fn manifest() -> Option<Asset> {
    let (body, hash) = MANIFEST_BODY.as_ref()?;
    Some(Asset {
        path: MANIFEST,
        bytes: body,
        content_type: content_type(MANIFEST),
        hash,
    })
}

/// The asset for a `GET`: the file itself, or the app shell for a route
/// that names no file. A path with an extension that is missing stays missing.
pub fn resolve(path: &str) -> Option<Asset> {
    if let Some(asset) = get(path) {
        return Some(asset);
    }
    let last = path.rsplit('/').next().unwrap_or_default();
    if last.contains('.') {
        return None;
    }
    get(INDEX)
}

/// The strong validator for an asset: its SHA-256, quoted.
pub fn etag(asset: &Asset) -> String {
    format!("\"{}\"", asset.hash)
}

/// How long a response for `asset` may be reused, given the request's query.
/// Only a URL naming the exact content it got is immutable.
pub fn cache_control(asset: &Asset, query: Option<&str>) -> &'static str {
    let revalidated =
        asset.path == INDEX || asset.path == MANIFEST || SERVICE_WORKERS.contains(&asset.path);
    if !revalidated && version(query) == Some(asset.hash) {
        return CACHE_CONTROL_IMMUTABLE;
    }
    CACHE_CONTROL
}

/// The value of the `v` parameter in a raw query string.
fn version(query: Option<&str>) -> Option<&str> {
    query?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(name, value)| (name == VERSION_PARAMETER).then_some(value))
}

/// Whether an `If-None-Match` header names this asset's content. A weak
/// comparison, as RFC 9110 asks for on `GET` and `HEAD`.
pub fn is_fresh(if_none_match: &str, asset: &Asset) -> bool {
    let quoted = etag(asset);
    if_none_match.split(',').map(str::trim).any(|candidate| {
        let tag = candidate.strip_prefix("W/").unwrap_or(candidate);
        tag == "*" || tag == quoted
    })
}

/// The `Content-Type` for a path, by extension. Unknown types download.
pub fn content_type(path: &str) -> &'static str {
    let extension = path.rsplit_once('.').map_or("", |(_, extension)| extension);
    match extension {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::selection::{is_embedded, is_embedded_dir};
    use super::*;

    #[test]
    fn the_app_shell_is_embedded_and_typed() {
        let index = get(INDEX).expect("web/index.html is embedded");
        assert!(!index.bytes.is_empty());
        assert_eq!(index.content_type, "text/html; charset=utf-8");
        assert_eq!(etag(&index).len(), 64 + 2, "a quoted SHA-256");
    }

    #[test]
    fn an_app_route_gets_the_shell_and_a_missing_file_does_not() {
        assert_eq!(resolve("/pair").map(|asset| asset.path), Some(INDEX));
        assert_eq!(resolve("/").map(|asset| asset.path), Some(INDEX));
        assert_eq!(resolve("/missing.js"), None);
        assert_eq!(
            content_type("/app/main.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type("/blob"), "application/octet-stream");
    }

    #[test]
    fn only_runtime_files_ship() {
        for shipped in [
            "index.html",
            "app.js",
            "sw.js",
            "styles.css",
            "manifest.webmanifest",
            "assets/icons/icon-192.png",
            "ui/shell.js",
            "core/protocol/frames.js",
            "features/pairing/pair.css",
        ] {
            assert!(is_embedded(shipped), "{shipped}");
        }
        for left_out in [
            "README.md",
            "ui/NOTES.md",
            "package.json",
            "package-lock.json",
            "playwright.config.js",
            "ui/shell.test.js",
            "app.test.js",
            "dev/server.js",
            "tests/pair.spec.js",
            "scripts/build.js",
            "node_modules/left-pad/index.js",
            "ui/node_modules/x/index.js",
            "core/tests/frames.js",
            "test-results/report.js",
            "playwright-report/index.html",
            ".eslintrc.js",
            "ui/.cache/x.js",
            "other/thing.js",
            "notes.html",
            "data.json",
        ] {
            assert!(!is_embedded(left_out), "{left_out}");
        }
        assert!(is_embedded_dir("features/pairing"));
        assert!(!is_embedded_dir("node_modules"));
        assert!(!is_embedded_dir("features/dev"));
    }

    #[test]
    fn a_url_naming_its_content_is_immutable_and_the_shell_never_is() {
        let index = get(INDEX).expect("shell");
        let exact = format!("v={}", index.hash);
        assert_eq!(cache_control(&index, Some(&exact)), CACHE_CONTROL);
        let manifest = get(MANIFEST).expect("generated");
        assert_eq!(manifest.content_type, "application/json");
        let listed: BTreeMap<String, String> =
            serde_json::from_slice(manifest.bytes).expect("a JSON object");
        assert_eq!(listed.get(INDEX).map(String::as_str), Some(index.hash));

        let script = Asset {
            path: "/ui/app.js",
            ..index
        };
        assert_eq!(cache_control(&script, None), CACHE_CONTROL);
        assert_eq!(cache_control(&script, Some("v=stale")), CACHE_CONTROL);
        assert_eq!(
            cache_control(&script, Some(&format!("x=1&{exact}"))),
            CACHE_CONTROL_IMMUTABLE
        );
        let worker = Asset {
            path: "/sw.js",
            ..index
        };
        assert_eq!(cache_control(&worker, Some(&exact)), CACHE_CONTROL);
    }

    #[test]
    fn if_none_match_compares_validators_weakly() {
        let index = get(INDEX).expect("shell");
        let tag = etag(&index);
        assert!(is_fresh(&tag, &index));
        assert!(is_fresh(&format!("\"other\", W/{tag}"), &index));
        assert!(is_fresh("*", &index));
        assert!(!is_fresh("\"other\"", &index));
        assert!(!is_fresh(index.hash, &index), "an unquoted hash is no ETag");
    }
}
