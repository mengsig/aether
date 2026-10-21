//! What the operator wants viewed. Their own actions set it; a tree only
//! fills it while nothing is wanted, and drops a viewed agent only once a
//! tree requested after this client met that agent leaves it out.

use std::path::Path;

use super::{AgentId, App, Command, Effect, StartId, Then};

/// What the operator wants viewed; `selected` is what is viewed now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Intent {
    /// No tree yet: the first one picks the view, `--agent`'s agent first.
    FirstTree,
    /// The selection as it stands; with nothing selected, a tree may show a live root.
    Selection,
    /// The root start `StartId` makes, viewed once it exists.
    Start(StartId),
}

impl App {
    /// The operator chose to view `agent`.
    pub fn select(&mut self, agent: AgentId) -> Vec<Effect> {
        self.intend(Intent::Selection);
        self.show(agent)
    }

    /// The operator wants `intent` now; a failed start they leave is dropped.
    pub(super) fn intend(&mut self, intent: Intent) {
        let left = std::mem::replace(&mut self.intent, intent);
        if let Intent::Start(start) = left
            && left != intent
        {
            self.abandon_start(start);
        }
    }

    /// The start whose root the operator waits to view.
    pub(super) fn awaited_start(&self) -> Option<StartId> {
        match self.intent {
            Intent::Start(start) => Some(start),
            Intent::FirstTree | Intent::Selection => None,
        }
    }

    /// `agent` on screen, its history fetched the first time.
    fn show(&mut self, agent: AgentId) -> Vec<Effect> {
        if self.selected != Some(agent) {
            self.voice.selection += 1;
        }
        self.swap_drafts(agent);
        self.selected = Some(agent);
        self.dirty = true;
        let mut effects: Vec<Effect> = self.backfill_request(agent).into_iter().collect();
        if !self.context_tokens.contains_key(&agent) {
            effects.push(Effect::Request {
                command: Command::ListInvocations { agent },
                then: Then::Invocations(agent),
            });
        }
        effects
    }

    /// Nothing viewed: the composer's draft is parked with its agent, so
    /// what is typed next belongs to whichever agent is shown or created next.
    pub(super) fn clear_selection(&mut self) {
        self.selected = None;
        let Some(owner) = self.draft_owner.take() else {
            return;
        };
        self.close_popup();
        self.input.stale_reads += std::mem::take(&mut self.input.image_reads);
        let outgoing = self.input.swap_draft(Default::default());
        if !outgoing.is_empty() && !self.gone(owner) {
            self.drafts.insert(owner, outgoing);
        }
    }

    /// Park the composer's draft under its owner and bring back `agent`'s.
    /// A draft typed while no agent owned the composer goes to `agent`.
    fn swap_drafts(&mut self, agent: AgentId) {
        if self.draft_owner == Some(agent) {
            return;
        }
        self.close_popup();
        let Some(leaving) = self.draft_owner else {
            self.draft_owner = Some(agent);
            let Some(parked) = self.drafts.remove(&agent) else {
                return;
            };
            if self.input.text.is_empty() && self.input.images.is_empty() {
                self.input.swap_draft(parked);
                return;
            }
            let merged = !parked.is_empty();
            self.input.absorb_draft(parked);
            if merged {
                self.flash("an earlier draft for this agent was added below what you typed");
            }
            return;
        };
        self.input.stale_reads += std::mem::take(&mut self.input.image_reads);
        let incoming = self.drafts.remove(&agent).unwrap_or_default();
        let outgoing = self.input.swap_draft(incoming);
        if !outgoing.is_empty() {
            self.drafts.insert(leaving, outgoing);
        }
        self.draft_owner = Some(agent);
        debug_assert!(!self.drafts.contains_key(&agent));
    }

    /// A reply named `agent`, which the tree may not show yet: a tree
    /// requested before now cannot say it is gone.
    pub(super) fn met(&mut self, agent: AgentId) {
        if !self.tree.nodes.contains_key(&agent) {
            self.unconfirmed.entry(agent).or_insert(self.tree_requests);
        }
    }

    /// Whether the tree says `agent` no longer exists.
    pub(super) fn gone(&self, agent: AgentId) -> bool {
        !self.tree.nodes.contains_key(&agent) && !self.unconfirmed.contains_key(&agent)
    }

    /// A new tree confirms the met agents it shows, and rules out those it
    /// leaves out if it was requested after they were met.
    pub(super) fn settle_unconfirmed(&mut self) {
        // `refresh_tree` keeps one request outstanding: this tree answers the latest.
        let answers = self.tree_requests;
        let nodes = &self.tree.nodes;
        self.unconfirmed
            .retain(|id, met_at| !nodes.contains_key(id) && answers <= *met_at);
    }

    /// Bring the view in line with a new tree without overriding the operator:
    /// a root is adopted, or the launch workspace's started, only while nothing is wanted.
    pub(super) fn reconcile_view(&mut self, effects: &mut Vec<Effect>) {
        // Gone for a reason this client did not pick (another client, `--all-finished`):
        // cleared, then re-chosen below like any empty view.
        if self.selected.is_some_and(|id| self.gone(id)) {
            self.clear_selection();
        }
        let first = self.intent == Intent::FirstTree;
        if first {
            self.intent = Intent::Selection;
        }
        // `--agent` picks the first view only, and never over the operator's own choice.
        let focus = self.options.focus.take().filter(|_| first);
        // An awaited start, failed or not, rules out adopting a root or starting another.
        if self.intent != Intent::Selection {
            return;
        }
        // On the first tree, a launch directory with no live root of its own
        // gets one, rather than the operator landing in another project.
        let startup_create = first
            && self.options.auto_create_root
            && !self.root_create_attempted
            && focus.is_none()
            && self.active_workspace().is_some();
        if self.selected.is_none() {
            let candidate = focus
                .filter(|id| self.tree.nodes.get(id).is_some_and(|n| !n.agent.archived))
                .or_else(|| {
                    let here = self.active_workspace()?.to_path_buf();
                    self.latest_live_root_in(&here)
                })
                .or_else(|| {
                    self.tree
                        .roots
                        .iter()
                        .copied()
                        .find(|id| !startup_create && !self.is_terminal(*id))
                });
            if let Some(id) = candidate {
                effects.extend(self.show(id));
            }
        }
        let any_live = self
            .tree
            .nodes
            .values()
            .any(|n| !n.agent.status.is_terminal());
        let wanted = !any_live || (startup_create && self.selected.is_none());
        if self.options.auto_create_root && wanted && !self.root_create_attempted {
            let here = self.active_workspace().map(Path::to_path_buf);
            effects.extend(self.await_root_in(here, None));
        }
    }
}
