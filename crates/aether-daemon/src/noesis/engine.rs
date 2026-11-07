//! The teaching loop's [`Engine`] in the daemon: each Noesis agent is an
//! Aether agent in the `noesis` run with subsystem `noesis` and no Aether
//! tools. Its inputs and replies are `agent.message` transcript entries, so
//! the runtime's seal writes every sealed field as `"<sealed>"`. Each model
//! call is admitted by the service budget and passes the runtime's one
//! [`NoesisCharge::before_invocation`] hook before it is sent.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use aether_core::agent::{Agent, AgentConfig};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, RunId};
use aether_core::model::{
    ContentBlock, CostGap, Message, MessageRole, ModelError, ModelErrorClass, ModelRef,
    ModelRequest, ModelResponse, StreamAccumulator, StreamEvent, SystemBlock, ToolResultBlock,
    ToolSpec, Usage,
};
use aether_core::tool::ToolExposure;
use aether_model::{ModelClient, ModelStream, Models};
use aether_noesis::budget::{Billing, INTERACTIVE_BUDGET_SETTING, Spend};
use aether_noesis::prompts;
use aether_noesis::teach::engine::ModelInfo;
use aether_noesis::teach::{
    AgentSpec, Engine, EngineError, Input, Sealer, Step, ToolCall, ToolResult, WITHHELD,
    agent_for_role,
};
use aether_noesis::tools::{self, ToolName};
use aether_runtime::noesis_guard::{NoesisBudget, NoesisCharge, NoesisRefusal, lane_of};
use aether_runtime::{BusMessage, CreateAgentSpec, NoesisStepStream, Runtime};
use aether_store::Commit;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use super::budget::{ServiceBudget, runs_on_engine, spend_of};
use super::engine_step::{self, EngineStepClient};

/// What every engine of one running service shares.
pub(super) struct EngineHost {
    pub runtime: Arc<Runtime>,
    pub models: Arc<Models>,
    pub budget: Arc<ServiceBudget>,
    /// Replaces an API provider's client in tests, as the runtime's override
    /// does; an engine model's steps always go to its engine.
    pub client: Option<Arc<dyn ModelClient>>,
    /// Cancelled when the service stops, ending a model call in flight.
    pub cancel: CancellationToken,
    pub prompt_overrides: Option<PathBuf>,
    /// The one run every Noesis agent is created in.
    pub run: RunId,
}

/// Every listed model, ready only when a step on it could run now: an
/// engine detected and launchable with its CLI signed in, an API provider
/// with credentials, or any API model while `test_client` stands in.
pub(super) async fn model_offers(
    runtime: &Runtime,
    models: &Models,
    test_client: bool,
) -> Vec<ModelInfo> {
    let mut signed_in: HashMap<String, bool> = HashMap::new();
    let mut offers = Vec::new();
    for info in models.list() {
        let provider = info.model.provider.clone();
        let engine = runs_on_engine(models, &info.model);
        if !signed_in.contains_key(&provider) {
            let status = models.auth_status(&provider).await;
            signed_in.insert(provider.clone(), status.configured);
        }
        let credentials = signed_in.get(&provider).copied().unwrap_or(false);
        let ready = if engine {
            credentials && runtime.engine_launchable(&info.model) == Some(true)
        } else {
            credentials || test_client
        };
        offers.push(ModelInfo {
            id: info.model.to_string(),
            billing: if engine {
                Billing::Subscription
            } else {
                Billing::Api
            },
            ready,
        });
    }
    debug_assert!(offers.iter().all(|offer| !offer.id.is_empty()));
    offers
}

impl EngineHost {
    fn is_engine(&self, model: &ModelRef) -> bool {
        runs_on_engine(&self.models, model)
    }

    /// The client a step of `agent` is sent to: its engine for an engine
    /// model, else the test client or the provider's own.
    async fn client_for(&self, agent: &Agent) -> Result<Arc<dyn ModelClient>, EngineError> {
        if self.is_engine(&agent.model) {
            let inner = self
                .runtime
                .noesis_engine_client(agent)
                .await
                .map_err(not_sent)?;
            return Ok(Arc::new(EngineStepClient::new(inner)));
        }
        match &self.client {
            Some(client) => Ok(client.clone()),
            None => self.models.client(&agent.model).await.map_err(not_sent),
        }
    }
}

/// The engine for one teaching call; cheap, it borrows the host. Like a
/// runtime turn, an agent's steps in one call share one budget admission,
/// settled when another agent steps or the engine is dropped.
pub(super) struct AetherEngine<'a> {
    host: &'a EngineHost,
    charge: Option<OpenCharge>,
}

