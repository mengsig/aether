use super::*;
use aether_core::model::ModelRef;
use aether_core::tool::SideEffectClass;
use std::io::Write as _;

struct FailingWriter {
    write_error: Option<io::ErrorKind>,
    flush_error: Option<io::ErrorKind>,
}

impl io::Write for FailingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self.write_error {
            Some(kind) => Err(io::Error::from(kind)),
            None => Ok(buffer.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.flush_error {
            Some(kind) => Err(io::Error::from(kind)),
            None => Ok(()),
        }
    }
}

fn failing_writer(
    write_error: Option<io::ErrorKind>,
    flush_error: Option<io::ErrorKind>,
) -> FailingWriter {
    FailingWriter {
        write_error,
        flush_error,
    }
}

#[test]
fn artifact_output_accepts_a_closed_pipe_from_write_or_flush() {
    for mut out in [
        failing_writer(Some(io::ErrorKind::BrokenPipe), None),
        failing_writer(None, Some(io::ErrorKind::BrokenPipe)),
    ] {
        write_artifact_output(&mut out, |out| out.write_all(b"artifact"))
            .expect("a consumer may close its pipe after reading enough output");
    }
}

#[test]
fn artifact_output_reports_other_write_and_flush_failures() {
    for mut out in [
        failing_writer(Some(io::ErrorKind::PermissionDenied), None),
        failing_writer(None, Some(io::ErrorKind::WriteZero)),
    ] {
        let error = write_artifact_output(&mut out, |out| out.write_all(b"artifact"))
            .expect_err("only a closed pipe is successful");
        assert_eq!(
            error.downcast_ref::<io::Error>().map(io::Error::kind),
            out.write_error.or(out.flush_error)
        );
        assert!(error.to_string().contains("write artifact to stdout"));
    }
}

fn model(provider: &str, model: &str) -> ModelRef {
    ModelRef::new(provider, model)
}

/// A daemon left from the previous build refuses the
/// handshake while it is very much running, so "no daemon here, start
/// one" is both wrong and unfollowable - `aether` will not replace it.
/// The line the operator reads first has to say to stop it instead.
#[test]
fn a_daemon_that_refused_this_build_is_not_reported_as_a_missing_one() {
    let socket = Path::new("/isolated/aetherd.sock");
    let ordinary = || format!("no daemon at {} (start one)", socket.display());

    let refused = failed_connection(
        socket,
        ClientError::Incompatible("protocol 11 vs 10".into()),
        ordinary(),
    );
    let said = format!("{refused:#}");
    assert!(
        said.contains("aether --socket '/isolated/aetherd.sock' stop"),
        "{said}"
    );
    assert!(!said.contains("no daemon at"), "{said}");
    assert!(
        said.contains("protocol 11 vs 10"),
        "the cause is kept: {said}"
    );
    // And `stop` finds the refusal by type through that added context,
    // which is what lets it fall back to signalling.
    assert!(is_version_skew(&refused), "{said}");

    let gone = failed_connection(socket, ClientError::Disconnected, ordinary());
    assert!(format!("{gone:#}").contains("no daemon at"), "{gone:#}");
    assert!(!is_version_skew(&gone), "only a refusal is signalled");
    assert!(
        !is_version_skew(&anyhow::anyhow!("incompatible daemon: protocol 11 vs 10")),
        "the message alone must not stand in for the typed error"
    );
}

/// The daemon to signal is the peer of the connection that refused this
/// build - read from the socket's peer credentials, so it is whoever
/// holds the socket now rather than a pid file that may name something
/// else. Here that peer is this very process, which is the one identity
/// the test knows, and which must never be signalled.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_refusing_daemon_is_signalled_by_the_pid_holding_the_socket() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("aetherd.sock");
    let greeted = stub_daemon(&socket, Answer::Refuse);

    let error = stop_unaskable_daemon(&socket, true)
        .await
        .expect_err("the peer here is this very process");
    assert!(
        format!("{error:#}").contains("itself listening"),
        "the pid signalled is the socket's peer, and this process is not signalled: {error:#}"
    );
    assert!(
        matches!(greeted.try_recv(), Ok(aether_protocol::Command::Hello(_))),
        "the refusal is confirmed on the connection whose pid would be signalled"
    );
}

