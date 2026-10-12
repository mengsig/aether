//! preferences reducer regressions.
use super::*;

#[test]
fn the_overrides_survive_interactive_theme_switches() {
    use crate::ui::theme::overrides::Overrides;
    use crate::ui::theme::{ColorDepth, ThemeMode, ThemeSpec};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let config = "# keep this comment\n[theme]\naccent = \"#ff8800\"\n";
    std::fs::write(&path, config).unwrap();
    let overrides = Overrides::load(&path).unwrap().hues;
    assert!(!overrides.is_empty());
    let mut app = App::new(TuiOptions {
        theme_spec: ThemeSpec {
            mode: ThemeMode::Dark,
            depth: ColorDepth::Truecolor,
            overrides,
            ..Default::default()
        },
        prefs_dir: Some(dir.path().to_path_buf()),
        definitions_dir: Some(dir.path().to_path_buf()),
        ..Default::default()
    });
    let expected = app.theme.accent();
    for name in ["light", "mono", "nord", "dark"] {
        app.run_command(SlashCommand::SetTheme(name.parse().unwrap()));
        assert_eq!(app.theme_choice.to_string(), name);
        assert_eq!(app.theme.spec().overrides, overrides);
        if name != "mono" {
            assert_eq!(app.theme.accent(), expected, "{name}");
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), config);
    }
    let saved = crate::prefs::UiPrefs::load(dir.path()).unwrap();
    assert_eq!(saved.theme.map(|t| t.to_string()).as_deref(), Some("dark"));
}

#[test]
fn a_theme_file_is_read_when_chosen_and_its_problems_are_shown() {
    let dir = tempfile::tempdir().unwrap();
    let themes = dir.path().join("themes");
    std::fs::create_dir_all(&themes).unwrap();
    std::fs::write(
        themes.join("ember.toml"),
        "base = \"gruvbox\"\nrunning = \"#ff8800\"\n",
    )
    .unwrap();
    std::fs::write(
        themes.join("broken.toml"),
        "base = \"nord\"\nrunnign = \"#ff8800\"\n",
    )
    .unwrap();
    let mut app = App::new(TuiOptions {
        prefs_dir: Some(dir.path().to_path_buf()),
        definitions_dir: Some(dir.path().to_path_buf()),
        ..Default::default()
    });
    app.run_command(SlashCommand::SetTheme("ember".parse().unwrap()));
    let running = app
        .theme
        .spec()
        .scheme
        .dark
        .get(crate::ui::theme::Hue::Running);
    assert_eq!(running, crate::ui::theme::tokens::Rgb(0xff, 0x88, 0x00));
    assert_eq!(app.theme_choice.to_string(), "ember");

    // A broken file paints its base and names the file and the key.
    app.run_command(SlashCommand::SetTheme("broken".parse().unwrap()));
    assert_eq!(
        app.theme.spec().scheme,
        crate::ui::theme::Builtin::Nord.scheme()
    );
    let flash = app
        .flash
        .as_ref()
        .map(|f| f.text.clone())
        .unwrap_or_default();
    assert!(
        flash.contains("broken.toml") && flash.contains("runnign"),
        "{flash}"
    );

    // A missing one changes nothing, and says so.
    let before = app.theme.spec();
    app.run_command(SlashCommand::SetTheme("absent".parse().unwrap()));
    assert_eq!(app.theme.spec(), before);
    assert_eq!(app.theme_choice.to_string(), "broken");
    let flash = app
        .flash
        .as_ref()
        .map(|f| f.text.clone())
        .unwrap_or_default();
    assert!(flash.contains("no theme named absent"), "{flash}");
}

