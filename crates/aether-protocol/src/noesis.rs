//! The Noesis commands' payloads, replies and boundary checks
//! (docs/vnext/CONTRACTS.md §6). The wire objects themselves live in
//! `aether_core::noesis`, because the `noesis.*` events carry them.

use aether_core::model::Usage;
use serde::{Deserialize, Serialize};

pub use aether_core::noesis::*;

use crate::{Access, Command};

pub const MAX_GOAL_CHARS: usize = 2000;
pub const MAX_TEXT_CHARS: usize = 20_000;
pub const MAX_NAME_CHARS: usize = 200;
pub const MAX_REASON_CHARS: usize = 500;
pub const MAX_SESSIONS_LIMIT: u32 = 200;
pub const MAX_GRAPH_LIMIT: u32 = 5000;
pub const MAX_LEARNER_LIMIT: u32 = 1000;
pub const MAX_EVIDENCE_LIMIT: u32 = 500;
/// The most excerpt text, in bytes, one evidence bundle may carry.
pub const MAX_BUNDLE_EXCERPT_BYTES: usize = 32 * 1024;
pub const MAX_INTERACTIVE_CONCURRENCY: u8 = 4;
pub const MAX_BACKGROUND_CONCURRENCY: u8 = 3;
pub const MAX_GRACE_SECONDS: u16 = 300;

/// A learner's answer to an open question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AnswerInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub dont_know: bool,
    /// One of [`CONFIDENCE_LEVELS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<u8>,
    /// What the learner reasoned, kept for the teacher; never shown back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

/// What deleting a session does to the propositions it produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeleteSessionGraph {
    Preserve,
    DeleteNodes,
}

/// What deleting a source does to the propositions it produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeleteSourceGraph {
    Preserve,
    DeleteUnsupported,
}

/// A subset of the `[noesis]` table to write. Absent keys keep their value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NoesisSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engineering_context: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction: Option<ExtractionPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<ConcurrencyPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<PriorityPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<ModelsPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing: Option<BillingPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable: Option<DisablePatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsPatch>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExtractionPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions: Option<SessionExtraction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engineering: Option<EngineeringExtraction>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BudgetPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive_usd_daily: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_usd_daily: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_tokens_daily: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConcurrencyPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PriorityPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<BackgroundPriority>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModelsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strong: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cheap: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BillingPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_api: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DisablePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_seconds: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PromptsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<bool>,
}

/// What Noesis spent today, split by who asked for the work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SpendToday {
    pub interactive: Usage,
    pub background: Usage,
}

/// What a disable does to work in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DisablePolicy {
    pub grace_seconds: u16,
    pub background: BackgroundDisable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundDisable {
    CancelAtModelBoundary,
}

/// Reply to `noesis_status` and `noesis_configure`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NoesisStatus {
    pub enabled: bool,
    /// As stored.
    pub settings: NoesisSettingsView,
    /// After prerequisites: a switch whose prerequisite is off reads off.
    pub effective: NoesisSettingsView,
    pub jobs_running: u32,
    pub spend_today: SpendToday,
    pub disable_policy: DisablePolicy,
    /// The prompt segments a file under the data dir currently replaces.
    pub prompt_overrides: Vec<String>,
}

fn sets_budget(patch: &NoesisSettingsPatch) -> bool {
    patch.budget.as_ref().is_some_and(|budget| {
        budget.interactive_usd_daily.is_some()
            || budget.background_usd_daily.is_some()
            || budget.background_tokens_daily.is_some()
    })
}

/// Writing `enabled`, a budget, billing or prompt overrides reaches into the
/// host's spending and files; the other keys are the operator's.
pub fn configure_access(patch: &NoesisSettingsPatch) -> Access {
    let sets_billing = patch
        .billing
        .as_ref()
        .is_some_and(|billing| billing.allow_api.is_some());
    let sets_overrides = patch
        .prompts
        .as_ref()
        .is_some_and(|prompts| prompts.overrides.is_some());
    if patch.enabled.is_some() || sets_budget(patch) || sets_billing || sets_overrides {
        Access::HostOnly
    } else {
        Access::Operate
    }
}

