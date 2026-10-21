use super::super::driver::{McpInventory, mcp_inventory};
use super::*;

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[tokio::test]
async fn exec_server_hides_provider_auth_while_app_server_keeps_it() {
    const PUBLIC: &str = "AETHER_TEST_ENGINE_PUBLIC";
    const KEY: &str = "AETHER_TEST_ENGINE_PROVIDER";
    const SECRET: &str = "AETHER_TEST_ENGINE_DAEMON";
    const CAPTURE: &str = "AETHER_TEST_ENGINE_CAPTURE";
    // Re-exec gives both engine children inherited credentials without changing test globals.
    if std::env::var(PUBLIC).as_deref() != Ok("visible") {
        let dir = tempfile::tempdir().unwrap();
        for program in ["bwrap", "codex", "socat"] {
            crate::test_script::write(
                &dir.path().join(program),
                &format!(
                    "#!/bin/sh\nprintf '%s|%s|%s\\n' \"${{AETHER_TEST_ENGINE_PROVIDER-unset}}\" \"${{AETHER_TEST_ENGINE_DAEMON-unset}}\" \"$AETHER_TEST_ENGINE_PUBLIC\" > \"$AETHER_TEST_ENGINE_CAPTURE/{program}.env\"\nexit 1\n"
                ),
            );
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "codex_app::tests::launch::exec_server_hides_provider_auth_while_app_server_keeps_it",
                "--nocapture",
            ])
            .env("PATH", dir.path())
            .env(CAPTURE, dir.path())
            .env(KEY, "provider-secret")
            .env(SECRET, "daemon-secret")
            .env(PUBLIC, "visible")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let dir = PathBuf::from(std::env::var_os(CAPTURE).unwrap());
    let engine = CodexAppEngine::new(dir.join("codex").to_str().unwrap(), "0.159.3");
    let mut t = turn(None, false);
    t.cwd = dir.clone();
    t.hidden_env = vec![SECRET.into()];
    t.tool_hidden_env = vec![SECRET.into(), KEY.into()];
    t.network_access = false;
    t.mcp = None;
    let result = engine.run(t.clone(), CancellationToken::new()).await;
    assert!(matches!(result, Err(crate::EngineError::Unsupported(_))));
    assert_eq!(
        std::fs::read_to_string(dir.join("bwrap.env")).unwrap(),
        "unset|unset|visible\n"
    );

    t.network_access = true;
    let mut run = engine.run(t, CancellationToken::new()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while run.events.recv().await.is_some() {}
    })
    .await
    .expect("fake app-server exited");
    assert_eq!(
        std::fs::read_to_string(dir.join("codex.env")).unwrap(),
        "provider-secret|unset|visible\n"
    );
}

#[tokio::test]
async fn fresh_and_resumed_app_servers_get_the_agent_temp_dir_and_build_cache() {
    let dir = tempfile::tempdir().unwrap();
    let leaf = dir.path().join("0123456789AB");
    std::fs::create_dir(&leaf).unwrap();
    let cache = dir.path().join("cargo-target");
    crate::test_script::write(
        &dir.path().join("codex"),
        "#!/bin/sh\nprintf '%s|%s|%s|%s' \"$TMPDIR\" \"$TMP\" \"$TEMP\" \"$CARGO_TARGET_DIR\" > \"$TMPDIR/seen\"\nexit 1\n",
    );
    let engine = CodexAppEngine::new(dir.path().join("codex").to_str().unwrap(), "0.162.1");
    for session in [None, Some("th")] {
        let mut t = turn(session, false);
        t.cwd = dir.path().into();
        t.temp_dir = Some(leaf.clone());
        t.build_env = vec![("CARGO_TARGET_DIR".into(), cache.clone())];
        t.mcp = None;
        let mut run = engine.run(t, CancellationToken::new()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while run.events.recv().await.is_some() {}
        })
        .await
        .expect("fake app-server exited");
        let seen = std::fs::read_to_string(leaf.join("seen")).unwrap();
        let expected = format!("{0}|{0}|{0}|{1}", leaf.display(), cache.display());
        assert_eq!(seen, expected, "{session:?}");
        std::fs::remove_file(leaf.join("seen")).unwrap();
    }
}

#[test]
fn codex_commands_get_the_agent_temp_dir_and_may_write_it() {
    let engine = CodexAppEngine::new("codex", "0.162.1");
    let mut t = turn(None, false);
    assert!(
        !engine
            .args(&t)
            .join(" ")
            .contains("shell_environment_policy")
    );
    assert_eq!(
        turn_params(&t, "th", false)["sandboxPolicy"]["writableRoots"],
        json!(["/shared"])
    );
    t.temp_dir = Some(PathBuf::from("/var/t/0123456789AB"));
    let args = engine.args(&t);
    for name in aether_core::agent_temp::ENV_VARS {
        let set = format!("shell_environment_policy.set.{name}=\"/var/t/0123456789AB\"");
        assert!(
            args.windows(2).any(|pair| pair == ["-c", set.as_str()]),
            "{args:?}"
        );
    }
    for (session, network) in [
        (None, true),
        (Some("th"), true),
        (None, false),
        (Some("th"), false),
    ] {
        t.session = session.map(str::to_owned);
        t.network_access = network;
        assert_eq!(
            turn_params(&t, "th", false)["sandboxPolicy"]["writableRoots"],
            json!(["/shared", "/var/t/0123456789AB"]),
            "{session:?} network={network}"
        );
    }
}

