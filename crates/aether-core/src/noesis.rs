//! The Noesis wire objects (docs/vnext/CONTRACTS.md §6): the teaching
//! session, its questions and problems, the proposition graph, extraction
//! jobs, the learner model, the evidence bundle a session starts from and the
//! settings view. They live in core because the `noesis.*` events carry them
//! and the event vocabulary cannot depend on the protocol crate.

use serde::{Deserialize, Serialize};

use crate::id::AgentId;
use crate::model::Usage;
use crate::time::Timestamp;

/// `ns_`-prefixed.
pub type SessionId = String;
/// `q_`-prefixed.
pub type QuestionId = String;
/// `p_`-prefixed and content-derived.
pub type PropositionId = String;
/// `c_`-prefixed and content-derived.
pub type ChainId = String;
/// `k_`-prefixed.
pub type ConceptId = String;
/// `s_`-prefixed.
pub type SourceId = String;
/// `j_`-prefixed.
pub type JobId = String;

/// The confidence values a learner may attach to an answer.
pub const CONFIDENCE_LEVELS: [u8; 6] = [0, 30, 50, 70, 90, 100];
/// The highest rung of the hint ladder.
pub const MAX_HINT_RUNG: u8 = 5;

/// Whether `confidence` is one of [`CONFIDENCE_LEVELS`].
pub fn validate_confidence(confidence: u8) -> Result<(), String> {
    if CONFIDENCE_LEVELS.contains(&confidence) {
        return Ok(());
    }
    Err(format!(
        "confidence {confidence} is not one of {CONFIDENCE_LEVELS:?}"
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Teach,
    Practice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Probe,
    Plan,
    Teach,
    Practice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NoesisSession {
    pub id: SessionId,
    pub goal: String,
    pub mode: SessionMode,
    pub phase: Phase,
    pub agent: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanTree>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_question: Option<Question>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_problem: Option<Problem>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// Every agent that ran the session, in the order they ran: the Prober,
    /// then the Teacher or the Practitioner. Only `noesis_get_session` fills
    /// the four history fields below; events and older daemons leave them out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<SessionAgent>,
    /// Every question posed, oldest first, with the answer and its result.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<AskedQuestion>,
    /// Every practice problem posed, oldest first, with its conclusion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<PosedProblem>,
    /// What the session's model calls used, split by how they were paid for.
    /// Boxed so the session's event stays small; the wire form is the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Box<SessionUsage>>,
    /// The evidence bundle the session started from, when it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<EvidenceBundle>,
    /// Why the session's last turn could not run, until a later turn runs.
    /// Every session read carries it; events leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stalled: Option<SessionStall>,
}

/// A turn that stopped before any model call, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionStall {
    pub reason: StallReason,
    /// What the learner can do about it, naming the setting to change.
    pub message: String,
    pub at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StallReason {
    /// Today's interactive budget (`noesis.budget.interactive_usd_daily`) is spent.
    BudgetExhausted,
}

/// What an agent did in its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionRole {
    Prober,
    Teacher,
    Practitioner,
}

/// One agent of a session and the inputs Noesis wrote to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionAgent {
    pub agent: AgentId,
    pub role: SessionRole,
    /// The transcript seqs of the user-role messages Noesis wrote for the
    /// agent (the opening context, a hand-off). They are never the
    /// learner's own words, so a client does not show them as the learner's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub internal: Vec<u64>,
}

/// A posed question with what the learner answered and what it revealed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AskedQuestion {
    pub question: Question,
    /// Absent while the question is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<RecordedAnswer>,
    /// Absent while the question is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<QuestionResult>,
}

/// The learner's answer as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RecordedAnswer {
    /// The labels of the options chosen, in the order they were shown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chosen: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub dont_know: bool,
    /// One of [`CONFIDENCE_LEVELS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<u8>,
    /// The reasoning the learner gave with the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    pub answered_at: Timestamp,
}

/// A posed problem with its conclusion once it has one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PosedProblem {
    pub problem: Problem,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<ProblemConclusion>,
}

/// How a problem ended, as `noesis.practice_concluded` announced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProblemConclusion {
    pub outcome: PracticeOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rung: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub miss_class: Option<MissClass>,
    pub retest: bool,
    pub solution: String,
    pub concluded_at: Timestamp,
}

