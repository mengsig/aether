use super::*;
use clap::Parser as _;

/// VISION §17: "CLI parity with `--json`". One test walks the whole command
/// table and holds every command that prints to it, so a command added
/// without `--json` fails here rather than being found by whoever tried to
/// script it.
#[cfg(test)]
mod json_parity {
    use clap::CommandFactory as _;

    /// Commands that take a terminal, a socket or a process instead of
    /// printing a result. Each says why, so adding one is a decision rather
    /// than a way past the test.
    const NOT_A_PRINTING_COMMAND: &[(&str, &str)] = &[
        ("aether tui", "it is the terminal UI"),
        ("aether daemon", "it becomes the daemon"),
        ("aether mcp-bridge", "it speaks MCP on stdio to an engine"),
        ("aether mcp serve", "it serves MCP on stdio or HTTP"),
        (
            "aether login",
            "an interactive flow: it prints a URL and waits for the person",
        ),
        ("aether serve", "it becomes the webhook process"),
        (
            "aether schema",
            "its whole output is the JSON Schema document",
        ),
        (
            "aether remote setup",
            "a guided flow: it asks before each step and waits for the person",
        ),
        (
            "aether remote pair",
            "an interactive flow: it shows a code and waits for a confirmation",
        ),
        ("aether bridge", "it is a byte pump on stdio for ssh"),
    ];

    /// Every leaf command, as the path a person would type, with its flags.
    fn leaves(command: &clap::Command, prefix: &str, out: &mut Vec<(String, Vec<String>)>) {
        let path = if prefix.is_empty() {
            command.get_name().to_owned()
        } else {
            format!("{prefix} {}", command.get_name())
        };
        if command.get_subcommands().next().is_none() {
            let flags = command
                .get_arguments()
                .map(|a| a.get_id().to_string())
                .collect();
            out.push((path, flags));
            return;
        }
        for sub in command.get_subcommands() {
            leaves(sub, &path, out);
        }
    }

    fn table() -> Vec<(String, Vec<String>)> {
        let mut found = Vec::new();
        leaves(&super::Args::command(), "", &mut found);
        found
    }

    #[test]
    fn every_command_that_prints_has_json() {
        let found = table();
        assert!(
            found.len() > 20,
            "the walk found only {} commands",
            found.len()
        );
        let missing: Vec<&String> = found
            .iter()
            .filter(|(path, _)| !NOT_A_PRINTING_COMMAND.iter().any(|(n, _)| n == path))
            .filter(|(_, flags)| !flags.iter().any(|f| f == "json"))
            .map(|(path, _)| path)
            .collect();
        assert!(
            missing.is_empty(),
            "these commands print with no --json: {missing:?}. Add the flag, or add the \
             command to NOT_A_PRINTING_COMMAND with the reason."
        );
    }

    #[test]
    fn the_exemption_list_names_only_commands_that_exist() {
        let found = table();
        for (name, _) in NOT_A_PRINTING_COMMAND {
            assert!(
                found.iter().any(|(path, _)| path == name),
                "{name} is exempted but is not a command; the list has gone stale"
            );
        }
    }
}

#[cfg(test)]
mod network_flag_tests {
    use super::*;

    #[test]
    fn network_flag_supports_inheritance_allow_and_explicit_deny() {
        for (flags, expected) in [
            (vec![], None),
            (vec!["--network"], Some(true)),
            (vec!["--network=false"], Some(false)),
        ] {
            let mut args = vec!["aether", "run", "hello"];
            args.extend(flags);
            let parsed = Args::try_parse_from(args).expect("valid network override");
            assert!(
                matches!(parsed.command, Some(Cmd::Run { network, .. }) if network == expected)
            );
        }
    }
}

#[cfg(test)]
mod schedule_cli_tests {
    use super::*;

    #[test]
    fn schedules_require_exactly_one_positive_recurrence() {
        let base = [
            "aether",
            "schedule",
            "create",
            "Read docs",
            "--target",
            "agt_fixture",
            "--payload",
            "Read the documentation",
        ];
        let parse = |extra: &[&str]| {
            Args::try_parse_from(base.iter().copied().chain(extra.iter().copied()))
        };
        assert!(parse(&[]).is_err());
        assert!(parse(&["--every", "0"]).is_err());
        assert!(parse(&["--every", "60", "--cron", "* * * * *"]).is_err());
        assert!(parse(&["--every", "60", "--timezone", "UTC"]).is_err());
        assert!(parse(&["--every", "60", "--max-deliveries", "0"]).is_err());
        assert!(parse(&["--every", "60", "--json"]).is_ok());
        assert!(
            parse(&[
                "--cron",
                "0 9 * * *",
                "--timezone",
                "Europe/Copenhagen",
                "--skip-missed"
            ])
            .is_ok()
        );
    }

