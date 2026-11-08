//! The Noesis service in the daemon: started and stopped from the `[noesis]`
//! config, switched by `noesis_configure`, and drained on disable (a grace
//! for interactive turns, then cancel). The runtime's guard follows its gate.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aether_core::event::Actor;
use aether_core::idempotency::Submission;
use aether_model::Models;
use aether_noesis::Noesis;
use aether_noesis::settings::NoesisSettings;
use aether_protocol::noesis::NoesisSettingsView;
use aether_protocol::{Command, ErrorReply, Reply};
use aether_runtime::noesis_guard::NoesisGate;
use aether_store::Store;

mod budget;
mod config_file;
mod configure;
mod drain;
mod engine;
mod engine_step;
mod graph;
mod learner;
mod routes;
mod status;
mod teaching;
#[cfg(test)]
mod teaching_tests;
#[cfg(test)]
mod tests;

use budget::ServiceBudget;
use config_file::ConfigFile;

/// Where a stopped interactive turn is waited for between checks.
const DRAIN_POLL: Duration = Duration::from_millis(200);

enum State {
    Off,
    Running(Live),
    /// Disabled while turns were in flight; `generation` names this drain so
    /// a re-enable can call it off.
    Draining {
        live: Live,
        generation: u64,
    },
}

/// A started service, the budget its gate admits through, what its
/// teaching loop keeps between calls, and its extraction jobs.
struct Live {
    /// Locked for store operations, but released during model calls.
    desk: Arc<tokio::sync::Mutex<Desk>>,
    settings: NoesisSettings,
    budget: Arc<ServiceBudget>,
    extraction: Arc<graph::Extraction>,
    /// Cancelled when this service stops, ending a teaching model call.
    cancel: tokio_util::sync::CancellationToken,
}

/// The open `noesis.db` and what teaching calls hand on to the next one.
struct Desk {
    /// Taken when the service stops; a call that finds none was too late.
    noesis: Option<Noesis>,
    /// Inputs a teaching call could not deliver, for the next call.
    undelivered: aether_noesis::teach::Undelivered,
    /// The run every Noesis agent is created in, found on first use.
    run: Option<aether_core::id::RunId>,
    turn: Arc<TurnReservation>,
}

/// Reserves write ordering while a turn is between its store operations.
#[derive(Default)]
struct TurnReservation {
    active: std::sync::atomic::AtomicBool,
    changed: tokio::sync::Notify,
}

struct ReservedTurn(Arc<TurnReservation>);

impl Drop for ReservedTurn {
    fn drop(&mut self) {
        let was_active = self
            .0
            .active
            .swap(false, std::sync::atomic::Ordering::SeqCst);
        debug_assert!(was_active);
        self.0.changed.notify_waiters();
    }
}

impl Desk {
    fn reserve_turn(&self) -> ReservedTurn {
        let was_active = self
            .turn
            .active
            .swap(true, std::sync::atomic::Ordering::SeqCst);
        debug_assert!(!was_active);
        ReservedTurn(self.turn.clone())
    }
}

/// Wait for the current turn to finish without delaying independent reads.
async fn writable_desk(desk: &tokio::sync::Mutex<Desk>) -> tokio::sync::MutexGuard<'_, Desk> {
    loop {
        let guard = desk.lock().await;
        if !guard.turn.active.load(std::sync::atomic::Ordering::SeqCst) {
            return guard;
        }
        let turn = guard.turn.clone();
        let notified = turn.changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        drop(guard);
        if turn.active.load(std::sync::atomic::Ordering::SeqCst) {
            notified.await;
        }
    }
}

impl Live {
    /// Retire the budget, stop extraction jobs at their next model-call
    /// boundary, end a teaching model call in flight, and close `noesis.db`
    /// once the call holding it lets go.
    async fn stop(self) -> Result<(), aether_noesis::NoesisError> {
        self.extraction.stop();
        self.cancel.cancel();
        self.budget.retire();
        let noesis = writable_desk(&self.desk).await.noesis.take();
        debug_assert!(noesis.is_some(), "a service stops once");
        noesis.map_or(Ok(()), Noesis::stop)
    }
}

