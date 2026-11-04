//! The transcript seal: answers a Noesis agent commits to stay off every read
//! path until Noesis reveals them.
//!
//! Sealed tool calls stream no input delta and are persisted with each sealed
//! field replaced by [`SEALED`]. A Noesis agent's thinking is neither streamed
//! nor persisted. The tool itself still runs with its real input: only what
//! is published and written is sealed.

use std::borrow::Cow;

use aether_core::id::AgentId;
use aether_core::idempotency::IdempotencyKey;
use aether_core::model::{ContentBlock, Message};
use aether_core::tool::ToolCall;
use aether_store::{Commit, Mutation, Store};
use serde_json::Value;

use crate::Result;
use crate::bus::DeltaKind;
use crate::noesis_guard::NOESIS_SUBSYSTEM;

/// What a sealed field reads as wherever it is published or persisted.
pub const SEALED: &str = "<sealed>";

/// Canonical tool names and the input fields each seals.
const SEALED_TOOLS: &[(&str, &[&str])] = &[
    ("noesis_ask", &["answer_key", "explanation"]),
    ("noesis_problem", &["solution"]),
];

/// A tool name with any MCP prefix (`mcp__<server>__`) or bridge prefix
/// (`aether__`, `aether.`) stripped and `.` read as `_`, so every spelling
/// of one tool compares equal.
pub fn canonical_tool_name(name: &str) -> Cow<'_, str> {
    let unbridged = ["mcp__aether.", "aether__", "aether."]
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .unwrap_or(name);
    let bare = unbridged
        .strip_prefix("mcp__")
        .and_then(|rest| rest.split_once("__"))
        .map_or(unbridged, |(_server, tool)| tool);
    if bare.contains('.') {
        Cow::Owned(bare.replace('.', "_"))
    } else {
        Cow::Borrowed(bare)
    }
}

/// The fields `tool` seals, or `None` for a tool that seals nothing.
pub fn sealed_fields(tool: &str) -> Option<&'static [&'static str]> {
    let canonical = canonical_tool_name(tool);
    SEALED_TOOLS
        .iter()
        .find(|(name, _)| *name == canonical)
        .map(|(_, fields)| *fields)
}

/// Replace each sealed field present in `input` with [`SEALED`]. Returns
/// whether anything changed; an unsealed tool's input is never touched and
/// a sealed tool's non-object input is sealed whole.
pub fn seal_input(tool: &str, input: &mut Value) -> bool {
    let Some(fields) = sealed_fields(tool) else {
        return false;
    };
    let Some(object) = input.as_object_mut() else {
        // The tool parser also accepts the argument object as JSON text.
        let already_sealed = input.as_str() == Some(SEALED);
        *input = Value::String(SEALED.to_owned());
        return !already_sealed;
    };
    let mut changed = false;
    for field in fields {
        if let Some(value) = object.get_mut(*field)
            && value.as_str() != Some(SEALED)
        {
            *value = Value::String(SEALED.to_owned());
            changed = true;
        }
    }
    debug_assert!(
        fields
            .iter()
            .all(|f| object.get(*f).is_none_or(|v| v == SEALED)),
        "every sealed field present reads as sealed"
    );
    changed
}

/// Key a sealed call over its sealed input. The default key digests the real
/// input, against which a reader could test candidate answers.
pub fn seal_call_key(call: &mut ToolCall) {
    if call.idempotency_key.is_none() || sealed_fields(&call.name).is_none() {
        return;
    }
    let sealed = sealed_view(&call.name, &call.input);
    call.idempotency_key = Some(IdempotencyKey::derive(&call.name, &sealed));
}

/// `input` as it may be shown or logged: sealed when its tool seals anything.
pub(crate) fn sealed_view<'a>(tool: &str, input: &'a Value) -> Cow<'a, Value> {
    if sealed_fields(tool).is_none() {
        return Cow::Borrowed(input);
    }
    let mut sealed = input.clone();
    seal_input(tool, &mut sealed);
    Cow::Owned(sealed)
}

