//! The reads behind `status`, one at a time and shared by every request.
//!
//! Those reads walk every live agent, run and task, take the scheduler's lock
//! and stat the disk. On an overloaded host one took over two minutes, and each
//! client that asked meanwhile started another, so every `status` timed out.
//! Now one read runs, a request waits at most its patience for one that
//! started after it arrived, and past that it is answered with the last read,
//! marked stale, while the read in flight goes on.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_core::time::{Timestamp, now};
use aether_protocol::ErrorReply;
use parking_lot::Mutex;
use tokio::sync::watch;

/// How long a `status` or `get_tree` request waits for a fresh read before it
/// answers with the last one: dozens of healthy reads, and well under a second.
pub(crate) const PATIENCE: Duration = Duration::from_millis(800);

type Read<T> = Arc<dyn Fn() -> Result<T, ErrorReply> + Send + Sync>;

/// What one read for `status` takes from the runtime and the store.
pub(crate) struct Figures {
    pub(crate) runtime: aether_runtime::RuntimeStatus,
    pub(crate) last_seq: u64,
}

/// One finished read.
struct Finished<T> {
    started: Instant,
    result: Result<Arc<T>, ErrorReply>,
}

struct State<T> {
    /// When the read in flight started; `None` when none runs.
    running: Option<Instant>,
    /// A request arrived after the read in flight started: run another after it.
    again: bool,
    last: Option<Finished<T>>,
    /// The last read that succeeded, and when it started.
    last_good: Option<(Timestamp, Arc<T>)>,
}

/// What a request gets.
pub(crate) enum Answer<T> {
    /// From a read that started after the request arrived.
    Fresh(Arc<T>),
    /// No such read finished in time: the last good one, and when it was
    /// taken, or nothing when none finished yet.
    Stale(Option<(Timestamp, Arc<T>)>),
}

pub(crate) struct StatusReads<T> {
    state: Arc<Mutex<State<T>>>,
    /// Bumped each time a read finishes.
    finished: Arc<watch::Sender<u64>>,
}

impl<T> std::fmt::Debug for StatusReads<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.lock();
        f.debug_struct("StatusReads")
            .field("running", &state.running)
            .field("again", &state.again)
            .finish_non_exhaustive()
    }
}

impl<T> Default for StatusReads<T> {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                running: None,
                again: false,
                last: None,
                last_good: None,
            })),
            finished: Arc::new(watch::Sender::new(0)),
        }
    }
}

impl<T> StatusReads<T> {
    /// Whether a read is in flight, which its caller may have stopped waiting for.
    pub(crate) fn reading(&self) -> bool {
        self.state.lock().running.is_some()
    }
}

impl<T: Send + Sync + 'static> StatusReads<T> {
    /// The result of a `read` that started after this call, if one finishes
    /// within `patience`; otherwise the last good one, marked stale. A read
    /// that fails is this request's error, as it would be without sharing.
    pub(crate) async fn answer(
        &self,
        patience: Duration,
        read: impl Fn() -> Result<T, ErrorReply> + Send + Sync + 'static,
    ) -> Result<Answer<T>, ErrorReply> {
        let arrived = Instant::now();
        let mut finished = self.finished.subscribe();
        self.start_after(arrived, Arc::new(read));
        let deadline = tokio::time::Instant::from_std(arrived + patience);
        loop {
            if let Some(result) = self.finished_since(arrived) {
                return result.map(Answer::Fresh);
            }
            let changed = tokio::time::timeout_at(deadline, finished.changed()).await;
            let Ok(changed) = changed else {
                return Ok(Answer::Stale(self.state.lock().last_good.clone()));
            };
            assert!(changed.is_ok(), "`self` owns the sender");
        }
    }

    fn finished_since(&self, arrived: Instant) -> Option<Result<Arc<T>, ErrorReply>> {
        let state = self.state.lock();
        let last = state.last.as_ref().filter(|last| last.started >= arrived)?;
        Some(last.result.clone())
    }

    /// Make sure a read that starts at or after `arrived` runs: start one, or
    /// have the one in flight followed by another.
    fn start_after(&self, arrived: Instant, read: Read<T>) {
        let mut state = self.state.lock();
        match state.running {
            Some(started) if started >= arrived => {}
            Some(_) => state.again = true,
            None => {
                let started = Instant::now();
                assert!(started >= arrived, "the clock is monotonic");
                state.running = Some(started);
                drop(state);
                let (state, finished) = (self.state.clone(), self.finished.clone());
                tokio::task::spawn_blocking(move || read_until_current(&state, &finished, &read));
            }
        }
    }
}

/// Run `read` until no request is waiting for a newer one.
fn read_until_current<T>(state: &Mutex<State<T>>, finished: &watch::Sender<u64>, read: &Read<T>) {
    loop {
        let started = state.lock().running;
        let Some(started) = started else {
            unreachable!("a read runs only once marked running");
        };
        let at = now();
        let result = catch_unwind(AssertUnwindSafe(|| read()))
            .unwrap_or_else(|_| Err(ErrorReply::internal("the status read panicked")))
            .map(Arc::new);
        let mut state = state.lock();
        if let Ok(value) = &result {
            state.last_good = Some((at, value.clone()));
        }
        state.last = Some(Finished { started, result });
        let again = std::mem::take(&mut state.again);
        state.running = again.then(Instant::now);
        drop(state);
        finished.send_modify(|count| *count += 1);
        if !again {
            return;
        }
    }
}

#[cfg(test)]
#[path = "status_reads_tests.rs"]
mod tests;
