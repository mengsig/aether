//! Compile the committed web bundle, `web/dist/` at the repository root, into the binary as an
//! `include_bytes!` table sorted by URL path; no Node, no bundler. `selection.rs` says what ships.

// A build script has nobody to return an error to: it stops the build.
#![allow(clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[path = "src/assets/selection.rs"]
mod selection;

fn main() {
    let manifest = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let root = manifest.join(selection::BUNDLE_DIR);
    // Cargo rescans a directory given here, so adding a file rebuilds too.
    println!("cargo:rerun-if-changed={}", root.display());
    println!("cargo:rerun-if-changed=src/assets/selection.rs");
    println!("cargo:rerun-if-changed=build.rs");
    assert!(
        root.is_dir(),
        "{} is the committed web bundle (npm run build in web/) and must exist",
        root.display()
    );
    let mut files = Vec::new();
    collect(&root, "", &mut files);
    files.sort();
    for entry in selection::REQUIRED {
        assert!(
            files.iter().any(|(url, _)| url == entry),
            "the bundle has no {entry}: rebuild web/dist with npm run build in web/"
        );
    }
    assert!(
        files.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "asset paths are unique"
    );
    let mut table = String::from("pub(crate) static ASSETS: &[(&str, &[u8])] = &[\n");
    for (url, file) in &files {
        writeln!(table, "    ({url:?}, include_bytes!({file:?})),").expect("write to a String");
    }
    table.push_str("];\n");
    let out = PathBuf::from(env("OUT_DIR")).join("assets.rs");
    std::fs::write(&out, table).unwrap_or_else(|e| panic!("writing {}: {e}", out.display()));
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Every shipped file under `dir`, as `(URL path, file)`. `prefix` is the
/// path of `dir` below the bundle, empty at the top or ending in `/`.
fn collect(dir: &Path, prefix: &str, files: &mut Vec<(String, PathBuf)>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_else(|| panic!("{}: a bundle file name is UTF-8", path.display()));
        if !selection::is_shipped_name(name) {
            continue;
        }
        let relative = format!("{prefix}{name}");
        assert!(
            selection::is_url_safe(&relative),
            "bundle paths use URL-safe ASCII only: {relative}"
        );
        if path.is_dir() {
            collect(&path, &format!("{relative}/"), files);
        } else {
            assert!(path.is_file(), "{}: not a regular file", path.display());
            files.push((format!("/{relative}"), path));
        }
    }
}
