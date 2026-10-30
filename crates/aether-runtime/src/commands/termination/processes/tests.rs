use super::*;
use aether_core::time::now;
use aether_store::Mutation;

async fn idle_agent(runtime: &Runtime, parent: Option<AgentId>) -> AgentId {
    let (agent, _) = runtime
        .create_agent(
            crate::CreateAgentSpec {
                role: "worker".into(),
                parent,
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    agent.id
}

/// Move an idle agent to `status`, through a turn when only a turn can end so.
fn end(runtime: &Runtime, agent: AgentId, status: AgentStatus) {
    let mut row = runtime.store.agent(agent).unwrap().unwrap();
    if matches!(status, AgentStatus::Completed { .. }) {
        row.transition(AgentStatus::Ready, now()).unwrap();
        row.transition(AgentStatus::Running, now()).unwrap();
    }
    row.transition(status, now()).unwrap();
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(row)))
        .unwrap();
}

fn completed() -> AgentStatus {
    AgentStatus::Completed { summary: None }
}

/// An end owns the reaped agents that handed up to it, directly or through
/// other reaped ones; not a live one, nor what a live one holds.
#[tokio::test]
async fn an_end_owns_what_reaped_agents_handed_to_it() {
    let (runtime, _config) = crate::test_runtime(|_| {}).await;
    runtime.shutdown.cancel();
    let root = idle_agent(&runtime, None).await;
    let child = idle_agent(&runtime, Some(root)).await;
    let done = idle_agent(&runtime, Some(child)).await;
    let live = idle_agent(&runtime, Some(child)).await;
    let under_live = idle_agent(&runtime, Some(live)).await;
    let under_done = idle_agent(&runtime, Some(done)).await;
    let stopped = idle_agent(&runtime, Some(child)).await;
    end(&runtime, done, completed());
    end(&runtime, under_live, completed());
    end(&runtime, under_done, completed());
    end(&runtime, stopped, AgentStatus::Failed { error: "x".into() });
    end(&runtime, child, AgentStatus::Failed { error: "x".into() });
    runtime
        .handed_to
        .lock()
        .extend([(done, child), (under_live, live), (under_done, done)]);

    let hold = runtime.hold();
    let owned = runtime.owned_agents(&hold, child);
    assert_eq!(
        owned,
        vec![child, done, under_done],
        "a stopped agent handed nothing"
    );
    assert_eq!(runtime.owned_agents(&hold, live), vec![live, under_live]);
}

/// A failed agent woken after a child handed up past it does not take that
/// child's processes, and the ancestor that took them keeps them meanwhile.
#[tokio::test]
async fn a_hand_up_stays_with_its_owner_when_a_skipped_agent_wakes() {
    let (runtime, _config) = crate::test_runtime(|_| {}).await;
    runtime.shutdown.cancel();
    let grandparent = idle_agent(&runtime, None).await;
    let parent = idle_agent(&runtime, Some(grandparent)).await;
    let child = idle_agent(&runtime, Some(parent)).await;
    end(&runtime, parent, AgentStatus::Failed { error: "x".into() });
    end(&runtime, child, completed());
    let hold = runtime.hold();
    assert_eq!(runtime.handed_up_to(&hold, child), Some(grandparent));
    runtime.handed_to.lock().insert(child, grandparent);
    drop(hold);

    end(&runtime, parent, AgentStatus::Ready);
    let hold = runtime.hold();
    assert_eq!(runtime.owned_agents(&hold, parent), vec![parent]);
    assert_eq!(
        runtime.owned_agents(&hold, grandparent),
        vec![grandparent, child],
        "past the live parent"
    );
}

