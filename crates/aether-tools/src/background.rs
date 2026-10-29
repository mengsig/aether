//! Background shell processes: started by the shell tool, polled and stopped
//! by their own tools, and killed when the agent that started them ends.
//!
//! The registry owns every handle. One task per process owns its `Child`
//! and is the only thing that ever signals or reaps it, so a stop request
//! targets only its owned process group. Output is captured into a bounded
//! log, with bounded post-stop retention until eviction or agent settlement.

#[cfg(target_os = "linux")]
pub(crate) mod journal;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aether_core::id::{AgentId, ProcessId};
use aether_core::time::{Timestamp, now};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::capture::{Capture, pump};
use crate::exec::{Teardown, prepare_interactive};
use crate::tool::{ToolContext, ToolError};

/// How long a stopped process has to exit on SIGTERM before SIGKILL.
pub const STOP_GRACE: Duration = Duration::from_millis(2000);
/// How long the exiting process's pipes have to drain before it is reported
/// finished anyway; a grandchild can hold them open indefinitely.
const PIPE_GRACE: Duration = Duration::from_millis(1000);
/// Concurrent background processes one agent may hold. Each costs a host
/// process plus a bounded output page in memory. Stopped logs have the same cap.
pub const MAX_PER_AGENT: usize = 16;

/// What a background process has produced, and whether it is still going.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessStatus {
    pub id: ProcessId,
    pub command: String,
    pub started_at: Timestamp,
    pub running: bool,
    /// Set once the process is reaped; `None` while it runs, and `None` after
    /// it was terminated by a signal.
    pub exit_code: Option<i32>,
    /// Bytes produced but not yet read by this agent.
    pub pending_bytes: usize,
    /// Bytes discarded after the capture byte limit.
    pub dropped_bytes: usize,
    pub output_path: PathBuf,
    pub output_error: Option<String>,
    pub tty: bool,
}

/// Output captured since the caller last read.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessOutput {
    pub status: ProcessStatus,
    pub text: String,
    /// Total bytes discarded after the capture byte limit.
    pub dropped_bytes: usize,
}

#[derive(Debug, Default)]
struct ProcessState {
    consumed: u64,
    /// Set when the process is reaped; a signal death leaves it `None`.
    exit_code: Option<i32>,
    finished: bool,
}

struct Process {
    id: ProcessId,
    agent: AgentId,
    command: String,
    started_at: Timestamp,
    /// Asks the owning task to signal the group. Taken when the process is
    /// stopped; `None` afterwards.
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    /// Flips once the owning task has reaped the child.
    reaped: tokio::sync::watch::Receiver<bool>,
    state: Arc<Mutex<ProcessState>>,
    capture: Arc<Capture>,
    stdin: Option<Arc<tokio::sync::Mutex<pty_process::OwnedWritePty>>>,
}

/// Every background process this daemon started, across agents.
#[derive(Default)]
pub struct BackgroundProcesses {
    processes: Mutex<HashMap<ProcessId, Process>>,
    stopped_output: Mutex<HashMap<AgentId, VecDeque<Arc<Capture>>>>,
    #[cfg(target_os = "linux")]
    journal: Mutex<Option<journal::Journal>>,
    /// Closed once the daemon stops; see [`BackgroundProcesses::close_spawns`].
    spawns: aether_spawn::SpawnGate,
}

impl std::fmt::Debug for BackgroundProcesses {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.processes.lock().map(|p| p.len()).unwrap_or(0);
        f.debug_struct("BackgroundProcesses")
            .field("processes", &count)
            .finish()
    }
}

impl BackgroundProcesses {
    pub fn new() -> Self {
        Self::default()
    }

