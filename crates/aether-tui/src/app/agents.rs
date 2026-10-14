//! Application agents.
use std::path::Path;

use super::images::Outgoing;
use super::transcript::ScrollIntent;
use super::{
    AgentId, AgentPatch, AgentTree, App, ApprovalDefault, ApprovalPolicy, Command, Effect, Effort,
    InputAction, ModelChain, ShowFilter, Then, commands,
};

impl App {
    pub(super) fn apply_tree(&mut self, tree: AgentTree, effects: &mut Vec<Effect>) {
        for (id, node) in &tree.nodes {
            if let Some(tokens) = node.agent.stats.context_tokens {
                self.context_tokens.entry(*id).or_insert(tokens);
            }
        }
        self.tree = tree;
        self.finish_board_removals(effects);
        self.names_version = super::names_version(&self.tree, &self.schedules.jobs);
        self.refresh_branches();
        self.rebuild_board();
        self.settle_unconfirmed();
        self.release_ended_roots();
        self.reconcile_view(effects);
    }

    /// The destination `/model` would update. Resolve a restart chain once
    /// so the picker and the action describe the same agent.
    pub(crate) fn model_target(&self) -> Option<AgentId> {
        let mut target = self.selected;
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = target {
            if !seen.insert(id) {
                target = None;
                break;
            }
            let Some(node) = self.tree.nodes.get(&id) else {
                // A just-created root is selected before the tree refresh
                // returns it. Keep that selected id addressable; only a
                // missing successor discovered through a restart is stale.
                if self.selected != Some(id) && !self.holds_root(id) {
                    target = None;
                }
                break;
            };
            if let Some(successor) = node.agent.restarted_as {
                target = Some(successor);
                continue;
            }
            if node.agent.status.is_terminal()
                && node.agent.status.kind() != aether_core::agent::AgentStatusKind::Failed
            {
                target = None;
            }
            break;
        }
        target
    }

    /// Set the selected agent's model for its next turn, or choose the model
    /// the next agent this client creates will use.
    pub(super) fn use_model(&mut self, chain: ModelChain, default_only: bool) -> Vec<Effect> {
        if default_only {
            self.set_session_default(&chain);
            self.remember_model(&chain);
            self.flash(format!("{chain} for new agents"));
            return Vec::new();
        }
        match self.model_target() {
            // The runtime keeps a compatible native session or prepares a
            // handoff when the engine changes. A running turn keeps its model.
            Some(id) => {
                if self.holds_root(id) {
                    let Some(patch) = self.choose_held_root_model(id, chain) else {
                        return Vec::new();
                    };
                    let mut effects = if self.selected == Some(id) {
                        Vec::new()
                    } else {
                        self.select(id)
                    };
                    effects.push(patch);
                    return effects;
                }
                let mut effects = if self.selected == Some(id) {
                    Vec::new()
                } else {
                    self.select(id)
                };
                let name = self.agent_name(id);
                if self.model_patches_in_flight.contains_key(&id) {
                    self.queued_model_patches.insert(id, chain.clone());
                    self.flash(format!("{chain} queued as the latest choice for {name}"));
                    return effects;
                }
                self.model_patches_in_flight.insert(id, chain.clone());
                let running = self.tree.nodes.get(&id).is_some_and(|node| {
                    node.agent.status.kind() == aether_core::agent::AgentStatusKind::Running
                });
                if running {
                    self.flash(format!(
                        "switching {name} to {chain} next turn; current turn continues"
                    ));
                } else {
                    self.flash(format!("switching {name} to {chain}"));
                }
                effects.push(Self::model_patch_request(id, chain));
                effects
            }
            // A finished agent cannot be updated. Keep the choice for the
            // next agent, without starting a conversation on a command alone.
            None => {
                self.set_session_default(&chain);
                self.remember_model(&chain);
                if let Some(start) = self.awaited_in_flight() {
                    start.model = Some(chain.clone());
                    self.flash(format!("{chain} queued for the agent being started"));
                    return Vec::new();
                }
                self.flash(format!("{chain} selected for the next agent"));
                Vec::new()
            }
        }
    }