/// One agent's admission and what its steps have spent under it.
struct OpenCharge {
    agent: AgentId,
    charge: Box<dyn NoesisCharge>,
    spent: Usage,
}

impl<'a> AetherEngine<'a> {
    pub(super) fn new(host: &'a EngineHost) -> Self {
        Self { host, charge: None }
    }

    /// The admission `agent` steps under: kept from its last step, else new.
    fn admitted(&mut self, agent: &Agent) -> Result<&mut OpenCharge, EngineError> {
        if self
            .charge
            .as_ref()
            .is_some_and(|open| open.agent != agent.id)
        {
            self.settle();
        }
        if self.charge.is_none() {
            let charge = self
                .host
                .budget
                .admit(agent, lane_of(agent.role.as_str()))
                .map_err(|refusal| self.refused(agent, refusal))?;
            self.charge = Some(OpenCharge {
                agent: agent.id,
                charge,
                spent: Usage::default(),
            });
        }
        let open = self.charge.as_mut();
        debug_assert!(open.as_ref().is_some_and(|open| open.agent == agent.id));
        open.ok_or_else(|| not_sent("no admission"))
    }

    fn settle(&mut self) {
        if let Some(open) = self.charge.take() {
            open.charge.settle(&open.spent);
        }
    }
}

impl Drop for AetherEngine<'_> {
    fn drop(&mut self) {
        self.settle();
    }
}

/// The run of the oldest Noesis agent, or a fresh one when there is none.
pub(super) fn noesis_run(runtime: &Runtime) -> aether_store::Result<RunId> {
    let agents = runtime.store().agents(&aether_store::AgentFilter {
        exclude_archived: false,
        ..aether_store::AgentFilter::default()
    })?;
    for agent in agents
        .iter()
        .filter(|a| agent_for_role(a.role.as_str()).is_some())
    {
        if runtime.store().agent_subsystem(agent.id)?.as_deref() == Some("noesis") {
            return Ok(agent.run);
        }
    }
    Ok(RunId::new())
}

/// A Noesis tool's model-facing name: providers accept no `.` in names.
fn wire_name(name: &str) -> String {
    name.replace('.', "_")
}

/// The Noesis tool name a model-facing name stands for, else it unchanged.
fn tool_name(wire: &str) -> String {
    ToolName::ALL
        .into_iter()
        .map(ToolName::as_str)
        .find(|name| wire_name(name) == wire)
        .unwrap_or(wire)
        .to_string()
}

fn failed(error: impl std::fmt::Display) -> EngineError {
    EngineError::Failed(error.to_string())
}

fn not_sent(error: impl std::fmt::Display) -> EngineError {
    EngineError::NotSent(error.to_string())
}

