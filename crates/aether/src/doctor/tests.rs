use super::*;
use aether_core::mcp::{McpServerHealth, McpServerState};
use aether_protocol::remote::RemoteStatus;

fn paths(dir: &Path) -> Paths {
    Paths {
        config_dir: dir.join("config"),
        data_dir: dir.join("data"),
        socket: dir.join("aetherd.sock"),
    }
}

fn auth(provider: &str, configured: bool) -> AuthInfo {
    AuthInfo {
        provider: provider.into(),
        provider_name: provider.into(),
        configured,
        source: None,
        methods: Vec::new(),
        subscription: false,
        detail: None,
        program: None,
    }
}

#[test]
fn a_missing_config_directory_is_not_a_problem() {
    let dir = tempfile::tempdir().expect("dir");
    let check = check_config(&paths(dir.path()));
    assert_eq!(check.level, Level::Ok);
    assert!(check.fix.is_none());
}

#[test]
fn a_config_that_does_not_parse_fails_with_the_file_named() {
    let dir = tempfile::tempdir().expect("dir");
    let p = paths(dir.path());
    std::fs::create_dir_all(&p.config_dir).expect("mkdir");
    std::fs::write(p.config_dir.join("config.toml"), "[model\nbroken").expect("write");
    let check = check_config(&p);
    assert_eq!(check.level, Level::Fail);
    assert!(check.detail.contains("config.toml"), "{}", check.detail);
    assert!(check.fix.is_some());
}

#[test]
fn a_parse_failure_names_the_place_without_quoting_the_line() {
    let dir = tempfile::tempdir().expect("dir");
    let p = paths(dir.path());
    std::fs::create_dir_all(&p.config_dir).expect("mkdir");
    // Unterminated, so the offending line is the one carrying the value.
    std::fs::write(
        p.config_dir.join("config.toml"),
        "token = \"AKIAIOSFODNN7EXAMPLE\nnext = 1\n",
    )
    .expect("write");
    let check = check_config(&p);
    assert_eq!(check.level, Level::Fail);
    assert!(
        !check.detail.contains("AKIAIOSFODNN7EXAMPLE"),
        "the source line was echoed: {}",
        check.detail
    );
    assert!(check.detail.contains("config.toml"), "{}", check.detail);
    assert!(check.detail.contains("line 1"), "{}", check.detail);
    assert!(
        !check.detail.contains('\n'),
        "one line per check: {:?}",
        check.detail
    );
}

#[test]
fn a_providers_file_that_does_not_parse_fails_the_config_check() {
    let dir = tempfile::tempdir().expect("dir");
    let p = paths(dir.path());
    std::fs::create_dir_all(&p.config_dir).expect("mkdir");
    // config.toml is fine; only providers.toml is broken, which the
    // check used never to open despite saying it read both.
    std::fs::write(p.config_dir.join("config.toml"), "").expect("write");
    std::fs::write(
        p.config_dir.join("providers.toml"),
        "[providers.anthropic\nbase_url = 1\n",
    )
    .expect("write");
    let check = check_config(&p);
    assert_eq!(check.level, Level::Fail);
    assert!(check.detail.contains("providers.toml"), "{}", check.detail);
}

#[test]
fn a_retired_model_setting_fails_with_its_successor_unless_providers_define_it() {
    let dir = tempfile::tempdir().expect("dir");
    let p = paths(dir.path());
    std::fs::create_dir_all(&p.config_dir).expect("mkdir");
    std::fs::write(
        p.config_dir.join("config.toml"),
        "[model]\ndefault = \"codex/gpt-5.6-terra\"\ncompactor = \"anthropic/haiku-4-5\"\n",
    )
    .expect("write");
    let check = check_config(&p);
    assert_eq!(check.level, Level::Fail);
    assert_eq!(
        check.detail,
        "[model] default: codex/gpt-5.6-terra was retired; use codex/gpt-6-luna; \
         [model] compactor: anthropic/haiku-4-5 was retired; use anthropic/claude-haiku-5-5"
    );
    std::fs::write(
        p.config_dir.join("providers.toml"),
        "[[providers.codex.add_models]]\nid = \"gpt-5.6-terra\"\n\
         [[providers.anthropic.add_models]]\nid = \"claude-haiku-4-5\"\naliases = [\"haiku-4-5\"]\n",
    )
    .expect("write");
    assert_eq!(check_config(&p).level, Level::Ok);
}

