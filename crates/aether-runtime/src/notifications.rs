//! The notification center: posts from agents and services, the operator's
//! reads, resolutions and answers, each committed with its event. An answer
//! reaches a posting agent as an inbox trigger in the same commit.

use aether_core::event::Actor;
use aether_core::id::{AgentId, NotificationId, RunId};
use aether_core::inbox::InboxKind;
use aether_core::notification::{
    ANSWER_TRIGGER_SOURCE, Notification, NotificationChange, NotificationPost, check_service_source,
};
use aether_core::time::now;
use aether_store::{Commit, Mutation, NotificationFilter};
use serde_json::json;

use crate::{CommitHold, Result, Runtime, RuntimeError};

/// Resolved notifications kept; older ones are deleted when a new one is posted.
pub const MAX_RESOLVED_KEPT: usize = 500;

/// Who posts a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poster {
    Agent(AgentId),
    /// A service on the socket, by name, such as `archon`.
    Service(String),
}

/// What a post did: the record as it now stands, and the change it recorded,
/// `None` for a repost that changed nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Posted {
    pub notification: Notification,
    pub change: Option<NotificationChange>,
}

/// An answer as recorded, and the agent whose inbox received it, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Answered {
    pub notification: Notification,
    pub delivered_to: Option<AgentId>,
}

impl Runtime {
    /// Post `post`, or update the notification its key already names.
    pub fn post_notification(&self, poster: Poster, post: NotificationPost) -> Result<Posted> {
        post.check().map_err(RuntimeError::Invalid)?;
        let (source, by, run) = self.poster_identity(&poster)?;
        let at = now();
        let hold = self.hold();
        let existing = match &post.key {
            Some(key) => self.store.notification_by_key(&source, key)?,
            None => None,
        };
        let (notification, change) = match existing {
            Some(mut current) => {
                if !current.repost(post, at) {
                    return Ok(Posted {
                        notification: current,
                        change: None,
                    });
                }
                (current, NotificationChange::Updated)
            }
            None => (
                Notification::new(source, post, at),
                NotificationChange::Posted,
            ),
        };
        let mut commit = Commit::new();
        if change == NotificationChange::Posted {
            commit.push_mutation(Mutation::PruneResolvedNotifications {
                keep: MAX_RESOLVED_KEPT,
            });
        }
        self.commit_change(hold, commit, &notification, change, by, run)?;
        Ok(Posted {
            notification,
            change: Some(change),
        })
    }

    /// Mark a notification seen. Returns it as it now stands.
    pub fn mark_notification_read(&self, id: NotificationId, by: Actor) -> Result<Notification> {
        let hold = self.hold();
        let mut notification = self.stored_notification(id)?;
        if !notification.mark_read(now()) {
            return Ok(notification);
        }
        let run = self.poster_run(&notification)?;
        let commit = Commit::new();
        self.commit_change(
            hold,
            commit,
            &notification,
            NotificationChange::Read,
            by,
            run,
        )?;
        Ok(notification)
    }

    /// Resolve the notification `id` names, or `source` posted under `key`.
    pub fn resolve_notification(&self, id: NotificationId, by: Actor) -> Result<Notification> {
        let hold = self.hold();
        let notification = self.stored_notification(id)?;
        self.resolve_held(hold, notification, by)
    }

    pub fn resolve_notification_by_key(
        &self,
        source: &str,
        key: &str,
        by: Actor,
    ) -> Result<Notification> {
        let hold = self.hold();
        let notification = self
            .store
            .notification_by_key(source, key)?
            .ok_or_else(|| RuntimeError::NotFound(format!("notification `{key}` of {source}")))?;
        self.resolve_held(hold, notification, by)
    }

