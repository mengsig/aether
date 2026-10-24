//! Board test fixtures, shared by the projection and the snapshot tests.

use std::collections::BTreeMap;

use aether_core::agent::{Agent, AgentConfig, AgentStats, AgentStatus, Role, Scope};
use aether_core::event::Actor;
use aether_core::id::{AgentId, ApprovalId, RunId, ToolCallId};
use aether_core::model::{Effort, ModelRef, Usage};
use aether_core::task::Task;
use aether_core::time::Timestamp;
use aether_core::tool::{ApprovalRequest, ApprovalStatus, SideEffectClass};
use aether_protocol::{AgentTree, TreeNode};
use chrono::Duration;

use crate::app::transcript::{Cell, CellId, CellModel, CellRevision};
use crate::app::{App, TuiOptions};

/// The fixed clock every board test reads.
pub(crate) fn now() -> Timestamp {
    "2026-10-06T12:00:00Z".parse().expect("a valid timestamp")
}

/// Builds an `AgentTree` agent by agent; creation order is insertion order.
#[derive(Default)]
pub(crate) struct TreeBuilder {
    tree: AgentTree,
}

impl TreeBuilder {
    pub(crate) fn add(
        &mut self,
        parent: Option<AgentId>,
        name: &str,
        role: &str,
        status: AgentStatus,
    ) -> AgentId {
        let id = AgentId::new();
        let created_at =
            now() - Duration::hours(1) + Duration::seconds(self.tree.nodes.len() as i64);
        let agent = Agent {
            id,
            parent,
            role: Role::new(role),
            name: name.into(),
            scope: Scope::default(),
            model: ModelRef::new("anthropic", "sonnet-5.5"),
            fallbacks: Vec::new(),
            status,
            current_task: None,
            inbox_cursor: 0,
            checkpoint: None,
            engine_session: None,
            engine_usage: None,
            engine_sent: None,
            status_since: Some(created_at),
            archived: false,
            paused: false,
            paused_via: None,
            resume_wait: None,
            restarted_from: None,
            restarted_as: None,
            engine_alive: false,
            last_event_at: None,
            config: AgentConfig::default(),
            stats: AgentStats {
                turns: 1,
                ..AgentStats::default()
            },
            run: RunId::new(),
            created_at,
            updated_at: created_at,
        };
        self.tree.nodes.insert(
            id,
            TreeNode {
                agent,
                children: Vec::new(),
                task: None,
                waiting_on: Vec::new(),
                lease: None,
                subtree_usage: Usage::default(),
                hidden_usage: None,
            },
        );
        match parent.and_then(|p| self.tree.nodes.get_mut(&p)) {
            Some(parent) => parent.children.push(id),
            None => self.tree.roots.push(id),
        }
        id
    }

    pub(crate) fn agent(&mut self, id: AgentId) -> &mut Agent {
        &mut self.node(id).agent
    }

    pub(crate) fn node(&mut self, id: AgentId) -> &mut TreeNode {
        self.tree
            .nodes
            .get_mut(&id)
            .expect("an agent this builder added")
    }

    /// An agent that never took a turn, like an orchestration run node.
    pub(crate) fn passive(&mut self, id: AgentId) {
        self.agent(id).stats.turns = 0;
    }

    pub(crate) fn model(&mut self, id: AgentId, model: &str, effort: Effort) {
        let agent = self.agent(id);
        agent.model = ModelRef::new("anthropic", model);
        agent.config.effort = Some(effort);
    }

    /// An exact, fully priced spend.
    pub(crate) fn cost(&mut self, id: AgentId, usd: f64) {
        self.agent(id).stats.usage = Usage {
            cost_usd: usd,
            ..Usage::default()
        };
    }

    /// Spend nothing could price (`cost_of` is `None`).
    pub(crate) fn unpriced(&mut self, id: AgentId) {
        self.agent(id).stats.usage = Usage {
            cost_complete: false,
            ..Usage::default()
        };
    }

    /// A live agent that entered its status `secs` before [`now`].
    pub(crate) fn since(&mut self, id: AgentId, secs: i64) {
        self.agent(id).status_since = Some(now() - Duration::seconds(secs));
    }

    /// A finished agent that ran for `secs`.
    pub(crate) fn took(&mut self, id: AgentId, secs: i64) {
        let agent = self.agent(id);
        agent.status_since = Some(agent.created_at + Duration::seconds(secs));
    }

    pub(crate) fn workspace(&mut self, id: AgentId, path: &str) {
        self.agent(id).scope.workspace_root = Some(path.into());
    }

    pub(crate) fn objective(&mut self, id: AgentId, text: &str) {
        let at = now();
        self.node(id).task = Some(Task::new(text, Actor::System, RunId::new(), at));
    }

