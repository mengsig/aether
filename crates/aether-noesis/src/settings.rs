//! The `[noesis]` settings of CONTRACTS.md §6.1: parsing with useful errors,
//! validation, serialization back to TOML, and the prerequisite rules of
//! ARCHITECTURE.md §g that turn stored settings into effective ones.

use std::ops::RangeInclusive;

use toml::{Table, Value};

pub const INTERACTIVE_CONCURRENCY: RangeInclusive<u8> = 1..=4;
pub const BACKGROUND_CONCURRENCY: RangeInclusive<u8> = 0..=3;
pub const GRACE_SECONDS: RangeInclusive<u16> = 0..=300;

/// One invalid `[noesis]` setting, named by its full dotted key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{key}` {reason}")]
pub struct SettingsError {
    pub key: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionExtraction {
    Manual,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineeringExtraction {
    Off,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundPriority {
    Low,
    Normal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoesisSettings {
    pub enabled: bool,
    pub engineering_context: bool,
    pub extraction_sessions: SessionExtraction,
    pub extraction_engineering: EngineeringExtraction,
    pub budget: BudgetSettings,
    pub concurrency_interactive: u8,
    pub concurrency_background: u8,
    pub priority_background: BackgroundPriority,
    /// `provider/model`; `None` means the first ready subscription model.
    pub model_strong: Option<String>,
    pub model_cheap: Option<String>,
    pub billing_allow_api: bool,
    pub disable_grace_seconds: u16,
    pub prompt_overrides: bool,
}

/// Daily limits. A USD limit of 0 refuses every call on its lane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BudgetSettings {
    pub interactive_usd_daily: f64,
    pub background_usd_daily: f64,
    pub background_tokens_daily: u64,
}

impl Default for NoesisSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            engineering_context: false,
            extraction_sessions: SessionExtraction::Manual,
            extraction_engineering: EngineeringExtraction::Off,
            budget: BudgetSettings {
                interactive_usd_daily: 5.0,
                background_usd_daily: 1.0,
                background_tokens_daily: 2_000_000,
            },
            concurrency_interactive: 2,
            concurrency_background: 1,
            priority_background: BackgroundPriority::Low,
            model_strong: None,
            model_cheap: None,
            billing_allow_api: false,
            disable_grace_seconds: 60,
            prompt_overrides: false,
        }
    }
}

const SESSION_EXTRACTION: &[(&str, SessionExtraction)] = &[
    ("manual", SessionExtraction::Manual),
    ("auto", SessionExtraction::Auto),
];
const ENGINEERING_EXTRACTION: &[(&str, EngineeringExtraction)] = &[
    ("off", EngineeringExtraction::Off),
    ("auto", EngineeringExtraction::Auto),
];
const PRIORITY: &[(&str, BackgroundPriority)] = &[
    ("low", BackgroundPriority::Low),
    ("normal", BackgroundPriority::Normal),
];

const BUDGET_KEYS: &[&str] = &[
    "interactive_usd_daily",
    "background_usd_daily",
    "background_tokens_daily",
];

const ROOT_KEYS: &[&str] = &[
    "enabled",
    "engineering_context",
    "extraction",
    "budget",
    "concurrency",
    "priority",
    "models",
    "billing",
    "disable",
    "prompts",
];

impl NoesisSettings {
    /// Read the `[noesis]` table (the contents, not the wrapper). Missing keys
    /// take their defaults; unknown keys and bad values are errors.
    pub fn from_table(table: &Table) -> Result<Self, SettingsError> {
        let root = Scope::root(table, ROOT_KEYS)?;
        let mut settings = Self::default();
        settings.read_switches(&root)?;
        settings.read_limits(&root)?;
        settings.validate()?;
        Ok(settings)
    }