    /// Recover this store's host jobs before the runtime resumes agents.
    /// The caller must hold the daemon's exclusive store lock throughout use.
    pub fn recover_host_jobs(&self, store_path: &std::path::Path) -> Result<(), ToolError> {
        #[cfg(target_os = "linux")]
        {
            let processes = self.lock()?;
            if !processes.is_empty() {
                return Err(ToolError::Internal(
                    "cannot recover while background jobs are registered".into(),
                ));
            }
            let mut journal = self
                .journal
                .lock()
                .map_err(|_| ToolError::Internal("background journal poisoned".into()))?;
            let mut directory = store_path.as_os_str().to_owned();
            directory.push(".background-processes");
            *journal = Some(journal::Journal::recover(directory.into())?);
        }
        #[cfg(not(target_os = "linux"))]
        tracing::warn!(store = %store_path.display(), "host background crash recovery requires Linux");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn register_host_child(
        &self,
        job: ProcessId,
        pid: u32,
    ) -> std::io::Result<Option<journal::Registration>> {
        let journal = self
            .journal
            .lock()
            .map_err(|_| std::io::Error::other("background journal poisoned"))?;
        let Some(journal) = journal.as_ref() else {
            return Ok(None);
        };
        let (registration, durability) = journal.register(job, pid)?;
        durability.sync()?;
        Ok(Some(registration))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<ProcessId, Process>>, ToolError> {
        self.processes
            .lock()
            .map_err(|_| ToolError::Internal("background process registry poisoned".into()))
    }

    /// Start `command` under the agent's backend, in its own process group
    /// and detached from the calling tool call, and return its handle.
    pub fn start(
        &self,
        agent: AgentId,
        command: &str,
        ctx: &ToolContext,
    ) -> Result<ProcessStatus, ToolError> {
        self.start_with_terminal(agent, command, ctx, false, &CancellationToken::new())
    }

    /// [`Self::start`], on a terminal if asked. Refused once `cancel`, the
    /// calling turn's token, is cancelled.
    pub fn start_with_terminal(
        &self,
        agent: AgentId,
        command: &str,
        ctx: &ToolContext,
        terminal: bool,
        cancel: &CancellationToken,
    ) -> Result<ProcessStatus, ToolError> {
        self.reap_settled(agent)?;
        let mut processes = self.lock()?;
        let held = processes.values().filter(|p| p.agent == agent).count();
        if held >= MAX_PER_AGENT {
            return Err(ToolError::InvalidInput(format!(
                "this agent already holds {held} background processes (limit {MAX_PER_AGENT}); \
                 stop one with shell_stop first"
            )));
        }
        let prepared = prepare_interactive(command, ctx, terminal)?;
        let capture = Capture::background(ctx)?;
        let id = ProcessId::new();
        let mut cmd = prepared.command;
        cmd.kill_on_drop(true);
        #[cfg(target_os = "linux")]
        cmd.env(journal::JOB_ENV, id.to_string());
        let (mut child, pty) = self.spawn_host(cancel, || {
            if terminal {
                let (child, pty) = crate::terminal::spawn(&cmd)?;
                Ok((child, Some(pty)))
            } else {
                Ok((cmd.spawn()?, None))
            }
        })?;
        #[cfg(target_os = "linux")]
        let (registration, durability) = {
            let registered = self
                .journal
                .lock()
                .map_err(|_| std::io::Error::other("background journal poisoned"))
                .and_then(|journal| {
                    journal
                        .as_ref()
                        .map(|journal| {
                            journal.register(
                                id,
                                child.id().ok_or_else(|| {
                                    std::io::Error::other("background child has no PID")
                                })?,
                            )
                        })
                        .transpose()
                });
            match registered {
                Ok(registration) => registration.unzip(),
                Err(error) => {
                    if let Some(pid) = child.id()
                        && let Err(cleanup) =
                            signal_group(Pid::from_raw(pid as i32), Signal::SIGKILL)
                    {
                        tracing::error!(pid, %cleanup, "cannot kill unregistered background process group");
                    }
                    if let Some(teardown) = prepared.teardown {
                        teardown.run();
                    }
                    return Err(error.into());
                }
            }
        };
        let state = Arc::new(Mutex::new(ProcessState::default()));
        let mut pumps = Vec::with_capacity(2);
        let stdin = if let Some(pty) = pty {
            let (reader, writer) = pty.into_split();
            pumps.push(tokio::spawn(pump(reader, capture.clone(), true)));
            Some(Arc::new(tokio::sync::Mutex::new(writer)))
        } else {
            if let Some(out) = child.stdout.take() {
                pumps.push(tokio::spawn(pump(out, capture.clone(), false)));
            }
            if let Some(err) = child.stderr.take() {
                pumps.push(tokio::spawn(pump(err, capture.clone(), false)));
            }
            None
        };
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let (reaped_tx, reaped_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(own(
            child,
            prepared.teardown,
            stop_rx,
            reaped_tx,
            state.clone(),
            capture.clone(),
            pumps,
            id,
            #[cfg(target_os = "linux")]
            registration,
        ));
        let process = Process {
            id,
            agent,
            command: command.to_owned(),
            started_at: now(),
            stop: Some(stop_tx),
            reaped: reaped_rx,
            state,
            capture,
            stdin,
        };
        let status = self.status_of(&process)?;
        processes.insert(process.id, process);
        drop(processes);
        // An already-finished job may have unlinked its record; syncing its
        // open file is harmless, and fsync never blocks registry readers.
        #[cfg(target_os = "linux")]
        if let Some(durability) = durability
            && let Err(error) = durability.sync()
        {
            let error = format!("persisting background ownership: {error}");
            if let Some(process) = self.lock()?.remove(&id) {
                process.capture.fail(&error);
            }
            return Err(ToolError::Internal(error));
        }
        Ok(status)
    }

    /// Forget the agent's processes that have exited and whose output has
    /// been read: their tail is not lost, and nothing accumulates.
    fn reap_settled(&self, agent: AgentId) -> Result<(), ToolError> {
        let mut processes = self.lock()?;
        let settled: Vec<ProcessId> = processes
            .values()
            .filter(|p| p.agent == agent)
            .filter(|p| match p.state.lock() {
                Ok(b) => b.finished && b.consumed >= p.capture.size(),
                Err(_) => false,
            })
            .map(|p| p.id)
            .collect();
        for id in settled {
            processes.remove(&id);
        }
        Ok(())
    }

    fn status_of(&self, process: &Process) -> Result<ProcessStatus, ToolError> {
        let b = process
            .state
            .lock()
            .map_err(|_| ToolError::Internal("process state poisoned".into()))?;
        Ok(ProcessStatus {
            id: process.id,
            command: process.command.clone(),
            started_at: process.started_at,
            running: !b.finished,
            exit_code: b.exit_code,
            pending_bytes: usize::try_from(process.capture.size().saturating_sub(b.consumed))
                .unwrap_or(usize::MAX),
            dropped_bytes: process.capture.dropped(),
            output_path: process.capture.path.clone(),
            output_error: process.capture.error()?,
            tty: process.stdin.is_some(),
        })
    }

    /// Read what the process produced since the last read. A finished process
    /// stays readable until it is stopped, so its tail is never lost.
    pub fn read(&self, agent: AgentId, id: ProcessId) -> Result<ProcessOutput, ToolError> {
        let processes = self.lock()?;
        let process = Self::owned(&processes, agent, id)?;
        let mut status = self.status_of(process)?;
        let mut b = process
            .state
            .lock()
            .map_err(|_| ToolError::Internal("process state poisoned".into()))?;
        let (text, end) = process.capture.page(b.consumed, b.finished)?;
        b.consumed = end;
        status.pending_bytes =
            usize::try_from(process.capture.size().saturating_sub(end)).unwrap_or(usize::MAX);
        Ok(ProcessOutput {
            status,
            text,
            dropped_bytes: process.capture.dropped(),
        })
    }

    /// Write only to a terminal owned by this agent. Do not hold the registry lock
    /// across I/O: a blocked terminal must not prevent stop or agent cancellation.
    pub async fn write_stdin(
        &self,
        agent: AgentId,
        id: ProcessId,
        chars: &str,
        cancel: CancellationToken,
    ) -> Result<usize, ToolError> {
        let writer = {
            let processes = self.lock()?;
            let process = Self::owned(&processes, agent, id)?;
            if !self.status_of(process)?.running {
                return Err(ToolError::InvalidInput("process has exited".into()));
            }
            process.stdin.clone().ok_or_else(|| {
                ToolError::InvalidInput("process has no terminal; start bash with tty: true".into())
            })?
        };
        let mut written = 0;
        let write = async {
            let mut writer = writer.lock().await;
            while written < chars.len() {
                let n = writer.write(&chars.as_bytes()[written..]).await?;
                if n == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "terminal accepted no input",
                    ));
                }
                written += n;
            }
            Ok::<(), std::io::Error>(())
        };
        let result = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(5), write) => match result {
                Ok(result) => result.map_err(|e| e.to_string()),
                Err(_) => Err("terminal input timed out".into()),
            },
            _ = cancel.cancelled() => Err("terminal input cancelled".into()),
        };
        result.map_err(|e| {
            ToolError::Internal(format!(
                "{e}; {written} input bytes accepted; do not resend accepted bytes"
            ))
        })?;
        Ok(written)
    }

    pub fn list(&self, agent: AgentId) -> Result<Vec<ProcessStatus>, ToolError> {
        let processes = self.lock()?;
        let mut out = Vec::new();
        for process in processes.values().filter(|p| p.agent == agent) {
            out.push(self.status_of(process)?);
        }
        out.sort_by_key(|s| s.started_at);
        Ok(out)
    }

    /// Run `spawn`, which starts a host command, unless spawns are closed or
    /// `cancel` is; the gate stays held meanwhile, so a fence waits for it.
    pub(crate) fn spawn_host<T>(
        &self,
        cancel: &CancellationToken,
        spawn: impl FnOnce() -> Result<T, ToolError>,
    ) -> Result<T, ToolError> {
        self.spawns
            .spawn(|| cancel.is_cancelled(), spawn)
            .map_err(|_| ToolError::Cancelled)?
    }

    /// Wait out the host commands being started, so a sweep after a turn's
    /// cancellation sees each one that will ever start for that turn.
    pub fn fence_spawns(&self) {
        self.spawns.fence();
    }

    /// Refuse every host command from now on, once the spawns under way have
    /// finished, so a daemon stop's sweep sees every process there will be.
    pub fn close_spawns(&self) {
        self.spawns.close();
    }

    /// The gate every host command starts through, for a sweep to hold.
    pub fn spawn_gate(&self) -> aether_spawn::SpawnGate {
        self.spawns.clone()
    }

    /// Stop and forget the handle, retaining its log until eviction or agent end.
    pub async fn stop(&self, agent: AgentId, id: ProcessId) -> Result<ProcessStatus, ToolError> {
        let process = {
            let mut processes = self.lock()?;
            Self::owned(&processes, agent, id)?;
            let process = processes
                .remove(&id)
                .ok_or_else(|| ToolError::InvalidInput(format!("no process {id}")))?;
            let mut stopped = self
                .stopped_output
                .lock()
                .map_err(|_| ToolError::Internal("stopped output registry poisoned".into()))?;
            let logs = stopped.entry(agent).or_default();
            if logs.len() == MAX_PER_AGENT {
                logs.pop_front();
            }
            logs.push_back(process.capture.clone());
            process
        };
        let status = self.status_of(&process)?;
        end(process).await?;
        Ok(status)
    }

    /// Stop everything an agent started. Called when the agent ends, so a
    /// cancelled agent never leaves a process behind. Returns how many.
    pub async fn stop_agent(&self, agent: AgentId) -> usize {
        self.stop_agents(&[agent]).await
    }

    /// Take what `agents` hold, and their stopped logs, out of the registry
    /// now, before the returned future stops it: a process an agent woken
    /// again starts later is not among them. The future returns how many.
    pub fn stop_agents(&self, agents: &[AgentId]) -> impl Future<Output = usize> + Send + 'static {
        let doomed = self.take_matching(|p| agents.contains(&p.agent));
        assert!(
            doomed.iter().all(|p| agents.contains(&p.agent)),
            "only the ending agents' processes are taken"
        );
        match self.stopped_output.lock() {
            Ok(mut stopped) => {
                for agent in agents {
                    stopped.remove(agent);
                }
            }
            Err(error) => tracing::error!(%error, "cannot remove stopped shell output"),
        }
        stop_taken(doomed)
    }

    /// Give `to` every process `from` holds, as a child that completed hands
    /// what it started to its nearest live ancestor. Returns those still running.
    pub fn hand_up(&self, from: AgentId, to: AgentId) -> Result<Vec<ProcessStatus>, ToolError> {
        assert_ne!(from, to, "an agent hands nothing to itself");
        let mut handed = Vec::new();
        {
            let mut processes = self.lock()?;
            for process in processes.values_mut().filter(|p| p.agent == from) {
                process.agent = to;
                handed.push(self.status_of(process)?);
            }
        }
        match self.stopped_output.lock() {
            Ok(mut stopped) => {
                stopped.remove(&from);
            }
            Err(error) => tracing::error!(%error, "cannot remove stopped shell output"),
        }
        handed.retain(|status| status.running);
        handed.sort_by_key(|status| status.started_at);
        Ok(handed)
    }

    /// Stop every process this daemon started, whatever agent owns it. The
    /// daemon must not exit leaving processes nothing can reach.
    pub async fn stop_all(&self) -> usize {
        let count = stop_taken(self.take_matching(|_| true)).await;
        match self.stopped_output.lock() {
            Ok(mut stopped) => stopped.clear(),
            Err(error) => tracing::error!(%error, "cannot remove stopped shell output"),
        }
        count
    }

    fn take_matching(&self, wanted: impl Fn(&Process) -> bool) -> Vec<Process> {
        match self.processes.lock() {
            Ok(mut processes) => {
                let ids: Vec<ProcessId> = processes
                    .values()
                    .filter(|p| wanted(p))
                    .map(|p| p.id)
                    .collect();
                ids.iter().filter_map(|id| processes.remove(id)).collect()
            }
            Err(_) => {
                tracing::error!("background registry poisoned; cannot stop processes");
                Vec::new()
            }
        }
    }

    /// The process's own exit signal, so a test can await the reap instead of
    /// polling a status whose deadline a loaded host can outrun.
    #[cfg(test)]
    pub(crate) fn reaped(
        &self,
        agent: AgentId,
        id: ProcessId,
    ) -> Result<tokio::sync::watch::Receiver<bool>, ToolError> {
        let processes = self.lock()?;
        Ok(Self::owned(&processes, agent, id)?.reaped.clone())
    }

    /// An agent may only touch processes it started.
    fn owned(
        processes: &HashMap<ProcessId, Process>,
        agent: AgentId,
        id: ProcessId,
    ) -> Result<&Process, ToolError> {
        match processes.get(&id) {
            Some(p) if p.agent == agent => Ok(p),
            // Same message either way: an agent must not learn that another
            // agent's process exists.
            _ => Err(ToolError::InvalidInput(format!("no process {id}"))),
        }
    }
}

