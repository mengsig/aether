use super::*;
use crate::commands::CreateAgentSpec;
use crate::test_runtime;
use aether_core::agent::Scope;
use aether_core::artifact::{Artifact, ArtifactKind};
use aether_core::id::{AgentId, SnapshotId, TaskId};
use aether_core::model::ModelRef;
use aether_core::snapshot::InvocationParams;
use aether_store::EventFilter;
use aether_tools::{ArtifactSink, ToolError};

struct NoSink;

#[async_trait::async_trait]
impl ArtifactSink for NoSink {
    async fn store(
        &self,
        _kind: ArtifactKind,
        _name: &str,
        _summary: &str,
        _content_type: Option<&str>,
        _bytes: Vec<u8>,
        _agent: AgentId,
        _task: Option<TaskId>,
    ) -> std::result::Result<Artifact, ToolError> {
        Err(ToolError::Internal("no sink".into()))
    }
}

/// A context shaped like the one `tool_context` builds for an agent with
/// no workspace root: the cwd is still resolved.
fn ctx_without_workspace_root(cwd: &std::path::Path) -> ToolContext {
    ToolContext {
        agent_id: AgentId::new(),
        task_id: None,
        instance_id: "test-instance".into(),
        scope: Scope::default(),
        cwd: cwd.to_path_buf(),
        artifacts: Arc::new(NoSink),
        env: BTreeMap::new(),
        hidden_env: Vec::new().into(),
        temp_dir: None,
        build_env: Vec::new(),
    }
}

/// `engine.interrupted`'s reason is an adapter error string with
/// no length contract of its own, so it must go through the same clamp
/// as its sibling `engine.parked` event before it reaches a durable
/// event or an unbounded TUI transcript render.
#[test]
fn clamp_bounds_engine_chosen_text_by_char_count() {
    let long = "x".repeat(MAX_ENGINE_ERROR + 500);
    let bounded = clamp(&long, MAX_ENGINE_ERROR);
    assert_eq!(bounded.chars().count(), MAX_ENGINE_ERROR);

    let short = "a short, ordinary failure reason";
    assert_eq!(
        clamp(short, MAX_ENGINE_ERROR),
        short,
        "must not pad or alter short text"
    );
}

/// The grace is a delay before the stop, never instead of it: refusing
/// bridge calls alone leaves the engine's own tools running past the
/// grant. Pinned here because the turn token's own cancel at finalize
/// would otherwise hide the line going missing.
#[tokio::test(start_paused = true)]
async fn the_refusal_grace_ends_in_a_stop() {
    let token = CancellationToken::new();
    let (fired_tx, mut fired_rx) = mpsc::channel(1);
    let (input_tx, _input_rx) = mpsc::channel(1);
    let mut input = Some(input_tx);
    let mut park = None;
    let stopped = stop_for_crossing(
        &mut park,
        AgentId::new(),
        REFUSAL_GRACE,
        &BTreeSet::new(),
        &mut input,
        None,
        &token,
        fired_tx,
    );
    assert!(!stopped, "the refusal gets its window first");
    assert!(!token.is_cancelled());
    assert!(input.is_none(), "the engine is asked to stop straight away");
    tokio::time::sleep(REFUSAL_GRACE + std::time::Duration::from_millis(1)).await;
    assert!(token.is_cancelled(), "the stop follows the grace");
    assert!(park.as_ref().is_some_and(Park::killed));
    assert!(fired_rx.recv().await.is_some(), "the loop learns of it");

    // A second crossing on an already parked turn stops it now.
    let token = CancellationToken::new();
    let (fired_tx, _fired_rx) = mpsc::channel(1);
    let mut park = Some(Park::started(&BTreeSet::new()));
    assert!(stop_for_crossing(
        &mut park,
        AgentId::new(),
        REFUSAL_GRACE,
        &BTreeSet::new(),
        &mut None,
        None,
        &token,
        fired_tx,
    ));
    assert!(token.is_cancelled());
}