    fn read_switches(&mut self, root: &Scope<'_>) -> Result<(), SettingsError> {
        let extraction = root.child("extraction", &["sessions", "engineering"])?;
        let models = root.child("models", &["strong", "cheap"])?;
        let billing = root.child("billing", &["allow_api"])?;
        let prompts = root.child("prompts", &["overrides"])?;
        self.enabled = root.bool("enabled", self.enabled)?;
        self.engineering_context = root.bool("engineering_context", self.engineering_context)?;
        self.extraction_sessions =
            extraction.choice("sessions", SESSION_EXTRACTION, self.extraction_sessions)?;
        self.extraction_engineering = extraction.choice(
            "engineering",
            ENGINEERING_EXTRACTION,
            self.extraction_engineering,
        )?;
        self.model_strong = models.model("strong")?;
        self.model_cheap = models.model("cheap")?;
        self.billing_allow_api = billing.bool("allow_api", self.billing_allow_api)?;
        self.prompt_overrides = prompts.bool("overrides", self.prompt_overrides)?;
        Ok(())
    }

    fn read_limits(&mut self, root: &Scope<'_>) -> Result<(), SettingsError> {
        let budget = root.child("budget", BUDGET_KEYS)?;
        let concurrency = root.child("concurrency", &["interactive", "background"])?;
        let priority = root.child("priority", &["background"])?;
        let disable = root.child("disable", &["grace_seconds"])?;
        let limits = &mut self.budget;
        limits.interactive_usd_daily =
            budget.usd("interactive_usd_daily", limits.interactive_usd_daily)?;
        limits.background_usd_daily =
            budget.usd("background_usd_daily", limits.background_usd_daily)?;
        limits.background_tokens_daily =
            budget.tokens("background_tokens_daily", limits.background_tokens_daily)?;
        self.concurrency_interactive = concurrency.small(
            "interactive",
            INTERACTIVE_CONCURRENCY,
            self.concurrency_interactive,
        )?;
        self.concurrency_background = concurrency.small(
            "background",
            BACKGROUND_CONCURRENCY,
            self.concurrency_background,
        )?;
        self.priority_background =
            priority.choice("background", PRIORITY, self.priority_background)?;
        self.disable_grace_seconds =
            disable.small("grace_seconds", GRACE_SECONDS, self.disable_grace_seconds)?;
        Ok(())
    }

    /// Check every value against its documented range. Fields are public, so
    /// a value built in code is checked here too, not only one read from TOML.
    pub fn validate(&self) -> Result<(), SettingsError> {
        let error = |key: &str, reason: String| SettingsError {
            key: format!("noesis.{key}"),
            reason,
        };
        for (key, usd) in [
            (
                "budget.interactive_usd_daily",
                self.budget.interactive_usd_daily,
            ),
            (
                "budget.background_usd_daily",
                self.budget.background_usd_daily,
            ),
        ] {
            if !usd.is_finite() || usd < 0.0 {
                return Err(error(
                    key,
                    format!("must be a finite number ≥ 0, got {usd}"),
                ));
            }
        }
        if i64::try_from(self.budget.background_tokens_daily).is_err() {
            return Err(error(
                "budget.background_tokens_daily",
                format!("must be at most {}", i64::MAX),
            ));
        }
        for (key, value, range) in [
            (
                "concurrency.interactive",
                self.concurrency_interactive,
                INTERACTIVE_CONCURRENCY,
            ),
            (
                "concurrency.background",
                self.concurrency_background,
                BACKGROUND_CONCURRENCY,
            ),
        ] {
            if !range.contains(&value) {
                return Err(error(key, out_of_range(&range, value)));
            }
        }
        if !GRACE_SECONDS.contains(&self.disable_grace_seconds) {
            let reason = out_of_range(&GRACE_SECONDS, self.disable_grace_seconds);
            return Err(error("disable.grace_seconds", reason));
        }
        for (key, model) in [
            ("models.strong", &self.model_strong),
            ("models.cheap", &self.model_cheap),
        ] {
            if let Some(model) = model {
                check_model_id(model).map_err(|reason| error(key, reason))?;
            }
        }
        Ok(())
    }

