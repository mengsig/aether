//! Relative links in the remote-access documentation point at files and
//! headings that exist.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

const DOCS: [&str; 7] = [
    "README.md",
    "ARCHITECTURE.md",
    "docs/README.md",
    "docs/remote.md",
    "docs/remote-setup.md",
    "docs/known-limitations.md",
    "docs/configuration.md",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// GitHub's heading anchor: lowercase, punctuation dropped, spaces to hyphens.
fn slug(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

fn anchors(text: &str) -> HashSet<String> {
    let mut in_fence = false;
    let mut found = HashSet::new();
    for line in text.lines() {
        if line.starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence && line.starts_with('#') {
            found.insert(slug(line.trim_start_matches('#')));
        }
    }
    found
}

/// Every `](target)` outside code fences.
fn link_targets(text: &str) -> Vec<String> {
    let mut in_fence = false;
    let mut targets = Vec::new();
    for line in text.lines() {
        if line.starts_with("```") {
            in_fence = !in_fence;
        }
        if in_fence {
            continue;
        }
        let mut rest = line;
        while let Some(start) = rest.find("](") {
            let after = &rest[start + 2..];
            let Some(end) = after.find(')') else { break };
            targets.push(after[..end].to_owned());
            rest = &after[end..];
        }
    }
    targets
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

#[test]
fn every_relative_link_and_anchor_in_the_remote_docs_resolves() {
    let mut broken = Vec::new();
    for doc in DOCS {
        let path = root().join(doc);
        let text = read(&path);
        for target in link_targets(&text) {
            if target.starts_with("http") || target.starts_with("mailto:") {
                continue;
            }
            let (file, fragment) = target
                .split_once('#')
                .map_or((target.as_str(), None), |(f, a)| (f, Some(a)));
            let destination = if file.is_empty() {
                path.clone()
            } else {
                path.parent().expect("dir").join(file)
            };
            if !destination.exists() {
                broken.push(format!("{doc}: {target} (no such file)"));
                continue;
            }
            let Some(fragment) = fragment else { continue };
            if destination.extension().is_some_and(|e| e == "md")
                && !anchors(&read(&destination)).contains(fragment)
            {
                broken.push(format!("{doc}: {target} (no such heading)"));
            }
        }
    }
    assert!(broken.is_empty(), "broken links:\n{}", broken.join("\n"));
}

#[test]
fn the_setup_guide_is_linked_from_the_places_people_start() {
    for doc in [
        "README.md",
        "docs/README.md",
        "docs/remote.md",
        "docs/known-limitations.md",
    ] {
        assert!(
            read(&root().join(doc)).contains("remote-setup.md"),
            "{doc} does not link docs/remote-setup.md"
        );
    }
}

#[test]
fn the_guide_names_every_command_and_flag_the_wizard_prints() {
    let guide = read(&root().join("docs/remote-setup.md"));
    for needle in [
        "aether remote setup",
        "--proxy-url",
        "--funnel",
        "--yes",
        "--json",
        "aether remote disable",
        "tailscale serve reset",
        "aether remote revoke",
        "https://login.tailscale.com/admin/dns",
    ] {
        assert!(
            guide.contains(needle),
            "docs/remote-setup.md never mentions `{needle}`"
        );
    }
}
