//! Roots this client starts, and what waits for them. Each start has an
//! identity its reply carries back, so a reply settles only its own start;
//! messages and a model chosen before the agent exists are bound to that
//! start, then held for that agent alone until its model change lands.

use std::path::{Path, PathBuf};

use super::images::Outgoing;
use super::{
    Agent, AgentConfig, AgentId, AgentPatch, App, ApprovalPolicy, Command, CreateAgent,
    DaemonStatus, Effect, Intent, ModelChain, Role, Scope, Then,
};
use crate::workspace;

/// Names one root creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StartId(u64);

/// A root being started, or, while the operator still waits on it, one
/// whose start failed and waits for a retry.
#[derive(Debug, Default)]
pub(super) struct RootStart {
    /// Where it works; `None` leaves it to the daemon.
    workspace: Option<PathBuf>,
    /// Sent to it once it exists, oldest first.
    messages: Vec<Outgoing>,
    /// Applied before any of those messages.
    pub(super) model: Option<ModelChain>,
    /// Its last request failed and none is outstanding.
    failed: bool,
    /// Its defaults arrived after the link was lost; the next connection resumes it.
    parked: bool,
}

/// A new root whose messages wait for its model change.
#[derive(Debug)]
pub(super) struct HeldRoot {
    /// Where it was started, for the notice if its messages are dropped.
    workspace: Option<PathBuf>,
    messages: Vec<Outgoing>,
    /// Chosen while a change was in flight; sent once that one settles.
    next_model: Option<ModelChain>,
    patching: bool,
}

impl App {
    /// `/new`: a root of its own, even beside one already starting there.
    pub(super) fn start_new_root(
        &mut self,
        objective: Option<Outgoing>,
        workspace: Option<PathBuf>,
    ) -> Effect {
        let id = StartId(self.next_start);
        self.next_start += 1;
        if workspace.is_some() {
            self.workspace.clone_from(&workspace);
        }
        let start = RootStart {
            workspace: workspace.clone(),
            messages: objective.into_iter().collect(),
            ..Default::default()
        };
        self.root_starts.insert(id, start);
        self.intend(Intent::Start(id));
        self.request_root(id, workspace)
    }

    /// Wait for a root in `workspace`, `message` queued for it: the one
    /// already starting there, a failed one retried, else a new one.
    pub(super) fn await_root_in(
        &mut self,
        workspace: Option<PathBuf>,
        message: Option<Outgoing>,
    ) -> Option<Effect> {
        if self.options.is_remote() && workspace.is_none() {
            self.flash_error("choose a host workspace with /workspace before starting an agent");
            return None;
        }
        let remote = self.options.is_remote();
        let canonical = |path: &Path| {
            if remote {
                path.to_owned()
            } else {
                workspace::canonical(path)
            }
        };
        let wanted = workspace.as_deref().map(canonical);
        let here = |start: &RootStart| start.workspace.as_deref().map(canonical) == wanted;
        let existing = self
            .awaited_start()
            .filter(|id| self.root_starts.get(id).is_some_and(here))
            .or_else(|| {
                let mut starts = self.root_starts.iter();
                starts.find(|(_, start)| here(start)).map(|(id, _)| *id)
            });
        let Some((id, start)) = existing.and_then(|id| Some((id, self.root_starts.get_mut(&id)?)))
        else {
            return Some(self.start_new_root(message, workspace));
        };
        start.messages.extend(message);
        let retry = start.failed.then(|| start.workspace.clone());
        if workspace.is_some() {
            self.workspace = workspace;
        }
        self.intend(Intent::Start(id));
        retry.map(|workspace| self.request_root(id, workspace))
    }

    /// The operator moved on from start `id`: one still in flight stays for
    /// its reply; a failed one, which nothing would retry, is dropped out loud.
    pub(super) fn abandon_start(&mut self, id: StartId) {
        if !self.root_starts.get(&id).is_some_and(|start| start.failed) {
            return;
        }
        let start = self.root_starts.remove(&id).unwrap_or_default();
        self.report_unsent(
            start.messages.len(),
            start.workspace.as_deref(),
            "starting its agent did not complete",
        );
    }

