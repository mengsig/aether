//! How a connection runs the commands it reads.
//!
//! One client sends everything on one connection: the TUI's `status` and
//! `get_tree` polls used to wait behind any slow request it sent before them
//! (#333). Reads now run at once, side by side; every other command still
//! runs one at a time in the order it arrived, and the reply carries the
//! request's id.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_core::event::Actor;
use aether_protocol::{Access, Command, ErrorReply, Reply, ServerFrame};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use super::{dispatch_version, starting};
use crate::Daemon;
use crate::authz::AuthContext;

/// Reads one connection runs at once; the next waits for one to finish.
const MAX_CONCURRENT_READS: usize = 16;

/// Reads waiting or running on one connection before it stops reading frames.
pub(super) const MAX_QUEUED_READS: usize = 256;

/// Commands waiting for the ordered lane before the connection stops reading.
const ORDERED_BACKLOG: usize = 64;

/// How long `shutdown` or a closing connection waits for the replies of the
/// commands before it; past it unsent ones are given up, as no client reads them.
pub(super) const DRAIN_PATIENCE: Duration = Duration::from_secs(30);

/// A request slower than this is logged, with where its time went.
pub(super) const SLOW_REQUEST: Duration = Duration::from_secs(2);

/// One command read off the connection, waiting to run.
struct Request {
    id: u64,
    command: Command,
    by: Actor,
    auth: AuthContext,
    arrived: Instant,
    _in_flight: InFlight,
}

/// Counts a command from when it is read until its reply is sent, panics included.
struct InFlight(Arc<watch::Sender<usize>>);

impl InFlight {
    fn start(count: &Arc<watch::Sender<usize>>) -> Self {
        count.send_modify(|started| *started += 1);
        Self(count.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.send_modify(|started| {
            assert!(
                *started > 0,
                "every finished command was counted when it started"
            );
            *started -= 1;
        });
    }
}

/// Where every command of one connection runs and answers.
#[derive(Clone)]
struct Context {
    daemon: Arc<Daemon>,
    out: mpsc::Sender<ServerFrame>,
    /// Cancelled when the connection closes.
    cancel: CancellationToken,
    /// Cancelled when the connection closes or its unsent replies are given up.
    replies: CancellationToken,
}

pub(super) struct Lanes {
    daemon: Arc<Daemon>,
    out: mpsc::Sender<ServerFrame>,
    cancel: CancellationToken,
    replies: CancellationToken,
    reads: JoinSet<()>,
    read_slots: Arc<Semaphore>,
    /// Commands read and not yet answered.
    in_flight: Arc<watch::Sender<usize>>,
    /// Started with the first command that is not a read; closed by a drain.
    ordered_queue: Option<mpsc::Sender<Request>>,
    ordered_lane: Option<JoinHandle<()>>,
}

/// Whether `command` only reads, so it may run beside the connection's others.
fn is_read(command: &Command) -> bool {
    !matches!(
        command,
        Command::Hello(_) | Command::Subscribe { .. } | Command::Unsubscribe { .. }
    ) && command.access() == Access::Observe
}

impl Lanes {
    pub(super) fn new(
        daemon: Arc<Daemon>,
        out: mpsc::Sender<ServerFrame>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            daemon,
            out,
            replies: cancel.child_token(),
            cancel,
            reads: JoinSet::new(),
            read_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_READS)),
            in_flight: Arc::new(watch::Sender::new(0)),
            ordered_queue: None,
            ordered_lane: None,
        }
    }

    /// Start `command`, answered on its own once it finishes. Waits only when
    /// the connection's read queue or ordered lane's backlog is full.
    pub(super) async fn run(&mut self, id: u64, command: Command, by: &Actor, auth: &AuthContext) {
        let request = Request {
            id,
            command,
            by: by.clone(),
            auth: auth.clone(),
            arrived: Instant::now(),
            _in_flight: InFlight::start(&self.in_flight),
        };
        let context = Context {
            daemon: self.daemon.clone(),
            out: self.out.clone(),
            cancel: self.cancel.clone(),
            replies: self.replies.clone(),
        };
        self.reap_reads();
        if is_read(&request.command) {
            self.read(context, request).await;
        } else {
            self.order(context, request).await;
        }
    }

    async fn read(&mut self, context: Context, request: Request) {
        while self.reads.len() >= MAX_QUEUED_READS {
            log_panic(self.reads.join_next().await, "a read");
        }
        let slots = self.read_slots.clone();
        self.reads.spawn(async move {
            // A read still waiting when its connection closes has no one to answer.
            let slot = tokio::select! {
                slot = slots.acquire_owned() => slot,
                () = context.replies.cancelled() => return,
            };
            let Ok(_slot) = slot else {
                unreachable!("the read semaphore is never closed");
            };
            answer(&context, request, "read").await;
        });
        assert!(self.reads.len() <= MAX_QUEUED_READS);
    }

    async fn order(&mut self, context: Context, request: Request) {
        let queue = match &self.ordered_queue {
            Some(queue) => queue.clone(),
            None => {
                assert!(
                    self.ordered_lane.is_none(),
                    "no command is read after a drain gave up"
                );
                let (queue, queued) = mpsc::channel::<Request>(ORDERED_BACKLOG);
                self.ordered_lane = Some(tokio::spawn(run_in_order(context, queued)));
                self.ordered_queue = Some(queue.clone());
                queue
            }
        };
        let accepted = queue.send(request).await;
        assert!(
            accepted.is_ok(),
            "the ordered lane runs until its queue closes"
        );
    }

    /// Whether a command read on this connection has not answered yet.
    pub(super) fn busy(&self) -> bool {
        *self.in_flight.borrow() > 0
    }

    /// Wait until every command read so far has answered.
    pub(super) async fn until_idle(&self) {
        let mut count = self.in_flight.subscribe();
        let idle = count.wait_for(|started| *started == 0).await;
        assert!(idle.is_ok(), "the lanes own the count's sender");
    }

    /// Forget reads that have answered, so a long connection holds no list of them.
    fn reap_reads(&mut self) {
        while let Some(finished) = self.reads.try_join_next() {
            log_panic(Some(finished), "a read");
        }
    }

    /// Wait for every command already read, as `shutdown` and a closing
    /// connection must: an accepted mutation finishes first, however long it
    /// runs. Past `patience` the replies still unsent are given up; whether
    /// everything answered within it.
    pub(super) async fn finish(&mut self, patience: Duration) -> bool {
        let answered = tokio::time::timeout(patience, self.drain()).await.is_ok();
        if !answered {
            self.replies.cancel();
            self.drain().await;
        }
        assert!(!self.busy(), "every finished command has run");
        answered
    }

    /// Cancel-safe, so [`Self::finish`] can resume it after its patience.
    async fn drain(&mut self) {
        self.ordered_queue = None;
        if let Some(lane) = self.ordered_lane.as_mut() {
            let ran = lane.await;
            self.ordered_lane = None;
            log_panic(Some(ran), "a command");
        }
        while let Some(finished) = self.reads.join_next().await {
            log_panic(Some(finished), "a read");
        }
        assert!(!self.busy(), "every drained command has answered");
    }
}