/// A daemon this build can talk to is asked to stop, never signalled -
/// including when it took the socket between the refusal and this
/// connection, which is the only way to end up here with a live daemon.
#[tokio::test]
async fn a_daemon_that_can_be_asked_to_stop_is_not_signalled() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("aetherd.sock");
    let greeted = stub_daemon(&socket, Answer::Welcome);

    stop_unaskable_daemon(&socket, true)
        .await
        .expect("a daemon that answers is asked to stop");
    assert!(
        matches!(greeted.recv(), Ok(aether_protocol::Command::Hello(_))),
        "it was greeted"
    );
    assert!(
        matches!(greeted.recv(), Ok(aether_protocol::Command::Shutdown)),
        "and asked to shut down, rather than signalled"
    );
}

/// How the stub daemon answers a hello: as a daemon from another build
/// does, or as one this build can talk to.
enum Answer {
    Refuse,
    Welcome,
}

/// A daemon on `socket` that answers the handshake and, once welcomed,
/// the shutdown that follows it. The channel carries what it was asked,
/// so a test can tell being asked from being signalled.
fn stub_daemon(
    socket: &Path,
    answer: Answer,
) -> std::sync::mpsc::Receiver<aether_protocol::Command> {
    use aether_protocol::{Command as Rpc, ErrorCode, ErrorReply, Reply, ServerFrame};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let listener = tokio::net::UnixListener::bind(socket).expect("bind the stub daemon");
    let (asked, heard) = std::sync::mpsc::channel();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("a client");
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let frame: aether_protocol::ClientFrame =
                serde_json::from_str(&line).expect("a client frame");
            let result = match (&frame.command, &answer) {
                (Rpc::Hello(_), Answer::Refuse) => Err(ErrorReply::new(
                    ErrorCode::Incompatible,
                    "client protocol 10 is not served: this daemon speaks 11 to 12",
                )),
                (Rpc::Hello(_), Answer::Welcome) => Ok(Reply::Welcome(welcome())),
                (Rpc::Shutdown, _) => Ok(Reply::Ok),
                (other, _) => panic!("the stub daemon was asked {other:?}"),
            };
            asked.send(frame.command).expect("the test is listening");
            let mut encoded = serde_json::to_string(&ServerFrame::Reply {
                id: frame.id,
                result,
            })
            .expect("encode");
            encoded.push('\n');
            if write.write_all(encoded.as_bytes()).await.is_err() {
                return;
            }
        }
    });
    heard
}

fn welcome() -> aether_protocol::Welcome {
    aether_protocol::Welcome {
        protocol_version: aether_protocol::PROTOCOL_VERSION,
        daemon_version: "stub".into(),
        instance_id: "stub".into(),
        schema_version: 1,
        pid: std::process::id(),
        features: vec![],
        daemon_protocol_version: None,
        min_protocol_version: None,
        idle_timeout_secs: None,
        device: None,
        submission_horizon: None,
    }
}

/// The daemon a client starts is invoked through this argv and nothing
/// reads it until that child is running with its stdio on `/dev/null`. A
/// renamed flag would kill every spawned daemon on a parse error nobody
/// can see, and turn each auto-start into a timeout with no cause.
#[test]
fn the_argv_a_client_spawns_parses_as_a_detached_foreground_daemon() {
    use clap::Parser as _;

    let socket = PathBuf::from("/run/user/1000/aether/aetherd.sock");
    let command =
        daemon_command(&socket, true, Some(4), true).expect("this test executable has a path");
    let argv: Vec<&std::ffi::OsStr> = std::iter::once(std::ffi::OsStr::new("aether"))
        .chain(command.get_args())
        .collect();

    let parsed = crate::Args::try_parse_from(argv).expect("the spawned argv must parse");
    assert_eq!(parsed.socket.as_deref(), Some(socket.as_path()));
    let Some(crate::Cmd::Daemon {
        foreground,
        detached,
        yolo,
        max_concurrent,
        allow_remote,
        ..
    }) = parsed.command
    else {
        panic!("the spawned argv must name the daemon subcommand");
    };
    assert!(foreground, "the child is the daemon, not another launcher");
    assert!(detached, "and it must know that nobody reads its stderr");
    assert_eq!((yolo, max_concurrent, allow_remote), (true, Some(4), true));
}

