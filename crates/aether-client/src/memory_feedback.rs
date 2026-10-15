use aether_core::id::{MemoryFeedbackId, MemoryId};
use aether_core::memory_feedback::{MemoryFeedback, MemoryFeedbackList, MemoryFeedbackSpec};
use aether_protocol::{Command, Reply};

use crate::{Client, ClientError, Result, unexpected};

impl Client {
    pub async fn record_memory_feedback(&self, spec: MemoryFeedbackSpec) -> Result<MemoryFeedback> {
        self.require_memory_feedback()?;
        let expected = spec.clone();
        match self
            .request(Command::RecordMemoryFeedback {
                memory: spec.memory,
                verdict: spec.verdict,
                evidence: spec.evidence,
                task: spec.task,
                invocation: spec.invocation,
                reason: spec.reason,
                idempotency_key: None,
            })
            .await?
        {
            Reply::MemoryFeedback(record) => {
                if record.spec != expected {
                    return Err(ClientError::Malformed(
                        "daemon returned different feedback evidence".into(),
                    ));
                }
                if record.retracted_at.is_some() || record.retraction_reason.is_some() {
                    return Err(ClientError::Malformed(
                        "new feedback was already retracted".into(),
                    ));
                }
                Ok(record)
            }
            other => Err(unexpected("memory feedback", &other)),
        }
    }

    pub async fn retract_memory_feedback(
        &self,
        feedback: MemoryFeedbackId,
        reason: String,
    ) -> Result<MemoryFeedback> {
        self.require_memory_feedback()?;
        match self
            .request(Command::RetractMemoryFeedback { feedback, reason })
            .await?
        {
            Reply::MemoryFeedback(record) => {
                if record.id != feedback {
                    return Err(ClientError::Malformed(
                        "daemon retracted different feedback".into(),
                    ));
                }
                if record.retracted_at.is_none()
                    || record
                        .retraction_reason
                        .as_ref()
                        .is_none_or(|r| r.trim().is_empty())
                {
                    return Err(ClientError::Malformed(
                        "daemon did not mark feedback retracted".into(),
                    ));
                }
                Ok(record)
            }
            other => Err(unexpected("memory feedback", &other)),
        }
    }

    pub async fn memory_feedback(
        &self,
        memory: MemoryId,
        limit: Option<usize>,
    ) -> Result<MemoryFeedbackList> {
        self.require_memory_feedback()?;
        match self
            .request(Command::MemoryFeedback { memory, limit })
            .await?
        {
            Reply::MemoryFeedbackList(found) => {
                if found.records.len() > limit.unwrap_or(100) {
                    return Err(ClientError::Malformed(
                        "daemon exceeded the feedback limit".into(),
                    ));
                }
                if found
                    .records
                    .iter()
                    .any(|record| record.spec.memory != memory)
                {
                    return Err(ClientError::Malformed(
                        "daemon returned another memory's feedback".into(),
                    ));
                }
                Ok(found)
            }
            other => Err(unexpected("memory feedback list", &other)),
        }
    }

    fn require_memory_feedback(&self) -> Result<()> {
        if self.welcome().is_some_and(|welcome| {
            welcome
                .features
                .iter()
                .any(|feature| feature == aether_protocol::features::MEMORY_FEEDBACK)
        }) {
            return Ok(());
        }
        Err(ClientError::Incompatible(
            "daemon cannot record memory feedback; update Aether".into(),
        ))
    }
}
