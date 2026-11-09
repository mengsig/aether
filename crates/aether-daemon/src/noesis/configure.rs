//! `noesis_configure`: validate, write `[noesis]`, then start, restart or
//! drain the service to match, and record the outcome under its key.

use std::sync::Arc;

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::idempotency::Submission;
use aether_noesis::settings::NoesisSettings;
use aether_protocol::ErrorReply;
use aether_protocol::noesis::{NoesisSettingsPatch, NoesisStatus};
use aether_runtime::Runtime;
use aether_runtime::noesis_guard::record_noesis_receipt;
use aether_store::Commit;

use super::{Live, NoesisService, State, settings_of, view_of};

impl NoesisService {
    /// The config write always comes first; a retry under the same key
    /// replays the first outcome instead of configuring again.
    pub async fn configure(
        self: &Arc<Self>,
        runtime: &Arc<Runtime>,
        patch: &NoesisSettingsPatch,
        by: Actor,
        submission: Option<&Submission>,
    ) -> Result<NoesisStatus, ErrorReply> {
        aether_protocol::noesis::validate_patch(patch).map_err(ErrorReply::invalid)?;
        // Held to the end: configures are serialized, receipt check included.
        let mut state = self.state.lock().await;
        if let Some(receipt) = runtime
            .noesis_receipt(submission)
            .map_err(crate::connection::map_err)?
        {
            return serde_json::from_value(receipt)
                .map_err(|e| ErrorReply::internal(format!("decode the configure receipt: {e}")));
        }
        let (current, next) = self.patched_settings(patch)?;
        let deferred = self.save_settings(&state, &next)?;
        let generation = self
            .settings_generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        if deferred {
            self.settings_pending
                .store(true, std::sync::atomic::Ordering::SeqCst);
            self.defer_settings(runtime, generation);
        }
        let transition = if deferred {
            Ok(())
        } else {
            self.transition(&mut state, &next, runtime).await
        };
        let mut commit = Commit::new();
        if current.enabled || next.enabled {
            commit.push_event(self.announcement(&next, by)?);
        }
        if let Err(error) = transition {
            self.settings_applied();
            runtime.apply(commit).map_err(crate::connection::map_err)?;
            return Err(error);
        }
        if !deferred {
            self.settings_applied();
        }
        let status = self.status(runtime).await?;
        record_noesis_receipt(&mut commit, submission, &status)
            .map_err(crate::connection::map_err)?;
        if !commit.is_empty() {
            runtime.apply(commit).map_err(crate::connection::map_err)?;
        }
        drop(state);
        Ok(status)
    }

    fn save_settings(&self, state: &State, next: &NoesisSettings) -> Result<bool, ErrorReply> {
        debug_assert!(next.validate().is_ok());
        let table = next
            .to_table()
            .map_err(|error| ErrorReply::invalid(error.to_string()))?;
        let deferred = match self.prepare_swap(state, next) {
            Ok(()) => false,
            Err(error) if error.reason.as_deref() == Some("conflict") => true,
            Err(error) => return Err(error),
        };
        debug_assert!(!deferred || next.enabled);
        if let Err(error) = self.config.write_noesis(&table) {
            if let State::Running(live) = state {
                live.budget.reopen();
            }
            return Err(ErrorReply::internal(error));
        }
        Ok(deferred)
    }

    fn patched_settings(
        &self,
        patch: &NoesisSettingsPatch,
    ) -> Result<(NoesisSettings, NoesisSettings), ErrorReply> {
        let stored = self.config.read_noesis().map_err(ErrorReply::internal)?;
        let current = NoesisSettings::from_table(&stored)
            .map_err(|error| ErrorReply::invalid(error.to_string()))?;
        let next = settings_of(&aether_protocol::noesis::apply_patch(
            &view_of(&current)?,
            patch,
        ))?;
        debug_assert!(current.validate().is_ok());
        debug_assert!(next.validate().is_ok());
        Ok((current, next))
    }

    fn defer_settings(self: &Arc<Self>, runtime: &Arc<Runtime>, generation: u64) {
        let service = self.clone();
        let runtime = runtime.clone();
        tokio::spawn(async move {
            if let Err(error) = service.apply_when_idle(&runtime, generation).await {
                let _state = service.state.lock().await;
                if service
                    .settings_generation
                    .load(std::sync::atomic::Ordering::SeqCst)
                    != generation
                {
                    return;
                }
                service.settings_applied();
                runtime.report_problem(
                    "noesis.settings".into(),
                    format!("applying saved Noesis settings failed: {}", error.message),
                );
            }
        });
    }