    #[test]
    fn schedules_name_a_target_or_a_spawn_objective_but_not_both() {
        let parse = |extra: &[&str]| {
            Args::try_parse_from(
                ["aether", "schedule", "create", "Sweep", "--every", "3600"]
                    .into_iter()
                    .chain(extra.iter().copied()),
            )
        };
        assert!(parse(&[]).is_err());
        assert!(parse(&["--payload", "x"]).is_err());
        assert!(parse(&["--target", "agt_fixture", "--objective", "Sweep"]).is_err());
        assert!(parse(&["--target", "agt_fixture", "--payload", "x", "--queue-one"]).is_ok());
        assert!(
            parse(&[
                "--target",
                "agt_fixture",
                "--payload",
                "x",
                "--model",
                "a/b"
            ])
            .is_err()
        );
        assert!(parse(&["--objective", "Sweep", "--queue-one"]).is_err());
        let spawn = parse(&[
            "--objective",
            "Sweep",
            "--model",
            "mock/m1",
            "--effort",
            "high",
            "--token-budget",
            "50000",
            "--max-turns",
            "20",
            "--network=false",
            "--backend",
            "host",
        ])
        .expect("spawn template flags parse");
        assert!(matches!(
            spawn.command,
            Some(Cmd::Schedule {
                command: ScheduleCmd::Create {
                    target: None,
                    payload: None,
                    network: Some(false),
                    ..
                }
            })
        ));
    }
}

#[cfg(test)]
mod github_watcher_cli_tests {
    use super::*;

    #[test]
    fn allowed_users_parse_on_add_and_the_dedicated_command() {
        let parsed = Args::try_parse_from([
            "aether",
            "github-watcher",
            "add",
            "owner/repo",
            "--allowed-users",
            "octoCat,monaLisa",
        ])
        .expect("CLI");
        let Some(Cmd::GithubWatcher {
            command: GithubWatcherCmd::Add { allowed_users, .. },
        }) = parsed.command
        else {
            panic!("github-watcher add");
        };
        assert_eq!(
            allowed_users,
            Some(vec!["octoCat".into(), "monaLisa".into()])
        );
        let parsed = Args::try_parse_from([
            "aether",
            "github-watcher",
            "allowed-users",
            "owner/repo",
            "octoCat",
            "monaLisa",
        ])
        .expect("CLI");
        let Some(Cmd::GithubWatcher {
            command: GithubWatcherCmd::AllowedUsers { logins, clear, .. },
        }) = parsed.command
        else {
            panic!("github-watcher allowed-users");
        };
        assert_eq!(logins, ["octoCat", "monaLisa"]);
        assert!(!clear);
        let cleared = [
            "aether",
            "github-watcher",
            "allowed-users",
            "owner/repo",
            "--clear",
        ];
        assert!(Args::try_parse_from(cleared).is_ok());
        // Neither logins nor --clear, or both, is ambiguous.
        assert!(
            Args::try_parse_from(["aether", "github-watcher", "allowed-users", "owner/repo"])
                .is_err()
        );
        assert!(
            Args::try_parse_from([
                "aether",
                "github-watcher",
                "allowed-users",
                "owner/repo",
                "octoCat",
                "--clear",
            ])
            .is_err()
        );
    }

    #[test]
    fn operator_users_parse_as_a_list_or_clear_and_never_both() {
        let parsed = Args::try_parse_from([
            "aether",
            "github-watcher",
            "operator-users",
            "mengsig",
            "octoCat,monaLisa",
        ])
        .expect("CLI");
        let Some(Cmd::GithubWatcher {
            command: GithubWatcherCmd::OperatorUsers { logins, clear, .. },
        }) = parsed.command
        else {
            panic!("github-watcher operator-users");
        };
        assert_eq!(logins, ["mengsig", "octoCat", "monaLisa"]);
        assert!(!clear);
        let parsed =
            Args::try_parse_from(["aether", "github-watcher", "operator-users", "--clear"])
                .expect("CLI");
        let Some(Cmd::GithubWatcher {
            command: GithubWatcherCmd::OperatorUsers { logins, clear, .. },
        }) = parsed.command
        else {
            panic!("github-watcher operator-users --clear");
        };
        assert!(clear && logins.is_empty());
        // Neither logins nor --clear, or both, is ambiguous.
        assert!(Args::try_parse_from(["aether", "github-watcher", "operator-users"]).is_err());
        assert!(
            Args::try_parse_from([
                "aether",
                "github-watcher",
                "operator-users",
                "mengsig",
                "--clear",
            ])
            .is_err()
        );
    }

