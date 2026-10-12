//! Remote administration: settings and their revision, pairings and device records. Every change
//! is committed with its audit event before anything acts on it.

use aether_core::event::{Actor, EventDraft};
use aether_protocol::remote::{
    Device, DeviceId, DeviceIdentity, DeviceRole, Pairing, PairingState, RemoteSettings,
    RemoteStatus,
};
use aether_remote::auth::{self, DeviceRecord};
use aether_remote::pairing::{Enrolment, PairingError};
use aether_runtime::RuntimeError;
use aether_store::{Commit, Mutation};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::RemoteControl;
use super::devices::{audit, row};

/// Where the settings and their revision are kept.
const SETTINGS_KEY: &str = "remote.settings";
/// Longest reason a revocation is recorded with.
const MAX_REASON_CHARS: usize = 256;

/// Why an administrative request was refused.
#[derive(Debug)]
pub enum AdminError {
    /// The request itself is wrong, a stale revision included.
    Invalid(String),
    NotFound(String),
    /// Right request, wrong state for it.
    Transition(String),
    Runtime(RuntimeError),
    Internal(String),
}

impl std::fmt::Display for AdminError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message)
            | Self::NotFound(message)
            | Self::Transition(message)
            | Self::Internal(message) => f.write_str(message),
            Self::Runtime(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for AdminError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Runtime(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RuntimeError> for AdminError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<auth::AuthError> for AdminError {
    fn from(error: auth::AuthError) -> Self {
        Self::Internal(error.to_string())
    }
}

impl From<PairingError> for AdminError {
    fn from(error: PairingError) -> Self {
        let message = error.to_string();
        match error {
            PairingError::NotFound => Self::NotFound(message),
            PairingError::NotClaimed { .. } => Self::Transition(message),
            PairingError::Disabled(_) => Self::Invalid(message),
            PairingError::Random | PairingError::Enrol(_) => Self::Internal(message),
        }
    }
}

/// The settings as stored: whole, under the revision that wrote them.
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    revision: u64,
    settings: RemoteSettings,
}

type Admin<T> = Result<T, AdminError>;

/// The audit record of a settings change.
fn configured(by: Actor, saved: &Saved) -> EventDraft {
    audit(
        by,
        "remote.configured",
        json!({
            "revision": saved.revision,
            "enabled": saved.settings.enabled,
            "pairing": saved.settings.pairing,
            "public_origins": saved.settings.public_origins,
            "push": saved.settings.push,
        }),
    )
}

impl RemoteControl {
    fn saved(&self) -> Admin<Saved> {
        let stored = self.runtime.store().meta(SETTINGS_KEY);
        let Some(raw) = stored.map_err(RuntimeError::from)? else {
            return Ok(Saved::default());
        };
        serde_json::from_str(&raw).map_err(|e| {
            AdminError::Internal(format!("the stored remote settings are corrupt: {e}"))
        })
    }

    /// Make Web Push follow `settings`. Turning it on creates the VAPID key;
    /// turning it off drops what was waiting to be sent.
    fn follow_push(&self, settings: &RemoteSettings) -> Admin<()> {
        let Some(push) = &self.push else {
            return Ok(());
        };
        push.apply_settings(settings)
            .map_err(|e| AdminError::Internal(format!("push notifications: {e}")))
    }

    /// Bring Web Push and a listener that has no settings yet to what the
    /// stored ones say. Neither failing keeps the other from being tried.
    pub(super) async fn restore(&self) -> Admin<()> {
        let _admin = self.admin.lock().await;
        let saved = self.saved()?;
        debug_assert!(saved.revision > 0 || saved.settings == RemoteSettings::default());
        let pushing = self.follow_push(&saved.settings);
        self.server
            .apply(saved.settings)
            .await
            .map_err(|e| AdminError::Internal(format!("{e:#}")))?;
        pushing
    }

    pub async fn status(&self) -> Admin<RemoteStatus> {
        let _admin = self.admin.lock().await;
        Ok(self.server.status(self.saved()?.revision)?)
    }

    /// Replace the settings if `expected_revision` is still current, then
    /// open, close or re-police the listener before answering.
    pub async fn configure(
        &self,
        mut settings: RemoteSettings,
        expected_revision: u64,
        by: Actor,
    ) -> Admin<RemoteStatus> {
        let _admin = self.admin.lock().await;
        settings.public_origins = aether_remote::http::public_origins(&settings.public_origins)
            .map_err(AdminError::Invalid)?;
        let previous = self.saved()?;
        let current = previous.revision;
        if current != expected_revision {
            return Err(AdminError::Invalid(
                aether_protocol::remote::stale_revision_message(current, expected_revision),
            ));
        }
        let revision = current.checked_add(1).ok_or_else(|| {
            AdminError::Internal("the remote settings revision overflowed".into())
        })?;
        let saved = Saved { revision, settings };
        let value = serde_json::to_string(&saved)
            .map_err(|e| AdminError::Internal(format!("encoding the remote settings: {e}")))?;
        let key = SETTINGS_KEY.to_owned();
        let commit = Commit::new()
            .mutate(Mutation::SetMeta { key, value })
            .event(configured(by, &saved));
        // The key exists before push is saved as on: a host that cannot keep
        // one refuses the change instead of promising notifications.
        if saved.settings.push {
            self.follow_push(&saved.settings)?;
        }
        if let Err(error) = self.runtime.apply(commit) {
            // Not saved, so push goes back to what the stored settings say.
            if let Err(undo) = self.follow_push(&previous.settings) {
                tracing::error!(?undo, "push did not return to the stored settings");
            }
            return Err(error.into());
        }
        self.follow_push(&saved.settings)?;
        // Saved first: a listener that cannot open is reported, not undone.
        self.server
            .apply(saved.settings)
            .await
            .map_err(|e| AdminError::Internal(format!("{e:#}")))?;
        Ok(self.server.status(revision)?)
    }

    pub async fn start_pairing(&self, role: DeviceRole, by: Actor) -> Admin<Pairing> {
        let _admin = self.admin.lock().await;
        let pairing = self.server.start_pairing(role)?;
        debug_assert!(pairing.code.is_some(), "a new pairing carries its code");
        // The code is a credential and stays out of the log.
        let started =
            json!({"pairing": pairing.id, "role": role, "expires_at": pairing.expires_at});
        let event = audit(by, "remote.pairing_started", started);
        if let Err(error) = self.runtime.apply(Commit::new().event(event)) {
            // An unrecorded pairing must not stay open for a device to claim.
            self.server.cancel_pairing(&pairing.id)?;
            return Err(error.into());
        }
        Ok(pairing)
    }

    pub fn pairing(&self, id: &str) -> Admin<Pairing> {
        Ok(self.server.get_pairing(id)?)
    }

    /// Enrol or reject the device that claimed pairing `id`.
    pub async fn confirm_pairing(&self, id: &str, accept: bool, by: Actor) -> Admin<Pairing> {
        let _admin = self.admin.lock().await;
        let before = self.server.get_pairing(id)?;
        let enrol = |claimed: Enrolment<'_>| {
            self.enrol(claimed, by.clone())
                .map_err(|error| error.to_string())
        };
        let after = self.server.confirm_pairing(id, accept, enrol)?;
        let rejected_now =
            before.state == PairingState::Claimed && after.state == PairingState::Rejected;
        if let (true, Some(candidate)) = (rejected_now, after.candidate.as_ref()) {
            let rejected = json!({"name": candidate.name, "fingerprint": candidate.fingerprint});
            let event = audit(by, "remote.pairing_rejected", rejected);
            self.runtime.apply(Commit::new().event(event))?;
        }
        Ok(after)
    }

    /// Make the claimed device durable. Runs under the pairing lock, so one
    /// claim is enrolled at most once.
    fn enrol(&self, claimed: Enrolment<'_>, by: Actor) -> Admin<DeviceIdentity> {
        let device = DeviceRecord {
            id: DeviceId::new(),
            name: auth::check_device_name(claimed.name)?,
            role: claimed.role,
            public_key: claimed.public_key.to_vec(),
            created_at: aether_core::time::now(),
            last_seen_at: None,
            revoked_at: None,
        };
        debug_assert_eq!(auth::fingerprint(&device.public_key), claimed.fingerprint);
        let enrolled = json!({
            "device": device.id,
            "name": device.name,
            "role": device.role,
            "fingerprint": claimed.fingerprint,
        });
        let commit = Commit::new()
            .mutate(Mutation::PutDevice(
                row(&device).map_err(AdminError::Internal)?,
            ))
            .event(audit(by, "remote.device_enrolled", enrolled));
        self.runtime.apply(commit)?;
        Ok(device.identity())
    }

    pub async fn cancel_pairing(&self, id: &str, by: Actor) -> Admin<Pairing> {
        let _admin = self.admin.lock().await;
        let before = self.server.get_pairing(id)?;
        let after = self.server.cancel_pairing(id)?;
        let cancelled_now =
            before.state != PairingState::Cancelled && after.state == PairingState::Cancelled;
        if cancelled_now {
            let event = audit(by, "remote.pairing_cancelled", json!({"pairing": after.id}));
            self.runtime.apply(Commit::new().event(event))?;
        }
        Ok(after)
    }

    /// Enrolled devices, oldest first.
    pub fn devices(&self, include_revoked: bool) -> Admin<Vec<DeviceRecord>> {
        Ok(self.server.devices().devices(include_revoked)?)
    }

    fn device(&self, id: DeviceId) -> Admin<DeviceRecord> {
        let found = self.server.devices().device(id)?;
        found.ok_or_else(|| AdminError::NotFound(format!("no device {id}")))
    }

    /// Revoke `id`: it never authenticates again, and its connections and
    /// push subscription end before this returns. Repeating it changes nothing.
    pub async fn revoke(
        &self,
        id: DeviceId,
        reason: Option<String>,
        by: Actor,
    ) -> Admin<DeviceRecord> {
        let _admin = self.admin.lock().await;
        let reason = reason.map(|reason| check_reason(&reason)).transpose()?;
        let mut device = self.device(id)?;
        if device.revoked_at.is_none() {
            device.revoked_at = Some(aether_core::time::now());
            let revoked = json!({"device": device.id, "name": device.name, "reason": reason});
            let commit = Commit::new()
                .mutate(Mutation::PutDevice(
                    row(&device).map_err(AdminError::Internal)?,
                ))
                .event(audit(by, "remote.device_revoked", revoked));
            self.runtime.apply(commit)?;
        }
        assert!(
            device.revoked_at.is_some(),
            "a revoked device carries its time"
        );
        self.server.sessions().close(id);
        self.forget_push(id);
        Ok(device)
    }

    /// Drop a revoked device's push subscription. The revocation is already durable and the bus
    /// repeats this, so a failure is logged and a repeated revoke retries.
    fn forget_push(&self, id: DeviceId) {
        let Some(push) = &self.push else {
            return;
        };
        if let Err(error) = push.forget_device(id) {
            tracing::error!(device = %id, %error, "a revoked device's push subscription was not removed");
        }
    }

    /// Rename a device, change its role, or both. A new role ends the
    /// device's connections, which authenticated under the old one.
    pub async fn update(
        &self,
        id: DeviceId,
        name: Option<String>,
        role: Option<DeviceRole>,
        by: Actor,
    ) -> Admin<DeviceRecord> {
        let _admin = self.admin.lock().await;
        if name.is_none() && role.is_none() {
            return Err(AdminError::Invalid(
                "give a new name, a new role, or both".into(),
            ));
        }
        let name = name
            .map(|name| auth::check_device_name(&name))
            .transpose()
            .map_err(|refused| AdminError::Invalid(refused.to_string()))?;
        let mut device = self.device(id)?;
        if device.revoked_at.is_some() {
            return Err(AdminError::Transition(format!(
                "device {id} is revoked and cannot be changed"
            )));
        }
        let role_changed = role.is_some_and(|role| role != device.role);
        let renamed = name.as_ref().is_some_and(|name| *name != device.name);
        if !role_changed && !renamed {
            return Ok(device);
        }
        device.name = name.unwrap_or(device.name);
        device.role = role.unwrap_or(device.role);
        let updated = json!({"device": device.id, "name": device.name, "role": device.role});
        let commit = Commit::new()
            .mutate(Mutation::PutDevice(
                row(&device).map_err(AdminError::Internal)?,
            ))
            .event(audit(by, "remote.device_updated", updated));
        self.runtime.apply(commit)?;
        if role_changed {
            self.server.sessions().close(id);
        }
        Ok(device)
    }

    /// A device as a caller is told of it. Only the host, which has no
    /// device identity of its own, learns fingerprints.
    pub fn view(&self, device: &DeviceRecord, caller: Option<&DeviceIdentity>) -> Admin<Device> {
        let push = match &self.push {
            None => false,
            Some(push) => push
                .store()
                .subscription(device.id)
                .map_err(|e| AdminError::Internal(e.to_string()))?
                .is_some(),
        };
        Ok(Device {
            id: device.id,
            name: device.name.clone(),
            role: device.role,
            fingerprint: caller
                .is_none()
                .then(|| auth::fingerprint(&device.public_key)),
            created_at: device.created_at,
            last_seen_at: device.last_seen_at,
            current: caller.is_some_and(|caller| caller.id == device.id),
            revoked_at: device.revoked_at,
            connections: self.server.connections().of(device.id),
            push,
        })
    }
}

