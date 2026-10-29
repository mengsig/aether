//! Refuses new host processes once a stopping daemon has swept them.

use std::sync::{Arc, PoisonError, RwLock};

/// Whether host processes may still start. Clones share one gate. A spawn
/// holds it until its child has exec'd, so whoever closes it sees that child.
#[derive(Debug, Clone, Default)]
pub struct SpawnGate(Arc<RwLock<bool>>);

/// The gate was closed before the spawn could start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnsClosed;

impl std::fmt::Display for SpawnsClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no process starts once the daemon is stopping")
    }
}

impl std::error::Error for SpawnsClosed {}

impl SpawnGate {
    /// Run `spawn` unless the gate is closed. `spawn` must return only once
    /// its child has exec'd or failed to.
    pub fn spawn<T>(&self, spawn: impl FnOnce() -> T) -> Result<T, SpawnsClosed> {
        // A spawn that panicked poisons the lock, but the flag stays valid.
        let closed = self.0.read().unwrap_or_else(PoisonError::into_inner);
        if *closed {
            return Err(SpawnsClosed);
        }
        Ok(spawn())
    }

    /// Refuse every later spawn, after waiting out those under way.
    pub fn close(&self) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = true;
    }
}

impl PartialEq for SpawnGate {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SpawnGate {}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_gate_refuses_and_clones_share_it() {
        let gate = SpawnGate::default();
        let clone = gate.clone();
        assert_eq!(gate.spawn(|| 7), Ok(7));
        clone.close();
        let mut ran = false;
        assert_eq!(gate.spawn(|| ran = true), Err(SpawnsClosed));
        assert!(!ran, "a closed gate never runs the spawn");
        assert_eq!(gate, clone);
        assert_ne!(gate, SpawnGate::default());
    }

    #[test]
    fn closing_waits_for_a_spawn_under_way() {
        let gate = SpawnGate::default();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel::<()>();
        let spawner = {
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.spawn(|| {
                    started_tx.send(()).unwrap();
                    finish_rx.recv().unwrap();
                })
            })
        };
        started_rx.recv().unwrap();
        let (closed_tx, closed_rx) = std::sync::mpsc::channel();
        let closer = {
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.close();
                closed_tx.send(()).unwrap();
            })
        };
        assert!(
            closed_rx
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "close returned while a spawn was under way"
        );
        finish_tx.send(()).unwrap();
        assert_eq!(spawner.join().unwrap(), Ok(()));
        closed_rx
            .recv()
            .expect("close returns once the spawn is done");
        closer.join().unwrap();
        assert_eq!(gate.spawn(|| ()), Err(SpawnsClosed));
    }
}
