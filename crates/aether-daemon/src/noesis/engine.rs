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
use aether_noesis::teach::engine::{AskPolicy, ModelInfo, ReplyText};
use aether_noesis::teach::{
    AgentSpec, Engine, EngineError, Input, Sealer, Step, ToolCall, ToolResult, agent_for_role,
    kept_reply,
};
use aether_noesis::tools::{self, ToolName};
use aether_runtime::noesis_guard::{NoesisBudget, NoesisCharge, NoesisRefusal, lane_of};
use aether_runtime::{BusMessage, CreateAgentSpec, Runtime};
use aether_store::Commit;
use futures::StreamExt;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use super::budget::{ServiceBudget, runs_on_engine, spend_of};
use super::engine_step::{self, EngineStepClient};

const MODEL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const MODEL_CALL_LIMIT: Duration = Duration::from_secs(300);

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
    guidance: Option<(String, String)>,
    turn_stream: Option<(AgentId, aether_runtime::DeltaStream)>,
}

/// One agent's admission and what its steps have spent under it.
struct OpenCharge {
    agent: AgentId,
    charge: Box<dyn NoesisCharge>,
    spent: Usage,
}

impl<'a> AetherEngine<'a> {
    pub(super) fn new(host: &'a EngineHost) -> Self {
        Self {
            host,
            charge: None,
            guidance: None,
            turn_stream: None,
        }
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
        self.finish_turn_stream();
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
                .filter(|result| !answered.contains(&result.call))
                .map(result_block)
                .collect(),
            Input::Answer(mut payload) => {
                let ask = wire_name(ToolName::Ask.as_str());
                if !calls.iter().any(|(_, name)| *name == ask) {
                    return vec![ContentBlock::Text {
                        text: format!("The learner answered: {payload}"),
                    }];
                }
                let open = open_asks(&calls, &answered);
                debug_assert!(open.len() <= 1, "one question is open at a time");
                if open.is_empty() {
                    return Vec::new();
                }
                let learner = learner_answer(&mut payload);
                let mut blocks: Vec<_> = open
                    .into_iter()
                    .map(|call| {
                        result_block(ToolResult {
                            call,
                            content: payload.clone(),
                            is_error: false,
                        })
                    })
                    .collect();
                blocks.push(ContentBlock::Text { text: learner });
                blocks
            }
            Input::Guidance(_) | Input::Failure(_) => Vec::new(),
        }
    }

    /// One model call over the agent's transcript, ended if the service
    /// stops or the agent ends (a cancel) while it runs. The reply is
    /// buffered whole: nothing of it is published until it is kept.
    fn invoke(&mut self, agent: &Agent, request: ModelRequest) -> Result<CallEnd, EngineError> {
        if self
            .turn_stream
            .as_ref()
            .is_none_or(|(id, _)| *id != agent.id)
        {
            self.finish_turn_stream();
            let stream = self
                .host
                .runtime
                .noesis_turn_started(agent.id, lane_of(agent.role.as_str()));
            self.turn_stream = Some((agent.id, stream));
        }
        self.invoke_buffered(agent, request)
    }

    fn finish_turn_stream(&mut self) {
        if let Some((agent, mut stream)) = self.turn_stream.take() {
            self.host.runtime.noesis_turn_finished(agent, &mut stream);
        }
    }

    fn invoke_buffered(
        &self,
        agent: &Agent,
        request: ModelRequest,
    ) -> Result<CallEnd, EngineError> {
        let host = self.host;
        let handle = tokio::runtime::Handle::current();
        let cancel = host.cancel.child_token();
        tokio::task::block_in_place(|| {
            handle.block_on(async {
                let client = match tokio::time::timeout(MODEL_IDLE_TIMEOUT, host.client_for(agent)).await {
                    Ok(client) => client?,
                    Err(_) => return Ok(timed_out(Usage::default())),
                };
                let ended = agent_ended(&host.runtime, agent.id);
                tokio::pin!(ended);
                let opening = client.stream(request, cancel.clone());
                tokio::pin!(opening);
                let stream = tokio::select! {
                    stream = &mut opening => match stream {
                        Ok(stream) => stream,
                        Err(error) if error.class == ModelErrorClass::Timeout => return Ok(CallEnd::Broke(Usage::default(), error)),
                        Err(error) => return Err(not_sent(error)),
                    },
                    () = &mut ended => {
                        cancel.cancel();
                        return Ok(CallEnd::AgentEnded(settled_usage(opening).await));
                    }
                    () = cancel.cancelled() => {
                        return Ok(stopped(settled_usage(opening).await));
                    }
                    () = tokio::time::sleep(MODEL_IDLE_TIMEOUT) => {
                        cancel.cancel();
                        return Ok(timed_out(settled_usage(opening).await));
                    }
                };
                let idle = (!host.is_engine(&agent.model)).then_some(MODEL_IDLE_TIMEOUT);
                read_reply(stream, &cancel, ended, idle).await
            })
        })
    }

    /// Retry malformed replies with a corrective note, including one retry
    /// of a failed step. Each attempt is gated and its spend recorded.
    fn reply(
        &mut self,
        row: &Agent,
        request: &mut ModelRequest,
    ) -> Result<ModelResponse, EngineError> {
        let mut stalls = 0;
        for attempt in 1..=engine_step::ATTEMPTS + 1 {
            self.gate(row)?;
            match self.invoke(row, request.clone())? {
                CallEnd::Replied(response) => {
                    if let Some(open) = self.charge.as_mut() {
                        open.spent += response.usage;
                    }
                    return Ok(response);
                }
                CallEnd::AgentEnded(spent) => return Err(self.record_cut_short(row, spent)),
                CallEnd::Broke(mut spent, error) if error.class == ModelErrorClass::Timeout => {
                    spent.mark_cost_gap(CostGap::Interrupted);
                    self.charge_spent(row, spent).map_err(failed)?;
                    stalls += 1;
                    if stalls >= 2 {
                        return Err(EngineError::Stalled(error.to_string()));
                    }
                }
                CallEnd::Broke(spent, error)
                    if attempt < engine_step::ATTEMPTS && engine_step::is_malformed(&error) =>
                {
                    self.charge_spent(row, spent).map_err(failed)?;
                    with_note(&mut request.messages, engine_step::RETRY_NOTE);
                }
                CallEnd::Broke(spent, error) => {
                    return Err(self.record_broken(row, spent, error.to_string()));
                }
            }
        }
        unreachable!("the last attempt returns whatever it got")
    }

    /// Record the kept reply's `content` and the `usage` of the step `called`
    /// made, so the day's spend survives a restart. Only the agent's stats are
    /// written, over the row as it is now; the runtime seals a pose's sealed fields.
    fn record_reply(
        &self,
        called: &Agent,
        usage: Usage,
        content: Vec<ContentBlock>,
    ) -> Result<(), EngineError> {
        debug_assert!(
            content
                .iter()
                .all(|block| !matches!(block, ContentBlock::Thinking { .. })),
            "thinking is never kept"
        );
        let agent = called.id;
        let row = self
            .host
            .runtime
            .record_noesis_call(agent, &called.model, usage, |row, commit| {
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
    fn record_cut_short(&mut self, called: &Agent, mut spent: Usage) -> EngineError {
        let agent = called.id;
        spent.mark_cost_gap(CostGap::Interrupted);
        if let Some(open) = self.charge.as_mut() {
            debug_assert_eq!(open.agent, agent, "the call ran under its own admission");
            open.spent += spent;
        }
        match self
            .host
            .runtime
            .record_noesis_call(agent, &called.model, spent, |_, _| {})
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
    fn record_broken(&mut self, called: &Agent, mut spent: Usage, error: String) -> EngineError {
        debug_assert!(!error.is_empty(), "a broken reply says why");
        spent.mark_cost_gap(CostGap::Interrupted);
        match self.charge_spent(called, spent) {
            Ok(()) => failed(error),
            Err(record) => failed(format!(
                "{error}; and the call's spend was not recorded: {record}"
            )),
        }
    }

    /// Charge a call that produced no reply to keep, and record its usage.
    fn charge_spent(&mut self, called: &Agent, spent: Usage) -> Result<(), String> {
        let open = self.charge.as_mut();
        assert!(
            open.as_ref().is_some_and(|open| open.agent == called.id),
            "the call ran under its own admission"
        );
        if let Some(open) = open {
            open.spent += spent;
        }
        self.record_spend(called, spent)
            .map_err(|error| error.to_string())
    }

    /// Record what a call `called` made and already charged spent, with
    /// nothing of its reply.
    fn record_spend(&self, called: &Agent, spent: Usage) -> Result<(), EngineError> {
        debug_assert!(spent.cost_usd >= 0.0, "a call never refunds");
        let recorded =
            self.host
                .runtime
                .record_noesis_call(called.id, &called.model, spent, |_, _| {});
        recorded.map(|_| ()).map_err(failed)
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

    fn step_request(
        &mut self,
        row: &Agent,
        agent: &str,
        policy: ReplyText,
    ) -> Result<ModelRequest, EngineError> {
        let mut request = self.request(row, self.transcript(row.id)?)?;
        if let Some((for_agent, guidance)) = self.guidance.take() {
            if for_agent != agent {
                return Err(not_sent("guidance belongs to a different agent"));
            }
            with_note(&mut request.messages, &guidance);
        }
        if policy.asks == AskPolicy::Withhold {
            request
                .tools
                .retain(|tool| tool.name != wire_name(ToolName::Ask.as_str()));
            debug_assert!(request.tools.iter().all(|tool| tool.name != "noesis_ask"));
        }
        debug_assert!(!request.messages.is_empty());
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
        if let Input::Guidance(text) = input {
            debug_assert!(!agent.is_empty() && !text.trim().is_empty());
            self.guidance = Some((agent.to_string(), text));
            return Ok(None);
        }
        let row = self.agent(agent)?;
        if let Input::Failure(text) = input {
            return self
                .record(
                    &row,
                    MessageRole::Assistant,
                    vec![ContentBlock::Text { text }],
                )
                .map(Some);
        }
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

    fn last_input_is_answer(&self, agent: &str) -> Result<bool, EngineError> {
        let row = self.agent(agent)?;
        let history = self.transcript(row.id)?;
        let Some(last) = history
            .last()
            .filter(|message| message.role == MessageRole::User)
        else {
            return Ok(false);
        };
        let (calls, _) = last_calls(&history);
        let ask = wire_name(ToolName::Ask.as_str());
        let answered = last.content.iter().any(|block| {
            matches!(block,
            ContentBlock::ToolResult { tool_use_id, is_error: false, .. }
                if calls.iter().any(|(id, name)| id == tool_use_id && name == &ask))
        });
        let learner = last.content.iter().any(|block| {
            matches!(block,
            ContentBlock::Text { text } if text.starts_with("The learner answered: "))
        });
        Ok(answered && learner)
    }

    fn charges_own_calls(&self) -> bool {
        true
    }

    /// The step's reply is kept only as [`kept_reply`] decides: a reply
    /// that gives a sealed answer away is charged and discarded, never
    /// recorded, and made once more with the seal's reminder.
    fn step(
        &mut self,
        agent: &str,
        live: &Sealer,
        text_policy: ReplyText,
    ) -> Result<Step, EngineError> {
        let row = self.agent(agent)?;
        if row.status.is_terminal() {
            return Err(not_sent(format!("agent {agent} is {}", row.status.kind())));
        }
        let mut request = self.step_request(&row, agent, text_policy)?;
        let mut unrecorded = None;
        let replies = kept_reply(live, |reminder| {
            if let Some(reminder) = reminder {
                // The last reply was discarded: its spend lands before the next is made.
                if let Some(spent) = unrecorded.take() {
                    self.record_spend(&row, spent)?;
                }
                with_note(&mut request.messages, reminder);
                request
                    .tools
                    .retain(|tool| !matches!(tool.name.as_str(), "noesis_ask" | "noesis_problem"));
                debug_assert!(request.tools.iter().all(|tool| {
                    !matches!(tool.name.as_str(), "noesis_ask" | "noesis_problem")
                }));
            }
            let response = self.reply(&row, &mut request)?;
            unrecorded = Some(response.usage);
            Ok((step_of(&response), response))
        })?;
        let (usage, mut content) = match (&replies.kept, replies.discarded.last()) {
            (Some(kept), _) => (kept.usage, kept_content(kept)),
            (None, Some(last)) => (last.usage, text_content(&replies.step.text)),
            (None, None) => unreachable!("a step without a kept reply discarded two"),
        };
        let mut step = replies.step;
        let shaped = text_policy.shape(&mut step);
        if shaped.text_dropped {
            content.retain(|block| !matches!(block, ContentBlock::Text { .. }));
        }
        if shaped.asks_dropped {
            content.retain(|block| !is_ask_use(block));
        }
        if content.is_empty() {
            self.record_spend(&row, usage)?;
        } else {
            self.record_reply(&row, usage, content)?;
        }
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
    idle: Option<Duration>,
) -> Result<CallEnd, EngineError> {
    let mut reply = StreamAccumulator::new();
    let mut usage = Usage::default();
    let deadline = tokio::time::Instant::now() + MODEL_CALL_LIMIT;
    loop {
        let item = tokio::select! {
            item = stream.next() => item,
            () = &mut ended => {
                cancel.cancel();
                return Ok(CallEnd::AgentEnded(drained_usage(&mut stream, usage).await));
            }
            () = cancel.cancelled() => return Ok(stopped(drained_usage(&mut stream, usage).await)),
            () = async { match idle { Some(timeout) => tokio::time::sleep(timeout).await, None => std::future::pending().await } } => {
                cancel.cancel();
                return Ok(timed_out(drained_usage(&mut stream, usage).await));
            }
            () = tokio::time::sleep_until(deadline) => {
                cancel.cancel();
                return Ok(timed_out(drained_usage(&mut stream, usage).await));
            }
        };
        let event = match item {
            None => break,
            Some(Ok(event)) => event,
            Some(Err(error)) => return Ok(CallEnd::Broke(usage, error)),
        };
        if let StreamEvent::Usage { usage: reading } = &event {
            usage = *reading;
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

fn timed_out(usage: Usage) -> CallEnd {
    CallEnd::Broke(
        usage,
        ModelError::new(ModelErrorClass::Timeout, "Noesis model made no progress"),
    )
}

/// A kept reply's blocks as its transcript keeps them: thinking dropped.
fn kept_content(response: &ModelResponse) -> Vec<ContentBlock> {
    let content: Vec<ContentBlock> = response
        .content
        .iter()
        .filter(|block| !matches!(block, ContentBlock::Thinking { .. }))
        .cloned()
        .collect();
    debug_assert!(content.len() <= response.content.len());
    content
}

fn is_ask_use(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::ToolUse { name, .. }
        if tool_name(name) == ToolName::Ask.as_str())
}

/// The transcript entry of a step that kept no reply.
fn text_content(text: &str) -> Vec<ContentBlock> {
    debug_assert!(!text.is_empty(), "a neutral step says something");
    vec![ContentBlock::Text {
        text: text.to_string(),
    }]
}

/// Add `note` to the request's last message when it is the user's, else as
/// a user message of its own, so roles still alternate.
fn with_note(messages: &mut Vec<Message>, note: &str) {
    let text = ContentBlock::Text {
        text: note.to_string(),
    };
    match messages.last_mut() {
        Some(last) if last.role == MessageRole::User => last.content.push(text),
        _ => messages.push(Message::user_text(note)),
    }
    debug_assert!(
        messages
            .last()
            .is_some_and(|last| last.role == MessageRole::User)
    );
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

/// Separate the learner's words from the tool's grading metadata.
pub(super) fn learner_answer(payload: &mut serde_json::Value) -> String {
    let mut learner = serde_json::Map::new();
    if let Some(fields) = payload.as_object_mut() {
        for name in [
            "choices",
            "chosen",
            "text",
            "dont_know",
            "confidence",
            "thinking",
        ] {
            if let Some(value) = fields.remove(name) {
                learner.insert(name.to_string(), value);
            }
        }
    }
    debug_assert!(!learner.is_empty(), "an answer has learner fields");
    debug_assert!(payload.get("answer_key").is_some() || payload.get("question").is_some());
    format!(
        "The learner answered: {}",
        serde_json::Value::Object(learner)
    )
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
    let text_blocks = response
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    Step {
        text: response.text(),
        text_blocks,
        calls,
        usage: spend_of(&response.usage),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn silent_model_stream_is_cancelled_with_its_usage() {
        let spent = Usage {
            input_tokens: 17,
            ..Usage::default()
        };
        let events = futures::stream::iter(vec![Ok(StreamEvent::Usage { usage: spent })]);
        let stream: ModelStream = Box::pin(events.chain(futures::stream::pending()));
        let cancel = CancellationToken::new();
        let ended = std::future::pending::<()>();
        tokio::pin!(ended);
        let result = read_reply(stream, &cancel, ended, Some(Duration::from_secs(1))).await;
        let Ok(CallEnd::Broke(usage, error)) = result else {
            panic!("a silent call must fail");
        };
        assert_eq!(error.class, ModelErrorClass::Timeout);
        assert_eq!(usage.input_tokens, 17);
        assert!(cancel.is_cancelled());
    }

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
            let end = read_reply(settled_stream(spent), &cancel, ended, None).await;
            let Ok(CallEnd::Broke(usage, error)) = end else {
                panic!("a stopped call breaks");
            };
            assert_eq!(usage, spent, "the settled usage is charged");
            assert_eq!(error.class, ModelErrorClass::Cancelled);
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
            let end = read_reply(settled_stream(spent), &cancel, ended, None).await;
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
    fn learner_answer_is_separate_from_tool_grading_metadata() {
        let mut payload = serde_json::json!({
            "question": "q1", "text": "My answer", "thinking": "My reasoning",
            "chosen": ["Choice"], "confidence": 90, "dont_know": false,
            "answer_key": "Expected"
        });
        let text = learner_answer(&mut payload);
        assert!(text.contains("My reasoning") && text.contains("Choice"));
        assert!(!text.contains("Expected"));
        assert_eq!(
            payload,
            serde_json::json!({"question": "q1", "answer_key": "Expected"})
        );
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