#[test]
fn the_store_check_reads_the_filename_the_rest_of_the_system_writes() {
    let dir = tempfile::tempdir().expect("dir");
    let p = paths(dir.path());
    assert_eq!(
        p.store_path().file_name().and_then(|n| n.to_str()),
        Some(aether_protocol::paths::STORE_FILE_NAME),
    );
}

#[test]
fn two_mcp_servers_that_name_the_same_tools_fail() {
    let dir = tempfile::tempdir().expect("dir");
    let p = paths(dir.path());
    std::fs::create_dir_all(&p.config_dir).expect("mkdir");
    std::fs::write(
        p.config_dir.join("config.toml"),
        "[[mcp.servers]]\nname = \"my-files\"\ncommand = \"a\"\n\
         [[mcp.servers]]\nname = \"my_files\"\ncommand = \"b\"\n",
    )
    .expect("write");
    let check = check_config(&p);
    assert_eq!(check.level, Level::Fail);
    assert!(
        check.detail.contains("name the same tools"),
        "{}",
        check.detail
    );
}

#[test]
fn a_stale_socket_reads_differently_from_no_socket() {
    let dir = tempfile::tempdir().expect("dir");
    let p = paths(dir.path());
    let absent = check_socket(
        &p,
        SocketState::Silent,
        &aether_protocol::command_hint::command_hint(&p.socket, false),
    );
    assert_eq!(absent.level, Level::Warn);
    assert!(absent.detail.contains("no daemon"));
    std::fs::write(&p.socket, b"").expect("write");
    let stale = check_socket(
        &p,
        SocketState::Silent,
        &aether_protocol::command_hint::command_hint(&p.socket, false),
    );
    assert_eq!(stale.level, Level::Warn);
    assert!(stale.detail.contains("nothing answers"), "{}", stale.detail);
}

/// A daemon left running from the previous build refuses the
/// handshake. Reporting that as "nothing answers" sends the operator to
/// start a daemon that cannot start, so it is its own failing check.
#[test]
fn a_daemon_that_refuses_this_build_is_not_reported_as_silence() {
    let dir = tempfile::tempdir().expect("dir");
    let p = paths(dir.path());
    std::fs::write(&p.socket, b"").expect("write");
    let check = check_socket(
        &p,
        SocketState::Refused("client protocol 11 is not served"),
        &aether_protocol::command_hint::command_hint(&p.socket, false),
    );
    assert_eq!(check.level, Level::Fail, "{}", check.detail);
    assert!(
        check.detail.contains("client protocol 11 is not served"),
        "the daemon's own words reach the report: {}",
        check.detail
    );
    assert!(
        !check.detail.contains("nothing answers"),
        "a daemon that answered must not read as silence: {}",
        check.detail
    );
    assert!(
        check.fix.as_deref().unwrap_or_default().contains(&format!(
            "{} stop",
            aether_protocol::command_hint::command_hint(&p.socket, false)
        )),
        "the fix is to stop the daemon that is running: {:?}",
        check.fix
    );
}

#[test]
fn a_missing_store_is_a_first_run_not_a_fault() {
    let dir = tempfile::tempdir().expect("dir");
    let check = check_store(&paths(dir.path()));
    assert_eq!(check.level, Level::Ok);
}

