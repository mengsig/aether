//! Extraction jobs in the daemon: workers on blocking threads, at most
//! `noesis.concurrency.background` at once, each on its own `noesis.db`
//! connection. The extractor and auditor run as Aether agents with the
//! background roles, so the runtime's guard admits and charges every turn.

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, ArtifactId, TaskId};
use aether_core::model::{MessageRole, ModelRef};
use aether_core::task::TaskStatus;
use aether_noesis::Noesis;
use aether_noesis::budget::Spend;
use aether_noesis::extract::jobs::fail_interrupted;
use aether_noesis::extract::session_origin;
use aether_noesis::extract::{
    BackgroundBudget, CancelFlag, Corpus, ExtractError, ExtractModel, JobEvent, JobPorts, Message,
    ModelError, Reply, Role, Slots, claim_next, run_job,
};
use aether_noesis::store::NoesisStore;
use aether_protocol::ErrorReply;
use aether_runtime::noesis_guard::NOESIS_SUBSYSTEM;
use aether_runtime::{CreateAgentSpec, Runtime};
use aether_store::{Commit, Store};

use crate::noesis::budget::ServiceBudget;

/// The largest source an import reads (parity X8).
pub(super) const MAX_SOURCE_BYTES: u64 = 25 * 1024 * 1024;
/// How often a worker looks whether its extraction agent finished.
const CALL_POLL: Duration = Duration::from_millis(200);
/// The longest one extractor or auditor call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The running service's extraction jobs.
pub(in crate::noesis) struct Extraction {
    db_path: PathBuf,
    slots: Slots,
    flags: parking_lot::Mutex<HashMap<String, CancelFlag>>,
    budget: Arc<ServiceBudget>,
    model: Option<ModelRef>,
    /// Both switches of engineering extraction, as this service started.
    engineering_in_effect: bool,
    stopped: AtomicBool,
}

impl std::fmt::Debug for Extraction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extraction")
            .field("running", &self.slots.running())
            .finish_non_exhaustive()
    }
}

impl Extraction {
    /// The jobs of a service just started. Jobs a stopped service left
    /// unfinished end as failed first: extracting again resumes from coverage.
    pub(in crate::noesis) fn start(
        noesis: &Noesis,
        budget: Arc<ServiceBudget>,
    ) -> Result<Arc<Self>, ExtractError> {
        let interrupted = fail_interrupted(noesis.store().connection())?;
        if !interrupted.is_empty() {
            tracing::warn!(
                count = interrupted.len(),
                "ended Noesis jobs a stopped service left unfinished"
            );
        }
        let settings = noesis.settings();
        let model = match settings.model_cheap.as_deref() {
            None => None,
            Some(text) => Some(ModelRef::parse(text).ok_or_else(|| {
                ExtractError::Invalid(format!(
                    "noesis.models.cheap `{text}` is not provider/model"
                ))
            })?),
        };
        Ok(Arc::new(Self {
            db_path: noesis.store().path().to_path_buf(),
            slots: Slots::new(settings.concurrency_background),
            flags: parking_lot::Mutex::new(HashMap::new()),
            budget,
            model,
            engineering_in_effect: aether_noesis::evidence::in_effect(&noesis.effective()),
            stopped: AtomicBool::new(false),
        }))
    }

    /// Ask a running job to stop at its next model-call boundary. A job
    /// claimed but not yet registered finds the cancelled flag left for it.
    pub(in crate::noesis) fn cancel(&self, job: &str) {
        assert!(job.starts_with("j_"), "a job id");
        self.flags
            .lock()
            .entry(job.to_string())
            .or_default()
            .cancel();
    }

    #[cfg(test)]
    pub(in crate::noesis) fn is_cancelling(&self, job: &str) -> bool {
        self.flags
            .lock()
            .get(job)
            .is_some_and(CancelFlag::is_cancelled)
    }