    /// Say that `count` messages for `workspace` will never be sent, and `why`.
    fn report_unsent(&mut self, count: usize, workspace: Option<&Path>, why: &str) {
        if let Some(note) = self.unsent(count, workspace) {
            self.flash_error(format!("{note}: {why}"));
        }
    }

    /// "N messages for <workspace> not sent", or `None` for no messages.
    pub(super) fn unsent(&self, count: usize, workspace: Option<&Path>) -> Option<String> {
        if count == 0 {
            return None;
        }
        let noun = if count == 1 { "message" } else { "messages" };
        let place = match workspace {
            Some(path) => format!(
                " for {}",
                workspace::display(path, self.options.home.as_deref())
            ),
            None => String::new(),
        };
        Some(format!("{count} {noun}{place} not sent"))
    }

    /// Whether the operator waits on a create request still outstanding in
    /// `path`, a canonical path; a failed one holds nothing back.
    pub(super) fn awaits_reply_in(&self, path: &Path) -> bool {
        self.awaited_start()
            .and_then(|id| self.root_starts.get(&id))
            .filter(|start| !start.failed)
            .and_then(|start| start.workspace.as_deref())
            .is_some_and(|dir| self.workspace_path(dir) == path)
    }

    /// The start the operator waits on, while its request is outstanding.
    pub(super) fn awaited_in_flight(&mut self) -> Option<&mut RootStart> {
        let id = self.awaited_start()?;
        self.root_starts.get_mut(&id).filter(|start| !start.failed)
    }

    /// The create request for start `id`. A root a person starts is always
    /// the coordinator of its own tree: no parent, and the coordinating role.
    fn request_root(&mut self, id: StartId, workspace: Option<PathBuf>) -> Effect {
        self.root_create_attempted = true;
        if let Some(start) = self.root_starts.get_mut(&id) {
            start.failed = false;
        }
        let Some(status) = &self.status else {
            return Effect::Request {
                command: Command::Status,
                then: Then::RootDefaults {
                    start: id,
                    link: self.link,
                },
            };
        };
        let network = status.network_default;
        Effect::Request {
            command: Command::CreateAgent(CreateAgent {
                role: Role::ASSISTANT.into(),
                name: Some(Role::ASSISTANT.into()),
                parent: None,
                model: self.options.default_model.clone(),
                fallbacks: self.options.default_fallbacks.clone(),
                scope: workspace.map(|dir| Scope {
                    workspace_root: Some(dir),
                    network,
                    ..Default::default()
                }),
                config: self.default_effort.map(|effort| AgentConfig {
                    effort: Some(effort),
                    ..Default::default()
                }),
                // Absent leaves it to the daemon's configured default.
                approval: self.options.yolo.map(|on| {
                    if on {
                        ApprovalPolicy::none()
                    } else {
                        ApprovalPolicy::default()
                    }
                }),
                start: true,
                ..Default::default()
            }),
            then: Then::AgentCreated(id),
        }
    }

    /// The daemon's defaults, read over `link`, arrived: ask for start `id`'s
    /// root now. Defaults from before a reconnect may have changed since.
    pub(super) fn root_defaults_arrived(
        &mut self,
        id: StartId,
        link: u64,
        status: DaemonStatus,
    ) -> Option<Effect> {
        debug_assert!(link <= self.link, "a reply over a link not yet made");
        let start = self.root_starts.get_mut(&id)?;
        debug_assert!(!start.parked, "a parked start has no request outstanding");
        // Anything sent now would go out on the lost link; the next connection resumes it.
        if !self.connected {
            start.parked = true;
            return None;
        }
        let workspace = start.workspace.clone();
        if link == self.link {
            self.status = Some(status);
        }
        let request = self.request_root(id, workspace);
        debug_assert!(
            !matches!(&request, Effect::Request { then: Then::RootDefaults { link, .. }, .. }
                if *link != self.link),
            "defaults are read again over the current link"
        );
        Some(request)
    }

