//! The Ctrl+B board: its interaction state and the tree/inspector projection
//! (SPEC §1.2, §2, §7.4 "Board data"). Rendering is `ui::board`. Owner: W3.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use aether_core::agent::{
    Agent, AgentStatus, AgentStatusKind, BlockReason, ExhaustionClass, WaitCondition,
};
use aether_core::id::AgentId;
use aether_core::model::{Effort, Usage};
use aether_core::provider::UsageWindow;
use aether_core::time::Timestamp;
use aether_protocol::{AgentTree, TreeNode};

use super::helpers::sibling_names;
use super::transcript::{
    AgentTranscript, CallStatus, CallTarget, CellModel, ChildOutcome, CompactionState, Cost,
    ExploreItem, ExploreVerb, FileChange, MemoryNote, inner_command,
};
use super::{App, Effect, ShowFilter, name_of_id};
use crate::ui::layout::{BOARD_DEFAULT_OPEN_COLS, BOARD_DOCK_MIN_COLS};

/// Who has the keyboard. Views inside the bottom pane are not a focus owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BoardFocus {
    #[default]
    BottomPane,
    Board,
}

/// Board interaction state. The viewed agent is `App::selected`; the cursor is
/// independent of it (arrows move the inspector, Enter views).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoardState {
    pub open: bool,
    pub focus: BoardFocus,
    pub cursor: Option<AgentId>,
    /// Live subtrees the user folded with `←`.
    pub collapsed: HashSet<AgentId>,
    /// Finished subtrees the user unfolded with `→` (they auto-collapse otherwise).
    pub expanded: HashSet<AgentId>,
    /// Rebuilt by [`App::rebuild_board`] when tree, approvals, questions or folds change.
    pub model: BoardModel,
    /// Bumped only when a rebuild changed `model`; keys the renderer's label cache.
    pub revision: u64,
    pub(crate) confirmation_pending: Option<AgentId>,
    /// Accepted stop-and-remove requests waiting for a terminal tree snapshot.
    pub(crate) pending_removals: std::collections::BTreeSet<AgentId>,
}

impl BoardState {
    pub fn focused(&self) -> bool {
        self.open && self.focus == BoardFocus::Board
    }
}

/// Status as the board shows it (§2.1 glyph table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardStatus {
    Running,
    Waiting,
    Idle,
    NeedsYou,
    Blocked { quota: bool },
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl BoardStatus {
    /// The board status of a bare status kind. Pauses and quota blocks need
    /// the whole agent record, so the projection applies those itself.
    pub fn of(kind: AgentStatusKind, needs_you: bool) -> BoardStatus {
        if needs_you && !kind.is_terminal() {
            return BoardStatus::NeedsYou;
        }
        match kind {
            AgentStatusKind::Running => BoardStatus::Running,
            AgentStatusKind::Waiting => BoardStatus::Waiting,
            AgentStatusKind::Created | AgentStatusKind::Ready => BoardStatus::Idle,
            AgentStatusKind::Blocked => BoardStatus::Blocked { quota: false },
            AgentStatusKind::Completed | AgentStatusKind::Imported => BoardStatus::Completed,
            AgentStatusKind::Failed => BoardStatus::Failed,
            AgentStatusKind::Cancelled => BoardStatus::Cancelled,
        }
    }

    /// Terminal: elapsed is the total runtime rather than time in status.
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            BoardStatus::Completed | BoardStatus::Failed | BoardStatus::Cancelled
        )
    }
}

/// Needs-you first, then an operator pause, then a quota block, then the kind.
/// Blind to the tree, so an inbox wait still reads `Waiting`; the board uses [`status_in_tree`].
pub(crate) fn status_of(agent: &Agent, needs_you: bool) -> BoardStatus {
    let kind = match &agent.status {
        AgentStatus::Imported { was } if was.is_terminal() => *was,
        status => status.kind(),
    };
    let status = BoardStatus::of(kind, needs_you);
    if status == BoardStatus::NeedsYou || kind.is_terminal() {
        return status;
    }
    if agent.paused {
        return BoardStatus::Paused;
    }
    match &agent.status {
        AgentStatus::Blocked {
            reason: BlockReason::Exhausted { .. },
        } => BoardStatus::Blocked { quota: true },
        _ => status,
    }
}

/// [`status_of`], except that waiting on nothing in particular is `Idle`: an
/// inbox wait, or a deadline with no live child to report back before it.
pub(crate) fn status_in_tree(agent: &Agent, needs_you: bool, live_child: bool) -> BoardStatus {
    let status = status_of(agent, needs_you);
    let idle = match &agent.status {
        AgentStatus::Waiting { condition } => match condition {
            WaitCondition::Inbox => true,
            WaitCondition::Deadline { .. } => !live_child,
            _ => false,
        },
        _ => false,
    };
    if status == BoardStatus::Waiting && idle {
        BoardStatus::Idle
    } else {
        status
    }
}

