//! A scripted engine for the parity tests: each agent kind replays its
//! scripted steps in order, and every spawn, input and step is recorded.

use std::collections::{HashMap, VecDeque};

use serde_json::Value;

use super::engine::{AgentSpec, Engine, EngineError, Input, ModelInfo, Step, ToolCall};
use crate::budget::{Billing, Spend};
use crate::prompts::Agent;

pub const STEP_ESTIMATE: Spend = Spend {
    usd: 0.01,
    tokens: 0,
};

#[derive(Debug, Default)]
pub struct FauxEngine {
    pub spawned: Vec<(String, AgentSpec)>,
    pub inputs: Vec<(String, Input)>,
    pub steps_run: usize,
    pub fail_spawn: bool,
    /// Refuse this many next deliveries with `NotSent`.
    pub fail_deliveries: usize,
    /// Offer only a model billed per request.
    pub api_only: bool,
    scripts: HashMap<Agent, VecDeque<Result<Step, EngineError>>>,
    next_call: u32,
}

impl FauxEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue one model step for the next agent of `agent`'s kind.
    pub fn script(&mut self, agent: Agent, text: &str, calls: Vec<(&str, Value)>) -> &mut Self {
        let calls = calls
            .into_iter()
            .map(|(name, input)| {
                self.next_call += 1;
                ToolCall {
                    id: format!("call_{}", self.next_call),
                    name: name.to_string(),
                    input,
                }
            })
            .collect();
        let step = Step {
            text: text.to_string(),
            calls,
            usage: Spend {
                usd: 0.002,
                tokens: 100,
            },
        };
        self.scripts.entry(agent).or_default().push_back(Ok(step));
        self
    }

    /// Queue one model step exactly as given, call ids included.
    pub fn script_step(&mut self, agent: Agent, step: Step) -> &mut Self {
        self.scripts.entry(agent).or_default().push_back(Ok(step));
        self
    }

    pub fn fail_next(&mut self, agent: Agent, error: EngineError) -> &mut Self {
        self.scripts.entry(agent).or_default().push_back(Err(error));
        self
    }

    pub fn remaining(&self, agent: Agent) -> usize {
        self.scripts.get(&agent).map_or(0, VecDeque::len)
    }

    pub fn kind_of(&self, agent: &str) -> Option<Agent> {
        self.spawned
            .iter()
            .find(|(id, _)| id == agent)
            .map(|(_, spec)| spec.agent)
    }

    pub fn inputs_of(&self, agent: &str) -> Vec<&Input> {
        self.inputs
            .iter()
            .filter(|(id, _)| id == agent)
            .map(|(_, input)| input)
            .collect()
    }
}

impl Engine for FauxEngine {
    fn models(&self) -> Vec<ModelInfo> {
        let mut models = vec![ModelInfo {
            id: "api/billed".into(),
            billing: Billing::Api,
            ready: true,
        }];
        if !self.api_only {
            models.push(ModelInfo {
                id: "faux/subscription".into(),
                billing: Billing::Subscription,
                ready: true,
            });
        }
        models
    }

    fn spawn(&mut self, spec: AgentSpec) -> Result<String, EngineError> {
        if self.fail_spawn {
            return Err(EngineError::NotSent("spawn refused by the script".into()));
        }
        let id = format!("agt_faux{}", self.spawned.len() + 1);
        self.spawned.push((id.clone(), spec));
        Ok(id)
    }

    /// Every input is recorded; its seq is its place among all inputs.
    fn deliver(&mut self, agent: &str, input: Input) -> Result<Option<u64>, EngineError> {
        assert!(
            self.kind_of(agent).is_some(),
            "input for an unknown agent {agent}"
        );
        if self.fail_deliveries > 0 {
            self.fail_deliveries -= 1;
            return Err(EngineError::NotSent(
                "delivery refused by the script".into(),
            ));
        }
        self.inputs.push((agent.to_string(), input));
        let seq = u64::try_from(self.inputs.len()).expect("a test's inputs fit");
        Ok(Some(seq))
    }

    fn estimate(&self, _agent: &str) -> Spend {
        STEP_ESTIMATE
    }

    /// An agent without a scripted step replies with nothing and waits.
    fn step(&mut self, agent: &str) -> Result<Step, EngineError> {
        let kind = self.kind_of(agent).expect("a step for a spawned agent");
        self.steps_run += 1;
        let next = self.scripts.get_mut(&kind).and_then(VecDeque::pop_front);
        next.unwrap_or_else(|| Ok(Step::default()))
    }
}