    /// Starts parked while the link was lost, resumed once each on the new one.
    pub(super) fn resume_parked_starts(&mut self) -> Vec<Effect> {
        debug_assert!(self.connected, "resumed only once a link is up");
        let parked: Vec<_> = self
            .root_starts
            .iter_mut()
            .filter(|(_, start)| start.parked)
            .map(|(id, start)| {
                start.parked = false;
                (*id, start.workspace.clone())
            })
            .collect();
        let effects: Vec<_> = parked
            .into_iter()
            .map(|(id, workspace)| self.request_root(id, workspace))
            .collect();
        debug_assert!(
            effects.iter().all(|effect| matches!(effect,
                Effect::Request { then: Then::RootDefaults { link, .. }, .. } if *link == self.link)),
            "a resumed start reads the new link's defaults, never creates from old ones"
        );
        effects
    }

    /// The awaited start `id` was refused: its messages go back into the composer it owns.
    pub(super) fn restore_root_start(&mut self, id: StartId) {
        debug_assert_eq!(
            self.awaited_start(),
            Some(id),
            "the composer is not this start's"
        );
        self.root_create_attempted = false;
        let Some(start) = self.root_starts.get_mut(&id) else {
            return;
        };
        debug_assert!(!start.failed, "only an outstanding start can be rejected");
        start.failed = true;
        start.model = None;
        let messages = std::mem::take(&mut start.messages);
        let text = messages
            .iter()
            .map(|message| message.text.as_str())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let images = messages
            .into_iter()
            .flat_map(|message| message.images)
            .collect();
        self.input.restore_message(Outgoing { text, images });
        debug_assert!(self.input.cursor <= self.input.text.len());
    }

    /// Start `id` failed. While the operator waits on it, it keeps its
    /// workspace and messages for the next message to retry; otherwise it is
    /// dropped, and what it leaves unsent is returned to be said.
    pub(super) fn root_start_failed(&mut self, id: StartId) -> Option<String> {
        self.root_create_attempted = false;
        if self.awaited_start() != Some(id) {
            let start = self.root_starts.remove(&id)?;
            return self.unsent(start.messages.len(), start.workspace.as_deref());
        }
        let start = self.root_starts.get_mut(&id)?;
        start.failed = true;
        start.model = None;
        None
    }

    /// Start `id` made `agent`: view it if the operator still waits on it,
    /// then hand it what waited for it, unless the tree already shows it ended.
    pub(super) fn root_started(&mut self, id: StartId, agent: AgentId) -> Vec<Effect> {
        let start = self.root_starts.remove(&id).unwrap_or_default();
        self.met(agent);
        let mut effects = Vec::new();
        if self.awaited_start() == Some(id) {
            effects.extend(self.select(agent));
        }
        if let Some(why) = self.ended(agent) {
            let why = format!("its agent {why}");
            self.report_unsent(start.messages.len(), start.workspace.as_deref(), &why);
            return effects;
        }
        // Its model first: the messages wait until that change lands.
        if let Some(chain) = start.model {
            let held = HeldRoot {
                workspace: start.workspace,
                messages: start.messages,
                next_model: None,
                patching: true,
            };
            self.held_roots.insert(agent, held);
            effects.push(Self::root_model_request(agent, chain));
            return effects;
        }
        for message in start.messages {
            effects.extend(self.send_message(agent, message));
        }
        effects
    }

    /// The messages of `to`, a new root still waiting on its model change.
    pub(super) fn held_messages(&mut self, to: AgentId) -> Option<&mut Vec<Outgoing>> {
        self.held_roots.get_mut(&to).map(|held| &mut held.messages)
    }

