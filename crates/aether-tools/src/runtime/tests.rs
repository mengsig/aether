use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn a_secret_in_structured_output_is_redacted_at_every_depth() {
    // An MCP server picks `structured` freely and it lands in the event
    // log whole, so the redactor has to reach inside it.
    let redactor = Redactor::default();
    let mut value = serde_json::json!({
        "note": "token sk-ant-abcdefghijklmnop",
        "nested": [{"deep": "AKIAIOSFODNN7EXAMPLE"}],
        "count": 3
    });
    super::redact_json(&redactor, &mut value);
    assert_eq!(
        value["note"],
        serde_json::json!("token [REDACTED:anthropic_key]")
    );
    assert_eq!(
        value["nested"][0]["deep"],
        serde_json::json!("[REDACTED:aws_access_key]")
    );
    assert_eq!(
        value["count"],
        serde_json::json!(3),
        "numbers are untouched"
    );
}

#[test]
fn a_secret_in_an_object_key_is_redacted_and_no_field_is_lost() {
    let (first, second) = (
        "fleet-token-0123456789abcdef",
        "other-token-fedcba9876543210",
    );
    let redactor = Redactor::with_known_secrets([first, second]).expect("compiles");
    // Both secrets mask to the same key, which an existing key already
    // holds along with its first numbered suffix.
    let mut value = serde_json::json!({
        "plain": 1,
        "nested": [{
            first: "first",
            second: "second",
            "[REDACTED:known_secret]": "existing",
            "[REDACTED:known_secret] (2)": "numbered"
        }],
    });
    super::redact_json(&redactor, &mut value);
    let text = value.to_string();
    assert!(!text.contains(first) && !text.contains(second), "{text}");
    assert_eq!(value["plain"], 1);
    let nested = value["nested"][0].as_object().expect("an object");
    assert_eq!(nested.len(), 4, "no field is lost: {nested:?}");
    let mut kept: Vec<&str> = nested.values().filter_map(|v| v.as_str()).collect();
    kept.sort_unstable();
    assert_eq!(kept, ["existing", "first", "numbered", "second"]);
}

use aether_core::id::{AgentId, ToolCallId};
use aether_core::model::ToolSpec;
use aether_core::tool::{ResourceKey, ToolOutcome};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::*;
use crate::testing::ctx_in;

/// Records how many executions overlap, so scheduling can be asserted.
struct Probe {
    name: String,
    class: SideEffectClass,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    hold: Duration,
}

#[async_trait]
impl Tool for Probe {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            spec: ToolSpec {
                name: self.name.clone(),
                description: "probe".into(),
                input_schema: json!({"type": "object"}),
                strict: false,
            },
            side_effect: self.class,
            needs_network: false,
        }
    }

    fn resources(&self, input: &Value, _ctx: &ToolContext) -> Vec<ResourceKey> {
        input["file"]
            .as_str()
            .map(ResourceKey::file)
            .into_iter()
            .collect()
    }

    async fn execute(
        &self,
        _input: Value,
        _ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        let now_active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now_active, Ordering::SeqCst);
        tokio::select! {
            _ = tokio::time::sleep(self.hold) => {}
            _ = cancel.cancelled() => {
                self.active.fetch_sub(1, Ordering::SeqCst);
                return Err(ToolError::Cancelled);
            }
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(ToolExecution::Outcome(ToolOutcome::ok(
            "sk-ant-secretsecretsecret done",
        )))
    }
}

fn probe(
    name: &str,
    class: SideEffectClass,
    peak: &Arc<AtomicUsize>,
    hold_ms: u64,
) -> Arc<dyn Tool> {
    Arc::new(Probe {
        name: name.into(),
        class,
        active: Arc::new(AtomicUsize::new(0)),
        peak: peak.clone(),
        hold: Duration::from_millis(hold_ms),
    })
}

fn call(name: &str, input: Value) -> ToolCall {
    ToolCall {
        id: ToolCallId::new(),
        provider_call_id: "p".into(),
        name: name.into(),
        input,
        agent_id: AgentId::new(),
        task_id: None,
        invocation_id: None,
        resources: Vec::new(),
        idempotency_key: None,
        requested_at: now(),
    }
}