#[test]
fn codex_commands_get_the_build_cache_and_may_write_it() {
    let engine = CodexAppEngine::new("codex", "0.162.1");
    let mut t = turn(None, false);
    t.temp_dir = Some(PathBuf::from("/var/t/0123456789AB"));
    t.build_env = vec![(
        "CARGO_TARGET_DIR".into(),
        "/var/cache/r/run/cargo-target".into(),
    )];
    let args = engine.args(&t);
    let set = "shell_environment_policy.set.CARGO_TARGET_DIR=\"/var/cache/r/run/cargo-target\"";
    assert!(args.windows(2).any(|pair| pair == ["-c", set]), "{args:?}");
    assert_eq!(
        turn_params(&t, "th", false)["sandboxPolicy"]["writableRoots"],
        json!([
            "/shared",
            "/var/t/0123456789AB",
            "/var/cache/r/run/cargo-target"
        ])
    );
    t.summary_only = true;
    assert_eq!(
        turn_params(&t, "th", false)["sandboxPolicy"],
        json!({"type": "readOnly"}),
        "a summary turn stays read-only"
    );
}

/// Writes an executable shell script named `codex` under a fresh temp
/// dir and returns that dir, so `detect_verbose_on` can be pointed at it
/// instead of the host's real CLI. `help_script` answers `app-server
/// --help`; every other argv (including plain `--version`) falls through
/// to `version_script`.
fn fake_codex(version_script: &str, help_script: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    crate::test_script::write(
        &dir.path().join("codex"),
        &format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n  \"app-server --help\")\n    {help_script}\n    ;;\n  *)\n    {version_script}\n    ;;\nesac\n"
        ),
    );
    dir
}

#[tokio::test]
async fn detect_reports_a_good_version() {
    let dir = fake_codex("echo 'codex-cli 0.154.0'", "exit 0");
    let engine = CodexAppEngine::detect_verbose_on(Some(dir.path()))
        .await
        .expect("app-server support detects");
    assert_eq!(engine.version, "0.154.0");
}

