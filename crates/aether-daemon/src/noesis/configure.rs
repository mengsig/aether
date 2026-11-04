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
        let stored = self.config.read_noesis().map_err(ErrorReply::internal)?;
        let current =
            NoesisSettings::from_table(&stored).map_err(|e| ErrorReply::invalid(e.to_string()))?;
        let next = settings_of(&aether_protocol::noesis::apply_patch(
            &view_of(&current)?,
            patch,
        ))?;
        let table = next
            .to_table()
            .map_err(|e| ErrorReply::invalid(e.to_string()))?;
        self.prepare_swap(&state, &next)?;
        if let Err(error) = self.config.write_noesis(&table) {
            // Nothing changed: the running service admits again.
            if let State::Running(live) = &*state {
                live.budget.reopen();
            }
            return Err(ErrorReply::internal(error));
        }
        let transition = self.transition(&mut state, &next, runtime).await;
        let mut commit = Commit::new();
        if current.enabled || next.enabled {
            commit.push_event(self.announcement(&next, by)?);
        }
        if let Err(error) = transition {
            runtime.apply(commit).map_err(crate::connection::map_err)?;
            return Err(error);
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

    /// Before a running service is replaced by one with other settings: close
    /// its budget, and refuse while turns it admitted have not settled, since
    /// their charges would land in a ledger nobody reads any more.
    fn prepare_swap(&self, state: &State, next: &NoesisSettings) -> Result<(), ErrorReply> {
        let live = match state {
            State::Off => return Ok(()),
            State::Running(live) | State::Draining { live, .. } => live,
        };
        if !next.enabled || live.noesis.settings() == next {
            return Ok(());
        }
        let outstanding = live.budget.close();
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
            if live.noesis.settings() == next {
                self.reopen_gate(&live);
                *state = State::Running(live);
                return Ok(());
            }
            debug_assert_eq!(live.budget.outstanding(), 0, "checked by prepare_swap");
            live.stop().map_err(ErrorReply::internal)?;
        }
        match self.start(next, runtime.store()).await {
            Ok(live) => {
                *state = State::Running(live);
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
