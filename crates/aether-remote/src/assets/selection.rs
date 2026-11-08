//! What of the committed web bundle is compiled into the binary. `build.rs` includes this file by
//! path, so it uses `core` alone. The bundle is a build output: every file in it ships but hidden ones.

/// The committed bundle, relative to this crate's manifest directory.
pub const BUNDLE_DIR: &str = "../../web/dist";

/// URL paths a usable bundle always has, including the isolated diagram renderer.
pub const REQUIRED: &[&str] = &[
    "/index.html",
    "/sw.js",
    "/manifest.webmanifest",
    "/mermaid-frame.html",
];

/// Whether a file or directory called `name` ships. Hidden ones (`.DS_Store`, `.gitkeep`) never do.
pub fn is_shipped_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.')
}

/// Whether `relative` (`/` separators, no leading slash) can be served as written: URL-safe
/// ASCII in non-empty segments, so the path needs no escaping and normalization keeps it.
pub fn is_url_safe(relative: &str) -> bool {
    let safe_byte = |byte: u8| byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte);
    !relative.is_empty()
        && relative.split('/').all(|segment| {
            !segment.is_empty() && !segment.starts_with('.') && segment.bytes().all(safe_byte)
        })
}