/// Hand the process to its owning task and wait for it to be reaped. The
/// task holds the `Child`, so the pid is still ours while it signals.
async fn end(mut process: Process) -> Result<(), ToolError> {
    if let Some(stop) = process.stop.take() {
        // A closed receiver means the task already finished on its own.
        let _ = stop.send(());
    }
    tokio::time::timeout(
        STOP_GRACE + PIPE_GRACE + Duration::from_secs(5),
        process.reaped.wait_for(|done| *done),
    )
    .await
    .map_err(|_| ToolError::Internal("background process cleanup timed out".into()))?
    .map_err(|error| {
        ToolError::Internal(format!(
            "background process owner ended without cleanup: {error}"
        ))
    })?;
    Ok(())
}

/// Stop processes already taken out of the registry. Returns how many.
async fn stop_taken(mut doomed: Vec<Process>) -> usize {
    let count = doomed.len();
    for process in &mut doomed {
        if let Some(stop) = process.stop.take() {
            let _ = stop.send(());
        }
    }
    for process in doomed {
        if let Err(error) = end(process).await {
            tracing::error!(%error, "cannot finish stopping background process");
        }
    }
    count
}

/// Own the child and clean its group before publishing completion.
#[allow(clippy::too_many_arguments)]
async fn own(
    mut child: tokio::process::Child,
    teardown: Option<Teardown>,
    stop: tokio::sync::oneshot::Receiver<()>,
    reaped: tokio::sync::watch::Sender<bool>,
    state: Arc<Mutex<ProcessState>>,
    capture: Arc<Capture>,
    mut pumps: Vec<tokio::task::JoinHandle<()>>,
    job: ProcessId,
    #[cfg(target_os = "linux")] registration: Option<journal::Registration>,
) {
    let pgid = child.id().map(|p| Pid::from_raw(p as i32));
    let exited = tokio::select! {
        exit = leader_exited(&mut child) => Some(exit),
        _ = stop => None,
        _ = capture.failed.cancelled() => None,
    };
    let status = match exited {
        Some(exit) => {
            let cleanup = match pgid {
                Some(pgid) => stop_exited_group(pgid).await,
                None => Ok(()),
            };
            // Reaped only now: until then its zombie kept the group id from reuse.
            let status = exit.and(child.wait().await);
            status.and_then(|status| cleanup.map(|_| status))
        }
        None => signal_until_gone(&mut child, pgid, teardown.as_ref()).await,
    };
    #[cfg(target_os = "linux")]
    // What the job detached gets the same SIGTERM, then SIGKILL, as its group.
    let status = journal::stop_job(job, STOP_GRACE).await.and(status);
    #[cfg(not(target_os = "linux"))]
    let _ = job;
    // What is still in the pipes belongs to this process: drain it before
    // reporting it finished, or a poll right after exit misses the tail and
    // reap_settled can drop the handle holding it.
    let drained = tokio::time::timeout(PIPE_GRACE, async {
        for pump in &mut pumps {
            if let Err(e) = pump.await {
                capture.fail(format!("output reader failed: {e}"));
            }
        }
    })
    .await;
    if drained.is_err() {
        for pump in &pumps {
            pump.abort();
        }
        for pump in pumps {
            if !pump.is_finished() {
                let _ = pump.await;
            }
        }
        capture.fail("output pipes did not close after process exit; log may be incomplete");
    }
    if let Err(e) = &status {
        capture.fail(format!("waiting for process: {e}"));
    }
    #[cfg(target_os = "linux")]
    if status.is_ok()
        && let Some(registration) = registration
        && let Err(error) = registration.clear()
    {
        capture.fail(format!("removing background process record: {error}"));
    }
    if let Ok(mut b) = state.lock() {
        b.exit_code = status.ok().and_then(|s| s.code());
        b.finished = true;
    }
    drop(capture);
    let _ = reaped.send(true);
}