#[test]
fn a_file_that_is_not_a_store_fails() {
    let dir = tempfile::tempdir().expect("dir");
    let p = paths(dir.path());
    std::fs::create_dir_all(&p.data_dir).expect("mkdir");
    std::fs::write(p.data_dir.join("aether.db"), b"not a database at all").expect("write");
    let check = check_store(&p);
    assert_eq!(check.level, Level::Fail);
    assert!(check.fix.is_some());
}

#[test]
fn a_real_store_passes_integrity_and_names_its_schema() {
    let dir = tempfile::tempdir().expect("dir");
    let p = paths(dir.path());
    std::fs::create_dir_all(&p.data_dir).expect("mkdir");
    aether_store::SqliteStore::open(p.data_dir.join("aether.db")).expect("store");
    let check = check_store(&p);
    assert_eq!(check.level, Level::Ok, "{}", check.detail);
    assert!(
        check
            .detail
            .contains(&format!("schema {}", aether_store::SCHEMA_VERSION)),
        "{}",
        check.detail
    );
    assert!(check.detail.contains("integrity ok"));
}

#[test]
fn a_store_from_a_newer_build_fails_rather_than_being_migrated_down() {
    let dir = tempfile::tempdir().expect("dir");
    let p = paths(dir.path());
    std::fs::create_dir_all(&p.data_dir).expect("mkdir");
    let db = p.data_dir.join("aether.db");
    aether_store::SqliteStore::open(&db).expect("store");
    let conn = rusqlite::Connection::open(&db).expect("open");
    conn.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
        [(aether_store::SCHEMA_VERSION + 9).to_string()],
    )
    .expect("bump");
    drop(conn);
    let check = check_store(&p);
    assert_eq!(check.level, Level::Fail);
    assert!(
        check.detail.contains("this build reads"),
        "{}",
        check.detail
    );
}

#[test]
fn disk_headroom_is_measured_where_the_data_will_go() {
    let dir = tempfile::tempdir().expect("dir");
    // The data directory does not exist yet: the check walks up to the
    // filesystem it would land on rather than giving up.
    let check = check_disk(&paths(dir.path()), Some(DiskFloor::Default), None);
    assert_ne!(check.level, Level::Fail, "{}", check.detail);
    assert!(check.detail.contains("GiB free"), "{}", check.detail);
}

#[test]
fn turns_held_on_a_low_disk_fail_and_a_disk_under_the_floor_warns() {
    const GIB: u64 = 1 << 30;
    let probe = Path::new("/data");
    let held = LowDisk {
        path: "/data/aether.db".into(),
        available_bytes: GIB,
        floor_bytes: 5 * GIB,
        since: aether_core::time::now(),
    };
    // The daemon's word wins: its turns wait even if space freed since it looked.
    let check = judge_disk(probe, 6 * GIB, Some(5 * GIB), Some(&held));
    assert_eq!(check.level, Level::Fail);
    assert!(check.detail.contains("new turns wait"), "{}", check.detail);
    assert!(
        check
            .fix
            .as_deref()
            .is_some_and(|fix| fix.contains("5.5 GiB"))
    );

    let under = judge_disk(probe, 4 * GIB, Some(5 * GIB), None);
    assert_eq!(under.level, Level::Warn);
    assert!(
        under.detail.contains("below the 5.0 GiB floor"),
        "{}",
        under.detail
    );

    let ok = judge_disk(probe, 6 * GIB, Some(5 * GIB), None);
    assert_eq!(ok.level, Level::Ok);
    assert!(
        ok.detail.contains("new turns wait below 5.0 GiB"),
        "{}",
        ok.detail
    );

    // No floor: the old fixed thresholds still apply.
    assert_eq!(judge_disk(probe, GIB / 2, None, None).level, Level::Warn);
    assert_eq!(judge_disk(probe, GIB / 20, None, None).level, Level::Fail);
    assert_eq!(judge_disk(probe, 6 * GIB, None, None).level, Level::Ok);
}