fn has_live_child(tree: &AgentTree, node: &TreeNode) -> bool {
    node.children
        .iter()
        .any(|id| tree.nodes.get(id).is_some_and(|n| n.agent.is_live()))
}

/// `(since, finished_after_ms)`: live agents count from their status change,
/// finished ones report creation → final status.
fn timing(agent: &Agent) -> (Option<Timestamp>, Option<u64>) {
    if !agent.status.is_terminal() {
        return (agent.status_since, None);
    }
    let took = agent
        .status_since
        .map(|end| millis_between(agent.created_at, end));
    (Some(agent.created_at), took)
}

fn millis_between(from: Timestamp, to: Timestamp) -> u64 {
    u64::try_from((to - from).num_milliseconds()).unwrap_or(0)
}

/// What the elapsed column shows at `now`; `None` when unknown or idle.
fn elapsed_at(
    status: BoardStatus,
    since: Option<Timestamp>,
    finished_after_ms: Option<u64>,
    now: Timestamp,
) -> Option<u64> {
    match status {
        status if status.is_finished() => finished_after_ms,
        // Time spent ready for the operator is not progress, and would tick forever.
        BoardStatus::Idle => None,
        _ => since.map(|since| millis_between(since, now)),
    }
}

/// Spend as the board shows it: a complete total, an estimate or floor
/// (`~$`), or `None` (shown as nothing) when nothing at all was priced.
pub fn cost_of(usage: &Usage) -> Option<Cost> {
    if usage.cost_complete {
        return Some(Cost {
            usd: usage.cost_usd,
            estimated: usage.cost_basis == aether_core::model::CostBasis::ListPrice,
            basis: usage.cost_basis,
        });
    }
    let usd = usage
        .api_equivalent_usd()
        .or((usage.cost_usd > 0.0).then_some(usage.cost_usd))?;
    Some(Cost {
        usd,
        estimated: true,
        basis: usage.cost_basis,
    })
}

/// The role worth printing: empty when it only repeats the name.
fn role_label(agent: &Agent) -> String {
    let role = agent.role.as_str();
    if role == agent.name {
        String::new()
    } else {
        role.to_owned()
    }
}

/// Tree guide in one indentation column, left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guide {
    /// `│` ancestor continues below.
    Pipe,
    /// Ancestor was the last child: blank.
    Blank,
    /// `├` this node, more siblings follow.
    Tee,
    /// `└` this node is the last sibling.
    Corner,
}

impl Guide {
    /// The same column one row further down (a subtitle, or a descendant).
    pub fn continued(self) -> Guide {
        match self {
            Guide::Pipe | Guide::Tee => Guide::Pipe,
            Guide::Blank | Guide::Corner => Guide::Blank,
        }
    }
}

/// One agent in the tree: title and subtitle rows (§2.1). Strings are
/// unformatted; elapsed/cost are formatted at draw time from `App::now`.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardRow {
    pub id: AgentId,
    pub depth: u16,
    pub guides: Vec<Guide>,
    pub status: BoardStatus,
    pub name: String,
    pub role: String,
    pub model: String,
    pub effort: Option<Effort>,
    /// When the current status began (live) or the run started (finished).
    pub since: Option<Timestamp>,
    /// Total runtime for finished agents.
    pub finished_after_ms: Option<u64>,
    pub cost: Option<Cost>,
    pub needs_you: bool,
    /// Folded: children hidden (finished subtrees auto-fold).
    pub folded: bool,
    pub has_children: bool,
    /// First row of a new root; a blank row separates roots.
    pub root_objective: Option<String>,
    /// Agents hidden under this row while it is folded.
    pub folded_agents: usize,
    /// One of those hidden agents needs you; the fold must not hide that.
    pub folded_needs_you: bool,
    /// Where it works, after the model: every root's shorthand; a child's
    /// only when it differs from its parent's (`isolated`, or a sub-directory).
    pub workspace: Option<String>,
    /// A passive parent that never took a turn is drawn as a group header
    /// of this pipeline: its role without an orchestration prefix.
    pub group: Option<String>,
}

impl BoardRow {
    pub fn elapsed_ms(&self, now: Timestamp) -> Option<u64> {
        elapsed_at(self.status, self.since, self.finished_after_ms, now)
    }
}

/// Header counts: `N · K running · $total`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoardSummary {
    pub agents: usize,
    pub running: usize,
    pub total_cost: Option<Cost>,
}

/// The board's projection of the tree: visible rows in creation order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoardModel {
    pub rows: Vec<BoardRow>,
    pub summary: BoardSummary,
    /// Agents waiting on the operator, as of the last rebuild.
    pub needs_you: BTreeSet<AgentId>,
    /// `worker 2` for the second sibling named `worker`; see [`sibling_names`].
    pub names: HashMap<AgentId, String>,
}

/// Everything the projection reads; kept explicit so it is testable headless.
#[derive(Debug, Clone, Copy)]
pub struct BoardInputs<'a> {
    pub tree: &'a AgentTree,
    pub needs_you: &'a BTreeSet<AgentId>,
    pub collapsed: &'a HashSet<AgentId>,
    pub expanded: &'a HashSet<AgentId>,
    pub show_archived: bool,
    /// For `~` in workspace labels.
    pub home: Option<&'a Path>,
}

