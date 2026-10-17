use super::*;

const ARCHON: &str = r#"{"version":1,"host":"archon","commands":[
    {"name":"answer","args":"<n> <text>","about":"answer question n"},
    {"name":"questions","about":"list open questions"}
]}"#;

fn names(loaded: &HostCommands) -> Vec<&str> {
    loaded.commands.iter().map(|c| c.name.as_str()).collect()
}

#[test]
fn a_valid_file_yields_every_command_in_order_and_no_warning() {
    let loaded = HostCommands::parse(ARCHON).unwrap();
    assert_eq!(loaded.host, "archon");
    assert_eq!(names(&loaded), ["answer", "questions"]);
    assert_eq!(
        loaded.commands[0],
        HostCommand {
            name: "answer".into(),
            args: "<n> <text>".into(),
            about: "answer question n".into(),
        }
    );
    assert_eq!(loaded.commands[1].args, "");
    assert_eq!(loaded.warning, None);
    assert!(loaded.find("Answer").is_some());
    assert!(loaded.find("a").is_none());
}

#[test]
fn a_file_without_a_host_marks_its_commands_as_host() {
    let loaded = HostCommands::parse(r#"{"version":1,"commands":[{"name":"merge"}]}"#).unwrap();
    assert_eq!(loaded.host, "host");
    assert_eq!(names(&loaded), ["merge"]);
}

#[test]
fn a_bad_entry_is_refused_by_name_and_the_rest_still_load() {
    let text = r#"{"version":1,"commands":[
        {"name":"/answer"},
        {"name":"Approve"},
        {"name":"9lives"},
        {"name":"two words"},
        {"name":"reject","about":"line\nbreak"},
        {"name":"merge"},
        {"name":"merge"},
        {"name":"queue-all"}
    ]}"#;
    let loaded = HostCommands::parse(text).unwrap();
    assert_eq!(names(&loaded), ["merge", "queue-all"]);
    let warning = loaded.warning.unwrap();
    for refused in ["//answer", "/Approve", "/9lives", "/two words", "/reject"] {
        assert!(warning.contains(&format!("{refused} refused")), "{warning}");
    }
    assert!(
        warning.contains("/merge refused: listed twice"),
        "{warning}"
    );
}

#[test]
fn a_built_in_name_or_alias_is_never_shadowed() {
    let text = r#"{"version":1,"commands":[
        {"name":"model"},{"name":"pause"},{"name":"q"},{"name":"answer"}
    ]}"#;
    let loaded = HostCommands::parse(text).unwrap();
    assert_eq!(names(&loaded), ["answer"]);
    let warning = loaded.warning.unwrap();
    for name in ["model", "pause", "q"] {
        assert!(
            warning.contains(&format!("/{name} refused: shadows a built-in command")),
            "{warning}"
        );
    }
}

#[test]
fn a_broken_file_is_refused_whole() {
    for (text, reason) in [
        ("not json", "not valid JSON"),
        (r#"{"commands":[]}"#, "not valid JSON"),
        (
            r#"{"version":2,"commands":[]}"#,
            "version 2 is not supported",
        ),
        (
            r#"{"version":1,"commands":[{"about":"no name"}]}"#,
            "not valid JSON",
        ),
    ] {
        let error = HostCommands::parse(text).unwrap_err();
        assert!(error.contains(reason), "{text}: {error}");
    }
    let many = format!(
        r#"{{"version":1,"commands":[{}]}}"#,
        vec![r#"{"name":"x"}"#; MAX_COMMANDS + 1].join(",")
    );
    assert!(
        HostCommands::parse(&many)
            .unwrap_err()
            .contains("more than")
    );
}

#[test]
fn loading_a_missing_or_broken_file_leaves_no_commands_and_one_warning() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("absent.json");
    let loaded = HostCommands::load(&missing);
    assert!(loaded.commands.is_empty());
    let warning = loaded.warning.unwrap();
    assert!(warning.starts_with("host commands from "), "{warning}");
    assert!(warning.contains("absent.json ignored"), "{warning}");

    let broken = dir.path().join("broken.json");
    std::fs::write(&broken, "{").unwrap();
    let loaded = HostCommands::load(&broken);
    assert!(loaded.commands.is_empty());
    assert!(loaded.warning.unwrap().contains("not valid JSON"));

    let good = dir.path().join("archon.json");
    std::fs::write(&good, ARCHON).unwrap();
    let loaded = HostCommands::load(&good);
    assert_eq!(names(&loaded), ["answer", "questions"]);
    assert_eq!(loaded.warning, None);
}

#[test]
fn help_rows_show_usage_with_args() {
    let loaded = HostCommands::parse(ARCHON).unwrap();
    assert_eq!(loaded.heading(), "Host commands (archon)");
    let rows: Vec<_> = loaded.help_rows().collect();
    assert_eq!(
        rows,
        [
            ("/answer <n> <text>".to_owned(), "answer question n"),
            ("/questions".to_owned(), "list open questions"),
        ]
    );
}