impl AetherEngine<'_> {
    fn agent(&self, agent: &str) -> Result<Agent, EngineError> {
        let id = AgentId::from_str(agent).map_err(|_| not_sent(format!("agent id `{agent}`")))?;
        let row = self.host.runtime.store().agent(id).map_err(not_sent)?;
        let row = row.ok_or_else(|| not_sent(format!("no agent `{agent}`")))?;
        if agent_for_role(row.role.as_str()).is_none() {
            return Err(not_sent(format!("`{agent}` is not a Noesis agent")));
        }
        Ok(row)
    }

    fn transcript(&self, agent: AgentId) -> Result<Vec<Message>, EngineError> {
        let entries = self
            .host
            .runtime
            .store()
            .transcript(agent, 0)
            .map_err(not_sent)?;
        Ok(entries.into_iter().map(|entry| entry.message).collect())
    }

    /// Append one transcript entry for `agent`; the runtime seals it.
    fn record(
        &self,
        agent: &Agent,
        role: MessageRole,
        content: Vec<ContentBlock>,
    ) -> Result<u64, EngineError> {
        debug_assert!(!content.is_empty(), "an entry carries content");
        let mut commit = Commit::new();
        commit.push_event(message_draft(agent, role, content));
        let events = self.host.runtime.apply(commit).map_err(not_sent)?;
        debug_assert_eq!(events.len(), 1, "one message event per entry");
        let message = events
            .first()
            .ok_or_else(|| not_sent("the transcript entry was not committed"))?;
        Ok(message.seq)
    }

    /// The blocks `input` adds to the transcript; results the last reply's
    /// calls already have are dropped, so a repeated delivery adds nothing.
    fn blocks_of(&self, history: &[Message], input: Input) -> Vec<ContentBlock> {
        let (calls, answered) = last_calls(history);
        match input {
            Input::Learner(text) | Input::Opening(text) | Input::Handoff(text) => {
                vec![ContentBlock::Text { text }]
            }
            Input::ToolResults(results) => results
                .into_iter()
                .map(|result| ToolResult {
                    call: recorded_id(&calls, result.call),
                    ..result
                })
                .filter(|result| !answered.contains(&result.call))
                .map(result_block)
                .collect(),
            Input::Answer(payload) => {
                let ask = wire_name(ToolName::Ask.as_str());
                if !calls.iter().any(|(_, name)| *name == ask) {
                    // No parked call to answer: the answer still reaches the agent.
                    return vec![ContentBlock::Text {
                        text: format!("The learner answered: {payload}"),
                    }];
                }
                let open = open_asks(&calls, &answered);
                debug_assert!(open.len() <= 1, "one question is open at a time");
                open.into_iter()
                    .map(|call| {
                        result_block(ToolResult {
                            call,
                            content: payload.clone(),
                            is_error: false,
                        })
                    })
                    .collect()
            }
        }
    }

    /// One model call over the agent's transcript, ended if the service
    /// stops or the agent ends (a cancel) while it runs. Its text streams
    /// live only when `streamed` is given, which gathers what was shown.
    fn invoke(
        &self,
        agent: &Agent,
        request: ModelRequest,
        mut streamed: Option<&mut String>,
    ) -> Result<CallEnd, EngineError> {
        let host = self.host;
        let handle = tokio::runtime::Handle::current();
        let cancel = host.cancel.child_token();
        let turn = step_number(&request.messages);
        tokio::task::block_in_place(|| {
            handle.block_on(async {
                let client = host.client_for(agent).await?;
                let mut live = NoesisStepStream::start(host.runtime.clone(), agent.id, turn);
                let ended = agent_ended(&host.runtime, agent.id);
                tokio::pin!(ended);
                let opening = client.stream(request, cancel.clone());
                tokio::pin!(opening);
                let stream = tokio::select! {
                    stream = &mut opening => stream.map_err(not_sent)?,
                    () = &mut ended => {
                        cancel.cancel();
                        return Ok(CallEnd::AgentEnded(settled_usage(opening).await));
                    }
                    () = cancel.cancelled() => {
                        return Ok(stopped(settled_usage(opening).await));
                    }
                };
                let mut on_text = |text: &str| {
                    if let Some(shown) = streamed.as_deref_mut() {
                        live.text(text);
                        shown.push_str(text);
                    }
                };
                read_reply(stream, &cancel, ended, &mut on_text).await
            })
        })
    }

    /// Record the reply and its usage, so the day's spend survives a restart.
    /// Only the agent's stats are written, over the row as it is now, and
    /// its blocks as `live` and the `step`'s own calls seal them.
    fn record_reply(
        &self,
        agent: AgentId,
        response: &ModelResponse,
        step: &Step,
        live: &Sealer,
    ) -> Result<(), EngineError> {
        let content = transcript_content(response, live.with_step(step).as_ref());
        let row = self
            .host
            .runtime
            .record_noesis_call(agent, response.usage, |row, commit| {
                // An agent ended meanwhile acts on no reply: only the spend lands.
                if !content.is_empty() && !row.status.is_terminal() {
                    commit.push_event(message_draft(row, MessageRole::Assistant, content));
                }
            })
            .map_err(failed)?;
        if row.status.is_terminal() {
            return Err(failed(format!("agent {agent} is {}", row.status.kind())));
        }
        Ok(())
    }

    /// The agent ended mid-call: what the provider reported spending so far
    /// is charged and recorded, as the runtime records a cancelled call.
    fn record_cut_short(&mut self, agent: AgentId, mut spent: Usage) -> EngineError {
        spent.mark_cost_gap(CostGap::Interrupted);
        if let Some(open) = self.charge.as_mut() {
            debug_assert_eq!(open.agent, agent, "the call ran under its own admission");
            open.spent += spent;
        }
        match self
            .host
            .runtime
            .record_noesis_call(agent, spent, |_, _| {})
        {
            Ok(row) => failed(format!(
                "agent {agent} became {} during its model call",
                row.status.kind()
            )),
            Err(error) => failed(format!(
                "agent {agent} ended during its model call, and its spend was not recorded: {error}"
            )),
        }
    }

    /// The reply failed mid-stream: what the provider reported spending is
    /// charged and recorded, so a failed step still counts against the budget.
    fn record_broken(&mut self, agent: AgentId, mut spent: Usage, error: String) -> EngineError {
        debug_assert!(!error.is_empty(), "a broken reply says why");
        spent.mark_cost_gap(CostGap::Interrupted);
        match self.charge_spent(agent, spent) {
            Ok(()) => failed(error),
            Err(record) => failed(format!(
                "{error}; and the call's spend was not recorded: {record}"
            )),
        }
    }

    /// Charge a call that produced no reply to keep, and record its usage.
    fn charge_spent(&mut self, agent: AgentId, spent: Usage) -> Result<(), String> {
        let open = self.charge.as_mut();
        assert!(
            open.as_ref().is_some_and(|open| open.agent == agent),
            "the call ran under its own admission"
        );
        if let Some(open) = open {
            open.spent += spent;
        }
        let recorded = self
            .host
            .runtime
            .record_noesis_call(agent, spent, |_, _| {});
        recorded.map(|_| ()).map_err(|error| error.to_string())
    }

    /// Refuse a call the agent's admission does not cover, given its spend
    /// so far; asked again before each attempt of a step.
    fn gate(&mut self, row: &Agent) -> Result<(), EngineError> {
        let open = self.admitted(row)?;
        debug_assert_eq!(open.agent, row.id, "the admission is the agent's");
        let refused = open
            .charge
            .refuses_model(&row.model)
            .or_else(|| open.charge.before_invocation(&open.spent));
        let Some(reason) = refused else {
            return Ok(());
        };
        // A spent budget is told apart: the session records it as its stall.
        Err(match self.host.budget.spent(lane_of(row.role.as_str())) {
            Some(spent) => EngineError::BudgetSpent(spent),
            None => not_sent(reason),
        })
    }

    /// The error of a refused admission; a spent budget names its setting.
    fn refused(&self, agent: &Agent, refusal: NoesisRefusal) -> EngineError {
        match refusal {
            NoesisRefusal::Busy(reason) => not_sent(reason),
            NoesisRefusal::Exhausted(reason) => EngineError::BudgetSpent(
                self.host
                    .budget
                    .spent(lane_of(agent.role.as_str()))
                    .unwrap_or_else(|| {
                        format!("{reason}; raise {INTERACTIVE_BUDGET_SETTING} to continue today")
                    }),
            ),
        }
    }

    fn request(&self, agent: &Agent, history: Vec<Message>) -> Result<ModelRequest, EngineError> {
        let Some(kind) = agent_for_role(agent.role.as_str()) else {
            return Err(not_sent("not a Noesis role"));
        };
        let prompt =
            prompts::compose(kind, self.host.prompt_overrides.as_deref()).map_err(not_sent)?;
        let mut request = ModelRequest::new(agent.model.clone(), merged(history));
        request.system = vec![SystemBlock {
            text: prompt.text,
            cache: true,
        }];
        request.tools = tools::definitions(kind)
            .into_iter()
            .map(|def| ToolSpec {
                name: wire_name(def.name),
                description: def.description.to_string(),
                input_schema: def.input_schema,
                strict: false,
            })
            .collect();
        debug_assert!(request.tools.iter().all(|tool| !tool.name.contains('.')));
        Ok(request)
    }
}