/// Which deltas one invocation's stream publishes. Decided once per stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamSeal {
    /// An agent outside Noesis: only sealed tools' input is withheld.
    #[default]
    Ordinary,
    /// A Noesis agent: its thinking is withheld as well.
    Noesis,
}

impl StreamSeal {
    pub fn of(noesis: bool) -> Self {
        if noesis { Self::Noesis } else { Self::Ordinary }
    }

    /// Whether `delta` may be published at all. Asked before it is numbered,
    /// so a withheld delta leaves no gap in the stream's seqs.
    pub(crate) fn admits(self, delta: &DeltaKind) -> bool {
        match delta {
            DeltaKind::ToolInput { name, .. } => sealed_fields(name).is_none(),
            DeltaKind::Thinking { .. } => self == Self::Ordinary,
            DeltaKind::TurnStarted { .. }
            | DeltaKind::Text { .. }
            | DeltaKind::Accounting { .. }
            | DeltaKind::TurnFinished => true,
        }
    }
}

/// Whether each agent a commit touches belongs to Noesis, read at most once
/// per agent and only when a thinking block makes it matter.
struct NoesisAgents<'a> {
    store: &'a dyn Store,
    known: Vec<(AgentId, bool)>,
}

impl NoesisAgents<'_> {
    fn is_noesis(&mut self, agent: AgentId) -> Result<bool> {
        if let Some((_, noesis)) = self.known.iter().find(|(id, _)| *id == agent) {
            return Ok(*noesis);
        }
        let noesis = self.store.agent_subsystem(agent)?.as_deref() == Some(NOESIS_SUBSYSTEM);
        self.known.push((agent, noesis));
        Ok(noesis)
    }
}

/// Seal a live commit before it is written. An `agent.message` left with no
/// content once its thinking is withheld is dropped: it would carry nothing.
pub(crate) fn seal_commit(store: &dyn Store, commit: &mut Commit) -> Result<()> {
    let mut agents = NoesisAgents {
        store,
        known: Vec::new(),
    };
    let mut emptied = Vec::new();
    for (index, draft) in commit.events.iter_mut().enumerate() {
        if seal_event(&mut agents, draft.agent_id, &mut draft.kind)? {
            emptied.push(index);
        }
    }
    for index in emptied.into_iter().rev() {
        commit.events.remove(index);
    }
    seal_mutations(&mut agents, &mut commit.mutations)
}

/// Seal imported events and mutations before they are written. Imported
/// events keep their ids, so an emptied message is kept rather than dropped.
pub(crate) fn seal_import(
    store: &dyn Store,
    events: &mut [aether_store::ImportedEvent],
    mutations: &mut [Mutation],
) -> Result<()> {
    let mut agents = NoesisAgents {
        store,
        known: Vec::new(),
    };
    for event in events.iter_mut() {
        seal_event(&mut agents, event.draft.agent_id, &mut event.draft.kind)?;
    }
    seal_mutations(&mut agents, mutations)
}