/// A completed agent hands up to its nearest live ancestor, past ended ones;
/// one that failed, or has none live, hands up nothing.
#[tokio::test]
async fn only_a_completed_agent_with_a_live_ancestor_hands_up() {
    let (runtime, _config) = crate::test_runtime(|_| {}).await;
    runtime.shutdown.cancel();
    let root = idle_agent(&runtime, None).await;
    let middle = idle_agent(&runtime, Some(root)).await;
    let leaf = idle_agent(&runtime, Some(middle)).await;
    let failed = idle_agent(&runtime, Some(root)).await;
    end(&runtime, middle, completed());
    end(&runtime, leaf, completed());
    end(&runtime, failed, AgentStatus::Failed { error: "x".into() });
    let hold = runtime.hold();

    assert_eq!(
        runtime.handed_up_to(&hold, leaf),
        Some(root),
        "past the ended parent"
    );
    assert_eq!(runtime.handed_up_to(&hold, middle), Some(root));
    assert_eq!(
        runtime.handed_up_to(&hold, failed),
        None,
        "a failure stops its own"
    );

    drop(hold);
    end(&runtime, root, completed());
    let hold = runtime.hold();
    assert_eq!(
        runtime.handed_up_to(&hold, leaf),
        None,
        "no live ancestor is left"
    );
    assert_eq!(
        runtime.handed_up_to(&hold, root),
        None,
        "a root has no one to hand to"
    );
}

/// An ended agent whose reap is still to come keeps what is handed to it:
/// its own reap decides, so a sweep never takes what it already handed on.
#[tokio::test]
async fn an_ancestor_awaiting_its_reap_still_owns_what_is_handed_up() {
    let (runtime, _config) = crate::test_runtime(|_| {}).await;
    runtime.shutdown.cancel();
    let grandparent = idle_agent(&runtime, None).await;
    let parent = idle_agent(&runtime, Some(grandparent)).await;
    let child = idle_agent(&runtime, Some(parent)).await;
    end(&runtime, parent, AgentStatus::Failed { error: "x".into() });
    end(&runtime, child, completed());

    let hold = runtime.hold();
    runtime.awaiting_reap.lock().extend([parent, child]);
    assert_eq!(
        runtime.handed_up_to(&hold, child),
        Some(parent),
        "the failed parent's reap is to come"
    );
    runtime.handed_to.lock().insert(child, parent);
    assert_eq!(
        runtime.owned_agents(&hold, parent),
        vec![parent],
        "a child whose reap is to come is not swept"
    );

    runtime.awaiting_reap.lock().remove(&child);
    assert_eq!(runtime.owned_agents(&hold, parent), vec![parent, child]);
    runtime.awaiting_reap.lock().remove(&parent);
    assert_eq!(
        runtime.handed_up_to(&hold, child),
        Some(grandparent),
        "past the parent once it is reaped"
    );
    runtime.awaiting_reap.lock().insert(parent);
    drop(hold);
    assert_eq!(
        runtime.settle_owner(parent),
        None,
        "the owner's own reap reports it"
    );
}

/// What reaches an owner that has since been reaped went on with its own
/// hand-up, past every reaped hop; to no one once a stop took it.
#[tokio::test]
async fn a_late_hand_up_settles_with_the_live_owner_it_would_have_reached() {
    let (runtime, _config) = crate::test_runtime(|_| {}).await;
    runtime.shutdown.cancel();
    let root = idle_agent(&runtime, None).await;
    let upper = idle_agent(&runtime, Some(root)).await;
    let middle = idle_agent(&runtime, Some(upper)).await;
    assert_eq!(runtime.settle_owner(middle), Some(middle));

    end(&runtime, middle, completed());
    end(&runtime, upper, completed());
    runtime
        .handed_to
        .lock()
        .extend([(middle, upper), (upper, root)]);
    assert_eq!(
        runtime.settle_owner(middle),
        Some(root),
        "past two reaped owners"
    );

    end(&runtime, root, AgentStatus::Cancelled { reason: None });
    assert_eq!(runtime.settle_owner(middle), None, "the root holds nothing");
    runtime.handed_to.lock().remove(&upper);
    assert_eq!(runtime.settle_owner(middle), None, "a stop took it");
}