    #[test]
    fn adding_a_repository_accepts_no_target_at_the_clap_level() {
        // Whether a target is required depends on whether the repository
        // already exists, which clap cannot know; `run()` enforces that.
        assert!(matches!(
            Args::try_parse_from(["aether", "github-watcher", "add", "owner/repo"])
                .expect("CLI")
                .command,
            Some(Cmd::GithubWatcher {
                command: GithubWatcherCmd::Add { .. }
            })
        ));
        assert!(matches!(
            Args::try_parse_from([
                "aether",
                "github-watcher",
                "add",
                "owner/repo",
                "--agent",
                "agt_fixture",
                "--json"
            ])
            .expect("CLI"),
            Args {
                command: Some(Cmd::GithubWatcher {
                    command: GithubWatcherCmd::Add { .. }
                }),
                ..
            }
        ));
    }

    #[test]
    fn spawn_flags_conflict_with_agent_and_role_requires_objective() {
        assert!(
            Args::try_parse_from([
                "aether",
                "github-watcher",
                "add",
                "owner/repo",
                "--agent",
                "agt_fixture",
                "--spawn-role",
                "worker",
                "--spawn-objective",
                "Handle threads.",
            ])
            .is_err()
        );
        assert!(
            Args::try_parse_from([
                "aether",
                "github-watcher",
                "add",
                "owner/repo",
                "--spawn-role",
                "worker",
            ])
            .is_err()
        );
        assert!(
            Args::try_parse_from([
                "aether",
                "github-watcher",
                "add",
                "owner/repo",
                "--spawn-objective",
                "Handle threads.",
            ])
            .is_err()
        );
        assert!(matches!(
            Args::try_parse_from([
                "aether",
                "github-watcher",
                "add",
                "owner/repo",
                "--spawn-role",
                "worker",
                "--spawn-objective",
                "Handle threads.",
                "--spawn-effort",
                "high",
            ])
            .expect("CLI")
            .command,
            Some(Cmd::GithubWatcher {
                command: GithubWatcherCmd::Add { .. }
            })
        ));
    }

    #[test]
    fn regex_arguments_are_preserved_and_enabling_can_be_scoped() {
        let args = Args::try_parse_from([
            "aether",
            "github-watcher",
            "configure",
            "--label-regex",
            "(?i)^aether(-review)?$",
        ])
        .expect("CLI");
        let Some(Cmd::GithubWatcher {
            command: GithubWatcherCmd::Configure { label_regex, .. },
        }) = args.command
        else {
            panic!("configure");
        };
        assert_eq!(label_regex.as_deref(), Some("(?i)^aether(-review)?$"));
        assert!(Args::try_parse_from(["aether", "github-watcher", "enable"]).is_ok());
        assert!(Args::try_parse_from(["aether", "github-watcher", "enable", "owner/repo"]).is_ok());
    }
}

#[cfg(test)]
mod mouse_options {
    use super::*;
    use aether_tui::prefs::MouseMode;

    #[test]
    fn capture_defaults_to_the_tui_and_explicit_flags_override_in_order() {
        for (flags, expected) in [
            (vec![], None),
            (vec!["--mouse"], Some(true)),
            (vec!["--no-mouse"], Some(false)),
            (vec!["--mouse", "--no-mouse"], Some(false)),
            (vec!["--no-mouse", "--mouse"], Some(true)),
        ] {
            let args = Args::try_parse_from(["aether", "tui"].into_iter().chain(flags)).unwrap();
            let Some(Cmd::Tui {
                mouse, no_mouse, ..
            }) = args.command
            else {
                panic!("TUI command")
            };
            let choice = mouse_flag(mouse, no_mouse);
            assert_eq!(choice, expected);
            assert_eq!(
                choice.unwrap_or(MouseMode::default() != MouseMode::Off),
                expected.unwrap_or(true)
            );
        }
    }