    fn model_patch_request(id: AgentId, chain: ModelChain) -> Effect {
        Effect::Request {
            command: Command::UpdateAgent {
                id,
                patch: AgentPatch {
                    model: Some(chain.primary),
                    // A single `/model x/y` also clears an older fallback chain.
                    fallbacks: Some(chain.fallbacks),
                    ..Default::default()
                },
                idempotency_key: None,
            },
            then: Then::ModelPatched(id),
        }
    }

    /// Send the latest choice only after the previous update for this agent
    /// has settled; requests run concurrently in the TUI runner.
    pub(super) fn finish_model_patch(&mut self, id: AgentId) -> Option<Effect> {
        self.model_patches_in_flight.remove(&id)?;
        let next = self.queued_model_patches.remove(&id)?;
        self.model_patches_in_flight.insert(id, next.clone());
        Some(Self::model_patch_request(id, next))
    }

    /// The model every new agent this client starts gets, saved so it
    /// survives the session.
    pub(super) fn set_session_default(&mut self, chain: &ModelChain) {
        self.options.default_model = Some(chain.primary.clone());
        self.options.default_fallbacks.clone_from(&chain.fallbacks);
        let stored = chain.to_string();
        if let Err(reason) = self.save_prefs(|p| p.model = Some(stored)) {
            self.flash_error(format!("model not saved: {reason}"));
        }
    }

    /// Move `chain` to the front of the recent list, in memory and on disk:
    /// it is what orders the picker next time.
    pub(super) fn remember_model(&mut self, chain: &ModelChain) {
        // Canonical, so an alias and its model share one recent slot.
        let mut chain = chain.clone();
        if let Some(info) = self.model_info(&chain.primary) {
            chain.primary = info.model.clone();
        }
        let name = chain.primary.to_string();
        self.recent_models.retain(|m| *m != name);
        self.recent_models.insert(0, name);
        if let Err(reason) = self.save_prefs(|p| p.remember_model(&chain)) {
            self.flash_error(format!("model not saved: {reason}"));
        }
    }

    /// Stop the agent's current turn only; it keeps its transcript and waits
    /// for the next message. The reply says whether anything was running.
    pub(super) fn interrupt_agent(&mut self, agent: AgentId) -> Vec<Effect> {
        vec![Effect::Request {
            command: Command::Interrupt {
                agent,
                idempotency_key: None,
            },
            then: Then::Interrupted(agent),
        }]
    }

    /// Set the selected agent's effort, or remember it for the first agent.
    pub(super) fn use_effort(&mut self, effort: Effort) -> Vec<Effect> {
        match self.selected {
            Some(id) => vec![Effect::Request {
                command: Command::UpdateAgent {
                    id,
                    patch: AgentPatch {
                        effort: Some(effort),
                        ..Default::default()
                    },
                    idempotency_key: None,
                },
                then: Then::AgentPatched,
            }],
            None => {
                self.default_effort = Some(effort);
                self.flash(format!("effort {effort} for the next agent"));
                Vec::new()
            }
        }
    }

    /// Flip approvals for the selected agent and everything beneath it, and
    /// move the session default with it so the next agent matches what the
    /// operator just chose.
    pub(super) fn toggle_yolo(&mut self) -> Vec<Effect> {
        let on = match self.selected_agent() {
            Some(agent) => !agent.config.approval.is_yolo(),
            None => !self.yolo_default(),
        };
        let Some(agent) = self.selected else {
            // Nothing to ask the daemon about, so there is no request that
            // can fail: safe to persist the choice right away.
            self.options.yolo = Some(on);
            if let Err(reason) = self.save_prefs(|p| p.yolo = Some(on)) {
                self.flash_error(format!("yolo not saved: {reason}"));
            } else {
                self.flash(if on {
                    self.theme.decoration(
                        "yolo on for new agents · nothing will ask first",
                        "yolo on for new agents | nothing will ask first",
                    )
                } else {
                    "yolo off for new agents"
                });
            }
            return Vec::new();
        };
        // The saved default moves only once the daemon confirms the change
        // (Then::ApprovalsSet), not here: it can refuse (daemon-wide yolo, an
        // engine that cannot gate, a stricter live descendant).
        vec![Effect::Request {
            command: Command::SetApprovals {
                agent,
                policy: if on {
                    ApprovalPolicy::none()
                } else {
                    ApprovalPolicy::default()
                },
                subtree: true,
                idempotency_key: None,
            },
            then: Then::ApprovalsSet { agent, on },
        }]
    }

