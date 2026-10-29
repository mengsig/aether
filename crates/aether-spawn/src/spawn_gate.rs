//! Refuses new host processes once a sweep for them has begun.

use std::sync::{Arc, PoisonError, RwLock, RwLockWriteGuard};

/// Whether host processes may still start. Clones share one gate. A spawn
/// holds it until its child has exec'd, so whoever closes or fences it after
/// that spawn began sees that child.
#[derive(Debug, Clone, Default)]
pub struct SpawnGate(Arc<RwLock<bool>>);

/// The gate was closed, or the caller had stopped, before the spawn began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnRefused;

impl std::fmt::Display for SpawnRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no process starts once its turn or the daemon has stopped")
    }
}

impl std::error::Error for SpawnRefused {}

impl SpawnGate {
    /// Run `spawn` unless the gate is closed or `stopped` returns true; both
    /// are read under the gate. `spawn` must return only once its child has
    /// exec'd or failed to.
    pub fn spawn<T>(
        &self,
        stopped: impl FnOnce() -> bool,
        spawn: impl FnOnce() -> T,
    ) -> Result<T, SpawnRefused> {
        // A spawn that panicked poisons the lock, but the flag stays valid.
        let closed = self.0.read().unwrap_or_else(PoisonError::into_inner);
        if *closed || stopped() {
            return Err(SpawnRefused);
        }
        Ok(spawn())
    }

    /// Wait out the spawns under way. A spawn that begins later sees any
    /// stop its `stopped` reports from before this call.
    pub fn fence(&self) {
        drop(self.0.write().unwrap_or_else(PoisonError::into_inner));
    }

    /// Refuse every later spawn, after waiting out those under way.
    pub fn close(&self) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = true;
    }

    /// Wait out the spawns under way, then hold every later one off until the
    /// guard drops, so what a sweep decides and signals meanwhile stays true.
    pub fn hold(&self) -> SpawnHold<'_> {
        SpawnHold(self.0.write().unwrap_or_else(PoisonError::into_inner))
    }

    /// Run `change` with no hold in place, even once the gate is closed: a
    /// change a sweep's decision rests on lands before or after it, not inside.
    pub fn outside_holds<T>(&self, change: impl FnOnce() -> T) -> T {
        let _open = self.0.read().unwrap_or_else(PoisonError::into_inner);
        change()
    }
}

/// Spawns wait while this lives; see [`SpawnGate::hold`].
#[must_use = "spawns are held off only while the guard lives"]
#[derive(Debug)]
pub struct SpawnHold<'a>(#[allow(dead_code)] RwLockWriteGuard<'a, bool>);

impl PartialEq for SpawnGate {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SpawnGate {}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_closed_gate_refuses_and_clones_share_it() {
        let gate = SpawnGate::default();
        let clone = gate.clone();
        assert_eq!(gate.spawn(|| false, || 7), Ok(7));
        clone.close();
        let mut ran = false;
        assert_eq!(gate.spawn(|| false, || ran = true), Err(SpawnRefused));
        assert!(!ran, "a closed gate never runs the spawn");
        assert_eq!(gate, clone);
        assert_ne!(gate, SpawnGate::default());
    }

    #[test]
    fn a_stopped_caller_is_refused_while_others_still_spawn() {
        let gate = SpawnGate::default();
        let mut ran = false;
        assert_eq!(gate.spawn(|| true, || ran = true), Err(SpawnRefused));
        assert!(!ran, "a stopped caller's spawn ran");
        gate.fence();
        assert_eq!(gate.spawn(|| false, || 1), Ok(1), "a fence closes nothing");
    }

    /// Holds a spawn inside the gate on a thread of its own until released.
    struct Paused {
        thread: std::thread::JoinHandle<Result<(), SpawnRefused>>,
        finish: mpsc::Sender<()>,
    }