    #[test]
    fn host_commands_is_an_optional_path_on_tui() {
        let parsed = |flags: &[&str]| {
            let args = Args::try_parse_from(["aether", "tui"].iter().chain(flags)).unwrap();
            let Some(Cmd::Tui { host_commands, .. }) = args.command else {
                panic!("TUI command")
            };
            host_commands
        };
        assert_eq!(parsed(&[]), None);
        assert_eq!(
            parsed(&["--host-commands", "/home/user/commands.json"]),
            Some(std::path::PathBuf::from("/home/user/commands.json"))
        );
        assert!(Args::try_parse_from(["aether", "tui", "--host-commands"]).is_err());
    }
}

#[cfg(test)]
mod ascii_args {
    use super::*;

    #[test]
    fn ascii_is_global_opt_in_and_independent_of_plain_theme_and_motion() {
        for args in [vec!["aether", "--ascii"], vec!["aether", "tui", "--ascii"]] {
            let args = Args::try_parse_from(args).expect("ascii parses");
            assert!(args.ascii);
            assert!(!args.plain);
            assert!(!args.reduced_motion);
        }
        let args = Args::try_parse_from(["aether", "--theme", "mono"]).expect("mono");
        assert!(!args.ascii);
        let args = Args::try_parse_from(["aether", "--ascii", "--plain"]).expect("both");
        assert!(args.ascii && args.plain);
        assert!(!Args::try_parse_from(["aether"]).expect("default").ascii);
    }

    #[test]
    fn ascii_help_describes_interactive_content_preservation() {
        use clap::CommandFactory;
        let help = Args::command().render_long_help().to_string();
        assert!(help.contains("--ascii"));
        assert!(help.contains("AETHER_ASCII"));
        assert!(help.contains("message text is unchanged"));
    }
}

#[test]
fn runtime_exit_is_bounded_with_blocked_pool_work() {
    let (release, blocked) = std::sync::mpsc::channel::<()>();
    let (started, ready) = tokio::sync::oneshot::channel();
    let began = std::time::Instant::now();
    let result = run_with_runtime(async move {
        tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            assert!(matches!(
                blocked.recv_timeout(std::time::Duration::from_secs(5)),
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ));
        });
        ready.await.unwrap();
        Ok(0)
    });
    let elapsed = began.elapsed();
    if elapsed < std::time::Duration::from_secs(4) {
        release.send(()).unwrap();
    }
    assert_eq!(result.unwrap(), 0);
    assert!(
        elapsed < std::time::Duration::from_secs(4),
        "exit took {elapsed:?}"
    );
}

#[test]
fn view_mode_is_a_global_display_flag() {
    for args in [
        vec!["aether", "--view-mode", "focus"],
        vec!["aether", "tui", "--view-mode", "brief"],
    ] {
        let parsed = Args::try_parse_from(args).unwrap();
        assert!(matches!(
            parsed.view_mode.as_deref(),
            Some("focus" | "brief")
        ));
    }
}

#[test]
fn headless_commands_print_help_and_keep_hidden_flags_valued() {
    for name in ["daemon", "serve"] {
        let help = crate::args::command()
            .try_get_matches_from(["aether", name, "--help"])
            .expect_err("--help ends parsing with the help text");
        assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp, "{name}");
        assert!(!help.to_string().contains("--view-mode"), "{name}");
        let parsed = crate::args::command()
            .try_get_matches_from(["aether", name, "--view-mode", "focus"])
            .unwrap_or_else(|e| panic!("{name} --view-mode focus: {e}"));
        assert_eq!(
            parsed.get_one::<String>("view_mode").map(String::as_str),
            Some("focus")
        );
    }
}

#[test]
fn schedule_target_needs_no_payload_and_run_takes_budgets() {
    let parsed = Args::try_parse_from([
        "aether", "schedule", "create", "tick", "--every", "90", "--target", "planner",
    ])
    .expect("--payload is optional with --target");
    assert!(matches!(parsed.command, Some(Cmd::Schedule { .. })));
    let parsed = Args::try_parse_from([
        "aether",
        "run",
        "go",
        "--token-budget",
        "50000",
        "--cost-budget",
        "1.5",
        "--max-turns",
        "9",
    ])
    .expect("run accepts the budget flags");
    assert!(matches!(
        parsed.command,
        Some(Cmd::Run {
            token_budget: Some(50_000),
            max_turns: Some(9),
            ..
        })
    ));
}