#[test]
fn symlink_aliases_share_prepared_file_leases() {
    use crate::builtin::{edit_file::EditFileTool, write_file::WriteFileTool};
    use aether_core::lease::ResourceHolding;

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("target"), "original").unwrap();
    std::os::unix::fs::symlink("target", dir.path().join("alias_a")).unwrap();
    std::os::unix::fs::symlink("target", dir.path().join("alias_b")).unwrap();
    let ctx = ctx_in(dir.path());
    let mut writer = call("write_file", json!({"path": "alias_a", "content": "new"}));
    let mut editor = call(
        "edit_file",
        json!({"path": "alias_b", "old_string": "original", "new_string": "edited"}),
    );
    ToolRuntime::prepare(&mut writer, &WriteFileTool, &ctx);
    ToolRuntime::prepare(&mut editor, &EditFileTool::default(), &ctx);
    let holding = ResourceHolding {
        agent: writer.agent_id,
        keys: writer.resources,
        since: now(),
        quarantined: false,
    };
    assert_ne!(holding.agent, editor.agent_id);
    assert!(
        editor
            .resources
            .iter()
            .any(|key| holding.conflict_with(key).is_some()),
        "two agents writing aliases of the same file must conflict"
    );
}

#[test]
fn symlinked_parents_share_leases_before_the_file_exists() {
    use crate::builtin::write_file::WriteFileTool;

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    std::os::unix::fs::symlink("real", dir.path().join("alias")).unwrap();
    let ctx = ctx_in(dir.path());
    let mut alias = call(
        "write_file",
        json!({"path": "alias/new/file", "content": "a"}),
    );
    let mut direct = call(
        "write_file",
        json!({"path": "real/new/file", "content": "b"}),
    );
    ToolRuntime::prepare(&mut alias, &WriteFileTool, &ctx);
    ToolRuntime::prepare(&mut direct, &WriteFileTool, &ctx);
    assert!(
        alias
            .resources
            .iter()
            .any(|a| direct.resources.iter().any(|b| a.conflicts_with(b)))
    );
    assert!(
        alias
            .resources
            .contains(&ResourceKey::file(dir.path().join("alias/new/file"))),
        "the declared path remains available to resource rules"
    );
}

#[test]
fn symlink_parent_traversal_leases_the_execution_target() {
    use crate::builtin::write_file::WriteFileTool;

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("real/nested")).unwrap();
    std::os::unix::fs::symlink("real/nested", dir.path().join("alias")).unwrap();
    let ctx = ctx_in(dir.path());
    let mut alias = call(
        "write_file",
        json!({"path": "alias/../new", "content": "a"}),
    );
    let mut direct = call("write_file", json!({"path": "real/new", "content": "b"}));
    ToolRuntime::prepare(&mut alias, &WriteFileTool, &ctx);
    ToolRuntime::prepare(&mut direct, &WriteFileTool, &ctx);
    assert!(
        alias
            .resources
            .iter()
            .any(|a| direct.resources.iter().any(|b| a.conflicts_with(b)))
    );
}

#[test]
fn dangling_symlink_aliases_share_leases_with_the_missing_target() {
    use crate::builtin::write_file::WriteFileTool;

    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("missing/file", dir.path().join("alias_a")).unwrap();
    std::os::unix::fs::symlink("missing/file", dir.path().join("alias_b")).unwrap();
    let ctx = ctx_in(dir.path());
    let target = ResourceKey::file(dir.path().canonicalize().unwrap().join("missing/file"));
    for path in ["alias_a", "alias_b", "missing/file"] {
        let resources = WriteFileTool.resources(&json!({"path": path}), &ctx);
        assert!(resources.contains(&target), "{path}: {resources:?}");
    }
}

