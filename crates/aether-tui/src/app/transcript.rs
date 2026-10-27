//! Per-agent transcripts as cell models (SPEC §3.1, §7.4). Data only: the
//! grouping rules live here so `--plain` and headless tests share them.
//! Rendering is `ui::cells`.

mod ingest;
mod notice_text;
mod tools;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use aether_core::event::Event;
use aether_core::id::{
    AgentId, ApprovalId, ArtifactId, InvocationId, RunId, ScheduleId, ToolCallId,
};
use aether_core::model::Effort;
use aether_core::time::Timestamp;
use aether_protocol::{AgentTree, Delta};

use ingest::Private;
pub(super) use ingest::{
    PARENT_VISIBLE_CHILD_TYPES, bound_history, ingest_replay_event, replace_transcript,
    restore_steering,
};
pub use ingest::{ingest_event, schedule_of};
pub use notice_text::{NoticeSeparator, NoticeText};
pub use tools::inner_command;

/// Stable per-transcript identity of a cell; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CellId(pub u64);

/// Which life of an agent's transcript this is. Cell ids restart when an
/// archived agent's transcript is recreated, so caches key on both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TranscriptEpoch(u64);

impl Default for TranscriptEpoch {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// Bumped on every in-place change, so layout caches key on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CellRevision(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub id: CellId,
    pub revision: CellRevision,
    pub at: Timestamp,
    pub model: CellModel,
}

/// Spend as shown: unknown is `None` upstream (`—`); `estimated` renders `~$`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cost {
    pub usd: f64,
    pub estimated: bool,
    pub basis: aether_core::model::CostBasis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStatus {
    /// Requested, held for the operator's approval.
    Awaiting,
    Running,
    Succeeded,
    Failed,
    Denied,
    Cancelled,
}