    /// Turn the archived filter on or off, or say what it is. The choice is
    /// saved, so the view a person cleared stays cleared next session.
    pub(super) fn show_filter(&mut self, filter: Option<ShowFilter>) -> Vec<Effect> {
        let Some(filter) = filter else {
            self.flash(if self.show_archived {
                self.theme.decoration(
                    "showing cleared work · . on the board hides it again",
                    "showing cleared work | . on the board hides it again",
                )
            } else {
                self.theme.decoration(
                    "cleared work is hidden · . on the board shows it",
                    "cleared work is hidden | . on the board shows it",
                )
            });
            return Vec::new();
        };
        let show = filter == ShowFilter::Archived;
        if show == self.show_archived {
            return Vec::new();
        }
        self.show_archived = show;
        if let Err(reason) = self.save_prefs(|p| p.show_archived = Some(show)) {
            self.flash_error(format!("view filter not saved: {reason}"));
        }
        self.flash(if show {
            "showing cleared work"
        } else {
            "cleared work hidden"
        });
        let mut effects = Vec::new();
        self.refresh_tree(&mut effects);
        effects
    }

    /// Whether a new agent auto-approves: what the operator chose this
    /// session, else what the daemon reports as its default.
    pub fn yolo_default(&self) -> bool {
        self.options.yolo.unwrap_or_else(|| {
            self.status
                .as_ref()
                .map(|s| s.approval_default == ApprovalDefault::Yolo)
                .unwrap_or(true)
        })
    }

    /// How many tasks the agent is waiting on, when it is waiting on any.
    pub fn waiting_on(&self, agent: AgentId) -> Option<usize> {
        self.tree
            .nodes
            .get(&agent)
            .map(|n| n.waiting_on.len())
            .filter(|n| *n > 0)
    }

    /// Recompute what the board shows after the tree changed under it.
    /// Public so the frames gate can draw a view the daemon filtered.
    pub fn rebuild_after_tree_change(&mut self) {
        self.rebuild_board();
    }

    // ----- selection and messaging -------------------------------------------------

    /// Whether the daemon has put `agent` beyond taking work. An agent the
    /// tree has not reported yet is not terminal: a root just created is
    /// addressable before the next tree arrives.
    pub(super) fn is_terminal(&self, agent: AgentId) -> bool {
        self.tree
            .nodes
            .get(&agent)
            .is_some_and(|node| node.agent.status.is_terminal())
    }

    pub(super) fn send_message(&mut self, to: AgentId, message: Outgoing) -> Vec<Effect> {
        if let Some(held) = self.held_messages(to) {
            held.push(message);
            self.flash("message queued until the new agent's model change succeeds");
            return Vec::new();
        }
        self.dirty = true;
        vec![Effect::Request {
            command: Command::SendMessage {
                to,
                content: message.text,
                subtree: false,
                attachments: message.images,
                // A person retrying is a new message, not a resubmission.
                idempotency_key: None,
            },
            then: Then::Ignore,
        }]
    }

    pub(super) fn submit_input(&mut self) -> Vec<Effect> {
        let text = self.input.take();
        self.dirty = true;
        let text = match commands::parse(&text) {
            // A command leaves the attached images for the message after it.
            InputAction::Command(cmd) => return self.run_command(cmd),
            InputAction::Empty if self.input.images.is_empty() => return Vec::new(),
            // Images need no words to be a message.
            InputAction::Empty => String::new(),
            InputAction::Message(text) => text,
        };
        let message = Outgoing {
            text,
            images: std::mem::take(&mut self.input.images),
        };
        // With no live agent, every message queues for the one being
        // created: the daemon refuses a message to a terminal agent, and
        // a second message typed before the agent arrives (a pipe does
        // this every time) must not start a second agent.
        match self.selected.filter(|id| !self.is_terminal(*id)) {
            Some(agent) => {
                // The reply lands at the bottom: follow it, leaving any older window.
                let mut effects = self.scroll(ScrollIntent::Latest);
                effects.extend(self.send_message(agent, message));
                effects
            }
            None => {
                let here = self.active_workspace().map(Path::to_path_buf);
                self.await_root_in(here, Some(message))
                    .into_iter()
                    .collect()
            }
        }
    }
}
