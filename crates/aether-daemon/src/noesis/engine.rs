//! The teaching loop's [`Engine`] in the daemon: each Noesis agent is an
//! Aether agent in the `noesis` run with subsystem `noesis` and no Aether
//! tools. Its inputs and replies are `agent.message` transcript entries, so
//! the runtime's seal writes every sealed field as `"<sealed>"`. Each model
//! call is admitted by the service budget and passes the runtime's one
//! [`NoesisCharge::before_invocation`] hook before it is sent.

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use aether_core::agent::{Agent, AgentConfig};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, RunId};
use aether_core::model::{
    ContentBlock, Message, MessageRole, ModelRef, ModelRequest, ModelResponse, SystemBlock,
    ToolResultBlock, ToolSpec, Usage,
};
use aether_core::tool::ToolExposure;
use aether_model::{ModelClient, Models};
use aether_noesis::budget::{Billing, Spend};
use aether_noesis::prompts;
use aether_noesis::teach::engine::ModelInfo;
use aether_noesis::teach::{
    AgentSpec, Engine, EngineError, Input, Step, ToolCall, ToolResult, agent_for_role,
};
use aether_noesis::tools::{self, ToolName};
use aether_runtime::noesis_guard::{NoesisBudget, NoesisCharge, lane_of};
use aether_runtime::{CreateAgentSpec, Runtime};
use aether_store::{Commit, Mutation};
use tokio_util::sync::CancellationToken;

use super::budget::{ServiceBudget, spend_of};

/// What every engine of one running service shares.
pub(super) struct EngineHost {
    pub runtime: Arc<Runtime>,
    pub models: Arc<Models>,
    pub budget: Arc<ServiceBudget>,
    /// Replaces the provider's client in tests, as the runtime's override does.
    pub client: Option<Arc<dyn ModelClient>>,
    /// Cancelled when the service stops, ending a model call in flight.
    pub cancel: CancellationToken,
    pub prompt_overrides: Option<PathBuf>,
    /// The one run every Noesis agent is created in.
    pub run: RunId,
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

    /// One model call over the agent's transcript, ended if the service stops.
    fn invoke(&self, agent: &Agent, request: ModelRequest) -> Result<ModelResponse, EngineError> {
        let host = self.host;
        let handle = tokio::runtime::Handle::current();
        let cancel = host.cancel.child_token();
        tokio::task::block_in_place(|| {
            handle.block_on(async {
                let client = match &host.client {
                    Some(client) => client.clone(),
                    None => host.models.client(&agent.model).await.map_err(not_sent)?,
                };
                let call = async {
                    let stream = client
                        .stream(request, cancel.clone())
                        .await
                        .map_err(not_sent)?;
                    aether_model::collect(stream, |_| {}).await.map_err(failed)
                };
                tokio::select! {
                    response = call => response,
                    () = cancel.cancelled() => Err(failed("the Noesis service stopped")),
                }
            })
        })
    }

    /// Record the reply and its usage, so the day's spend survives a restart.
    fn record_reply(&self, mut agent: Agent, response: &ModelResponse) -> Result<(), EngineError> {
        let content: Vec<ContentBlock> = response
            .content
            .iter()
            .filter(|block| !matches!(block, ContentBlock::Thinking { .. }))
            .cloned()
            .collect();
        agent.stats.usage += response.usage;
        agent.stats.turns = agent.stats.turns.saturating_add(1);
        let mut commit = Commit::new();
        if !content.is_empty() {
            commit.push_event(message_draft(&agent, MessageRole::Assistant, content));
        }
        commit.push_event(
            EventDraft::new(
                Actor::agent(agent.id),
                EventKind::AgentUsageUpdated {
                    total: agent.stats.usage,
                    turn_usage: response.usage,
                },
            )
            .agent(agent.id)
            .run(agent.run),
        );
        commit.push_mutation(Mutation::UpsertAgent(agent));
        self.host.runtime.apply(commit).map_err(failed)?;
        Ok(())
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
        let registry = self.host.models.registry();
        self.host
            .models
            .list()
            .into_iter()
            .map(|info| {
                let engine = registry
                    .provider(&info.model.provider)
                    .is_none_or(|provider| provider.protocol.is_engine());
                // Engine CLIs are driven by the runtime's turns, not by this loop.
                ModelInfo {
                    id: info.model.to_string(),
                    billing: if engine {
                        Billing::Subscription
                    } else {
                        Billing::Api
                    },
                    ready: !engine,
                }
            })
            .collect()
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
        let request = self.request(&row, self.transcript(row.id)?)?;
        let open = self.admitted(&row)?;
        let refused = open
            .charge
            .refuses_model(&row.model)
            .or_else(|| open.charge.before_invocation(&open.spent));
        if let Some(reason) = refused {
            return Err(not_sent(reason));
        }
        let response = self.invoke(&row, request)?;
        if let Some(open) = self.charge.as_mut() {
            open.spent += response.usage;
        }
        self.record_reply(row, &response)?;
        Ok(step_of(&response))
    }
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