impl Engine for AetherEngine<'_> {
    fn models(&self) -> Vec<ModelInfo> {
        let host = self.host;
        let handle = tokio::runtime::Handle::current();
        let offers = model_offers(&host.runtime, &host.models, host.client.is_some());
        tokio::task::block_in_place(|| handle.block_on(offers))
    }

    fn spawn(&mut self, spec: AgentSpec) -> Result<String, EngineError> {
        debug_assert!(spec.role.starts_with("noesis.") && spec.subsystem == "noesis");
        let model = ModelRef::parse(&spec.model)
            .ok_or_else(|| not_sent(format!("model `{}`", spec.model)))?;
        let config = AgentConfig {
            tools: ToolExposure::Only {
                names: Default::default(),
            },
            ..AgentConfig::default()
        };
        let create = CreateAgentSpec {
            role: spec.role.clone(),
            name: Some(spec.role),
            model: Some(model),
            run: Some(self.host.run),
            start: false,
            subsystem: Some(spec.subsystem.to_string()),
            config: Some(config),
            ..CreateAgentSpec::default()
        };
        let (agent, task) = self
            .host
            .runtime
            .create_agent(create, Actor::System)
            .map_err(not_sent)?;
        assert!(task.is_none(), "a Noesis agent is created without a task");
        Ok(agent.id.to_string())
    }

    fn deliver(&mut self, agent: &str, input: Input) -> Result<Option<u64>, EngineError> {
        let row = self.agent(agent)?;
        let history = self.transcript(row.id)?;
        let blocks = self.blocks_of(&history, input);
        if blocks.is_empty() {
            return Ok(None);
        }
        self.record(&row, MessageRole::User, blocks).map(Some)
    }

    fn estimate(&self, _agent: &str) -> Spend {
        Spend::default()
    }

    fn charges_own_calls(&self) -> bool {
        true
    }

    fn step(&mut self, agent: &str, live: &Sealer) -> Result<Step, EngineError> {
        let row = self.agent(agent)?;
        if row.status.is_terminal() {
            return Err(not_sent(format!("agent {agent} is {}", row.status.kind())));
        }
        let mut request = self.request(&row, self.transcript(row.id)?)?;
        let mut attempt = 1;
        // Text a retried attempt streamed was shown, so the turn's pose check reads it.
        let mut shown_before = String::new();
        let response = loop {
            self.gate(&row)?;
            let mut streamed = String::new();
            let stream = live.is_empty().then_some(&mut streamed);
            match self.invoke(&row, request.clone(), stream)? {
                CallEnd::Replied(response) => break response,
                CallEnd::AgentEnded(spent) => return Err(self.record_cut_short(row.id, spent)),
                CallEnd::Broke(spent, error)
                    if attempt < engine_step::ATTEMPTS && engine_step::is_malformed(&error) =>
                {
                    self.charge_spent(row.id, spent).map_err(failed)?;
                    request.messages.push(engine_step::retry_note());
                    shown_before.push_str(&streamed);
                    shown_before.push('\n');
                    attempt += 1;
                }
                CallEnd::Broke(spent, error) => {
                    return Err(self.record_broken(row.id, spent, error.to_string()));
                }
            }
        };
        if let Some(open) = self.charge.as_mut() {
            open.spent += response.usage;
        }
        let step = Step {
            shown_before,
            ..step_of(&response)
        };
        self.record_reply(row.id, &response, &step, live)?;
        Ok(step)
    }
}

