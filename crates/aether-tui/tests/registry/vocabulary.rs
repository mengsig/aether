use super::*;

/// §9.5: the palette is the command vocabulary, so `parse` resolves names
/// through `COMMANDS` and a name the table does not carry cannot parse. The
/// old version of this test read `commands.rs` as text, which a reformat
/// broke and a guarded arm escaped.
#[test]
fn the_parser_accepts_exactly_what_the_palette_lists() {
    for spec in COMMANDS {
        for name in std::iter::once(spec.name).chain(spec.hidden_aliases.iter().copied()) {
            assert_eq!(
                commands::spec_for(name).map(|s| s.name),
                Some(spec.name),
                "/{name} resolves somewhere else"
            );
            // Case is not part of the name.
            let typed = format!("/{}", name.to_ascii_uppercase());
            assert!(
                !matches!(
                    parse(&typed),
                    InputAction::Command(SlashCommand::Unknown(_))
                ),
                "{typed} did not parse"
            );
        }
    }
    for absent in ["nope", "mdoel", "themes", "priority", "timeline", "approve"] {
        assert!(commands::spec_for(absent).is_none(), "/{absent} is listed");
        assert!(
            matches!(
                parse(&format!("/{absent}")),
                InputAction::Command(SlashCommand::Unknown(_))
            ),
            "/{absent} parsed despite not being in the palette"
        );
    }
}

#[test]
fn every_palette_entry_parses_to_a_real_command() {
    for spec in COMMANDS {
        // Typed bare, a command either runs or explains its own usage. What it
        // must never be is unknown.
        let typed = format!("/{}", spec.name);
        match parse(&typed) {
            InputAction::Command(SlashCommand::Unknown(name)) => {
                panic!("/{name} is in the palette but the parser does not know it")
            }
            InputAction::Command(_) => {}
            other => panic!("/{} parsed as {other:?}", spec.name),
        }
        for alias in spec.hidden_aliases {
            assert!(
                !matches!(
                    parse(&format!("/{alias}")),
                    InputAction::Command(SlashCommand::Unknown(_))
                ),
                "/{alias} is listed as an alias of /{} but does not parse",
                spec.name
            );
        }
    }
}

/// A command may advertise a whole key row or one of its alternatives.
/// Every advertised chord must still appear in KEYS.
#[test]
fn every_key_a_command_advertises_is_in_the_key_table() {
    let mut named = 0;
    for spec in COMMANDS {
        if spec.key.is_empty() {
            continue;
        }
        named += 1;
        assert!(
            KEYS.iter().any(|k| {
                k.keys == spec.key || k.keys.split(" / ").any(|chord| chord == spec.key)
            }),
            "/{} advertises {:?}, which is not in the key table",
            spec.name,
            spec.key
        );
    }
    assert!(
        named > 0,
        "no command names its key, so this proves nothing"
    );
}

#[test]
fn the_palette_has_no_duplicates() {
    let mut seen = std::collections::HashSet::new();
    for spec in COMMANDS {
        assert!(seen.insert(spec.name), "/{} is listed twice", spec.name);
        for alias in spec.hidden_aliases {
            assert!(seen.insert(alias), "{alias} is both a command and an alias");
        }
    }
}

/// SPEC §6.2: 30 commands, listed in rank order, aliases never listed.
#[test]
fn the_palette_is_the_ranked_vocabulary() {
    let names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
    assert_eq!(
        names,
        [
            "model",
            "new",
            "workspace",
            "agent",
            "compact",
            "copy",
            "image",
            "memory",
            "approvals",
            "usage",
            "settings",
            "remember",
            "stop",
            "cancel",
            "pause",
            "resume",
            "wake",
            "restart",
            "rename",
            "instructions",
            "artifacts",
            "mcp",
            "schedules",
            "report",
            "doctor",
            "voice",
            "login",
            "logout",
            "clear",
            "help",
            "quit",
            "mode",
            "outbox",
        ]
    );
    for (index, spec) in COMMANDS.iter().enumerate() {
        assert_eq!(
            usize::from(spec.rank),
            index + 1,
            "/{} is out of rank",
            spec.name
        );
    }
    let listed: Vec<&str> = commands::palette_matches("/")
        .iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(listed, names, "the popup lists exactly the ranked commands");
    for spec in COMMANDS {
        for alias in spec.hidden_aliases {
            assert!(
                commands::palette_matches(&format!("/{alias}"))
                    .iter()
                    .all(|c| c.name != *alias),
                "hidden alias /{alias} is listed"
            );
        }
    }
}

/// No orphan aliases: every alias the parser accepts belongs to a listed command,
/// and every alias a command claims reaches the parser.
#[test]
fn every_hidden_alias_resolves_to_its_command() {
    for spec in COMMANDS {
        for alias in spec.hidden_aliases {
            assert_eq!(
                commands::spec_for(alias).map(|s| s.name),
                Some(spec.name),
                "/{alias} is claimed by /{} but resolves elsewhere",
                spec.name
            );
        }
    }
}

#[test]
fn the_key_table_covers_what_the_spec_promises() {
    // SPEC §4.1-4.2, by the action each row must describe.
    let required = [
        "send",
        "newline",
        "queue",
        "history",
        "interrupt",
        "quit",
        "agents board",
        "parent agent",
        "details",
        "find in the transcript",
        "effort",
        "scroll the transcript",
        "shortcuts",
        "commands",
        "mention",
        "delete word back",
        "dictation",
    ];
    for needle in required {
        assert!(
            KEYS.iter().any(|k| k.action.contains(needle)),
            "no key in the table does {needle:?}"
        );
    }
    // Every scope is represented, so the help never shows an empty section.
    for scope in [
        KeyScope::Global,
        KeyScope::Input,
        KeyScope::Panel,
        KeyScope::Overlay,
    ] {
        assert!(
            KEYS.iter().any(|k| k.scope == scope),
            "no keys in scope {scope:?}"
        );
    }
}

/// SPEC §6.3: rows for removed features stay removed, and nothing binds `q` to quit.
/// Ctrl+O no longer folds tool output; it opens the workspace picker.
#[test]
fn removed_key_rows_stay_removed() {
    for gone in ["Ctrl+L", "Ctrl+N", "Ctrl+P", "Ctrl+X twice", "j", "k", "q"] {
        assert!(
            KEYS.iter()
                .all(|k| k.keys.split(" / ").all(|part| part != gone)),
            "{gone} is still in the key table"
        );
    }
}