#[test]
fn retention_preview_accepts_json_and_has_no_destructive_mode() {
    use clap::Parser as _;
    assert!(matches!(
        Args::try_parse_from(["aether", "retention", "preview", "--json"])
            .unwrap()
            .command,
        Some(Cmd::Retention {
            command: RetentionCmd::Preview { json: true }
        })
    ));
    assert!(Args::try_parse_from(["aether", "retention", "delete"]).is_err());
}

#[test]
fn outbox_lists_by_default_and_every_decision_names_one_key() {
    use clap::Parser as _;
    let parse = |args: &[&str]| Args::try_parse_from(args).map(|parsed| parsed.command);
    assert!(matches!(
        parse(&["aether", "--remote", "box", "outbox"]),
        Ok(Some(Cmd::Outbox { command: None }))
    ));
    assert!(matches!(
        parse(&["aether", "outbox", "list", "--json"]),
        Ok(Some(Cmd::Outbox {
            command: Some(OutboxCmd::List { json: true })
        }))
    ));
    for decision in ["delivered", "resend", "discard"] {
        assert!(parse(&["aether", "outbox", decision, "ssh-1"]).is_ok());
        assert!(parse(&["aether", "outbox", decision, "ssh-1", "--json"]).is_ok());
        assert!(
            parse(&["aether", "outbox", decision]).is_err(),
            "{decision} must name the operation it decides"
        );
    }
    let outbox = Cmd::Outbox { command: None };
    assert!(check_remote_command(Some(&outbox)).is_ok());
}

#[test]
fn storage_command_supports_status_and_dry_run() {
    let args = Args::try_parse_from(["aether", "storage", "--json"]).unwrap();
    assert!(matches!(
        args.command,
        Some(Cmd::Storage {
            command: None,
            json: true
        })
    ));
    let args = Args::try_parse_from(["aether", "storage", "gc", "--dry-run", "--json"]).unwrap();
    assert!(matches!(
        args.command,
        Some(Cmd::Storage {
            command: Some(StorageCmd::Gc {
                dry_run: true,
                prune_build_output: false,
                json: true
            }),
            json: false
        })
    ));
    let args = Args::try_parse_from([
        "aether",
        "storage",
        "gc",
        "--prune-build-output",
        "--dry-run",
    ])
    .unwrap();
    assert!(matches!(
        args.command,
        Some(Cmd::Storage {
            command: Some(StorageCmd::Gc {
                dry_run: true,
                prune_build_output: true,
                json: false
            }),
            json: false
        })
    ));
}

mod id_references {
    use super::*;
    use std::any::TypeId;

    /// Argument ids that name an agent, run, task, approval or schedule, or
    /// a memory scope that may hold an agent. Matched by name, so a new
    /// argument called one of these is checked too.
    const ID_ARGUMENTS: &[&str] = &[
        "scope",
        "agent",
        "subtree",
        "run",
        "task",
        "owner",
        "depends_on",
        "approval",
        "target",
    ];
    /// Strict on purpose: engines spawn it with the full id of their agent.
    const STRICT: &[&str] = &["aether mcp-bridge agent"];
    /// Named like an id argument but holding something else.
    const NOT_IDS: &[&str] = &["aether set approval"];

    fn arguments(command: &clap::Command, prefix: &str, out: &mut Vec<(String, clap::Arg)>) {
        let path = if prefix.is_empty() {
            command.get_name().to_owned()
        } else {
            format!("{prefix} {}", command.get_name())
        };
        for arg in command.get_arguments() {
            out.push((format!("{path} {}", arg.get_id()), arg.clone()));
        }
        for sub in command.get_subcommands() {
            arguments(sub, &path, out);
        }
    }

    fn takes_an_id(path: &str) -> bool {
        let (command, arg) = path.rsplit_once(' ').expect("path names an argument");
        ID_ARGUMENTS.contains(&arg) || (arg == "id" && command.starts_with("aether schedule "))
    }

    fn resolves(arg: &clap::Arg) -> bool {
        let parser = arg.get_value_parser().type_id();
        [
            TypeId::of::<cli::AgentRef>(),
            TypeId::of::<cli::RunRef>(),
            TypeId::of::<cli::TaskRef>(),
            TypeId::of::<cli::ApprovalRef>(),
            TypeId::of::<cli::ScheduleRef>(),
            TypeId::of::<cli::TargetRef>(),
            TypeId::of::<cli::MemoryScopeRef>(),
            TypeId::of::<cli::OperatorScopeRef>(),
        ]
        .iter()
        .any(|kind| parser == *kind)
    }