/// Seal one event in place. True when it is an `agent.message` the seal emptied.
fn seal_event(
    agents: &mut NoesisAgents<'_>,
    agent: Option<AgentId>,
    kind: &mut aether_core::event::EventKind,
) -> Result<bool> {
    use aether_core::event::EventKind;
    match kind {
        EventKind::ToolRequested { call, .. } => {
            seal_call_key(call);
            seal_input(&call.name, &mut call.input);
            Ok(false)
        }
        EventKind::AgentMessage { content, .. } => {
            let had_content = !content.is_empty();
            seal_content(agents, agent, content)?;
            Ok(had_content && content.is_empty())
        }
        EventKind::InvocationFailed { partial, .. } => {
            seal_content(agents, agent, partial)?;
            Ok(false)
        }
        // Decided variant by variant, never by `_`: a new event kind does not
        // compile until someone says whether it carries model content.
        EventKind::AgentCreated { .. }
        | EventKind::AgentReady
        | EventKind::AgentStarted { .. }
        | EventKind::AgentTurnCompleted { .. }
        | EventKind::AgentInterrupted { .. }
        | EventKind::AgentWaiting { .. }
        | EventKind::AgentResumed { .. }
        | EventKind::AgentPaused { .. }
        | EventKind::AgentArchived { .. }
        | EventKind::AgentBlocked { .. }
        | EventKind::AgentCompleted { .. }
        | EventKind::AgentFailed { .. }
        | EventKind::AgentRestarted { .. }
        | EventKind::AgentCancelled { .. }
        | EventKind::AgentConfigUpdated { .. }
        | EventKind::AgentUsageUpdated { .. }
        | EventKind::TaskCreated { .. }
        | EventKind::TaskAssigned { .. }
        | EventKind::TaskStarted
        | EventKind::TaskBlocked { .. }
        | EventKind::TaskUnblocked
        | EventKind::TaskRequeued { .. }
        | EventKind::TaskCompleted { .. }
        | EventKind::TaskFailed { .. }
        | EventKind::TaskCancelled { .. }
        | EventKind::ToolStarted { .. }
        | EventKind::ToolSucceeded { .. }
        | EventKind::ToolFailed { .. }
        | EventKind::ToolCancelled { .. }
        | EventKind::ToolDenied { .. }
        | EventKind::ToolReplayed { .. }
        | EventKind::ToolRetried { .. }
        | EventKind::SnapshotCreated { .. }
        | EventKind::InvocationCompleted { .. }
        | EventKind::CompactionRequested { .. }
        | EventKind::CheckpointCreated { .. }
        | EventKind::CompactionCompleted { .. }
        | EventKind::CompactionNative { .. }
        | EventKind::CompactionFailed { .. }
        | EventKind::ContextHandoff { .. }
        | EventKind::MemoryCandidate { .. }
        | EventKind::MemoryAccepted { .. }
        | EventKind::MemoryRejected { .. }
        | EventKind::MemoryForgotten { .. }
        | EventKind::MemoryRestored { .. }
        | EventKind::MemoryPinned { .. }
        | EventKind::MemorySuperseded { .. }
        | EventKind::MessageSent { .. }
        | EventKind::InboxDelivered { .. }
        | EventKind::InboxConsumed { .. }
        | EventKind::QuestionAsked { .. }
        | EventKind::ApprovalRequested { .. }
        | EventKind::ApprovalRuleAdded { .. }
        | EventKind::ApprovalRuleRevoked { .. }
        | EventKind::ApprovalResolved { .. }
        | EventKind::ArtifactCreated { .. }
        | EventKind::ArtifactDeduplicated { .. }
        | EventKind::ArtifactEvicted { .. }
        | EventKind::McpServerConnected { .. }
        | EventKind::McpServerFailed { .. }
        | EventKind::RunImported { .. }
        | EventKind::LeaseGranted { .. }
        | EventKind::LeaseRenewed { .. }
        | EventKind::LeaseReleased
        | EventKind::LeaseExpired { .. }
        | EventKind::ResourceAcquired { .. }
        | EventKind::ResourceReleased { .. }
        | EventKind::ResourceWaiting { .. }
        | EventKind::ProviderBackoff { .. }
        | EventKind::ProviderRecovered { .. }
        | EventKind::ProviderFallback { .. }
        | EventKind::ProviderQuota { .. }
        | EventKind::RecoveryClassified { .. }
        | EventKind::BudgetWarning { .. }
        | EventKind::DeadlinePassed { .. }
        | EventKind::BudgetExceeded { .. }
        | EventKind::RunHardStop { .. }
        | EventKind::DaemonStarted { .. }
        | EventKind::DaemonStopped { .. }
        | EventKind::NoesisSessionCreated { .. }
        | EventKind::NoesisPhaseChanged { .. }
        | EventKind::NoesisQuestionPosed { .. }
        | EventKind::NoesisAnswerRecorded { .. }
        | EventKind::NoesisPlanUpdated { .. }
        | EventKind::NoesisProblemPosed { .. }
        | EventKind::NoesisPracticeConcluded { .. }
        | EventKind::NoesisJobUpdated { .. }
        | EventKind::NoesisGraphMerged { .. }
        | EventKind::NoesisLearnerUpdated { .. }
        | EventKind::NoesisSettingsChanged { .. }
        | EventKind::Custom { .. } => Ok(false),
    }
}

