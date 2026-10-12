//! Lifecycle actions target the board cursor; removal waits for durable termination.

use aether_core::agent::AgentStatusKind;
use aether_protocol::ArchiveTarget;

use super::input::confirm::ConfirmCancel;
use super::{AgentId, App, ClientError, Command, Effect, Reply, Then};

impl App {
    pub(super) fn board_action(&mut self, key: char) -> Vec<Effect> {
        if self.board.confirmation_pending.is_some() {
            self.flash("waiting for the confirmation's agent snapshot");
            return Vec::new();
        }
        let Some(id) = self.board.cursor else {
            self.flash("select an agent first");
            return Vec::new();
        };
        let Some(node) = self.tree.nodes.get(&id) else {
            self.flash("agent no longer on the board");
            return Vec::new();
        };
        let agent = &node.agent;
        let finished = agent.status.is_terminal();
        let command = match key {
            'D' if agent.archived && !finished => {
                self.flash("cannot restore a live agent");
                return Vec::new();
            }
            'D' if agent.archived => return vec![Self::board_archive(id, false, false)],
            'D' if finished && self.selected != Some(id) && agent.parent.is_some() => {
                return vec![Self::board_archive(id, true, false)];
            }
            'D' | 'X' => {
                if self.board.pending_removals.contains(&id) {
                    self.flash("stop requested; waiting to remove this agent");
                } else {
                    self.board.confirmation_pending = Some(id);
                    self.flash("checking live workers before confirmation");
                    return vec![Effect::Request {
                        command: Command::GetTree {
                            run: None,
                            include_archived: true,
                        },
                        then: Then::BoardConfirm {
                            agent: id,
                            remove: key == 'D',
                            connection: self.connection,
                        },
                    }];
                }
                return Vec::new();
            }
            'P' if finished => {
                self.flash("finished agents cannot pause or resume");
                return Vec::new();
            }
            'P' if agent.paused => {
                return vec![Effect::Request {
                    command: Command::Resume {
                        agent: id,
                        subtree: true,
                        idempotency_key: None,
                    },
                    then: Then::Resumed(id),
                }];
            }
            'P' => Command::Pause {
                agent: id,
                subtree: true,
                idempotency_key: None,
            },
            'I' if agent.status.kind() != AgentStatusKind::Running => {
                self.flash("agent has no turn running");
                return Vec::new();
            }
            'I' => Command::Interrupt {
                agent: id,
                idempotency_key: None,
            },
            _ => return Vec::new(),
        };
        vec![Effect::Request {
            command,
            then: if key == 'P' {
                Then::Paused(id)
            } else {
                Then::Interrupted(id)
            },
        }]
    }

