//! Replies that change an agent's model or run state.
use crate::app::{App, ClientError, Effect, Reply, Then, explain};
use aether_core::agent::Agent;
use aether_core::id::AgentId;

impl App {
    pub(super) fn apply_model_reply(
        &mut self,
        then: Then,
        reply: Reply,
        effects: &mut Vec<Effect>,
    ) {
        match (then, reply) {
            (Then::ModelPatched(id), Reply::Agent(agent)) => {
                self.model_patched(id, &agent, effects);
            }
            (Then::ModelPatched(id), _) => {
                self.model_patches_in_flight.remove(&id);
                self.queued_model_patches.remove(&id);
                self.flash_error(
                    "model change returned an unexpected reply; check the agent and retry /model",
                );
            }
            (Then::PendingRootModelPatched(id), Reply::Agent(agent)) => {
                self.pending_root_model_patched(id, &agent, effects);
            }
            (Then::PendingRootModelPatched(id), _) => {
                self.root_model_unknown(id);
                self.flash_error(if self.holds_root(id) {
                    "new agent's model change returned an unexpected reply; queued messages were held. Use /model to retry"
                } else {
                    "new agent's model change returned an unexpected reply"
                });
            }
            // A running turn's interruption is told by its transcript cell.
            (Then::Interrupted(_), Reply::Interrupted { was_running: true }) => {}
            (Then::Interrupted(agent), Reply::Interrupted { was_running: false }) => {
                let name = self.agent_name(agent);
                self.flash(crate::ui_format!(
                    self.theme,
                    "{name} has no turn running · /cancel ends it",
                    "{name} has no turn running | /cancel ends it"
                ));
            }
            (Then::Paused(root), Reply::Paused { agents, mid_turn }) => {
                let notice = self.paused_notice(root, &agents, &mid_turn);
                self.echo(notice);
            }
            (Then::Resumed(root), Reply::Agents(agents)) => {
                let notice = match agents.as_slice() {
                    [] => format!("{} was not paused", self.agent_name(root)),
                    [one] => format!("resumed {}", self.agent_name(one.id)),
                    many => format!("resumed {} agents in the subtree", many.len()),
                };
                self.echo(notice);
            }
            (Then::ApprovalsSet { agent: root, on }, Reply::Agents(agents)) => {
                self.approvals_set(root, on, agents.len(), effects);
            }
            (then, reply) => self.apply_data_reply(then, reply, effects),
        }
    }

    pub(super) fn pending_root_model_disconnected(&mut self, id: AgentId) {
        self.root_model_unknown(id);
        self.flash_error(if self.holds_root(id) {
            "new agent's model change did not complete; queued messages were held. Use /model to retry"
        } else {
            "new agent's model change did not complete"
        });
    }

    pub(super) fn pending_root_model_failed(
        &mut self,
        id: AgentId,
        e: &ClientError,
    ) -> Option<Effect> {
        if let Some(retry) = self.root_model_failed(id) {
            return Some(retry);
        }
        self.flash_error(if self.holds_root(id) {
            format!("model change failed: {e}; queued messages were held. Use /model to retry")
        } else {
            format!("model change failed: {e}")
        });
        None
    }

    pub(super) fn model_patch_failed(
        &mut self,
        id: AgentId,
        e: &ClientError,
        effects: &mut Vec<Effect>,
    ) {
        let retry = self.finish_model_patch(id);
        let message = explain(
            &self.theme,
            e,
            Then::ModelPatched(id),
            &self.options.command_hint(),
        );
        if let Some(effect) = retry {
            effects.push(effect);
            self.flash_error(format!("{message}; trying the latest model choice"));
        } else {
            self.flash_error(message);
        }
    }

    fn model_patched(&mut self, id: AgentId, agent: &Agent, effects: &mut Vec<Effect>) {
        if agent.id != id {
            self.model_patches_in_flight.remove(&id);
            self.queued_model_patches.remove(&id);
            self.flash("model change returned a different agent; check its model and retry /model");
            return;
        }
        let next = self.finish_model_patch(id);
        self.remember_model(&agent.chain());
        let name = self.agent_display_name(agent.id);
        let kind = agent.status.kind();
        if next.is_some() {
            self.flash(format!("applying the latest model choice for {name}"));
        } else if kind == aether_core::agent::AgentStatusKind::Running {
            self.flash(crate::ui_format!(
                self.theme,
                "{} set to {} · current turn continues; next turn uses it",
                "{} set to {} | current turn continues; next turn uses it",
                name,
                agent.model
            ));
        } else if kind == aether_core::agent::AgentStatusKind::Failed {
            self.flash(format!("{name} set to {}; /wake to retry", agent.model));
        } else {
            self.flash(format!("{name} set to {} for the next turn", agent.model));
        }
        self.refresh_tree(effects);
        if let Some(effect) = next {
            effects.push(effect);
        }
    }

    fn pending_root_model_patched(
        &mut self,
        id: AgentId,
        agent: &Agent,
        effects: &mut Vec<Effect>,
    ) {
        if agent.id == id {
            effects.extend(self.root_model_changed(agent));
            return;
        }
        self.root_model_unknown(id);
        self.flash_error(if self.holds_root(id) {
            "new agent's model change returned a different agent; queued messages were held"
        } else {
            "new agent's model change returned a different agent"
        });
    }

    fn paused_notice(&self, root: AgentId, agents: &[Agent], mid_turn: &[AgentId]) -> String {
        match agents {
            [] if !mid_turn.is_empty() => crate::ui_format!(
                self.theme,
                "{} is already paused · {} still finishing a turn",
                "{} is already paused | {} still finishing a turn",
                self.agent_name(root),
                mid_turn.len()
            ),
            [] => format!("{} is already paused", self.agent_name(root)),
            [one] => {
                let name = self.agent_name(one.id);
                if mid_turn.contains(&one.id) {
                    format!("{name} pauses after the turn it is running")
                } else {
                    crate::ui_format!(
                        self.theme,
                        "paused {name} · no turns until resumed",
                        "paused {name} | no turns until resumed"
                    )
                }
            }
            many => {
                let tail = if mid_turn.is_empty() {
                    String::new()
                } else {
                    crate::ui_format!(
                        self.theme,
                        " · {} finish a turn first",
                        " | {} finish a turn first",
                        mid_turn.len()
                    )
                };
                format!("paused {} agents in the subtree{tail}", many.len())
            }
        }
    }

    fn approvals_set(&mut self, root: AgentId, on: bool, count: usize, effects: &mut Vec<Effect>) {
        // The daemon agreed (or the target was already there): only
        // now is it safe to save the choice as the session default -
        // saving it eagerly, before this reply, could leave a saved
        // default the daemon then refused. The
        // change already landed on the daemon regardless of whether
        // the save below succeeds, so the in-memory option and the
        // tree refresh always happen; a failed save only changes the
        // notice, it must not leave the tree stale too.
        self.options.yolo = Some(on);
        let notice = match self.save_prefs(|p| p.yolo = Some(on)) {
            Err(reason) => format!("yolo not saved: {reason}"),
            Ok(()) => match count {
                0 => format!("{} was already there", self.agent_name(root)),
                1 if on => format!("yolo on for {}", self.agent_name(root)),
                1 => format!("yolo off for {}", self.agent_name(root)),
                n if on => format!("yolo on for {n} agents in the subtree"),
                n => format!("yolo off for {n} agents in the subtree"),
            },
        };
        self.flash(notice);
        self.refresh_tree(effects);
    }
}
