//! A teaching call's reply against what lands while the model call runs: a
//! pause, cancel, `update_agent` or `set_approvals` committed meanwhile must
//! stand, and a cancel must end the call itself.

use std::sync::atomic::AtomicBool;

use aether_core::agent::{Agent, AgentStatus, ApprovalPolicy};
use aether_core::model::{ModelError, ModelRequest};
use aether_model::{ModelClient, ModelStream};
use aether_runtime::AgentUpdate;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use super::*;

/// How long a test waits for a call to reach or leave its gate.
const PATIENCE: Duration = Duration::from_secs(10);

/// Its first call reports [`incurred`] at once, then holds the rest of the
/// reply until released; it notes whether the call was cancelled or dropped
/// while held. Later calls pass straight through to `inner`.
struct GatedClient {
    inner: Arc<ScriptedClient>,
    used: AtomicBool,
    entered: CancellationToken,
    release: CancellationToken,
    aborted: Arc<AtomicBool>,
}

impl GatedClient {
    fn new(inner: Arc<ScriptedClient>) -> Self {
        Self {
            inner,
            used: AtomicBool::new(false),
            entered: CancellationToken::new(),
            release: CancellationToken::new(),
            aborted: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn wait_entered(&self) {
        tokio::time::timeout(PATIENCE, self.entered.cancelled())
            .await
            .expect("the teaching call reached the model");
    }

    async fn wait_aborted(&self) {
        let start = tokio::time::Instant::now();
        while !self.aborted.load(Ordering::SeqCst) {
            assert!(start.elapsed() < PATIENCE, "the held call was never ended");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// What the gated call has spent when it starts holding.
fn incurred() -> Usage {
    Usage {
        input_tokens: 1_000,
        output_tokens: 7,
        cost_usd: 0.02,
        ..Usage::default()
    }
}

/// Marks the held call aborted if it is dropped before its release.
struct AbortProbe {
    aborted: Arc<AtomicBool>,
    armed: bool,
}

impl Drop for AbortProbe {
    fn drop(&mut self) {
        if self.armed {
            self.aborted.store(true, Ordering::SeqCst);
        }
    }
}

#[async_trait::async_trait]
impl ModelClient for GatedClient {
    fn adapter(&self) -> &str {
        "gated/1"
    }

    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        let rest = self.inner.stream(request, cancel.clone()).await?;
        if self.used.swap(true, Ordering::SeqCst) {
            return Ok(rest);
        }
        let probe = AbortProbe {
            aborted: self.aborted.clone(),
            armed: true,
        };
        let (entered, release) = (self.entered.clone(), self.release.clone());
        let hold = async move {
            // Moved whole: a field capture would drop the probe right here.
            let mut probe = probe;
            entered.cancel();
            tokio::select! {
                () = release.cancelled() => {
                    probe.armed = false;
                    None
                }
                () = cancel.cancelled() => Some(Err(aether_model::http::cancelled())),
            }
        };
        let spent = futures::stream::iter([Ok(StreamEvent::Usage { usage: incurred() })]);
        let held = futures::stream::once(hold).filter_map(futures::future::ready);
        Ok(Box::pin(spent.chain(held).chain(rest)))
    }
}

/// A harness whose first teaching call is in flight and held.
async fn held_call(home: &Home) -> (Harness, Arc<GatedClient>, NoesisSession) {
    let client = Arc::new(ScriptedClient::new());
    client.push(ScriptedClient::tool_call(ASK, capital_question()));
    let gated = Arc::new(GatedClient::new(client.clone()));
    let harness = Harness::open_with(home, client, gated.clone()).await;
    harness.configure(teaching_on(|_| {})).await;
    let created = harness.create(json!({})).await;
    gated.wait_entered().await;
    assert_eq!(harness.client.requests().len(), 1, "one call is in flight");
    (harness, gated, created)
}

fn row(harness: &Harness, agent: AgentId) -> Agent {
    let row = harness.daemon.runtime.store().agent(agent).expect("read");
    row.expect("the agent exists")
}

/// The usage each `agent.usage_updated` recorded for `agent`.
fn usage_records(harness: &Harness, agent: AgentId) -> Vec<Usage> {
    harness
        .events()
        .into_iter()
        .filter(|event| event.agent_id == Some(agent))
        .filter_map(|event| match event.kind {
            EventKind::AgentUsageUpdated { turn_usage, .. } => Some(turn_usage),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pause_during_the_call_survives_its_reply() {
    let home = Home::new();
    let (harness, gated, created) = held_call(&home).await;
    // A failed assertion still frees the held call, so the test cannot hang.
    let _free = gated.release.clone().drop_guard();
    let runtime = &harness.daemon.runtime;
    runtime
        .pause_agent(created.agent, false, Actor::user())
        .expect("paused");

    gated.release.cancel();
    harness.posed(&created.id).await;
    harness.settled().await;
    let agent = row(&harness, created.agent);
    assert!(agent.paused, "the reply kept the pause");
    assert_eq!(agent.stats.turns, 1, "the reply is counted");
    assert_eq!(usage_records(&harness, created.agent).len(), 1);
    assert_eq!(asks(&harness.transcript(created.agent)).len(), 1);
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_during_the_call_ends_it_and_records_only_what_it_spent() {
    let home = Home::new();
    let (harness, gated, created) = held_call(&home).await;
    // A failed assertion still frees the held call, so the test cannot hang.
    let _free = gated.release.clone().drop_guard();
    let runtime = &harness.daemon.runtime;
    runtime
        .cancel_agent(created.agent, Some("stop".into()), Actor::user())
        .expect("cancelled");

    gated.wait_aborted().await;
    harness.settled().await;
    let agent = row(&harness, created.agent);
    assert!(
        matches!(agent.status, AgentStatus::Cancelled { .. }),
        "still cancelled: {:?}",
        agent.status
    );
    let records = usage_records(&harness, created.agent);
    assert_eq!(records.len(), 1, "the spend is recorded once: {records:?}");
    for usage in [records[0], agent.stats.usage] {
        assert_eq!(usage.input_tokens, incurred().input_tokens);
        assert_eq!(usage.output_tokens, incurred().output_tokens);
        assert_eq!(usage.cost_usd, incurred().cost_usd);
    }
    assert_eq!(agent.stats.turns, 1);
    let transcript = harness.transcript(created.agent);
    assert!(
        transcript.iter().all(|m| m.role != MessageRole::Assistant),
        "a cancelled call leaves no reply"
    );
    assert_eq!(
        harness.client.requests().len(),
        1,
        "no call after the cancel"
    );
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_update_and_approvals_set_during_the_call_survive_its_reply() {
    let home = Home::new();
    let (harness, gated, created) = held_call(&home).await;
    // A failed assertion still frees the held call, so the test cannot hang.
    let _free = gated.release.clone().drop_guard();
    let runtime = &harness.daemon.runtime;
    let update = AgentUpdate {
        name: Some("renamed".into()),
        max_turns: Some(7),
        ..AgentUpdate::default()
    };
    runtime
        .update_agent(created.agent, update, Actor::user())
        .expect("updated");
    let gated_policy = ApprovalPolicy::default();
    assert_ne!(row(&harness, created.agent).config.approval, gated_policy);
    runtime
        .set_approvals(created.agent, gated_policy.clone(), false, Actor::user())
        .expect("approvals set");

    gated.release.cancel();
    harness.posed(&created.id).await;
    harness.settled().await;
    let agent = row(&harness, created.agent);
    assert_eq!(agent.name, "renamed");
    assert_eq!(agent.config.max_turns, Some(7));
    assert_eq!(agent.config.approval, gated_policy, "the approvals stand");
    assert_eq!(agent.stats.turns, 1);
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_normal_turn_records_its_usage_turn_and_reply() {
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    client.push(ScriptedClient::tool_call(ASK, capital_question()));
    let harness = Harness::open(&home, client).await;
    harness.configure(teaching_on(|_| {})).await;
    let created = harness.create(json!({})).await;
    harness.posed(&created.id).await;
    harness.settled().await;

    let agent = row(&harness, created.agent);
    let records = usage_records(&harness, created.agent);
    assert_eq!(records.len(), 1, "one call, one record");
    assert_eq!(agent.stats.usage, records[0], "the total is that call");
    assert_eq!(agent.stats.usage.input_tokens, 10, "the scripted reading");
    assert_eq!(agent.stats.turns, 1);
    assert!(!agent.paused && !agent.status.is_terminal());
    let transcript = harness.transcript(created.agent);
    assert_eq!(asks(&transcript).len(), 1, "the reply is in the transcript");
    harness.close().await;
}
