#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use aether_core::view_modes::{self, Catalog, Category, Definition, Density, RESERVED_NAMES};
use std::collections::BTreeSet;

#[test]
fn builtins_have_stable_order_and_standard_covers_every_category() {
    let catalog = Catalog::default();
    assert_eq!(
        catalog
            .ordered()
            .map(|mode| mode.name.as_str())
            .collect::<Vec<_>>(),
        ["standard", "focus"]
    );
    let standard = &catalog.modes["standard"].definition;
    assert_eq!(standard.density, Density::Comfortable);
    assert_eq!(standard.show, Category::ALL);
    assert_eq!(Category::ALL.into_iter().collect::<BTreeSet<_>>().len(), 14);
    assert!(
        catalog.modes["focus"]
            .definition
            .show
            .iter()
            .all(|category| standard.show.contains(category))
    );
    assert_eq!(
        serde_json::to_value(Category::ALL).unwrap(),
        serde_json::json!([
            "user_text",
            "assistant_text",
            "delegation",
            "agent_message",
            "completion",
            "status",
            "question",
            "approval",
            "error",
            "tool",
            "reasoning",
            "memory",
            "compaction",
            "usage"
        ])
    );
}

#[test]
fn every_reserved_name_is_refused_by_discovery() {
    let config = tempfile::tempdir().unwrap();
    let modes = config.path().join("modes");
    std::fs::create_dir(&modes).unwrap();
    for name in RESERVED_NAMES {
        std::fs::write(modes.join(format!("{name}.toml")), "version=1\nshow=[]").unwrap();
    }
    let catalog = view_modes::discover(config.path());
    assert_eq!(catalog.modes, Catalog::default().modes);
    assert_eq!(catalog.diagnostics.len(), RESERVED_NAMES.len());
}

#[test]
fn discovery_accepts_exact_limits_and_refuses_the_next_byte_or_file() {
    let config = tempfile::tempdir().unwrap();
    let modes = config.path().join("modes");
    std::fs::create_dir(&modes).unwrap();
    let text = "version=1\nshow=['tool']\n#";
    let limit = format!("{text}{}", "x".repeat(64 * 1024 - text.len()));
    for index in 0..128 {
        std::fs::write(modes.join(format!("mode{index:03}.toml")), &limit).unwrap();
    }
    let catalog = view_modes::discover(config.path());
    assert_eq!(catalog.modes.len(), 130);
    assert!(catalog.diagnostics.is_empty());
    std::fs::write(modes.join("mode000.toml"), format!("{limit}x")).unwrap();
    let catalog = view_modes::discover(config.path());
    assert_eq!(catalog.modes.len(), 129);
    assert_eq!(catalog.diagnostics[0].reason, "mode exceeds 64 KiB");
    std::fs::write(modes.join("overflow.toml"), text).unwrap();
    let catalog = view_modes::discover(config.path());
    assert_eq!(catalog.modes, Catalog::default().modes);
    assert!(
        catalog.diagnostics[0]
            .reason
            .contains("external catalog rejected")
    );
}

#[test]
fn parser_preserves_exact_scope_and_rejects_invalid_definitions() {
    let definition = Definition::parse(
        "version=1\nshow=['tool']\n[scope]\nname='archon'\nrole='archon-orchestrator'",
    )
    .unwrap();
    assert!(definition.matches(Some("archon"), Some("archon-orchestrator")));
    assert!(!definition.matches(Some("Archon"), Some("archon-orchestrator")));
    assert!(!definition.matches(Some("archon"), None));
    for invalid in [
        "version=2\nshow=[]",
        "version=1\nshow=['tool','tool']",
        "version=1\nshow=[]\n[scope]",
    ] {
        assert!(Definition::parse(invalid).is_err(), "{invalid}");
    }
}