impl BoardModel {
    pub fn project(inputs: BoardInputs<'_>) -> BoardModel {
        let mut walk = Walk::new(inputs);
        let roots = walk.shown_in_creation_order(&inputs.tree.roots);
        let root_paths: Vec<PathBuf> = roots
            .iter()
            .filter_map(|root| root.agent.scope.workspace_root.clone())
            .collect();
        walk.root_labels = crate::workspace::shorthands(&root_paths, inputs.home);
        let several_roots = roots.len() > 1;
        let mut agents = 0;
        let mut total: Option<Usage> = None;
        for root in roots {
            let objective = several_roots
                .then(|| root_objective(inputs.tree, root))
                .flatten();
            agents += walk.visit(root, Vec::new(), true, objective).agents;
            *total.get_or_insert_with(Usage::default) += root.subtree_usage;
        }
        BoardModel {
            rows: walk.rows,
            summary: BoardSummary {
                agents,
                running: walk.running,
                total_cost: total.as_ref().and_then(cost_of),
            },
            needs_you: inputs.needs_you.clone(),
            names: walk.names,
        }
    }

    pub fn index_of(&self, id: AgentId) -> Option<usize> {
        self.rows.iter().position(|r| r.id == id)
    }

    /// `agent`'s name on the board, told apart from same-named siblings.
    pub fn name_of(&self, agent: &Agent) -> String {
        self.names
            .get(&agent.id)
            .cloned()
            .unwrap_or_else(|| agent.name.clone())
    }
}

fn root_objective(tree: &AgentTree, root: &TreeNode) -> Option<String> {
    let objective = root.task.as_ref()?.objective.lines().next()?.trim();
    (!objective.is_empty()).then(|| named(tree, objective))
}

/// `text` with the agent and task ids the daemon wrote into it read as names.
fn named(tree: &AgentTree, text: &str) -> String {
    crate::format::name_ids(text, |id| name_of_id(tree, id)).into_owned()
}

/// What one subtree contributed to the walk.
struct Subtree {
    agents: usize,
    needs_you: bool,
    /// The subtree's rolled-up status, which a group header above shows.
    status: BoardStatus,
    /// The latest status change in the subtree: when a finished group ended.
    last_change: Option<Timestamp>,
}

/// A parent that has children and never took a turn of its own.
fn is_passive_parent(agent: &Agent, has_children: bool) -> bool {
    let stats = &agent.stats;
    has_children && stats.turns == 0 && stats.tool_calls == 0 && stats.last_turn_at.is_none()
}

/// The pipeline a passive parent stands for: its role past the last `:`.
fn pipeline_of(agent: &Agent) -> String {
    let role = agent.role.as_str();
    role.rsplit_once(':')
        .map_or(role, |(_, name)| name)
        .to_owned()
}

/// Running if any child runs, else failed if any failed, else done when all
/// are finished; anything idle or waiting leaves the header's own status.
fn rolled_up(own: BoardStatus, children: &[BoardStatus]) -> BoardStatus {
    let active = |s: &BoardStatus| matches!(s, BoardStatus::Running | BoardStatus::NeedsYou);
    if children.iter().any(active) {
        return BoardStatus::Running;
    }
    if children.contains(&BoardStatus::Failed) {
        return BoardStatus::Failed;
    }
    if !children.is_empty() && children.iter().all(|s| s.is_finished()) {
        let done = children.contains(&BoardStatus::Completed);
        return if done {
            BoardStatus::Completed
        } else {
            BoardStatus::Cancelled
        };
    }
    own
}

/// One depth-first pass over the tree: rows outside folds, plus counts.
struct Walk<'a> {
    inputs: BoardInputs<'a>,
    /// Agents with a live descendant: their subtree is not finished.
    live_below: HashSet<AgentId>,
    /// Guards a cycle in stored child links, which the daemon does not promise against.
    seen: HashSet<AgentId>,
    rows: Vec<BoardRow>,
    running: usize,
    names: HashMap<AgentId, String>,
    /// Shorthand per root workspace, told apart where names collide.
    root_labels: HashMap<PathBuf, String>,
}

impl<'a> Walk<'a> {
    fn new(inputs: BoardInputs<'a>) -> Self {
        let nodes = &inputs.tree.nodes;
        let mut live_below = HashSet::new();
        for node in nodes.values().filter(|n| n.agent.is_live()) {
            let mut parent = node.agent.parent;
            // Bounded by the node count, so a parent cycle cannot spin.
            for _ in 0..nodes.len() {
                let Some(id) = parent else { break };
                if !live_below.insert(id) {
                    break;
                }
                parent = nodes.get(&id).and_then(|n| n.agent.parent);
            }
        }
        Walk {
            inputs,
            live_below,
            seen: HashSet::new(),
            rows: Vec::new(),
            running: 0,
            names: sibling_names(inputs.tree),
            root_labels: HashMap::new(),
        }
    }