    /// `/model` on a new root whose messages are held: changes run one at a
    /// time, a later choice waiting for the one in flight.
    pub(super) fn choose_held_root_model(
        &mut self,
        id: AgentId,
        chain: ModelChain,
    ) -> Option<Effect> {
        let held = self.held_roots.get_mut(&id)?;
        if held.patching {
            held.next_model = Some(chain.clone());
            self.flash(format!("{chain} queued for the new agent"));
            return None;
        }
        held.next_model = None;
        held.patching = true;
        Some(Self::root_model_request(id, chain))
    }

    /// A new root's model change landed: a later choice goes next, else its
    /// messages go out.
    pub(super) fn root_model_changed(&mut self, agent: &Agent) -> Vec<Effect> {
        let id = agent.id;
        if let Some(held) = self.held_roots.get_mut(&id) {
            held.patching = false;
            if let Some(next) = held.next_model.take()
                && next != agent.chain()
            {
                held.patching = true;
                return vec![Self::root_model_request(id, next)];
            }
        }
        self.remember_model(&agent.chain());
        self.flash(format!(
            "{} now on {}",
            self.agent_display_name(id),
            agent.model
        ));
        let mut effects = Vec::new();
        self.refresh_tree(&mut effects);
        let held = self.held_roots.remove(&id);
        for message in held.map(|held| held.messages).unwrap_or_default() {
            effects.extend(self.send_message(id, message));
        }
        effects
    }

    /// A new root's model change did not land: a later choice is tried, else
    /// its messages stay held for `/model` to retry.
    pub(super) fn root_model_failed(&mut self, id: AgentId) -> Option<Effect> {
        let held = self.held_roots.get_mut(&id)?;
        held.patching = false;
        let next = held.next_model.take()?;
        held.patching = true;
        Some(Self::root_model_request(id, next))
    }

    /// A new root's model change has an unknown outcome: stop waiting on it,
    /// messages still held, so `/model` can retry.
    pub(super) fn root_model_unknown(&mut self, id: AgentId) {
        if let Some(held) = self.held_roots.get_mut(&id) {
            held.patching = false;
        }
    }

    fn root_model_request(id: AgentId, chain: ModelChain) -> Effect {
        Effect::Request {
            command: Command::UpdateAgent {
                id,
                patch: AgentPatch {
                    model: Some(chain.primary),
                    fallbacks: Some(chain.fallbacks),
                    ..Default::default()
                },
                subtree: false,
                idempotency_key: None,
            },
            then: Then::PendingRootModelPatched(id),
        }
    }

    /// The model `/model` last chose for a new root still waiting on its change.
    pub(super) fn held_root_model(&self, id: AgentId) -> Option<Option<&ModelChain>> {
        self.held_roots
            .get(&id)
            .map(|held| held.next_model.as_ref())
    }

    pub(super) fn holds_root(&self, id: AgentId) -> bool {
        self.held_roots.contains_key(&id)
    }

    /// Why `agent` can no longer take messages held for it, if it cannot.
    fn ended(&self, agent: AgentId) -> Option<&'static str> {
        if self.gone(agent) {
            return Some("is gone");
        }
        let node = self.tree.nodes.get(&agent)?;
        if node.agent.archived {
            Some("was archived")
        } else if node.agent.restarted_as.is_some() {
            Some("was restarted")
        } else if node.agent.status.is_terminal() {
            Some("has ended")
        } else {
            None
        }
    }

    /// Drop the held messages of roots that ended or are gone, out loud; they
    /// are never handed to a successor.
    pub(super) fn release_ended_roots(&mut self) {
        let ended: Vec<_> = self
            .held_roots
            .keys()
            .filter_map(|id| Some((*id, self.ended(*id)?)))
            .collect();
        for (id, why) in ended {
            let Some(held) = self.held_roots.remove(&id) else {
                continue;
            };
            let why = format!("its agent {why}");
            self.report_unsent(held.messages.len(), held.workspace.as_deref(), &why);
        }
    }
}