#[test]
fn the_log_check_names_the_file_the_daemon_writes_now() {
    let dir = tempfile::tempdir().expect("dir");
    let log = dir.path().join("aetherd.log/segments-v1/events.0.log");
    let unwritten = check_logs(&log);
    assert_eq!(unwritten.level, Level::Ok);
    assert!(
        unwritten.detail.contains("will write"),
        "{}",
        unwritten.detail
    );
    std::fs::create_dir_all(log.parent().expect("parent")).expect("mkdir");
    std::fs::write(&log, "line\n").expect("write");
    let written = check_logs(&log);
    assert_eq!(written.level, Level::Ok);
    assert!(
        written.detail.contains(&log.display().to_string()) && written.detail.contains("last at"),
        "{}",
        written.detail
    );
}

#[test]
fn no_configured_provider_is_a_failure_and_one_is_enough() {
    assert_eq!(
        check_providers(&[auth("anthropic", false), auth("openai", false)], "aether").level,
        Level::Fail
    );
    let ok = check_providers(
        &[auth("anthropic", false), auth("claude-code", true)],
        "aether",
    );
    assert_eq!(ok.level, Level::Ok);
    assert!(ok.detail.contains("claude-code"));
}

#[test]
fn an_engine_that_is_present_but_not_signed_in_is_called_out() {
    assert_eq!(check_engines(&[], "aether").level, Level::Warn);
    assert_eq!(
        check_engines(
            &[(
                "claude-code".into(),
                EngineStatus::Unavailable("not installed".into())
            )],
            "aether"
        )
        .level,
        Level::Warn
    );
    let both = check_engines(
        &[
            (
                "claude-code".into(),
                EngineStatus::Ready("claude-code/2.1.0".into()),
            ),
            (
                "codex".into(),
                EngineStatus::Unavailable("codex is not installed or not on PATH".into()),
            ),
        ],
        "aether",
    );
    assert_eq!(both.level, Level::Warn);
    assert!(
        both.detail.contains("not usable: codex: codex is not"),
        "{}",
        both.detail
    );
    let all = check_engines(
        &[(
            "claude-code".into(),
            EngineStatus::Ready("claude-code/2.1.0".into()),
        )],
        "aether",
    );
    assert_eq!(all.level, Level::Ok);
    assert!(all.fix.is_none());
}

/// A version too old to speak a flag a future adapter's `args()`
/// genuinely depends on must name the detected version, the requirement
/// and the remedy, not just say the engine is missing. (Claude Code's own
/// floor does not gate registration any more; see the limitation test
/// below for that path.)
#[test]
fn a_too_old_engine_names_the_upgrade_remedy() {
    let report = check_engines(&[(
        "claude-code".into(),
        EngineStatus::Unavailable(
            "claude 2.0.0 is older than the 2.1.219 Aether requires; upgrade claude to >= 2.1.219"
                .into(),
        ),
    )], "aether");
    assert_eq!(report.level, Level::Warn);
    assert!(report.detail.contains("upgrade claude to >= 2.1.219"));
}

/// A `claude` below the offline-sandbox version floor
/// still registers and runs; doctor must show that as "usable, with
/// limits", never folded into "not usable" alongside a genuinely absent
/// engine.
#[test]
fn a_version_limitation_reads_distinctly_from_not_usable() {
    let check = check_engines(
        &[(
            "claude-code".into(),
            EngineStatus::Limited {
                adapter: "claude-code/2.0.5".into(),
                limitation: "network-denied turns require claude >= 2.1.219 (found 2.0.5)".into(),
            },
        )],
        "aether",
    );
    assert_eq!(check.level, Level::Warn);
    assert!(
        check.detail.contains("usable, with limits"),
        "{}",
        check.detail
    );
    assert!(
        !check.detail.contains("not usable"),
        "a limitation must not read as a refusal: {}",
        check.detail
    );

    let alongside_missing = check_engines(
        &[
            (
                "claude-code".into(),
                EngineStatus::Limited {
                    adapter: "claude-code/2.0.5".into(),
                    limitation: "network-denied turns require claude >= 2.1.219 (found 2.0.5)"
                        .into(),
                },
            ),
            (
                "codex".into(),
                EngineStatus::Unavailable("codex is not installed or not on PATH".into()),
            ),
        ],
        "aether",
    );
    assert_eq!(alongside_missing.level, Level::Warn);
    assert!(
        alongside_missing.detail.contains("usable, with limits"),
        "{}",
        alongside_missing.detail
    );
    assert!(
        alongside_missing.detail.contains("not usable: codex"),
        "a genuinely missing engine is still called out: {}",
        alongside_missing.detail
    );
}

