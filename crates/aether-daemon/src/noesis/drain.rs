//! The disable policy (ARCHITECTURE.md §g): refuse new work at once, let
//! turns in flight finish for up to the grace (background turns stop at their
//! next model call on their own), then cancel what is left and stop.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use aether_noesis::settings::NoesisSettings;
use aether_runtime::Runtime;

use super::{DRAIN_POLL, Live, NoesisService, State};

impl NoesisService {
    /// Close the gate to new work and start the grace; the returned state
    /// keeps the service until the drain ends or a re-enable calls it off.
    pub(super) fn begin_drain(
        self: &Arc<Self>,
        live: Live,
        next: &NoesisSettings,
        runtime: &Arc<Runtime>,
    ) -> State {
        debug_assert!(!next.enabled, "a drain follows a disable");
        self.gate.drain();
        // A pass that fetched the budget before the drain admits nothing more.
        live.budget.close();
        let generation = self.drains.fetch_add(1, Ordering::SeqCst) + 1;
        let grace = Duration::from_secs(u64::from(next.disable_grace_seconds));
        let service = self.clone();
        let runtime = runtime.clone();
        tokio::spawn(async move { service.drain(runtime, grace, generation).await });
        State::Draining { live, generation }
    }

    fn current_drain(&self, generation: u64) -> bool {
        self.drains.load(Ordering::SeqCst) == generation
    }

    async fn drain(self: Arc<Self>, runtime: Arc<Runtime>, grace: Duration, generation: u64) {
        let deadline = Instant::now() + grace;
        while self.current_drain(generation)
            && self.turns_left(&runtime).await
            && Instant::now() < deadline
        {
            tokio::time::sleep(DRAIN_POLL.min(grace)).await;
        }
        // Teaching turns never hold the state lock, so it is free here; a
        // teaching model call still in flight ends when the service stops.
        let mut state = self.state.lock().await;
        let ours = matches!(&*state, State::Draining { generation: g, .. } if *g == generation);
        if !ours || !self.current_drain(generation) {
            return;
        }
        // Retired and disabled first: a turn admitted earlier and registered
        // after this is refused by the scheduler, one registered before is
        // cancelled. A later re-enable cannot revive the retired admissions.
        if let State::Draining { live, .. } = &*state {
            live.budget.retire();
        }
        self.gate.disable();
        match runtime.cancel_noesis_turns(None).await {
            Ok(0) => {}
            Ok(cancelled) => {
                tracing::info!(cancelled, "cancelled Noesis turns after the disable grace")
            }
            Err(error) => {
                tracing::error!(%error, "cancelling Noesis turns after the disable grace failed")
            }
        }
        let State::Draining { live, .. } = std::mem::replace(&mut *state, State::Off) else {
            debug_assert!(false, "checked to be draining above");
            return;
        };
        if let Err(error) = live.stop().await {
            tracing::error!(%error, "closing noesis.db after the disable failed");
        }
        debug_assert!(matches!(*state, State::Off) && !self.gate.is_enabled());
    }

    /// Whether a Noesis turn still runs, or an admission is not yet settled.
    async fn turns_left(&self, runtime: &Runtime) -> bool {
        if !runtime.noesis_turns_running().is_empty() {
            return true;
        }
        // Teaching calls run outside the state lock; they count themselves.
        if self.teaching_calls.load(Ordering::SeqCst) > 0 {
            return true;
        }
        match &*self.state.lock().await {
            State::Draining { live, .. } => live.budget.outstanding() > 0,
            State::Off | State::Running(_) => false,
        }
    }
}