/// `--detached` only describes how a foreground daemon was started, so on
/// its own it would silently do nothing.
#[test]
fn detached_is_refused_without_foreground() {
    use clap::Parser as _;

    let argv = ["aether", "daemon", "--detached"];
    assert!(crate::Args::try_parse_from(argv).is_err());
}

/// The gate a detached daemon's own failure relies on: drop it, and every
/// auto-start failure reverts to a bare exit status with no cause.
#[test]
fn the_detached_gate_records_a_failure_only_when_detached() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("aetherd.sock");
    let log = aether_protocol::paths::startup_log_path_for(&socket);

    report_startup_result(&socket, false, &Err(anyhow!("boom")));
    assert!(!log.exists(), "a foreground daemon reports its own failure");

    report_startup_result(&socket, true, &Err(anyhow!("boom")));
    let contents = std::fs::read_to_string(&log).expect("the gate must write it");
    assert!(contents.contains("boom"), "{contents}");

    std::fs::remove_file(&log).expect("clear it");
    report_startup_result(&socket, true, &Ok(()));
    assert!(!log.exists(), "success has nothing to record");
}

/// What `record_startup_failure` itself leaves behind: readable only by
/// this user, and naming the error that stopped the daemon.
#[test]
fn record_startup_failure_writes_a_user_only_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("aetherd.sock");
    record_startup_failure(&socket, &anyhow!("could not bind: address in use"));

    let log = aether_protocol::paths::startup_log_path_for(&socket);
    let contents = std::fs::read_to_string(&log).expect("written");
    assert!(
        contents.contains("could not bind: address in use"),
        "{contents}"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&log)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{mode:o}");
    }
}

#[cfg(unix)]
#[test]
fn startup_diagnostics_harden_existing_public_file_before_replacing_contents() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o1777)).unwrap();
    let socket = root.path().join("aether.sock");
    let log = aether_protocol::paths::startup_log_path_for(&socket);
    std::fs::write(&log, "old diagnostics").unwrap();
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();
    super::lifecycle::record_startup_failure(&socket, &anyhow::anyhow!("replacement"));
    assert_eq!(
        std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "Error: replacement\n"
    );
    assert_eq!(
        std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o7777,
        0o1777
    );
}

