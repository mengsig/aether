//! Reading a release's notes out of the changelog it publishes.

use super::notes::{self, Notes};
use super::release::Version;

fn version(text: &str) -> Version {
    Version::parse(text).expect(text)
}

/// The shape `CHANGELOG.md` has: a preamble, then one section per release,
/// newest first, some with a "Before updating" section.
const CHANGELOG: &str = "# Changelog

What each release brings.

## 1.2.0

Agents can now pause.

Twice.

### New

- **Pausing.** It pauses.

### Before updating

- **Store schema 30.** Adds the pause table. The migration is one
  way: a binary that predates schema 30 refuses the store.

## 1.1.1

Fixes only.

### Fixed

- **A fix.** It is fixed.

## 1.1.0

Storage cleanup.

### Before updating

- **Storage cleanup is on by default.** Old logs go after fourteen days.

## 1.0.0

The first release.

### Before updating

- **Store schema 1.** The migration is one-way.
";

#[test]
fn an_upgrade_reads_the_targets_summary_and_every_crossed_release_oldest_first() {
    let read = notes::parse(CHANGELOG, version("1.0.0"), version("1.2.0")).expect("notes");
    assert_eq!(read.summary, "Agents can now pause.\n\nTwice.");
    let crossed: Vec<String> = read
        .before_updating
        .iter()
        .map(|note| note.version.to_string())
        .collect();
    assert_eq!(crossed, ["1.1.0", "1.2.0"], "1.0.0 is already installed");
    assert_eq!(
        read.before_updating[0].text,
        "- **Storage cleanup is on by default.** Old logs go after fourteen days."
    );
    assert!(
        read.before_updating[1].text.ends_with("refuses the store."),
        "the section ends at the next release: {}",
        read.before_updating[1].text
    );
    assert!(read.one_way_migration(), "said across a line break");
}

#[test]
fn notes_without_a_one_way_migration_ask_for_nothing() {
    let read = notes::parse(CHANGELOG, version("1.0.0"), version("1.1.1")).expect("notes");
    assert_eq!(read.summary, "Fixes only.");
    assert_eq!(read.before_updating.len(), 1);
    assert!(!read.one_way_migration());
    let read = notes::parse(CHANGELOG, version("1.1.0"), version("1.1.1")).expect("notes");
    assert!(read.before_updating.is_empty());
    assert!(!Notes::Published(read).one_way_migration());
}

#[test]
fn a_changelog_that_does_not_describe_the_release_is_refused() {
    for (changelog, says) in [
        (CHANGELOG, "no section for 9.0.0"),
        ("## 9.0.0\n\n### New\n\n- x\n", "has no summary"),
        (
            "## 9.0.0\n\nA.\n\n## 9.0.0\n\nB.\n",
            "two sections for 9.0.0",
        ),
    ] {
        let error = notes::parse(changelog, version("1.0.0"), version("9.0.0"))
            .expect_err("refused")
            .to_string();
        assert!(error.contains(says), "{error}");
    }
    let error = notes::parse(CHANGELOG, version("1.2.0"), version("1.1.0"))
        .expect_err("a downgrade has no notes to read")
        .to_string();
    assert!(error.contains("upgrade only"), "{error}");
}

#[test]
fn a_heading_that_names_no_release_ends_the_section_before_it() {
    let changelog = "## Unreleased\n\nNot yet.\n\n## 2.0.0\n\nNew.\n\n### Before updating\n\n- Read this.\n\n## Notes\n\n- Not a release note.\n";
    let read = notes::parse(changelog, version("1.0.0"), version("2.0.0")).expect("notes");
    assert_eq!(read.summary, "New.");
    assert_eq!(read.before_updating[0].text, "- Read this.");
}

#[test]
fn rendered_notes_name_each_release_and_json_says_whether_any_were_published() {
    let read = notes::parse(CHANGELOG, version("1.0.0"), version("1.2.0")).expect("notes");
    let published = Notes::Published(read);
    let text = published.render(version("1.2.0"));
    assert!(
        text.starts_with("What 1.2.0 brings:\n\nAgents can now pause."),
        "{text}"
    );
    let before_1_1 = text.find("Before updating to 1.1.0:").expect("1.1.0");
    let before_1_2 = text.find("Before updating to 1.2.0:").expect("1.2.0");
    assert!(before_1_1 < before_1_2, "oldest first: {text}");
    let json = published.to_json();
    assert_eq!(json["published"], true);
    assert_eq!(json["one_way_migration"], true);
    assert_eq!(json["before_updating"][0]["version"], "1.1.0");

    let page = "https://example.com/releases/tag/v1.2.0".to_owned();
    let unpublished = Notes::Unpublished { page: page.clone() };
    assert!(!unpublished.one_way_migration());
    assert!(unpublished.render(version("1.2.0")).contains(&page));
    assert_eq!(
        unpublished.to_json(),
        serde_json::json!({"published": false, "url": page})
    );
}

/// The repository's own changelog, over releases that are history: 1.0.7 to
/// 1.0.9 each migrate the store, 1.0.9's wording breaking "one way" across lines.
#[test]
fn the_real_changelog_reads_as_the_notes_of_the_releases_it_crosses() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../CHANGELOG.md");
    let changelog = std::fs::read_to_string(path).expect("CHANGELOG.md");
    let read = notes::parse(&changelog, version("1.0.6"), version("1.0.9")).expect("notes");
    assert!(
        read.summary.starts_with("Cost figures are more accurate."),
        "{}",
        read.summary
    );
    let crossed: Vec<String> = read
        .before_updating
        .iter()
        .map(|note| note.version.to_string())
        .collect();
    assert_eq!(crossed, ["1.0.7", "1.0.8", "1.0.9"]);
    assert!(
        read.before_updating[2]
            .text
            .contains("**Store schema 24.**")
    );
    assert!(read.one_way_migration());
    let patch = notes::parse(&changelog, version("1.0.4"), version("1.0.6")).expect("notes");
    assert!(patch.before_updating.is_empty(), "{patch:?}");
}
