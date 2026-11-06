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

use aether_core::agent::{Agent, AgentConfig};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, RunId};
use aether_core::model::{
    ContentBlock, CostGap, Message, MessageRole, ModelRef, ModelRequest, ModelResponse,
    StreamAccumulator, StreamEvent, SystemBlock, ToolResultBlock, ToolSpec, Usage,
};
use aether_core::tool::ToolExposure;
use aether_model::{ModelClient, ModelStream, Models};
use aether_noesis::budget::{Billing, Spend};
use aether_noesis::prompts;
use aether_noesis::teach::engine::ModelInfo;
use aether_noesis::teach::{
    AgentSpec, Engine, EngineError, Input, Step, ToolCall, ToolResult, agent_for_role,
};
use aether_noesis::tools::{self, ToolName};
use aether_runtime::noesis_guard::{NoesisBudget, NoesisCharge, lane_of};
use aether_runtime::{BusMessage, CreateAgentSpec, Runtime};
use aether_store::Commit;
use futures::StreamExt;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use super::budget::{ServiceBudget, spend_of};
use super::engine_step::EngineStepClient;

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

impl EngineHost {
    /// Whether `model` runs on an engine CLI (a subscription), not an API.
    fn is_engine(&self, model: &ModelRef) -> bool {
        self.models
            .registry()
            .provider(&model.provider)
            .is_some_and(|provider| provider.protocol.is_engine())
    }

    /// Every listed model, ready only when a step on it could run now: an
    /// engine detected and launchable with its CLI signed in, an API
    /// provider with credentials (or the test client standing in for it).
    pub(super) async fn model_offers(&self) -> Vec<ModelInfo> {
        let mut signed_in: HashMap<String, bool> = HashMap::new();
        let mut offers = Vec::new();
        for info in self.models.list() {
            let provider = info.model.provider.clone();
            let engine = self.is_engine(&info.model);
            if !signed_in.contains_key(&provider) {
                let status = self.models.auth_status(&provider).await;
                signed_in.insert(provider.clone(), status.configured);
            }
            let credentials = signed_in.get(&provider).copied().unwrap_or(false);
            let ready = if engine {
                credentials && self.runtime.engine_launchable(&info.model) == Some(true)
            } else {
                credentials || self.client.is_some()
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
                .map_err(|refusal| not_sent(format!("{refusal:?}")))?;
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
    ) -> Result<(), EngineError> {
        debug_assert!(!content.is_empty(), "an entry carries content");
        let mut commit = Commit::new();
        commit.push_event(message_draft(agent, role, content));
        self.host.runtime.apply(commit).map_err(not_sent)?;
        Ok(())
    }

    /// The blocks `input` adds to the transcript; results the last reply's
    /// calls already have are dropped, so a repeated delivery adds nothing.
    fn blocks_of(&self, history: &[Message], input: Input) -> Vec<ContentBlock> {
        let (calls, answered) = last_calls(history);
        match input {
            Input::Learner(text) | Input::Handoff(text) => vec![ContentBlock::Text { text }],
            Input::ToolResults(results) => results
                .into_iter()
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
    /// stops or the agent ends (a cancel) while it runs.
    fn invoke(&self, agent: &Agent, request: ModelRequest) -> Result<CallEnd, EngineError> {
        let host = self.host;
        let handle = tokio::runtime::Handle::current();
        let cancel = host.cancel.child_token();
        tokio::task::block_in_place(|| {
            handle.block_on(async {
                let client = host.client_for(agent).await?;
                let ended = agent_ended(&host.runtime, agent.id);
                tokio::pin!(ended);
                let stream = tokio::select! {
                    stream = client.stream(request, cancel.clone()) => stream.map_err(not_sent)?,
                    () = &mut ended => {
                        cancel.cancel();
                        return Ok(CallEnd::AgentEnded(Usage::default()));
                    }
                    () = cancel.cancelled() => return Err(failed(SERVICE_STOPPED)),
                };
                read_reply(stream, &cancel, ended).await
            })
        })
    }

    /// Record the reply and its usage, so the day's spend survives a restart.
    /// Only the agent's stats are written, over the row as it is now.
    fn record_reply(&self, agent: AgentId, response: &ModelResponse) -> Result<(), EngineError> {
        let content: Vec<ContentBlock> = response
            .content
            .iter()
            .filter(|block| !matches!(block, ContentBlock::Thinking { .. }))
            .cloned()
            .collect();
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
        if let Some(open) = self.charge.as_mut() {
            debug_assert_eq!(open.agent, agent, "the call ran under its own admission");
            open.spent += spent;
        }
        match self
            .host
            .runtime
            .record_noesis_call(agent, spent, |_, _| {})
        {
            Ok(_) => failed(error),
            Err(record) => failed(format!(
                "{error}; and the call's spend was not recorded: {record}"
            )),
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
        let handle = tokio::runtime::Handle::current();
        tokio::task::block_in_place(|| handle.block_on(self.host.model_offers()))
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

    fn deliver(&mut self, agent: &str, input: Input) -> Result<(), EngineError> {
        let row = self.agent(agent)?;
        let history = self.transcript(row.id)?;
        let blocks = self.blocks_of(&history, input);
        if blocks.is_empty() {
            return Ok(());
        }
        self.record(&row, MessageRole::User, blocks)
    }

    fn estimate(&self, _agent: &str) -> Spend {
        Spend::default()
    }

    fn charges_own_calls(&self) -> bool {
        true
    }

    fn step(&mut self, agent: &str) -> Result<Step, EngineError> {
        let row = self.agent(agent)?;
        if row.status.is_terminal() {
            return Err(not_sent(format!("agent {agent} is {}", row.status.kind())));
        }
        let request = self.request(&row, self.transcript(row.id)?)?;
        let open = self.admitted(&row)?;
        let refused = open
            .charge
            .refuses_model(&row.model)
            .or_else(|| open.charge.before_invocation(&open.spent));
        if let Some(reason) = refused {
            return Err(not_sent(reason));
        }
        let response = match self.invoke(&row, request)? {
            CallEnd::Replied(response) => response,
            CallEnd::AgentEnded(spent) => return Err(self.record_cut_short(row.id, spent)),
            CallEnd::Broke(spent, error) => return Err(self.record_broken(row.id, spent, error)),
        };
        if let Some(open) = self.charge.as_mut() {
            open.spent += response.usage;
        }
        self.record_reply(row.id, &response)?;
        Ok(step_of(&response))
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
    Broke(Usage, String),
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
) -> Result<CallEnd, EngineError> {
    let mut reply = StreamAccumulator::new();
    let mut usage = Usage::default();
    loop {
        let item = tokio::select! {
            item = stream.next() => item,
            () = &mut ended => {
                cancel.cancel();
                return Ok(CallEnd::AgentEnded(usage));
            }
            () = cancel.cancelled() => return Err(failed(SERVICE_STOPPED)),
        };
        let event = match item {
            None => break,
            Some(Ok(event)) => event,
            Some(Err(error)) => return Ok(CallEnd::Broke(usage, error.to_string())),
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
