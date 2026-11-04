//! The disable policy (ARCHITECTURE.md §g): refuse new work at once, let
//! turns in flight finish for up to the grace (background turns stop at their
//! next model call on their own), then cancel what is left and stop.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use aether_noesis::Noesis;
use aether_noesis::settings::NoesisSettings;
use aether_runtime::Runtime;

use super::{DRAIN_POLL, NoesisService, State};

impl NoesisService {
    /// Close the gate to new work and start the grace; the returned state
    /// keeps the service until the drain ends or a re-enable calls it off.
    pub(super) fn begin_drain(
        self: &Arc<Self>,
        noesis: Noesis,
        next: &NoesisSettings,
        runtime: &Arc<Runtime>,
    ) -> State {
        debug_assert!(!next.enabled, "a drain follows a disable");
        self.gate.drain();
        let generation = self.drains.fetch_add(1, Ordering::SeqCst) + 1;
        let grace = Duration::from_secs(u64::from(next.disable_grace_seconds));
        let service = self.clone();
        let runtime = runtime.clone();
        tokio::spawn(async move { service.drain(runtime, grace, generation).await });
        State::Draining { noesis, generation }
    }

    fn current_drain(&self, generation: u64) -> bool {
        self.drains.load(Ordering::SeqCst) == generation
    }

    async fn drain(self: Arc<Self>, runtime: Arc<Runtime>, grace: Duration, generation: u64) {
        let deadline = Instant::now() + grace;
        while self.current_drain(generation)
            && !runtime.noesis_turns_running().is_empty()
            && Instant::now() < deadline
        {
            tokio::time::sleep(DRAIN_POLL.min(grace)).await;
        }
        let mut state = self.state.lock().await;
        let ours = matches!(&*state, State::Draining { generation: g, .. } if *g == generation);
        if !ours || !self.current_drain(generation) {
            return;
        }
        match runtime.cancel_noesis_turns(None).await {
            Ok(0) => {}
            Ok(cancelled) => {
                tracing::info!(cancelled, "cancelled Noesis turns after the disable grace")
            }
            Err(error) => {
                tracing::error!(%error, "cancelling Noesis turns after the disable grace failed")
            }
        }
        self.gate.disable();
        let State::Draining { noesis, .. } = std::mem::replace(&mut *state, State::Off) else {
            debug_assert!(false, "checked to be draining above");
            return;
        };
        if let Err(error) = noesis.stop() {
            tracing::error!(%error, "closing noesis.db after the disable failed");
        }
        debug_assert!(matches!(*state, State::Off) && !self.gate.is_enabled());
    }
}