/// The daemon's one Noesis service, dormant unless `noesis.enabled`.
pub struct NoesisService {
    config: ConfigFile,
    data_dir: PathBuf,
    models: Arc<Models>,
    gate: NoesisGate,
    state: tokio::sync::Mutex<State>,
    drains: std::sync::atomic::AtomicU64,
    /// Teaching calls running or waiting for the desk, which a drain
    /// waits for through the grace.
    teaching_calls: std::sync::atomic::AtomicUsize,
    /// Replaces every provider's client for teaching calls, in tests.
    model_client: Option<Arc<dyn aether_model::ModelClient>>,
}

impl std::fmt::Debug for NoesisService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoesisService")
            .field("data_dir", &self.data_dir)
            .field("enabled", &self.gate.is_enabled())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NoesisStartError {
    #[error(transparent)]
    Config(#[from] config_file::ConfigFileError),
    #[error("invalid [noesis] setting {0}")]
    Settings(#[from] aether_noesis::settings::SettingsError),
    #[error(transparent)]
    Noesis(#[from] aether_noesis::NoesisError),
    #[error("noesis.enabled is false, so Noesis did not start")]
    Disabled,
    #[error("reading today's Noesis spend: {0}")]
    Spend(#[from] aether_store::StoreError),
    #[error("ending the extraction jobs a stopped service left: {0}")]
    Jobs(#[from] aether_noesis::extract::ExtractError),
}

impl NoesisService {
    /// A stopped service whose gate the runtime is built with; `data_dir`
    /// holds `noesis.db`, opened only once Noesis is enabled.
    pub fn new(config_dir: &Path, data_dir: &Path, models: Arc<Models>, gate: NoesisGate) -> Self {
        debug_assert!(!gate.is_enabled(), "a new service starts stopped");
        Self {
            config: ConfigFile::in_dir(config_dir),
            data_dir: data_dir.to_path_buf(),
            models,
            gate,
            state: tokio::sync::Mutex::new(State::Off),
            drains: std::sync::atomic::AtomicU64::new(0),
            teaching_calls: std::sync::atomic::AtomicUsize::new(0),
            model_client: None,
        }
    }

    /// Teaching model calls go to `client` instead of a provider: for tests.
    #[cfg(test)]
    pub(crate) fn with_model_client(mut self, client: Arc<dyn aether_model::ModelClient>) -> Self {
        self.model_client = Some(client);
        self
    }

    /// A stopped service on `runtime`'s gate whose config lives nowhere, so
    /// a configure fails loudly: for daemons assembled in tests.
    #[cfg(test)]
    pub(crate) fn detached(runtime: &aether_runtime::Runtime, models: &Arc<Models>) -> Arc<Self> {
        let nowhere = Path::new("/nonexistent/aether-noesis-test");
        Arc::new(Self::new(
            nowhere,
            nowhere,
            models.clone(),
            runtime.noesis_gate().clone(),
        ))
    }

    pub fn gate(&self) -> &NoesisGate {
        &self.gate
    }

    /// Start the service when the stored settings enable it, before the
    /// runtime recovers its agents. Disabled, this reads the config only.
    pub async fn start_at_boot(&self, store: &Arc<dyn Store>) -> Result<bool, NoesisStartError> {
        let settings = NoesisSettings::from_table(&self.config.read_noesis()?)?;
        let mut state = self.state.lock().await;
        debug_assert!(
            matches!(*state, State::Off),
            "boot starts a stopped service"
        );
        if !settings.enabled {
            return Ok(false);
        }
        *state = State::Running(self.start(&settings, store).await?);
        Ok(true)
    }

    /// Open `noesis.db` with today's spend seeded, and open the gate on it.
    async fn start(
        &self,
        settings: &NoesisSettings,
        store: &Arc<dyn Store>,
    ) -> Result<Live, NoesisStartError> {
        debug_assert!(settings.enabled, "only an enabled service starts");
        let day = budget::today();
        let reader = store.clone();
        let spent = tokio::task::spawn_blocking(move || budget::spent_today(reader.as_ref(), day))
            .await
            .map_err(|e| {
                aether_store::StoreError::Corrupt(format!("the spend read did not finish: {e}"))
            })??;
        let noesis = Noesis::start_if_enabled(settings, &self.data_dir, day, spent)?
            .ok_or(NoesisStartError::Disabled)?;
        let budget = Arc::new(ServiceBudget::new(
            noesis.accountant().clone(),
            settings,
            self.models.clone(),
        ));
        let extraction = graph::Extraction::start(&noesis, budget.clone())?;
        self.gate.enable(budget.clone());
        debug_assert!(self.gate.is_enabled());
        let cancel = tokio_util::sync::CancellationToken::new();
        let desk = Desk {
            noesis: Some(noesis),
            undelivered: Default::default(),
            run: None,
            turn: Arc::new(TurnReservation::default()),
        };
        Ok(Live {
            desk: Arc::new(tokio::sync::Mutex::new(desk)),
            settings: settings.clone(),
            budget,
            extraction,
            cancel,
        })
    }

    /// The settings in effect: their prerequisites applied, and off while
    /// the service is not running whatever is stored.
    fn effective(&self, stored: &NoesisSettings) -> NoesisSettings {
        if self.gate.is_enabled() {
            return stored.effective();
        }
        NoesisSettings {
            enabled: false,
            ..stored.clone()
        }
        .effective()
    }

    /// Stop the service with the daemon; in-flight turns end with it.
    pub async fn shutdown(&self) {
        self.gate.disable();
        let previous = std::mem::replace(&mut *self.state.lock().await, State::Off);
        let live = match previous {
            State::Off => return,
            State::Running(live) | State::Draining { live, .. } => live,
        };
        if let Err(error) = live.stop().await {
            tracing::error!(%error, "closing noesis.db failed");
        }
    }
}

/// The stored settings in their wire shape, which mirrors the TOML table.
fn view_of(settings: &NoesisSettings) -> Result<NoesisSettingsView, ErrorReply> {
    let table = settings
        .to_table()
        .map_err(|e| ErrorReply::internal(e.to_string()))?;
    toml::Value::Table(table)
        .try_into()
        .map_err(|e: toml::de::Error| ErrorReply::internal(format!("the [noesis] wire shape: {e}")))
}

/// Settings from their wire shape, validated as the config loader does.
fn settings_of(view: &NoesisSettingsView) -> Result<NoesisSettings, ErrorReply> {
    let table = toml::Table::try_from(view).map_err(|e| ErrorReply::internal(e.to_string()))?;
    NoesisSettings::from_table(&table).map_err(|e| ErrorReply::invalid(e.to_string()))
}

/// Every `noesis_*` command.
pub(crate) fn owns(command: &Command) -> bool {
    command.is_noesis()
}

/// Answer a Noesis command through its group in [`routes::group_of`]. Every
/// one but status and configure is refused with `noesis_disabled` while the
/// service is not running, before anything else.
pub(crate) async fn dispatch(
    daemon: &crate::Daemon,
    command: Command,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    debug_assert!(command.is_noesis(), "only noesis commands reach here");
    let service = &daemon.noesis;
    let group = routes::group_of(&command);
    if group == routes::Group::Service {
        return service_command(daemon, command, by, submission).await;
    }
    debug_assert!(command.needs_noesis_service());
    if !service.gate.is_enabled() {
        return Err(ErrorReply::noesis_disabled());
    }
    aether_protocol::noesis::validate(&command).map_err(ErrorReply::invalid)?;
    let runtime = &daemon.runtime;
    match group {
        routes::Group::Service => Err(ErrorReply::internal("routed above")),
        routes::Group::Learner => {
            service
                .learner(runtime, command, by, submission.as_ref())
                .await
        }
        routes::Group::Teaching => {
            service
                .teaching(runtime, command, by, submission.as_ref())
                .await
        }
        routes::Group::Graph => service.graph(runtime, command, submission.as_ref()).await,
    }
}

/// `noesis_status` and `noesis_configure`, which work while disabled.
async fn service_command(
    daemon: &crate::Daemon,
    command: Command,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let service = &daemon.noesis;
    match command {
        Command::NoesisStatus => Ok(Reply::NoesisStatus(service.status(&daemon.runtime).await?)),
        Command::NoesisConfigure { patch, .. } => {
            let status = service
                .configure(&daemon.runtime, &patch, by, submission.as_ref())
                .await?;
            Ok(Reply::NoesisStatus(status))
        }
        other => Err(ErrorReply::internal(format!(
            "`{}` is not a service command",
            other.name()
        ))),
    }
}
