//! Event → cell folding (SPEC §3.1) and the grouping rules (§7.4): explore
//! merging, wait → waited, backoff → recovered in place, steer preview → user
//! cell, spawn and child-result cells in the parent, the turn separator.

use std::collections::{BTreeSet, HashMap};

use aether_core::agent::{AgentStatusKind, WaitCondition};
use aether_core::approval::RuleScope;
use aether_core::event::{Actor, Event, EventKind};
use aether_core::id::{
    AgentId, ApprovalId, ArtifactId, InboxItemId, InvocationId, ScheduleId, TaskId, ToolCallId,
};
use aether_core::inbox::InboxKind;
use aether_core::model::{ContentBlock, Effort, MessageRole, ModelErrorClass, ModelRef, Usage};
use aether_core::time::Timestamp;
use aether_core::tool::{ApprovalDecision, ToolOutcome};

use super::tools::{self, Delegation, ToolShape, first_line};
use super::{
    AgentTranscript, ApprovalScope, ArtifactLink, CallStatus, CallTarget, Cell, CellId, CellModel,
    CellRevision, ChildOutcome, CompactionState, Cost, DescendantSummary, IngestCtx, MemoryNote,
    NoticeKind, NoticeSeparator, NoticeText, Transcripts, UserFrom, reasoning_title,
};
use crate::app::helpers::sibling_names;

/// What the grouping rules remember between events. Never rendered.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct Private {
    /// Position of each committed cell in `cells`.
    pub index: HashMap<CellId, usize>,
    own_result_cells: HashMap<Option<TaskId>, CellId>,
    delivered: std::collections::HashSet<InboxItemId>,
    /// Tool calls not yet settled, and the cell each one draws into.
    calls: HashMap<ToolCallId, OpenCall>,
    /// What events said about this agent; the tree can lag behind them.
    who: Identity,
    /// Delivered messages awaiting `InboxConsumed`, oldest first.
    inbox: Vec<Unconsumed>,
    /// Approvals this agent asked for: what they would run, and the call.
    approvals: HashMap<ApprovalId, (String, ToolCallId)>,
    /// Turns since the agent last went idle; the separator sums them.
    session: Option<Session>,
    /// This agent's own cumulative spend, as last reported.
    spent: Option<Cost>,
    /// This agent's and its descendants' spend seen so far.
    subtree: Option<Cost>,
    /// The parent's child-result cell for the current run of this agent.
    result_cell: Option<CellId>,
    /// The last wait parked on the inbox alone, which is idling, not waiting.
    idle_wait: bool,
    /// Why the last invocation failed on a spent quota.
    quota: Option<String>,
    delegations: Vec<Delegation>,
    /// Open backoff notice per provider, replaced in place on recovery.
    incidents: HashMap<String, CellId>,
    compaction: Option<CellId>,
    /// Artifact records seen, and the cell linking each, so either may come first.
    artifacts: HashMap<ArtifactId, (String, u64, u64)>,
    artifact_cells: HashMap<ArtifactId, CellId>,
    /// The operator's last approval here: its event seq and record cell.
    approved: Option<(u64, CellId)>,
    /// The invocation whose reply is already drawn, by its message or by a
    /// stream cut short, so its cancelled partial is not drawn again.
    drawn: Option<InvocationId>,
    /// What a stop drew from a stream it cut short, until a message for that
    /// invocation lands: one adding nothing to it is not drawn again.
    cut: Option<CutStream>,
}

/// A delivered message awaiting `InboxConsumed`: who sent it, its text, its
/// pending cell (an agent's message), and when it was delivered.
#[derive(Debug, Clone, PartialEq)]
struct Unconsumed {
    id: InboxItemId,
    from: UserFrom,
    text: String,
    pending: Option<CellId>,
    at: Timestamp,
}

#[derive(Debug, Clone, PartialEq)]
struct CutStream {
    invocation: InvocationId,
    reasoning: String,
    text: String,
}

#[derive(Debug, Clone, PartialEq)]
struct OpenCall {
    name: String,
    cell: CellId,
    /// For hidden calls, the cell is minted only if the call fails.
    hidden: Option<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Identity {
    name: Option<String>,
    role: Option<String>,
    parent: Option<AgentId>,
    created_at: Option<Timestamp>,
    model: Option<ModelRef>,
    effort: Option<Effort>,
    task: Option<TaskId>,
}

#[derive(Debug, Clone, PartialEq)]
struct Session {
    started: Timestamp,
    cost: Option<Cost>,
    agents: BTreeSet<AgentId>,
}

// ----- cell bookkeeping ---------------------------------------------------------

impl AgentTranscript {
    /// A stream for another invocation means the cut one's message never came.
    pub(super) fn retire_cut(&mut self, streaming: InvocationId) {
        self.private.cut.take_if(|cut| cut.invocation != streaming);
        debug_assert!(
            self.private
                .cut
                .as_ref()
                .is_none_or(|cut| cut.invocation == streaming)
        );
    }

    /// A grouped call's own failure, as the call cell only named modes draw.
    fn mode_failure(&mut self, at: Timestamp, name: &str, status: CallStatus, error: &str) {
        self.push_named(
            at,
            CellModel::Called {
                target: CallTarget::Tool {
                    name: name.to_owned(),
                },
                args: String::new(),
                result: String::new(),
                status,
                error: (!error.is_empty()).then(|| error.to_owned()),
                artifacts: Vec::new(),
            },
        );
    }

    fn push_named(&mut self, at: Timestamp, model: CellModel) -> CellId {
        // Supplemental cells must not split the stock view's active tool groups.
        let cell = self.mint(at, model);
        let id = cell.id;
        self.private.index.insert(id, self.cells.len());
        self.cells.push(cell);
        self.named_only.insert(id);
        self.changed(Some(id));
        id
    }

    fn changed(&mut self, cell: Option<CellId>) {
        self.committed = self.committed.wrapping_add(1);
        if self.changes.len() == 256 {
            self.changes.pop_front();
        }
        self.changes.push_back((self.committed, cell));
    }

    /// When the turns the next separator closes began: its `Worked for`
    /// counts from here.
    pub(crate) fn session_started(&self) -> Option<Timestamp> {
        self.private.session.as_ref().map(|s| s.started)
    }

    pub(crate) fn client_errors(&self) -> Vec<Cell> {
        self.cells
            .iter()
            .filter(|cell| {
                matches!(
                    cell.model,
                    CellModel::Notice {
                        kind: NoticeKind::ClientError | NoticeKind::Echo,
                        ..
                    }
                )
            })
            .cloned()
            .collect()
    }

    pub(crate) fn client_error(&mut self, at: Timestamp, text: String) {
        // Local notices have no event to age out with the persisted window.
        let errors = self
            .cells
            .iter()
            .filter(|cell| {
                matches!(
                    cell.model,
                    CellModel::Notice {
                        kind: NoticeKind::ClientError,
                        ..
                    }
                )
            })
            .count();
        if errors >= 64
            && let Some(at) = self.cells.iter().position(|cell| {
                matches!(
                    cell.model,
                    CellModel::Notice {
                        kind: NoticeKind::ClientError,
                        ..
                    }
                )
            })
        {
            let removed = self.cells.remove(at);
            self.private.index.remove(&removed.id);
            for (position, cell) in self.cells.iter().enumerate().skip(at) {
                self.private.index.insert(cell.id, position);
            }
            self.changed(None);
        }
        self.push(
            at,
            CellModel::Notice {
                kind: NoticeKind::ClientError,
                text: text.into(),
            },
        );
    }

    pub(crate) fn echo(&mut self, at: Timestamp, text: String) {
        self.push(
            at,
            CellModel::Notice {
                kind: NoticeKind::Echo,
                text: text.into(),
            },
        );
    }

    pub(crate) fn retain_client_errors(&mut self, errors: Vec<Cell>) {
        for cell in errors {
            self.private.index.insert(cell.id, self.cells.len());
            let id = cell.id;
            self.cells.push(cell);
            self.changed(Some(id));
        }
    }

    /// The name `child` was spawned under here, numbered like its siblings.
    fn spawned_name(&self, child: AgentId) -> Option<String> {
        self.cells.iter().rev().find_map(|cell| match &cell.model {
            CellModel::Spawned { agent, name, .. } if *agent == child => Some(name.clone()),
            _ => None,
        })
    }

    fn mint(&mut self, at: Timestamp, model: CellModel) -> Cell {
        Cell {
            id: self.next_cell_id(),
            revision: CellRevision::default(),
            at,
            model,
        }
    }

    fn commit_active(&mut self) {
        if let Some(cell) = self.active.take() {
            self.private.index.insert(cell.id, self.cells.len());
            let id = cell.id;
            self.cells.push(cell);
            self.changed(Some(id));
        }
    }

    /// Append a committed cell. An open explore group closes; an open wait
    /// stays live below it (child results land while their parent waits).
    fn push(&mut self, at: Timestamp, model: CellModel) -> CellId {
        if !matches!(
            self.active.as_ref().map(|c| &c.model),
            Some(CellModel::Waiting { .. })
        ) {
            self.commit_active();
        }
        let cell = self.mint(at, model);
        let id = cell.id;
        self.private.index.insert(id, self.cells.len());
        self.cells.push(cell);
        self.changed(Some(id));
        id
    }

    /// Commit a cell just above `before`; `before`'s position if it is not committed.
    fn insert_before(&mut self, before: CellId, at: Timestamp, model: CellModel) -> CellId {
        let Some(&pos) = self.private.index.get(&before) else {
            return self.push(at, model);
        };
        let cell = self.mint(at, model);
        let id = cell.id;
        self.cells.insert(pos, cell);
        for (i, cell) in self.cells.iter().enumerate().skip(pos) {
            self.private.index.insert(cell.id, i);
        }
        self.changed(None);
        id
    }

    fn set_active(&mut self, at: Timestamp, model: CellModel) -> CellId {
        self.commit_active();
        let cell = self.mint(at, model);
        let id = cell.id;
        self.active = Some(cell);
        id
    }

    fn cell_mut(&mut self, id: CellId) -> Option<&mut Cell> {
        match self.private.index.get(&id) {
            Some(&at) => self.cells.get_mut(at),
            None => self.active.as_mut().filter(|c| c.id == id),
        }
    }