    #[test]
    fn every_id_argument_goes_through_the_resolver() {
        let mut all = Vec::new();
        arguments(&crate::args::command(), "", &mut all);
        let id_arguments: Vec<_> = all
            .iter()
            .filter(|(path, arg)| arg.get_action().takes_values() && takes_an_id(path))
            .filter(|(path, _)| !NOT_IDS.contains(&path.as_str()))
            .collect();
        // Enough to know the walk reached the nested subcommands.
        assert!(id_arguments.len() > 30, "found only {}", id_arguments.len());
        let strict: Vec<_> = id_arguments
            .iter()
            .filter(|(path, arg)| !STRICT.contains(&path.as_str()) && !resolves(arg))
            .map(|(path, _)| path.as_str())
            .collect();
        assert!(strict.is_empty(), "parse these as references: {strict:?}");
        for path in NOT_IDS {
            assert!(
                all.iter().any(|(p, _)| p == path),
                "{path} no longer exists"
            );
        }
        for path in STRICT {
            let (_, arg) = id_arguments
                .iter()
                .find(|(p, _)| p == path)
                .unwrap_or_else(|| panic!("{path} is exempt but no longer exists"));
            assert!(!resolves(arg), "{path} must stay a full id");
        }
    }

    #[test]
    fn references_reach_the_commands_as_typed() {
        let parsed = Args::try_parse_from(["aether", "send", "@last", "hello"]).expect("CLI");
        let Some(Cmd::Send { agent, .. }) = parsed.command else {
            panic!("send");
        };
        assert_eq!(agent.to_string(), "@last");
        let refused = Args::try_parse_from(["aether", "cancel", " "]);
        assert!(refused.is_err(), "an empty reference names nothing");
        let parsed = Args::try_parse_from(["aether", "memory", "list", "--scope", "agent:@viewed"]);
        let Some(Cmd::Memory {
            command: MemoryCmd::List { scope, .. },
        }) = parsed.expect("CLI").command
        else {
            panic!("memory list");
        };
        assert!(matches!(
            scope,
            Some(cli::MemoryScopeRef::Agent(agent)) if agent.to_string() == "@viewed"
        ));
    }
}

mod help_text {
    use super::*;

    /// Options with no help line of their own, as the path a person would type.
    /// Nothing may be added: a new option says what it does.
    const WITHOUT_HELP: &[&str] = &[];

    fn missing_help(command: &clap::Command, prefix: &str, out: &mut Vec<String>) {
        let path = format!("{prefix} {}", command.get_name());
        for arg in command.get_arguments() {
            let globals_are_listed_once = arg.is_global_set() && !prefix.is_empty();
            let id = arg.get_id().as_str();
            let is_option = arg.get_long().is_some();
            if !is_option || arg.is_hide_set() || globals_are_listed_once {
                continue;
            }
            if arg.get_help().is_none() && arg.get_long_help().is_none() {
                out.push(format!("{} {id}", path.trim()));
            }
        }
        for sub in command.get_subcommands().filter(|s| !s.is_hide_set()) {
            missing_help(sub, &path, out);
        }
    }

    #[test]
    fn headless_commands_do_not_list_the_ui_flags() {
        let mut command = crate::args::command();
        for name in ["daemon", "serve"] {
            let help = command
                .find_subcommand_mut(name)
                .expect("subcommand")
                .render_help()
                .to_string();
            for flag in [
                "--theme",
                "--reduced-motion",
                "--plain",
                "--ascii",
                "--view-mode",
            ] {
                assert!(!help.contains(flag), "{name} --help lists {flag}:\n{help}");
            }
        }
        let tui = command
            .find_subcommand_mut("tui")
            .expect("tui")
            .render_help();
        assert!(tui.to_string().contains("--theme"), "tui keeps the flags");
        let parsed = Args::try_parse_from(["aether", "daemon", "--theme", "mono"]);
        assert!(parsed.is_ok(), "hidden flags are still accepted");
    }

    #[test]
    fn every_visible_option_has_a_help_line() {
        let mut missing = Vec::new();
        let command = crate::args::command();
        missing_help(&command, "", &mut missing);
        missing.retain(|path| !WITHOUT_HELP.contains(&path.as_str()));
        assert!(missing.is_empty(), "these have no help text: {missing:#?}");
    }
}