fn signal_group(pgid: Pid, signal: Signal) -> std::io::Result<()> {
    match killpg(pgid, signal) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Wait until the job's shell exits, leaving it unreaped, so its zombie
/// keeps the group id from reuse while the group is stopped.
#[cfg(target_os = "linux")]
async fn leader_exited(child: &mut tokio::process::Child) -> std::io::Result<()> {
    let Some(pid) = child.id() else {
        return child.wait().await.map(drop);
    };
    match exited_unreaped(pid).await {
        Ok(()) => Ok(()),
        Err(error) => {
            tracing::warn!(pid, %error, "cannot wait for the job's shell unreaped; reaping it");
            child.wait().await.map(drop)
        }
    }
}

#[cfg(not(target_os = "linux"))]
async fn leader_exited(child: &mut tokio::process::Child) -> std::io::Result<()> {
    child.wait().await.map(drop)
}

/// Wait until `pid`, a child of this process, exits, without reaping it.
#[cfg(target_os = "linux")]
async fn exited_unreaped(pid: u32) -> std::io::Result<()> {
    use rustix::process::{Pid as ProcessPid, WaitId, WaitIdOptions, waitid};
    use tokio::signal::unix::{SignalKind, signal};

    let raw = i32::try_from(pid)
        .ok()
        .and_then(ProcessPid::from_raw)
        .ok_or_else(|| std::io::Error::other(format!("not a process id: {pid}")))?;
    let peek = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    // Listening before the first look, so an exit between a look and the wait still wakes it.
    let mut exits = signal(SignalKind::child())?;
    while waitid(WaitId::Pid(raw), peek)?.is_none() {
        if exits.recv().await.is_none() {
            return Err(std::io::Error::other("the SIGCHLD stream closed"));
        }
    }
    Ok(())
}

/// What a job whose shell exited left in its group: SIGTERM, then SIGKILL
/// once it outlives the stop grace, unless a member is kept. The shell is
/// still unreaped, so the group id cannot be reused meanwhile.
#[cfg(target_os = "linux")]
async fn stop_exited_group(pgid: Pid) -> std::io::Result<()> {
    use journal::ExitedGroup;
    if journal::exited_group(pgid)? != ExitedGroup::Live {
        return Ok(());
    }
    if let Err(error) = signal_group(pgid, Signal::SIGTERM) {
        tracing::warn!(%pgid, %error, "SIGTERM failed; will attempt SIGKILL");
    }
    wait_group_gone(pgid).await;
    match journal::exited_group(pgid)? {
        ExitedGroup::Live => signal_group(pgid, Signal::SIGKILL),
        ExitedGroup::Empty | ExitedGroup::Kept => Ok(()),
    }
}

#[cfg(not(target_os = "linux"))]
async fn stop_exited_group(pgid: Pid) -> std::io::Result<()> {
    signal_group(pgid, Signal::SIGKILL)
}

/// Wait until `pgid` has no live member, at most the stop grace.
async fn wait_group_gone(pgid: Pid) {
    let deadline = tokio::time::Instant::now() + STOP_GRACE;
    loop {
        #[cfg(target_os = "linux")]
        match journal::group_is_running(pgid) {
            Ok(false) => return,
            Ok(true) => {}
            Err(error) => {
                tracing::warn!(%pgid, %error, "cannot inspect group; allowing full stop grace");
                tokio::time::sleep_until(deadline).await;
                return;
            }
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return;
        }
        tokio::time::sleep(remaining.min(Duration::from_millis(50))).await;
    }
}

/// Keep the leader unreaped through the grace period, pinning its group ID
/// even if it exits before descendants that ignore SIGTERM.
async fn signal_until_gone(
    child: &mut tokio::process::Child,
    pgid: Option<Pid>,
    teardown: Option<&Teardown>,
) -> std::io::Result<std::process::ExitStatus> {
    let mut killed = Ok(());
    if let Some(pgid) = pgid {
        if let Err(error) = signal_group(pgid, Signal::SIGTERM) {
            tracing::warn!(%pgid, %error, "SIGTERM failed; will attempt SIGKILL");
        }
        wait_group_gone(pgid).await;
        killed = signal_group(pgid, Signal::SIGKILL);
    }
    if let Some(teardown) = teardown {
        teardown.run();
    }
    killed?;
    child.wait().await
}

#[cfg(test)]
mod tests;