const SERVICE_STOPPED: &str = "the Noesis service stopped";

/// How a model call ended, short of an error.
enum CallEnd {
    Replied(ModelResponse),
    /// The agent ended mid-call: the usage the provider reported so far.
    AgentEnded(Usage),
    /// The reply broke off with an error after the provider reported usage,
    /// which is charged as a cut-short call's is.
    Broke(Usage, ModelError),
}

/// Resolves once `agent` is in a terminal state, as a cancel leaves it.
/// Subscribed before the row is read, so an end landing between is seen.
async fn agent_ended(runtime: &Runtime, agent: AgentId) {
    let mut bus = runtime.subscribe();
    // A failed read keeps the call going: the reply rereads the row anyway.
    let ended = || match runtime.store().agent(agent) {
        Ok(row) => row.is_none_or(|row| row.status.is_terminal()),
        Err(error) => {
            tracing::warn!(%agent, %error, "reading a Noesis agent's status mid-call failed");
            false
        }
    };
    if ended() {
        return;
    }
    loop {
        let recheck = match bus.recv().await {
            Ok(BusMessage::Event(event)) => {
                event.agent_id == Some(agent) && ends_agent(&event.kind)
            }
            Ok(BusMessage::Delta { .. }) => false,
            Err(RecvError::Lagged(_)) => true,
            // The runtime is gone; the service stop ends the call.
            Err(RecvError::Closed) => return std::future::pending().await,
        };
        if recheck && ended() {
            return;
        }
    }
}

fn ends_agent(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::AgentCancelled { .. }
            | EventKind::AgentFailed { .. }
            | EventKind::AgentCompleted { .. }
    )
}

/// Fold the reply's stream, keeping the provider's latest usage reading so
/// a call cut short by the agent's end still records what it spent.
async fn read_reply(
    mut stream: ModelStream,
    cancel: &CancellationToken,
    mut ended: Pin<&mut impl Future<Output = ()>>,
    on_text: &mut impl FnMut(&str),
) -> Result<CallEnd, EngineError> {
    let mut reply = StreamAccumulator::new();
    let mut usage = Usage::default();
    loop {
        let item = tokio::select! {
            item = stream.next() => item,
            () = &mut ended => {
                cancel.cancel();
                return Ok(CallEnd::AgentEnded(drained_usage(&mut stream, usage).await));
            }
            () = cancel.cancelled() => return Ok(stopped(drained_usage(&mut stream, usage).await)),
        };
        let event = match item {
            None => break,
            Some(Ok(event)) => event,
            Some(Err(error)) => return Ok(CallEnd::Broke(usage, error)),
        };
        match &event {
            StreamEvent::Usage { usage: reading } => usage = *reading,
            StreamEvent::TextDelta { text, .. } => on_text(text),
            _ => {}
        }
        reply
            .apply(&event)
            .map_err(|e| failed(format!("malformed model stream: {e}")))?;
    }
    let response = reply
        .finish()
        .map_err(|e| failed(format!("malformed model stream: {e}")))?;
    debug_assert_eq!(response.usage, usage, "the reply carries the last reading");
    Ok(CallEnd::Replied(response))
}

