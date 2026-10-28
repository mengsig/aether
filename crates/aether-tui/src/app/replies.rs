//! Application replies.
use super::{ClientError, Effect, Reply, Then, explain, then_operation};
use crate::app::App;

mod data;
mod lifecycle;
mod memory;
mod model;

/// Where a reply goes after the dedicated handlers have had first refusal.
enum Routed {
    Done(Vec<Effect>),
    General(Box<(Then, Result<Reply, ClientError>)>),
}

impl App {
    pub(super) fn on_reply(
        &mut self,
        then: Then,
        result: Result<Reply, ClientError>,
    ) -> Vec<Effect> {
        let then = self.settle_remote_ack(then, &result);
        if matches!(result, Err(ClientError::DeliveryUnconfirmed(_)))
            && !matches!(then, Then::MessageSent { .. })
        {
            self.delivery_unconfirmed(&then, &result);
            return Vec::new();
        }
        let (then, result) = match self.route_dedicated(then, result) {
            Routed::Done(effects) => return effects,
            Routed::General(general) => *general,
        };
        let mut effects = Vec::new();
        if self.is_stale_operator_reply(&then) {
            self.stale_operator_reply(then, result, &mut effects);
            return effects;
        }
        if let Then::ScheduleHistory(id) = then
            && !self.wants_schedule_history(id)
        {
            return effects;
        }
        self.record_reply_status(&then, &result);
        let reply = match result {
            Ok(reply) => reply,
            Err(ClientError::Disconnected) => return self.reply_disconnected(then),
            Err(e) => return self.reply_failed(e, then),
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
        self.apply_lifecycle_reply(then, reply, &mut effects);
        effects
    }

    fn settle_remote_ack(&mut self, then: Then, result: &Result<Reply, ClientError>) -> Then {
        let Then::RemoteAck { key, next } = then else {
            return then;
        };
        if matches!(result, Ok(_) | Err(ClientError::Protocol(_))) {
            self.remote_pending.remove(&key);
        }
        *next
    }

    fn delivery_unconfirmed(&mut self, then: &Then, result: &Result<Reply, ClientError>) {
        self.flash_error(format!(
            "{}: {}; delivery unconfirmed; refresh host state before retrying",
            then_operation(then),
            result
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default()
        ));
    }

    fn route_dedicated(&mut self, then: Then, result: Result<Reply, ClientError>) -> Routed {
        if let Then::Host(context) = then {
            return Routed::Done(self.host_reply(context, result));
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
            return Routed::Done(self.board_lifecycle_reply(then, result));
        }
        if then == Then::Tree
            && !self.board.pending_removals.is_empty()
            && let Err(error) = &result
        {
            self.board.pending_removals.clear();
            self.flash_error(format!("stop requested; removal did not complete: {error}"));
            self.tree_in_flight = false;
            return Routed::Done(Vec::new());
        }
        if let Then::VoiceReply(id) = then {
            return Routed::Done(self.voice_send_result(id, result));
        }
        if let Then::Backfill { agent, connection } = then {
            return Routed::Done(self.on_backfill(agent, connection, result));
        }
        if then.is_notification() {
            return Routed::Done(self.notification_reply(&then, result));
        }
        if let Then::QueuedSent(agent) = then {
            self.queued_sent(agent, result.err());
            return Routed::Done(Vec::new());
        }
        Routed::General(Box::new((then, result)))
    }

    fn is_stale_operator_reply(&self, then: &Then) -> bool {
        match then {
            Then::AgentRenamed(id) => self.selected != Some(*id),
            Then::MemoryCreated { selected } => self.selected != *selected,
            _ => false,
        }
    }

    fn stale_operator_reply(
        &mut self,
        then: Then,
        result: Result<Reply, ClientError>,
        effects: &mut Vec<Effect>,
    ) {
        match result {
            Ok(_) => {
                self.refresh_tree(effects);
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
    }

    fn record_reply_status(&mut self, then: &Then, result: &Result<Reply, ClientError>) {
        if *then == Then::Usage {
            self.usage_loading = false;
            self.usage_error = result.as_ref().err().map(ToString::to_string);
            self.dirty = true;
        }
        // Guarded by `run`: a slower reply to a report the user has since
        // closed or replaced must not clobber the one now on screen.
        if let Then::RunReport(run) = then
            && self.report.run == Some(*run)
        {
            self.report.loading = false;
            self.report.error = result.as_ref().err().map(ToString::to_string);
        }
        if matches!(
            then,
            Then::Schedules | Then::ScheduleHistory(_) | Then::ScheduleChanged
        ) {
            if *then == Then::Schedules {
                self.schedules.loading = false;
            }
            self.schedules.error = result.as_ref().err().map(ToString::to_string);
        }
    }

    fn reply_disconnected(&mut self, then: Then) -> Vec<Effect> {
        if matches!(then, Then::MessageSent { .. }) {
            self.disconnect_reply_noticed = true;
            self.disconnect_notice_shown = true;
            return self.reply_failed(ClientError::Disconnected, then);
        }
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
            return Vec::new();
        }
        if then == Then::Tree {
            self.tree_in_flight = false;
        }
        if let Then::ApprovalResolved(id) = then {
            self.resolving.remove(&id);
            self.dirty = true;
        }
        if let Then::PendingRootModelPatched(id) = then {
            self.pending_root_model_disconnected(id);
            return Vec::new();
        }
        if let Then::ModelPatched(id) = then {
            self.model_patches_in_flight.remove(&id);
            self.queued_model_patches.remove(&id);
            self.flash_error(
                "model change outcome unknown after disconnect; reconnect and check the agent before retrying /model",
            );
            return Vec::new();
        }
        // These already surface the disconnect through a dedicated error field.
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
        Vec::new()
    }

    fn reply_failed(&mut self, e: ClientError, then: Then) -> Vec<Effect> {
        if let Then::MessageSent { agent, message } = then {
            let had_images = !message.images.is_empty();
            self.restore_agent_message(agent, message);
            let recovery = if !matches!(e, ClientError::Protocol(_)) {
                "your message is back in the draft; delivery uncertain: check the transcript before sending it again"
            } else if had_images {
                "your message is back in the draft - remove the images (Backspace at the start of the draft or /image --clear) to send the text"
            } else {
                "your message is back in the draft"
            };
            self.flash_error(format!("{e}; {recovery}"));
            return Vec::new();
        }
        let mut effects = Vec::new();
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
            // Only the start still on screen owns the composer; an abandoned one is reported unsent.
            if matches!(&e, ClientError::Protocol(reply) if reply.code == aether_protocol::ErrorCode::NoModel)
                && self.awaited_start() == Some(start)
            {
                self.restore_root_start(start);
                self.flash_error(message);
                self.keep_flash_until_key();
                return effects;
            }
            match self.root_start_failed(start) {
                Some(kept) => self.flash_error(format!("{message}; {kept}")),
                None => self.flash_error(message),
            }
            return effects;
        }
        if let Then::PendingRootModelPatched(id) = then {
            effects.extend(self.pending_root_model_failed(id, &e));
            return effects;
        }
        if let Then::ModelPatched(id) = then {
            self.model_patch_failed(id, &e, &mut effects);
            return effects;
        }
        self.flash_error(explain(&self.theme, &e, then, &self.options.command_hint()));
        effects
    }
}
