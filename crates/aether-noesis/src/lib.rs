//! Noesis: Aether's learning service, dormant unless `noesis.enabled` is set.
//!
//! While disabled nothing is opened, registered or spawned: [`Noesis::start_if_enabled`]
//! validates the settings and returns `None` without touching the data directory.
//! A clean-room implementation; see the crate README for its provenance.

use std::io;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

pub mod budget;
pub mod settings;
pub mod store;

use budget::{Accountant, LaneSpend};
use settings::{NoesisSettings, SettingsError};
use store::{NoesisStore, StoreError};

/// The file name of Noesis's store inside Aether's data directory.
pub const DB_FILE: &str = "noesis.db";

#[derive(Debug, thiserror::Error)]
pub enum NoesisError {
    #[error("invalid [noesis] setting {0}")]
    Settings(#[from] SettingsError),
    #[error("Aether data directory {path} is unusable for Noesis: {source}")]
    DataDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// A running Noesis service: its settings, its store and its budget book.
#[derive(Debug)]
pub struct Noesis {
    settings: NoesisSettings,
    store: NoesisStore,
    accountant: Accountant,
}

impl Noesis {
    /// Start the service if `settings.enabled`; otherwise return `None` having
    /// opened, created and started nothing. `spent_today` seeds the budgets.
    pub fn start_if_enabled(
        settings: &NoesisSettings,
        data_dir: &Path,
        today: NaiveDate,
        spent_today: LaneSpend,
    ) -> Result<Option<Self>, NoesisError> {
        settings.validate()?;
        if !settings.enabled {
            return Ok(None);
        }
        let data_dir_error = |source| NoesisError::DataDir {
            path: data_dir.to_path_buf(),
            source,
        };
        let metadata = std::fs::metadata(data_dir).map_err(data_dir_error)?;
        if !metadata.is_dir() {
            let source = io::Error::new(io::ErrorKind::NotADirectory, "not a directory");
            return Err(data_dir_error(source));
        }
        let store = NoesisStore::open(&data_dir.join(DB_FILE))?;
        let accountant = Accountant::new(settings, today, spent_today);
        Ok(Some(Self {
            settings: settings.clone(),
            store,
            accountant,
        }))
    }

    /// Stop the service and close `noesis.db`.
    pub fn stop(self) -> Result<(), NoesisError> {
        debug_assert!(self.settings.enabled, "only an enabled service runs");
        self.store.close()?;
        Ok(())
    }

    /// The settings as stored.
    pub fn settings(&self) -> &NoesisSettings {
        &self.settings
    }

    /// The settings after the prerequisites of ARCHITECTURE.md §g.
    pub fn effective(&self) -> NoesisSettings {
        self.settings.effective()
    }

    pub fn store(&self) -> &NoesisStore {
        &self.store
    }

    pub fn accountant(&self) -> &Accountant {
        &self.accountant
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 4).unwrap()
    }

    fn entries(dir: &Path) -> Vec<String> {
        let names = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned());
        names.collect()
    }

    /// The daemon's path: read Aether's config, then start and stop Noesis.
    fn start_and_stop(config_dir: &Path, data_dir: &Path) -> bool {
        let config = aether_model::config::load_config(config_dir).unwrap();
        let settings = NoesisSettings::from_table(&config.noesis).unwrap();
        let started = Noesis::start_if_enabled(&settings, data_dir, today(), LaneSpend::default());
        match started.unwrap() {
            Some(noesis) => {
                noesis.stop().unwrap();
                true
            }
            None => false,
        }
    }

    #[test]
    fn with_the_default_config_noesis_db_never_exists() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        assert!(!start_and_stop(config_dir.path(), data_dir.path()));
        assert!(!data_dir.path().join(DB_FILE).exists());
        assert!(
            entries(data_dir.path()).is_empty(),
            "{:?}",
            entries(data_dir.path())
        );
        let text = "[noesis]\nenabled = false\nengineering_context = true\n";
        std::fs::write(config_dir.path().join("config.toml"), text).unwrap();
        assert!(!start_and_stop(config_dir.path(), data_dir.path()));
        assert!(
            entries(data_dir.path()).is_empty(),
            "{:?}",
            entries(data_dir.path())
        );
    }

    #[test]
    fn disabled_start_does_not_even_look_at_the_data_dir() {
        let missing = Path::new("/nonexistent/aether-noesis-test");
        let started = Noesis::start_if_enabled(
            &NoesisSettings::default(),
            missing,
            today(),
            LaneSpend::default(),
        );
        assert!(started.unwrap().is_none());
        assert!(!missing.exists());
    }

    #[test]
    fn enabling_opens_a_migrated_store_that_survives_a_restart() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            config_dir.path().join("config.toml"),
            "[noesis]\nenabled = true\n",
        )
        .unwrap();
        assert!(start_and_stop(config_dir.path(), data_dir.path()));
        assert!(data_dir.path().join(DB_FILE).is_file());
        assert!(start_and_stop(config_dir.path(), data_dir.path()));
        let settings = NoesisSettings {
            enabled: true,
            ..NoesisSettings::default()
        };
        let noesis =
            Noesis::start_if_enabled(&settings, data_dir.path(), today(), LaneSpend::default())
                .unwrap()
                .unwrap();
        assert_eq!(
            noesis.store().schema_version().unwrap(),
            store::SCHEMA_VERSION
        );
        assert_eq!(noesis.settings(), &settings);
        assert_eq!(noesis.effective(), settings.effective());
        assert_eq!(noesis.accountant().running(budget::Lane::Interactive), 0);
        noesis.stop().unwrap();
    }

    #[test]
    fn invalid_settings_are_refused_before_anything_is_opened() {
        let data_dir = tempfile::tempdir().unwrap();
        let settings = NoesisSettings {
            enabled: true,
            concurrency_interactive: 0,
            ..NoesisSettings::default()
        };
        let error =
            Noesis::start_if_enabled(&settings, data_dir.path(), today(), LaneSpend::default())
                .unwrap_err();
        assert!(matches!(error, NoesisError::Settings(_)), "{error}");
        assert!(
            error.to_string().contains("noesis.concurrency.interactive"),
            "{error}"
        );
        assert!(entries(data_dir.path()).is_empty());
    }

    #[test]
    fn an_enabled_start_needs_an_existing_data_dir() {
        let parent = tempfile::tempdir().unwrap();
        let missing = parent.path().join("missing");
        let settings = NoesisSettings {
            enabled: true,
            ..NoesisSettings::default()
        };
        let error = Noesis::start_if_enabled(&settings, &missing, today(), LaneSpend::default())
            .unwrap_err();
        assert!(matches!(error, NoesisError::DataDir { .. }), "{error}");
        assert!(!missing.exists());
    }

    #[test]
    fn the_started_budget_is_seeded_with_todays_spend() {
        let data_dir = tempfile::tempdir().unwrap();
        let mut settings = NoesisSettings {
            enabled: true,
            ..NoesisSettings::default()
        };
        settings.budget.interactive_usd_daily = 2.0;
        let spent = LaneSpend {
            interactive: budget::Spend {
                usd: 2.0,
                tokens: 0,
            },
            background: budget::Spend::default(),
        };
        let noesis = Noesis::start_if_enabled(&settings, data_dir.path(), today(), spent)
            .unwrap()
            .unwrap();
        let refusal = noesis
            .accountant()
            .admit(budget::Lane::Interactive, budget::Spend::default(), today())
            .unwrap_err();
        assert!(
            matches!(refusal, budget::Refusal::BudgetExhausted { .. }),
            "{refusal}"
        );
        noesis.stop().unwrap();
    }
}
