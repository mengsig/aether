//! Which files under `web/` are compiled into the binary. `build.rs` includes this file by path,
//! so it uses `std` alone; the tree's tests, tooling and notes do not ship.

/// Directories of runtime files, embedded recursively.
pub const RUNTIME_DIRS: &[&str] = &["assets", "ui", "core", "features"];

/// Directories never embedded, at any depth.
pub const EXCLUDED_DIRS: &[&str] = &[
    "dev",
    "tests",
    "scripts",
    "node_modules",
    "test-results",
    "playwright-report",
];

/// Package and tool files never embedded, at any depth.
const TOOL_FILES: &[&str] = &[
    "package.json",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lockb",
    "tsconfig.json",
    "jsconfig.json",
];

/// Whether a directory under `web/` can hold embedded files at all.
/// `relative` uses `/` separators and has no leading slash.
pub fn is_embedded_dir(relative: &str) -> bool {
    let first = relative.split('/').next().unwrap_or_default();
    RUNTIME_DIRS.contains(&first) && relative.split('/').all(is_shipped_dir_name)
}

/// Whether the file at `relative` (`/` separators, no leading slash) ships.
pub fn is_embedded(relative: &str) -> bool {
    let Some((dir, name)) = relative.rsplit_once('/') else {
        return is_shipped_file_name(relative) && is_root_runtime_file(relative);
    };
    is_embedded_dir(dir) && is_shipped_file_name(name)
}

fn is_shipped_dir_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !EXCLUDED_DIRS.contains(&name)
}

fn is_shipped_file_name(name: &str) -> bool {
    let hidden = name.is_empty() || name.starts_with('.');
    let test = name.ends_with(".test.js");
    let notes = name.ends_with(".md");
    !hidden && !test && !notes && !is_tool_file(name)
}

/// `package.json`, lockfiles, and `<tool>.config.<ext>` such as
/// `playwright.config.js`.
fn is_tool_file(name: &str) -> bool {
    let config = name
        .rsplit_once('.')
        .is_some_and(|(stem, _)| stem.ends_with(".config"));
    config || TOOL_FILES.contains(&name)
}

/// At the top of `web/` only the entry points are runtime files.
fn is_root_runtime_file(name: &str) -> bool {
    name == "index.html"
        || name == "manifest.webmanifest"
        || name.ends_with(".js")
        || name.ends_with(".css")
}