#[test]
fn shell_cwd_aliases_conflict_with_direct_file_leases() {
    use crate::builtin::write_file::WriteFileTool;

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    std::os::unix::fs::symlink("real", dir.path().join("alias")).unwrap();
    let mut ctx = ctx_in(dir.path());
    let resources = WriteFileTool.resources(&json!({"path": "real/new"}), &ctx);
    ctx.cwd = dir.path().join("alias");
    let shell = crate::shell_resources(&ctx);
    assert!(
        shell
            .iter()
            .any(|a| resources.iter().any(|b| a.conflicts_with(b)))
    );
}

#[test]
fn relative_shell_cwd_aliases_conflict_with_file_leases() {
    use crate::builtin::write_file::WriteFileTool;

    let cwd = std::env::current_dir().unwrap();
    let dir = tempfile::tempdir_in(&cwd).unwrap();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    std::os::unix::fs::symlink("real", dir.path().join("alias")).unwrap();
    let mut ctx = ctx_in(dir.path());
    ctx.cwd = std::path::Path::new(".")
        .join(dir.path().strip_prefix(&cwd).unwrap())
        .join("alias");
    ctx.scope.workspace_root = Some(ctx.cwd.clone());
    let resources = WriteFileTool.resources(&json!({"path": "new"}), &ctx);
    let shell = crate::shell_resources(&ctx);
    assert!(
        shell
            .iter()
            .any(|a| resources.iter().any(|b| a.conflicts_with(b))),
        "relative cwd keys must resolve once: {shell:?} vs {resources:?}"
    );
    assert!(shell.contains(&ResourceKey::repo(
        dir.path().canonicalize().unwrap().join("real")
    )));
    assert!(
        shell.contains(&ResourceKey::repo(&ctx.cwd)),
        "existing rule keys must remain available"
    );
}

#[test]
fn a_dot_shell_cwd_does_not_claim_every_repository() {
    use crate::builtin::write_file::WriteFileTool;

    let cwd = std::env::current_dir().unwrap();
    let dir = tempfile::tempdir_in(&cwd).unwrap();
    let mut ctx = ctx_in(&cwd);
    ctx.cwd = std::path::PathBuf::from(".");
    let shell = crate::shell_resources(&ctx);
    let path = dir.path().strip_prefix(&cwd).unwrap().join("new");
    let resources = WriteFileTool.resources(&json!({"path": path}), &ctx);
    assert!(
        shell
            .iter()
            .any(|a| resources.iter().any(|b| a.conflicts_with(b)))
    );
    assert!(
        shell.contains(&ResourceKey::repo(".")),
        "retain the existing rule key"
    );
    let outside = ResourceKey::file(cwd.parent().unwrap().join("other/new"));
    assert!(
        !shell.iter().any(|key| key.conflicts_with(&outside)),
        "a dot cwd must not claim other trees: {shell:?}"
    );
}

#[tokio::test]
async fn read_only_calls_run_concurrently() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx_in(dir.path()));
    let peak = Arc::new(AtomicUsize::new(0));
    let shared_active = Arc::new(AtomicUsize::new(0));
    let mk = |n: &str| {
        Arc::new(Probe {
            name: n.into(),
            class: SideEffectClass::ReadOnly,
            active: shared_active.clone(),
            peak: peak.clone(),
            hold: Duration::from_millis(80),
        }) as Arc<dyn Tool>
    };
    let calls = vec![
        (call("a", json!({"file": "/r/x"})), mk("a")),
        (call("b", json!({"file": "/r/x"})), mk("b")),
    ];
    let results = ToolRuntime::new()
        .execute_batch(
            calls,
            ctx,
            CancellationToken::new(),
            CancellationToken::new(),
            &ExecutionPolicy::default(),
        )
        .await;
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.result.is_ok()));
    assert_eq!(peak.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn conflicting_mutations_serialize_and_disjoint_ones_overlap() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx_in(dir.path()));
    let shared_active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mk = |n: &str| {
        Arc::new(Probe {
            name: n.into(),
            class: SideEffectClass::Mutation,
            active: shared_active.clone(),
            peak: peak.clone(),
            hold: Duration::from_millis(60),
        }) as Arc<dyn Tool>
    };
    let calls = vec![
        (call("w1", json!({"file": "/r/same"})), mk("w1")),
        (call("w2", json!({"file": "/r/same"})), mk("w2")),
    ];
    let rt = ToolRuntime::new();
    let results = rt
        .execute_batch(
            calls,
            ctx.clone(),
            CancellationToken::new(),
            CancellationToken::new(),
            &ExecutionPolicy::default(),
        )
        .await;
    assert_eq!(results.len(), 2);
    assert_eq!(
        peak.load(Ordering::SeqCst),
        1,
        "same-file mutations must not overlap"
    );
    assert!(results[0].finished_at <= results[1].started_at);

    peak.store(0, Ordering::SeqCst);
    let calls = vec![
        (call("w1", json!({"file": "/r/a"})), mk("w1")),
        (call("w2", json!({"file": "/r/b"})), mk("w2")),
    ];
    let _ = rt
        .execute_batch(
            calls,
            ctx,
            CancellationToken::new(),
            CancellationToken::new(),
            &ExecutionPolicy::default(),
        )
        .await;
    assert_eq!(
        peak.load(Ordering::SeqCst),
        2,
        "disjoint mutations may overlap"
    );
}

