//! Replies that refresh cached daemon data: approvals, schedules, providers, artifacts.
use crate::app::{App, ApprovalStatus, Effect, Reply, Then, decode_artifact, eviction_notice};
use crate::ui::views::ViewKind;
use aether_core::artifact::Artifact;
use aether_core::id::{AgentId, ArtifactId};
use aether_core::schedule::Schedule;
use aether_core::tool::ApprovalRequest;
use aether_protocol::ProviderInfo;

impl App {
    pub(super) fn apply_data_reply(&mut self, then: Then, reply: Reply, effects: &mut Vec<Effect>) {
        match (then, reply) {
            (Then::Approvals { revision }, Reply::Approvals(list)) => {
                self.approvals_arrived(revision, list, effects);
            }
            (Then::RuleRevoked, Reply::ApprovalRule(rule)) => {
                self.flash(format!("revoked {}", rule.id));
                effects.push(self.rules_request());
            }
            (Then::RunReport(run), Reply::RunReport(report)) => {
                if self.report.run == Some(run) {
                    self.report.report = Some(report);
                }
            }
            (Then::ApprovalRules, Reply::ApprovalRules(rules)) => {
                self.approval_rules = rules;
            }
            (Then::McpServers, Reply::McpServers(servers)) => {
                self.mcp_servers = servers;
                self.mcp_refreshing = false;
            }
            (Then::Models, Reply::Models(models)) => {
                self.models = models;
            }
            (Then::Schedules, Reply::Schedules(jobs)) => self.schedules_arrived(jobs, effects),
            (Then::ScheduleHistory(id), Reply::ScheduleHistory(history)) => {
                self.schedules.history = Some((id, history));
            }
            (Then::ScheduleChanged, Reply::Schedule(job)) => self.schedule_changed(&job, effects),
            (Then::Providers | Then::Usage, Reply::Providers(providers)) => {
                self.providers_arrived(providers);
            }
            (Then::Auth, Reply::Auth(list)) => {
                self.auth = list;
            }
            (Then::Invocations(agent), Reply::Invocations(list)) => {
                self.invocations_arrived(agent, &list);
            }
            (Then::Artifacts(agent), Reply::Artifacts(list)) => {
                if self.artifacts.for_agent == Some(agent) {
                    self.artifacts.list = list;
                }
            }
            (
                Then::ArtifactBytes { id, read },
                Reply::ArtifactBytes {
                    artifact,
                    base64,
                    truncated,
                },
            ) => self.artifact_bytes_arrived(id, read, artifact, &base64, truncated),
            (Then::ApprovalResolved(id), _) => {
                self.approvals_revision = self.approvals_revision.wrapping_add(1);
                self.resolving.remove(&id);
                self.approvals.retain(|a| a.id != id);
                self.rebuild_board();
            }
            (then, reply) => self.apply_memory_reply(then, reply, effects),
        }
    }

    fn schedules_arrived(&mut self, jobs: Vec<Schedule>, effects: &mut Vec<Effect>) {
        self.schedules.jobs = jobs;
        self.names_version = crate::app::names_version(&self.tree, &self.schedules.jobs);
        effects.extend(self.after_schedules_for_view());
    }

    fn schedule_changed(&mut self, job: &Schedule, effects: &mut Vec<Effect>) {
        self.flash(format!("schedule {} updated", job.name));
        self.schedules.confirm_cancel = None;
        effects.push(self.schedules_request());
    }

    fn approvals_arrived(
        &mut self,
        revision: u64,
        list: Vec<ApprovalRequest>,
        effects: &mut Vec<Effect>,
    ) {
        if revision != self.approvals_revision {
            effects.push(self.approvals_request());
            return;
        }
        self.approvals = list
            .into_iter()
            .filter(|a| a.status == ApprovalStatus::Pending)
            .collect();
        self.rebuild_board();
    }

    fn providers_arrived(&mut self, mut providers: Vec<ProviderInfo>) {
        for provider in &mut providers {
            if let Some(report) = provider.health.usage.take() {
                self.remember_usage(&provider.provider, report);
            }
            if let Some(latest) = self.quotas.get(&provider.provider) {
                provider.health.quota = latest.preferred_window();
                provider.health.usage = Some(latest.clone());
            }
        }
        self.providers = providers;
    }

    fn invocations_arrived(&mut self, agent: AgentId, list: &[aether_core::snapshot::Invocation]) {
        if self.context_tokens.contains_key(&agent) {
            return;
        }
        if let Some(latest) = list
            .iter()
            .filter(|i| i.usage.context_tokens() > 0)
            .max_by_key(|i| i.started_at)
        {
            self.context_tokens
                .insert(agent, latest.usage.context_tokens());
        }
    }

    fn artifact_bytes_arrived(
        &mut self,
        id: ArtifactId,
        read: u64,
        artifact: Artifact,
        base64: &str,
        truncated: bool,
    ) {
        debug_assert!(
            read <= self.artifact_reads,
            "a reply to a read not yet made"
        );
        if self.artifacts.pending_bytes != Some((id, read)) {
            return;
        }
        self.artifacts.pending_bytes = None;
        // Authoritative: read from the store just now, unlike a
        // cached list entry or an `artifact.created` snapshot.
        if !artifact.has_bytes() || (base64.is_empty() && artifact.size_bytes > 0) {
            // Nothing will arrive, so a pager left open would say "downloading" forever.
            let pager = Some(ViewKind::ArtifactPager(id));
            self.bottom.view_stack.retain(|view| view.kind() != pager);
            self.flash(eviction_notice(&artifact, self.now));
        } else {
            self.artifacts.open = Some(decode_artifact(artifact, base64, truncated));
        }
    }
}