/// A sweep is for one end: an agent woken since, or ended again, is spared,
/// as what that later end left is its own to stop or hand up.
#[tokio::test]
async fn a_sweep_spares_an_agent_that_ended_again_since() {
    let (runtime, _config) = crate::test_runtime(|_| {}).await;
    runtime.shutdown.cancel();
    let parent = idle_agent(&runtime, None).await;
    let agent = idle_agent(&runtime, Some(parent)).await;
    end(&runtime, agent, AgentStatus::Failed { error: "x".into() });
    let ends = runtime.current_ends(&[agent]);
    assert!(runtime.still_in_end(agent, ends.get(&agent)));

    end(&runtime, agent, AgentStatus::Ready);
    assert!(!runtime.still_in_end(agent, ends.get(&agent)), "woken");
    end(&runtime, agent, AgentStatus::Running);
    end(&runtime, agent, AgentStatus::Failed { error: "y".into() });
    assert!(
        !runtime.still_in_end(agent, ends.get(&agent)),
        "ended again"
    );
    assert!(
        !runtime.still_in_end(agent, None),
        "an unread end spares it"
    );
}

struct NoSink;

#[async_trait::async_trait]
impl aether_tools::ArtifactSink for NoSink {
    async fn store(
        &self,
        _kind: aether_core::artifact::ArtifactKind,
        _name: &str,
        _summary: &str,
        _content_type: Option<&str>,
        _bytes: Vec<u8>,
        _agent: AgentId,
        _task: Option<aether_core::id::TaskId>,
    ) -> std::result::Result<aether_core::artifact::Artifact, aether_tools::ToolError> {
        Err(aether_tools::ToolError::Internal("no sink".into()))
    }
}

fn tool_context(agent: AgentId, cwd: &std::path::Path) -> aether_tools::ToolContext {
    aether_tools::ToolContext {
        agent_id: agent,
        task_id: None,
        instance_id: "test-instance".into(),
        scope: aether_core::agent::Scope::default(),
        cwd: cwd.to_path_buf(),
        artifacts: std::sync::Arc::new(NoSink),
        env: Default::default(),
        hidden_env: Vec::new().into(),
        temp_dir: None,
        build_env: Vec::new(),
    }
}

/// An agent woken between its terminal commit and its reap keeps what it
/// runs: the reap decides under the commit hold, which a wake also takes.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_woken_before_its_reap_keeps_its_jobs() {
    let (runtime, _config) = crate::test_runtime(|_| {}).await;
    runtime.shutdown.cancel();
    let dir = tempfile::tempdir().unwrap();
    let agent = idle_agent(&runtime, None).await;
    let jobs = runtime.tools.background().expect("the shell jobs").clone();
    let job = jobs
        .start(agent, "sleep 30", &tool_context(agent, dir.path()))
        .unwrap();
    end(&runtime, agent, AgentStatus::Failed { error: "x".into() });
    end(&runtime, agent, AgentStatus::Ready);

    runtime.reap_ended_agent(PendingReap(agent));

    let held = jobs.list(agent).unwrap();
    assert!(
        held.iter()
            .any(|status| status.id == job.id && status.running),
        "{held:?}"
    );
    assert_eq!(jobs.stop_agent(agent).await, 1);
}

#[test]
fn the_message_names_each_job_and_process_and_the_rest_by_pid() {
    let processes: Vec<AgentProcess> = (2..=(HANDED_UP_LISTED as u32 + 3))
        .map(|pid| AgentProcess {
            pid,
            command: format!("server {pid}"),
        })
        .collect();
    let jobs = vec![("prc_1".to_owned(), "npm run dev".to_owned())];
    let text = hand_up_message(AgentId::new(), &processes, &jobs);
    assert!(text.contains("- prc_1: npm run dev"), "{text}");
    assert!(text.contains("- 2: server 2"), "{text}");
    assert!(
        !text.contains(&format!("server {}", HANDED_UP_LISTED as u32 + 2)),
        "{text}"
    );
    let last = HANDED_UP_LISTED as u32 + 3;
    assert!(
        text.ends_with(&format!("- and 2 more, by pid: {} {last}", last - 1)),
        "{text}"
    );
}