#[tokio::test]
async fn results_keep_input_order_and_are_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx_in(dir.path()));
    let peak = Arc::new(AtomicUsize::new(0));
    let slow = probe("slow", SideEffectClass::ReadOnly, &peak, 80);
    let fast = probe("fast", SideEffectClass::ReadOnly, &peak, 5);
    let calls = vec![
        (call("slow", json!({})), slow),
        (call("fast", json!({})), fast),
    ];
    let results = ToolRuntime::new()
        .execute_batch(
            calls,
            ctx,
            CancellationToken::new(),
            CancellationToken::new(),
            &ExecutionPolicy::default(),
        )
        .await;
    assert_eq!(results[0].call.name, "slow");
    assert_eq!(results[1].call.name, "fast");
    match &results[0].result {
        Ok(ToolExecution::Outcome(o)) => assert_eq!(o.content, "[REDACTED:anthropic_key] done"),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn timeout_and_cancellation_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx_in(dir.path()));
    let peak = Arc::new(AtomicUsize::new(0));
    let policy = ExecutionPolicy {
        default_timeout: Duration::from_millis(30),
        ..Default::default()
    };
    let calls = vec![(
        call("t", json!({})),
        probe("t", SideEffectClass::ReadOnly, &peak, 5000),
    )];
    let results = ToolRuntime::new()
        .execute_batch(
            calls,
            ctx.clone(),
            CancellationToken::new(),
            CancellationToken::new(),
            &policy,
        )
        .await;
    assert!(matches!(results[0].result, Err(ToolError::Timeout)));

    let interrupt = CancellationToken::new();
    let calls = vec![(
        call("c", json!({})),
        probe("c", SideEffectClass::ReadOnly, &peak, 5000),
    )];
    let c2 = interrupt.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        c2.cancel();
    });
    let results = ToolRuntime::new()
        .execute_batch(
            calls,
            ctx,
            interrupt,
            CancellationToken::new(),
            &ExecutionPolicy::default(),
        )
        .await;
    assert!(matches!(results[0].result, Err(ToolError::Cancelled)));
}

/// A harness-action stand-in: instant, no yield points, records whether
/// it ran. `interruptible` mirrors `Tool::interruptible`.
struct Decisive {
    ran: Arc<AtomicUsize>,
    interruptible: bool,
}

#[async_trait]
impl Tool for Decisive {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            spec: ToolSpec {
                name: "decisive".into(),
                description: "decisive".into(),
                input_schema: json!({"type": "object"}),
                strict: false,
            },
            side_effect: SideEffectClass::ReadOnly,
            needs_network: false,
        }
    }

    fn resources(&self, _input: &Value, _ctx: &ToolContext) -> Vec<ResourceKey> {
        Vec::new()
    }

    fn interruptible(&self) -> bool {
        self.interruptible
    }

    async fn execute(
        &self,
        _input: Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        Ok(ToolExecution::Outcome(ToolOutcome::ok("done")))
    }
}