/// How long a cancelled call may take to report what it spent. An engine
/// client settles its accounting as soon as it sees the cancel.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// The usage a call cancelled while opening reports once it has settled;
/// none if it does not open in time, which the cost gap then records.
async fn settled_usage(
    opening: Pin<&mut impl Future<Output = Result<ModelStream, ModelError>>>,
) -> Usage {
    match tokio::time::timeout(SETTLE_TIMEOUT, opening).await {
        Ok(Ok(mut stream)) => drained_usage(&mut stream, Usage::default()).await,
        Ok(Err(_)) => Usage::default(),
        Err(_) => {
            tracing::warn!("a cancelled Noesis model call did not settle in time");
            Usage::default()
        }
    }
}

/// The latest usage reading left in a cancelled call's stream, read for at
/// most [`SETTLE_TIMEOUT`]; `seen` when it holds none.
async fn drained_usage(stream: &mut ModelStream, seen: Usage) -> Usage {
    let mut usage = seen;
    let drained = tokio::time::timeout(SETTLE_TIMEOUT, async {
        while let Some(Ok(event)) = stream.next().await {
            if let StreamEvent::Usage { usage: reading } = event {
                usage = reading;
            }
        }
    })
    .await;
    if drained.is_err() {
        tracing::warn!("a cancelled Noesis model call did not report its usage in time");
    }
    usage
}

/// The service stopped mid-call: the call is charged as a broken one.
fn stopped(usage: Usage) -> CallEnd {
    CallEnd::Broke(
        usage,
        ModelError::new(ModelErrorClass::Cancelled, SERVICE_STOPPED),
    )
}

/// A reply's blocks as its transcript keeps them, thinking dropped, sealed
/// by `sealer`: the secrets live while it was written and its own calls'.
/// `None` fails closed, as when a sealing call's input is unreadable.
fn transcript_content(response: &ModelResponse, sealer: Option<&Sealer>) -> Vec<ContentBlock> {
    let withhold_text = sealer.is_none_or(|sealer| text_reveals(response, sealer));
    let mut content = Vec::with_capacity(response.content.len());
    let mut withheld = false;
    for block in &response.content {
        match block {
            ContentBlock::Thinking { .. } => {}
            // One withheld note stands for every text block, which together gave it away.
            ContentBlock::Text { .. } if withhold_text => {
                if !withheld {
                    content.push(ContentBlock::Text {
                        text: WITHHELD.to_string(),
                    });
                    withheld = true;
                }
            }
            ContentBlock::ToolUse { id, name, input } => {
                content.push(kept_call(id, name, input, sealer));
            }
            other => content.push(other.clone()),
        }
    }
    debug_assert!(
        !withhold_text
            || !content.iter().any(|block| matches!(block,
        ContentBlock::Text { text } if text != WITHHELD))
    );
    debug_assert!(
        content
            .iter()
            .all(|block| !matches!(block, ContentBlock::Thinking { .. }))
    );
    content
}

/// Whether the reply's text gives a secret away, read as its readers join
/// the blocks: run together, as the step reads them, or apart, as Learn shows them.
fn text_reveals(response: &ModelResponse, sealer: &Sealer) -> bool {
    let blocks: Vec<&str> = response
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    sealer.holds(&blocks.concat()) || sealer.holds(&blocks.join("\n"))
}

/// A call as the transcript keeps it. An id holding a secret becomes
/// [`sealed_id`], which its result is mapped to; a name holding one is
/// replaced unless it is a Noesis tool's own. `None` seals all three.
fn kept_call(id: &str, wire: &str, input: &Value, sealer: Option<&Sealer>) -> ContentBlock {
    let reveals = |text: &str| sealer.is_none_or(|sealer| sealer.holds(text));
    let known = ToolName::parse(&tool_name(wire)).is_some();
    let name = if known || !reveals(wire) {
        wire.to_string()
    } else {
        SEALED_TOOL.to_string()
    };
    let id = if reveals(id) {
        sealed_id(id)
    } else {
        id.to_string()
    };
    debug_assert!(known || name == SEALED_TOOL || !reveals(&name));
    debug_assert!(
        !id.is_empty(),
        "a kept call keeps an id its result can name"
    );
    ContentBlock::ToolUse {
        id,
        name,
        input: kept_input(wire, input, sealer),
    }
}

