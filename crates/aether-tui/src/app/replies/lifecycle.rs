//! Replies that create, rename, restart, patch or clear agents.
use crate::app::{AgentTree, Command, Effect, Reply, Then};
use crate::app::{App, StartId};
use aether_core::agent::Agent;
use aether_core::id::AgentId;

impl App {
    pub(super) fn apply_lifecycle_reply(
        &mut self,
        then: Then,
        reply: Reply,
        effects: &mut Vec<Effect>,
    ) {
        match (then, reply) {
            (Then::RootDefaults { start, link }, Reply::Status(status)) => {
                effects.extend(self.root_defaults_arrived(start, link, status));
            }
            (Then::Tree, Reply::Tree(tree)) => self.tree_arrived(tree, effects),
            (Then::AgentCreated(start), Reply::Agent(agent)) => {
                self.agent_created(start, agent, effects);
            }
            (Then::Restarted { from }, Reply::Agent(agent)) => {
                self.agent_restarted(from, agent.id, effects);
            }
            (Then::Woken, Reply::Agent(agent)) => {
                self.refresh_tree(effects);
                self.flash(crate::ui_format!(
                    self.theme,
                    "{} woken · history kept",
                    "{} woken | history kept",
                    agent.id
                ));
            }
            (Then::FannedOut, Reply::MessagesSent { deliveries }) => {
                let n = deliveries.len();
                let noun = if n == 1 { "agent" } else { "agents" };
                self.flash(format!("delivered to {n} {noun} in the subtree"));
            }
            (Then::AgentRenamed(id), Reply::Agent(agent)) if id == agent.id => {
                self.flash(format!(
                    "renamed {} to {}",
                    crate::ui::views::short_id(&self.theme, &id),
                    agent.name
                ));
                self.refresh_tree(effects);
            }
            (Then::AgentRenamed(_), _) => self.operator_reply_unexpected(),
            (Then::Archived, Reply::Agents(agents)) => self.agents_archived(agents.len(), effects),
            (Then::AgentPatched, Reply::Agent(agent)) => self.agent_patched(&agent, effects),
            (then, reply) => self.apply_model_reply(then, reply, effects),
        }
    }

    pub(super) fn operator_reply_unexpected(&mut self) {
        self.flash_error("unexpected operator command reply; refresh /memory or the agent tree to verify the outcome");
    }

    fn tree_arrived(&mut self, tree: AgentTree, effects: &mut Vec<Effect>) {
        self.tree_in_flight = false;
        self.apply_tree(tree, effects);
        if self.tree_stale {
            effects.push(self.tree_request());
        }
    }

    fn agent_created(&mut self, start: StartId, agent: Agent, effects: &mut Vec<Effect>) {
        let id = agent.id;
        self.transcripts.entry(id).or_default().loaded = true;
        if let Some(path) = &agent.scope.workspace_root {
            self.remember_workspace(path);
        }
        // Startup status predates this root; short plain sessions can
        // finish before a periodic refresh reports the new live agent.
        if self.connected {
            effects.push(Effect::Request {
                command: Command::Status,
                then: Then::Status,
            });
        }
        effects.extend(self.root_started(start, id));
        self.refresh_tree(effects);
    }

    fn agent_restarted(&mut self, from: AgentId, id: AgentId, effects: &mut Vec<Effect>) {
        self.met(id);
        self.refresh_tree(effects);
        if self.selected == Some(from) && self.intent == crate::app::intent::Intent::Selection {
            effects.extend(self.select(id));
        }
        self.flash(crate::ui_format!(
            self.theme,
            "restarted as {} · from its last checkpoint",
            "restarted as {} | from its last checkpoint",
            id
        ));
    }

    fn agents_archived(&mut self, count: usize, effects: &mut Vec<Effect>) {
        self.flash(match count {
            0 => "nothing finished to clear".to_owned(),
            // `.` on the board only reveals the cleared view again;
            // it is not an undo.
            1 => self
                .theme
                .decoration(
                    "cleared 1 agent · . on the board shows it",
                    "cleared 1 agent | . on the board shows it",
                )
                .to_owned(),
            n => crate::ui_format!(
                self.theme,
                "cleared {n} agents · . on the board shows them",
                "cleared {n} agents | . on the board shows them"
            ),
        });
        // Each cleared agent's own `agent.archived` event drops its
        // timeline rows and transcript as it arrives over the
        // subscription (on_event), and the refresh below drops a
        // dangling selection (apply_tree) - nothing left to
        // special-case here.
        self.refresh_tree(effects);
    }

    fn agent_patched(&mut self, agent: &Agent, effects: &mut Vec<Effect>) {
        self.flash(crate::ui_format!(
            self.theme,
            "{} now on {} · effort {}",
            "{} now on {} | effort {}",
            self.agent_display_name(agent.id),
            agent.model,
            agent
                .config
                .effort
                .map(|e| e.to_string())
                .unwrap_or_else(|| "default".into())
        ));
        self.refresh_tree(effects);
    }
}
