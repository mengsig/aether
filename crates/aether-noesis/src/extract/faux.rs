//! A scripted faux extraction model: each call takes the next scripted
//! reply for its role and records the prompt. Never a paid model.

use std::collections::VecDeque;

use aether_core::model::Usage;

use super::{CancelFlag, ExtractModel, ModelError, Reply, Role};

pub(super) struct FauxModel {
    pub extractor: VecDeque<Result<String, ModelError>>,
    pub auditor: VecDeque<Result<String, ModelError>>,
    pub calls: Vec<(Role, String)>,
    /// Cancel this flag when the call with this index (from 0) is made.
    pub cancel_at: Option<(usize, CancelFlag)>,
    pub tokens_per_call: u64,
}

impl FauxModel {
    pub fn new(extractor: Vec<&str>, auditor: Vec<&str>) -> Self {
        let script = |replies: Vec<&str>| replies.into_iter().map(|r| Ok(r.to_string())).collect();
        Self {
            extractor: script(extractor),
            auditor: script(auditor),
            calls: Vec::new(),
            cancel_at: None,
            tokens_per_call: 100,
        }
    }

    pub fn roles(&self) -> Vec<Role> {
        self.calls.iter().map(|(role, _)| *role).collect()
    }
}

impl ExtractModel for FauxModel {
    fn invoke(&mut self, role: Role, prompt: &str) -> Result<Reply, ModelError> {
        if let Some((at, flag)) = &self.cancel_at
            && *at == self.calls.len()
        {
            flag.cancel();
        }
        self.calls.push((role, prompt.to_string()));
        let queue = match role {
            Role::Extractor => &mut self.extractor,
            Role::Auditor => &mut self.auditor,
        };
        let next = queue
            .pop_front()
            .unwrap_or_else(|| Err(ModelError::NotSent("script ran out".into())));
        let text = next?;
        let usage = Usage {
            input_tokens: self.tokens_per_call,
            cost_usd: 0.01,
            ..Usage::default()
        };
        Ok(Reply { text, usage })
    }
}