    pub(super) fn settings_applied(&self) {
        self.settings_pending
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.settings_ready.notify_waiters();
    }

    async fn apply_when_idle(
        self: &Arc<Self>,
        runtime: &Arc<Runtime>,
        generation: u64,
    ) -> Result<(), ErrorReply> {
        loop {
            let mut state = self.state.lock().await;
            if self
                .settings_generation
                .load(std::sync::atomic::Ordering::SeqCst)
                != generation
            {
                return Ok(());
            }
            let stored = self.config.read_noesis().map_err(ErrorReply::internal)?;
            let next = NoesisSettings::from_table(&stored)
                .map_err(|e| ErrorReply::invalid(e.to_string()))?;
            if !next.enabled || matches!(&*state, State::Running(live) if live.settings == next) {
                self.settings_applied();
                return Ok(());
            }
            match self.prepare_swap(&state, &next) {
                Ok(()) => {}
                Err(error) if error.reason.as_deref() == Some("conflict") => {
                    drop(state);
                    tokio::time::sleep(super::DRAIN_POLL).await;
                    continue;
                }
                Err(error) => return Err(error),
            }
            self.transition(&mut state, &next, runtime).await?;
            self.settings_applied();
            let event = self.announcement(&next, Actor::System)?;
            runtime
                .apply(Commit::new().event(event))
                .map_err(crate::connection::map_err)?;
            runtime.clear_problem("noesis.settings");
            return Ok(());
        }
    }

    /// Before a running service is replaced by one with other settings: close
    /// its budget, and refuse while turns it admitted have not settled, since
    /// their charges would land in a ledger nobody reads any more.
    fn prepare_swap(&self, state: &State, next: &NoesisSettings) -> Result<(), ErrorReply> {
        let live = match state {
            State::Off => return Ok(()),
            State::Running(live) | State::Draining { live, .. } => live,
        };
        if !next.enabled || live.settings == *next {
            return Ok(());
        }
        let teaching = self
            .teaching_calls
            .load(std::sync::atomic::Ordering::SeqCst)
            .saturating_sub(
                self.waiting_settings
                    .load(std::sync::atomic::Ordering::SeqCst),
            );
        let outstanding = live
            .budget
            .close()
            .max(u32::try_from(teaching).unwrap_or(u32::MAX));
        if outstanding == 0 {
            return Ok(());
        }
        if matches!(state, State::Running(_)) {
            live.budget.reopen();
        }
        Err(ErrorReply::conflict(format!(
            "{outstanding} Noesis turn(s) are in flight; change the settings once they finish"
        )))
    }

    async fn transition(
        self: &Arc<Self>,
        state: &mut State,
        next: &NoesisSettings,
        runtime: &Arc<Runtime>,
    ) -> Result<(), ErrorReply> {
        let previous = std::mem::replace(state, State::Off);
        if !next.enabled {
            *state = match previous {
                State::Running(live) => self.begin_drain(live, next, runtime),
                other => other,
            };
            return Ok(());
        }
        let running = match previous {
            State::Off => None,
            State::Running(live) | State::Draining { live, .. } => Some(live),
        };
        if let Some(live) = running {
            if live.settings == *next {
                self.reopen_gate(&live);
                *state = State::Running(live);
                return Ok(());
            }
            debug_assert_eq!(live.budget.outstanding(), 0, "checked by prepare_swap");
            live.stop().await.map_err(ErrorReply::internal)?;
        }
        match self.start(next, runtime.store()).await {
            Ok(live) => {
                *state = State::Running(live);
                self.resume_teaching(runtime);
                Ok(())
            }
            Err(error) => {
                self.gate.disable();
                Err(ErrorReply::internal(format!(
                    "Noesis did not start: {error}"
                )))
            }
        }
    }

    /// Call off a drain in progress and admit through the same budget again.
    fn reopen_gate(&self, live: &Live) {
        self.drains
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        live.budget.reopen();
        self.gate.enable(live.budget.clone());
        debug_assert!(self.gate.is_enabled());
    }

    /// `noesis.settings_changed` with the stored and effective settings.
    fn announcement(&self, next: &NoesisSettings, by: Actor) -> Result<EventDraft, ErrorReply> {
        let settings = view_of(next)?;
        let effective = view_of(&self.effective(next))?;
        Ok(EventDraft::new(
            by,
            EventKind::NoesisSettingsChanged {
                settings,
                effective,
            },
        ))
    }
}