    /// Shown agents among `ids` (archived ones only on request), oldest first.
    fn shown_in_creation_order(&self, ids: &[AgentId]) -> Vec<&'a TreeNode> {
        let mut nodes: Vec<&TreeNode> = ids
            .iter()
            .filter_map(|id| self.inputs.tree.nodes.get(id))
            .filter(|n| self.inputs.show_archived || !n.agent.archived)
            .collect();
        nodes.sort_by_key(|n| n.agent.created_at);
        nodes
    }

    /// What the subtitle says about where `agent` works; see [`BoardRow::workspace`].
    fn workspace_label(&self, agent: &Agent) -> Option<String> {
        let own = agent.scope.workspace_root.as_deref()?;
        let Some(parent) = agent.parent.and_then(|p| self.inputs.tree.nodes.get(&p)) else {
            return self.root_labels.get(own).cloned();
        };
        let home = self.inputs.home;
        match parent.agent.scope.workspace_root.as_deref() {
            Some(theirs) if theirs == own => None,
            Some(theirs) if own.starts_with(theirs) => Some(crate::workspace::shorthand(own, home)),
            // Only a managed worktree leaves its parent's tree, and its
            // directory name (`workspace`) says nothing.
            Some(_) => Some("isolated".to_owned()),
            None => Some(crate::workspace::shorthand(own, home)),
        }
    }

    /// Visit `node` and its whole subtree; rows are pushed only while `visible`.
    fn visit(
        &mut self,
        node: &'a TreeNode,
        guides: Vec<Guide>,
        visible: bool,
        root_objective: Option<String>,
    ) -> Subtree {
        let agent = &node.agent;
        if !self.seen.insert(agent.id) {
            return Subtree {
                agents: 0,
                needs_you: false,
                status: BoardStatus::Idle,
                last_change: None,
            };
        }
        let needs_you = self.inputs.needs_you.contains(&agent.id);
        let status = status_in_tree(agent, needs_you, has_live_child(self.inputs.tree, node));
        if status == BoardStatus::Running {
            self.running += 1;
        }
        let children = self.shown_in_creation_order(&node.children);
        let finished_subtree = !agent.is_live() && !self.live_below.contains(&agent.id);
        let folded = !children.is_empty()
            && (self.inputs.collapsed.contains(&agent.id)
                || (finished_subtree && !self.inputs.expanded.contains(&agent.id)));
        let group = is_passive_parent(agent, !children.is_empty()).then(|| pipeline_of(agent));
        let row = visible.then(|| {
            let (since, finished_after_ms) = timing(agent);
            self.rows.push(BoardRow {
                id: agent.id,
                depth: u16::try_from(guides.len()).unwrap_or(u16::MAX),
                guides: guides.clone(),
                status,
                name: self.names.get(&agent.id).unwrap_or(&agent.name).clone(),
                role: role_label(agent),
                model: agent.model.model.clone(),
                effort: agent.config.effort,
                since,
                finished_after_ms,
                cost: cost_of(if folded || group.is_some() {
                    &node.subtree_usage
                } else {
                    &agent.stats.usage
                }),
                needs_you,
                folded,
                has_children: !children.is_empty(),
                root_objective,
                folded_agents: 0,
                folded_needs_you: false,
                workspace: self.workspace_label(agent),
                group: group.clone(),
            });
            self.rows.len() - 1
        });

        let inherited: Vec<Guide> = guides.iter().map(|g| g.continued()).collect();
        let last = children.len().saturating_sub(1);
        let mut below = Subtree {
            agents: 0,
            needs_you: false,
            status,
            last_change: agent.status_since,
        };
        let mut child_statuses = Vec::with_capacity(children.len());
        for (i, child) in children.into_iter().enumerate() {
            let mut child_guides = inherited.clone();
            child_guides.push(if i == last { Guide::Corner } else { Guide::Tee });
            let sub = self.visit(child, child_guides, visible && !folded, None);
            below.agents += sub.agents;
            below.needs_you |= sub.needs_you;
            below.last_change = below.last_change.max(sub.last_change);
            child_statuses.push(sub.status);
        }
        if folded && let Some(index) = row {
            self.rows[index].folded_agents = below.agents;
            self.rows[index].folded_needs_you = below.needs_you;
        }
        // An ordinary agent's own state counts too; only a header's does not.
        if group.is_none() {
            child_statuses.push(status);
        }
        let subtree_status = rolled_up(status, &child_statuses);
        if let (Some(_), Some(index)) = (&group, row) {
            self.rows[index].status = subtree_status;
            self.rows[index].since = Some(agent.created_at);
            self.rows[index].finished_after_ms = None;
            if let Some(end) = below.last_change.filter(|_| subtree_status.is_finished()) {
                self.rows[index].finished_after_ms = Some(millis_between(agent.created_at, end));
            }
        }
        Subtree {
            agents: 1 + below.agents,
            needs_you: needs_you || below.needs_you,
            status: subtree_status,
            last_change: below.last_change,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChildLine {
    pub id: AgentId,
    pub status: BoardStatus,
    pub name: String,
    pub role: String,
    pub since: Option<Timestamp>,
    pub cost: Option<Cost>,
    /// Total runtime for finished children.
    pub finished_after_ms: Option<u64>,
}

impl ChildLine {
    pub fn elapsed_ms(&self, now: Timestamp) -> Option<u64> {
        elapsed_at(self.status, self.since, self.finished_after_ms, now)
    }
}

/// Why an agent is waiting on the operator (`◆ …` in the inspector).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asks {
    Question(String),
    /// What the gated call would do, e.g. `Wants to run cargo test`.
    Approval(String),
}

/// The inspector's `Last activity`: `verb rest`, in transcript wording.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub verb: String,
    pub rest: String,
    pub at: Timestamp,
    /// Failed or refused: the verb reads as a failure.
    pub failed: bool,
}