/// The settings after `patch`, with every key it names replaced.
pub fn apply_patch(view: &NoesisSettingsView, patch: &NoesisSettingsPatch) -> NoesisSettingsView {
    let mut out = view.clone();
    set(&mut out.enabled, patch.enabled);
    set(&mut out.engineering_context, patch.engineering_context);
    if let Some(extraction) = &patch.extraction {
        set(&mut out.extraction.sessions, extraction.sessions);
        set(&mut out.extraction.engineering, extraction.engineering);
    }
    if let Some(budget) = &patch.budget {
        set(
            &mut out.budget.interactive_usd_daily,
            budget.interactive_usd_daily,
        );
        set(
            &mut out.budget.background_usd_daily,
            budget.background_usd_daily,
        );
        set(
            &mut out.budget.background_tokens_daily,
            budget.background_tokens_daily,
        );
    }
    if let Some(concurrency) = &patch.concurrency {
        set(&mut out.concurrency.interactive, concurrency.interactive);
        set(&mut out.concurrency.background, concurrency.background);
    }
    if let Some(priority) = &patch.priority {
        set(&mut out.priority.background, priority.background);
    }
    if let Some(models) = &patch.models {
        replace(&mut out.models.strong, &models.strong);
        replace(&mut out.models.cheap, &models.cheap);
    }
    if let Some(billing) = &patch.billing {
        set(&mut out.billing.allow_api, billing.allow_api);
    }
    if let Some(disable) = &patch.disable {
        set(&mut out.disable.grace_seconds, disable.grace_seconds);
    }
    if let Some(prompts) = &patch.prompts {
        set(&mut out.prompts.overrides, prompts.overrides);
    }
    out
}

fn set<T>(slot: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *slot = value;
    }
}

fn replace(slot: &mut Option<String>, value: &Option<String>) {
    if value.is_some() {
        slot.clone_from(value);
    }
}

/// Check a command's fields against the documented ranges. Commands that are
/// not Noesis commands, and Noesis commands with nothing to range-check, pass.
pub fn validate(command: &Command) -> Result<(), String> {
    match command {
        Command::NoesisConfigure { patch, .. } => validate_patch(patch),
        Command::NoesisCreateSession {
            goal,
            context,
            practice_targets,
            ..
        } => validate_create_session(goal, context, practice_targets),
        Command::NoesisSend { session, text, .. } => {
            id("session", session)?;
            chars("text", text, MAX_TEXT_CHARS)
        }
        Command::NoesisAnswer {
            session,
            question,
            answer,
            ..
        } => {
            id("session", session)?;
            id("question", question)?;
            validate_answer(answer)
        }
        Command::NoesisGetSession { session } | Command::NoesisDeleteSession { session, .. } => {
            id("session", session)
        }
        Command::NoesisListSessions { after, limit } => {
            if let Some(after) = after {
                id("after", after)?;
            }
            limit_in("limit", *limit, MAX_SESSIONS_LIMIT)
        }
        Command::NoesisGraph {
            session,
            source,
            limit,
            ..
        } => {
            optional_id("session", session)?;
            optional_id("source", source)?;
            limit_in("limit", *limit, MAX_GRAPH_LIMIT)
        }
        _ => validate_references(command),
    }
}

fn validate_create_session(
    goal: &str,
    context: &Option<EvidenceBundle>,
    practice_targets: &Option<Vec<PropositionId>>,
) -> Result<(), String> {
    chars("goal", goal, MAX_GOAL_CHARS)?;
    for target in practice_targets.iter().flatten() {
        id("practice_targets", target)?;
    }
    context.as_ref().map_or(Ok(()), validate_bundle)
}

fn validate_references(command: &Command) -> Result<(), String> {
    match command {
        Command::NoesisDeleteProposition { proposition, .. } => id("proposition", proposition),
        Command::NoesisDeleteSource { source, .. } => id("source", source),
        Command::NoesisExtract {
            session, source, ..
        } => validate_extract_target(session, source),
        Command::NoesisImportSource { name, .. } => chars("name", name, MAX_NAME_CHARS),
        Command::NoesisCancelJob { job, .. } => id("job", job),
        Command::NoesisLearner {
            concepts,
            query: _,
            limit,
        } => {
            for concept in concepts.iter().flatten() {
                id("concepts", concept)?;
            }
            limit_in("limit", *limit, MAX_LEARNER_LIMIT)
        }
        Command::NoesisLearnerEvidence { concept, limit, .. } => {
            id("concept", concept)?;
            limit_in("limit", *limit, MAX_EVIDENCE_LIMIT)
        }
        Command::NoesisCorrectLearner {
            concept, reason, ..
        } => {
            id("concept", concept)?;
            chars("reason", reason, MAX_REASON_CHARS)
        }
        _ => Ok(()),
    }
}