#[cfg(unix)]
#[test]
fn startup_diagnostics_do_not_modify_symlink_or_hardlink_targets() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o1777)).unwrap();
    let socket = root.path().join("aether.sock");
    let log = aether_protocol::paths::startup_log_path_for(&socket);
    let victim = root.path().join("keep");
    std::fs::write(&victim, "keep bytes").unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::os::unix::fs::symlink(&victim, &log).unwrap();
    super::lifecycle::record_startup_failure(&socket, &anyhow::anyhow!("replace"));
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep bytes");
    std::fs::remove_file(&log).unwrap();
    std::fs::hard_link(&victim, &log).unwrap();
    super::lifecycle::record_startup_failure(&socket, &anyhow::anyhow!("replace"));
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep bytes");
    assert_eq!(
        std::fs::metadata(victim).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

/// Every scope a memory can carry is nameable, and its label reads back.
#[test]
fn every_memory_scope_is_nameable_on_the_command_line() {
    use aether_core::memory::MemoryScope;
    let agent = aether_core::id::AgentId::new();
    let cases = [
        ("global", MemoryScope::Global),
        (
            "github.com/o/r",
            MemoryScope::Repository {
                id: "github.com/o/r".into(),
            },
        ),
        (
            "role:team.worker",
            MemoryScope::Role {
                role: "team.worker".into(),
            },
        ),
        (&*format!("agent:{agent}"), MemoryScope::Agent { id: agent }),
        (
            &*format!("subtree:{agent}"),
            MemoryScope::Subtree { root: agent },
        ),
        (
            "repo-role:team.steward@/home/u@corp/repo",
            MemoryScope::RepositoryRole {
                id: "/home/u@corp/repo".into(),
                role: "team.steward".into(),
            },
        ),
    ];
    for (text, scope) in cases {
        match (parse_memory_scope(text).expect(text), scope) {
            (MemoryScopeRef::Fixed(parsed), scope) => assert_eq!(parsed, scope, "{text}"),
            (MemoryScopeRef::Agent(agent), MemoryScope::Agent { id })
            | (MemoryScopeRef::Subtree(agent), MemoryScope::Subtree { root: id }) => {
                assert_eq!(agent.to_string(), id.to_string(), "{text}");
            }
            (parsed, scope) => panic!("{text}: {parsed:?} is not {scope:?}"),
        }
    }
    // The agent part is a reference: a name or a prefix resolves later.
    assert!(matches!(
        parse_memory_scope("subtree: planner").expect("a name"),
        MemoryScopeRef::Subtree(root) if root.to_string() == "planner"
    ));
    assert_eq!(
        memory_scope_label(&MemoryScope::RepositoryRole {
            id: "r".into(),
            role: "team".into()
        }),
        "repo-role team@r"
    );
    for bad in [
        "repo-role:team",
        "repo-role:has space@r",
        "repo-role:team@",
        "subtree: ",
        "agent:",
    ] {
        assert!(parse_memory_scope(bad).is_err(), "{bad}");
    }
}

/// `aether run` reads the same saved chain the TUI does;
/// `filter_known_models` is the validation both now share:
/// a stale primary drops the chain, a
/// stale fallback is dropped on its own rather than reaching creation.
#[test]
fn a_stale_primary_drops_the_whole_chain_but_a_stale_fallback_is_dropped_alone() {
    let known = [model("anthropic", "claude"), model("openai", "gpt")];

    let live = ModelChain::new(model("anthropic", "claude"), vec![model("openai", "gpt")]);
    assert_eq!(
        filter_known_models(live.clone(), &known),
        Some(live),
        "every entry still resolves, so nothing changes"
    );

    let stale_primary = ModelChain::new(model("anthropic", "removed"), vec![]);
    assert_eq!(
        filter_known_models(stale_primary, &known),
        None,
        "a stale primary must drop the whole chain, not fall through unvalidated"
    );

    let stale_fallback = ModelChain::new(
        model("anthropic", "claude"),
        vec![model("openai", "removed")],
    );
    assert_eq!(
        filter_known_models(stale_fallback, &known),
        Some(ModelChain::new(model("anthropic", "claude"), vec![])),
        "a stale fallback must not reach create_agent and fail there"
    );
}

fn scope(agent: Option<&str>, everywhere: bool) -> RuleScopeArgs {
    RuleScopeArgs {
        agent: agent.map(|a| a.parse().expect("agent reference")),
        subtree: None,
        run: None,
        everywhere,
    }
}

/// `runs import` reads a file this machine did not write and prints what
/// it found before anything else. Nothing out of that file may move a
/// cursor, set a title or reorder the line it lands in.
#[test]
fn nothing_out_of_an_imported_file_can_repaint_the_terminal() {
    let hostile = "\u{1b}[2Jowned\u{9b}0;x\u{202e}rehto\u{202c}";
    let lines = imported_lines(
        &aether_core::transfer::ImportSummary {
            run: aether_core::id::RunId::new(),
            source_run: aether_core::id::RunId::new(),
            source_instance: hostile.into(),
            exported_at: aether_core::time::now(),
            agents: 1,
            tasks: 1,
            events: 2,
            snapshots: 0,
            invocations: 0,
            checkpoints: 0,
            artifacts: 0,
            artifact_payloads: 0,
        },
        "aether",
    );
    assert!(lines.contains("owned"), "the value itself still shows");
    for c in ['\u{1b}', '\u{9b}', '\u{202e}', '\u{202c}'] {
        assert!(!lines.contains(c), "{c:?} reached the terminal: {lines:?}");
    }
}

/// The same for the error strings `aether events` and `aether replay`
/// print, which an import serves like any other event.
#[test]
fn an_imported_error_string_is_stripped_before_it_is_summarized() {
    let hostile = "boom\u{1b}[2J\u{9b}0;x";
    for kind in [
        EventKind::AgentFailed {
            error: hostile.into(),
        },
        EventKind::TaskFailed {
            error: hostile.into(),
            structured: None,
        },
        EventKind::ToolFailed {
            call: aether_core::id::ToolCallId::new(),
            error: hostile.into(),
            retryable: false,
            duration_ms: None,
        },
    ] {
        let line = summarize(&kind);
        assert!(line.contains("boom"), "{line:?}");
        assert!(
            !line.contains('\u{1b}') && !line.contains('\u{9b}'),
            "{line:?}"
        );
    }
}

#[test]
fn deadline_in_the_past_is_refused_naming_the_value() {
    let err = parse_deadline("2020-01-01T00:00:00Z").expect_err("past");
    let message = err.to_string();
    assert!(message.contains("2020-01-01T00:00:00Z"), "{message}");
    assert!(message.contains("not in the future"), "{message}");
}

#[test]
fn deadline_parses_an_instant_or_removes_it() {
    assert_eq!(parse_deadline("none").expect("none"), DeadlineUpdate::Clear);
    assert_eq!(parse_deadline("NONE").expect("none"), DeadlineUpdate::Clear);
    let at = parse_deadline("2030-01-02T03:04:05Z").expect("rfc 3339");
    assert!(matches!(at, DeadlineUpdate::Set(_)));
    let err = parse_deadline("tomorrow").expect_err("not an instant");
    assert_eq!(
        err.to_string(),
        "deadline must be RFC 3339, or none: input contains invalid characters"
    );
}

#[test]
fn backend_parses_both_forms_and_refuses_a_container_without_an_image() {
    assert_eq!(parse_backend("host").expect("host"), ExecutionBackend::Host);
    assert_eq!(
        parse_backend("container:rust:1").expect("image"),
        ExecutionBackend::Container {
            image: "rust:1".into()
        }
    );
    // A container without an image names no sandbox, so it is not one.
    for bad in ["container", "container:", "container: ", "vm", "host:x", ""] {
        assert_eq!(
            parse_backend(bad).expect_err("refused").to_string(),
            format!(
                "`{}` is not a backend; use `host` or `container:<image>`",
                bad.trim()
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn a_scope_is_never_everywhere_by_omission() {
    // No flag is "the caller decides": answering an approval keeps it for
    // the agent that was answered, and a hand-written rule is refused.
    assert!(!scope(None, false).given().expect("no flag"));
    assert!(scope(None, true).given().expect("everywhere"));
    let id = aether_core::id::AgentId::new().to_string();
    assert!(scope(Some(&id), false).given().expect("one agent"));
    assert!(scope(Some(&id), true).given().is_err(), "one scope only");
}

#[test]
fn network_and_priority_take_their_documented_words() {
    assert_eq!(parse_network("off").expect("off"), NetworkPolicy::Denied);
    assert_eq!(
        parse_network("ALLOW").expect("allow"),
        NetworkPolicy::Allowed
    );
    assert_eq!(
        parse_network("maybe").expect_err("refused").to_string(),
        "network must be denied or allowed: maybe"
    );
    assert_eq!(parse_priority("Critical").expect("p"), Priority::Critical);
    assert_eq!(
        parse_priority("urgent").expect_err("refused").to_string(),
        "priority must be low, normal, high or critical: urgent"
    );
}

#[test]
fn approval_takes_none_or_a_list_of_classes() {
    assert_eq!(
        parse_approval("None").expect("none"),
        ApprovalPolicy::none()
    );
    let policy = parse_approval("mutation, external").expect("two classes");
    assert_eq!(
        policy.require_for,
        [SideEffectClass::Mutation, SideEffectClass::External].into()
    );
    assert_eq!(
        parse_approval("mutation,writes")
            .expect_err("refused")
            .to_string(),
        "approval class must be read_only, idempotent_mutation, mutation, external or none: writes"
    );
}

#[test]
fn error_message_keeps_each_distinct_cause_once() {
    let refused = anyhow::Error::new(ClientError::Protocol(aether_protocol::ErrorReply::new(
        aether_protocol::ErrorCode::NotFound,
        "unknown agent",
    )))
    .context("sending to agt_1");
    assert_eq!(
        error_message(&refused),
        "sending to agt_1: daemon refused: NotFound: unknown agent"
    );
    let io = anyhow::Error::new(io::Error::other("disk full"))
        .context("writing the log")
        .context("starting the run");
    assert_eq!(
        error_message(&io),
        "starting the run: writing the log: disk full"
    );
}

/// An answer that is an error, and an engine message whose spend is reported
/// elsewhere, must not read like a success and a free call.
#[test]
fn error_answers_and_engine_usage_are_summarized_for_what_they_are() {
    let answered = |is_error| EventKind::ToolSucceeded {
        call: aether_core::id::ToolCallId::new(),
        outcome: aether_core::tool::ToolOutcome {
            content: "no such file".into(),
            is_error,
            ..Default::default()
        },
    };
    assert_eq!(summarize(&answered(true)), "error: no such file");
    assert_eq!(summarize(&answered(false)), "no such file");

    let completed = |usage| EventKind::InvocationCompleted {
        invocation: aether_core::id::InvocationId::new(),
        snapshot: aether_core::id::SnapshotId::new(),
        model: model("mock", "m1"),
        usage,
        stop_reason: None,
        duration_ms: 12,
    };
    let engine = summarize(&completed(aether_core::model::Usage::default()));
    assert_eq!(engine, "12 ms; usage in agent.usage_updated");
    let api = aether_core::model::Usage {
        input_tokens: 5,
        output_tokens: 2,
        ..Default::default()
    };
    assert_eq!(summarize(&completed(api)), "5 in / 2 out in 12 ms");
    let updated = EventKind::AgentUsageUpdated {
        total: api,
        turn_usage: api,
    };
    assert_eq!(summarize(&updated), "5 in / 2 out this turn");
}

#[test]
fn message_tool_results_and_custom_events_have_sanitized_details() {
    use aether_core::model::{ContentBlock, MessageRole, ToolResultBlock};
    let hostile = "failed\u{1b}[2J\nretry\u{202e}";
    let kind = EventKind::AgentMessage {
        role: MessageRole::User,
        content: vec![
            ContentBlock::text("answer:"),
            ContentBlock::ToolResult {
                tool_use_id: "call".into(),
                content: vec![
                    ToolResultBlock::Text {
                        text: hostile.into(),
                    },
                    ToolResultBlock::Image {
                        media_type: "image/png".into(),
                        data: "private base64".into(),
                    },
                ],
                is_error: true,
            },
        ],
        invocation: None,
    };
    let text = summarize(&kind);
    assert!(
        text.contains("User: answer: tool result error: failed"),
        "{text}"
    );
    assert!(text.contains("retry") && text.contains("[image]"), "{text}");
    assert!(!text.contains("private base64"));
    assert!(!text.contains(['\u{1b}', '\u{202e}', '\n']));
    let ok = EventKind::AgentMessage {
        role: MessageRole::User,
        content: vec![
            ContentBlock::tool_result_text("call", "done", false),
            ContentBlock::text("next"),
        ],
        invocation: None,
    };
    assert_eq!(summarize(&ok), "User: tool result: done next");
    let call = EventKind::AgentMessage {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "call".into(),
            name: "complete_task".into(),
            input: serde_json::json!({"summary": "private input"}),
        }],
        invocation: None,
    };
    assert_eq!(summarize(&call), "Assistant: calls complete_task");
    let text = summarize(&EventKind::Custom {
        name: "native\u{1b}[2Jnote".into(),
        payload: serde_json::json!({"detail": hostile}),
    });
    assert!(
        text.contains("native")
            && text.contains("note")
            && text.contains("detail")
            && text.contains("retry"),
        "{text}"
    );
    assert!(!text.contains(['\u{1b}', '\u{202e}', '\n']));
}

#[test]
fn empty_approval_requires_explicit_none() {
    for spec in ["", " ", ",", " , , "] {
        let error = parse_approval(spec).expect_err("empty approval rejected");
        assert!(
            error
                .to_string()
                .contains("use none to disable approval explicitly")
        );
    }
}

#[test]
fn terminal_error_message_sanitises_untrusted_causes() {
    let error = anyhow::anyhow!("server\x1b[2J\u{202e}failed").context("request\rfailed");
    assert_eq!(error_message(&error), "requestfailed: server[2Jfailed");
}