#[test]
fn a_missing_sandbox_dependency_is_a_limitation_with_a_dependency_fix() {
    let check = check_engines(
        &[(
            "claude-code".into(),
            EngineStatus::Limited {
                adapter: "claude-code/2.1.265".into(),
                limitation: "offline sandbox dependencies on PATH; missing: socat".into(),
            },
        )],
        "aether",
    );
    assert_eq!(check.level, Level::Warn);
    assert!(check.detail.contains("usable, with limits"));
    assert!(check.detail.contains("missing: socat"));
    assert!(!check.detail.contains("not usable"));
    let fix = check.fix.unwrap();
    assert!(fix.contains("missing dependencies"));
    assert!(!fix.contains("upgrade the CLI"));
}

#[test]
fn an_mcp_server_that_is_not_answering_is_a_warning_with_the_retry() {
    assert_eq!(check_mcp(0, &[], "aether").level, Level::Ok);
    assert_eq!(check_mcp(2, &[], "aether").level, Level::Warn);
    let health = vec![
        McpServerHealth::unknown("files", "stdio: a"),
        McpServerHealth {
            state: McpServerState::Failed,
            error: Some("connection refused".into()),
            ..McpServerHealth::unknown("issues", "stdio: b")
        },
    ];
    let check = check_mcp(2, &health, "aether");
    assert_eq!(check.level, Level::Warn);
    assert!(check.detail.contains("issues"), "{}", check.detail);
    assert!(check.fix.expect("a fix").contains("mcp refresh"));
}

#[test]
fn the_reports_verdict_is_its_worst_check() {
    let report = Report::new(vec![
        Check::ok("a", "fine"),
        Check::warn("b", "hm", "do this"),
        Check::ok("c", "fine"),
    ]);
    assert_eq!(report.level, Level::Warn);
    assert_eq!(report.level.exit_code(), 1);
    let bad = Report::new(vec![Check::fail("d", "broken", "fix it")]);
    assert_eq!(bad.level.exit_code(), 2);
    assert_eq!(Report::new(Vec::new()).level.exit_code(), 0);
}

#[test]
fn missing_log_and_store_destinations_must_be_creatable_without_writes() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let p = paths(root.path());
    std::fs::create_dir(&p.data_dir).unwrap();
    let log = p.data_dir.join("logs/aetherd.log/segments-v1/events.0.log");
    assert_eq!(check_store(&p).level, Level::Ok);
    assert_eq!(check_logs(&log).level, Level::Ok);
    assert!(!p.store_path().exists());
    assert!(!p.data_dir.join("logs").exists());
    if !rustix::process::geteuid().is_root() {
        for mode in [0o500, 0o600, 0o000] {
            std::fs::set_permissions(&p.data_dir, std::fs::Permissions::from_mode(mode)).unwrap();
            for (check, path) in [
                (check_store(&p), p.store_path()),
                (check_logs(&log), log.clone()),
            ] {
                assert_ne!(check.level, Level::Ok, "{check:?}");
                assert!(
                    check.detail.contains(&path.display().to_string()),
                    "{check:?}"
                );
            }
        }
        std::fs::set_permissions(&p.data_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    assert_eq!(std::fs::read_dir(&p.data_dir).unwrap().count(), 0);
}

#[test]
fn missing_destinations_reject_file_ancestors_and_dangling_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let p = paths(root.path());
    std::fs::write(&p.data_dir, "file").unwrap();
    assert_eq!(check_store(&p).level, Level::Fail);
    assert_ne!(
        check_logs(&p.data_dir.join("logs/events.log")).level,
        Level::Ok
    );
    let link = root.path().join("dangling");
    std::os::unix::fs::symlink(root.path().join("absent"), &link).unwrap();
    assert_eq!(check_logs(&link.join("events.log")).level, Level::Fail);
    assert!(!root.path().join("absent").exists());
}