    fn pause_a_spawn(gate: &SpawnGate) -> Paused {
        let (started_tx, started_rx) = mpsc::channel();
        let (finish, finish_rx) = mpsc::channel::<()>();
        let gate = gate.clone();
        let thread = std::thread::spawn(move || {
            gate.spawn(
                || false,
                || {
                    started_tx.send(()).unwrap();
                    finish_rx.recv().unwrap();
                },
            )
        });
        started_rx.recv().unwrap();
        Paused { thread, finish }
    }

    /// Runs `wait` on a thread and reports whether it returned within a while.
    fn returns_while_paused(wait: impl FnOnce() + Send + 'static) -> mpsc::Receiver<()> {
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            wait();
            done_tx.send(()).unwrap();
        });
        assert!(
            done_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "returned while a spawn was under way"
        );
        done_rx
    }

    #[test]
    fn a_hold_waits_for_a_spawn_under_way_and_holds_off_the_next() {
        let gate = SpawnGate::default();
        let paused = pause_a_spawn(&gate);
        let holder = gate.clone();
        let (release, released) = mpsc::channel::<()>();
        let held = returns_while_paused(move || {
            let _hold = holder.hold();
            released.recv().unwrap();
        });
        paused.finish.send(()).unwrap();
        assert_eq!(paused.thread.join().unwrap(), Ok(()));
        let spawner = gate.clone();
        let next = returns_while_paused(move || assert_eq!(spawner.spawn(|| false, || ()), Ok(())));
        release.send(()).unwrap();
        held.recv().expect("the hold ends once released");
        next.recv().expect("the next spawn runs once the hold ends");
    }

    #[test]
    fn a_change_outside_holds_waits_for_a_hold_and_runs_once_closed() {
        let gate = SpawnGate::default();
        let holder = gate.clone();
        let (held_tx, held_rx) = mpsc::channel::<()>();
        let (release, released) = mpsc::channel::<()>();
        let holding = std::thread::spawn(move || {
            let _hold = holder.hold();
            held_tx.send(()).unwrap();
            released.recv().unwrap();
        });
        held_rx.recv().unwrap();
        let changer = gate.clone();
        let changed = returns_while_paused(move || assert_eq!(changer.outside_holds(|| 7), 7));
        release.send(()).unwrap();
        holding.join().unwrap();
        changed.recv().expect("the change runs once the hold ends");
        gate.close();
        assert!(
            gate.outside_holds(|| true),
            "a closed gate still lets a change through"
        );
    }

    #[test]
    fn closing_waits_for_a_spawn_under_way() {
        let gate = SpawnGate::default();
        let paused = pause_a_spawn(&gate);
        let closer = gate.clone();
        let closed = returns_while_paused(move || closer.close());
        paused.finish.send(()).unwrap();
        assert_eq!(paused.thread.join().unwrap(), Ok(()));
        closed.recv().expect("close returns once the spawn is done");
        assert_eq!(gate.spawn(|| false, || ()), Err(SpawnRefused));
    }

    /// A turn stopped while one of its spawns is under way: the fence returns
    /// only after that spawn, and every later spawn of the turn is refused.
    #[test]
    fn a_fence_after_a_stop_orders_every_spawn_of_the_stopped_caller() {
        let gate = SpawnGate::default();
        let stopped = Arc::new(AtomicBool::new(false));
        let paused = pause_a_spawn(&gate);
        stopped.store(true, Ordering::SeqCst);
        let fencer = gate.clone();
        let fenced = returns_while_paused(move || fencer.fence());
        paused.finish.send(()).unwrap();
        assert_eq!(paused.thread.join().unwrap(), Ok(()));
        fenced
            .recv()
            .expect("the fence returns once the spawn is done");
        let late = stopped.clone();
        let mut ran = false;
        let refused = gate.spawn(move || late.load(Ordering::SeqCst), || ran = true);
        assert_eq!(refused, Err(SpawnRefused));
        assert!(!ran, "a spawn of the stopped caller ran after the fence");
    }
}