/// The inspector for one agent (§2.2). No ids, no `Debug` text.
#[derive(Debug, Clone, PartialEq)]
pub struct InspectorModel {
    pub id: AgentId,
    pub name: String,
    pub role: String,
    pub status: BoardStatus,
    pub parent_name: Option<String>,
    pub turn: Option<u32>,
    pub since: Option<Timestamp>,
    /// Total runtime for finished agents.
    pub finished_after_ms: Option<u64>,
    pub objective: Option<String>,
    /// Failed agents: error first line.
    pub error: Option<String>,
    /// Quota exhaustion with provider and window; shown with the error, never silent.
    pub quota: Option<String>,
    /// `◆ Asks: …` when it needs you.
    pub asks: Option<Asks>,
    pub context_used: Option<u64>,
    pub context_window: Option<u64>,
    /// Where the agent compacts; the context bar measures against it.
    pub compaction_threshold: Option<u64>,
    pub model: String,
    pub effort: Option<Effort>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub tokens_cache_write: u64,
    pub tokens_cached: u64,
    pub cost: Option<Cost>,
    /// Subtree spend; `None` for an agent without children.
    pub cost_with_children: Option<Cost>,
    pub workspace: Option<String>,
    /// `None` off a branch: child worktrees are usually detached.
    pub branch: Option<String>,
    pub children: Vec<ChildLine>,
    pub last_activity: Option<Activity>,
}

impl InspectorModel {
    pub fn elapsed_ms(&self, now: Timestamp) -> Option<u64> {
        elapsed_at(self.status, self.since, self.finished_after_ms, now)
    }

    /// The inspector for `agent`, or `None` when it left the tree.
    pub fn build(app: &App, agent: AgentId) -> Option<InspectorModel> {
        let tree = &app.tree;
        let board = &app.board.model;
        let node = tree.nodes.get(&agent)?;
        let a = &node.agent;
        let needs_you = board.needs_you.contains(&agent);
        let (since, finished_after_ms) = timing(a);
        let usage = &a.stats.usage;
        let error = match &a.status {
            AgentStatus::Failed { error } => error
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(|line| named(tree, line)),
            _ => None,
        };
        let mut child_agents: Vec<&Agent> = node
            .children
            .iter()
            .filter_map(|id| app.tree.nodes.get(id))
            .map(|n| &n.agent)
            .filter(|child| app.show_archived || !child.archived)
            .collect();
        child_agents.sort_by_key(|child| child.created_at);
        let children = child_agents
            .into_iter()
            .filter_map(|child| tree.nodes.get(&child.id))
            .map(|child| child_line(app, child))
            .collect();
        Some(InspectorModel {
            id: agent,
            name: board.name_of(a),
            role: role_label(a),
            status: status_in_tree(a, needs_you, has_live_child(tree, node)),
            parent_name: a
                .parent
                .and_then(|p| tree.nodes.get(&p))
                .map(|p| board.name_of(&p.agent)),
            turn: (a.stats.turns > 0).then_some(a.stats.turns),
            since,
            finished_after_ms,
            objective: node
                .task
                .as_ref()
                .map(|t| named(tree, t.objective.trim()))
                .filter(|o| !o.is_empty()),
            error,
            quota: quota_line(app, a),
            asks: needs_you.then(|| asks(app, agent)).flatten(),
            context_used: app
                .context_tokens
                .get(&agent)
                .copied()
                .or(a.stats.context_tokens),
            context_window: app.context_window(&a.model),
            compaction_threshold: app.compaction_threshold(&a.model),
            model: a.model.model.clone(),
            effort: a.config.effort,
            tokens_in: usage.input_tokens,
            tokens_out: usage.output_tokens,
            tokens_cache_write: usage.cache_write_tokens,
            tokens_cached: usage.cache_read_tokens,
            cost: cost_of(usage),
            cost_with_children: (!node.children.is_empty())
                .then(|| cost_of(&node.subtree_usage))
                .flatten(),
            workspace: a
                .scope
                .workspace_root
                .as_deref()
                .map(crate::format::short_path),
            branch: a
                .scope
                .workspace_root
                .as_deref()
                .and_then(|root| app.branch_at(root))
                .map(str::to_owned),
            children,
            last_activity: app
                .transcripts
                .get(&agent)
                .map(AgentTranscript::live_state)
                .and_then(last_activity)
                .map(|activity| Activity {
                    rest: named(tree, &activity.rest),
                    ..activity
                }),
        })
    }
}

