//! Daemon events: the transcript cells come from `transcript::ingest_event`;
//! this adds the side effects (approvals, attention, voice, refreshes).
use super::{App, ApprovalRequest, ApprovalStatus, Effect, Event, EventKind, transcript};
use crate::ui::views::ViewKind;

impl App {
    pub(super) fn on_event(&mut self, event: Event) -> Vec<Effect> {
        if self.options.is_remote() && self.last_seq.is_some_and(|last| event.seq <= last) {
            return Vec::new();
        }
        let mut effects = Vec::new();
        self.backfill_note(&event);
        // Another agent's event only moves board rows and counts: no rush.
        if event.agent_id.is_none() || event.agent_id == self.selected {
            self.dirty = true;
        } else {
            self.background_dirty = true;
        }
        self.last_seq = Some(event.seq.max(self.last_seq.unwrap_or(0)));
        let family = event.kind.family();
        let agent = event.agent_id;
        if matches!(
            event.kind,
            EventKind::InvocationCompleted { .. } | EventKind::InvocationFailed { .. }
        ) && let Some(agent) = agent
        {
            self.live_accounting.remove(&agent);
        }
        match &event.kind {
            EventKind::AgentUsageUpdated { total, .. } => {
                if let Some(agent) = agent {
                    let context = self.context_tokens.get(&agent).copied();
                    self.project_accounting(agent, *total, context);
                    self.live_accounting.remove(&agent);
                }
            }
            EventKind::ArtifactCreated { artifact } => {
                // Adopt into the list `/artifacts` shows when it is on this
                // agent, so a link names its target the moment it appears.
                if agent.is_some()
                    && agent == self.artifacts.for_agent.or(self.selected)
                    && !self.artifacts.list.iter().any(|a| a.id == artifact.id)
                {
                    self.artifacts.list.push(artifact.clone());
                }
            }
            EventKind::ApprovalRequested {
                approval,
                call,
                tool,
                description,
                argument,
                side_effect,
            } => {
                if let Some(agent) = agent
                    && !self.approvals.iter().any(|a| a.id == *approval)
                {
                    self.approvals_revision = self.approvals_revision.wrapping_add(1);
                    // The event carries no `resources`; the list request below
                    // replaces this placeholder with the daemon's full record.
                    self.approvals.push(ApprovalRequest {
                        id: *approval,
                        agent_id: agent,
                        task_id: event.task_id,
                        call_id: *call,
                        tool: tool.clone(),
                        description: description.clone(),
                        argument: argument.clone(),
                        resources: Vec::new(),
                        side_effect: *side_effect,
                        requested_at: event.at,
                        status: ApprovalStatus::Pending,
                    });
                    effects.push(self.approvals_request());
                    self.attention(&mut effects, agent, "an approval is waiting");
                    self.rebuild_board();
                }
            }
            EventKind::ApprovalResolved { approval, .. } => {
                self.approvals_revision = self.approvals_revision.wrapping_add(1);
                self.approvals.retain(|a| a.id != *approval);
                self.rebuild_board();
            }
            EventKind::QuestionAsked { .. } => {
                if let Some(agent) = agent {
                    self.attention(&mut effects, agent, "an agent has a question");
                }
            }
            EventKind::AgentTurnCompleted { next, .. } => {
                let over = |agent| {
                    self.transcripts
                        .get(&agent)
                        .is_none_or(|t| t.turn_over(*next))
                };
                if let Some(agent) = agent.filter(|&agent| over(agent)) {
                    if Some(agent) == self.selected {
                        self.attention(&mut effects, agent, "the turn finished");
                    }
                    effects.extend(self.composer_turn_ended(agent, *next, event.seq));
                }
            }
            EventKind::InvocationCompleted { usage, .. } => {
                let carried = usage.context_tokens();
                if let Some(agent) = agent
                    && carried > 0
                {
                    self.context_tokens.insert(agent, carried);
                }
            }
            EventKind::Custom { name, .. } if name.starts_with("schedule.") => {
                if self.bottom.showing(ViewKind::Schedules) {
                    if self.schedules.loading {
                        self.schedules.refresh_pending = true;
                    } else {
                        effects.push(self.schedules_request());
                    }
                }
            }
            EventKind::Custom { name, payload } if name == "context.usage" => {
                if let Some(agent) = agent
                    && let Some(tokens) = payload["tokens"].as_u64()
                {
                    self.context_tokens.insert(agent, tokens);
                    if let Some(reading) = self.live_accounting.get_mut(&agent) {
                        reading.context_tokens = Some(tokens);
                    }
                    if let Some(node) = self.tree.nodes.get_mut(&agent) {
                        node.agent.stats.context_tokens = Some(tokens);
                    }
                }
            }
            EventKind::ProviderQuota {
                provider,
                window,
                usage,
            } => {
                self.remember_usage(
                    provider,
                    usage
                        .clone()
                        .unwrap_or_else(|| aether_core::provider::ProviderUsage {
                            observed_at: event.at,
                            windows: vec![aether_core::provider::UsageWindow {
                                name: "quota".into(),
                                quota: *window,
                            }],
                        }),
                );
                let latest = &self.quotas[provider];
                for account in self
                    .providers
                    .iter_mut()
                    .filter(|account| account.provider == *provider)
                {
                    account.health.quota = latest.preferred_window();
                    account.health.usage = Some(latest.clone());
                }
            }
            _ => {}
        }
        if self.note_unlisted_schedule(&event) {
            effects.extend(self.schedule_names_request());
        }
        let ctx = transcript::IngestCtx {
            tree: &self.tree,
            now: self.now,
        };
        if let Some(alert) = transcript::ingest_event(&mut self.transcripts, &event, &ctx) {
            self.quota_alert = Some(alert);
        }
        // After ingest: a spoken question must find its own cell to stay live.
        if let (EventKind::QuestionAsked { question }, Some(agent)) = (&event.kind, agent) {
            self.voice_question(agent, event.seq, question);
        }
        if matches!(
            family.as_str(),
            "agent" | "task" | "lease" | "approval" | "recovery"
        ) {
            self.refresh_tree(&mut effects);
        }
        effects
    }
}