#[test]
fn every_daemon_command_in_doctor_fixes_preserves_the_selected_socket() {
    let dir = tempfile::tempdir().expect("dir");
    let p = paths(dir.path());
    let command = aether_protocol::command_hint::command_hint(&p.socket, true);
    let unavailable = [(
        "codex".into(),
        EngineStatus::Unavailable("not installed".into()),
    )];
    let failed = [McpServerHealth {
        state: McpServerState::Failed,
        ..McpServerHealth::unknown("files", "stdio: a")
    }];
    let report = Report::new(vec![
        check_socket(&p, SocketState::Silent, &command),
        check_socket(&p, SocketState::Refused("version mismatch"), &command),
        Check::no_daemon_providers(&command),
        check_providers(&[], &command),
        check_providers(&[auth("anthropic", false)], &command),
        check_engines(&unavailable, &command),
        check_mcp(1, &[], &command),
        check_mcp(1, &failed, &command),
    ]);
    for check in report.checks {
        let fix = check.fix.expect("each scenario has a fix");
        assert!(
            !fix.replace(&command, "<selected daemon>")
                .contains("`aether "),
            "{}: {fix}",
            check.name
        );
        assert!(
            fix.contains(&format!("`{command}")),
            "{}: {fix}",
            check.name
        );
    }
}

#[test]
fn unknown_mcp_servers_are_not_reported_as_connected() {
    let unknown = McpServerHealth::unknown("pending", "stdio");
    let ready = McpServerHealth {
        state: McpServerState::Ready,
        ..McpServerHealth::unknown("ready", "stdio")
    };
    let check = check_mcp(2, &[ready.clone(), unknown], "aether");
    assert_eq!(check.level, Level::Warn);
    assert!(check.detail.contains("1 of 2 server(s) connected"));
    assert!(check.detail.contains("not contacted yet: pending"));
    assert_eq!(
        check_mcp(1, std::slice::from_ref(&ready), "aether").level,
        Level::Ok
    );
}

#[test]
fn log_check_rejects_non_files_and_links_without_changing_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    std::fs::write(&target, "keep").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
    let directory = root.path().join("directory");
    std::fs::create_dir(&directory).unwrap();
    let link = root.path().join("link");
    symlink(&target, &link).unwrap();
    let dangling = root.path().join("dangling");
    symlink(root.path().join("absent"), &dangling).unwrap();
    for path in [&directory, &link, &dangling] {
        assert_ne!(check_logs(path).level, Level::Ok, "{}", path.display());
    }
    assert_eq!(check_logs(&target).level, Level::Ok);
    let hardlink = root.path().join("hardlink");
    std::fs::hard_link(&target, &hardlink).unwrap();
    assert_ne!(check_logs(&hardlink).level, Level::Ok);
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
    assert!(!root.path().join("absent").exists());
}

#[test]
fn healthy_daemon_mcp_servers_survive_local_config_changes() {
    let ready = McpServerHealth {
        state: McpServerState::Ready,
        ..McpServerHealth::unknown("ready", "stdio")
    };
    for (configured, health) in [
        (1, vec![ready.clone(), ready.clone()]),
        (2, vec![ready.clone()]),
        (0, vec![ready]),
    ] {
        let check = check_mcp(configured, &health, "aether");
        assert_eq!(check.level, Level::Ok, "{check:?}");
        assert!(
            check
                .detail
                .contains(&format!("{} server(s) connected", health.len()))
        );
        assert!(
            check.detail.contains(&format!(
                "config/daemon differ: {configured} locally configured, {} reported by daemon",
                health.len()
            )),
            "{}",
            check.detail
        );
    }
}