fn child_line(app: &App, node: &TreeNode) -> ChildLine {
    let agent = &node.agent;
    let needs_you = app.board.model.needs_you.contains(&agent.id);
    let (since, finished_after_ms) = timing(agent);
    ChildLine {
        id: agent.id,
        status: status_in_tree(agent, needs_you, has_live_child(&app.tree, node)),
        name: app.board.model.name_of(agent),
        role: role_label(agent),
        since,
        cost: cost_of(&agent.stats.usage),
        finished_after_ms,
    }
}

/// The open question `agent` asked (latest first, its own transcript first),
/// else its oldest unanswered approval.
fn asks(app: &App, agent: AgentId) -> Option<Asks> {
    let own = app.transcripts.get(&agent).into_iter();
    let others = app
        .transcripts
        .iter()
        .filter(|(id, _)| **id != agent)
        .map(|(_, t)| t);
    let question = own.chain(others).find_map(|t| {
        t.live_state()
            .cells
            .iter()
            .rev()
            .find_map(|cell| match &cell.model {
                CellModel::Question {
                    asker,
                    text,
                    state: crate::app::transcript::QuestionState::Pending,
                    ..
                } if *asker == agent => Some(text.clone()),
                _ => None,
            })
    });
    if let Some(text) = question {
        return Some(Asks::Question(named(&app.tree, &text)));
    }
    app.approvals
        .iter()
        .find(|a| a.agent_id == agent && !app.resolving.contains(&a.id))
        .map(|a| Asks::Approval(named(&app.tree, &super::approvals::wants(a))))
}

/// Quota exhaustion, with the provider and the window, for a blocked or failed agent.
fn quota_line(app: &App, agent: &Agent) -> Option<String> {
    match &agent.status {
        AgentStatus::Blocked {
            reason:
                BlockReason::Exhausted {
                    model,
                    class,
                    next_probe,
                    ..
                },
        } => {
            let what = match class {
                ExhaustionClass::QuotaExhausted => "quota exhausted",
                ExhaustionClass::UsageLimit => "usage limit reached",
            };
            let dot = app.theme.glyphs().dot;
            Some(format!(
                "{} {what} {dot} retrying at {}",
                model.provider,
                next_probe.with_timezone(&chrono::Local).format("%H:%M")
            ))
        }
        AgentStatus::Failed { .. } => {
            let usage = app.quotas.get(&agent.model.provider)?;
            let window = usage.windows.iter().find(|w| w.quota.used_percent >= 100)?;
            Some(format!(
                "{} {} limit reached",
                agent.model.provider,
                window_label(window)
            ))
        }
        _ => None,
    }
}

fn window_label(window: &UsageWindow) -> String {
    match window.quota.window_minutes {
        Some(1_440) => "daily".into(),
        Some(10_080) => "weekly".into(),
        Some(minutes) if minutes % 60 == 0 => format!("{}-hour", minutes / 60),
        Some(minutes) => format!("{minutes}-minute"),
        None => window.name.replace('_', " "),
    }
}

/// The last committed cell that reads as an action, in transcript wording.
fn last_activity(transcript: &AgentTranscript) -> Option<Activity> {
    transcript
        .cells
        .iter()
        .rev()
        .filter(|cell| !transcript.named_only.contains(&cell.id))
        .find_map(|cell| {
            let (verb, rest, failed) = activity(&cell.model)?;
            Some(Activity {
                verb,
                rest,
                at: cell.at,
                failed,
            })
        })
}

/// A tool call's verb forms: `Running`, `Ran`, `run`.
#[derive(Clone, Copy)]
struct Verbs {
    doing: &'static str,
    done: &'static str,
    base: &'static str,
}

const RUN: Verbs = Verbs {
    doing: "Running",
    done: "Ran",
    base: "run",
};
const EDIT: Verbs = Verbs {
    doing: "Editing",
    done: "Edited",
    base: "edit",
};
const ADD: Verbs = Verbs {
    doing: "Adding",
    done: "Added",
    base: "add",
};
const DELETE: Verbs = Verbs {
    doing: "Deleting",
    done: "Deleted",
    base: "delete",
};
const CALL: Verbs = Verbs {
    doing: "Calling",
    done: "Called",
    base: "call",
};
const FETCH: Verbs = Verbs {
    doing: "Fetching",
    done: "Fetched",
    base: "fetch",
};
const SEARCH: Verbs = Verbs {
    doing: "Searching",
    done: "Searched",
    base: "search",
};