    /// Change a cell in place; the revision bump invalidates its cached layout.
    fn update(&mut self, id: CellId, change: impl FnOnce(&mut CellModel)) {
        if let Some(cell) = self.cell_mut(id) {
            change(&mut cell.model);
            cell.revision.0 = cell.revision.0.wrapping_add(1);
        }
        if self.private.index.contains_key(&id) {
            self.changed(Some(id));
        }
    }

    /// Whether an `AgentTurnCompleted` going to `next` ends the turn the
    /// operator sees: `Ready` means tool results or a wake run another turn at
    /// once, and a wait on anything but the inbox resumes by itself.
    pub fn turn_over(&self, next: AgentStatusKind) -> bool {
        if let Some(live) = &self.live {
            return live.turn_over(next);
        }
        match next {
            AgentStatusKind::Ready => false,
            AgentStatusKind::Waiting => self.private.idle_wait,
            _ => true,
        }
    }

    fn end_wait(&mut self, at: Timestamp) {
        if let Some(Cell {
            model: CellModel::Waiting { ended, .. },
            revision,
            ..
        }) = &mut self.active
        {
            *ended = Some(at);
            revision.0 = revision.0.wrapping_add(1);
            self.commit_active();
        }
    }

    /// Keep what streamed when a turn stops before its message is committed.
    fn commit_stream(&mut self, at: Timestamp) {
        let stream = self.stream.take();
        self.push_reply(at, &stream.reasoning, &stream.text, stream.reasoning_since);
        let shown = !stream.reasoning.trim().is_empty() || !stream.text.trim().is_empty();
        if let (true, Some(invocation)) = (shown, stream.invocation) {
            self.private.drawn = Some(invocation);
            self.private.cut = Some(CutStream {
                invocation,
                reasoning: stream.reasoning,
                text: stream.text,
            });
        }
    }

    fn push_reply(&mut self, at: Timestamp, reasoning: &str, text: &str, since: Option<Timestamp>) {
        if !reasoning.trim().is_empty() {
            self.push(
                at,
                CellModel::Reasoning {
                    title: reasoning_title(reasoning),
                    text: reasoning.trim().to_owned(),
                    duration_ms: since.map(|s| millis(s, at)),
                },
            );
        }
        if !text.trim().is_empty() {
            self.push(
                at,
                CellModel::Assistant {
                    text: text.trim().to_owned(),
                },
            );
        }
    }

    fn close_questions(&mut self, next: super::QuestionState) {
        let open: Vec<CellId> = self
            .cells
            .iter()
            .filter(|c| {
                matches!(
                    c.model,
                    CellModel::Question {
                        state: crate::app::transcript::QuestionState::Pending,
                        ..
                    }
                )
            })
            .map(|c| c.id)
            .collect();
        for id in open {
            self.update(id, |m| {
                if let CellModel::Question { state, .. } = m {
                    *state = next;
                }
            });
        }
    }

    fn close_calls(&mut self, at: Timestamp) {
        // Agent terminal events can arrive without individual call outcomes.
        let calls: Vec<_> = self.private.calls.keys().copied().collect();
        for call in calls {
            tool_settled(self, at, call, CallStatus::Cancelled, "", None);
        }
        self.private.approvals.clear();
        self.private
            .artifacts
            .retain(|id, _| self.private.artifact_cells.contains_key(id));
        self.commit_active();
    }

    fn close_session(&mut self, at: Timestamp, agent: AgentId, separator: bool) {
        let Some(session) = self.private.session.take() else {
            return;
        };
        if !separator {
            return;
        }
        let agents = session.agents.iter().filter(|a| **a != agent).count() + 1;
        self.push(
            at,
            CellModel::Separator {
                elapsed_ms: millis(session.started, at),
                cost: session.cost,
                agents: u32::try_from(agents).unwrap_or(u32::MAX),
                finished_at: at,
            },
        );
    }
}

fn own_result(t: &mut AgentTranscript, agent: AgentId, event: &Event, look: &Lookup<'_>) {
    let (outcome, result) = match &event.kind {
        EventKind::TaskCompleted { result } => (ChildOutcome::Finished, result.summary.clone()),
        EventKind::AgentCompleted { summary } => {
            (ChildOutcome::Finished, summary.clone().unwrap_or_default())
        }
        EventKind::TaskFailed { error, .. } | EventKind::AgentFailed { error } => (
            ChildOutcome::Failed {
                error: error.clone(),
                quota: t.private.quota.clone(),
            },
            String::new(),
        ),
        EventKind::TaskCancelled { .. } | EventKind::AgentCancelled { .. } => {
            (ChildOutcome::Cancelled, String::new())
        }
        _ => return,
    };
    let task = event.task_id.or(t.private.who.task);
    if let Some(id) = t.private.own_result_cells.get(&task).copied() {
        t.update(id, |model| {
            if let CellModel::ChildResult {
                outcome: old,
                result: text,
                ..
            } = model
            {
                if severity(&outcome) > severity(old) {
                    *old = outcome;
                }
                if text.is_empty() {
                    *text = result;
                }
            }
        });
        return;
    }
    let id = t.push_named(
        event.at,
        CellModel::ChildResult {
            agent,
            name: look.own_name(agent, &t.private.who),
            role: t.private.who.role.clone().unwrap_or_default(),
            outcome,
            result,
            elapsed_ms: 0,
            cost_with_children: None,
            descendants: Vec::new(),
        },
    );
    t.private.own_result_cells.insert(task, id);
}

fn millis(from: Timestamp, to: Timestamp) -> u64 {
    u64::try_from((to - from).num_milliseconds()).unwrap_or(0)
}

fn add_cost(total: Option<Cost>, delta: Option<Cost>) -> Option<Cost> {
    match (total, delta) {
        (None, d) => d,
        (t, None) => t,
        (Some(t), Some(d)) => Some(Cost {
            usd: t.usd + d.usd,
            estimated: t.estimated || d.estimated,
            basis: if t.usd == 0.0 && t.basis == aether_core::model::CostBasis::Unknown {
                d.basis
            } else if d.usd == 0.0 && d.basis == aether_core::model::CostBasis::Unknown {
                t.basis
            } else {
                t.basis.combine(d.basis)
            },
        }),
    }
}

/// A usage's spend: billed plus estimated, marked estimated unless fully billed.
/// `None` when nothing in it was priced at all.
fn cost_of(usage: &Usage) -> Option<Cost> {
    let estimate = usage.estimated_cost_usd;
    if usage.cost_usd == 0.0 && estimate.is_none() && !usage.cost_complete {
        return None;
    }
    Some(Cost {
        usd: usage.cost_usd + estimate.unwrap_or(0.0),
        estimated: estimate.is_some()
            || !usage.cost_complete
            || usage.cost_basis == aether_core::model::CostBasis::ListPrice,
        basis: usage.cost_basis,
    })
}

// ----- names --------------------------------------------------------------------

/// Read-only view of everything but the transcript being written.
struct Lookup<'a> {
    ctx: &'a IngestCtx<'a>,
    others: &'a Transcripts,
}

impl Lookup<'_> {
    fn name(&self, id: AgentId) -> String {
        if let Some(node) = self.ctx.tree.nodes.get(&id) {
            return crate::app::helpers::agent_display_name(self.ctx.tree, node.agent.id)
                .unwrap_or_else(|| node.agent.name.clone());
        }
        self.others
            .get(&id)
            .and_then(|t| t.private.who.name.clone())
            .unwrap_or_else(|| short_id(id))
    }

    /// The transcript being written is out of `others` while it is written.
    fn own_name(&self, agent: AgentId, who: &Identity) -> String {
        match (self.ctx.tree.nodes.get(&agent), &who.name) {
            (Some(_), _) => self.name(agent),
            (None, Some(name)) => name.clone(),
            (None, None) => short_id(agent),
        }
    }

    fn actor(&self, actor: &Actor) -> String {
        match actor {
            Actor::Agent { id } => self.name(*id),
            Actor::User { .. } => "you".into(),
            other => other.label(),
        }
    }

    /// The agent working on `task`, by what events said, then the tree.
    fn task_owner(&self, task: TaskId) -> Option<AgentId> {
        self.others
            .iter()
            .find(|(_, t)| t.private.who.task == Some(task))
            .map(|(id, _)| *id)
            .or_else(|| {
                self.ctx.tree.nodes.iter().find_map(|(id, node)| {
                    let owns = node.task.as_ref().is_some_and(|t| t.id == task)
                        || node.agent.current_task == Some(task);
                    owns.then_some(*id)
                })
            })
    }

    fn waited_on(&self, condition: &WaitCondition, out: &mut Vec<AgentId>) {
        match condition {
            WaitCondition::Task { id } => out.extend(self.task_owner(*id)),
            WaitCondition::MessageFrom { agent } => out.push(*agent),
            WaitCondition::Any { of }
            | WaitCondition::All { of }
            | WaitCondition::AtLeast { of, .. } => {
                for c in of {
                    self.waited_on(c, out);
                }
            }
            _ => {}
        }
    }
}

fn short_id(id: AgentId) -> String {
    let text = id.to_string();
    text.chars().take(12).collect()
}

/// `claude-code/claude-sonnet-5-5` → `sonnet-5.5`.
pub fn model_label(model: &ModelRef) -> String {
    let name = model.model.strip_prefix("claude-").unwrap_or(&model.model);
    let bytes = name.as_bytes();
    let n = bytes.len();
    let versioned = n >= 3
        && bytes[n - 1].is_ascii_digit()
        && bytes[n - 2] == b'-'
        && bytes[n - 3].is_ascii_digit();
    if versioned {
        format!("{}.{}", &name[..n - 2], &name[n - 1..])
    } else {
        name.to_owned()
    }
}

