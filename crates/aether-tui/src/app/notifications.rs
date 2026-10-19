//! The daemon's notification center as the TUI holds it: listed once per
//! connection, then kept current by `notification.*` events.
use std::collections::BTreeSet;

use aether_core::event::Event;
use aether_core::id::{AgentId, NotificationId};
use aether_core::notification::{Notification, NotificationChange, NotificationState};
use aether_protocol::{Command, Reply};

use super::{App, ClientError, Effect, Then};

/// Most notifications kept; the oldest settled ones go first past it.
pub const MAX_KEPT: usize = 200;

#[derive(Debug, Default)]
pub struct NotificationCenter {
    /// The daemon announced `notifications`; without it nothing is asked or shown.
    served: bool,
    /// Newest first by creation.
    items: Vec<Notification>,
    /// While a list is in flight: ids changed live since it was asked for,
    /// which a list computed before those changes must not drop or roll back.
    live_since_list: Option<BTreeSet<NotificationId>>,
}

impl NotificationCenter {
    pub fn served(&self) -> bool {
        self.served
    }

    /// What this connection's daemon announced. One without the feature
    /// shows nothing, so nothing from an earlier daemon lingers.
    pub fn set_served(&mut self, served: bool) {
        self.served = served;
        if !served {
            self.items.clear();
            self.live_since_list = None;
        }
        debug_assert!(
            served || self.unread() == 0,
            "an old daemon shows no indicator"
        );
    }

    pub fn items(&self) -> &[Notification] {
        &self.items
    }

    pub fn get(&self, id: NotificationId) -> Option<&Notification> {
        self.items.iter().find(|n| n.id == id)
    }

    pub fn unread(&self) -> usize {
        self.unread_matching(|_| true)
    }

    /// Unread questions and blockers: work waiting on the operator.
    pub fn urgent_unread(&self) -> usize {
        self.unread_matching(|n| n.kind.is_urgent())
    }

    fn unread_matching(&self, keep: impl Fn(&Notification) -> bool) -> usize {
        self.items
            .iter()
            .filter(|n| n.state == NotificationState::Unread && keep(n))
            .count()
    }

    /// Unread, then read, then resolved; newest first within each.
    pub fn grouped(&self) -> Vec<&Notification> {
        let mut out: Vec<&Notification> = Vec::with_capacity(self.items.len());
        for state in NotificationState::ALL {
            out.extend(self.items.iter().filter(|n| n.state == state));
        }
        debug_assert_eq!(out.len(), self.items.len(), "every state is a group");
        out
    }

    /// Adopt `record` unless an equal or newer version is already held.
    /// Returns whether anything changed.
    pub fn apply(&mut self, record: Notification) -> bool {
        if let Some(held) = self.items.iter_mut().find(|n| n.id == record.id) {
            if held.updated_at > record.updated_at || *held == record {
                return false;
            }
            *held = record;
            return true;
        }
        let at = self
            .items
            .partition_point(|n| (n.created_at, n.id) > (record.created_at, record.id));
        self.items.insert(at, record);
        self.trim();
        debug_assert!(self.items.len() <= MAX_KEPT, "the center stays bounded");
        true
    }

    /// A list was asked for: changes from here on outrank it.
    fn list_requested(&mut self) {
        self.live_since_list = Some(BTreeSet::new());
    }

    /// A record changed live; it outranks a list still in flight.
    fn changed_live(&mut self, record: Notification) -> bool {
        if let Some(live) = self.live_since_list.as_mut() {
            live.insert(record.id);
        }
        self.apply(record)
    }

    /// Replace the held records with a fresh list, keeping what changed live
    /// after the list was asked for.
    fn listed(&mut self, listed: Vec<Notification>) {
        let live_ids = self.live_since_list.take().unwrap_or_default();
        let live: Vec<Notification> = std::mem::take(&mut self.items)
            .into_iter()
            .filter(|n| live_ids.contains(&n.id))
            .collect();
        for record in listed {
            self.apply(record);
        }
        for record in live {
            self.apply(record);
        }
        debug_assert!(
            self.items
                .windows(2)
                .all(|w| (w[0].created_at, w[0].id) > (w[1].created_at, w[1].id)),
            "newest first, no duplicates"
        );
    }

    /// Past [`MAX_KEPT`], drop the oldest resolved, then read, then unread.
    fn trim(&mut self) {
        while self.items.len() > MAX_KEPT {
            let victim = NotificationState::ALL
                .iter()
                .rev()
                .find_map(|state| self.items.iter().rposition(|n| n.state == *state));
            let Some(index) = victim else {
                return;
            };
            self.items.remove(index);
        }
    }
}