/// Past tense only once the call succeeded; `failed` when it did not happen.
/// Not a `match`: any other state (running, or not yet started) reads present.
fn call_verb(verbs: Verbs, status: CallStatus) -> (String, bool) {
    match status {
        CallStatus::Awaiting => (format!("Wants to {}", verbs.base), false),
        CallStatus::Running => (verbs.doing.to_owned(), false),
        CallStatus::Succeeded => (verbs.done.to_owned(), false),
        CallStatus::Failed => (format!("Failed to {}", verbs.base), true),
        CallStatus::Denied => (format!("Not allowed to {}", verbs.base), true),
        CallStatus::Cancelled => {
            let doing = verbs.doing.to_ascii_lowercase();
            (format!("Stopped {doing}"), false)
        }
    }
}

fn activity(model: &CellModel) -> Option<(String, String, bool)> {
    let said = |verb: &str, rest: &str| Some((verb.to_owned(), first_line(rest), false));
    let call = |verbs: Verbs, status: CallStatus, rest: &str| {
        let (verb, failed) = call_verb(verbs, status);
        Some((verb, first_line(rest), failed))
    };
    match model {
        CellModel::Exec {
            command,
            status,
            exit,
            is_search,
            ..
        } => match exit {
            // The command did run: `Ran`, in red unless grep-like exit 1 (no match).
            Some(code) if *status == CallStatus::Failed => {
                let rest = format!("{} (exit {code})", first_line(&inner_command(command)));
                Some((RUN.done.to_owned(), rest, !(*is_search && *code == 1)))
            }
            _ => call(RUN, *status, &inner_command(command)),
        },
        CellModel::Explored { items, .. } => items.last().map(|item| {
            let (verb, rest) = explore_activity(item);
            (verb, rest, false)
        }),
        CellModel::Edited { files, status } => match files.as_slice() {
            [] => None,
            [file] => {
                let verbs = match file.change {
                    FileChange::Added => ADD,
                    FileChange::Edited => EDIT,
                    FileChange::Deleted => DELETE,
                };
                let rest = format!("{} (+{} -{})", file.path, file.added, file.removed);
                call(verbs, *status, &rest)
            }
            files => call(EDIT, *status, &format!("{} files", files.len())),
        },
        CellModel::Called { target, status, .. } => {
            let (verbs, rest) = match target {
                CallTarget::Mcp { server, tool } => (CALL, format!("{server}.{tool}")),
                CallTarget::WebSearch { query } => (SEARCH, query.clone()),
                CallTarget::WebFetch { url } => (FETCH, url.clone()),
                CallTarget::Tool { name } => (CALL, name.clone()),
            };
            call(verbs, *status, &rest)
        }
        CellModel::Memory(MemoryNote::Recalled { names }) => said("Recalled", &names.join(", ")),
        CellModel::Memory(MemoryNote::Remembered { text }) => said("Remembered", text),
        CellModel::Spawned { name, .. } => said("Spawned", name),
        CellModel::Waiting { on, ended, .. } => {
            let names: Vec<&str> = on.iter().map(|(_, name)| name.as_str()).collect();
            let verb = if ended.is_some() {
                "Waited for"
            } else {
                "Waiting for"
            };
            said(verb, &names.join(", "))
        }
        CellModel::ChildResult { name, outcome, .. } => {
            let what = match outcome {
                ChildOutcome::Finished => "finished",
                ChildOutcome::Failed { .. } => "failed",
                ChildOutcome::Cancelled => "was cancelled",
            };
            said(name, what)
        }
        CellModel::Sent { to, .. } => said("Sent to", to),
        CellModel::Assistant { text } => said("Replied", text),
        CellModel::Question { text, .. } => said("Asked", text),
        CellModel::Error { message, .. } => Some(("Failed".to_owned(), first_line(message), true)),
        CellModel::Compaction(CompactionState::Done { .. }) => said("Compacted", "context"),
        _ => None,
    }
}

