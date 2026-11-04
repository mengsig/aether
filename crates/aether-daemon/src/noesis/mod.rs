//! The Noesis service in the daemon: started and stopped from the `[noesis]`
//! config, switched by `noesis_configure`, and drained on disable (a grace
//! for interactive turns, then cancel). The runtime's guard follows its gate.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_model::Models;
use aether_noesis::Noesis;
use aether_noesis::settings::NoesisSettings;
use aether_protocol::noesis::{NoesisSettingsPatch, NoesisSettingsView, NoesisStatus};
use aether_protocol::{Command, ErrorReply, Reply};
use aether_runtime::Runtime;
use aether_runtime::noesis_guard::NoesisGate;
use aether_store::{Commit, Store};

mod budget;
mod config_file;
mod drain;
mod status;
#[cfg(test)]
mod tests;

use budget::ServiceBudget;
use config_file::ConfigFile;

/// Where a stopped interactive turn is waited for between checks.
const DRAIN_POLL: Duration = Duration::from_millis(200);

enum State {
    Off,
    Running(Noesis),
    /// Disabled while turns were in flight; `generation` names this drain so
    /// a re-enable can call it off.
    Draining {
        noesis: Noesis,
        generation: u64,
    },
}

/// The daemon's one Noesis service, dormant unless `noesis.enabled`.
pub struct NoesisService {
    config: ConfigFile,
    data_dir: PathBuf,
    models: Arc<Models>,
    gate: NoesisGate,
    state: tokio::sync::Mutex<State>,
    drains: std::sync::atomic::AtomicU64,
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
        }
    }

    /// A stopped service on `runtime`'s gate whose config lives nowhere, so
    /// a configure fails loudly: for daemons assembled in tests.
    #[cfg(test)]
    pub(crate) fn detached(runtime: &Runtime, models: &Arc<Models>) -> Arc<Self> {
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
    ) -> Result<Noesis, NoesisStartError> {
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
        self.gate.enable(Arc::new(ServiceBudget {
            accountant: noesis.accountant().clone(),
            allow_api: settings.billing_allow_api,
            models: self.models.clone(),
        }));
        Ok(noesis)
    }

    /// `noesis_configure`: validate, write `[noesis]`, then start, restart or
    /// drain the service to match. The write always comes first.
    pub async fn configure(
        self: &Arc<Self>,
        runtime: &Arc<Runtime>,
        patch: &NoesisSettingsPatch,
        by: Actor,
    ) -> Result<NoesisStatus, ErrorReply> {
        aether_protocol::noesis::validate_patch(patch).map_err(ErrorReply::invalid)?;
        let mut state = self.state.lock().await;
        let stored = self.config.read_noesis().map_err(ErrorReply::internal)?;
        let current =
            NoesisSettings::from_table(&stored).map_err(|e| ErrorReply::invalid(e.to_string()))?;
        let next_view = aether_protocol::noesis::apply_patch(&view_of(&current)?, patch);
        let next = settings_of(&next_view)?;
        let table = next
            .to_table()
            .map_err(|e| ErrorReply::invalid(e.to_string()))?;
        self.config
            .write_noesis(&table)
            .map_err(ErrorReply::internal)?;
        let transition = self.transition(&mut state, &next, runtime).await;
        drop(state);
        if current.enabled || next.enabled {
            self.announce(runtime, &next, by)?;
        }
        transition?;
        self.status(runtime).await
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
                State::Running(noesis) => self.begin_drain(noesis, next, runtime),
                other => other,
            };
            return Ok(());
        }
        let running = match previous {
            State::Off => None,
            State::Running(noesis) | State::Draining { noesis, .. } => Some(noesis),
        };
        if let Some(noesis) = running {
            if noesis.settings() == next {
                self.reopen_gate(&noesis);
                *state = State::Running(noesis);
                return Ok(());
            }
            noesis.stop().map_err(ErrorReply::internal)?;
        }
        let started = self.start(next, runtime.store()).await;
        match started {
            Ok(noesis) => {
                *state = State::Running(noesis);
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

    fn reopen_gate(&self, noesis: &Noesis) {
        self.drains
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.gate.enable(Arc::new(ServiceBudget {
            accountant: noesis.accountant().clone(),
            allow_api: noesis.settings().billing_allow_api,
            models: self.models.clone(),
        }));
    }

    /// Record `noesis.settings_changed` with the stored and effective settings.
    fn announce(
        &self,
        runtime: &Runtime,
        next: &NoesisSettings,
        by: Actor,
    ) -> Result<(), ErrorReply> {
        let settings = view_of(next)?;
        let effective = view_of(&self.effective(next))?;
        let event = EventDraft::new(
            by,
            EventKind::NoesisSettingsChanged {
                settings,
                effective,
            },
        );
        runtime
            .apply(Commit::new().event(event))
            .map_err(crate::connection::map_err)?;
        Ok(())
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
        let noesis = match previous {
            State::Off => return,
            State::Running(noesis) | State::Draining { noesis, .. } => noesis,
        };
        if let Err(error) = noesis.stop() {
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

/// Answer a Noesis command. Every one but status and configure is refused
/// with `noesis_disabled` while the service is not running, before anything else.
pub(crate) async fn dispatch(
    daemon: &crate::Daemon,
    command: Command,
    by: Actor,
) -> Result<Reply, ErrorReply> {
    debug_assert!(command.is_noesis(), "only noesis commands reach here");
    let service = &daemon.noesis;
    match command {
        Command::NoesisStatus => Ok(Reply::NoesisStatus(service.status(&daemon.runtime).await?)),
        Command::NoesisConfigure { patch, .. } => {
            let status = service.configure(&daemon.runtime, &patch, by).await?;
            Ok(Reply::NoesisStatus(status))
        }
        other => {
            debug_assert!(other.needs_noesis_service());
            if !service.gate.is_enabled() {
                return Err(ErrorReply::noesis_disabled());
            }
            aether_protocol::noesis::validate(&other).map_err(ErrorReply::invalid)?;
            Err(ErrorReply::unimplemented(format!(
                "`{}` (the Noesis teaching, learner and graph services)",
                other.name()
            )))
        }
    }
}