/// Who a user-shaped cell came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserFrom {
    Operator,
    /// The agent's task objective, from its parent.
    Task {
        from: String,
    },
    /// An inbox message from another agent.
    Message {
        from: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExploreVerb {
    Read,
    Search,
    List,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExploreItem {
    pub verb: ExploreVerb,
    /// Read: file names (deduped on merge). Search: the query. List: the dir.
    pub targets: Vec<String>,
    /// Search scope (`in crates/aether`).
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    Added,
    Edited,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSign {
    Add,
    Remove,
    Context,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub line_no: Option<u32>,
    pub sign: DiffSign,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdit {
    pub path: String,
    pub change: FileChange,
    pub added: u32,
    pub removed: u32,
    pub diff: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallTarget {
    Mcp {
        server: String,
        tool: String,
    },
    WebSearch {
        query: String,
    },
    WebFetch {
        url: String,
    },
    /// Any other tool, by name (a harness call that failed, an engine's own tool).
    Tool {
        name: String,
    },
}

/// A tool output too big to keep inline, stored as an artifact (§9.3).
/// Named once its `artifact.created` record arrives, in either order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactLink {
    pub id: ArtifactId,
    pub name: Option<String>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryNote {
    Recalled { names: Vec<String> },
    Remembered { text: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChildOutcome {
    Finished,
    Failed {
        error: String,
        /// `anthropic weekly limit reached`; also raises the footer chip.
        quota: Option<String>,
    },
    Cancelled,
}

/// One `✓ tester · 1m 20s · $0.09` row under a child result.
#[derive(Debug, Clone, PartialEq)]
pub struct DescendantSummary {
    pub name: String,
    pub outcome: ChildOutcome,
    pub elapsed_ms: u64,
    pub cost: Option<Cost>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalScope {
    Once,
    Agent {
        name: String,
    },
    Run {
        id: RunId,
    },
    /// For the named agent and every agent it delegates to.
    Subtree {
        root: String,
    },
    Everywhere,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionState {
    Pending,
    Answered,
    AskerEnded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeKind {
    ClientError,
    /// Provider backoff; a later `Recovered` with the same incident replaces it.
    Backoff {
        incident: String,
    },
    Recovered {
        incident: String,
    },
    Fallback,
    McpFailed,
    BudgetWarning,
    Reconnected,
    Interrupted,
    ModelChange,
    Handoff,
    Cancelled,
    /// The outcome of a command the operator typed, kept in the transcript.
    Echo,
    /// An engine's own report (`engine.interrupted`, kill-switch bypass).
    Engine,
    /// A store write refused during an outage, recorded after recovery.
    Store,
    /// An external trigger or schedule delivery: what woke the agent.
    Trigger {
        /// The delivering schedule, named on screen once the TUI knows it.
        schedule: Option<ScheduleId>,
    },
    /// A note a client appended; it woke nobody.
    Note(aether_core::note::NoteLevel),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompactionState {
    Active,
    /// Context tokens either side, when the invocations around it reported them.
    /// `by` names the engine or provider that folded it natively, with its time.
    Done {
        before: Option<u64>,
        after: Option<u64>,
        by: Option<String>,
        duration_ms: Option<u64>,
    },
    Failed {
        error: String,
    },
}

/// Every cell of §3.1. Plain data; no styles, widths or lines.
#[derive(Debug, Clone, PartialEq)]
pub enum CellModel {
    /// Session header (first frame). Rendered by W1's `ui/cells/header.rs`.
    Header {
        version: String,
        cwd: String,
        needs_sign_in: bool,
        sign_in_details: Vec<String>,
        /// The host's greeting; `None` keeps the plain aether header.
        welcome: Option<crate::commands::Welcome>,
    },
    User {
        from: UserFrom,
        text: String,
    },
    Assistant {
        text: String,
    },
    Reasoning {
        title: Option<String>,
        text: String,
        duration_ms: Option<u64>,
    },
    Exec {
        call: Option<ToolCallId>,
        command: String,
        status: CallStatus,
        output: String,
        exit: Option<i32>,
        duration_ms: Option<u64>,
        /// grep/rg-like: exit 1 means "no match", not failure.
        is_search: bool,
        artifacts: Vec<ArtifactLink>,
    },
    Explored {
        items: Vec<ExploreItem>,
        active: bool,
    },
    Edited {
        files: Vec<FileEdit>,
        status: CallStatus,
    },
    Called {
        target: CallTarget,
        args: String,
        result: String,
        status: CallStatus,
        error: Option<String>,
        artifacts: Vec<ArtifactLink>,
    },
    Memory(MemoryNote),
    Spawned {
        agent: AgentId,
        name: String,
        role: String,
        model: String,
        effort: Option<Effort>,
        objective: String,
    },
    /// Active while `ended` is `None`; commits as `Waited … for …`.
    Waiting {
        on: Vec<(AgentId, String)>,
        since: Timestamp,
        ended: Option<Timestamp>,
    },
    ChildResult {
        agent: AgentId,
        name: String,
        role: String,
        outcome: ChildOutcome,
        elapsed_ms: u64,
        cost_with_children: Option<Cost>,
        result: String,
        descendants: Vec<DescendantSummary>,
    },
    Sent {
        to: String,
        text: String,
    },
    ApprovalRecord {
        approval: ApprovalId,
        approved: bool,
        what: String,
        scope: ApprovalScope,
    },
    Question {
        asker: AgentId,
        asker_name: String,
        text: String,
        state: QuestionState,
        /// The `question.asked` event's seq: what a voice reply answers.
        seq: u64,
    },
    Error {
        message: String,
        cause: Option<String>,
        hint: Option<NoticeText>,
    },
    Notice {
        kind: NoticeKind,
        text: NoticeText,
    },
    Compaction(CompactionState),
    /// `── Worked for 6m 14s • $1.84 • 5 agents • 14:32 ──`
    Separator {
        elapsed_ms: u64,
        cost: Option<Cost>,
        agents: u32,
        finished_at: Timestamp,
    },
}

impl CellModel {
    /// This cell with the ids the daemon wrote into it read as names
    /// ([`crate::format::name_ids`]); `None` when nothing changes. What the
    /// operator, the model or a command printed stays verbatim.
    pub fn with_names(&self, name_of: impl Fn(&str) -> Option<String>) -> Option<CellModel> {
        let mut named = self.clone();
        let mut changed = false;
        let mut name = |text: &mut String| {
            if let std::borrow::Cow::Owned(next) = crate::format::name_ids(text, &name_of)
                && next != *text
            {
                *text = next;
                changed = true;
            }
        };
        match &mut named {
            CellModel::Called {
                args,
                result,
                error,
                ..
            } => {
                name(args);
                name(result);
                error.iter_mut().for_each(&mut name);
            }
            CellModel::Spawned { objective, .. } => name(objective),
            CellModel::ChildResult {
                result, outcome, ..
            } => {
                name(result);
                if let ChildOutcome::Failed { error, .. } = outcome {
                    name(error);
                }
            }
            CellModel::Sent { text, .. } | CellModel::Question { text, .. } => name(text),
            CellModel::Notice { text, .. } => text.map_parts(&mut name),
            CellModel::ApprovalRecord { what, .. } => name(what),
            CellModel::Error {
                message,
                cause,
                hint,
            } => {
                name(message);
                cause.iter_mut().for_each(&mut name);
                if let Some(hint) = hint {
                    hint.map_parts(&mut name);
                }
            }
            CellModel::Compaction(CompactionState::Failed { error }) => name(error),
            _ => {}
        }
        changed.then_some(named)
    }

    /// A schedule delivery named after its schedule when `name_of` knows it;
    /// details keep the id. `None` when nothing changes.
    pub fn with_schedule_name(
        &self,
        name_of: impl Fn(ScheduleId) -> Option<String>,
        detail: bool,
    ) -> Option<CellModel> {
        let CellModel::Notice {
            kind: kind @ NoticeKind::Trigger { schedule: Some(id) },
            text,
        } = self
        else {
            return None;
        };
        let name = ingest::trigger_source(&name_of(*id)?);
        if name.is_empty() {
            return None;
        }
        let mut head = Some(if detail {
            format!("Trigger from schedule {name} ({id})")
        } else {
            format!("Trigger from schedule {name}")
        });
        let mut text = text.clone();
        text.map_parts(&mut |part| {
            if let Some(head) = head.take() {
                // Keep the space the separator was joined with.
                *part = format!("{head}{}", &part[part.trim_end().len()..]);
            }
        });
        Some(CellModel::Notice {
            kind: kind.clone(),
            text,
        })
    }
}

/// Source text arriving for the in-flight reply. Rendering state (Phase 1's
/// `MarkdownStream`) lives in the view's cache, so off-screen agents only append.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StreamState {
    /// Bumped by every [`StreamState::reset`], so a reply streamed after a
    /// commit under the same invocation is never drawn as its continuation.
    pub generation: u64,
    pub invocation: Option<InvocationId>,
    pub text: String,
    pub reasoning: String,
    pub reasoning_title: Option<String>,
    pub reasoning_since: Option<Timestamp>,
}

impl StreamState {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.reasoning.is_empty()
    }

    /// Empty, ready for the next reply.
    pub fn reset(&mut self) {
        drop(self.take());
    }

    /// The reply so far, leaving an empty stream of the next generation.
    pub fn take(&mut self) -> StreamState {
        let next = StreamState {
            generation: self.generation.wrapping_add(1),
            ..StreamState::default()
        };
        std::mem::replace(self, next)
    }
}

/// A steering message shown as `↪` until the daemon reports `InboxConsumed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Steer {
    pub text: String,
    pub sent_at: Timestamp,
}

/// One agent's transcript. Off-screen agents are only ingested, never laid out.
/// The scroll anchor lives in `ui::transcript::TranscriptView` (needs row heights).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentTranscript {
    /// Unconsumed inter-agent messages; promotion retains their cell IDs.
    pub pending_messages: BTreeMap<CellId, Cell>,
    /// Supplemental outcomes keep standard presentation unchanged.
    pub named_only: std::collections::HashSet<CellId>,
    /// None invalidates positions after insertion or replay replacement.
    pub(crate) changes: VecDeque<(u64, Option<CellId>)>,
    /// Event boundaries for trimming cells and requesting the preceding window.
    pub(super) history: VecDeque<HistoryPoint>,
    pub(super) older_available: bool,
    /// The bounded live projection while an older window is displayed.
    pub(super) live: Option<Box<AgentTranscript>>,
    pub cells: Vec<Cell>,
    /// The one live cell (streaming reply, running exec, open explore group, wait).
    pub active: Option<Cell>,
    pub stream: StreamState,
    /// Tab-queued messages, sent when the turn ends; persisted in ui.toml. W4 writes.
    pub queue: Vec<String>,
    /// Steering previews awaiting `InboxConsumed`. W4 pushes, W2 retires.
    pub steering: Vec<Steer>,
    /// Backfill from `Transcript` replies has arrived.
    pub loaded: bool,
    pub max_seq: u64,
    pub epoch: TranscriptEpoch,
    /// Missing ids at or below this boundary belong to an earlier rebuild.
    pub(crate) id_floor: CellId,
    next_id: u64,
    /// Bumped whenever `cells` change; streaming and the live cell leave it alone.
    committed: u64,
    /// Bookkeeping the grouping rules need; never rendered.
    private: Private,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct HistoryPoint {
    pub seq: u64,
    last_cell: CellId,
}

impl AgentTranscript {
    pub(crate) fn live_state(&self) -> &Self {
        self.live.as_deref().unwrap_or(self)
    }

    pub(super) fn live_state_mut(&mut self) -> &mut Self {
        match self {
            Self {
                live: Some(live), ..
            } => live,
            current => current,
        }
    }

    /// The next unused id. Ids grow monotonically for the transcript's life.
    pub fn next_cell_id(&mut self) -> CellId {
        self.next_id += 1;
        CellId(self.next_id)
    }

    /// Rebuild with fresh cells and stream, keeping cell ids monotonic and
    /// retaining the operator's queue and steering previews.
    pub fn continuing(&self) -> Self {
        Self {
            queue: self.queue.clone(),
            steering: self.steering.clone(),
            epoch: self.epoch,
            next_id: self.next_id,
            id_floor: CellId(self.next_id),
            ..Self::default()
        }
    }

    /// Fold one daemon event about this agent into its own cells (§3.1).
    /// Cross-agent cells (spawn, child result, cost) need [`ingest_event`].
    pub fn ingest(&mut self, event: &Event, ctx: &IngestCtx<'_>) {
        ingest::own(self, event, ctx);
    }

    /// Append a streamed delta. Only source text changes; nothing is laid out.
    pub fn stream(&mut self, invocation: InvocationId, delta: &Delta, now: Timestamp) {
        if matches!(delta, Delta::Accounting { .. }) {
            return;
        }
        if let Some(live) = &mut self.live {
            live.stream(invocation, delta, now);
            return;
        }
        if self.stream.invocation != Some(invocation) {
            self.stream.reset();
            self.stream.invocation = Some(invocation);
            self.retire_cut(invocation);
        }
        match delta {
            Delta::Text { text } => self.stream.text.push_str(text),
            Delta::Thinking { text } => {
                self.stream.reasoning_since.get_or_insert(now);
                self.stream.reasoning.push_str(text);
                if self.stream.reasoning_title.is_none() {
                    self.stream.reasoning_title = reasoning_title(&self.stream.reasoning);
                }
            }
            Delta::TurnStarted { .. }
            | Delta::ToolInput { .. }
            | Delta::Accounting { .. }
            | Delta::TurnFinished => {}
        }
    }

    /// Changes with every change to `cells`, so caches over them can skip a walk.
    pub fn committed_revision(&self) -> u64 {
        self.committed
    }

    pub fn presentation_cells(&self, named: bool) -> impl Iterator<Item = &Cell> {
        self.all_cells()
            .filter(move |c| named || !self.named_only.contains(&c.id))
            .chain(
                named
                    .then_some(&self.pending_messages)
                    .into_iter()
                    .flat_map(|pending| pending.values()),
            )
    }

    /// Committed cells, then the live one, in display order.
    pub fn all_cells(&self) -> impl Iterator<Item = &Cell> {
        self.cells.iter().chain(self.active.as_ref())
    }

    /// Display position of a cell in [`Self::all_cells`] order.
    pub fn position(&self, id: CellId) -> Option<usize> {
        match self.private.index.get(&id) {
            Some(&at) => Some(at),
            None => self
                .active
                .as_ref()
                .filter(|c| c.id == id)
                .map(|_| self.cells.len()),
        }
    }

    pub fn cell(&self, id: CellId) -> Option<&Cell> {
        match self.private.index.get(&id) {
            Some(&at) => self.cells.get(at),
            None => self
                .active
                .as_ref()
                .filter(|c| c.id == id)
                .or_else(|| self.pending_messages.get(&id)),
        }
    }
}

/// The first `**bold**` run of reasoning text: the status-row header (§3.3).
pub fn reasoning_title(text: &str) -> Option<String> {
    let start = text.find("**")? + 2;
    let len = text[start..].find("**")?;
    let title = text[start..start + len].trim();
    (!title.is_empty()).then(|| title.to_owned())
}

/// Read-only context ingestion needs: names, roles and lineage come from the tree.
#[derive(Debug, Clone, Copy)]
pub struct IngestCtx<'a> {
    pub tree: &'a AgentTree,
    pub now: Timestamp,
}

/// All per-agent transcripts.
pub(super) const HISTORY_WINDOW: usize = 5_000;

pub type Transcripts = HashMap<AgentId, AgentTranscript>;

/// A semantic scroll request; `ui::transcript` resolves it against row heights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollIntent {
    LineUp(u16),
    LineDown(u16),
    PageUp,
    PageDown,
    Top,
    /// Ctrl+End: jump to the bottom and resume following.
    Latest,
    /// Bring a cell into view (find, approval `o`).
    Reveal(CellId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollRequest {
    pub seq: u64,
    pub agent: AgentId,
    pub intent: ScrollIntent,
}

/// Append-only intent log: the reducer pushes, the view applies entries newer
/// than the last `seq` it saw. Bounded; a frame drains far faster than keys arrive.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScrollLog {
    next_seq: u64,
    entries: VecDeque<ScrollRequest>,
}

impl ScrollLog {
    pub const CAPACITY: usize = 64;

    pub fn push(&mut self, agent: AgentId, intent: ScrollIntent) {
        self.next_seq += 1;
        if self.entries.len() == Self::CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(ScrollRequest {
            seq: self.next_seq,
            agent,
            intent,
        });
    }

    /// Requests with `seq > after`, oldest first.
    pub fn since(&self, after: u64) -> impl Iterator<Item = &ScrollRequest> {
        self.entries.iter().filter(move |r| r.seq > after)
    }
}

#[cfg(test)]
#[path = "transcript/tests.rs"]
mod ingest_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_log_replays_only_newer_requests_and_stays_bounded() {
        let mut log = ScrollLog::default();
        let agent = AgentId::new();
        for _ in 0..(ScrollLog::CAPACITY + 3) {
            log.push(agent, ScrollIntent::PageUp);
        }
        assert_eq!(log.since(0).count(), ScrollLog::CAPACITY);
        let last = log.since(0).last().unwrap().seq;
        assert_eq!(log.since(last).count(), 0);
        assert_eq!(log.since(last - 2).count(), 2);
    }

    #[test]
    fn cell_ids_are_monotonic() {
        let mut t = AgentTranscript::default();
        let a = t.next_cell_id();
        let b = t.next_cell_id();
        assert!(b > a);
    }
}