    /// The settings as a `[noesis]` table, with every key written out, so the
    /// stored file says what is in force. Refuses to write invalid settings.
    pub fn to_table(&self) -> Result<Table, SettingsError> {
        self.validate()?;
        let tokens =
            i64::try_from(self.budget.background_tokens_daily).map_err(|_| SettingsError {
                key: "noesis.budget.background_tokens_daily".to_string(),
                reason: "is out of range".to_string(),
            })?;
        let mut models = Table::new();
        for (key, model) in [("strong", &self.model_strong), ("cheap", &self.model_cheap)] {
            if let Some(model) = model {
                models.insert(key.to_string(), Value::from(model.as_str()));
            }
        }
        let mut table = Table::new();
        table.insert("enabled".into(), self.enabled.into());
        table.insert(
            "engineering_context".into(),
            self.engineering_context.into(),
        );
        table.insert(
            "extraction".into(),
            section([
                (
                    "sessions",
                    name_of(SESSION_EXTRACTION, self.extraction_sessions).into(),
                ),
                (
                    "engineering",
                    name_of(ENGINEERING_EXTRACTION, self.extraction_engineering).into(),
                ),
            ]),
        );
        table.insert(
            "budget".into(),
            section([
                (
                    "interactive_usd_daily",
                    self.budget.interactive_usd_daily.into(),
                ),
                (
                    "background_usd_daily",
                    self.budget.background_usd_daily.into(),
                ),
                ("background_tokens_daily", tokens.into()),
            ]),
        );
        table.insert(
            "concurrency".into(),
            section([
                (
                    "interactive",
                    i64::from(self.concurrency_interactive).into(),
                ),
                ("background", i64::from(self.concurrency_background).into()),
            ]),
        );
        table.insert(
            "priority".into(),
            section([(
                "background",
                name_of(PRIORITY, self.priority_background).into(),
            )]),
        );
        table.insert("models".into(), Value::Table(models));
        table.insert(
            "billing".into(),
            section([("allow_api", self.billing_allow_api.into())]),
        );
        table.insert(
            "disable".into(),
            section([(
                "grace_seconds",
                i64::from(self.disable_grace_seconds).into(),
            )]),
        );
        table.insert(
            "prompts".into(),
            section([("overrides", self.prompt_overrides.into())]),
        );
        Ok(table)
    }

    /// The settings in effect after ARCHITECTURE.md §g's prerequisites: a
    /// switch is effective only when its prerequisites are. Stored values stay.
    pub fn effective(&self) -> Self {
        let engineering_context = self.enabled && self.engineering_context;
        let extraction_sessions = if self.enabled {
            self.extraction_sessions
        } else {
            SessionExtraction::Manual
        };
        let extraction_engineering = if engineering_context {
            self.extraction_engineering
        } else {
            EngineeringExtraction::Off
        };
        Self {
            engineering_context,
            extraction_sessions,
            extraction_engineering,
            ..self.clone()
        }
    }
}

fn section<const N: usize>(entries: [(&str, Value); N]) -> Value {
    let table: Table = entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    debug_assert_eq!(table.len(), N, "duplicate key in a settings section");
    Value::Table(table)
}