/// A revocation reason fit for the event log: short, one line.
fn check_reason(reason: &str) -> Admin<String> {
    let reason = reason.trim();
    let fits = reason.chars().count() <= MAX_REASON_CHARS;
    if reason.is_empty() || !fits || reason.chars().any(char::is_control) {
        return Err(AdminError::Invalid(format!(
            "a reason is 1 to {MAX_REASON_CHARS} characters with no control characters"
        )));
    }
    Ok(reason.to_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use aether_model::config::RemoteSection;
    use aether_remote::push::testing::{
        MemoryPushStore, RecordingTransport, StaticFleet, UserAgent,
    };
    use aether_remote::push::{Notifier, PushError, PushStore, StoredSubscription};
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::push::tests::{ENDPOINT, Fixture};

    /// A subscription store whose removals fail while `stuck` is set.
    #[derive(Debug, Default)]
    struct StuckStore {
        inner: MemoryPushStore,
        stuck: AtomicBool,
    }

    impl PushStore for StuckStore {
        fn subscription(&self, device: DeviceId) -> Result<Option<StoredSubscription>, PushError> {
            self.inner.subscription(device)
        }

        fn subscriptions(&self) -> Result<Vec<StoredSubscription>, PushError> {
            self.inner.subscriptions()
        }

        fn put_subscription(&self, subscription: &StoredSubscription) -> Result<(), PushError> {
            self.inner.put_subscription(subscription)
        }

        fn remove_subscription(&self, device: DeviceId) -> Result<(), PushError> {
            if self.stuck.load(Ordering::SeqCst) {
                return Err(PushError::Store("the disk is full".into()));
            }
            self.inner.remove_subscription(device)
        }

        fn remove_exact_subscription(&self, dead: &StoredSubscription) -> Result<bool, PushError> {
            self.inner.remove_exact_subscription(dead)
        }
    }

    #[tokio::test]
    async fn a_revocation_stands_when_the_subscription_cannot_be_removed_and_a_repeat_retries() {
        let fixture = Fixture::start().await;
        let store = Arc::new(StuckStore::default());
        let path = std::path::Path::new("aether.db");
        let config =
            crate::push::configured(&RemoteSection::default(), "inst", path).expect("push config");
        let notifier = Notifier::new(
            config,
            store.clone(),
            Arc::new(StaticFleet::default()),
            Arc::new(RecordingTransport::default()),
        );
        let daemon = Arc::new(fixture.daemon);
        let remote = crate::remote::configured(&RemoteSection::default()).expect("remote config");
        let control =
            crate::remote::assemble(&daemon, remote, Some(notifier), CancellationToken::new())
                .expect("control");

        let device = DeviceRecord {
            id: DeviceId::new(),
            name: "Kim's phone".into(),
            role: DeviceRole::Observer,
            public_key: vec![4; auth::PUBLIC_KEY_BYTES],
            created_at: aether_core::time::now(),
            last_seen_at: None,
            revoked_at: None,
        };
        let put = Commit::new().mutate(Mutation::PutDevice(row(&device).expect("row")));
        control.runtime.apply(put).expect("enrolled");
        let subscription = StoredSubscription {
            device: device.id,
            subscription: UserAgent::generate().subscription(ENDPOINT),
            kinds: Vec::new(),
        };
        store.put_subscription(&subscription).expect("subscribed");

        store.stuck.store(true, Ordering::SeqCst);
        let revoked = control
            .revoke(device.id, None, Actor::user())
            .await
            .expect("the revocation is committed, so it is reported");
        assert!(revoked.revoked_at.is_some());
        assert!(store.subscription(device.id).expect("read").is_some());

        store.stuck.store(false, Ordering::SeqCst);
        let again = control
            .revoke(device.id, None, Actor::user())
            .await
            .expect("a repeat succeeds");
        assert_eq!(
            again.revoked_at, revoked.revoked_at,
            "the first time stands"
        );
        assert_eq!(store.subscription(device.id).expect("read"), None);
        daemon.runtime.shutdown().await;
    }

    #[test]
    fn a_reason_is_one_short_line() {
        assert_eq!(
            check_reason("  lost on the train ").ok().as_deref(),
            Some("lost on the train")
        );
        for bad in ["", "  ", "two\nlines", &"x".repeat(MAX_REASON_CHARS + 1)] {
            assert!(
                matches!(check_reason(bad), Err(AdminError::Invalid(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn an_admin_error_reads_as_its_message_not_as_its_debug_form() {
        let refused = AdminError::from(auth::check_device_name("a\tb").expect_err("a tab"));
        let said = refused.to_string();
        assert!(!said.contains("Internal("), "{said}");
        assert_eq!(said, auth::AuthError::BadName.to_string());
        assert_eq!(AdminError::Invalid("no".into()).to_string(), "no");
    }

    #[test]
    fn a_pairing_refusal_maps_to_the_documented_error() {
        assert!(matches!(
            PairingError::NotFound.into(),
            AdminError::NotFound(_)
        ));
        let not_claimed = PairingError::NotClaimed {
            state: "open",
            asked: "confirmed",
        };
        assert!(matches!(not_claimed.into(), AdminError::Transition(_)));
        let disabled = PairingError::Disabled("pairing is disabled");
        assert!(matches!(disabled.into(), AdminError::Invalid(_)));
    }
}