/// A call's input as the transcript keeps it: sealed whole where it holds a
/// secret, or for a sealing tool field by field, its sealed fields left for
/// the runtime; a field whose own name holds one seals that input whole.
fn kept_input(wire: &str, input: &Value, sealer: Option<&Sealer>) -> Value {
    let sealed = Value::String(tools::SEALED.to_string());
    let Some(sealer) = sealer else {
        return sealed;
    };
    let sealed_fields = ToolName::parse(&tool_name(wire)).map_or(&[][..], ToolName::sealed_fields);
    let mut kept = input.clone();
    let Some(fields) = kept.as_object_mut().filter(|_| !sealed_fields.is_empty()) else {
        sealer.value(&mut kept);
        return kept;
    };
    let named = fields
        .keys()
        .any(|field| !sealed_fields.contains(&field.as_str()) && sealer.holds(field));
    if named {
        return sealed;
    }
    fields
        .iter_mut()
        .filter(|(field, _)| !sealed_fields.contains(&field.as_str()))
        .for_each(|(_, value)| sealer.value(value));
    debug_assert!(kept.is_object(), "a sealing tool's object input stays one");
    debug_assert!(
        sealed_fields
            .iter()
            .all(|field| kept.get(*field) == input.get(*field)),
        "the runtime, not this, seals a pose's sealed fields"
    );
    kept
}

/// The name a call whose own name holds a secret is kept under.
const SEALED_TOOL: &str = "sealed_tool";

/// The id a call whose own id holds a secret is kept under: a stable FNV-1a
/// digest of it, so its result, which names the original, can be mapped.
fn sealed_id(id: &str) -> String {
    let digest = id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("sealed_{digest:016x}")
}

/// The id the transcript keeps for the engine's call id `call`: it, or its
/// [`sealed_id`] when the last reply kept that instead.
fn recorded_id(calls: &[(String, String)], call: String) -> String {
    if calls.iter().any(|(id, _)| *id == call) {
        return call;
    }
    let sealed = sealed_id(&call);
    if calls.iter().any(|(id, _)| *id == sealed) {
        return sealed;
    }
    call
}

/// The step's turn number for live deltas: one more than the replies so far.
fn step_number(history: &[Message]) -> u32 {
    let replies = history
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .count();
    u32::try_from(replies).map_or(u32::MAX, |replies| replies.saturating_add(1))
}

fn message_draft(agent: &Agent, role: MessageRole, content: Vec<ContentBlock>) -> EventDraft {
    let actor = match role {
        MessageRole::Assistant => Actor::agent(agent.id),
        _ => Actor::System,
    };
    EventDraft::new(
        actor,
        EventKind::AgentMessage {
            role,
            content,
            invocation: None,
        },
    )
    .agent(agent.id)
    .run(agent.run)
}

fn result_block(result: ToolResult) -> ContentBlock {
    let text = match result.content {
        serde_json::Value::String(text) => text,
        other => other.to_string(),
    };
    ContentBlock::ToolResult {
        tool_use_id: result.call,
        content: vec![ToolResultBlock::Text { text }],
        is_error: result.is_error,
    }
}

/// The calls of the last assistant entry, and the ids among them already
/// given a result after it. Call ids are unique only within one reply.
fn last_calls(history: &[Message]) -> (Vec<(String, String)>, Vec<String>) {
    let Some(last) = history
        .iter()
        .rposition(|message| message.role == MessageRole::Assistant)
    else {
        return (Vec::new(), Vec::new());
    };
    let calls: Vec<(String, String)> = history[last]
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, .. } => Some((id.clone(), name.clone())),
            _ => None,
        })
        .collect();
    let answered = history[last + 1..]
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
            _ => None,
        })
        .collect();
    (calls, answered)
}

/// The `noesis.ask` calls of the last reply still waiting for their answer.
fn open_asks(calls: &[(String, String)], answered: &[String]) -> Vec<String> {
    let ask = wire_name(ToolName::Ask.as_str());
    calls
        .iter()
        .filter(|(id, name)| *name == ask && !answered.contains(id))
        .map(|(id, _)| id.clone())
        .collect()
}