fn crossed() -> Crossed {
    Crossed {
        reason: "token budget of 40000 exceeded".into(),
        usage: Usage::default(),
    }
}

fn park() -> Option<Park> {
    Some(Park::started(&BTreeSet::new()))
}

fn next() -> Option<Next> {
    Some(Next::Completed(Some("done".into())))
}

/// Both stops cancel the turn's own token, so the cancel flag is always
/// set alongside them. Read it first and a budget stop becomes a bare
/// cancel, and an end the task already recorded becomes one too.
/// The budget outranks the recorded end: the grant is gone either
/// way, and the operator has to see which.
#[test]
fn a_budget_stop_outranks_the_cancel_it_caused() {
    assert!(matches!(
        engine_verdict(Some(crossed()), None, None, true),
        EngineVerdict::Budget(_, None)
    ));
    assert!(matches!(
        engine_verdict(Some(crossed()), None, None, false),
        EngineVerdict::Budget(_, None)
    ));
    // A crossing over a wind-down carries the park through, whether or
    // not the turn's own token was cancelled: the record of what the
    // engine did after the stop is written on this path too.
    assert!(matches!(
        engine_verdict(Some(crossed()), park(), next(), true),
        EngineVerdict::Budget(_, Some(_))
    ));
    assert!(matches!(
        engine_verdict(Some(crossed()), park(), next(), false),
        EngineVerdict::Budget(_, Some(_))
    ));
    // The wind-down a refusal itself starts (`stop_for_crossing`) never
    // produces a `Next` - the budget stop is what ends the turn. The
    // park still has to be carried, or its trace is lost.
    assert!(matches!(
        engine_verdict(Some(crossed()), park(), None, true),
        EngineVerdict::Budget(_, Some(_))
    ));
    assert!(matches!(
        engine_verdict(None, park(), next(), true),
        EngineVerdict::Parked(..)
    ));
    assert!(matches!(
        engine_verdict(None, park(), next(), false),
        EngineVerdict::Parked(..)
    ));
    assert!(matches!(
        engine_verdict(None, None, None, true),
        EngineVerdict::Cancelled
    ));
    assert!(matches!(
        engine_verdict(None, None, None, false),
        EngineVerdict::Ran
    ));
}

/// A stop the runtime forced and a kill the adapter had to force are two
/// different facts about the same run; the tie must not swallow either.
#[test]
fn a_forced_kill_during_a_runtime_stop_names_both() {
    let forced = EngineOutcome {
        forced: true,
        ..Default::default()
    };
    let mut park = Park::started(&BTreeSet::new());
    assert_eq!(stopped_by(&park, &EngineOutcome::default()), "engine");
    assert_eq!(stopped_by(&park, &forced), "adapter");
    park.stop_now(&CancellationToken::new());
    assert_eq!(stopped_by(&park, &EngineOutcome::default()), "runtime");
    assert_eq!(stopped_by(&park, &forced), "runtime+adapter");
}

/// The engine is the untrusted side: its closing figure may add to what it
/// streamed, never take away.
#[test]
fn a_closing_figure_below_the_stream_does_not_lower_it() {
    let streamed = Usage {
        input_tokens: 40_000,
        output_tokens: 1_000,
        reasoning_tokens: Some(200),
        ..Default::default()
    };
    let mut shrunk = Usage {
        input_tokens: 10,
        output_tokens: 1,
        ..Default::default()
    };
    assert!(raise_to(&mut shrunk, streamed), "it had to be raised");
    assert_eq!(shrunk.input_tokens, 40_000);
    assert_eq!(shrunk.output_tokens, 1_000);
    assert_eq!(shrunk.reasoning_tokens, Some(200));
    // The other direction is the point of preferring the closing figure:
    // it knows cache lifetimes the stream did not report.
    let mut fuller = Usage {
        input_tokens: 41_000,
        output_tokens: 1_000,
        cache_read_tokens: 900,
        reasoning_tokens: Some(200),
        ..Default::default()
    };
    assert!(!raise_to(&mut fuller, streamed), "nothing to raise");
    assert_eq!(fuller.cache_read_tokens, 900);
}

