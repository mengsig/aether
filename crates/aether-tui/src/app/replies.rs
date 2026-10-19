//! Application replies.
use super::{
    App, ApprovalStatus, ClientError, Command, Effect, Reply, Then, decode_artifact,
    eviction_notice, explain, then_operation,
};
use crate::ui::views::ViewKind;

impl App {
    pub(super) fn on_reply(
        &mut self,
        then: Then,
        result: Result<Reply, ClientError>,
    ) -> Vec<Effect> {
        let then = if let Then::RemoteAck { key, next } = then {
            if matches!(result, Ok(_) | Err(ClientError::Protocol(_))) {
                self.remote_pending.remove(&key);
            }
            *next
        } else {
            then
        };
        if matches!(result, Err(ClientError::DeliveryUnconfirmed(_))) {
            self.flash_error(format!(
                "{}: {}; delivery unconfirmed; refresh host state before retrying",
                then_operation(&then),
                result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default()
            ));
            return Vec::new();
        }
        if let Then::Host(context) = then {
            return self.host_reply(context, result);
        }
        let then = if then == Then::StatusPoll {
            self.status_poll_in_flight = false;
            Then::Status
        } else {
            then
        };
        if matches!(
            then,
            Then::BoardConfirm { .. } | Then::BoardStopped { .. } | Then::BoardArchived { .. }
        ) {
            return self.board_lifecycle_reply(then, result);
        }
        if then == Then::Tree
            && !self.board.pending_removals.is_empty()
            && let Err(error) = &result
        {
            self.board.pending_removals.clear();
            self.flash_error(format!("stop requested; removal did not complete: {error}"));
            self.tree_in_flight = false;
            return Vec::new();
        }
        if let Then::VoiceReply(id) = then {
            return self.voice_send_result(id, result);
        }
        if let Then::Backfill { agent, connection } = then {
            return self.on_backfill(agent, connection, result);
        }
        if let Then::QueuedSent(agent) = then {
            self.queued_sent(agent, result.err());
            return Vec::new();
        }
        let mut effects = Vec::new();
        let stale_operator_reply = match &then {
            Then::AgentRenamed(id) => self.selected != Some(*id),
            Then::MemoryCreated { selected } => self.selected != *selected,
            _ => false,
        };
        if stale_operator_reply {
            match result {
                Ok(_) => {
                    self.refresh_tree(&mut effects);
                    effects.push(self.memories_request());
                }
                // The selection moved before the reply landed, but the
                // mutation was still issued and still failed: name the
                // original operation and target (by id, since a name may be
                // stale) up front, since the daemon's own error text may not
                // mention either; never attribute it to the new selection.
                Err(e) => {
                    let target = match &then {
                        Then::AgentRenamed(id) | Then::MemoryCreated { selected: Some(id) } => {
                            Some(crate::ui::views::short_id(&self.theme, id))
                        }
                        _ => None,
                    };
                    let operation = then_operation(&then);
                    let prefix = match target {
                        Some(id) => format!("{operation} for {id}"),
                        None => operation.to_string(),
                    };
                    self.flash_error(format!(
                        "{prefix}: {}",
                        explain(&self.theme, &e, then, &self.options.command_hint())
                    ));
                }
            }
            return effects;
        }
        if let Then::ScheduleHistory(id) = then
            && !self.wants_schedule_history(id)
        {
            return effects;
        }
        if then == Then::Usage {
            self.usage_loading = false;
            self.usage_error = result.as_ref().err().map(ToString::to_string);
            self.dirty = true;
        }
        // Guarded by `run`: a slower reply to a report the user has since
        // closed or replaced must not clobber the one now on screen.
        if let Then::RunReport(run) = then
            && self.report.run == Some(run)
        {
            self.report.loading = false;
            self.report.error = result.as_ref().err().map(ToString::to_string);
        }
        if matches!(
            then,
            Then::Schedules | Then::ScheduleHistory(_) | Then::ScheduleChanged
        ) {
            if then == Then::Schedules {
                self.schedules.loading = false;
            }
            self.schedules.error = result.as_ref().err().map(ToString::to_string);
        }
        let reply = match result {
            Ok(reply) => reply,
            Err(ClientError::Disconnected) => {
                // One notice: the generic one or the runner's would replace it.
                if let Then::AgentCreated(start) | Then::RootDefaults { start, .. } = then
                    && let Some(unsent) = self.root_start_failed(start)
                {
                    self.disconnect_reply_noticed = true;
                    self.disconnect_notice_shown = true;
                    self.flash_error(crate::ui_format!(
                        self.theme,
                        "{} did not complete · the daemon connection dropped; {unsent}",
                        "{} did not complete | the daemon connection dropped; {unsent}",
                        then_operation(&then)
                    ));
                    return effects;
                }
                if then == Then::Tree {
                    self.tree_in_flight = false;
                }
                if let Then::ApprovalResolved(id) = then {
                    self.resolving.remove(&id);
                    self.dirty = true;
                }
                if let Then::PendingRootModelPatched(id) = then {
                    self.root_model_unknown(id);
                    self.flash_error(if self.holds_root(id) {
                        "new agent's model change did not complete; queued messages were held. Use /model to retry"
                    } else {
                        "new agent's model change did not complete"
                    });
                    return effects;
                }
                if let Then::ModelPatched(id) = then {
                    self.model_patches_in_flight.remove(&id);
                    self.queued_model_patches.remove(&id);
                    self.flash_error(
                        "model change outcome unknown after disconnect; reconnect and check the agent before retrying /model",
                    );
                    return effects;
                }
                // These already surface the disconnect through a dedicated
                // error field above; a second general notice would duplicate it.
                if !matches!(
                    then,
                    Then::Usage
                        | Then::RunReport(_)
                        | Then::Schedules
                        | Then::ScheduleHistory(_)
                        | Then::ScheduleChanged
                ) && !self.disconnect_reply_noticed
                {
                    self.disconnect_reply_noticed = true;
                    self.flash_error(crate::ui_format!(
                        self.theme,
                        "{} did not complete · the daemon connection dropped",
                        "{} did not complete | the daemon connection dropped",
                        then_operation(&then)
                    ));
                }
                return effects;
            }
            Err(e) => {
                if then == Then::Tree {
                    self.tree_in_flight = false;
                }
                // The answer did not land, so the card must come back rather
                // than leave the agent waiting on nothing.
                if let Then::ApprovalResolved(id) = then {
                    self.resolving.remove(&id);
                }
                // Creation failed: the next message tries again in the same
                // workspace instead of queueing behind an agent that will never arrive.
                if let Then::AgentCreated(start) | Then::RootDefaults { start, .. } = then {
                    let message = explain(&self.theme, &e, then, &self.options.command_hint());
                    match self.root_start_failed(start) {
                        Some(kept) => self.flash_error(format!("{message}; {kept}")),
                        None => self.flash_error(message),
                    }
                    return effects;
                }
                if let Then::PendingRootModelPatched(id) = then {
                    if let Some(retry) = self.root_model_failed(id) {
                        effects.push(retry);
                        return effects;
                    }
                    self.flash_error(if self.holds_root(id) {
                        format!(
                            "model change failed: {e}; queued messages were held. Use /model to retry"
                        )
                    } else {
                        format!("model change failed: {e}")
                    });
                    return effects;
                }
                if let Then::ModelPatched(id) = then {
                    let retry = self.finish_model_patch(id);
                    let message = explain(
                        &self.theme,
                        &e,
                        Then::ModelPatched(id),
                        &self.options.command_hint(),
                    );
                    if let Some(effect) = retry {
                        effects.push(effect);
                        self.flash_error(format!("{message}; trying the latest model choice"));
                    } else {
                        self.flash_error(message);
                    }
                    return effects;
                }
                self.flash_error(explain(&self.theme, &e, then, &self.options.command_hint()));
                return effects;
            }
        };
        // Polls and tree refreshes follow daemon activity, not keys: an unchanged
        // status is no frame at all, and the rest redraw at the background rate.
        if then == Then::Status
            && let Reply::Status(status) = reply
        {
            if self.status.as_ref() != Some(&status) {
                self.status = Some(status);
                self.background_dirty = true;
            }
            return effects;
        }
        if then == Then::Tree {
            self.background_dirty = true;
        } else {
            self.dirty = true;
        }
        match (then, reply) {
            (Then::RootDefaults { start, link }, Reply::Status(status)) => {
                effects.extend(self.root_defaults_arrived(start, link, status));
            }
            (Then::Tree, Reply::Tree(tree)) => {
                self.tree_in_flight = false;
                self.apply_tree(tree, &mut effects);
                if self.tree_stale {
                    effects.push(self.tree_request());
                }
            }
            (Then::AgentCreated(start), Reply::Agent(agent)) => {
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
                self.refresh_tree(&mut effects);
            }
            (Then::Restarted { from }, Reply::Agent(agent)) => {
                let id = agent.id;
                self.met(id);
                self.refresh_tree(&mut effects);
                if self.selected == Some(from) && self.intent == super::intent::Intent::Selection {
                    effects.extend(self.select(id));
                }
                self.flash(crate::ui_format!(
                    self.theme,
                    "restarted as {} · from its last checkpoint",
                    "restarted as {} | from its last checkpoint",
                    id
                ));
            }
            (Then::Woken, Reply::Agent(agent)) => {
                self.refresh_tree(&mut effects);
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
            (Then::Approvals { revision }, Reply::Approvals(list)) => {
                if revision != self.approvals_revision {
                    effects.push(self.approvals_request());
                    return effects;
                }
                self.approvals = list
                    .into_iter()
                    .filter(|a| a.status == ApprovalStatus::Pending)
                    .collect();
                self.rebuild_board();
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
            (Then::Schedules, Reply::Schedules(jobs)) => {
                self.schedules.jobs = jobs;
                self.names_version = super::names_version(&self.tree, &self.schedules.jobs);
                effects.extend(self.after_schedules_for_view());
            }
            (Then::ScheduleHistory(id), Reply::ScheduleHistory(history)) => {
                self.schedules.history = Some((id, history));
            }
            (Then::ScheduleChanged, Reply::Schedule(job)) => {
                self.flash(format!("schedule {} updated", job.name));
                self.schedules.confirm_cancel = None;
                effects.push(self.schedules_request());
            }
            (Then::Providers | Then::Usage, Reply::Providers(mut providers)) => {
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
            (Then::Memories, Reply::Memories(mut records)) => {
                // Pinned first, then current, then superseded/forgotten;
                // newest within each group.
                records.sort_by(|a, b| {
                    b.pinned
                        .cmp(&a.pinned)
                        .then(b.is_current().cmp(&a.is_current()))
                        .then(b.created_at.cmp(&a.created_at))
                });
                self.memories = records;
            }
            (Then::MemoryCreated { .. }, Reply::Memory(record)) => {
                let replaced = record
                    .supersedes
                    .map(|id| {
                        format!(
                            "; replaced {}",
                            crate::ui::views::short_id(&self.theme, &id)
                        )
                    })
                    .unwrap_or_default();
                self.flash(format!(
                    "operator memory {} admitted for {}{replaced}; /memory shows it (not agent-authored)",
                    crate::ui::views::short_id(&self.theme, &record.id),
                    crate::ui::views::memory::scope_label(&record.scope, &self.theme)
                ));
                effects.push(self.memories_request());
            }
            (Then::AgentRenamed(id), Reply::Agent(agent)) if id == agent.id => {
                self.flash(format!(
                    "renamed {} to {}",
                    crate::ui::views::short_id(&self.theme, &id),
                    agent.name
                ));
                self.refresh_tree(&mut effects);
            }
            (Then::MemoryMutated(action), Reply::Memory(record)) => {
                self.flash(format!(
                    "{action} {}",
                    crate::ui::views::short_id(&self.theme, &record.id)
                ));
                effects.push(self.memories_request());
            }
            (Then::Auth, Reply::Auth(list)) => {
                self.auth = list;
            }
            (Then::Invocations(agent), Reply::Invocations(list)) => {
                if !self.context_tokens.contains_key(&agent)
                    && let Some(latest) = list
                        .iter()
                        .filter(|i| i.usage.context_tokens() > 0)
                        .max_by_key(|i| i.started_at)
                {
                    self.context_tokens
                        .insert(agent, latest.usage.context_tokens());
                }
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
            ) => {
                debug_assert!(
                    read <= self.artifact_reads,
                    "a reply to a read not yet made"
                );
                if self.artifacts.pending_bytes == Some((id, read)) {
                    self.artifacts.pending_bytes = None;
                    // Authoritative: read from the store just now, unlike a
                    // cached list entry or an `artifact.created` snapshot.
                    if !artifact.has_bytes() || (base64.is_empty() && artifact.size_bytes > 0) {
                        // Nothing will arrive, so a pager left open would say "downloading" forever.
                        let pager = Some(ViewKind::ArtifactPager(id));
                        self.bottom.view_stack.retain(|view| view.kind() != pager);
                        self.flash(eviction_notice(&artifact, self.now));
                    } else {
                        self.artifacts.open = Some(decode_artifact(artifact, &base64, truncated));
                    }
                }
            }
            (Then::AgentPatched, Reply::Agent(agent)) => {
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
                self.refresh_tree(&mut effects);
            }
            (Then::ModelPatched(id), Reply::Agent(agent)) => {
                if agent.id != id {
                    self.model_patches_in_flight.remove(&id);
                    self.queued_model_patches.remove(&id);
                    self.flash(
                        "model change returned a different agent; check its model and retry /model",
                    );
                    return effects;
                }
                let next = self.finish_model_patch(id);
                self.remember_model(&agent.chain());
                if next.is_some() {
                    self.flash(format!(
                        "applying the latest model choice for {}",
                        self.agent_display_name(agent.id)
                    ));
                } else if agent.status.kind() == aether_core::agent::AgentStatusKind::Running {
                    self.flash(crate::ui_format!(
                        self.theme,
                        "{} set to {} · current turn continues; next turn uses it",
                        "{} set to {} | current turn continues; next turn uses it",
                        self.agent_display_name(agent.id),
                        agent.model
                    ));
                } else if agent.status.kind() == aether_core::agent::AgentStatusKind::Failed {
                    self.flash(format!(
                        "{} set to {}; /wake to retry",
                        self.agent_display_name(agent.id),
                        agent.model
                    ));
                } else {
                    self.flash(format!(
                        "{} set to {} for the next turn",
                        self.agent_display_name(agent.id),
                        agent.model
                    ));
                }
                self.refresh_tree(&mut effects);
                if let Some(effect) = next {
                    effects.push(effect);
                }
            }
            (Then::PendingRootModelPatched(id), Reply::Agent(agent)) => {
                if agent.id == id {
                    effects.extend(self.root_model_changed(&agent));
                } else {
                    self.root_model_unknown(id);
                    self.flash_error(if self.holds_root(id) {
                        "new agent's model change returned a different agent; queued messages were held"
                    } else {
                        "new agent's model change returned a different agent"
                    });
                }
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
                let notice = match agents.as_slice() {
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
                };
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
                    Ok(()) => match agents.len() {
                        0 => format!("{} was already there", self.agent_name(root)),
                        1 if on => format!("yolo on for {}", self.agent_name(root)),
                        1 => format!("yolo off for {}", self.agent_name(root)),
                        n if on => format!("yolo on for {n} agents in the subtree"),
                        n => format!("yolo off for {n} agents in the subtree"),
                    },
                };
                self.flash(notice);
                self.refresh_tree(&mut effects);
            }
            (Then::Archived, Reply::Agents(agents)) => {
                self.flash(match agents.len() {
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
                self.refresh_tree(&mut effects);
            }
            (Then::ApprovalResolved(id), _) => {
                self.approvals_revision = self.approvals_revision.wrapping_add(1);
                self.resolving.remove(&id);
                self.approvals.retain(|a| a.id != id);
                self.rebuild_board();
            }
            (Then::AgentRenamed(_), _) | (Then::MemoryCreated { .. }, _) => {
                self.flash_error("unexpected operator command reply; refresh /memory or the agent tree to verify the outcome");
            }
            (Then::PendingRootModelPatched(id), _) => {
                self.root_model_unknown(id);
                self.flash_error(if self.holds_root(id) {
                    "new agent's model change returned an unexpected reply; queued messages were held. Use /model to retry"
                } else {
                    "new agent's model change returned an unexpected reply"
                });
            }
            (Then::ModelPatched(id), _) => {
                self.model_patches_in_flight.remove(&id);
                self.queued_model_patches.remove(&id);
                self.flash_error(
                    "model change returned an unexpected reply; check the agent and retry /model",
                );
            }
            (Then::Ignore, _) => {}
            (Then::CompactionAccepted(agent), Reply::Ok) => {
                self.flash(aether_core::event::compaction_request_notice(&format!(
                    "aether events --agent {agent} --follow"
                )));
            }
            (Then::CompactionAccepted(_), _) => {
                self.flash_error(
                    "compaction request returned an unexpected reply; check the transcript",
                );
            }
            (then, reply) => {
                tracing::debug!("unhandled reply {:?} for {:?}", reply, then);
            }
        }
        effects
    }
}
