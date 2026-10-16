//! What a release asks of the person installing it: its summary, and the
//! "Before updating" sections of every release an update crosses, read from
//! the `CHANGELOG.md` the release publishes beside its archives.

use anyhow::{bail, ensure};

use super::release::Version;

/// The asset holding the changelog as of a release, that release's section included.
pub(super) const NOTES_ASSET: &str = "CHANGELOG.md";
const BEFORE_UPDATING: &str = "### Before updating";

/// What the release being installed says about itself, if it says anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Notes {
    Published(ReleaseNotes),
    /// Releases before the notes asset existed publish none; `page` has them.
    Unpublished {
        page: String,
    },
}

impl Notes {
    pub(super) fn one_way_migration(&self) -> bool {
        matches!(self, Self::Published(notes) if notes.one_way_migration())
    }

    /// What is shown before anything is downloaded.
    pub(super) fn render(&self, wanted: Version) -> String {
        let notes = match self {
            Self::Unpublished { page } => {
                return format!(
                    "{wanted} publishes no release notes for `aether update`; read them at {page}."
                );
            }
            Self::Published(notes) => notes,
        };
        let mut text = format!("What {wanted} brings:\n\n{}", notes.summary);
        for note in &notes.before_updating {
            text.push_str(&format!(
                "\n\nBefore updating to {}:\n\n{}",
                note.version, note.text
            ));
        }
        text
    }

    /// The notes as `--json` reports them.
    pub(super) fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Unpublished { page } => serde_json::json!({"published": false, "url": page}),
            Self::Published(notes) => notes.to_json(),
        }
    }
}

/// One release's "Before updating" section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BeforeUpdating {
    pub(super) version: Version,
    pub(super) text: String,
}

/// What an update from one release to a newer one should be read first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReleaseNotes {
    /// The target release's opening text, before its first `###` heading.
    pub(super) summary: String,
    /// Every crossed release that has the section, oldest first.
    pub(super) before_updating: Vec<BeforeUpdating>,
}

impl ReleaseNotes {
    /// Whether a crossed release migrates the store one way. The release policy
    /// (docs/ci.md) has every such note say "one way" or "one-way".
    pub(super) fn one_way_migration(&self) -> bool {
        self.before_updating.iter().any(|note| {
            let words = note.text.split_whitespace().collect::<Vec<_>>().join(" ");
            let words = words.to_ascii_lowercase();
            words.contains("one way") || words.contains("one-way")
        })
    }

    fn to_json(&self) -> serde_json::Value {
        let before: Vec<_> = self
            .before_updating
            .iter()
            .map(|note| serde_json::json!({"version": note.version.to_string(), "text": note.text}))
            .collect();
        serde_json::json!({
            "published": true,
            "summary": self.summary,
            "before_updating": before,
            "one_way_migration": self.one_way_migration(),
        })
    }
}

/// The notes for updating from `running` to `wanted`, from `wanted`'s changelog.
pub(super) fn parse(
    changelog: &str,
    running: Version,
    wanted: Version,
) -> anyhow::Result<ReleaseNotes> {
    ensure!(
        running < wanted,
        "release notes are read for an upgrade only, not {running} to {wanted}"
    );
    let sections = sections(changelog)?;
    let Some((_, target)) = sections.iter().find(|(version, _)| *version == wanted) else {
        bail!("the published changelog has no section for {wanted}");
    };
    let summary = up_to_heading(target);
    ensure!(
        !summary.is_empty(),
        "the changelog section for {wanted} has no summary"
    );
    let mut before_updating: Vec<BeforeUpdating> = sections
        .iter()
        .filter(|(version, _)| running < *version && *version <= wanted)
        .filter_map(|(version, lines)| {
            let text = before_updating_text(lines);
            (!text.is_empty()).then_some(BeforeUpdating {
                version: *version,
                text,
            })
        })
        .collect();
    before_updating.sort_by_key(|note| note.version);
    Ok(ReleaseNotes {
        summary,
        before_updating,
    })
}

/// Each `## MAJOR.MINOR.PATCH` section and its lines. A `##` heading that names
/// no version ends the section before it and starts none.
fn sections(changelog: &str) -> anyhow::Result<Vec<(Version, Vec<&str>)>> {
    let mut sections: Vec<(Version, Vec<&str>)> = Vec::new();
    let mut current: Option<(Version, Vec<&str>)> = None;
    for line in changelog.lines() {
        let Some(heading) = line.strip_prefix("## ") else {
            if let Some((_, lines)) = current.as_mut() {
                lines.push(line);
            }
            continue;
        };
        sections.extend(current.take());
        current = Version::parse(heading.trim())
            .ok()
            .map(|version| (version, Vec::new()));
        if let Some((version, _)) = &current {
            ensure!(
                sections.iter().all(|(seen, _)| seen != version),
                "the published changelog has two sections for {version}"
            );
        }
    }
    sections.extend(current);
    Ok(sections)
}

/// The text under a section's "Before updating" heading, up to the next heading.
fn before_updating_text(lines: &[&str]) -> String {
    let start = lines
        .iter()
        .position(|line| line.trim_end() == BEFORE_UPDATING);
    let Some(start) = start else {
        return String::new();
    };
    up_to_heading(&lines[start + 1..])
}

/// The lines before the first `###` heading, joined, without the blank lines around them.
fn up_to_heading(lines: &[&str]) -> String {
    let end = lines
        .iter()
        .position(|line| line.starts_with("### "))
        .unwrap_or(lines.len());
    lines[..end].join("\n").trim().to_owned()
}