fn pending(tool: &str) -> PendingCall {
    PendingCall {
        id: ToolCallId::new(),
        tool: tool.to_owned(),
        input: json!({}),
        started: Instant::now(),
    }
}

/// A permission prompt gates some other tool. Matched on `permission` it
/// closed nothing and the gated call dangled; matched on a name with two
/// of that name in flight it closed one nobody had refused.
#[test]
fn a_refusal_closes_the_call_it_gated_or_none_at_all() {
    let mut calls = BTreeMap::new();
    calls.insert("toolu_bash".to_owned(), pending("Bash"));
    // The provider's own id, when the engine sends it.
    assert_eq!(
        refused_call_id(&calls, &("Bash".into(), Some("toolu_bash".into()))).as_deref(),
        Some("toolu_bash")
    );
    // An id for a call nobody announced closes nothing.
    assert_eq!(
        refused_call_id(&calls, &("Bash".into(), Some("toolu_other".into()))),
        None
    );
    // Without an id, the only pending call of that name.
    assert_eq!(
        refused_call_id(&calls, &("Bash".into(), None)).as_deref(),
        Some("toolu_bash")
    );
    assert_eq!(refused_call_id(&calls, &("Write".into(), None)), None);
    // With two of a name in flight and no id, never a guess.
    calls.insert("toolu_bash_2".to_owned(), pending("Bash"));
    assert_eq!(refused_call_id(&calls, &("Bash".into(), None)), None);
    assert_eq!(
        refused_call_id(&calls, &("Bash".into(), Some("toolu_bash_2".into()))).as_deref(),
        Some("toolu_bash_2"),
        "the id still identifies it"
    );
}

#[test]
fn a_permission_request_names_the_tool_it_gates() {
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let req = EngineRequest {
        name: "permission".into(),
        input: json!({"tool_name": "Bash", "tool_use_id": "toolu_1", "input": {}}),
        reply: tx,
    };
    assert_eq!(
        gated_call(&req),
        ("Bash".to_owned(), Some("toolu_1".to_owned()))
    );
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let plain = EngineRequest {
        name: "remember".into(),
        input: json!({"text": "x"}),
        reply: tx,
    };
    assert_eq!(gated_call(&plain), ("remember".to_owned(), None));
}

/// Keyed on the raw store commit counter this cache never hit: every
/// accounting commits this agent's row immediately before the check, so a
/// supervisor paid a whole-fleet roll-up per assistant message.
#[test]
fn the_subtree_roll_up_survives_this_turns_own_commits() {
    let mut subtree = SubtreeUsage::default();
    let mut store_commits = 100;
    let (mut hits, mut misses) = (0, 0);
    let check = |subtree: &SubtreeUsage, store_commits: u64| {
        subtree.read_at == Some(subtree.key(store_commits))
    };
    // First accounting of the turn: nothing cached yet.
    assert!(!check(&subtree, store_commits));
    misses += 1;
    subtree.read_at = Some(subtree.key(store_commits));
    // Four more accountings, each committing this agent's own row first.
    for _ in 0..4 {
        store_commits += 2;
        subtree.note_own_commits(2);
        if check(&subtree, store_commits) {
            hits += 1;
        } else {
            misses += 1;
            subtree.read_at = Some(subtree.key(store_commits));
        }
    }
    assert_eq!((hits, misses), (4, 1), "one read for the whole turn");
    // Someone else commits - a child's own figure may have moved.
    store_commits += 1;
    assert!(
        !check(&subtree, store_commits),
        "a commit this turn did not make invalidates the roll-up"
    );
}

#[test]
fn both_claude_spawn_tool_names_are_rejected() {
    for name in ["Task", "Agent"] {
        assert_eq!(
            disallowed_native_spawn_tool(EngineKind::CLAUDE_CODE, &[name.into()]),
            Some(name)
        );
    }
    assert_eq!(
        disallowed_native_spawn_tool(EngineKind::CLAUDE_CODE, &["Bash".into()]),
        None
    );
}