    /// The tree with `subtree_usage` rolled up the way the daemon reports it.
    pub(crate) fn build(mut self) -> AgentTree {
        let roots = self.tree.roots.clone();
        for root in roots {
            roll_up(&mut self.tree.nodes, root);
        }
        self.tree
    }
}

fn roll_up(nodes: &mut BTreeMap<AgentId, TreeNode>, id: AgentId) -> Usage {
    let children = nodes[&id].children.clone();
    let mut total = nodes[&id].agent.stats.usage;
    for child in children {
        total += roll_up(nodes, child);
    }
    nodes
        .get_mut(&id)
        .expect("a node in the tree")
        .subtree_usage = total;
    total
}

/// An app on the fixed clock showing `tree`, board open, its model built.
pub(crate) fn app_with(tree: AgentTree) -> App {
    let mut app = App::new(TuiOptions::default());
    app.now = now();
    app.selected = tree.roots.first().copied();
    app.tree = tree;
    app.board.open = true;
    app.rebuild_board();
    app
}

/// The design mocks' fleet (b-board-open-160).
pub(crate) struct Fleet {
    pub(crate) app: App,
    pub(crate) aether: AgentId,
    pub(crate) researcher: AgentId,
    pub(crate) implementer: AgentId,
    pub(crate) tester: AgentId,
    pub(crate) docs: AgentId,
}

pub(crate) fn fleet() -> Fleet {
    let mut t = TreeBuilder::default();
    let aether = t.add(None, "aether", "orchestrator", AgentStatus::Running);
    t.model(aether, "opus-5.5", Effort::High);
    t.cost(aether, 1.12);
    t.since(aether, 362);
    t.objective(aether, "Add a --json flag to aether runs list");

    let completed = AgentStatus::Completed { summary: None };
    let researcher = t.add(Some(aether), "researcher", "explorer", completed);
    t.model(researcher, "sonnet-5.5", Effort::Medium);
    t.cost(researcher, 0.21);
    t.took(researcher, 63);

    let implementer = t.add(Some(aether), "implementer", "worker", AgentStatus::Running);
    t.model(implementer, "sonnet-5.5", Effort::Medium);
    t.cost(implementer, 0.38);
    t.since(implementer, 160);
    t.agent(implementer).stats.turns = 6;
    t.objective(
        implementer,
        "Add --json to aether runs list (one object per line), keep the table \
         byte-identical, delegate tests, update docs/cli.md.",
    );

    let tester = t.add(
        Some(implementer),
        "tester",
        "verifier",
        AgentStatus::Running,
    );
    t.model(tester, "haiku-4.5", Effort::Low);
    t.cost(tester, 0.09);
    t.since(tester, 41);
    t.agent(tester).stats.turns = 3;

    let docs = t.add(Some(implementer), "docs", "writer", AgentStatus::Ready);
    t.model(docs, "haiku-4.5", Effort::Low);
    t.cost(docs, 0.04);
    t.since(docs, 12);

    let mut app = app_with(t.build());
    ask(
        &mut app,
        implementer,
        tester,
        "should --json include imported runs, or only live ones?",
    );
    Fleet {
        app,
        aether,
        researcher,
        implementer,
        tester,
        docs,
    }
}

/// `asker` puts an open question into `transcript`'s agent's transcript.
pub(crate) fn ask(app: &mut App, transcript: AgentId, asker: AgentId, text: &str) {
    let asker_name = app.tree.nodes[&asker].agent.name.clone();
    push_cell(
        app,
        transcript,
        CellModel::Question {
            asker,
            asker_name,
            text: text.into(),
            state: crate::app::transcript::QuestionState::Pending,
            seq: 0,
        },
    );
}

pub(crate) fn push_cell(app: &mut App, agent: AgentId, model: CellModel) {
    let at = app.now;
    push_cell_at(app, agent, at, model);
}

pub(crate) fn push_cell_at(app: &mut App, agent: AgentId, at: Timestamp, model: CellModel) {
    let transcript = app.transcripts.entry(agent).or_default();
    let id: CellId = transcript.next_cell_id();
    transcript.cells.push(Cell {
        id,
        revision: CellRevision::default(),
        at,
        model,
    });
    app.rebuild_board();
}

/// A pending approval for `agent` to run the shell `command`.
pub(crate) fn approval(app: &mut App, agent: AgentId, command: &str) -> ApprovalId {
    let id = ApprovalId::new();
    app.approvals.push(ApprovalRequest {
        id,
        agent_id: agent,
        task_id: None,
        call_id: ToolCallId::new(),
        tool: "shell".into(),
        description: format!("shell {{\"command\":{command:?}}}"),
        argument: Some(command.into()),
        resources: Vec::new(),
        side_effect: SideEffectClass::Mutation,
        requested_at: app.now,
        status: ApprovalStatus::Pending,
    });
    app.rebuild_board();
    id
}
