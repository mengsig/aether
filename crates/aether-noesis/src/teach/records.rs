//! The Noesis vocabulary and the wire-shaped records of CONTRACTS.md §6.2.
//! Every record here is safe to show: none carries a sealed value before its
//! reveal, because none has a field for one.

use serde::Serialize;

/// A closed set of snake_case names, parsed and printed the same way.
macro_rules! string_enum {
    ($(#[$doc:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
        pub enum $name {
            $(#[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            pub const NAMES: &'static [&'static str] = &[$($text),+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            pub fn parse(text: &str) -> Option<Self> {
                match text {
                    $($text => Some($name::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

string_enum!(
    /// What a session is for.
    Mode { Teach = "teach", Practice = "practice" }
);

string_enum!(
    /// Where a session is. Stored on the session and changed only by the
    /// service; [`super::phase::fold`] rebuilds it from the events.
    Phase { Probe = "probe", Plan = "plan", Teach = "teach", Practice = "practice" }
);

string_enum!(QuestionKind {
    Choice = "choice",
    Recall = "recall",
    Goal = "goal"
});

string_enum!(
    /// Why the Teacher asks: a check after a step, or a prediction before it.
    Intent { Check = "check", Predict = "predict" }
);

string_enum!(PracticeMode {
    Recall = "recall",
    Discrimination = "discrimination",
    Composition = "composition"
});

string_enum!(Outcome {
    Solved = "solved",
    SolvedWithHints = "solved_with_hints",
    MovedOn = "moved_on",
    Surrendered = "surrendered",
    Dismissed = "dismissed"
});

string_enum!(MissClass {
    Cue = "cue",
    Discrimination = "discrimination",
    Execution = "execution",
    Careless = "careless",
    Structural = "structural"
});

string_enum!(NodeStatus {
    Upcoming = "upcoming",
    Current = "current",
    Visited = "visited"
});

string_enum!(
    /// The evidence kinds of NOESIS.md §5 that the teaching loop produces.
    EvidenceKind {
        ProbeAnswer = "probe_answer",
        PredictAnswer = "predict_answer",
        CheckAnswer = "check_answer",
        PracticeConclude = "practice_conclude",
        MisconceptionObserved = "misconception_observed"
    }
);

string_enum!(EvidenceOutcome {
    Correct = "correct",
    Incorrect = "incorrect",
    DontKnow = "dont_know",
    Observed = "observed"
});

/// The highest hint rung, R5: the full worked solution.
pub const MAX_RUNG: u8 = 5;

/// The confidence stops a learner can state; 50 when none is given.
pub const CONFIDENCE_STOPS: [u8; 6] = [0, 30, 50, 70, 90, 100];
pub const DEFAULT_CONFIDENCE: u8 = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ChoiceOption {
    pub value: String,
    pub label: String,
}

/// A posed question as every reader sees it. The answer key and the
/// explanation are sealed and have no field here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Question {
    pub id: String,
    pub kind: QuestionKind,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<ChoiceOption>>,
    pub multi: bool,
    pub collapsed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_node: Option<String>,
    pub concepts: Vec<String>,
    pub posed_at: String,
}

/// A recorded answer's reveal: the only place a question's key and
/// explanation appear, and only after the answer is recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuestionResult {
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correct: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer_key: Option<RevealedKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
}

/// `string[]` for a choice question, `string` for a recall question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum RevealedKey {
    Choices(Vec<String>),
    Text(String),
}

/// An active or concluded problem as every reader sees it; the solution is
/// sealed and revealed only by [`NoesisEvent::PracticeConcluded`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub id: String,
    pub statement: String,
    pub mode: PracticeMode,
    pub targets: Vec<String>,
    pub posed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanNode {
    pub id: String,
    pub title: String,
    pub status: NodeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
pub struct PlanEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanTree {
    pub revision: u32,
    pub nodes: Vec<PlanNode>,
    pub edges: Vec<PlanEdge>,
}

/// `noesis_session`: the session as every reader sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionView {
    pub id: String,
    pub goal: String,
    pub mode: Mode,
    pub phase: Phase,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanTree>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_question: Option<Question>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_problem: Option<Problem>,
    pub created_at: String,
    pub updated_at: String,
}

/// The durable `noesis.*` events the teaching loop emits, for F3 to publish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum NoesisEvent {
    SessionCreated {
        session: Box<SessionView>,
    },
    PhaseChanged {
        session: String,
        from: Phase,
        to: Phase,
    },
    QuestionPosed {
        session: String,
        question: Question,
    },
    AnswerRecorded {
        session: String,
        result: QuestionResult,
    },
    PlanUpdated {
        session: String,
        plan: PlanTree,
    },
    ProblemPosed {
        session: String,
        problem: Problem,
    },
    PracticeConcluded {
        session: String,
        problem: String,
        outcome: Outcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        rung: Option<u8>,
        #[serde(skip_serializing_if = "Option::is_none")]
        miss_class: Option<MissClass>,
        retest: bool,
        solution: String,
    },
}

impl NoesisEvent {
    /// The frozen event name.
    pub fn name(&self) -> &'static str {
        match self {
            NoesisEvent::SessionCreated { .. } => "noesis.session_created",
            NoesisEvent::PhaseChanged { .. } => "noesis.phase_changed",
            NoesisEvent::QuestionPosed { .. } => "noesis.question_posed",
            NoesisEvent::AnswerRecorded { .. } => "noesis.answer_recorded",
            NoesisEvent::PlanUpdated { .. } => "noesis.plan_updated",
            NoesisEvent::ProblemPosed { .. } => "noesis.problem_posed",
            NoesisEvent::PracticeConcluded { .. } => "noesis.practice_concluded",
        }
    }
}

/// One piece of learner evidence for the learner model (F4) to append.
/// `concepts` names what it bears on: the question's concepts, else its plan
/// node; empty means the session's goal topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceDraft {
    pub session: String,
    pub concepts: Vec<String>,
    pub kind: EvidenceKind,
    pub outcome: EvidenceOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<u8>,
    pub dont_know: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<PracticeMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint_rung: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub miss_class: Option<MissClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl EvidenceDraft {
    /// Whether this record may count toward mastery: a correct retrieval.
    /// "Don't know" is evidence of absence and never counts.
    pub fn supports_mastery(&self) -> bool {
        let retrieval = matches!(
            self.kind,
            EvidenceKind::ProbeAnswer
                | EvidenceKind::CheckAnswer
                | EvidenceKind::PredictAnswer
                | EvidenceKind::PracticeConclude
        );
        retrieval && !self.dont_know && self.outcome == EvidenceOutcome::Correct
    }
}

/// A learner's answer to the open question (`noesis_answer`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answer {
    pub choices: Vec<String>,
    pub text: Option<String>,
    pub dont_know: bool,
    pub confidence: Option<u8>,
    pub thinking: Option<String>,
}