/// The saved yolo default must not move
/// until the daemon actually agrees - a refusal (daemon-wide yolo, an
/// engine that cannot gate, a stricter live descendant) must leave the
/// saved preference exactly as it was.
#[test]
fn yolo_toggle_is_saved_only_once_the_daemon_agrees() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (mut app, id) = app_with_agent();
    app.options.prefs_dir = Some(dir.path().to_path_buf());
    assert_eq!(app.options.yolo, None);

    // The request goes out; nothing is saved yet.
    let effects = app.run_command(SlashCommand::Yolo);
    assert!(matches!(
        &effects[..],
        [Effect::Request {
            command: Command::SetApprovals { .. },
            then: Then::ApprovalsSet { agent, on: true },
        }] if *agent == id
    ));
    assert_eq!(app.options.yolo, None, "nothing saved before the reply");
    assert_eq!(
        crate::prefs::UiPrefs::load(dir.path()).expect("load").yolo,
        None
    );

    // The daemon refuses (e.g. a stricter live descendant); the saved
    // default must stay untouched.
    app.reduce(AppEvent::Reply {
        then: Then::ApprovalsSet {
            agent: id,
            on: true,
        },
        result: Err(ClientError::Protocol(aether_protocol::ErrorReply::new(
            aether_protocol::ErrorCode::InvalidArgument,
            "agent is a live descendant gated less than that policy",
        ))),
    });
    assert_eq!(app.options.yolo, None, "a refusal must not move it");
    assert_eq!(
        crate::prefs::UiPrefs::load(dir.path()).expect("load").yolo,
        None
    );

    // A confirmed change does save it.
    app.reduce(AppEvent::Reply {
        then: Then::ApprovalsSet {
            agent: id,
            on: true,
        },
        result: Ok(Reply::Agents(vec![agent(id, "worker")])),
    });
    assert_eq!(app.options.yolo, Some(true));
    assert_eq!(
        crate::prefs::UiPrefs::load(dir.path()).expect("load").yolo,
        Some(true)
    );
}

/// The daemon already agreed by the time
/// this reply lands, so a failed `save_prefs` must not hide that the
/// change landed - the in-memory option and the tree refresh must still
/// happen; only the notice should say the save failed.
#[test]
fn a_failed_yolo_save_still_updates_the_option_and_refreshes_the_tree() {
    let (mut app, id) = app_with_agent();
    // A regular file, not a directory: UiPrefs::load/save both fail on it.
    let not_a_dir = tempfile::NamedTempFile::new().expect("file");
    app.options.prefs_dir = Some(not_a_dir.path().to_path_buf());
    assert_eq!(app.options.yolo, None);

    let effects = app.reduce(AppEvent::Reply {
        then: Then::ApprovalsSet {
            agent: id,
            on: true,
        },
        result: Ok(Reply::Agents(vec![agent(id, "worker")])),
    });
    assert_eq!(
        app.options.yolo,
        Some(true),
        "the daemon already agreed; a save failure must not hide that"
    );
    assert!(
        app.live_flash()
            .is_some_and(|text| text.contains("not saved")),
        "the notice must say the save failed: {:?}",
        app.flash
    );
    assert!(
        effects.iter().any(|e| matches!(
            e,
            Effect::Request {
                command: Command::GetTree { .. },
                ..
            }
        )),
        "the tree must still refresh so the view is not left stale"
    );
}

#[test]
fn remote_theme_and_mode_reload_read_shared_definitions_and_save_endpoint_state() {
    let shared = tempfile::tempdir().unwrap();
    let endpoint = tempfile::tempdir().unwrap();
    std::fs::create_dir(shared.path().join("themes")).unwrap();
    std::fs::create_dir(shared.path().join("modes")).unwrap();
    std::fs::write(shared.path().join("themes/ember.toml"), "base = \"nord\"\n").unwrap();
    std::fs::write(
        shared.path().join("modes/custom.toml"),
        "version = 1\nshow = [\"user_text\"]\n",
    )
    .unwrap();
    let mut app = App::new(TuiOptions {
        definitions_dir: Some(shared.path().into()),
        prefs_dir: Some(endpoint.path().into()),
        ..Default::default()
    });
    app.run_command(SlashCommand::SetTheme("ember".parse().unwrap()));
    assert_eq!(app.theme_choice.to_string(), "ember");
    assert_eq!(
        crate::prefs::UiPrefs::load(endpoint.path()).unwrap().theme,
        Some(app.theme_choice.clone())
    );
    assert!(!shared.path().join(crate::prefs::FILE).exists());
    let effects = app.run_command(SlashCommand::ViewMode(Some("reload".into())));
    let [Effect::ReloadViewModes(directory)] = effects.as_slice() else {
        panic!("reload effect");
    };
    assert_eq!(directory, shared.path());
    app.modes_loaded(crate::view_modes::discover(directory));
    app.run_command(SlashCommand::ViewMode(Some("custom".into())));
    assert_eq!(app.view_mode, "custom");
    assert_eq!(
        crate::prefs::UiPrefs::load(endpoint.path())
            .unwrap()
            .view_mode
            .as_deref(),
        Some("custom")
    );
}