fn name_of<T: Copy + PartialEq>(options: &[(&'static str, T)], value: T) -> &'static str {
    let name = options.iter().find(|(_, option)| *option == value);
    debug_assert!(name.is_some(), "every variant has a TOML name");
    name.map_or("", |(name, _)| name)
}

fn out_of_range<T: std::fmt::Display>(range: &RangeInclusive<T>, value: T) -> String {
    format!(
        "must be between {} and {}, got {value}",
        range.start(),
        range.end()
    )
}

fn check_model_id(model: &str) -> Result<(), String> {
    if aether_core::model::ModelRef::parse(model).is_none() {
        return Err(format!(
            "must be provider/model, got `{model}`; leave it unset for the first ready subscription model"
        ));
    }
    if model.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("must not contain whitespace or control characters".to_string());
    }
    Ok(())
}

/// One table of `[noesis]` being read, with the dotted path for its errors.
struct Scope<'a> {
    table: Option<&'a Table>,
    prefix: String,
}

impl<'a> Scope<'a> {
    fn root(table: &'a Table, known: &[&str]) -> Result<Self, SettingsError> {
        let scope = Self {
            table: Some(table),
            prefix: "noesis.".to_string(),
        };
        scope.reject_unknown(known)?;
        Ok(scope)
    }

    fn child(&self, name: &str, known: &[&str]) -> Result<Scope<'a>, SettingsError> {
        let table = match self.get(name) {
            None => None,
            Some(Value::Table(table)) => Some(table),
            Some(other) => return Err(self.wrong_type(name, "a table", other)),
        };
        let scope = Scope {
            table,
            prefix: format!("{}{name}.", self.prefix),
        };
        scope.reject_unknown(known)?;
        Ok(scope)
    }

    fn reject_unknown(&self, known: &[&str]) -> Result<(), SettingsError> {
        let Some(table) = self.table else {
            return Ok(());
        };
        let mut unknown: Vec<&str> = table
            .keys()
            .map(String::as_str)
            .filter(|key| !known.contains(key))
            .collect();
        unknown.sort_unstable();
        let Some(first) = unknown.first() else {
            return Ok(());
        };
        let mut reason = format!("is not a setting; known keys here: {}", known.join(", "));
        if unknown.len() > 1 {
            let all = unknown.iter().map(|key| format!("`{}{key}`", self.prefix));
            reason.push_str(&format!(
                " (all unknown: {})",
                all.collect::<Vec<_>>().join(", ")
            ));
        }
        Err(self.error(first, &reason))
    }

    fn get(&self, key: &str) -> Option<&'a Value> {
        self.table.and_then(|table| table.get(key))
    }

    fn error(&self, key: &str, reason: &str) -> SettingsError {
        SettingsError {
            key: format!("{}{key}", self.prefix),
            reason: reason.to_string(),
        }
    }

    fn wrong_type(&self, key: &str, expected: &str, found: &Value) -> SettingsError {
        let reason = format!("must be {expected}, got {} `{found}`", found.type_str());
        self.error(key, &reason)
    }

    fn bool(&self, key: &str, default: bool) -> Result<bool, SettingsError> {
        match self.get(key) {
            None => Ok(default),
            Some(Value::Boolean(value)) => Ok(*value),
            Some(other) => Err(self.wrong_type(key, "true or false", other)),
        }
    }

    fn choice<T: Copy>(
        &self,
        key: &str,
        options: &[(&str, T)],
        default: T,
    ) -> Result<T, SettingsError> {
        let names = options
            .iter()
            .map(|(name, _)| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(" or ");
        let text = match self.get(key) {
            None => return Ok(default),
            Some(Value::String(text)) => text,
            Some(other) => return Err(self.wrong_type(key, &names, other)),
        };
        options
            .iter()
            .find(|(name, _)| name == text)
            .map(|(_, value)| *value)
            .ok_or_else(|| self.error(key, &format!("must be {names}, got \"{text}\"")))
    }

    fn usd(&self, key: &str, default: f64) -> Result<f64, SettingsError> {
        let value = match self.get(key) {
            None => return Ok(default),
            Some(Value::Float(value)) => *value,
            Some(Value::Integer(value)) => *value as f64,
            Some(other) => return Err(self.wrong_type(key, "a number of USD ≥ 0", other)),
        };
        if !value.is_finite() || value < 0.0 {
            return Err(self.error(key, &format!("must be a finite number ≥ 0, got {value}")));
        }
        Ok(value)
    }

    fn tokens(&self, key: &str, default: u64) -> Result<u64, SettingsError> {
        let value = self.integer(key, 0..=i64::MAX.unsigned_abs())?;
        Ok(value.unwrap_or(default))
    }

    fn small<T>(&self, key: &str, range: RangeInclusive<T>, default: T) -> Result<T, SettingsError>
    where
        T: Copy + Into<u64> + TryFrom<u64>,
    {
        let wide = (*range.start()).into()..=(*range.end()).into();
        let Some(value) = self.integer(key, wide)? else {
            return Ok(default);
        };
        T::try_from(value).map_err(|_| self.error(key, "is out of range"))
    }

    fn integer(&self, key: &str, range: RangeInclusive<u64>) -> Result<Option<u64>, SettingsError> {
        let value = match self.get(key) {
            None => return Ok(None),
            Some(Value::Integer(value)) => *value,
            Some(other) => {
                let expected = format!("a whole number from {} to {}", range.start(), range.end());
                return Err(self.wrong_type(key, &expected, other));
            }
        };
        match u64::try_from(value) {
            Ok(value) if range.contains(&value) => Ok(Some(value)),
            _ => {
                let reason = format!(
                    "must be between {} and {}, got {value}",
                    range.start(),
                    range.end()
                );
                Err(self.error(key, &reason))
            }
        }
    }

    fn model(&self, key: &str) -> Result<Option<String>, SettingsError> {
        let model = match self.get(key) {
            None => return Ok(None),
            Some(Value::String(model)) => model,
            Some(other) => return Err(self.wrong_type(key, "a provider/model string", other)),
        };
        check_model_id(model).map_err(|reason| self.error(key, &reason))?;
        Ok(Some(model.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<NoesisSettings, SettingsError> {
        let table: Table = toml::from_str(text).unwrap();
        NoesisSettings::from_table(&table)
    }

    fn every_value_changed() -> NoesisSettings {
        NoesisSettings {
            enabled: true,
            engineering_context: true,
            extraction_sessions: SessionExtraction::Auto,
            extraction_engineering: EngineeringExtraction::Auto,
            budget: BudgetSettings {
                interactive_usd_daily: 0.0,
                background_usd_daily: 12.25,
                background_tokens_daily: 7,
            },
            concurrency_interactive: 4,
            concurrency_background: 0,
            priority_background: BackgroundPriority::Normal,
            model_strong: Some("claude-code/opus".into()),
            model_cheap: Some("codex/gpt-mini".into()),
            billing_allow_api: true,
            disable_grace_seconds: 300,
            prompt_overrides: true,
        }
    }

    #[test]
    fn defaults_are_the_contract_values() {
        let settings = parse("").unwrap();
        assert_eq!(settings, NoesisSettings::default());
        assert!(!settings.enabled && !settings.engineering_context);
        assert_eq!(settings.extraction_sessions, SessionExtraction::Manual);
        assert_eq!(settings.extraction_engineering, EngineeringExtraction::Off);
        assert_eq!(settings.budget.interactive_usd_daily, 5.0);
        assert_eq!(settings.budget.background_usd_daily, 1.0);
        assert_eq!(settings.budget.background_tokens_daily, 2_000_000);
        assert_eq!(
            (
                settings.concurrency_interactive,
                settings.concurrency_background
            ),
            (2, 1)
        );
        assert_eq!(settings.priority_background, BackgroundPriority::Low);
        assert_eq!((settings.model_strong, settings.model_cheap), (None, None));
        assert!(!settings.billing_allow_api && !settings.prompt_overrides);
        assert_eq!(settings.disable_grace_seconds, 60);
    }

    #[test]
    fn settings_round_trip_through_a_table_and_toml_text() {
        for settings in [NoesisSettings::default(), every_value_changed()] {
            let table = settings.to_table().unwrap();
            assert_eq!(NoesisSettings::from_table(&table).unwrap(), settings);
            let text = toml::to_string(&table).unwrap();
            assert_eq!(parse(&text).unwrap(), settings, "{text}");
        }
    }

    #[test]
    fn written_tables_spell_out_every_key() {
        let table = every_value_changed().to_table().unwrap();
        let text = toml::to_string(&table).unwrap();
        for key in [
            "enabled = true",
            "engineering_context = true",
            "sessions = \"auto\"",
            "engineering = \"auto\"",
            "interactive_usd_daily = 0.0",
            "background_tokens_daily = 7",
            "background = \"normal\"",
            "strong = \"claude-code/opus\"",
            "allow_api = true",
            "grace_seconds = 300",
            "overrides = true",
        ] {
            assert!(text.contains(key), "missing `{key}` in\n{text}");
        }
    }

    #[test]
    fn a_partial_table_keeps_the_other_defaults() {
        let settings = parse("[budget]\ninteractive_usd_daily = 3\n").unwrap();
        assert_eq!(settings.budget.interactive_usd_daily, 3.0);
        assert_eq!(settings.budget.background_usd_daily, 1.0);
        assert_eq!(settings.concurrency_interactive, 2);
    }

    #[test]
    fn invalid_values_name_the_key_and_say_what_is_allowed() {
        let cases = [
            (
                "enabled = \"yes\"",
                "noesis.enabled",
                "true or false, got string",
            ),
            (
                "enabeld = true",
                "noesis.enabeld",
                "known keys here: enabled,",
            ),
            ("budget = 5", "noesis.budget", "must be a table"),
            (
                "[budget]\ndaily = 1",
                "noesis.budget.daily",
                "interactive_usd_daily",
            ),
            (
                "[budget]\ninteractive_usd_daily = -1",
                "noesis.budget.interactive_usd_daily",
                "≥ 0",
            ),
            (
                "[budget]\nbackground_usd_daily = nan",
                "noesis.budget.background_usd_daily",
                "finite",
            ),
            (
                "[budget]\ninteractive_usd_daily = inf",
                "noesis.budget.interactive_usd_daily",
                "finite",
            ),
            (
                "[budget]\ninteractive_usd_daily = \"5\"",
                "noesis.budget.interactive_usd_daily",
                "USD",
            ),
            (
                "[budget]\nbackground_tokens_daily = -1",
                "noesis.budget.background_tokens_daily",
                "got -1",
            ),
            (
                "[budget]\nbackground_tokens_daily = 1.5",
                "noesis.budget.background_tokens_daily",
                "whole number",
            ),
            (
                "[concurrency]\ninteractive = 0",
                "noesis.concurrency.interactive",
                "between 1 and 4, got 0",
            ),
            (
                "[concurrency]\ninteractive = 5",
                "noesis.concurrency.interactive",
                "between 1 and 4, got 5",
            ),
            (
                "[concurrency]\nbackground = 4",
                "noesis.concurrency.background",
                "between 0 and 3, got 4",
            ),
            (
                "[disable]\ngrace_seconds = 301",
                "noesis.disable.grace_seconds",
                "between 0 and 300",
            ),
            (
                "[extraction]\nsessions = \"always\"",
                "noesis.extraction.sessions",
                "\"manual\" or \"auto\"",
            ),
            (
                "[extraction]\nengineering = \"on\"",
                "noesis.extraction.engineering",
                "\"off\" or \"auto\"",
            ),
            (
                "[priority]\nbackground = \"high\"",
                "noesis.priority.background",
                "\"low\" or \"normal\"",
            ),
            (
                "[models]\nstrong = \"opus\"",
                "noesis.models.strong",
                "provider/model",
            ),
            (
                "[models]\ncheap = \"\"",
                "noesis.models.cheap",
                "provider/model",
            ),
            (
                "[models]\ncheap = \"a/b c\"",
                "noesis.models.cheap",
                "whitespace",
            ),
            (
                "[billing]\nallow_api = 1",
                "noesis.billing.allow_api",
                "true or false",
            ),
        ];
        for (text, key, reason) in cases {
            let error = parse(text).unwrap_err();
            assert_eq!(error.key, key, "{text}: {error}");
            assert!(error.to_string().contains(reason), "{text}: {error}");
        }
    }

    #[test]
    fn several_unknown_keys_are_all_listed() {
        let error = parse("zeta = 1\nalpha = 2").unwrap_err();
        assert_eq!(error.key, "noesis.alpha");
        assert!(
            error.reason.contains("`noesis.alpha`, `noesis.zeta`"),
            "{error}"
        );
    }

    #[test]
    fn values_built_in_code_are_validated_and_never_written_invalid() {
        let mut settings = NoesisSettings {
            concurrency_interactive: 9,
            ..NoesisSettings::default()
        };
        assert_eq!(
            settings.validate().unwrap_err().key,
            "noesis.concurrency.interactive"
        );
        assert!(settings.to_table().is_err());
        settings.concurrency_interactive = 1;
        settings.budget.background_usd_daily = f64::NAN;
        assert_eq!(
            settings.validate().unwrap_err().key,
            "noesis.budget.background_usd_daily"
        );
        settings.budget.background_usd_daily = 1.0;
        settings.budget.background_tokens_daily = u64::MAX;
        assert!(settings.to_table().is_err());
        settings.budget.background_tokens_daily = 1;
        settings.model_strong = Some("nope".into());
        assert_eq!(settings.validate().unwrap_err().key, "noesis.models.strong");
    }

    // ARCHITECTURE.md §g, written out row by row rather than recomputed:
    // (enabled, engineering_context, sessions auto, engineering auto)
    //   -> effective (engineering_context, sessions auto, engineering auto).
    const PREREQUISITES: [([bool; 4], [bool; 3]); 16] = [
        ([false, false, false, false], [false, false, false]),
        ([false, false, false, true], [false, false, false]),
        ([false, false, true, false], [false, false, false]),
        ([false, false, true, true], [false, false, false]),
        ([false, true, false, false], [false, false, false]),
        ([false, true, false, true], [false, false, false]),
        ([false, true, true, false], [false, false, false]),
        ([false, true, true, true], [false, false, false]),
        ([true, false, false, false], [false, false, false]),
        ([true, false, false, true], [false, false, false]),
        ([true, false, true, false], [false, true, false]),
        ([true, false, true, true], [false, true, false]),
        ([true, true, false, false], [true, false, false]),
        ([true, true, false, true], [true, false, true]),
        ([true, true, true, false], [true, true, false]),
        ([true, true, true, true], [true, true, true]),
    ];

    #[test]
    fn effective_settings_follow_the_prerequisite_table_in_every_combination() {
        let mut seen = std::collections::BTreeSet::new();
        for (stored, expected) in PREREQUISITES {
            assert!(seen.insert(stored), "duplicate row {stored:?}");
            let [enabled, engineering_context, sessions, engineering] = stored;
            let settings = NoesisSettings {
                enabled,
                engineering_context,
                extraction_sessions: if sessions {
                    SessionExtraction::Auto
                } else {
                    SessionExtraction::Manual
                },
                extraction_engineering: if engineering {
                    EngineeringExtraction::Auto
                } else {
                    EngineeringExtraction::Off
                },
                ..every_value_changed()
            };
            let before = settings.clone();
            let effective = settings.effective();
            let got = [
                effective.engineering_context,
                effective.extraction_sessions == SessionExtraction::Auto,
                effective.extraction_engineering == EngineeringExtraction::Auto,
            ];
            assert_eq!(got, expected, "stored {stored:?}");
            assert_eq!(
                effective.enabled, enabled,
                "the master switch has no prerequisite"
            );
            assert_eq!(
                settings, before,
                "a prerequisite never rewrites a stored value"
            );
            let others = NoesisSettings {
                engineering_context: settings.engineering_context,
                extraction_sessions: settings.extraction_sessions,
                extraction_engineering: settings.extraction_engineering,
                ..effective.clone()
            };
            assert_eq!(others, settings, "only the three switches are resolved");
            assert_eq!(
                effective.effective(),
                effective,
                "resolving twice changes nothing"
            );
        }
        assert_eq!(seen.len(), 16, "every combination of the four switches");
    }
}