fn text_of(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn thinking_of(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Thinking { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// A delivered message as its cell reads: the text, then a line per image.
fn message_text(content: &[ContentBlock]) -> String {
    let labels = content.iter().filter_map(|block| match block {
        ContentBlock::ImageRef(image) => Some(image.label()),
        _ => None,
    });
    crate::app::with_image_lines(&text_of(content), labels)
}

// ----- routing --------------------------------------------------------------------

/// Route one event to the transcript(s) it belongs to: the agent's own, the
/// parent's for spawn and child-result cells, and the sender's for `Sent`.
/// Returns a quota alert to raise in the footer (SPEC §3.1 "never silent").
pub fn ingest_event(
    transcripts: &mut Transcripts,
    event: &Event,
    ctx: &IngestCtx<'_>,
) -> Option<String> {
    // Historical windows stay still while the board continues consuming live events.
    let reading: Vec<_> = transcripts
        .iter()
        .filter_map(|(id, t)| t.live.is_some().then_some(*id))
        .collect();
    let mut saved = Vec::new();
    for id in reading {
        if let Some(mut transcript) = transcripts.remove(&id) {
            if let Some(mut live) = transcript.live.take() {
                live.queue = std::mem::take(&mut transcript.queue);
                live.steering = std::mem::take(&mut transcript.steering);
                transcripts.insert(id, *live);
            }
            saved.push((id, transcript));
        }
    }
    let alert = route_event(transcripts, event, ctx, true);
    remember_event(transcripts, event);
    bound_history(transcripts, ctx);
    for (id, mut transcript) in saved {
        if let Some(mut live) = transcripts.remove(&id) {
            transcript.queue = std::mem::take(&mut live.queue);
            transcript.steering = std::mem::take(&mut live.steering);
            transcript.live = Some(Box::new(live));
            transcripts.insert(id, transcript);
        }
    }
    alert
}

pub(in crate::app) fn bound_history(transcripts: &mut Transcripts, ctx: &IngestCtx<'_>) {
    let mut trimmed = BTreeSet::new();
    for (id, transcript) in transcripts.iter_mut() {
        let t = transcript.live_state_mut();
        if t.history.len() <= super::HISTORY_WINDOW {
            continue;
        }
        trimmed.insert(*id);
        let remove = t.history.len() - super::HISTORY_WINDOW / 2;
        let cutoff = t.history[remove - 1].last_cell;
        t.history.drain(..remove);
        let active = t.active.as_ref().map(|cell| cell.id);
        let mut pinned: BTreeSet<_> = t.private.calls.values().map(|call| call.cell).collect();
        pinned.extend(t.private.incidents.values().copied());
        pinned.extend(t.private.compaction);
        pinned.extend(active);
        t.cells.retain(|cell| {
            cell.id > cutoff
                || pinned.contains(&cell.id)
                || matches!(
                    cell.model,
                    CellModel::Question {
                        state: super::QuestionState::Pending,
                        ..
                    } | CellModel::Waiting { ended: None, .. }
                        | CellModel::Notice {
                            kind: NoticeKind::ClientError,
                            ..
                        }
                )
        });
        // Split settled groups at window boundaries; open groups keep their identity.
        if active.is_some_and(|id| !t.private.calls.values().any(|call| call.cell == id))
            && t.active
                .as_ref()
                .is_some_and(|cell| matches!(cell.model, CellModel::Explored { active: false, .. }))
        {
            t.commit_active();
        }
        t.private.index = t
            .cells
            .iter()
            .enumerate()
            .map(|(i, cell)| (cell.id, i))
            .collect();
        let retained = &t.private.index;
        let present = |id: CellId| retained.contains_key(&id) || active == Some(id);
        t.named_only.retain(|id| present(*id));
        t.private.own_result_cells.retain(|_, id| present(*id));
        t.private.artifact_cells.retain(|artifact, id| {
            if present(*id) {
                true
            } else {
                t.private.artifacts.remove(artifact);
                false
            }
        });
        if t.private.approved.is_some_and(|(_, id)| !present(id)) {
            t.private.approved = None;
        }
        let oldest = t.history.front().map_or(0, |point| point.seq);
        t.private
            .artifacts
            .retain(|_, (_, _, seq)| *seq >= oldest || !t.private.calls.is_empty());
        t.older_available = true;
        t.changed(None);
    }
    if trimmed.is_empty() {
        return;
    }
    // Child result references belong to their parent's live projection.
    let results: HashMap<_, _> = transcripts
        .iter()
        .filter(|(id, _)| trimmed.contains(id))
        .map(|(id, t)| {
            (
                *id,
                t.live_state()
                    .cells
                    .iter()
                    .filter_map(|cell| match cell.model {
                        CellModel::ChildResult { agent, .. } => Some((agent, cell.id)),
                        _ => None,
                    })
                    .collect::<HashMap<_, _>>(),
            )
        })
        .collect();
    for (id, t) in transcripts.iter_mut() {
        let t = t.live_state_mut();
        let parent = t
            .private
            .who
            .parent
            .or_else(|| ctx.tree.nodes.get(id).and_then(|node| node.agent.parent));
        if t.private.result_cell.is_some()
            && let Some(parent) = parent
            && trimmed.contains(&parent)
        {
            t.private.result_cell = results
                .get(&parent)
                .and_then(|cells| cells.get(id))
                .copied();
        }
    }
}

fn remember_event(transcripts: &mut Transcripts, event: &Event) {
    let sender = match event.kind {
        EventKind::MessageSent {
            from: Actor::Agent { id },
            ..
        } => Some(id),
        _ => None,
    };
    let global = event.agent_id.is_none()
        && matches!(
            event.kind,
            EventKind::ProviderBackoff { .. }
                | EventKind::ProviderRecovered { .. }
                | EventKind::ApprovalRuleAdded { .. }
        );
    for (id, transcript) in transcripts.iter_mut() {
        let next = CellId(transcript.next_id);
        let last = transcript.history.back();
        debug_assert!(last.is_none_or(|last| last.last_cell <= next));
        // A child's event counts toward its parent's window only when it added a
        // cell there, so a busy child cannot trim the parent's own replies away.
        let child_cell =
            event.parent_agent_id == Some(*id) && last.is_none_or(|last| last.last_cell != next);
        if (global || event.agent_id == Some(*id) || child_cell || sender == Some(*id))
            && (event.seq == 0 || last.is_none_or(|last| last.seq < event.seq))
        {
            transcript.history.push_back(super::HistoryPoint {
                seq: event.seq,
                last_cell: next,
            });
        }
    }
}

pub(in crate::app) fn ingest_replay_event(
    transcripts: &mut Transcripts,
    event: &Event,
    ctx: &IngestCtx<'_>,
) -> Option<String> {
    let alert = route_event(transcripts, event, ctx, false);
    remember_event(transcripts, event);
    alert
}

/// The types of a child's events that can change its parent's transcript,
/// directly or through the child state the parent's cells read. History pages
/// take only these from children; `route_event` asserts no other type does.
pub(in crate::app) const PARENT_VISIBLE_CHILD_TYPES: [&str; 14] = [
    "agent.created",        // the spawn cell; the child's name and model
    "agent.config_updated", // a rename or model the parent's cells name
    "agent.started",        // a new run gets its own result cell
    "agent.usage_updated",  // the session's and the result's cost
    "agent.completed",
    "agent.failed",
    "agent.cancelled",
    "task.created",  // the spawn cell's objective; the child's task
    "task.assigned", // which child a wait on a task names
    "task.completed",
    "task.failed",
    "task.cancelled",
    "invocation.failed", // a spent quota, shown on the child's failure
    "message.sent",      // the parent's own message to its child
];

/// What a child's event can change that its parent shows, now or later.
#[derive(Debug, PartialEq)]
struct ParentView {
    parent: Option<ParentFacts>,
    child: ChildFacts,
}

/// The parent's cells, live cell, and the session and spend it renders.
#[derive(Debug, PartialEq)]
struct ParentFacts {
    next_id: u64,
    committed: u64,
    active: Option<(CellId, CellRevision)>,
    session: Option<Session>,
    subtree: Option<Cost>,
}

/// What the parent's result, wait and message cells read from the child.
#[derive(Debug, PartialEq, Default)]
struct ChildFacts {
    who: Identity,
    quota: Option<String>,
    spent: Option<Cost>,
    subtree: Option<Cost>,
    result_cell: Option<CellId>,
}

impl ParentView {
    /// None unless a child emitted `event`. Live eviction of an archived
    /// child drops state that replay keeps; the parent shows neither.
    fn of(transcripts: &Transcripts, event: &Event, evict_archived: bool) -> Option<Self> {
        let (parent, child) = (event.parent_agent_id?, event.agent_id?);
        if evict_archived && matches!(event.kind, EventKind::AgentArchived { archived: true }) {
            return None;
        }
        let parent = transcripts.get(&parent).map(|t| ParentFacts {
            next_id: t.next_id,
            committed: t.committed,
            active: t.active.as_ref().map(|cell| (cell.id, cell.revision)),
            session: t.private.session.clone(),
            subtree: t.private.subtree,
        });
        let child = transcripts
            .get(&child)
            .map_or_else(ChildFacts::default, |t| {
                let p = &t.private;
                ChildFacts {
                    // Every event of the child names its parent: no change alone.
                    who: Identity {
                        parent: None,
                        ..p.who.clone()
                    },
                    quota: p.quota.clone(),
                    spent: p.spent,
                    subtree: p.subtree,
                    result_cell: p.result_cell,
                }
            });
        Some(Self { parent, child })
    }
}

fn route_event(
    transcripts: &mut Transcripts,
    event: &Event,
    ctx: &IngestCtx<'_>,
    evict_archived: bool,
) -> Option<String> {
    let before = cfg!(debug_assertions)
        .then(|| ParentView::of(transcripts, event, evict_archived))
        .flatten();
    let alert = route_to_transcripts(transcripts, event, ctx, evict_archived);
    if let Some(before) = before
        && ParentView::of(transcripts, event, evict_archived).as_ref() != Some(&before)
    {
        let kind = event.kind.type_name();
        debug_assert!(
            PARENT_VISIBLE_CHILD_TYPES.contains(&kind.as_str()),
            "a child's {kind} changed its parent's transcript; history pages leave it out"
        );
    }
    alert
}

fn route_to_transcripts(
    transcripts: &mut Transcripts,
    event: &Event,
    ctx: &IngestCtx<'_>,
    evict_archived: bool,
) -> Option<String> {
    match &event.kind {
        EventKind::ProviderBackoff {
            provider, until, ..
        } => {
            let secs = (*until - event.at).num_seconds().max(0);
            let text = NoticeText::joined(
                &[
                    &format!("{} busy", capitalized(provider)),
                    &format!("retrying in {secs}s"),
                ],
                NoticeSeparator::Dot,
            );
            provider_notice(transcripts, ctx, provider, event.at, Some(text));
            return None;
        }
        EventKind::ProviderRecovered { provider } => {
            provider_notice(transcripts, ctx, provider, event.at, None);
            return None;
        }
        EventKind::ApprovalRuleAdded { rule } if matches!(rule.created_by, Actor::User { .. }) => {
            remembered_answer(transcripts, ctx, event.seq, &rule.scope);
            return None;
        }
        EventKind::ProviderQuota {
            provider, window, ..
        } if window.used_percent >= 100 => {
            let span = window
                .window_label()
                .map(|w| format!("{w} "))
                .unwrap_or_default();
            return Some(format!("{provider} {span}limit reached"));
        }
        _ => {}
    }
    let agent = event.agent_id?;
    if evict_archived && matches!(event.kind, EventKind::AgentArchived { archived: true }) {
        transcripts.remove(&agent);
        return None;
    }
    let mut own = transcripts.remove(&agent).unwrap_or_default();
    if event.seq != 0 && event.seq <= own.max_seq {
        transcripts.insert(agent, own);
        return None;
    }
    own.max_seq = own.max_seq.max(event.seq);
    if own.private.who.parent.is_none() {
        own.private.who.parent = event.parent_agent_id;
    }
    own_event(
        &mut own,
        agent,
        event,
        &Lookup {
            ctx,
            others: transcripts,
        },
    );
    let quota = own.private.quota.clone();
    transcripts.insert(agent, own);
    cross_agent(transcripts, agent, event, ctx);
    match &event.kind {
        EventKind::InvocationFailed { .. } | EventKind::AgentFailed { .. } => quota,
        _ => None,
    }
}

/// Rebind references held by live children when their parent's cells change.
pub(in crate::app) fn replace_transcript(
    transcripts: &mut Transcripts,
    agent: AgentId,
    mut fresh: AgentTranscript,
    ctx: &IngestCtx<'_>,
) {
    if let Some(current) = transcripts
        .get(&agent)
        .map(AgentTranscript::live_state)
        .or(fresh.live.as_deref())
    {
        fresh.private.result_cell = current.private.result_cell;
        // Never the replaced transcript's revision, which find may hold.
        fresh.committed = current.committed;
        fresh.changes.clear();
        fresh.changed(None);
    }
    if fresh.live.is_some() {
        fresh.private.result_cell = None;
        transcripts.insert(agent, fresh);
        return;
    }
    let results: HashMap<_, _> = fresh
        .cells
        .iter()
        .filter_map(|cell| match cell.model {
            CellModel::ChildResult { agent, .. } => Some((agent, cell.id)),
            _ => None,
        })
        .collect();
    for (child, transcript) in transcripts.iter_mut() {
        let transcript = transcript.live_state_mut();
        let parent = transcript
            .private
            .who
            .parent
            .or_else(|| ctx.tree.nodes.get(child).and_then(|node| node.agent.parent));
        if parent == Some(agent) && transcript.private.result_cell.is_some() {
            transcript.private.result_cell = results.get(child).copied();
        }
    }
    transcripts.insert(agent, fresh);
}

/// Backoff and recovery carry no agent: they land with every busy agent on
/// that provider, one cell per incident, replaced in place on recovery.
fn provider_notice(
    transcripts: &mut Transcripts,
    ctx: &IngestCtx<'_>,
    provider: &str,
    at: Timestamp,
    backoff: Option<NoticeText>,
) {
    for (id, t) in transcripts.iter_mut() {
        let model = ctx
            .tree
            .nodes
            .get(id)
            .map(|n| &n.agent.model)
            .or(t.private.who.model.as_ref());
        if model.is_none_or(|m| m.provider != provider) {
            continue;
        }
        let open = t.private.incidents.get(provider).copied();
        match (backoff.clone(), open) {
            (Some(text), Some(cell)) => t.update(cell, |m| *m = backoff_cell(provider, text)),
            (Some(text), None) if t.private.session.is_some() => {
                let cell = t.push(at, backoff_cell(provider, text));
                t.private.incidents.insert(provider.to_owned(), cell);
            }
            (None, Some(cell)) => {
                t.private.incidents.remove(provider);
                t.update(cell, |m| {
                    *m = CellModel::Notice {
                        kind: NoticeKind::Recovered {
                            incident: provider.to_owned(),
                        },
                        text: format!("{} recovered", capitalized(provider)).into(),
                    }
                });
            }
            _ => {}
        }
    }
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn backoff_cell(provider: &str, text: NoticeText) -> CellModel {
    CellModel::Notice {
        kind: NoticeKind::Backoff {
            incident: provider.to_owned(),
        },
        text,
    }
}

// ----- the agent's own transcript ---------------------------------------------------

pub(super) fn own(t: &mut AgentTranscript, event: &Event, ctx: &IngestCtx<'_>) {
    let agent = event.agent_id.unwrap_or_default();
    let empty = Transcripts::new();
    own_event(
        t,
        agent,
        event,
        &Lookup {
            ctx,
            others: &empty,
        },
    );
}

fn own_event(t: &mut AgentTranscript, agent: AgentId, event: &Event, look: &Lookup<'_>) {
    let at = event.at;
    match &event.kind {
        EventKind::AgentCreated {
            role,
            name,
            parent,
            model,
            ..
        } => {
            let who = &mut t.private.who;
            who.name = Some(name.clone());
            who.role = Some(role.0.clone());
            who.parent = parent.or(who.parent);
            who.created_at = Some(at);
            who.model = Some(model.clone());
        }
        EventKind::AgentConfigUpdated {
            name,
            model,
            config,
            ..
        } => config_notice(t, at, name.as_ref(), model.as_ref(), config.effort),
        EventKind::AgentStarted { .. } => {
            t.end_wait(at);
            t.private.result_cell = None;
            t.private.session.get_or_insert_with(|| Session {
                started: at,
                cost: None,
                agents: BTreeSet::new(),
            });
        }
        EventKind::AgentResumed { .. } => t.end_wait(at),
        EventKind::AgentWaiting { condition } => {
            t.private.idle_wait = matches!(condition, WaitCondition::Inbox);
            let mut ids = Vec::new();
            look.waited_on(condition, &mut ids);
            ids.dedup();
            if !ids.is_empty() {
                let on = ids
                    .into_iter()
                    .map(|id| (id, t.spawned_name(id).unwrap_or_else(|| look.name(id))))
                    .collect();
                t.set_active(
                    at,
                    CellModel::Waiting {
                        on,
                        since: at,
                        ended: None,
                    },
                );
            }
        }
        EventKind::AgentTurnCompleted { next, .. } => {
            let done = matches!(next, AgentStatusKind::Completed | AgentStatusKind::Waiting)
                && t.turn_over(*next);
            if done {
                t.commit_active();
                t.close_session(at, agent, true);
            }
        }
        EventKind::AgentInterrupted { .. } => {
            t.end_wait(at);
            t.commit_stream(at);
            let name = look.own_name(agent, &t.private.who);
            t.push(
                at,
                CellModel::Notice {
                    kind: NoticeKind::Interrupted,
                    text: NoticeText::joined(
                        &[
                            "Interrupted",
                            &format!("tell {name} what to do differently"),
                        ],
                        NoticeSeparator::Dot,
                    ),
                },
            );
            t.close_session(at, agent, false);
        }
        EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content,
            invocation,
        } => {
            t.private.drawn = *invocation;
            let thinking = thinking_of(content);
            let text = text_of(content);
            if shown_since_cut(t, *invocation, &thinking, &text) {
                // A stop drew its start; only what streamed after that is new.
                if t.stream.invocation == *invocation {
                    t.commit_stream(at);
                }
                return;
            }
            let since = if *invocation == t.stream.invocation {
                let since = t.stream.reasoning_since;
                t.stream.reset();
                since
            } else {
                None
            };
            t.push_reply(at, &thinking, &text, since);
        }
        EventKind::TaskCreated { objective, .. } => {
            t.private.who.task = event.task_id;
            let from = match &event.actor {
                Actor::User { .. } => UserFrom::Operator,
                other => UserFrom::Task {
                    from: look.actor(other),
                },
            };
            t.push(
                at,
                CellModel::User {
                    from,
                    text: objective.trim().to_owned(),
                },
            );
        }
        EventKind::TaskAssigned { .. } => {
            t.private.who.task = event.task_id.or(t.private.who.task);
        }
        EventKind::InboxDelivered { item } => match &item.kind {
            InboxKind::Message { from, content, .. } => {
                let sender = match from {
                    Actor::User { .. } => UserFrom::Operator,
                    other => UserFrom::Message {
                        from: look.actor(other),
                    },
                };
                if !t.private.delivered.insert(item.id) {
                    return;
                }
                let text = message_text(content);
                let pending = if matches!(sender, UserFrom::Message { .. }) && !text.is_empty() {
                    let cell = t.mint(
                        at,
                        CellModel::User {
                            from: sender.clone(),
                            text: text.clone(),
                        },
                    );
                    let id = cell.id;
                    t.pending_messages.insert(id, cell);
                    t.changed(Some(id));
                    Some(id)
                } else {
                    None
                };
                t.private.inbox.push(Unconsumed {
                    id: item.id,
                    from: sender,
                    text,
                    pending,
                    at,
                });
            }
            InboxKind::ExternalTrigger {
                source, payload, ..
            } => {
                let kind = NoticeKind::Trigger {
                    schedule: schedule_of(source),
                };
                notice(t, at, kind, trigger_text(source, payload))
            }
            _ => {}
        },
        EventKind::InboxConsumed { items, .. } => consume_inbox(t, at, items),
        EventKind::QuestionAsked { question } => {
            t.push(
                at,
                CellModel::Question {
                    asker: agent,
                    asker_name: look.own_name(agent, &t.private.who),
                    text: question.trim().to_owned(),
                    state: crate::app::transcript::QuestionState::Pending,
                    seq: event.seq,
                },
            );
        }
        EventKind::ApprovalRequested {
            approval,
            call,
            tool,
            description,
            argument,
            ..
        } => {
            let what = argument
                .as_deref()
                .map(first_line)
                .filter(|a| !a.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    let d = first_line(description);
                    // Some tools describe a request as its JSON input; the tool's name reads better.
                    if d.is_empty() || d.starts_with(['{', '[']) {
                        tool.clone()
                    } else {
                        d.to_owned()
                    }
                });
            t.private.approvals.insert(*approval, (what, *call));
            if let Some(cell) = own_tool_cell(t, *call) {
                set_status(t, cell, CallStatus::Running, CallStatus::Awaiting);
            }
        }
        EventKind::ToolRequested { call, .. } => {
            tool_requested(t, at, call.id, &call.name, &call.input)
        }
        EventKind::ToolSucceeded { call, outcome } => tool_succeeded(t, at, *call, outcome),
        EventKind::ArtifactCreated { artifact } => artifact_created(t, artifact),
        EventKind::ToolFailed {
            call,
            error,
            duration_ms,
            ..
        } => tool_settled(t, at, *call, CallStatus::Failed, error, *duration_ms),
        EventKind::ToolDenied { call, reason } => {
            tool_settled(t, at, *call, CallStatus::Denied, reason, None)
        }
        EventKind::ToolCancelled { call } => {
            tool_settled(t, at, *call, CallStatus::Cancelled, "", None)
        }
        EventKind::InvocationFailed {
            invocation,
            error,
            class,
            retryable,
            cancelled,
            partial,
            ..
        } => {
            if *cancelled {
                cancelled_partial(t, at, *invocation, partial);
                return;
            }
            let provider = t.private.who.model.as_ref().map(|m| m.provider.clone());
            if matches!(
                class,
                Some(ModelErrorClass::UsageLimit | ModelErrorClass::QuotaExhausted)
            ) {
                let provider = provider.unwrap_or_else(|| "provider".into());
                t.private.quota = Some(format!("{provider} usage limit reached"));
            }
            // A retryable failure is retried; its backoff notice tells that story.
            if *retryable && t.private.quota.is_none() {
                return;
            }
            let thinking = thinking_of(partial);
            let partial = text_of(partial);
            let matching = t.stream.invocation == Some(*invocation);
            if partial.trim().is_empty() && thinking.trim().is_empty() {
                if matching {
                    t.commit_stream(at);
                }
            } else {
                if matching {
                    t.stream.reset();
                }
                t.push_reply(at, &thinking, &partial, None);
            }
            let hint = match t.private.quota {
                Some(_) => NoticeText::joined(&["/model to switch", "/wake"], NoticeSeparator::Dot),
                None => "/wake to retry".into(),
            };
            error_cell(t, at, error, Some(hint));
        }
        EventKind::CompactionRequested { .. } => {
            let cell = t.push(at, CellModel::Compaction(CompactionState::Active));
            t.private.compaction = Some(cell);
        }
        EventKind::CompactionCompleted {
            before_tokens,
            after_tokens,
            ..
        } => {
            let done = CompactionState::Done {
                before: before_tokens.map(u64::from),
                after: after_tokens.map(u64::from),
                by: None,
                duration_ms: None,
            };
            settle_compaction(t, at, done);
        }
        EventKind::CompactionNative {
            provider,
            before_tokens,
            after_tokens,
            duration_ms,
            ..
        } => {
            let done = CompactionState::Done {
                before: *before_tokens,
                after: *after_tokens,
                by: Some(provider.clone()),
                duration_ms: *duration_ms,
            };
            settle_compaction(t, at, done);
        }
        EventKind::CompactionFailed { error, .. } => {
            let failed = CompactionState::Failed {
                error: first_line(error).to_owned(),
            };
            settle_compaction(t, at, failed);
        }
        EventKind::ContextHandoff { .. } => notice(
            t,
            at,
            NoticeKind::Handoff,
            NoticeText::joined(
                &["Continued in a fresh context", "summary carried over"],
                NoticeSeparator::Dot,
            ),
        ),
        EventKind::ProviderFallback { to, .. } => notice(
            t,
            at,
            NoticeKind::Fallback,
            format!("Fell back to {}", model_label(to)).into(),
        ),
        EventKind::BudgetWarning { reason, .. } => {
            notice(t, at, NoticeKind::BudgetWarning, first_line(reason).into())
        }
        EventKind::BudgetExceeded { reason, .. } => {
            error_cell(t, at, &format!("Budget exceeded: {reason}"), None)
        }
        EventKind::RunHardStop {
            reason,
            limit,
            interrupted,
            ..
        } => error_cell(
            t,
            at,
            &format!("Run hard stop: {reason} ({limit}); {interrupted} running turns interrupted"),
            None,
        ),
        EventKind::McpServerFailed { server, error } => notice(
            t,
            at,
            NoticeKind::McpFailed,
            NoticeText::joined(
                &[&format!("MCP server {server} failed"), first_line(error)],
                NoticeSeparator::Dot,
            ),
        ),
        EventKind::AgentFailed { error } => {
            own_result(t, agent, event, look);
            t.end_wait(at);
            t.close_calls(at);
            t.commit_stream(at);
            t.close_questions(super::QuestionState::AskerEnded);
            let name = look.own_name(agent, &t.private.who);
            let hint = NoticeText::joined(
                &[&format!("/wake {name}"), &format!("/restart {name}")],
                NoticeSeparator::Dot,
            );
            error_cell(t, at, error, Some(hint));
            t.close_session(at, agent, false);
        }
        EventKind::AgentCancelled { reason } => {
            own_result(t, agent, event, look);
            t.end_wait(at);
            t.close_calls(at);
            t.commit_stream(at);
            t.close_questions(super::QuestionState::AskerEnded);
            let text = match reason.as_deref().map(first_line) {
                Some(r) if !r.is_empty() => {
                    NoticeText::joined(&["Cancelled", r], NoticeSeparator::Dot)
                }
                _ => "Cancelled".into(),
            };
            notice(t, at, NoticeKind::Cancelled, text);
            t.close_session(at, agent, false);
        }
        EventKind::TaskCompleted { .. }
        | EventKind::TaskFailed { .. }
        | EventKind::TaskCancelled { .. } => own_result(t, agent, event, look),
        EventKind::AgentCompleted { .. } => {
            own_result(t, agent, event, look);
            t.end_wait(at);
            t.close_calls(at);
            t.close_questions(super::QuestionState::AskerEnded);
        }
        EventKind::Custom { name, .. } if name == aether_core::note::NOTE_EVENT => {
            if let Some(note) = aether_core::note::Note::of(&event.kind) {
                let mut parts = vec![note.text.trim()];
                parts.extend(note.links.iter().map(String::as_str));
                let text = NoticeText::joined(&parts, NoticeSeparator::Dot);
                notice(t, at, NoticeKind::Note(note.level), text);
            }
        }
        EventKind::Custom { name, payload } if name == "store.write_failed" => {
            // Recorded after writes resume: an outage notice is history, not a live hold.
            let during = payload["during"].as_str().unwrap_or("a write");
            let refused = payload["refused_writes"].as_u64().unwrap_or(1);
            let error = payload["error"].as_str().unwrap_or("unknown error");
            notice(
                t,
                at,
                NoticeKind::Store,
                format!("the disk refused {refused} write(s) for {during}: {error}").into(),
            );
        }
        EventKind::Custom { name, payload } if name == "tool.interrupted" => {
            if let (Ok(call), Some(cause)) = (
                serde_json::from_value::<ToolCallId>(payload["call"].clone()),
                payload["cause"].as_str(),
            ) {
                let visible = t
                    .private
                    .calls
                    .get(&call)
                    .is_some_and(|open| open.hidden.is_none());
                tool_settled(
                    t,
                    at,
                    call,
                    CallStatus::Cancelled,
                    &format!("interrupted ({cause})"),
                    None,
                );
                // A deadline can end the agent before its engine finishes cleanup.
                if !visible {
                    let tool = payload["tool"].as_str().unwrap_or("Tool call");
                    notice(
                        t,
                        at,
                        NoticeKind::Engine,
                        format!("{tool} interrupted ({cause})").into(),
                    );
                }
            }
        }
        EventKind::Custom { name, payload } if name == "engine.interrupted" => {
            let engine = payload["engine"].as_str().unwrap_or("engine");
            let text = match payload["reason"].as_str() {
                Some(reason) => NoticeText::joined(
                    &[&format!("{engine} interrupted"), reason],
                    NoticeSeparator::Dot,
                ),
                None => format!("{engine} interrupted").into(),
            };
            notice(t, at, NoticeKind::Engine, text);
        }
        EventKind::Custom { name, payload } if name == "engine.transient_retry" => {
            let engine = payload["engine"].as_str().unwrap_or("engine");
            let class = payload["class"].as_str().unwrap_or("transient");
            let waiting_for = payload["waiting_for"].as_str().unwrap_or("provider");
            let attempt = payload["attempt"].as_u64().unwrap_or(0);
            let secs = payload["delay_ms"].as_u64().unwrap_or(0).div_ceil(1000);
            let text = NoticeText::joined(
                &[
                    &format!("{engine} hit a {class} error"),
                    &format!("waiting for {waiting_for}"),
                    &format!("retry {attempt} in {secs}s"),
                ],
                NoticeSeparator::Dot,
            );
            notice(t, at, NoticeKind::Engine, text);
        }
        EventKind::Custom { name, payload } if name == "agent.stalled" => {
            let engine = payload["engine"].as_str().unwrap_or("engine");
            let silent = payload["silent_secs"].as_u64().unwrap_or(0);
            let attempt = payload["attempt"].as_u64().unwrap_or(0);
            let text = NoticeText::joined(
                &[
                    &format!("{engine} stalled"),
                    &format!("silent for {silent}s"),
                    &format!("attempt {attempt}"),
                ],
                NoticeSeparator::Dot,
            );
            notice(t, at, NoticeKind::Engine, text);
        }
        EventKind::Custom { name, payload } if name == "engine.kill_switch_bypassed" => {
            let engine = payload["engine"].as_str().unwrap_or("engine");
            let tool = payload["tool"].as_str().unwrap_or("a native tool");
            let text = NoticeText::joined(
                &[
                    &format!("{engine} reached for {tool} natively, bypassing the kill switch"),
                    "stopped",
                ],
                NoticeSeparator::Dot,
            );
            notice(t, at, NoticeKind::Engine, text);
        }
        _ => {}
    }
}

/// A stop drew this invocation's stream before its message landed, as a
/// sweep does. The message repeats it when it holds nothing the live view
/// has not shown: the part the stop drew, then any tail streamed since.
fn shown_since_cut(
    t: &mut AgentTranscript,
    invocation: Option<InvocationId>,
    thinking: &str,
    text: &str,
) -> bool {
    let Some(cut) = t
        .private
        .cut
        .take_if(|cut| Some(cut.invocation) == invocation)
    else {
        return false;
    };
    debug_assert_eq!(t.private.drawn, invocation, "the message is drawn first");
    debug_assert!(
        !cut.reasoning.trim().is_empty() || !cut.text.trim().is_empty(),
        "only a stop that drew something can have cut a stream"
    );
    let tail = (t.stream.invocation == invocation).then_some(&t.stream);
    let shown = |part: &str, drawn: &str, tail: Option<&str>| {
        part.trim().is_empty() || part.trim() == format!("{drawn}{}", tail.unwrap_or("")).trim()
    };
    shown(thinking, &cut.reasoning, tail.map(|s| s.reasoning.as_str()))
        && shown(text, &cut.text, tail.map(|s| s.text.as_str()))
}

fn cancelled_partial(
    t: &mut AgentTranscript,
    at: Timestamp,
    invocation: InvocationId,
    partial: &[ContentBlock],
) {
    draw_cancelled_partial(t, at, invocation, partial);
    // The invocation's last record: no message for it follows to read the cut.
    t.private.cut.take_if(|cut| cut.invocation == invocation);
}

fn draw_cancelled_partial(
    t: &mut AgentTranscript,
    at: Timestamp,
    invocation: InvocationId,
    partial: &[ContentBlock],
) {
    if t.private.drawn == Some(invocation) {
        // A stop drew this stream already; what streamed after it is its tail.
        if t.stream.invocation == Some(invocation) {
            t.commit_stream(at);
        }
        return;
    }
    let thinking = thinking_of(partial);
    let text = text_of(partial);
    if thinking.trim().is_empty() && text.trim().is_empty() {
        return;
    }
    if t.stream.invocation == Some(invocation) {
        t.stream.reset();
    }
    t.push_reply(at, &thinking, &text, None);
    t.private.drawn = Some(invocation);
    debug_assert!(
        t.stream.invocation != Some(invocation),
        "a drawn partial left its stream live, so the stream would draw it again"
    );
}

/// Prefer an object's summary; details retain the same bounded JSON preview.
fn trigger_text(source: &str, payload: &serde_json::Value) -> NoticeText {
    let source = trigger_source(source);
    if let Some(summary) = payload
        .as_object()
        .and_then(|object| object.get("summary"))
        .and_then(serde_json::Value::as_str)
        .filter(|summary| !summary.trim().is_empty())
    {
        let summary = trigger_preview(summary.trim(), 4_096)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let head = format!("Trigger from {source}");
        let text = if summary.is_empty() {
            head.into()
        } else {
            NoticeText::joined(&[&head, &summary], NoticeSeparator::Dot)
        };
        return text.with_detail_payload(trigger_preview(&payload.to_string(), 4_096));
    }
    let payload = match payload {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(text) => trigger_preview(text.trim(), 4_096),
        other => trigger_preview(&other.to_string(), 4_096),
    };
    if payload.is_empty() {
        return format!("Trigger from {source}").into();
    }
    NoticeText::joined(
        &[&format!("Trigger from {source}"), &payload],
        NoticeSeparator::Dot,
    )
}

/// A trigger's source, or its schedule's name, as one safe and bounded line.
pub(super) fn trigger_source(text: &str) -> String {
    trigger_preview(text, 256)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The schedule behind a delivery: the runtime sends its triggers from `schedule:<id>`.
pub fn schedule_of(source: &str) -> Option<ScheduleId> {
    source.strip_prefix("schedule:")?.parse().ok()
}

fn trigger_preview(text: &str, limit: usize) -> String {
    let prefix = utf8_prefix(text, limit);
    let clean = crate::ui::cells::text::clean(prefix);
    let clean = aether_core::text::strip_controls(&clean);
    let visible = utf8_prefix(&clean, limit);
    let mut preview = visible.to_owned();
    if prefix.len() < text.len() || visible.len() < clean.len() {
        preview.push_str(" [truncated]");
    }
    preview
}

fn utf8_prefix(text: &str, limit: usize) -> &str {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn notice(t: &mut AgentTranscript, at: Timestamp, kind: NoticeKind, text: NoticeText) {
    t.push(at, CellModel::Notice { kind, text });
}

/// Finish the compaction cell a request opened, or add one when none is open.
fn settle_compaction(t: &mut AgentTranscript, at: Timestamp, state: CompactionState) {
    debug_assert!(state != CompactionState::Active, "only an outcome settles");
    match t.private.compaction.take() {
        Some(cell) => t.update(cell, |m| *m = CellModel::Compaction(state)),
        None => {
            t.push(at, CellModel::Compaction(state));
        }
    }
}

/// `✗ message`, `└ cause`: the first line is the message, the rest its cause.
fn error_cell(t: &mut AgentTranscript, at: Timestamp, error: &str, hint: Option<NoticeText>) {
    let error = error.trim();
    let (message, cause) = match error.split_once('\n') {
        Some((head, rest)) if !rest.trim().is_empty() => (head.trim(), Some(rest.trim())),
        _ => (error, None),
    };
    t.push(
        at,
        CellModel::Error {
            message: message.to_owned(),
            cause: cause.map(str::to_owned),
            hint,
        },
    );
}

fn config_notice(
    t: &mut AgentTranscript,
    at: Timestamp,
    name: Option<&String>,
    model: Option<&ModelRef>,
    effort: Option<Effort>,
) {
    let who = &mut t.private.who;
    if let Some(name) = name {
        who.name = Some(name.clone());
    }
    let model_changed = model.is_some_and(|m| who.model.as_ref().is_some_and(|old| old != m));
    let effort_changed = effort.is_some() && who.effort.is_some() && effort != who.effort;
    if let Some(model) = model {
        who.model = Some(model.clone());
    }
    who.effort = effort.or(who.effort);
    if !(model_changed || effort_changed) {
        return;
    }
    let label = who.model.as_ref().map(model_label).unwrap_or_default();
    let effort = who.effort.map(|e| format!(" {e}")).unwrap_or_default();
    notice(
        t,
        at,
        NoticeKind::ModelChange,
        NoticeText::joined(
            &["Model", &format!("{label}{effort} from the next turn")],
            NoticeSeparator::Arrow,
        ),
    );
}

/// Consumed messages become user cells where they landed; the operator's
/// retire their steering preview and answer open questions.
fn consume_inbox(t: &mut AgentTranscript, at: Timestamp, items: &[InboxItemId]) {
    for id in items {
        let Some(pos) = t.private.inbox.iter().position(|item| item.id == *id) else {
            continue;
        };
        let Unconsumed {
            from,
            text,
            pending,
            ..
        } = t.private.inbox.remove(pos);
        if from == UserFrom::Operator {
            retire_steer(t, &text, at);
            t.close_questions(super::QuestionState::Answered);
        }
        if let Some(id) = pending {
            if let Some(cell) = t.pending_messages.remove(&id) {
                t.commit_active();
                t.private.index.insert(id, t.cells.len());
                t.cells.push(cell);
                t.changed(Some(id));
            }
        } else if !text.is_empty() {
            t.push(at, CellModel::User { from, text });
        }
    }
}

/// Steering previews for the operator's delivered messages no preview shows
/// yet, as a client that sent them would show: a reattached one saw none sent.
/// Previews stay on the displayed transcript; an older window keeps the inbox
/// in its live state.
pub(in crate::app) fn restore_steering(t: &mut AgentTranscript) {
    let inbox = match t.live.as_deref() {
        Some(live) => &live.private.inbox,
        None => &t.private.inbox,
    };
    let mut shown = vec![false; t.steering.len()];
    let mut restored = Vec::new();
    for item in inbox {
        if item.from != UserFrom::Operator || item.text.is_empty() {
            continue;
        }
        let preview = t
            .steering
            .iter()
            .zip(shown.iter_mut())
            .find(|(steer, taken)| !**taken && steer.text.trim() == item.text.trim());
        match preview {
            Some((_, taken)) => *taken = true,
            None => restored.push(super::Steer {
                text: item.text.clone(),
                sent_at: item.at,
            }),
        }
    }
    let waiting = inbox
        .iter()
        .filter(|item| item.from == UserFrom::Operator && !item.text.is_empty())
        .count();
    t.steering.extend(restored);
    debug_assert!(
        t.steering.len() >= waiting,
        "every message the operator is still waiting on has a preview"
    );
}

/// The preview with the same text, else the oldest one sent before `at`.
fn retire_steer(t: &mut AgentTranscript, text: &str, at: Timestamp) {
    let exact = t.steering.iter().position(|s| s.text.trim() == text);
    let oldest = || t.steering.iter().position(|s| s.sent_at <= at);
    if let Some(pos) = exact.or_else(oldest) {
        t.steering.remove(pos);
    }
}

// ----- tool calls ---------------------------------------------------------------

fn tool_requested(
    t: &mut AgentTranscript,
    at: Timestamp,
    call: ToolCallId,
    name: &str,
    input: &serde_json::Value,
) {
    let cell = match tools::classify(name, input) {
        ToolShape::Exec { command, is_search } => t.push(
            at,
            CellModel::Exec {
                call: Some(call),
                command,
                status: CallStatus::Running,
                output: String::new(),
                exit: None,
                duration_ms: None,
                is_search,
                artifacts: Vec::new(),
            },
        ),
        ToolShape::Explore(item) => match &mut t.active {
            Some(Cell {
                id,
                revision,
                model: CellModel::Explored { items, active },
                ..
            }) => {
                merge_explore(items, item);
                *active = true;
                revision.0 = revision.0.wrapping_add(1);
                *id
            }
            _ => t.set_active(
                at,
                CellModel::Explored {
                    items: vec![item],
                    active: true,
                },
            ),
        },
        ToolShape::Edit(files) => t.push(
            at,
            CellModel::Edited {
                files,
                status: CallStatus::Running,
            },
        ),
        ToolShape::Call { target, args } => t.push(
            at,
            CellModel::Called {
                target,
                args,
                result: String::new(),
                status: CallStatus::Running,
                error: None,
                artifacts: Vec::new(),
            },
        ),
        ToolShape::Remember { text } => {
            t.push(at, CellModel::Memory(MemoryNote::Remembered { text }))
        }
        ToolShape::Hidden { name, args } => {
            if name == "delegate" {
                t.private.delegations.push(tools::delegation(input));
            }
            t.private.calls.insert(
                call,
                OpenCall {
                    name: name.clone(),
                    cell: CellId::default(),
                    hidden: Some((name, args)),
                },
            );
            return;
        }
    };
    t.private.calls.insert(
        call,
        OpenCall {
            name: name.into(),
            cell,
            hidden: None,
        },
    );
}

/// Reads merge into the last Read row with names deduped; a search or list
/// adds its own row unless it repeats the last one.
fn merge_explore(items: &mut Vec<super::ExploreItem>, item: super::ExploreItem) {
    if let Some(last) = items.last_mut()
        && last.verb == item.verb
        && last.verb == super::ExploreVerb::Read
    {
        for target in item.targets {
            if !last.targets.contains(&target) {
                last.targets.push(target);
            }
        }
        return;
    }
    if items.last() != Some(&item) {
        items.push(item);
    }
}

fn tool_succeeded(t: &mut AgentTranscript, at: Timestamp, call: ToolCallId, outcome: &ToolOutcome) {
    let Some(open) = t.private.calls.remove(&call) else {
        return;
    };
    let text = tools::tool_text(&outcome.content);
    if open.hidden.is_some() {
        if outcome.is_error {
            hidden_failed(t, at, open, &text.body);
        }
        return;
    }
    if outcome.is_error
        && matches!(
            t.cell(open.cell).map(|c| &c.model),
            Some(CellModel::Explored { .. } | CellModel::Edited { .. } | CellModel::Memory(_))
        )
    {
        t.mode_failure(at, &open.name, CallStatus::Failed, &text.body);
    }
    let still_open = t.private.calls.values().any(|c| c.cell == open.cell);
    let mut ids = outcome.artifacts.clone();
    ids.extend(text.artifact.filter(|id| !outcome.artifacts.contains(id)));
    let links = artifact_links(t, open.cell, &ids);
    t.update(open.cell, |model| match model {
        CellModel::Exec {
            status,
            output,
            exit,
            duration_ms,
            is_search,
            artifacts,
            ..
        } => {
            *artifacts = links;
            let (body, code) = tools::exec_result(&text.body, outcome.structured.as_ref());
            let no_match = *is_search && code == Some(1);
            *status = if outcome.is_error && !no_match {
                CallStatus::Failed
            } else {
                CallStatus::Succeeded
            };
            *output = body;
            *exit = code;
            *duration_ms = Some(outcome.duration_ms);
        }
        CellModel::Explored { active, .. } => *active = still_open,
        CellModel::Edited { files, status } => {
            tools::refine_edits(files, outcome);
            *status = if outcome.is_error {
                CallStatus::Failed
            } else {
                CallStatus::Succeeded
            };
        }
        CellModel::Called {
            result,
            status,
            error,
            artifacts,
            ..
        } => {
            *artifacts = links;
            if outcome.is_error {
                *status = CallStatus::Failed;
                *error = Some(text.body);
            } else {
                *status = CallStatus::Succeeded;
                *result = text.body;
            }
        }
        _ => {}
    });
}

/// The cell drawing `call` alone (exec, call or edit), if it has one: hidden
/// calls have none, and an explore group is shared.
fn own_tool_cell(t: &AgentTranscript, call: ToolCallId) -> Option<CellId> {
    let open = t.private.calls.get(&call).filter(|c| c.hidden.is_none())?;
    let shared = t
        .private
        .calls
        .iter()
        .any(|(id, c)| *id != call && c.cell == open.cell);
    let model = &t.cell(open.cell)?.model;
    let own = matches!(
        model,
        CellModel::Exec { .. } | CellModel::Called { .. } | CellModel::Edited { .. }
    );
    (own && !shared).then_some(open.cell)
}

fn set_status(t: &mut AgentTranscript, cell: CellId, from: CallStatus, to: CallStatus) {
    t.update(cell, |model| {
        if let CellModel::Exec { status, .. }
        | CellModel::Called { status, .. }
        | CellModel::Edited { status, .. } = model
            && *status == from
        {
            *status = to;
        }
    });
}

/// Links for a call's artifacts, named from records already seen; the rest
/// are named by [`artifact_created`] when theirs arrive.
fn artifact_links(t: &mut AgentTranscript, cell: CellId, ids: &[ArtifactId]) -> Vec<ArtifactLink> {
    ids.iter()
        .map(|id| {
            t.private.artifact_cells.insert(*id, cell);
            let known = t.private.artifacts.get(id);
            ArtifactLink {
                id: *id,
                name: known.map(|(name, _, _)| name.clone()),
                size_bytes: known.map(|(_, size, _)| *size),
            }
        })
        .collect()
}

fn artifact_created(t: &mut AgentTranscript, artifact: &aether_core::artifact::Artifact) {
    let (name, size) = (artifact.name.clone(), artifact.size_bytes);
    t.private
        .artifacts
        .insert(artifact.id, (name.clone(), size, t.max_seq));
    let Some(cell) = t.private.artifact_cells.get(&artifact.id).copied() else {
        return;
    };
    t.update(cell, |model| {
        if let CellModel::Exec { artifacts, .. } | CellModel::Called { artifacts, .. } = model
            && let Some(link) = artifacts.iter_mut().find(|l| l.id == artifact.id)
        {
            link.name = Some(name);
            link.size_bytes = Some(size);
        }
    });
}

fn tool_settled(
    t: &mut AgentTranscript,
    at: Timestamp,
    call: ToolCallId,
    settled: CallStatus,
    reason: &str,
    duration: Option<u64>,
) {
    let Some(open) = t.private.calls.remove(&call) else {
        return;
    };
    let reason = tools::tool_text(reason).body;
    if open.hidden.is_some() {
        if settled == CallStatus::Failed {
            hidden_failed(t, at, open, &reason);
        }
        return;
    }
    if matches!(
        t.cell(open.cell).map(|c| &c.model),
        Some(CellModel::Explored { .. } | CellModel::Edited { .. } | CellModel::Memory(_))
    ) {
        t.mode_failure(at, &open.name, settled, &reason);
    }
    let still_open = t.private.calls.values().any(|c| c.cell == open.cell);
    t.update(open.cell, |model| match model {
        CellModel::Exec {
            status,
            output,
            duration_ms,
            ..
        } => {
            *status = settled;
            if !reason.is_empty() {
                *output = reason;
            }
            *duration_ms = duration;
        }
        CellModel::Explored { active, .. } => *active = still_open,
        CellModel::Edited { status, .. } => *status = settled,
        CellModel::Called { status, error, .. } => {
            *status = settled;
            *error = (!reason.is_empty()).then_some(reason);
        }
        _ => {}
    });
}

/// A harness call another event would have drawn failed: show it, never hide it.
fn hidden_failed(t: &mut AgentTranscript, at: Timestamp, open: OpenCall, error: &str) {
    let Some((name, args)) = open.hidden else {
        return;
    };
    t.push(
        at,
        CellModel::Called {
            target: CallTarget::Tool { name },
            args,
            result: String::new(),
            status: CallStatus::Failed,
            error: Some(error.trim().to_owned()),
            artifacts: Vec::new(),
        },
    );
}

// ----- other agents' transcripts ------------------------------------------------------

/// The board's name for a new child, so same-named siblings read `worker`,
/// `worker 2` alike; a child the tree has not seen yet counts earlier spawns.
fn sibling_name(
    parent: &AgentTranscript,
    ctx: &IngestCtx<'_>,
    child: AgentId,
    name: &str,
) -> String {
    if ctx.tree.nodes.contains_key(&child) {
        return sibling_names(ctx.tree)
            .remove(&child)
            .unwrap_or_else(|| name.to_owned());
    }
    let taken: BTreeSet<&str> = parent
        .cells
        .iter()
        .filter_map(|cell| match &cell.model {
            CellModel::Spawned { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    if !taken.contains(name) {
        return name.to_owned();
    }
    let mut n = 2;
    while taken.contains(format!("{name} {n}").as_str()) {
        n += 1;
    }
    format!("{name} {n}")
}

fn cross_agent(transcripts: &mut Transcripts, agent: AgentId, event: &Event, ctx: &IngestCtx<'_>) {
    let at = event.at;
    match &event.kind {
        EventKind::AgentCreated {
            parent: Some(parent),
            name,
            role,
            model,
            ..
        } => {
            let from_tree = ctx
                .tree
                .nodes
                .get(&agent)
                .and_then(|n| n.agent.config.effort);
            let parent_t = transcripts.entry(*parent).or_default();
            let delegated = parent_t
                .private
                .delegations
                .iter()
                .position(|d| d.name.as_deref() == Some(name.as_str()))
                .or_else(|| {
                    parent_t
                        .private
                        .delegations
                        .iter()
                        .position(|d| d.name.is_none() && d.role == role.0)
                })
                .map(|pos| parent_t.private.delegations.remove(pos));
            let shown = sibling_name(parent_t, ctx, agent, name);
            parent_t.push(
                at,
                CellModel::Spawned {
                    agent,
                    name: shown,
                    role: role.0.clone(),
                    model: model_label(model),
                    effort: from_tree.or(delegated.and_then(|d| d.effort)),
                    objective: String::new(),
                },
            );
            if let Some(child) = transcripts.get_mut(&agent) {
                child.private.who.effort = from_tree.or(child.private.who.effort);
            }
        }
        EventKind::TaskCreated { objective, .. } => {
            let Some(child) = transcripts.get(&agent) else {
                return;
            };
            let Some(parent) = child.private.who.parent.or_else(|| {
                ctx.tree
                    .nodes
                    .get(&agent)
                    .and_then(|node| node.agent.parent)
            }) else {
                return;
            };
            if let Some(parent_t) = transcripts.get_mut(&parent) {
                let cell = parent_t
                    .cells
                    .iter()
                    .rev()
                    .find_map(|cell| match cell.model {
                        CellModel::Spawned { agent: spawned, .. } if spawned == agent => {
                            Some(cell.id)
                        }
                        _ => None,
                    });
                let Some(cell) = cell else {
                    return;
                };
                parent_t.update(cell, |m| {
                    if let CellModel::Spawned { objective: o, .. } = m {
                        *o = objective.trim().to_owned();
                    }
                });
            }
        }
        EventKind::AgentUsageUpdated { total, turn_usage } => {
            spend(transcripts, ctx, agent, total, turn_usage)
        }
        EventKind::MessageSent {
            from: Actor::Agent { id: sender },
            to,
            content,
            ..
        } => {
            let to_name = Lookup {
                ctx,
                others: transcripts,
            }
            .name(*to);
            let text = text_of(content).trim().to_owned();
            let t = transcripts.entry(*sender).or_default();
            if let Some(Cell {
                id,
                model: CellModel::Sent { text: last, .. },
                ..
            }) = t
                .cells
                .iter()
                .rev()
                .find(|cell| !t.named_only.contains(&cell.id))
                && *last == text
            {
                let id = *id;
                t.update(id, |m| {
                    if let CellModel::Sent { to, .. } = m {
                        to.push_str(", ");
                        to.push_str(&to_name);
                    }
                });
            } else {
                t.push(at, CellModel::Sent { to: to_name, text });
            }
        }
        EventKind::ApprovalResolved {
            approval,
            decision,
            by,
            ..
        } => {
            let owner = transcripts
                .iter()
                .find(|(_, t)| t.private.approvals.contains_key(approval))
                .map(|(id, _)| *id);
            let Some(t) = owner.and_then(|id| transcripts.get_mut(&id)) else {
                return;
            };
            let Some((what, call)) = t.private.approvals.remove(approval) else {
                return;
            };
            let approved = *decision == ApprovalDecision::Approved;
            let tool_cell = own_tool_cell(t, call);
            if approved && let Some(cell) = tool_cell {
                set_status(t, cell, CallStatus::Awaiting, CallStatus::Running);
            }
            // A rule or yolo answered: nothing to record (§3.1).
            if !matches!(by, Actor::User { .. }) {
                return;
            }
            let record = CellModel::ApprovalRecord {
                approval: *approval,
                approved,
                what,
                scope: ApprovalScope::Once,
            };
            let cell = match tool_cell {
                // Denied, the call never ran: its record is all that is left of it.
                Some(cell) if !approved => {
                    t.private.calls.remove(&call);
                    t.update(cell, |m| *m = record);
                    cell
                }
                // Approved, the record reads before the call it let through.
                Some(cell) if approved && t.private.index.contains_key(&cell) => {
                    t.insert_before(cell, at, record)
                }
                _ => t.push(at, record),
            };
            t.private.approved = approved.then_some((event.seq, cell));
        }
        EventKind::TaskCompleted { .. }
        | EventKind::TaskFailed { .. }
        | EventKind::TaskCancelled { .. }
        | EventKind::AgentCompleted { .. }
        | EventKind::AgentFailed { .. }
        | EventKind::AgentCancelled { .. } => child_result(transcripts, agent, event, ctx),
        _ => {}
    }
}

/// An answer the operator asked to remember lands its rule in the same
/// commit, the event right after it; the rule carries no agent or approval.
fn remembered_answer(
    transcripts: &mut Transcripts,
    ctx: &IngestCtx<'_>,
    seq: u64,
    scope: &RuleScope,
) {
    let answered = |t: &AgentTranscript| t.private.approved.filter(|(at, _)| at + 1 == seq);
    let Some((agent, cell)) = transcripts
        .iter()
        .find_map(|(id, t)| answered(t).map(|(_, cell)| (*id, cell)))
    else {
        return;
    };
    let scope = match scope {
        RuleScope::Subtree { root } => ApprovalScope::Subtree {
            root: Lookup {
                ctx,
                others: transcripts,
            }
            .name(*root),
        },
        RuleScope::Everywhere => ApprovalScope::Everywhere,
        RuleScope::Agent { id } => ApprovalScope::Agent {
            name: Lookup {
                ctx,
                others: transcripts,
            }
            .name(*id),
        },
        RuleScope::Run { id } => ApprovalScope::Run { id: *id },
    };
    let Some(t) = transcripts.get_mut(&agent) else {
        return;
    };
    t.private.approved = None;
    t.update(cell, |m| {
        if let CellModel::ApprovalRecord { scope: s, .. } = m {
            *s = scope;
        }
    });
}

/// A usage report adds its delta to the agent's and every ancestor's open
/// session (the separator's cost and agent count) and subtree total.
fn spend(
    transcripts: &mut Transcripts,
    ctx: &IngestCtx<'_>,
    agent: AgentId,
    total: &Usage,
    turn: &Usage,
) {
    let Some(own) = transcripts.get_mut(&agent) else {
        return;
    };
    let now = cost_of(total);
    let delta = match (own.private.spent, now) {
        (Some(before), Some(now)) => Some(Cost {
            usd: (now.usd - before.usd).max(0.0),
            estimated: now.estimated,
            basis: now.basis,
        }),
        // First report since attach: only this turn's share is new to us.
        (None, _) => cost_of(turn),
        (Some(_), None) => None,
    };
    own.private.spent = now.or(own.private.spent);
    let mut next = Some(agent);
    let mut hops = 0;
    while let Some(id) = next {
        // Lineage is a tree; the bound only guards against a corrupt cycle.
        hops += 1;
        if hops > 64 {
            break;
        }
        let Some(t) = transcripts.get_mut(&id) else {
            break;
        };
        t.private.subtree = add_cost(t.private.subtree, delta);
        if let Some(session) = &mut t.private.session {
            session.cost = add_cost(session.cost, delta);
            session.agents.insert(agent);
        }
        next = t
            .private
            .who
            .parent
            .or_else(|| ctx.tree.nodes.get(&id).and_then(|n| n.agent.parent));
    }
}

/// Failed outranks cancelled outranks finished: a crash after the task
/// completed still reads as a failure.
fn severity(outcome: &ChildOutcome) -> u8 {
    match outcome {
        ChildOutcome::Finished => 0,
        ChildOutcome::Cancelled => 1,
        ChildOutcome::Failed { .. } => 2,
    }
}

fn child_result(transcripts: &mut Transcripts, child: AgentId, event: &Event, ctx: &IngestCtx<'_>) {
    let Some(c) = transcripts.get(&child) else {
        return;
    };
    let who = &c.private.who;
    let Some(parent) = who
        .parent
        .or_else(|| ctx.tree.nodes.get(&child).and_then(|n| n.agent.parent))
    else {
        return;
    };
    let failed = |error: &str| ChildOutcome::Failed {
        error: first_line(error).to_owned(),
        quota: c.private.quota.clone(),
    };
    let (outcome, result) = match &event.kind {
        EventKind::TaskCompleted { result } => {
            (ChildOutcome::Finished, Some(result.summary.clone()))
        }
        EventKind::AgentCompleted { summary } => (ChildOutcome::Finished, summary.clone()),
        EventKind::TaskFailed { error, .. } | EventKind::AgentFailed { error } => {
            (failed(error), None)
        }
        EventKind::TaskCancelled { .. } | EventKind::AgentCancelled { .. } => {
            (ChildOutcome::Cancelled, None)
        }
        _ => return,
    };
    let node = ctx.tree.nodes.get(&child).map(|n| &n.agent);
    let created = who.created_at.or(node.map(|a| a.created_at));
    // By agent: the child's own transcript may not have existed to note its cell.
    let spawned_as = transcripts.get(&parent).and_then(|p| p.spawned_name(child));
    let name = spawned_as
        .or_else(|| who.name.clone())
        .or(node.map(|a| a.name.clone()))
        .unwrap_or_else(|| short_id(child));
    let role = who
        .role
        .clone()
        .or(node.map(|a| a.role.0.clone()))
        .unwrap_or_default();
    let descendants: Vec<DescendantSummary> = c
        .cells
        .iter()
        .filter(|cell| !c.named_only.contains(&cell.id))
        .filter_map(|cell| match &cell.model {
            CellModel::ChildResult {
                name,
                outcome,
                elapsed_ms,
                cost_with_children,
                ..
            } => Some(DescendantSummary {
                name: name.clone(),
                outcome: outcome.clone(),
                elapsed_ms: *elapsed_ms,
                cost: *cost_with_children,
            }),
            _ => None,
        })
        .collect();
    let elapsed_ms = created.map(|c| millis(c, event.at)).unwrap_or(0);
    let cost = c.private.subtree;
    let existing = c.private.result_cell;
    let parent_t = transcripts.entry(parent).or_default();
    let reuse = existing.filter(|id| {
        matches!(parent_t.cell(*id).map(|cell| &cell.model),
        Some(CellModel::ChildResult { agent, .. }) if *agent == child)
    });
    let cell = match reuse {
        Some(id) => {
            parent_t.update(id, |m| {
                if let CellModel::ChildResult {
                    outcome: o,
                    result: r,
                    elapsed_ms: e,
                    cost_with_children: k,
                    descendants: d,
                    ..
                } = m
                {
                    if severity(&outcome) > severity(o) {
                        *o = outcome;
                    }
                    if r.is_empty()
                        && let Some(text) = result
                    {
                        *r = text.trim().to_owned();
                    }
                    *e = elapsed_ms;
                    *k = cost;
                    *d = descendants;
                }
            });
            id
        }
        None => parent_t.push(
            event.at,
            CellModel::ChildResult {
                agent: child,
                name,
                role,
                outcome,
                elapsed_ms,
                cost_with_children: cost,
                result: result.unwrap_or_default().trim().to_owned(),
                descendants,
            },
        ),
    };
    if let Some(c) = transcripts.get_mut(&child) {
        c.private.result_cell = Some(cell);
    }
}

#[cfg(test)]
impl AgentTranscript {
    /// Append a committed cell directly, for view tests that need thousands.
    pub(crate) fn push_for_test(&mut self, at: Timestamp, model: CellModel) -> CellId {
        self.push(at, model)
    }
}

#[cfg(test)]
impl AgentTranscript {
    pub(super) fn holds_cut(&self) -> bool {
        self.private.cut.is_some()
    }

    pub(super) fn assert_calls_closed(&self) {
        assert!(self.private.calls.is_empty());
        assert!(self.private.approvals.is_empty());
        assert!(
            self.private
                .artifacts
                .keys()
                .all(|id| self.private.artifact_cells.contains_key(id))
        );
    }

    pub(super) fn assert_window_indexes(&self) {
        assert_eq!(self.private.index.len(), self.cells.len());
        for (i, cell) in self.cells.iter().enumerate() {
            assert_eq!(self.private.index.get(&cell.id), Some(&i));
        }
        for call in self
            .private
            .calls
            .values()
            .filter(|call| call.hidden.is_none())
        {
            assert!(self.cell(call.cell).is_some());
        }
        for cell in self
            .private
            .artifact_cells
            .values()
            .chain(self.private.incidents.values())
        {
            assert!(self.cell(*cell).is_some());
        }
        if let Some(cell) = self.private.compaction {
            assert!(self.cell(cell).is_some());
        }
        if let Some((_, cell)) = self.private.approved {
            assert!(self.cell(cell).is_some());
        }
    }
}
