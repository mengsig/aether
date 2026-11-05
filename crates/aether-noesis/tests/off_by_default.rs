#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! ARCHITECTURE.md §g at the crate's own boundary: the settings default to
//! off, prerequisites decide what is effective, a disabled service opens
//! nothing, and no tool is registered without a running service.

use aether_noesis::budget::LaneSpend;
use aether_noesis::settings::{EngineeringExtraction, NoesisSettings, SessionExtraction};
use aether_noesis::tools::registered_tools;
use aether_noesis::{DB_FILE, Noesis};

/// The roles of the agents that call Noesis tools; extraction and audit jobs call none.
const ROLES: &[&str] = &["noesis.prober", "noesis.teacher", "noesis.practitioner"];

fn today() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}

#[test]
fn settings_default_to_off_and_effective_follows_prerequisites() {
    let empty = NoesisSettings::from_table(&toml::Table::new()).expect("an empty [noesis]");
    assert_eq!(
        empty,
        NoesisSettings::default(),
        "a missing key takes its default"
    );
    assert!(!empty.enabled && !empty.engineering_context && !empty.billing_allow_api);
    assert_eq!(empty.extraction_sessions, SessionExtraction::Manual);
    assert_eq!(empty.extraction_engineering, EngineeringExtraction::Off);

    let stored = NoesisSettings {
        engineering_context: true,
        extraction_sessions: SessionExtraction::Auto,
        extraction_engineering: EngineeringExtraction::Auto,
        ..NoesisSettings::default()
    };
    let off = stored.effective();
    assert!(
        !off.engineering_context,
        "engineering context needs enabled"
    );
    assert_eq!(off.extraction_sessions, SessionExtraction::Manual);
    assert_eq!(off.extraction_engineering, EngineeringExtraction::Off);
    assert_eq!(
        stored.extraction_engineering,
        EngineeringExtraction::Auto,
        "stored values stay"
    );

    let on = NoesisSettings {
        enabled: true,
        ..stored.clone()
    }
    .effective();
    assert!(on.engineering_context);
    assert_eq!(on.extraction_engineering, EngineeringExtraction::Auto);
    let no_context = NoesisSettings {
        enabled: true,
        engineering_context: false,
        ..stored
    }
    .effective();
    assert_eq!(no_context.extraction_sessions, SessionExtraction::Auto);
    assert_eq!(
        no_context.extraction_engineering,
        EngineeringExtraction::Off,
        "needs engineering context too"
    );
}

#[test]
fn a_disabled_service_opens_nothing_and_an_enabled_one_opens_noesis_db() {
    let dir = tempfile::tempdir().unwrap();
    let off = Noesis::start_if_enabled(
        &NoesisSettings::default(),
        dir.path(),
        today(),
        LaneSpend::default(),
    );
    assert!(
        off.expect("starting a disabled service is no error")
            .is_none()
    );
    let entries = std::fs::read_dir(dir.path()).unwrap().count();
    assert_eq!(
        entries, 0,
        "a disabled service created nothing in the data dir"
    );

    let enabled = NoesisSettings {
        enabled: true,
        ..NoesisSettings::default()
    };
    let on = Noesis::start_if_enabled(&enabled, dir.path(), today(), LaneSpend::default());
    let on = on.expect("an enabled service starts").expect("and runs");
    assert!(
        dir.path().join(DB_FILE).exists(),
        "the enabled service opened {DB_FILE}"
    );
    on.stop().expect("stops");
}

#[test]
fn no_tool_is_registered_without_a_running_service_or_for_another_role() {
    for role in ROLES {
        assert!(
            registered_tools(None, role).is_empty(),
            "{role} got tools with no service"
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let enabled = NoesisSettings {
        enabled: true,
        ..NoesisSettings::default()
    };
    let running = Noesis::start_if_enabled(&enabled, dir.path(), today(), LaneSpend::default())
        .unwrap()
        .expect("running");
    for role in ROLES {
        let tools = registered_tools(Some(&running), role);
        assert!(
            !tools.is_empty(),
            "{role} gets its tools from a running service"
        );
        assert!(
            tools.iter().all(|tool| tool.name.starts_with("noesis.")),
            "{role}: only Noesis tools"
        );
    }
    for role in [
        "worker",
        "reviewer",
        "noesis",
        "noesisx.teacher",
        "noesis.extractor",
    ] {
        assert!(
            registered_tools(Some(&running), role).is_empty(),
            "{role} is not a Noesis role"
        );
    }
    running.stop().unwrap();
}