fn seal_mutations(agents: &mut NoesisAgents<'_>, mutations: &mut [Mutation]) -> Result<()> {
    for mutation in mutations.iter_mut() {
        match mutation {
            Mutation::PutSnapshot(snapshot) => {
                seal_messages(agents, snapshot.agent_id, &mut snapshot.messages)?;
            }
            Mutation::PutInvocation(invocation) => {
                seal_content(agents, Some(invocation.agent_id), &mut invocation.content)?;
            }
            // Decided variant by variant, never by `_`, as for events.
            Mutation::RecordEngineSessionOwnership(..)
            | Mutation::UpsertSchedule(..)
            | Mutation::RecordScheduleOccurrence(..)
            | Mutation::UpsertAgent(..)
            | Mutation::CreateAgent { .. }
            | Mutation::UpsertTask(..)
            | Mutation::DeliverInbox(..)
            | Mutation::SetLease(..)
            | Mutation::ClearLease(..)
            | Mutation::PutCheckpoint(..)
            | Mutation::PutApproval(..)
            | Mutation::PutApprovalRule(..)
            | Mutation::RecordToolOutcome(..)
            | Mutation::RecordSubmission(..)
            | Mutation::PutMemory(..)
            | Mutation::PutMemoryAdmission(..)
            | Mutation::RecordMemoryFeedback(..)
            | Mutation::RetractMemoryFeedback { .. }
            | Mutation::PutArtifact(..)
            | Mutation::SetMeta { .. }
            | Mutation::PutDevice(..)
            | Mutation::TouchDevice { .. }
            | Mutation::PutNotification(..)
            | Mutation::PruneResolvedNotifications { .. } => {}
        }
    }
    Ok(())
}

fn seal_messages(
    agents: &mut NoesisAgents<'_>,
    agent: AgentId,
    messages: &mut [Message],
) -> Result<()> {
    for message in messages.iter_mut() {
        seal_content(agents, Some(agent), &mut message.content)?;
    }
    Ok(())
}

/// Seal sealed tools' input in `content`, and drop its thinking when `agent`
/// belongs to Noesis. The subsystem is read only when thinking is present.
fn seal_content(
    agents: &mut NoesisAgents<'_>,
    agent: Option<AgentId>,
    content: &mut Vec<ContentBlock>,
) -> Result<()> {
    let mut thinking = false;
    for block in content.iter_mut() {
        match block {
            ContentBlock::ToolUse { name, input, .. } => {
                seal_input(name, input);
            }
            ContentBlock::Thinking { .. } => thinking = true,
            // Decided variant by variant: a new block kind must say whether
            // it carries a sealed value.
            ContentBlock::Text { .. }
            | ContentBlock::ToolResult { .. }
            | ContentBlock::Image { .. }
            | ContentBlock::ImageRef(_)
            | ContentBlock::Compaction { .. } => {}
        }
    }
    let Some(agent) = agent.filter(|_| thinking) else {
        return Ok(());
    };
    if agents.is_noesis(agent)? {
        content.retain(|block| !matches!(block, ContentBlock::Thinking { .. }));
        debug_assert!(
            !content
                .iter()
                .any(|b| matches!(b, ContentBlock::Thinking { .. })),
            "a Noesis agent's thinking is never written"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
