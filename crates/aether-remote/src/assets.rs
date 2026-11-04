//! The web app: the committed bundle `web/dist/`, compiled into the binary by `build.rs`.

pub mod selection;

use std::sync::LazyLock;

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// The document every app route falls back to.
pub const INDEX: &str = "/index.html";

/// Every asset: a cached copy is only ever used after its `ETag` matched. The service worker
/// keeps the offline copy, so a new release shows on the next load.
pub const CACHE_CONTROL: &str = "no-cache";

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

/// The asset served at exactly `path`.
pub fn get(path: &str) -> Option<Asset> {
    debug_assert!(
        ASSETS.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "build.rs emits the table sorted and without duplicates"
    );
    let index = ASSETS
        .binary_search_by(|(known, _)| (*known).cmp(path))
        .ok()?;
    let (path, bytes) = ASSETS[index];
    let hash = HASHES.get(index)?;
    debug_assert_eq!(hash.len(), 64, "a hex SHA-256");
    Some(Asset {
        path,
        bytes,
        content_type: content_type(path),
        hash,
    })
}

/// The asset for a `GET`: the file itself, or the app shell for a route at the top of the site
/// (`/`, `/pair`). The shell links its files as `./…`, so only there do they resolve; a deeper
/// path, or one with an extension, that names no file stays missing.
pub fn resolve(path: &str) -> Option<Asset> {
    debug_assert!(path.starts_with('/'), "{path}");
    if let Some(asset) = get(path) {
        return Some(asset);
    }
    let route = path.strip_prefix('/')?;
    if route.contains('/') || route.contains('.') {
        return None;
    }
    get(INDEX)
}

/// The strong validator for an asset: its SHA-256, quoted.
pub fn etag(asset: &Asset) -> String {
    format!("\"{}\"", asset.hash)
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
    use std::path::{Path, PathBuf};

    use super::selection::{BUNDLE_DIR, REQUIRED, is_shipped_name, is_url_safe};
    use super::*;

    /// Every shipped file of the bundle on disk, as `(URL path, file)`, sorted.
    fn bundle_on_disk() -> Vec<(String, PathBuf)> {
        fn walk(dir: &Path, prefix: &str, files: &mut Vec<(String, PathBuf)>) {
            for entry in std::fs::read_dir(dir).expect("the bundle is readable") {
                let path = entry.expect("a directory entry").path();
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .expect("UTF-8");
                if !is_shipped_name(name) {
                    continue;
                }
                let relative = format!("{prefix}/{name}");
                if path.is_dir() {
                    walk(&path, &relative, files);
                } else {
                    files.push((relative, path));
                }
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(BUNDLE_DIR);
        let mut files = Vec::new();
        walk(&root, "", &mut files);
        files.sort();
        files
    }

    #[test]
    fn the_app_shell_and_its_worker_are_embedded_and_typed() {
        let index = get(INDEX).expect("web/dist/index.html is embedded");
        assert!(index.bytes.starts_with(b"<!doctype html>"));
        assert_eq!(index.content_type, "text/html; charset=utf-8");
        assert_eq!(etag(&index).len(), 64 + 2, "a quoted SHA-256");
        // At the root, so the worker's default scope is the whole app.
        let worker = get("/sw.js").expect("the service worker is embedded");
        assert_eq!(worker.content_type, "text/javascript; charset=utf-8");
        for required in REQUIRED {
            assert!(get(required).is_some(), "{required}");
        }
    }

    #[test]
    fn the_committed_bundle_is_embedded_byte_for_byte() {
        let on_disk = bundle_on_disk();
        assert!(on_disk.len() >= REQUIRED.len(), "{on_disk:?}");
        let embedded: Vec<&str> = ASSETS.iter().map(|(path, _)| *path).collect();
        let expected: Vec<&str> = on_disk.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(embedded, expected, "build.rs embeds exactly web/dist");
        for (path, file) in &on_disk {
            let asset = get(path).expect("embedded");
            let bytes = std::fs::read(file).expect("the bundle file is readable");
            assert!(
                asset.bytes == bytes.as_slice(),
                "{path} differs from web/dist"
            );
        }
    }

    #[test]
    fn a_top_level_route_gets_the_shell_and_nothing_else_does() {
        for route in ["/", "/pair", "/fleet"] {
            assert_eq!(
                resolve(route).map(|asset| asset.path),
                Some(INDEX),
                "{route}"
            );
        }
        for missing in ["/missing.js", "/pair/deeper", "/assets/", "/archon/x"] {
            assert_eq!(resolve(missing), None, "{missing}");
        }
        assert_eq!(
            content_type("/assets/main.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type("/blob"), "application/octet-stream");
    }

    #[test]
    fn the_legacy_app_is_gone() {
        for legacy in [
            "/app.js",
            "/boot.js",
            "/precache.js",
            "/push-renewal.js",
            "/styles.css",
            "/types.js",
            "/asset-manifest.json",
            "/service-worker.js",
            "/ui/shell.js",
            "/core/protocol/frames.js",
            "/features/start/pair.js",
            "/assets/icons/icon-192.png",
        ] {
            assert_eq!(resolve(legacy), None, "{legacy}");
        }
        let legacy_dirs = ["/ui/", "/core/", "/features/", "/dev/", "/tests/"];
        for (path, _) in ASSETS {
            assert!(
                !legacy_dirs.iter().any(|dir| path.starts_with(dir)),
                "{path}"
            );
        }
    }

    #[test]
    fn hidden_and_unsafe_names_never_ship() {
        for shipped in [
            "index.html",
            "assets/index-DK-nI2hR.css",
            "icons/icon-192.png",
            "a~b_c",
        ] {
            assert!(is_url_safe(shipped), "{shipped}");
        }
        for refused in [
            "", "a//b", "../x", "a/../b", ".hidden", "a b", "%2e", "é.js", "a\\b", "a/",
        ] {
            assert!(!is_url_safe(refused), "{refused:?}");
        }
        assert!(is_shipped_name("sw.js") && is_shipped_name("assets"));
        assert!(
            !is_shipped_name(".DS_Store") && !is_shipped_name(".gitkeep") && !is_shipped_name("")
        );
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
