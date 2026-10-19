//! The notification center. Every call needs a daemon that announces
//! `notifications`; [`Client::serves_notifications`] lets a caller skip it.

use aether_core::id::{AgentId, NotificationId};
use aether_core::notification::{Notification, NotificationPost, NotificationState};
use aether_protocol::{Command, Reply};

use crate::{Client, ClientError, Result, unexpected};

impl Client {
    /// Whether the daemon serves the notification center. A client without
    /// it shows no notifications rather than an error.
    pub fn serves_notifications(&self) -> bool {
        self.welcome().is_some_and(|welcome| {
            welcome
                .features
                .iter()
                .any(|feature| feature == aether_protocol::features::NOTIFICATIONS)
        })
    }

    /// Post as the service `source`, or update the notification its key names.
    pub async fn post_notification(
        &self,
        source: impl Into<String>,
        notification: NotificationPost,
    ) -> Result<Notification> {
        let source = source.into();
        let key = notification.key.clone();
        let posted = self
            .one_notification(Command::PostNotification {
                source: source.clone(),
                notification,
            })
            .await?;
        if posted.source != source || posted.key != key {
            return Err(ClientError::Malformed(
                "daemon returned a notification under another source or key".into(),
            ));
        }
        Ok(posted)
    }

    /// Newest first; empty `states` means every state.
    pub async fn notifications(
        &self,
        states: Vec<NotificationState>,
        limit: Option<usize>,
    ) -> Result<Vec<Notification>> {
        let command = Command::ListNotifications {
            states: states.clone(),
            source: None,
            limit,
        };
        match self.request(command).await? {
            Reply::Notifications(listed) => {
                if !listed
                    .iter()
                    .all(|n| states.is_empty() || states.contains(&n.state))
                {
                    return Err(ClientError::Malformed(
                        "daemon listed a notification outside the asked states".into(),
                    ));
                }
                Ok(listed)
            }
            other => Err(unexpected("notifications", &other)),
        }
    }

    pub async fn notification(&self, id: NotificationId) -> Result<Notification> {
        self.notification_named(id, Command::GetNotification { id })
            .await
    }

    pub async fn mark_notification_read(&self, id: NotificationId) -> Result<Notification> {
        self.notification_named(id, Command::MarkNotificationRead { id })
            .await
    }

    pub async fn resolve_notification(&self, id: NotificationId) -> Result<Notification> {
        let command = Command::ResolveNotification {
            id: Some(id),
            source: None,
            key: None,
        };
        self.notification_named(id, command).await
    }

    /// Answer with an offered action, free text, or both. Returns the
    /// notification and the posting agent the answer reached, if any.
    pub async fn answer_notification(
        &self,
        id: NotificationId,
        action: Option<String>,
        text: Option<String>,
    ) -> Result<(Notification, Option<AgentId>)> {
        let command = Command::AnswerNotification { id, action, text };
        match self.request(command).await? {
            Reply::NotificationAnswered {
                notification,
                delivered_to,
            } if notification.id == id => Ok((notification, delivered_to)),
            other => Err(unexpected("notification_answered", &other)),
        }
    }

    async fn notification_named(
        &self,
        id: NotificationId,
        command: Command,
    ) -> Result<Notification> {
        let notification = self.one_notification(command).await?;
        if notification.id != id {
            return Err(ClientError::Malformed(format!(
                "asked for {id}, the daemon answered with {}",
                notification.id
            )));
        }
        Ok(notification)
    }

    async fn one_notification(&self, command: Command) -> Result<Notification> {
        match self.request(command).await? {
            Reply::Notification(notification) => Ok(notification),
            other => Err(unexpected("notification", &other)),
        }
    }
}