impl App {
    /// The connect snapshot's first list, of what is still open, when the
    /// daemon serves notifications. Resolved history is asked for after it.
    pub(super) fn notifications_request(&mut self) -> Option<Effect> {
        if !self.notifications.served {
            return None;
        }
        self.notifications.list_requested();
        Some(list(
            vec![NotificationState::Unread, NotificationState::Read],
            Then::Notifications,
        ))
    }

    /// A `notification.*` event: adopt the record it carries.
    pub(super) fn on_notification_event(&mut self, event: &Event, effects: &mut Vec<Effect>) {
        let Some((change, record)) = Notification::of(&event.kind) else {
            tracing::debug!(seq = event.seq, "undecodable notification event");
            return;
        };
        if !self.notifications.served {
            return;
        }
        let urgent_post = change == NotificationChange::Posted && record.kind.is_urgent();
        let title = record.title.clone();
        if self.notifications.changed_live(record) {
            self.dirty = true;
        }
        if urgent_post && !self.terminal_focused {
            effects.push(Effect::Bell);
            effects.push(Effect::Notify {
                title: crate::ui_format!(
                    self.theme,
                    "aether · notification",
                    "aether | notification"
                ),
                body: title,
            });
        }
    }

    /// Replies to the lists and to every mark, resolve and answer.
    pub(super) fn notification_reply(
        &mut self,
        then: &Then,
        result: Result<Reply, ClientError>,
    ) -> Vec<Effect> {
        debug_assert!(
            then.is_notification(),
            "only notification replies come here"
        );
        let reply = match result {
            Ok(reply) => reply,
            // The disconnect itself is reported once; the next connection lists again.
            Err(ClientError::Disconnected) => return Vec::new(),
            Err(error) => {
                self.flash_error(format!("notifications: {error}"));
                return Vec::new();
            }
        };
        self.dirty = true;
        match (then, reply) {
            // Open ones first, so resolved history can never crowd them out of the cap.
            (Then::Notifications, Reply::Notifications(open)) => {
                self.notifications.listed(open);
                return vec![list(
                    vec![NotificationState::Resolved],
                    Then::ResolvedNotifications,
                )];
            }
            (Then::ResolvedNotifications, Reply::Notifications(resolved)) => {
                debug_assert!(
                    resolved
                        .iter()
                        .all(|n| n.state == NotificationState::Resolved),
                    "the daemon filtered by state"
                );
                for record in resolved {
                    self.notifications.apply(record);
                }
            }
            (Then::NotificationChanged, Reply::Notification(record)) => {
                self.notifications.apply(record);
            }
            (
                Then::NotificationAnswered,
                Reply::NotificationAnswered {
                    notification,
                    delivered_to,
                },
            ) => self.answered(notification, delivered_to),
            (then, reply) => {
                tracing::debug!(?then, ?reply, "unexpected notification reply");
                self.flash_error("notifications: the daemon sent an unexpected reply");
            }
        }
        Vec::new()
    }

    fn answered(&mut self, notification: Notification, delivered_to: Option<AgentId>) {
        debug_assert!(
            notification.answer.is_some(),
            "an answered reply carries the answer"
        );
        let to = delivered_to.map(|agent| self.agent_display_name(agent));
        self.notifications.apply(notification);
        self.flash(match to {
            Some(name) => format!("answered and resolved; {name} has the answer in its inbox"),
            None => "answered and resolved".to_owned(),
        });
    }
}

impl Then {
    pub(crate) fn is_notification(&self) -> bool {
        matches!(
            self,
            Then::Notifications
                | Then::ResolvedNotifications
                | Then::NotificationChanged
                | Then::NotificationAnswered
        )
    }
}

/// The newest [`MAX_KEPT`] notifications in `states`.
fn list(states: Vec<NotificationState>, then: Then) -> Effect {
    Effect::Request {
        command: Command::ListNotifications {
            states,
            source: None,
            limit: Some(MAX_KEPT),
        },
        then,
    }
}

/// Mark `id` seen.
pub fn mark_read(id: NotificationId) -> Effect {
    Effect::Request {
        command: Command::MarkNotificationRead { id },
        then: Then::NotificationChanged,
    }
}

pub fn resolve(id: NotificationId) -> Effect {
    Effect::Request {
        command: Command::ResolveNotification {
            id: Some(id),
            source: None,
            key: None,
        },
        then: Then::NotificationChanged,
    }
}

/// Answer with an offered action or the operator's own words; this resolves it.
pub fn answer(id: NotificationId, action: Option<String>, text: Option<String>) -> Effect {
    debug_assert!(
        action.is_some() || text.is_some(),
        "an answer says something"
    );
    Effect::Request {
        command: Command::AnswerNotification { id, action, text },
        then: Then::NotificationAnswered,
    }
}

#[cfg(test)]
mod tests;