#[test]
fn log_check_requires_append_access_without_changing_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("log");
    std::fs::write(&log, "keep\n").unwrap();
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o444)).unwrap();
    let can_append = std::fs::OpenOptions::new().append(true).open(&log).is_ok();
    assert_eq!(check_logs(&log).level == Level::Ok, can_append);
    assert_eq!(
        std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
        0o444
    );
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "keep\n");
}

fn remote(enabled: bool, listening: Option<&str>, origins: &[&str], devices: u32) -> RemoteStatus {
    RemoteStatus {
        settings: aether_protocol::remote::RemoteSettings {
            enabled,
            public_origins: origins.iter().map(|o| (*o).to_owned()).collect(),
            ..Default::default()
        },
        listening: listening.map(str::to_owned),
        devices,
        ..Default::default()
    }
}

#[test]
fn remote_access_that_was_never_turned_on_is_fine_and_says_how() {
    let check = check_remote(Some(&remote(false, None, &[], 0)), "aether");
    assert_eq!(check.level, Level::Ok);
    assert!(
        check.detail.contains("aether remote setup"),
        "{}",
        check.detail
    );
    assert_eq!(check_remote(None, "aether").level, Level::Ok);
}

#[test]
fn a_listener_that_failed_to_open_is_a_failure_with_the_reason() {
    let mut status = remote(true, None, &[], 0);
    status.error = Some("address already in use".into());
    let check = check_remote(Some(&status), "aether");
    assert_eq!(check.level, Level::Fail);
    assert!(check.detail.contains("address already in use"));
    assert!(check.fix.is_some());
}

#[test]
fn a_listener_nobody_can_reach_by_name_is_a_warning() {
    let check = check_remote(
        Some(&remote(true, Some("127.0.0.1:8377"), &[], 0)),
        "aether",
    );
    assert_eq!(check.level, Level::Warn);
    assert!(
        check.detail.contains("phones are refused"),
        "{}",
        check.detail
    );
    let closed = check_remote(
        Some(&remote(true, None, &["https://a.ts.net"], 1)),
        "aether",
    );
    assert_eq!(closed.level, Level::Warn);
}

#[test]
fn a_published_listener_with_a_paired_device_is_healthy() {
    let status = remote(true, Some("127.0.0.1:8377"), &["https://a.ts.net"], 1);
    let check = check_remote(Some(&status), "aether");
    assert_eq!(check.level, Level::Ok);
    assert!(check.detail.contains("https://a.ts.net") && check.detail.contains("1 device(s)"));
    let unpaired = remote(true, Some("127.0.0.1:8377"), &["https://a.ts.net"], 0);
    let check = check_remote(Some(&unpaired), "aether");
    assert_eq!(check.level, Level::Warn);
    assert_eq!(
        check.fix.as_deref(),
        Some("`aether remote pair` adds a phone")
    );
}

#[cfg(unix)]
#[test]
fn unsafe_credential_directory_warns_with_exact_repair() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = paths(dir.path());
    aether_model::config::create_config_dir(&paths.config_dir).unwrap();
    assert_eq!(check_credentials_dir(&paths).level, Level::Ok);
    std::fs::set_permissions(&paths.config_dir, std::fs::Permissions::from_mode(0o775)).unwrap();
    let check = check_credentials_dir(&paths);
    assert_eq!(check.level, Level::Warn);
    assert!(check.detail.contains("group- or world-writable"));
    assert!(
        check
            .fix
            .unwrap()
            .contains(&aether_model::auth::store::credentials_dir_fix(
                &paths.config_dir
            ))
    );
    assert_eq!(
        std::fs::metadata(&paths.config_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o775
    );
}