    /// Stop taking jobs and cancel every running one at its next boundary;
    /// each keeps the merge it committed last.
    pub(in crate::noesis) fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        for flag in self.flags.lock().values() {
            flag.cancel();
        }
    }

    /// Start a worker if a slot is free; it runs queued jobs until none is left.
    pub(in crate::noesis) fn pump(self: &Arc<Self>, runtime: &Arc<Runtime>) {
        if self.stopped.load(Ordering::SeqCst) {
            return;
        }
        let (this, runtime) = (self.clone(), runtime.clone());
        tokio::task::spawn_blocking(move || {
            if let Err(error) = this.work(&runtime) {
                tracing::error!(%error, "a Noesis extraction worker stopped");
            }
        });
    }

    fn work(&self, runtime: &Arc<Runtime>) -> Result<(), String> {
        let store = NoesisStore::open(&self.db_path).map_err(|e| e.to_string())?;
        while !self.stopped.load(Ordering::SeqCst) {
            let Some((slot, job)) =
                claim_next(store.connection(), &self.slots).map_err(|e| e.to_string())?
            else {
                break;
            };
            let flag = self.flags.lock().entry(job.id.clone()).or_default().clone();
            if self.stopped.load(Ordering::SeqCst) {
                flag.cancel();
            }
            let id = job.id.clone();
            let outcome = self.run(&store, runtime, job, &flag);
            self.flags.lock().remove(&id);
            drop(slot);
            if let Err(error) = outcome {
                tracing::error!(job = %id, %error, "a Noesis extraction job failed in the store");
            }
        }
        store.close().map_err(|e| e.to_string())
    }

    fn run(
        &self,
        noesis: &NoesisStore,
        runtime: &Arc<Runtime>,
        job: aether_core::noesis::Job,
        flag: &CancelFlag,
    ) -> Result<(), String> {
        announce(runtime, JobEvent::Updated(job.clone()));
        let mut model = AgentModel {
            runtime: runtime.clone(),
            model: self.model.clone(),
            cancel: flag.clone(),
        };
        let mut budget = GuardBudget(self.budget.clone());
        let corpus = StoreCorpus::new(noesis, runtime.store().clone());
        let mut events = |event| announce(runtime, event);
        let ports = JobPorts {
            model: &mut model,
            budget: &mut budget,
            corpus: &corpus,
            cancel: flag,
            events: &mut events,
            engineering_in_effect: self.engineering_in_effect,
        };
        run_job(noesis.connection(), job, ports)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// Record a job's progress as a durable `noesis` event.
pub(in crate::noesis) fn announce(runtime: &Runtime, event: JobEvent) {
    let kind = match event {
        JobEvent::Updated(job) => EventKind::NoesisJobUpdated { job },
        JobEvent::Merged { job, stats } => EventKind::NoesisGraphMerged { job, stats },
    };
    let commit = Commit::new().event(EventDraft::new(Actor::System, kind));
    if let Err(error) = runtime.apply(commit) {
        tracing::error!(%error, "recording a Noesis job event failed");
    }
}

/// Refuses a call while the service's background budget is closed or spent.
/// The call's turn is then admitted, and charged, by the runtime's guard.
struct GuardBudget(Arc<ServiceBudget>);

impl BackgroundBudget for GuardBudget {
    fn before_invocation(&mut self, _estimate: Spend) -> Option<String> {
        self.0.background_refusal()
    }

    fn settle(&mut self, _spent: Spend) {}
}

/// Each call is one Aether agent under a background role, whose task
/// result carries the reply. A cancel cancels the agent and ends the wait.
struct AgentModel {
    runtime: Arc<Runtime>,
    model: Option<ModelRef>,
    cancel: CancelFlag,
}

impl ExtractModel for AgentModel {
    fn invoke(&mut self, role: Role, prompt: &str) -> Result<Reply, ModelError> {
        assert!(!prompt.trim().is_empty(), "a model call needs a prompt");
        let spec = CreateAgentSpec {
            role: role.aether_role().into(),
            name: Some(role.aether_role().replace('.', " ")),
            objective: Some(format!(
                "{prompt}\n\nWhen you have the JSON object, finish by calling complete_task \
                 with exactly that JSON object as its summary."
            )),
            model: self.model.clone(),
            subsystem: Some(NOESIS_SUBSYSTEM.into()),
            start: true,
            ..CreateAgentSpec::default()
        };
        let (agent, task) = self
            .runtime
            .create_agent(spec, Actor::System)
            .map_err(|e| ModelError::NotSent(e.to_string()))?;
        let task = task.ok_or_else(|| ModelError::NotSent("the agent got no task".into()))?;
        let waited = self.wait_for_result(agent.id, task.id);
        if waited.is_err() {
            self.stop_agent(agent.id);
        }
        let text = waited?;
        let usage = match self.runtime.store().agent(agent.id) {
            Ok(Some(agent)) => agent.stats.usage,
            Ok(None) => return Err(ModelError::Failed("the extraction agent vanished".into())),
            Err(error) => return Err(ModelError::Failed(error.to_string())),
        };
        Ok(Reply { text, usage })
    }
}

impl AgentModel {
    /// The finished task's reply, or why it has none: failed, parked by the
    /// guard (a spent budget or a disable), cancelled, or out of time.
    fn wait_for_result(&self, agent: AgentId, task: TaskId) -> Result<String, ModelError> {
        let store = self.runtime.store();
        let failed = |e: aether_store::StoreError| ModelError::Failed(e.to_string());
        let started = Instant::now();
        loop {
            let row = store.task(task).map_err(failed)?;
            let row = row.ok_or_else(|| ModelError::Failed("the agent's task vanished".into()))?;
            match row.status {
                TaskStatus::Completed { result } => {
                    return Ok(result.structured.map_or(result.summary, |v| v.to_string()));
                }
                TaskStatus::Failed { error, .. } => return Err(ModelError::Failed(error)),
                TaskStatus::Cancelled { .. } => return Err(ModelError::Failed("cancelled".into())),
                _ => {}
            }
            let row = store.agent(agent).map_err(failed)?;
            if row.is_none_or(|a| a.paused || a.status.is_terminal()) {
                let why = "the extraction agent was parked or ended without a result";
                return Err(ModelError::Failed(why.into()));
            }
            if self.cancel.is_cancelled() {
                return Err(ModelError::Failed("the job was cancelled".into()));
            }
            if started.elapsed() > CALL_TIMEOUT {
                return Err(ModelError::Failed("the extraction call timed out".into()));
            }
            std::thread::sleep(CALL_POLL);
        }
    }

    /// End an extraction agent the job no longer waits for, so it spends nothing more.
    fn stop_agent(&self, agent: AgentId) {
        let ended = self.runtime.store().agent(agent).ok().flatten();
        if ended.is_some_and(|a| a.status.is_terminal()) {
            return;
        }
        let reason = Some("the Noesis extraction job stopped waiting".into());
        if let Err(error) = self.runtime.cancel_agent(agent, reason, Actor::System) {
            tracing::warn!(%agent, %error, "cancelling an abandoned extraction agent failed");
        }
    }
}

/// Session prose from the session agent's persisted (sealed) transcript;
/// source text from the imported artifact.
pub(super) struct StoreCorpus<'a> {
    noesis: &'a NoesisStore,
    store: Arc<dyn Store>,
}