    fn live_worker_count(tree: &aether_protocol::AgentTree, id: AgentId) -> usize {
        let mut children = tree
            .nodes
            .get(&id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        let mut count = 0;
        while let Some(child) = children.pop() {
            if let Some(node) = tree.nodes.get(&child) {
                count += usize::from(node.agent.is_live());
                children.extend(&node.children);
            }
        }
        count
    }

    fn confirm_board_end(&mut self, id: AgentId, remove: bool, tree: &aether_protocol::AgentTree) {
        let Some(node) = tree.nodes.get(&id) else {
            self.flash("agent no longer exists; no action taken");
            return;
        };
        let name = self.agent_display_name(id);
        let finished = node.agent.status.is_terminal();
        let workers = Self::live_worker_count(tree, id);
        if !remove && finished && workers == 0 {
            self.flash("agent and its workers already finished");
            return;
        }
        if remove && node.agent.archived {
            self.flash("agent already archived; . shows it for restoration");
            return;
        }
        let (title, details) = if remove && finished {
            (
                format!("Remove {name} from the board?"),
                "Only this row is archived; history and workers stay.".to_owned(),
            )
        } else if finished {
            (
                format!("End the workers beneath {name}?"),
                format!("Ends all live descendants: currently {workers} live workers."),
            )
        } else {
            let verb = if remove { "Stop and remove" } else { "End" };
            let tail = if remove {
                " Only this row is archived; history stays."
            } else {
                ""
            };
            (
                format!("{verb} {name}?"),
                format!(
                    "Ends this agent and all live descendants: currently {workers} live workers.{tail}"
                ),
            )
        };
        self.leave_board();
        self.bottom.push_view(Box::new(ConfirmCancel::action(
            id,
            name,
            title,
            details,
            remove,
            remove && finished,
        )));
        self.dirty = true;
    }

    fn board_archive(id: AgentId, archived: bool, after_stop: bool) -> Effect {
        Effect::Request {
            command: Command::Archive {
                target: ArchiveTarget::Agent(id),
                archived,
                idempotency_key: None,
            },
            then: Then::BoardArchived {
                agent: id,
                archived,
                after_stop,
            },
        }
    }

    pub(super) fn finish_board_removals(&mut self, effects: &mut Vec<Effect>) {
        let ready: Vec<_> = self
            .board
            .pending_removals
            .iter()
            .copied()
            .filter(|id| {
                self.tree
                    .nodes
                    .get(id)
                    .is_none_or(|n| n.agent.status.is_terminal())
            })
            .collect();
        for id in ready {
            self.board.pending_removals.remove(&id);
            effects.push(Self::board_archive(id, true, true));
        }
    }

    pub(super) fn board_lifecycle_reply(
        &mut self,
        then: Then,
        result: Result<Reply, ClientError>,
    ) -> Vec<Effect> {
        let mut effects = Vec::new();
        match (then, result) {
            (Then::BoardConfirm { connection, .. }, _) if connection != self.connection => {}
            (Then::BoardConfirm { agent, remove, .. }, result) => {
                self.board.confirmation_pending = None;
                match result {
                    Ok(Reply::Tree(tree)) => self.confirm_board_end(agent, remove, &tree),
                    Err(error) => self.flash_error(format!(
                        "cannot confirm agent action: {error}; no action taken"
                    )),
                    Ok(_) => self.flash_error(
                        "cannot confirm agent action: unexpected daemon reply; no action taken",
                    ),
                }
            }
            (Then::BoardStopped { agent, remove }, Ok(Reply::Ok)) => {
                if remove {
                    self.board.pending_removals.insert(agent);
                    self.flash("stop requested; waiting for the agent to finish before removal");
                } else {
                    self.flash("stop requested for the agent and its live workers");
                }
                self.refresh_tree(&mut effects);
            }
            (
                Then::BoardArchived {
                    agent, archived, ..
                },
                Ok(Reply::Agents(agents)),
            ) if agents
                .iter()
                .all(|a| a.id == agent && a.archived == archived) =>
            {
                if let Some(node) = self.tree.nodes.get_mut(&agent) {
                    node.agent.archived = archived;
                }
                if archived {
                    // Keep the chat off the removed row even when archived rows are shown.
                    if self.selected == Some(agent) {
                        let next = self
                            .tree
                            .nodes
                            .values()
                            .filter(|n| n.agent.id != agent && !n.agent.archived)
                            .min_by_key(|n| {
                                (
                                    n.agent.status.is_terminal(),
                                    n.agent.parent.is_some(),
                                    n.agent.created_at,
                                )
                            })
                            .map(|n| n.agent.id);
                        self.selected = None;
                        if let Some(next) = next {
                            effects.extend(self.select(next));
                        }
                    }
                    if self.board.cursor == Some(agent) {
                        self.board.cursor = self
                            .tree
                            .nodes
                            .values()
                            .find(|n| n.agent.id != agent && !n.agent.archived)
                            .map(|n| n.agent.id);
                    }
                    self.flash(self.theme.decoration(
                        "removed from board · . shows archived; shift+d restores",
                        "removed from board | . shows archived; shift+d restores",
                    ));
                } else {
                    self.flash("restored to the board");
                }
                self.refresh_tree(&mut effects);
            }
            (then, result) => {
                let reason = match result {
                    Err(error) => error.to_string(),
                    Ok(_) => "unexpected daemon reply".to_owned(),
                };
                let action = match then {
                    Then::BoardStopped { remove: true, .. } => {
                        "stop did not complete; removal was not attempted"
                    }
                    Then::BoardStopped { remove: false, .. } => "stop did not complete",
                    Then::BoardArchived {
                        after_stop: true, ..
                    } => "stop requested, but removal did not complete",
                    Then::BoardArchived { archived: true, .. } => "removal did not complete",
                    _ => "restore did not complete",
                };
                self.flash_error(format!("{action}: {reason}"));
            }
        }
        effects
    }
}