/// A session's usage by billing: subscription use is covered by the
/// learner's plan and never charged per request; API use is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionUsage {
    pub subscription: Usage,
    pub api: Usage,
    /// Use on a provider no longer configured, so what paid for it is unknown.
    #[serde(default)]
    pub unknown: Usage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlanTree {
    pub revision: u32,
    pub nodes: Vec<PlanNode>,
    pub edges: Vec<PlanEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlanNode {
    /// A slug, unique within the plan.
    pub id: String,
    pub title: String,
    pub status: PlanNodeStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanNodeStatus {
    Upcoming,
    Current,
    Visited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlanEdge {
    pub from: String,
    pub to: String,
}

/// A question put to the learner. The answer key is never part of it: it
/// is revealed in a [`QuestionResult`] once the answer is recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Question {
    pub id: QuestionId,
    pub kind: QuestionKind,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<QuestionOption>>,
    pub multi: bool,
    pub collapsed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_node: Option<String>,
    pub concepts: Vec<ConceptId>,
    pub posed_at: Timestamp,
    /// The lesson step the question is, for a client to render it as one.
    /// Older daemons leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<QuestionStep>,
}

/// What a question does in the lesson: a probe measures, a teach-phase
/// question predicts before a step or checks after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuestionStep {
    Measure,
    Predict,
    Check,
    Goal,
    Practice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    Choice,
    Recall,
    Goal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QuestionOption {
    pub value: String,
    pub label: String,
}

/// What an answer revealed: one value for a recall question, several for a
/// multiple choice one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum AnswerKey {
    Many(Vec<String>),
    One(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QuestionResult {
    pub question: QuestionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correct: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_key: Option<AnswerKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
}

/// A practice problem. Its solution is revealed only by
/// `noesis.practice_concluded`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Problem {
    pub id: String,
    pub statement: String,
    pub mode: ProblemMode,
    pub targets: Vec<PropositionId>,
    pub posed_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProblemMode {
    Recall,
    Discrimination,
    Composition,
}

/// Where a proposition or chain came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "NoesisProvenance")]
pub struct Provenance {
    /// A [`SessionId`] or a [`SourceId`].
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Proposition {
    pub id: PropositionId,
    pub name: String,
    pub statement: String,
    pub aliases: Vec<String>,
    pub provenance: Vec<Provenance>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Chain {
    pub id: ChainId,
    pub premises: Vec<PropositionId>,
    pub conclusion: PropositionId,
    pub provenance: Vec<Provenance>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Cluster {
    pub id: String,
    pub session: SessionId,
    pub members: Vec<PropositionId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    /// The session or source the job reads.
    pub target: String,
    pub state: JobState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<JobStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub usage: Usage,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    ExtractSession,
    ExtractSource,
    ExtractEngineering,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Merging,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobState {
    /// Whether the job can no longer change.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct JobStats {
    pub nodes: u32,
    pub chains: u32,
    pub dropped_chains: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConceptStateKind {
    Unseen,
    Exposed,
    Demonstrated,
    Fragile,
    Misconception,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConceptKind {
    Proposition,
    PlanNode,
    Topic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConceptRef {
    pub id: ConceptId,
    pub kind: ConceptKind,
    /// The id of the proposition, plan node or topic the concept stands for.
    #[serde(rename = "ref")]
    pub reference: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConceptState {
    pub concept: ConceptRef,
    pub state: ConceptStateKind,
    /// 0 to 1.
    pub strength: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_due_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration: Option<f64>,
    pub misconceptions: Vec<String>,
    /// The id of the newest evidence this state was derived from.
    pub as_of_evidence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    ProbeAnswer,
    PredictAnswer,
    CheckAnswer,
    RecallJudged,
    PracticeConclude,
    Exposure,
    MisconceptionObserved,
    OperatorCorrection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceOutcome {
    Correct,
    Incorrect,
    DontKnow,
    Exposed,
    Observed,
    Corrected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MissClass {
    Cue,
    Discrimination,
    Execution,
    Careless,
    Structural,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PracticeOutcome {
    Solved,
    SolvedWithHints,
    MovedOn,
    Surrendered,
    Dismissed,
}

/// One append-only row of the learner's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Evidence {
    pub id: u64,
    pub concept: ConceptId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    pub at: Timestamp,
    pub kind: EvidenceKind,
    pub outcome: EvidenceOutcome,
    /// One of [`CONFIDENCE_LEVELS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ProblemMode>,
    /// 0 to [`MAX_HINT_RUNG`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint_rung: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub miss_class: Option<MissClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What the frontend gathered from reads the user can already make, stored
/// with the session that starts from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EvidenceBundle {
    pub title: String,
    pub items: Vec<EvidenceItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EvidenceItem {
    #[serde(rename = "ref")]
    pub reference: EvidenceRef,
    pub excerpt: String,
    pub status: EvidenceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EvidenceRef {
    pub kind: EvidenceRefKind,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRefKind {
    Pr,
    Issue,
    Item,
    Run,
    Stage,
    Commit,
    File,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Verified,
    Reviewed,
    Preliminary,
}

/// The `[noesis]` config table (§6.1) as one JSON object of the same shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NoesisSettingsView {
    pub enabled: bool,
    pub engineering_context: bool,
    pub extraction: ExtractionSettings,
    pub budget: BudgetSettings,
    pub concurrency: ConcurrencySettings,
    pub priority: PrioritySettings,
    pub models: ModelSettings,
    pub billing: BillingSettings,
    pub disable: DisableSettings,
    pub prompts: PromptSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExtractionSettings {
    pub sessions: SessionExtraction,
    pub engineering: EngineeringExtraction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionExtraction {
    Manual,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EngineeringExtraction {
    Off,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BudgetSettings {
    pub interactive_usd_daily: f64,
    pub background_usd_daily: f64,
    pub background_tokens_daily: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConcurrencySettings {
    pub interactive: u8,
    pub background: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PrioritySettings {
    pub background: BackgroundPriority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundPriority {
    Low,
    Normal,
}

/// Unset means the first ready subscription model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModelSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strong: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cheap: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BillingSettings {
    pub allow_api: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DisableSettings {
    pub grace_seconds: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PromptSettings {
    pub overrides: bool,
}

impl Default for NoesisSettingsView {
    /// The defaults of §6.1.
    fn default() -> Self {
        Self {
            enabled: false,
            engineering_context: false,
            extraction: ExtractionSettings {
                sessions: SessionExtraction::Manual,
                engineering: EngineeringExtraction::Off,
            },
            budget: BudgetSettings {
                interactive_usd_daily: 5.0,
                background_usd_daily: 1.0,
                background_tokens_daily: 2_000_000,
            },
            concurrency: ConcurrencySettings {
                interactive: 2,
                background: 1,
            },
            priority: PrioritySettings {
                background: BackgroundPriority::Low,
            },
            models: ModelSettings {
                strong: None,
                cheap: None,
            },
            billing: BillingSettings { allow_api: false },
            disable: DisableSettings { grace_seconds: 60 },
            prompts: PromptSettings { overrides: false },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_the_documented_confidence_levels_are_valid() {
        for level in CONFIDENCE_LEVELS {
            assert_eq!(validate_confidence(level), Ok(()));
        }
        for level in [1, 29, 31, 80, 99, 101, 255] {
            assert!(validate_confidence(level).is_err(), "{level}");
        }
    }

    #[test]
    fn an_answer_key_is_a_string_or_a_list_of_strings() {
        let many: AnswerKey = serde_json::from_value(json!(["a", "b"])).unwrap();
        assert_eq!(many, AnswerKey::Many(vec!["a".into(), "b".into()]));
        let one: AnswerKey = serde_json::from_value(json!("a")).unwrap();
        assert_eq!(one, AnswerKey::One("a".into()));
        assert_eq!(serde_json::to_value(&one).unwrap(), json!("a"));
    }

    #[test]
    fn the_settings_view_reads_the_shape_of_the_toml_table() {
        let wire = json!({
            "enabled": true, "engineering_context": false,
            "extraction": {"sessions": "auto", "engineering": "off"},
            "budget": {"interactive_usd_daily": 5.0, "background_usd_daily": 1.0,
                       "background_tokens_daily": 2_000_000},
            "concurrency": {"interactive": 2, "background": 0},
            "priority": {"background": "normal"},
            "models": {"strong": "claude-opus"},
            "billing": {"allow_api": false},
            "disable": {"grace_seconds": 60},
            "prompts": {"overrides": true},
        });
        let view: NoesisSettingsView = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(view.extraction.sessions, SessionExtraction::Auto);
        assert_eq!(view.models.strong.as_deref(), Some("claude-opus"));
        assert_eq!(view.models.cheap, None);
        assert_eq!(serde_json::to_value(&view).unwrap(), wire);
    }

    #[test]
    fn the_default_settings_are_the_documented_ones() {
        let wire = serde_json::to_value(NoesisSettingsView::default()).unwrap();
        assert_eq!(wire["enabled"], false);
        assert_eq!(wire["budget"]["background_tokens_daily"], 2_000_000);
        assert_eq!(
            wire["concurrency"],
            json!({"interactive": 2, "background": 1})
        );
        assert_eq!(wire["priority"]["background"], "low");
        assert_eq!(wire["disable"]["grace_seconds"], 60);
        assert!(wire["models"].as_object().unwrap().is_empty());
    }

    #[test]
    fn a_concept_and_an_evidence_bundle_name_their_reference_ref_on_the_wire() {
        let concept = ConceptRef {
            id: "k_1".into(),
            kind: ConceptKind::PlanNode,
            reference: "intro".into(),
            title: "Intro".into(),
        };
        assert_eq!(
            serde_json::to_value(&concept).unwrap(),
            json!({"id": "k_1", "kind": "plan_node", "ref": "intro", "title": "Intro"})
        );
        let bundle: EvidenceBundle = serde_json::from_value(json!({
            "title": "t",
            "items": [{"ref": {"kind": "pr", "id": "12"}, "excerpt": "e", "status": "verified"}]
        }))
        .unwrap();
        assert_eq!(bundle.items[0].reference.kind, EvidenceRefKind::Pr);
    }

    #[test]
    fn terminal_job_states_are_the_three_that_end() {
        let ended = [JobState::Succeeded, JobState::Failed, JobState::Cancelled];
        let live = [JobState::Queued, JobState::Running, JobState::Merging];
        assert!(ended.iter().all(|state| state.is_terminal()));
        assert!(live.iter().all(|state| !state.is_terminal()));
    }
}