impl<'a> StoreCorpus<'a> {
    pub(super) fn new(noesis: &'a NoesisStore, store: Arc<dyn Store>) -> Self {
        Self { noesis, store }
    }
}

impl Corpus for StoreCorpus<'_> {
    fn session_prose(&self, session: &str) -> Result<Vec<Message>, String> {
        let origin =
            session_origin(self.noesis.connection(), session).map_err(|e| e.to_string())?;
        let origin = origin.ok_or_else(|| format!("no session `{session}`"))?;
        if origin.practice {
            return Err("a practice session never writes to the graph".into());
        }
        let agent = AgentId::from_str(&origin.agent)
            .map_err(|e| format!("session agent `{}`: {e}", origin.agent))?;
        let entries = self.store.transcript(agent, 0).map_err(|e| e.to_string())?;
        Ok(entries
            .into_iter()
            .filter(|entry| entry.message.role == MessageRole::Assistant)
            .map(|entry| Message {
                message_ref: format!("{agent}:{}", entry.seq),
                text: entry.message.text(),
            })
            .filter(|message| !message.text.trim().is_empty())
            .collect())
    }

    fn source_text(&self, artifact: &str) -> Result<String, String> {
        let id =
            ArtifactId::from_str(artifact).map_err(|e| format!("artifact `{artifact}`: {e}"))?;
        read_source(self.store.as_ref(), id).map_err(|e| e.message)
    }
}

/// An artifact's text: at most [`MAX_SOURCE_BYTES`], and UTF-8.
pub(super) fn read_source(store: &dyn Store, artifact: ArtifactId) -> Result<String, ErrorReply> {
    let meta = store.artifact(artifact).map_err(store_error)?;
    let meta = meta.ok_or_else(|| ErrorReply::not_found(format!("artifact `{artifact}`")))?;
    if meta.size_bytes > MAX_SOURCE_BYTES {
        return Err(ErrorReply::invalid(format!(
            "the artifact is larger than the {MAX_SOURCE_BYTES}-byte source limit"
        )));
    }
    let bytes = store.read_artifact_bytes(artifact).map_err(store_error)?;
    let bytes = bytes
        .ok_or_else(|| ErrorReply::invalid(format!("artifact `{artifact}` stores no content")))?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(ErrorReply::invalid(
            "the artifact grew past the source limit",
        ));
    }
    String::from_utf8(bytes).map_err(|_| ErrorReply::invalid("a source must be UTF-8 text"))
}

fn store_error(error: aether_store::StoreError) -> ErrorReply {
    ErrorReply::internal(format!("reading the source artifact: {error}"))
}