#[tokio::test]
async fn a_decided_harness_action_still_runs_once_the_turn_is_already_interrupted() {
    // A child a model already asked delegate() to create must
    // not be dropped just because a mere interrupt was already set by
    // the time the call was picked up - see `Tool::interruptible`.
    // Before that flag existed every call raced the token the same way,
    // and this assertion failed.
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx_in(dir.path()));
    let interrupt = CancellationToken::new();
    interrupt.cancel();
    let cancel = CancellationToken::new();

    let ran = Arc::new(AtomicUsize::new(0));
    let calls = vec![(
        call("d", json!({})),
        Arc::new(Decisive {
            ran: ran.clone(),
            interruptible: false,
        }) as Arc<dyn Tool>,
    )];
    let results = ToolRuntime::new()
        .execute_batch(
            calls,
            ctx.clone(),
            interrupt.clone(),
            cancel.clone(),
            &ExecutionPolicy::default(),
        )
        .await;
    assert!(
        matches!(results[0].result, Ok(ToolExecution::Outcome(_))),
        "a non-interruptible call must run even against an already-interrupted token: {:?}",
        results[0].result
    );
    assert_eq!(ran.load(Ordering::SeqCst), 1);

    // An ordinary (interruptible) tool is unaffected: still skipped.
    let ran2 = Arc::new(AtomicUsize::new(0));
    let calls = vec![(
        call("i", json!({})),
        Arc::new(Decisive {
            ran: ran2.clone(),
            interruptible: true,
        }) as Arc<dyn Tool>,
    )];
    let results = ToolRuntime::new()
        .execute_batch(calls, ctx, interrupt, cancel, &ExecutionPolicy::default())
        .await;
    assert!(matches!(results[0].result, Err(ToolError::Cancelled)));
    assert_eq!(ran2.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_decided_harness_action_does_not_run_once_the_agent_is_hard_cancelled() {
    // The other half of the cancel-orphan fix: unlike a mere
    // interrupt, a hard cancel of the whole agent must stop even a
    // decided, non-interruptible harness action from running -
    // `Runtime::cancel_live_children` is the safety net for what slips
    // past this, not the only guard.
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx_in(dir.path()));
    let interrupt = CancellationToken::new();
    let cancel = CancellationToken::new();
    cancel.cancel();

    let ran = Arc::new(AtomicUsize::new(0));
    let calls = vec![(
        call("d", json!({})),
        Arc::new(Decisive {
            ran: ran.clone(),
            interruptible: false,
        }) as Arc<dyn Tool>,
    )];
    let results = ToolRuntime::new()
        .execute_batch(calls, ctx, interrupt, cancel, &ExecutionPolicy::default())
        .await;
    assert!(
        matches!(results[0].result, Err(ToolError::Cancelled)),
        "a hard cancel must stop a decided action too: {:?}",
        results[0].result
    );
    assert_eq!(ran.load(Ordering::SeqCst), 0);
}

mod retries;

#[tokio::test]
async fn prepare_keys_only_the_classes_whose_outcome_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let peak = Arc::new(AtomicUsize::new(0));
    let mut read = call("r", json!({"file": "/r/x"}));
    ToolRuntime::prepare(
        &mut read,
        probe("r", SideEffectClass::ReadOnly, &peak, 0).as_ref(),
        &ctx,
    );
    assert!(read.idempotency_key.is_none());
    let mut write = call("w", json!({"file": "/r/x"}));
    ToolRuntime::prepare(
        &mut write,
        probe("w", SideEffectClass::Mutation, &peak, 0).as_ref(),
        &ctx,
    );
    assert!(write.idempotency_key.is_some());
}

#[test]
fn approval_follows_policy() {
    let d = ToolDescriptor {
        spec: ToolSpec {
            name: "x".into(),
            description: String::new(),
            input_schema: json!({}),
            strict: false,
        },
        side_effect: SideEffectClass::External,
        needs_network: true,
    };
    assert!(requires_approval(&d, &ApprovalPolicy::default()));
    assert!(!requires_approval(&d, &ApprovalPolicy::none()));
}