/// `noesis_extract` reads one session or one source, never both or neither.
fn validate_extract_target(
    session: &Option<SessionId>,
    source: &Option<SourceId>,
) -> Result<(), String> {
    match (session, source) {
        (Some(session), None) => id("session", session),
        (None, Some(source)) => id("source", source),
        _ => Err("noesis_extract needs exactly one of session and source".into()),
    }
}

fn validate_answer(answer: &AnswerInput) -> Result<(), String> {
    if let Some(confidence) = answer.confidence {
        validate_confidence(confidence)?;
    }
    for choice in answer.choices.iter().flatten() {
        if choice.is_empty() {
            return Err("answer.choices holds an empty value".into());
        }
    }
    Ok(())
}

/// Check the excerpts of an evidence bundle against their shared budget.
pub fn validate_bundle(bundle: &EvidenceBundle) -> Result<(), String> {
    let total: usize = bundle.items.iter().map(|item| item.excerpt.len()).sum();
    if total > MAX_BUNDLE_EXCERPT_BYTES {
        return Err(format!(
            "context excerpts total {total} bytes; the limit is {MAX_BUNDLE_EXCERPT_BYTES}"
        ));
    }
    Ok(())
}

/// Check a settings patch against the ranges of §6.1.
pub fn validate_patch(patch: &NoesisSettingsPatch) -> Result<(), String> {
    if let Some(budget) = &patch.budget {
        usd("budget.interactive_usd_daily", budget.interactive_usd_daily)?;
        usd("budget.background_usd_daily", budget.background_usd_daily)?;
    }
    if let Some(concurrency) = &patch.concurrency {
        range(
            "concurrency.interactive",
            concurrency.interactive,
            1,
            MAX_INTERACTIVE_CONCURRENCY,
        )?;
        range(
            "concurrency.background",
            concurrency.background,
            0,
            MAX_BACKGROUND_CONCURRENCY,
        )?;
    }
    if let Some(disable) = &patch.disable {
        range(
            "disable.grace_seconds",
            disable.grace_seconds,
            0,
            MAX_GRACE_SECONDS,
        )?;
    }
    if let Some(models) = &patch.models {
        for (key, model) in [("strong", &models.strong), ("cheap", &models.cheap)] {
            if model.as_deref().is_some_and(str::is_empty) {
                return Err(format!("models.{key} must name a model"));
            }
        }
    }
    Ok(())
}

fn usd(key: &str, value: Option<f64>) -> Result<(), String> {
    match value {
        Some(value) if !value.is_finite() || value < 0.0 => {
            Err(format!("{key} must be a number of at least 0, got {value}"))
        }
        _ => Ok(()),
    }
}

fn range<T: PartialOrd + std::fmt::Display + Copy>(
    key: &str,
    value: Option<T>,
    min: T,
    max: T,
) -> Result<(), String> {
    match value {
        Some(value) if value < min || value > max => {
            Err(format!("{key} must be {min}..={max}, got {value}"))
        }
        _ => Ok(()),
    }
}

fn chars(field: &str, value: &str, max: usize) -> Result<(), String> {
    let count = value.chars().count();
    if (1..=max).contains(&count) {
        return Ok(());
    }
    Err(format!("{field} must be 1..={max} characters, got {count}"))
}

fn limit_in(field: &str, limit: Option<u32>, max: u32) -> Result<(), String> {
    range(field, limit, 1, max)
}

fn id(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    Ok(())
}

fn optional_id(field: &str, value: &Option<String>) -> Result<(), String> {
    value.as_deref().map_or(Ok(()), |value| id(field, value))
}

#[cfg(test)]
mod tests;