/// The transcript as a request: consecutive entries of one role joined, as
/// providers require roles to alternate.
fn merged(history: Vec<Message>) -> Vec<Message> {
    let mut messages: Vec<Message> = Vec::with_capacity(history.len());
    for message in history {
        match messages.last_mut() {
            Some(last) if last.role == message.role => last.content.extend(message.content),
            _ => messages.push(message),
        }
    }
    debug_assert!(messages.windows(2).all(|pair| pair[0].role != pair[1].role));
    messages
}

fn step_of(response: &ModelResponse) -> Step {
    let calls = response
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some(ToolCall {
                id: id.clone(),
                name: tool_name(name),
                input: input.clone(),
            }),
            _ => None,
        })
        .collect();
    Step {
        text: response.text(),
        calls,
        usage: spend_of(&response.usage),
        shown_before: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cancelled call's stream as an engine client settles it.
    fn settled_stream(spent: Usage) -> ModelStream {
        let events = vec![
            Ok(StreamEvent::MessageStart {
                provider_message_id: None,
                model: None,
            }),
            Ok(StreamEvent::Usage { usage: spent }),
            Err(ModelError::new(ModelErrorClass::Cancelled, "cancelled")),
        ];
        Box::pin(futures::stream::iter(events))
    }

    #[tokio::test]
    async fn a_stop_racing_a_settled_stream_still_charges_its_usage() {
        let spent = Usage {
            input_tokens: 500,
            output_tokens: 9,
            ..Usage::default()
        };
        // The select is unbiased: repeat so both branch orders are taken.
        for _ in 0..64 {
            let cancel = CancellationToken::new();
            cancel.cancel();
            let ended = std::future::pending::<()>();
            tokio::pin!(ended);
            let mut texts = Vec::new();
            let mut on_text = |text: &str| texts.push(text.to_string());
            let end = read_reply(settled_stream(spent), &cancel, ended, &mut on_text).await;
            let Ok(CallEnd::Broke(usage, error)) = end else {
                panic!("a stopped call breaks");
            };
            assert_eq!(usage, spent, "the settled usage is charged");
            assert_eq!(error.class, ModelErrorClass::Cancelled);
            assert!(texts.is_empty());
        }
    }

    #[tokio::test]
    async fn an_agent_end_racing_a_settled_stream_still_charges_its_usage() {
        let spent = Usage {
            input_tokens: 300,
            ..Usage::default()
        };
        for _ in 0..64 {
            let cancel = CancellationToken::new();
            let ended = std::future::ready(());
            tokio::pin!(ended);
            let end = read_reply(settled_stream(spent), &cancel, ended, &mut |_| {}).await;
            let usage = match end {
                Ok(CallEnd::AgentEnded(usage) | CallEnd::Broke(usage, _)) => usage,
                _ => panic!("the call ended without a reply"),
            };
            assert_eq!(usage, spent, "the settled usage is charged");
        }
    }

    #[test]
    fn tool_names_round_trip_through_their_wire_form() {
        for tool in ToolName::ALL {
            let wire = wire_name(tool.as_str());
            assert!(!wire.contains('.'), "{wire}");
            assert_eq!(tool_name(&wire), tool.as_str());
        }
        assert_eq!(tool_name("bash"), "bash");
    }

    fn ask_reply(id: &str) -> Message {
        Message {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: wire_name(ToolName::Ask.as_str()),
                input: serde_json::json!({}),
            }],
        }
    }

    fn result(id: &str) -> Message {
        Message {
            role: MessageRole::User,
            content: vec![result_block(ToolResult {
                call: id.into(),
                content: serde_json::json!("ok"),
                is_error: false,
            })],
        }
    }

    #[test]
    fn only_the_last_reply_s_calls_are_open_even_when_ids_repeat() {
        let history = vec![ask_reply("call_0"), result("call_0"), ask_reply("call_0")];
        let (calls, answered) = last_calls(&history);
        assert_eq!(calls.len(), 1);
        assert!(
            answered.is_empty(),
            "the earlier result answers the earlier call"
        );
        assert_eq!(open_asks(&calls, &answered), vec!["call_0".to_string()]);

        let mut answered_history = history;
        answered_history.push(result("call_0"));
        let (calls, answered) = last_calls(&answered_history);
        assert!(open_asks(&calls, &answered).is_empty(), "delivered once");
    }

    #[test]
    fn consecutive_entries_of_one_role_are_joined() {
        let history = vec![
            Message::user_text("a"),
            Message::user_text("b"),
            Message::assistant_text("c"),
        ];
        let merged = merged(history);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].content.len(), 2);
    }
}

#[cfg(test)]
#[path = "engine_seal_tests.rs"]
mod seal_tests;
