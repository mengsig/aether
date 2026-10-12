//! Compile the runtime files of `web/` into the binary as an `include_bytes!` table sorted by URL
//! path; no Node, no bundler. `selection.rs` decides what counts as a runtime file.

// A build script has nobody to return an error to: it stops the build.
#![allow(clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[path = "src/assets/selection.rs"]
mod selection;

fn main() {
    let manifest = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let root = manifest.join("web");
    // Cargo rescans a directory given here, so adding a file rebuilds too.
    println!("cargo:rerun-if-changed={}", root.display());
    println!("cargo:rerun-if-changed=src/assets/selection.rs");
    println!("cargo:rerun-if-changed=build.rs");
    let mut files = Vec::new();
    collect(&root, "", &mut files);
    files.sort();
    assert!(
        files.iter().any(|(url, _)| url == "/index.html"),
        "web/index.html is the app's entry point and must exist"
    );
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

/// Every runtime file under `dir`, as `(URL path, file)`. `prefix` is the
/// path of `dir` below `web/`, empty at the top or ending in `/`.
fn collect(dir: &Path, prefix: &str, files: &mut Vec<(String, PathBuf)>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let relative = format!("{prefix}{name}");
        if path.is_dir() {
            // Pruned here, so `node_modules` is never walked.
            if selection::is_embedded_dir(&relative) {
                collect(&path, &format!("{relative}/"), files);
            }
        } else if selection::is_embedded(&relative) {
            assert!(relative.is_ascii(), "asset paths are ASCII: {relative}");
            files.push((format!("/{relative}"), path));
        }
    }
}