#[test]
fn a_shell_call_names_the_tree_it_runs_in_even_without_a_workspace_root() {
    let ctx = ctx_without_workspace_root(std::path::Path::new("/repo"));
    let derived = permission_resources(&ctx, "Bash", &json!({ "command": "rm -rf /etc" }));
    assert_eq!(
        derived.as_deref(),
        Some(aether_tools::shell_resources(&ctx).as_slice()),
        "the engine must derive the same keys the built-in shell reports"
    );
    assert!(
        derived.is_some_and(|r| !r.is_empty()),
        "a known-empty resource set would let a tool-only allow decide the call"
    );
}

#[test]
fn a_derived_path_is_normalized_so_dot_dot_cannot_escape_a_deny() {
    let ctx = ctx_without_workspace_root(std::path::Path::new("/repo/src"));
    let derived = permission_resources(&ctx, "Write", &json!({ "file_path": "../../etc/passwd" }))
        .expect("a path names a resource");
    assert_eq!(derived, vec![ResourceKey::file("/etc/passwd")]);
}

#[test]
fn engine_permission_paths_share_the_builtin_symlink_resource_keys() {
    use aether_tools::Tool;

    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("new", dir.path().join("alias")).unwrap();
    let ctx = ctx_without_workspace_root(dir.path());
    let derived = permission_resources(&ctx, "Write", &json!({"file_path": "alias"})).unwrap();
    let builtin =
        aether_tools::builtin::write_file::WriteFileTool.resources(&json!({"path": "alias"}), &ctx);
    assert_eq!(derived, builtin);
    assert!(derived.contains(&ResourceKey::file(
        dir.path().canonicalize().unwrap().join("new")
    )));
    let matcher = crate::approval::rule_from_answer(
        &crate::approval::Remember::narrow(None),
        "Write",
        None,
        &derived,
    )
    .unwrap();
    assert_eq!(
        matcher.resource_prefix.as_deref(),
        Some(builtin[0].0.as_str())
    );

    let leases = Arc::new(crate::resources::ResourceLeases::new());
    let holder = AgentId::new();
    let held = leases
        .enter(holder, aether_core::task::Priority::Normal, &builtin)
        .try_take()
        .unwrap();
    let direct = permission_resources(&ctx, "Write", &json!({"file_path": "new"})).unwrap();
    let waiter = leases.enter(AgentId::new(), aether_core::task::Priority::Normal, &direct);
    assert_eq!(waiter.try_take().unwrap_err().holder, holder);
    drop(held);
    assert!(
        waiter.try_take().is_ok(),
        "the alias's lease releases its canonical target"
    );
}

#[test]
fn a_payload_with_nothing_keyable_leaves_the_resources_unknown() {
    let ctx = ctx_without_workspace_root(std::path::Path::new("/repo"));
    assert!(permission_resources(&ctx, "WebFetch", &json!({ "url": "https://x" })).is_none());
}

#[test]
fn an_argument_is_taken_from_whatever_field_the_call_is_keyed_on() {
    assert_eq!(
        permission_argument(&json!({ "url": "https://x" })).as_deref(),
        Some("https://x")
    );
    assert_eq!(
        permission_argument(&json!({ "pattern": "TODO" })).as_deref(),
        Some("TODO")
    );
    assert!(permission_argument(&json!({ "unheard_of": 1 })).is_none());
}

/// A started agent's turn context, as `execute` holds one.
pub(super) async fn started_agent_ctx(runtime: &Arc<Runtime>) -> TurnCtx {
    let (agent, _task) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                start: true,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create agent");
    TurnCtx {
        agent,
        task: None,
        turn: 0,
        turn_usage: Usage::default(),
        base_actor: Actor::user(),
        hard_cancel: CancellationToken::new(),
        recovery_checked_for: None,
        user_reply_pending: false,
        start_handoff: None,
    }
}

mod context;
mod receipt_cost;
mod steering;
mod usage;