    /// Record the operator's answer and deliver it to a posting agent that
    /// can still receive it. A service reads the `notification.answered` event.
    pub fn answer_notification(
        &self,
        id: NotificationId,
        action: Option<String>,
        text: Option<String>,
        by: Actor,
    ) -> Result<Answered> {
        let at = now();
        let hold = self.hold();
        let mut notification = self.stored_notification(id)?;
        notification
            .answer(action, text, by.clone(), at)
            .map_err(RuntimeError::Invalid)?;
        let mut commit = Commit::new();
        let recipient = match notification.poster_agent() {
            Some(agent) => self.store.agent(agent)?,
            None => None,
        };
        let run = recipient.as_ref().map(|agent| agent.run);
        let recipient = recipient.filter(|agent| !agent.status.is_terminal());
        if let Some(agent) = &recipient {
            let kind = answer_trigger(&notification)?;
            self.push_inbox(&hold, &mut commit, agent, kind, by.clone())?;
        }
        self.commit_change(
            hold,
            commit,
            &notification,
            NotificationChange::Answered,
            by,
            run,
        )?;
        Ok(Answered {
            notification,
            delivered_to: recipient.map(|agent| agent.id),
        })
    }

    pub fn notifications(&self, filter: &NotificationFilter) -> Result<Vec<Notification>> {
        Ok(self.store.notifications(filter)?)
    }

    pub fn notification(&self, id: NotificationId) -> Result<Notification> {
        self.stored_notification(id)
    }

    fn resolve_held(
        &self,
        hold: CommitHold<'_>,
        mut notification: Notification,
        by: Actor,
    ) -> Result<Notification> {
        if !notification.resolve(now()) {
            return Ok(notification);
        }
        let run = self.poster_run(&notification)?;
        let change = NotificationChange::Resolved;
        self.commit_change(hold, Commit::new(), &notification, change, by, run)?;
        Ok(notification)
    }

    fn stored_notification(&self, id: NotificationId) -> Result<Notification> {
        self.store
            .notification(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))
    }

    /// The source a poster files under, the actor it posts as, and its run.
    fn poster_identity(&self, poster: &Poster) -> Result<(String, Actor, Option<RunId>)> {
        match poster {
            Poster::Agent(id) => {
                let agent = self
                    .store
                    .agent(*id)?
                    .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
                Ok((id.to_string(), Actor::agent(*id), Some(agent.run)))
            }
            Poster::Service(name) => {
                check_service_source(name).map_err(RuntimeError::Invalid)?;
                let actor = Actor::Client { id: name.clone() };
                Ok((name.clone(), actor, None))
            }
        }
    }

    /// The run of a posting agent still in the store, for filing its events.
    fn poster_run(&self, notification: &Notification) -> Result<Option<RunId>> {
        let Some(agent) = notification.poster_agent() else {
            return Ok(None);
        };
        Ok(self.store.agent(agent)?.map(|agent| agent.run))
    }

    fn commit_change(
        &self,
        hold: CommitHold<'_>,
        mut commit: Commit,
        notification: &Notification,
        change: NotificationChange,
        by: Actor,
        run: Option<RunId>,
    ) -> Result<()> {
        let draft = notification
            .draft(change, by, run)
            .map_err(RuntimeError::Internal)?;
        commit.push_mutation(Mutation::PutNotification(notification.clone()));
        commit.push_event(draft);
        let events = self.apply_held(hold, commit)?;
        debug_assert!(
            events
                .iter()
                .any(|e| Notification::of(&e.kind).is_some_and(|(c, _)| c == change)),
            "the change committed its event"
        );
        Ok(())
    }
}

/// The inbox item an answer reaches its posting agent as.
fn answer_trigger(notification: &Notification) -> Result<InboxKind> {
    let Some(answer) = &notification.answer else {
        return Err(RuntimeError::Internal(format!(
            "{} was answered but carries no answer",
            notification.id
        )));
    };
    let label = answer.action.as_ref().and_then(|id| {
        notification
            .actions
            .iter()
            .find(|action| &action.id == id)
            .map(|action| action.label.clone())
    });
    Ok(InboxKind::ExternalTrigger {
        source: ANSWER_TRIGGER_SOURCE.to_owned(),
        payload: json!({
            "notification": notification.id,
            "key": notification.key,
            "title": notification.title,
            "action": answer.action,
            "label": label,
            "text": answer.text,
        }),
        operator: false,
    })
}