#[tokio::test]
async fn detect_reports_unparsable_output() {
    let dir = fake_codex("echo 'not a version'", "exit 0");
    let error = CodexAppEngine::detect_verbose_on(Some(dir.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DetectError::UnparsableVersion { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn detect_reports_a_nonzero_exit() {
    let dir = fake_codex("echo 'not logged in' >&2\nexit 1", "exit 0");
    let error = CodexAppEngine::detect_verbose_on(Some(dir.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DetectError::ExitFailure { .. }),
        "{error:?}"
    );
    assert!(error.to_string().contains("not logged in"));
}

#[tokio::test]
async fn detect_reports_an_absent_program() {
    let dir = tempfile::tempdir().unwrap();
    let error = CodexAppEngine::detect_verbose_on(Some(dir.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DetectError::NotInstalled { .. }),
        "{error:?}"
    );
}

/// The renamed or removed flag case: a `codex` recent enough
/// to answer `--version` but whose `app-server` subcommand does not
/// exist (or its `--help` fails for any other reason).
#[tokio::test]
async fn detect_reports_a_missing_app_server_capability() {
    let dir = fake_codex(
        "echo 'codex-cli 0.10.0'",
        "echo 'unrecognized subcommand app-server' >&2\nexit 2",
    );
    let error = CodexAppEngine::detect_verbose_on(Some(dir.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DetectError::MissingCapability { capability, .. } if capability == "app-server"),
        "{error:?}"
    );
    assert!(error.to_string().contains("unrecognized subcommand"));
    assert!(error.to_string().contains("upgrade codex"));
}

#[test]
fn mcp_inventory_requires_our_tools_and_handles_startup_and_pagination() {
    assert_eq!(
        mcp_inventory(&json!({"data":[], "nextCursor":"next"})),
        McpInventory::Next("next".into())
    );
    assert!(matches!(
        mcp_inventory(&json!({"data":[{"name":"other", "tools":{"delegate":{}}}]})),
        McpInventory::Unavailable(_)
    ));
    for status in ["notStarted", "starting"] {
        assert_eq!(
            mcp_inventory(&json!({"data":[{"name":"aether", "runtimeStatus":status, "tools":{}}]})),
            McpInventory::Starting
        );
    }
    for status in ["failed", "disabled", "authenticationRequired", "cancelled"] {
        assert!(matches!(
            mcp_inventory(
                &json!({"data":[{"name":"aether", "runtimeStatus":status, "tools":{"delegate":{}}}]})
            ),
            McpInventory::Unavailable(_)
        ));
    }
    assert!(matches!(
        mcp_inventory(
            &json!({"data":[{"name":"aether", "runtimeStatus":"connected", "tools":{}}]})
        ),
        McpInventory::Unavailable(_)
    ));
    assert_eq!(
        mcp_inventory(
            &json!({"data":[{"name":"aether", "runtimeStatus":"connected", "tools":{"delegate":{},"list_models":{}}}]})
        ),
        McpInventory::Ready(vec!["delegate".into(), "list_models".into()])
    );
    assert!(matches!(
        mcp_inventory(&json!({})),
        McpInventory::Unavailable(_)
    ));
}

#[test]
fn ordinary_turns_preserve_native_instruction_defaults_on_start_and_resume() {
    for session in [None, Some("existing-thread")] {
        let mut t = turn(session, false);
        t.system_prompt.clear();
        let (_, params) = thread_params(&t);
        assert!(params.get("baseInstructions").is_none());
        assert!(params.get("developerInstructions").is_none());
    }
}

#[test]
fn native_network_tracks_scope_on_fresh_and_resumed_turns() {
    let engine = CodexAppEngine::new("codex", "test");
    for session in [None, Some("existing")] {
        for bypass in [false, true] {
            let mut t = turn(session, bypass);
            assert!(
                !engine
                    .args(&t)
                    .iter()
                    .any(|arg| arg == "web_search=\"disabled\"")
            );
            assert!(
                !engine
                    .args(&t)
                    .iter()
                    .any(|arg| arg == "web_search=\"live\""),
                "ordinary turns preserve the native web-search setting"
            );
            if !bypass {
                // Approvals gate the network: hosts are reached by an approved escalation.
                assert_eq!(
                    turn_params(&t, "thread", false)["sandboxPolicy"]["networkAccess"],
                    false
                );
            }
            t.network_access = false;
            assert!(
                engine
                    .args(&t)
                    .iter()
                    .any(|arg| arg == "web_search=\"disabled\"")
            );
            assert!(
                !engine
                    .args(&t)
                    .iter()
                    .any(|arg| arg == "web_search=\"live\""),
                "network-denied turns must not force live search"
            );
            assert_eq!(thread_params(&t).1["sandbox"], "workspace-write");
            // On Linux Aether's loopback-only sandbox, not Codex's seccomp, denies egress.
            assert_eq!(
                turn_params(&t, "thread", false)["sandboxPolicy"]["networkAccess"],
                cfg!(target_os = "linux")
            );
        }
    }
}

/// Network-denied, non-summary Linux turns, and only those, select the
/// sandboxed exec-server; writable roots never change.
#[test]
fn loopback_sandbox_matches_network_and_summary_matrix() {
    for network_access in [true, false] {
        for summary_only in [true, false] {
            for bypass in [false, true] {
                for session in [None, Some("th1")] {
                    let mut t = turn(session, bypass);
                    t.network_access = network_access;
                    t.summary_only = summary_only;
                    let tp = turn_params(&t, "th1", false);
                    let label = format!(
                        "network={network_access} summary={summary_only} bypass={bypass} session={session:?}"
                    );
                    let sandboxed = cfg!(target_os = "linux") && !network_access && !summary_only;
                    let ours = json!([{"environmentId": "aether-loopback", "cwd": "/w"}]);
                    // thread/resume has no environments field; only a fresh thread gets it.
                    let (_, thread) = thread_params(&t);
                    if sandboxed && session.is_none() {
                        assert_eq!(thread["environments"], ours, "{label}");
                    } else {
                        assert!(thread.get("environments").is_none(), "{label}");
                    }
                    if sandboxed {
                        assert_eq!(tp["environments"], ours, "{label}");
                        assert_eq!(tp["sandboxPolicy"]["type"], "workspaceWrite", "{label}");
                        assert_eq!(tp["sandboxPolicy"]["networkAccess"], true, "{label}");
                        assert_eq!(
                            tp["sandboxPolicy"]["writableRoots"],
                            json!(["/shared"]),
                            "{label}"
                        );
                    } else {
                        assert!(tp.get("environments").is_none(), "{label}");
                    }
                    if summary_only {
                        assert_eq!(tp["sandboxPolicy"], json!({"type": "readOnly"}), "{label}");
                    }
                    if network_access && !summary_only && !bypass {
                        assert_eq!(
                            tp["sandboxPolicy"],
                            json!({
                                "type": "workspaceWrite",
                                "writableRoots": ["/shared"],
                                "networkAccess": false,
                            }),
                            "{label}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn loopback_sandbox_isolates_the_network_and_keeps_arguments_literal() {
    let args = super::super::loopback::bwrap_args(
        std::path::Path::new("/tmp/aether-codex-x/exec.sock"),
        "/opt/codex bin/codex",
        "abc123",
        std::path::Path::new("/tmp/aether-codex-x/unix.bpf"),
    );
    let script_at = args.iter().position(|a| a == "-c").unwrap();
    // The script must be pid 1, so the namespace ends with it.
    for flag in [
        "--unshare-net",
        "--unshare-pid",
        "--as-pid-1",
        "--die-with-parent",
    ] {
        assert!(args[..script_at].iter().any(|a| a == flag), "{flag}");
    }
    assert!(!args.iter().any(|a| a == "--share-net"));
    let script = &args[script_at + 1];
    assert!(script.contains("--ws-auth capability-token --ws-token-sha256 \"$3\""));
    assert!(script.contains("cat <&3 >/dev/null; kill \"$server\""));
    assert!(script.contains(
        "bwrap --dev-bind / / --unshare-pid --proc /proc --seccomp 9 -- \"$2\" exec-server"
    ));
    assert!(script.contains("</dev/null 9<\"$5\" &"));
    assert_eq!(
        args[args.len() - 6..],
        [
            "sh",
            "/tmp/aether-codex-x/exec.sock",
            "/opt/codex bin/codex",
            "abc123",
            "47001",
            "/tmp/aether-codex-x/unix.bpf"
        ]
    );
}

/// Run the classic BPF seccomp program on one system call with its first two arguments.
#[cfg(target_os = "linux")]
fn run_filter(filter: &[u8], arch: u32, nr: u32, args: [u32; 2]) -> u32 {
    use nix::libc;
    let mut accumulator = 0;
    let mut pc = 0;
    loop {
        let at = &filter[pc * 8..pc * 8 + 8];
        let code = u32::from(u16::from_ne_bytes([at[0], at[1]]));
        let (jt, jf) = (usize::from(at[2]), usize::from(at[3]));
        let k = u32::from_ne_bytes([at[4], at[5], at[6], at[7]]);
        pc += 1;
        match code {
            c if c == libc::BPF_RET | libc::BPF_K => return k,
            c if c == libc::BPF_LD | libc::BPF_W | libc::BPF_ABS => {
                accumulator = match k {
                    0 => nr,
                    4 => arch,
                    16 => args[0],
                    24 => args[1],
                    other => panic!("unexpected load offset {other}"),
                }
            }
            c if c == libc::BPF_ALU | libc::BPF_AND | libc::BPF_K => accumulator &= k,
            c if c == libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K => {
                pc += if accumulator == k { jt } else { jf }
            }
            c if c == libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K => {
                pc += if accumulator & k != 0 { jt } else { jf }
            }
            other => panic!("unexpected opcode {other:#x}"),
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn loopback_seccomp_filter_allows_only_confined_sockets() {
    use nix::libc;
    let Some(filter) = super::super::loopback::seccomp_filter() else {
        return;
    };
    let arch = if cfg!(target_arch = "x86_64") {
        0xC000_003E
    } else {
        0xC000_00B7
    };
    let allow = libc::SECCOMP_RET_ALLOW;
    let eperm = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
    let flags = (libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC) as u32;
    let socket = |domain: i32, kind: i32| {
        run_filter(
            &filter,
            arch,
            libc::SYS_socket as u32,
            [domain as u32, kind as u32],
        )
    };
    let pair = |domain: i32, kind: u32| {
        run_filter(
            &filter,
            arch,
            libc::SYS_socketpair as u32,
            [domain as u32, kind],
        )
    };
    for domain in [libc::AF_INET, libc::AF_INET6, libc::AF_NETLINK] {
        assert_eq!(socket(domain, libc::SOCK_STREAM), allow, "{domain}");
    }
    // Unix and vsock sockets reach host listeners from inside the namespace.
    for domain in [
        libc::AF_UNIX,
        libc::AF_VSOCK,
        libc::AF_PACKET,
        libc::AF_ALG,
        libc::AF_RDS,
        libc::AF_PPPOX,
        libc::AF_UNSPEC,
    ] {
        for kind in [libc::SOCK_STREAM, libc::SOCK_DGRAM, libc::SOCK_SEQPACKET] {
            assert_eq!(socket(domain, kind), eperm, "{domain} {kind}");
        }
    }
    // Connected stream pairs serve IPC such as a browser's; datagram pairs could
    // still connect or send to host sockets, and AF_UNIX SOCK_RAW is a datagram socket.
    for kind in [libc::SOCK_STREAM, libc::SOCK_SEQPACKET] {
        assert_eq!(pair(libc::AF_UNIX, kind as u32), allow, "{kind}");
        assert_eq!(pair(libc::AF_UNIX, kind as u32 | flags), allow, "{kind}");
        assert_eq!(pair(libc::AF_INET, kind as u32), eperm, "{kind}");
    }
    for kind in [libc::SOCK_DGRAM, libc::SOCK_RAW, libc::SOCK_RDM, 0, 0xf] {
        assert_eq!(pair(libc::AF_UNIX, kind as u32), eperm, "{kind}");
        assert_eq!(pair(libc::AF_UNIX, kind as u32 | flags), eperm, "{kind}");
    }
    assert_eq!(
        run_filter(&filter, arch, libc::SYS_io_uring_setup as u32, [0, 0]),
        libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32
    );
    for nr in [
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_sendto,
        libc::SYS_openat,
    ] {
        assert_eq!(
            run_filter(&filter, arch, nr as u32, [libc::AF_UNIX as u32, 0]),
            allow
        );
    }
    let kill = libc::SECCOMP_RET_KILL_PROCESS;
    let socket_nr = libc::SYS_socket as u32;
    assert_eq!(
        run_filter(
            &filter,
            arch,
            socket_nr | 0x4000_0000,
            [libc::AF_INET as u32, 1]
        ),
        kill
    );
    assert_eq!(
        run_filter(&filter, 0x4000_0003, socket_nr, [libc::AF_INET as u32, 1]),
        kill
    );
}

#[test]
fn loopback_sandbox_is_confirmed_only_when_it_is_the_sole_environment() {
    use super::super::loopback::confirms;
    let ours = json!({"environmentId": "aether-loopback", "cwd": "/w"});
    let local = json!({"environmentId": "local", "cwd": "/w"});
    assert!(confirms(&json!({"thread": {"environments": [ours]}})));
    assert!(!confirms(
        &json!({"thread": {"environments": [local, ours]}})
    ));
    assert!(!confirms(
        &json!({"thread": {"environments": [ours, local]}})
    ));
    assert!(!confirms(&json!({"thread": {"environments": []}})));
    assert!(!confirms(&json!({"thread": {"environments": null}})));
    assert!(!confirms(&json!({"thread": {}})));
}

#[test]
fn network_policy_requires_the_sandbox_dependencies() {
    let engine = CodexAppEngine::new("codex", "test");
    assert!(engine.validate_network_policy(true).is_ok());
    let reason = crate::offline::dependency_limitation("Codex", |p| p == "bwrap");
    if cfg!(target_os = "linux") {
        let reason = reason.unwrap();
        assert!(reason.starts_with("Network-denied Codex turns require"));
        assert!(reason.contains("missing: socat."));
    } else {
        assert_eq!(reason, None);
    }
}

/// The sandbox relies on experimental app-server methods verified on 0.159.
#[test]
fn loopback_sandbox_requires_a_verified_codex_and_its_dependencies() {
    let all = |_: &str| true;
    for version in ["0.159.0", "0.159.3", "codex-cli 0.160.1"] {
        assert!(
            CodexAppEngine::new("codex", version)
                .validate_loopback_support(all)
                .is_ok(),
            "{version}"
        );
    }
    for version in ["0.158.9", "test", ""] {
        let Err(crate::EngineError::Unsupported(reason)) =
            CodexAppEngine::new("codex", version).validate_loopback_support(all)
        else {
            panic!("{version} must be refused");
        };
        assert!(reason.contains("codex >= 0.159.0"), "{reason}");
    }
    let missing =
        CodexAppEngine::new("codex", "0.159.3").validate_loopback_support(|p| p == "bwrap");
    if cfg!(target_os = "linux") {
        let Err(crate::EngineError::Unsupported(reason)) = missing else {
            panic!("a missing socat must be refused");
        };
        assert!(reason.contains("missing: socat."), "{reason}");
    } else {
        assert!(missing.is_ok());
    }
}

/// Codex's permissions text claims network is enabled inside the loopback sandbox, so
/// each such turn says only loopback is reachable; an allowed turn after one says it no longer applies.
#[test]
fn network_notes_describe_only_the_current_turn() {
    let text = |text: &str| json!({"type": "text", "text": text});
    let offline = text(super::super::LOOPBACK_INSTRUCTIONS);
    let restored = text(super::super::NETWORK_RESTORED_INSTRUCTIONS);
    let gated = text(super::super::GATED_NETWORK_INSTRUCTIONS);
    for network_access in [true, false] {
        for summary_only in [true, false] {
            for bypass in [true, false] {
                for note_in_effect in [true, false] {
                    let session = note_in_effect.then_some("th1");
                    let mut t = turn(session, bypass);
                    t.network_access = network_access;
                    t.summary_only = summary_only;
                    t.system_prompt = "# Aether".into();
                    let label = format!(
                        "network={network_access} summary={summary_only} bypass={bypass} session={session:?} note_in_effect={note_in_effect}"
                    );
                    // Developer instructions persist across thread/resume, so no note goes there.
                    let (_, params) = thread_params(&t);
                    let instructions = params
                        .get("developerInstructions")
                        .or(params.get("baseInstructions"));
                    assert_eq!(instructions, Some(&json!("# Aether")), "{label}");
                    let input = turn_params(&t, "th1", note_in_effect)["input"].clone();
                    let prompt = text("do it");
                    let expected = if cfg!(target_os = "linux") && !network_access && !summary_only
                    {
                        json!([offline, prompt])
                    } else if network_access && !bypass && !summary_only {
                        json!([gated, prompt])
                    } else if note_in_effect && network_access && !summary_only {
                        json!([restored, prompt])
                    } else {
                        json!([prompt])
                    };
                    assert_eq!(input, expected, "{label}");
                }
            }
        }
    }
}

/// The latest note in `thread/resume`'s chronological history decides; an allowed turn
/// on a thread that was never offline stays unchanged.
#[test]
fn loopback_note_in_effect_follows_the_latest_network_note() {
    use super::super::{
        GATED_NETWORK_INSTRUCTIONS, LOOPBACK_INSTRUCTIONS, NETWORK_RESTORED_INSTRUCTIONS,
        loopback_note_in_effect,
    };
    let thread = |turns: &[&[&str]]| {
        let turns: Vec<Value> = turns
            .iter()
            .map(|inputs| {
                let content: Vec<Value> = inputs
                    .iter()
                    .map(|text| json!({"type": "text", "text": text, "text_elements": []}))
                    .collect();
                json!({"items": [
                    {"type": "userMessage", "content": content},
                    {"type": "agentMessage", "text": LOOPBACK_INSTRUCTIONS},
                ]})
            })
            .collect();
        json!({"id": "th", "turns": turns})
    };
    let (offline, restored) = (LOOPBACK_INSTRUCTIONS, NETWORK_RESTORED_INSTRUCTIONS);
    let quoted = format!("quote: {offline}");
    for (turns, in_effect) in [
        (vec![], false),
        (vec![&["hi"][..]], false),
        (vec![&[offline, "probe"][..]], true),
        (vec![&[offline, "probe"][..], &["steered"][..]], true),
        (
            vec![&[offline, "probe"][..], &[restored, "fetch"][..]],
            false,
        ),
        (
            vec![&[restored, "fetch"][..], &[offline, "probe"][..]],
            true,
        ),
        (vec![&[GATED_NETWORK_INSTRUCTIONS, "probe"][..]], true),
        (
            vec![
                &[GATED_NETWORK_INSTRUCTIONS, "probe"][..],
                &[restored, "fetch"][..],
            ],
            false,
        ),
        // Only our own input items count, not a prompt that quotes a note.
        (vec![&[quoted.as_str()][..]], false),
    ] {
        assert_eq!(
            loopback_note_in_effect(&thread(&turns)),
            in_effect,
            "{turns:?}"
        );
    }
    // Missing history: the correction is accurate on any allowed turn, so send it.
    assert!(loopback_note_in_effect(&json!({"id": "th"})));
}

#[test]
fn loopback_environment_debug_redacts_the_token() {
    let environment = super::super::loopback::Environment {
        url: "ws://127.0.0.1:9".into(),
        token: "bearer-secret".into(),
    };
    let shown = format!("{environment:?}");
    assert!(!shown.contains("bearer-secret"), "{shown}");
    assert!(shown.contains("ws://127.0.0.1:9"), "{shown}");
}

/// A newline-free stream must not grow memory, and lines after it still parse.
#[tokio::test]
async fn loopback_pipe_reads_are_bounded() {
    use super::super::loopback::{capped_line, stderr_head};
    let mut flood = vec![b'x'; 1 << 20];
    flood.extend_from_slice(b"\nws://127.0.0.1:1\ntail");
    let mut reader = tokio::io::BufReader::new(&flood[..]);
    let first = capped_line(&mut reader).await.unwrap().unwrap();
    assert_eq!(first.len(), 4_096);
    assert_eq!(
        capped_line(&mut reader).await.unwrap().unwrap(),
        b"ws://127.0.0.1:1"
    );
    assert_eq!(capped_line(&mut reader).await.unwrap().unwrap(), b"tail");
    assert_eq!(capped_line(&mut reader).await.unwrap(), None);
    let head = stderr_head(&flood[..]).await;
    assert_eq!(head.len(), 2_000);
    assert!(head.bytes().all(|b| b == b'x'));
}

/// Search configuration stays native unless isolation requires disabling it.
#[test]
fn web_search_override_matches_network_and_summary_matrix() {
    let engine = CodexAppEngine::new("codex", "test");
    for network_access in [true, false] {
        for summary_only in [true, false] {
            for bypass in [true, false] {
                let mut t = turn(None, bypass);
                t.network_access = network_access;
                t.summary_only = summary_only;
                let args = engine.args(&t);
                let has_live = args.iter().any(|arg| arg == "web_search=\"live\"");
                let has_disabled = args.iter().any(|arg| arg == "web_search=\"disabled\"");
                assert!(
                    !has_live,
                    "network_access={network_access} summary_only={summary_only} bypass={bypass}"
                );
                let expect_disabled = !network_access || summary_only;
                assert_eq!(
                    has_disabled, expect_disabled,
                    "network_access={network_access} summary_only={summary_only} bypass={bypass}"
                );
            }
        }
    }
}

#[test]
fn native_hooks_are_preserved_except_on_isolated_turns() {
    let engine = CodexAppEngine::new("codex", "test");
    for summary_only in [false, true] {
        for network_access in [false, true] {
            let mut t = turn(None, false);
            t.summary_only = summary_only;
            t.network_access = network_access;
            let args = engine.args(&t);
            let count = args
                .iter()
                .filter(|arg| *arg == "features.hooks=false")
                .count();
            assert_eq!(
                count,
                usize::from(summary_only || !network_access),
                "summary_only={summary_only} network_access={network_access}"
            );
        }
    }
}

/// Only explicitly configured effort overrides native settings.
#[test]
fn effort_overrides_are_explicit_and_native_defaults_are_preserved() {
    let engine = CodexAppEngine::new("codex", "test");
    let cases = [
        (Some(Effort::Low), "low"),
        (Some(Effort::Medium), "medium"),
        (Some(Effort::High), "high"),
        (Some(Effort::XHigh), "xhigh"),
        (Some(Effort::Max), "max"),
        (None, ""),
    ];
    for (effort, expected) in cases {
        let mut t = turn(None, false);
        t.effort = effort;
        let args = engine.args(&t);
        let count = args
            .iter()
            .filter(|a| a.starts_with("model_reasoning_effort="))
            .count();
        assert_eq!(count, usize::from(effort.is_some()), "effort={effort:?}");
        if effort.is_some() {
            assert!(
                args.contains(&format!("model_reasoning_effort=\"{expected}\"")),
                "effort={effort:?} args={args:?}"
            );
        }
    }
}

#[test]
fn turn_params_only_carries_explicit_effort() {
    let mut t = turn(None, false);
    t.effort = None;
    assert!(turn_params(&t, "th", false).get("effort").is_none());
    t.effort = Some(Effort::Max);
    assert_eq!(turn_params(&t, "th", false)["effort"], "max");
}

#[test]
fn a_resumed_thread_uses_the_model_selected_for_its_next_turn() {
    let mut t = turn(Some("existing-thread"), false);
    t.model = Some("gpt-6-sol".into());
    assert_eq!(thread_params(&t).0, "thread/resume");
    assert_eq!(
        turn_params(&t, "existing-thread", false)["model"],
        "gpt-6-sol"
    );

    t.model = Some("default".into());
    assert!(
        turn_params(&t, "existing-thread", false)
            .get("model")
            .is_none()
    );
    t.model = None;
    assert!(
        turn_params(&t, "existing-thread", false)
            .get("model")
            .is_none()
    );
}

#[test]
fn checkpoint_turn_is_isolated_and_read_only() {
    let mut t = turn(None, false);
    t.summary_only = true;
    t.mcp = None;
    let args = CodexAppEngine::new("codex", "test").args(&t);
    assert!(args.contains(&"mcp_servers={}".into()));
    assert!(args.contains(&"features.shell_tool=false".into()));
    assert_eq!(thread_params(&t).1["sandbox"], "read-only");
    assert_eq!(
        turn_params(&t, "thread", false)["sandboxPolicy"]["type"],
        "readOnly"
    );
}

#[test]
fn params_follow_the_approval_policy_and_session() {
    let engine = CodexAppEngine::new("codex", "0.153.4");
    let t = turn(None, false);
    assert_eq!(engine.args(&t)[0], "app-server");
    assert!(
        engine
            .args(&t)
            .contains(&"mcp_servers.aether.default_tools_approval_mode=\"approve\"".into())
    );
    assert!(
        engine
            .args(&t)
            .join(" ")
            .contains("mcp_servers.aether.command=\"/bin/aether\"")
    );
    let (method, params) = thread_params(&t);
    assert_eq!(method, "thread/start");
    assert_eq!(params["approvalPolicy"], "on-request");
    assert_eq!(params["sandbox"], "workspace-write");
    assert!(params.get("baseInstructions").is_none());
    assert_eq!(params["developerInstructions"], "# Aether");
    assert_eq!(params["model"], "gpt-5.3-codex-spark");
    let tp = turn_params(&t, "th1", false);
    assert_eq!(tp["threadId"], "th1");
    // A gated network turn says first how it reaches hosts.
    assert_eq!(
        tp["input"][0]["text"],
        super::super::GATED_NETWORK_INSTRUCTIONS
    );
    assert_eq!(tp["input"][1]["text"], "do it");
    assert_eq!(tp["effort"], "low");
    assert_eq!(tp["sandboxPolicy"]["type"], "workspaceWrite");
    assert_eq!(tp["sandboxPolicy"]["writableRoots"][0], "/shared");

    let (method, params) = thread_params(&turn(Some("th1"), true));
    assert_eq!(method, "thread/resume");
    assert_eq!(params["threadId"], "th1");
    assert!(params.get("baseInstructions").is_none());
    assert_eq!(params["developerInstructions"], "# Aether");
    assert_eq!(params["approvalPolicy"], "on-request");
    assert_eq!(params["sandbox"], "danger-full-access");
    assert_eq!(
        turn_params(&turn(None, true), "x", false)["sandboxPolicy"]["type"],
        "dangerFullAccess"
    );
    assert_eq!(engine.adapter(), "codex-app/0.153.4");
}

/// Only a bypass turn *with* network asks instead of
/// running `never`. Offline bypass keeps `never` (a granted escalation
/// would run outside the sandbox, with network) and summary-only keeps
/// read-only/never; gated agents were already asking.
#[test]
fn only_online_bypass_turns_move_from_never_to_on_request() {
    let cases = [
        // (bypass, network, summary_only) -> (approvalPolicy, sandbox, turn sandbox type)
        (
            true,
            true,
            false,
            "on-request",
            "danger-full-access",
            "dangerFullAccess",
        ),
        (
            true,
            false,
            false,
            "never",
            "workspace-write",
            "workspaceWrite",
        ),
        (
            false,
            true,
            false,
            "on-request",
            "workspace-write",
            "workspaceWrite",
        ),
        (
            false,
            false,
            false,
            "on-request",
            "workspace-write",
            "workspaceWrite",
        ),
        (true, true, true, "never", "read-only", "readOnly"),
        (false, false, true, "never", "read-only", "readOnly"),
    ];
    for (bypass, network, summary, approval, sandbox, turn_sandbox) in cases {
        for session in [None, Some("th1")] {
            let mut t = turn(session, bypass);
            t.network_access = network;
            t.summary_only = summary;
            let (_, params) = thread_params(&t);
            let tp = turn_params(&t, "th1", false);
            let label = format!("bypass={bypass} network={network} summary={summary}");
            assert_eq!(params["approvalPolicy"], approval, "{label}");
            assert_eq!(params["sandbox"], sandbox, "{label}");
            assert_eq!(tp["sandboxPolicy"]["type"], turn_sandbox, "{label}");
            if turn_sandbox == "workspaceWrite" {
                let loopback = cfg!(target_os = "linux") && !network;
                assert_eq!(
                    tp["sandboxPolicy"]["networkAccess"],
                    (network && bypass) || loopback,
                    "{label}"
                );
            }
        }
    }
}

/// Every spawn goes through Aether's `delegate`, never
/// Codex's own multi-agent feature.
#[test]
fn the_multi_agent_feature_is_always_disabled() {
    let engine = CodexAppEngine::new("codex", "0.153.4");
    assert!(
        engine
            .args(&turn(None, false))
            .join(" ")
            .contains("features.multi_agent=false")
    );
    assert!(
        engine
            .args(&turn(None, false))
            .contains(&"agents.enabled=false".into())
    );
    assert!(
        engine
            .args(&turn(Some("resumed"), false))
            .contains(&"agents.enabled=false".into())
    );
}

/// A message's images reach the app server as files, after the text, both
/// when the turn starts and when a message is steered into it.
#[test]
fn images_are_local_image_items_after_the_text() {
    let image = crate::EngineImage {
        path: PathBuf::from("/data/aether.attachments/abc.png"),
        media_type: "image/png".into(),
    };
    let mut t = turn(None, true);
    assert_eq!(
        turn_params(&t, "th1", false)["input"],
        json!([{"type": "text", "text": "do it"}]),
        "a message without images is what it always was"
    );
    t.images = vec![image.clone()];
    let item = json!({"type": "localImage", "path": "/data/aether.attachments/abc.png"});
    assert_eq!(
        turn_params(&t, "th1", false)["input"],
        json!([{"type": "text", "text": "do it"}, item])
    );

    let steered = crate::EngineInput {
        text: "and this\n".into(),
        images: vec![image],
    };
    assert_eq!(
        super::super::steer_params("th1", "turn1", &steered),
        json!({
            "threadId": "th1",
            "expectedTurnId": "turn1",
            "input": [{"type": "text", "text": "and this"}, item],
        })
    );
    assert_eq!(
        super::super::steer_params("th1", "turn1", &"plain".into())["input"],
        json!([{"type": "text", "text": "plain"}])
    );
}