fn explore_activity(item: &ExploreItem) -> (String, String) {
    let targets = item.targets.join(", ");
    match (item.verb, &item.scope) {
        (ExploreVerb::Read, _) => ("Read".into(), targets),
        (ExploreVerb::List, _) => ("List".into(), targets),
        (ExploreVerb::Search, Some(scope)) => ("Search".into(), format!("{targets} in {scope}")),
        (ExploreVerb::Search, None) => ("Search".into(), targets),
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_owned()
}

/// Whether the board starts open: the saved choice when the terminal can dock
/// it, else open by default on wide terminals (§1.2 persistence).
pub fn default_board_open(cols: u16, saved: Option<bool>) -> bool {
    match saved {
        Some(open) => open && cols >= BOARD_DOCK_MIN_COLS,
        None => cols >= BOARD_DEFAULT_OPEN_COLS,
    }
}

impl App {
    /// Recompute `board.model`. Called after the tree, approvals, questions or folds change.
    pub(crate) fn rebuild_board(&mut self) {
        let nodes = &self.tree.nodes;
        self.board.collapsed.retain(|id| nodes.contains_key(id));
        self.board.expanded.retain(|id| nodes.contains_key(id));
        let needs_you = self.agents_needing_you();
        let model = BoardModel::project(BoardInputs {
            tree: &self.tree,
            needs_you: &needs_you,
            collapsed: &self.board.collapsed,
            expanded: &self.board.expanded,
            show_archived: self.show_archived,
            home: self.options.home.as_deref(),
        });
        if model != self.board.model {
            self.board.model = model;
            self.board.revision = self.board.revision.wrapping_add(1);
        }
        // A cursor whose row folded away moves up to the nearest shown ancestor.
        self.board.cursor = self.board.cursor.and_then(|id| self.nearest_row(id));
    }

    /// The agent the inspector shows: the cursor while focused, else the viewed agent.
    pub fn inspected_agent(&self) -> Option<AgentId> {
        if self.board.focused() {
            self.board.cursor.or(self.selected)
        } else {
            self.selected
        }
    }

    /// `↑`/`↓`: move the cursor, starting from the viewed agent, clamped at the ends.
    pub(crate) fn board_move(&mut self, delta: isize) {
        let rows = &self.board.model.rows;
        let Some(last) = rows.len().checked_sub(1) else {
            self.board.cursor = None;
            return;
        };
        let from = self.board_cursor_index().unwrap_or(0);
        let to = from.saturating_add_signed(delta).min(last);
        self.board.cursor = Some(rows[to].id);
    }

    /// `←`: fold the cursor's subtree, or move to its parent when there is nothing to fold.
    pub(crate) fn board_fold(&mut self) {
        let Some(index) = self.board_cursor_index() else {
            return;
        };
        let row = &self.board.model.rows[index];
        let id = row.id;
        if row.has_children && !row.folded {
            self.board.expanded.remove(&id);
            self.board.collapsed.insert(id);
            self.board.cursor = Some(id);
            self.rebuild_board();
            return;
        }
        let parent = self.board.model.rows[..index]
            .iter()
            .rev()
            .find(|r| r.depth < row.depth)
            .map(|r| r.id);
        self.board.cursor = Some(parent.unwrap_or(id));
    }

    /// `→`: unfold the cursor's subtree, or move to its first child when already open.
    pub(crate) fn board_unfold(&mut self) {
        let Some(index) = self.board_cursor_index() else {
            return;
        };
        let row = &self.board.model.rows[index];
        let id = row.id;
        if row.folded {
            // The last explicit choice wins, so a finished subtree stays open.
            self.board.collapsed.remove(&id);
            self.board.expanded.insert(id);
            self.board.cursor = Some(id);
            self.rebuild_board();
            return;
        }
        let first_child = self
            .board
            .model
            .rows
            .get(index + 1)
            .filter(|next| next.depth > row.depth)
            .map(|next| next.id);
        self.board.cursor = Some(first_child.unwrap_or(id));
    }

    pub(crate) fn board_home(&mut self) {
        self.board.cursor = self.board.model.rows.first().map(|r| r.id);
    }

    pub(crate) fn board_end(&mut self) {
        self.board.cursor = self.board.model.rows.last().map(|r| r.id);
    }

    /// `.`: show or hide archived agents; the choice is saved like `/show`.
    pub(crate) fn board_toggle_archived(&mut self) -> Vec<Effect> {
        let show = !self.show_archived;
        let effects = self.show_filter(Some(if show {
            ShowFilter::Archived
        } else {
            ShowFilter::Live
        }));
        self.rebuild_board();
        self.flash(if show {
            "showing archived agents"
        } else {
            "archived agents hidden"
        });
        effects
    }

    /// Row of the cursor, or of the viewed agent before the cursor has moved.
    fn board_cursor_index(&self) -> Option<usize> {
        let model = &self.board.model;
        self.board
            .cursor
            .and_then(|id| model.index_of(id))
            .or_else(|| self.selected.and_then(|id| model.index_of(id)))
            .or((!model.rows.is_empty()).then_some(0))
    }

    /// `id` if it has a row, else its nearest ancestor that does.
    fn nearest_row(&self, id: AgentId) -> Option<AgentId> {
        let mut at = Some(id);
        for _ in 0..=self.tree.nodes.len() {
            let current = at?;
            if self.board.model.index_of(current).is_some() {
                return Some(current);
            }
            at = self.tree.nodes.get(&current).and_then(|n| n.agent.parent);
        }
        None
    }

    /// Agents blocked on the operator: an unanswered approval or question.
    fn agents_needing_you(&self) -> BTreeSet<AgentId> {
        let approvals = self
            .approvals
            .iter()
            .filter(|a| !self.resolving.contains(&a.id))
            .map(|a| a.agent_id);
        let questions = self
            .transcripts
            .values()
            .flat_map(|t| &t.live_state().cells)
            .filter_map(|cell| match &cell.model {
                CellModel::Question {
                    asker,
                    state: crate::app::transcript::QuestionState::Pending,
                    ..
                } => Some(*asker),
                _ => None,
            });
        approvals.chain(questions).collect()
    }
}

#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod tests;