/// The ordered lane: each command in turn, until the queue closes. Every
/// command read is accepted, so it runs even after its connection closed.
async fn run_in_order(context: Context, mut queued: mpsc::Receiver<Request>) {
    while let Some(request) = queued.recv().await {
        answer(&context, request, "ordered").await;
    }
}

fn log_panic(finished: Option<Result<(), tokio::task::JoinError>>, what: &str) {
    if let Some(Err(error)) = finished {
        tracing::error!(%error, "{what} on this connection panicked");
    }
}

/// Run `request` and send its reply, logging it if it was slow.
async fn answer(context: &Context, request: Request, lane: &'static str) {
    let started = Instant::now();
    let name = request.command.name();
    let queued = started.duration_since(request.arrived);
    let running = run_command(context, request.command, request.by, &request.auth);
    tokio::pin!(running);
    let result = match tokio::time::timeout(SLOW_REQUEST, &mut running).await {
        Ok(result) => result,
        Err(_) => {
            // A request that never finishes must still be in the log.
            tracing::warn!(
                command = %name,
                lane,
                queued_ms = queued.as_millis() as u64,
                ran_ms = SLOW_REQUEST.as_millis() as u64,
                "slow request still running"
            );
            running.await
        }
    };
    let ran = started.elapsed();
    if queued + ran >= SLOW_REQUEST {
        tracing::warn!(
            command = %name,
            lane,
            queued_ms = queued.as_millis() as u64,
            ran_ms = ran.as_millis() as u64,
            failed = result.is_err(),
            "slow request: queued_ms waited behind this connection's earlier commands, \
             ran_ms ran the command (startup probes, store and runtime waits included)"
        );
    }
    let reply = ServerFrame::Reply {
        id: request.id,
        result,
    };
    // A reply with room goes out even once replies are given up; a closed
    // connection only loses it, as the command has run.
    tokio::select! {
        biased;
        _ = context.out.send(reply) => {}
        () = context.replies.cancelled() => {}
    }
}

async fn run_command(
    context: &Context,
    command: Command,
    by: Actor,
    auth: &AuthContext,
) -> Result<Reply, ErrorReply> {
    // Giving up replies must not stop an accepted command from running.
    starting::await_startup_probes(&context.daemon, &command, &context.cancel).await?;
    dispatch_version(&context.daemon, command, by, auth).await
}

#[cfg(test)]
mod tests {
    use super::is_read;
    use aether_core::id::AgentId;
    use aether_protocol::Command;

    #[test]
    fn reads_run_aside_and_everything_else_in_order() {
        assert!(is_read(&Command::Status));
        assert!(is_read(&Command::GetTree {
            run: None,
            include_archived: false,
        }));
        assert!(is_read(&Command::GetTranscript {
            agent: AgentId::new(),
            after: 0,
        }));
        assert!(!is_read(&Command::Cancel {
            agent: AgentId::new(),
            reason: None,
            idempotency_key: None,
        }));
        assert!(!is_read(&Command::Unsubscribe { id: 1 }));
        assert!(!is_read(&Command::StorageGc {
            dry_run: true,
            prune_build_output: false,
        }));
    }
}
