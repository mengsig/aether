//! Question authoring (T5), the option shuffle and local grading (T6) and
//! the reveal rule (T7). Pure functions over arguments and answers.

use std::collections::BTreeSet;
use std::fmt;

use super::plan::PlanDag;
use super::records::{
    Answer, CONFIDENCE_STOPS, ChoiceOption, DEFAULT_CONFIDENCE, EvidenceKind, Intent, Phase,
    QuestionKind, QuestionStep,
};
use super::seal::Sealer;
use crate::tools::args::AskArgs;

pub const MAX_OPTIONS: usize = 12;
pub const MAX_ANSWER_TEXT: usize = 20_000;

/// A question that passed authoring, ready to store.
#[derive(Clone, PartialEq, Eq)]
pub struct Authored {
    pub options: Vec<ChoiceOption>,
    pub answer_key: Vec<String>,
    pub multi: bool,
}

impl fmt::Debug for Authored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Authored")
            .field("options", &self.options)
            .field("answer_key", &crate::tools::SEALED)
            .field("multi", &self.multi)
            .finish()
    }
}

/// Check an `noesis.ask` against the plan and the visible text of the turn
/// that poses it, reporting every problem. Problems never quote a key.
pub fn author(
    args: &AskArgs,
    plan: Option<&PlanDag>,
    visible_text: &str,
) -> Result<Authored, Vec<String>> {
    let mut problems = Vec::new();
    if let Some(node) = &args.plan_node {
        match plan {
            None => problems.push("`plan_node` needs a plan; call noesis.plan first".to_string()),
            Some(plan) if !plan.has_node(node) => {
                problems.push("`plan_node` is not a node of the current plan".to_string())
            }
            Some(_) => {}
        }
    }
    let authored = match args.kind {
        QuestionKind::Choice => author_choice(args, &mut problems),
        QuestionKind::Recall => author_recall(args, &mut problems),
        QuestionKind::Goal => author_goal(args, &mut problems),
    };
    if let Some(overlap) = shown_fields_overlap(args) {
        let detail = if args.plan_node.is_some() {
            format!(": {overlap}")
        } else {
            String::new()
        };
        problems.push(format!("the prompt, an option, a concept or the plan node gives away the answer or its explanation{detail}; rewrite the overlapping field or ask a different question"));
    }
    if let Some(key) = key_overlap(args, visible_text) {
        let detail = if args.plan_node.is_some() {
            format!(" (answer_key[{key}])")
        } else {
            String::new()
        };
        problems.push(format!("what your turn has shown gives away the answer{detail}; ask a different question with an answer not already shown"));
    }
    let explanation = Sealer::new(args.explanation.iter());
    if explanation.holds(visible_text) {
        let detail = if args.plan_node.is_some() {
            " (explanation)".to_string()
        } else {
            String::new()
        };
        problems.push(format!(
            "what your turn has shown gives away the explanation{detail}; use an explanation not already shown"
        ));
    }
    if problems.is_empty() {
        Ok(authored)
    } else {
        Err(problems)
    }
}

fn author_choice(args: &AskArgs, problems: &mut Vec<String>) -> Authored {
    let options: Vec<ChoiceOption> = args
        .options
        .iter()
        .map(|option| ChoiceOption {
            value: option.value.clone(),
            label: option.label.clone(),
        })
        .collect();
    if options.len() < 2 || options.len() > MAX_OPTIONS {
        problems.push(format!("a choice question has 2 to {MAX_OPTIONS} options"));
    }
    let values: BTreeSet<&str> = options.iter().map(|o| o.value.as_str()).collect();
    if values.len() != options.len() || values.contains("") {
        problems.push("option values must be distinct and non-empty".to_string());
    }
    let labels: BTreeSet<&str> = options.iter().map(|o| o.label.as_str()).collect();
    if labels.len() != options.len() || labels.contains("") {
        problems.push("option labels must be distinct and non-empty".to_string());
    }
    let key_labels: BTreeSet<&str> = args.answer_key.iter().map(String::as_str).collect();
    if key_labels.is_empty() {
        problems.push("a choice question needs an `answer_key` of exact option labels".to_string());
    }
    if !key_labels.iter().all(|label| labels.contains(label)) {
        problems.push("each `answer_key` entry must be the exact label of a correct option, not its value or position; update it whenever options change".to_string());
    }
    let key: Vec<String> = options
        .iter()
        .filter(|option| key_labels.contains(option.label.as_str()))
        .map(|option| option.value.clone())
        .collect();
    if prose_holds_a_value(args, &values) {
        problems.push(
            "option values are internal ids and never appear in the prompt or a label; use opaque values such as `opt1`"
                .to_string(),
        );
    }
    let multi = args.multi.unwrap_or(key_labels.len() > 1);
    if !multi && key_labels.len() > 1 {
        problems.push(
            "a single-answer question has exactly one `answer_key` entry; set `multi`".to_string(),
        );
    }
    Authored {
        options,
        answer_key: key,
        multi,
    }
}

fn author_recall(args: &AskArgs, problems: &mut Vec<String>) -> Authored {
    if !args.options.is_empty() {
        problems.push("a recall question has no options".to_string());
    }
    if args.answer_key.len() != 1 || args.answer_key[0].trim().is_empty() {
        problems.push("a recall question needs one committed `answer_key` text".to_string());
    }
    Authored {
        options: Vec::new(),
        answer_key: args.answer_key.clone(),
        multi: false,
    }
}

fn author_goal(args: &AskArgs, problems: &mut Vec<String>) -> Authored {
    if !args.answer_key.is_empty() {
        problems.push("a goal question has no `answer_key`".to_string());
    }
    if args.plan_node.is_some() {
        problems
            .push("a goal question must omit `plan_node`; remove that field and retry".to_string());
    }
    let options = args
        .options
        .iter()
        .map(|option| ChoiceOption {
            value: option.value.clone(),
            label: option.label.clone(),
        })
        .collect();
    Authored {
        options,
        answer_key: Vec::new(),
        multi: args.multi.unwrap_or(false),
    }
}

/// Whether the prompt or a label holds an option value. Readers see re-keyed
/// values only, so a value they could tie to an option would let text that
/// names it point at the key.
fn prose_holds_a_value(args: &AskArgs, values: &BTreeSet<&str>) -> bool {
    debug_assert_eq!(args.kind, QuestionKind::Choice, "goal options have no key");
    let values = Sealer::new(values.iter());
    let mut prose = vec![args.prompt.as_str()];
    prose.extend(args.options.iter().map(|option| option.label.as_str()));
    prose.iter().any(|text| values.holds(text))
}

/// The model's own values of a choice's correct options; posing re-keys
/// them, so a plan that shows one would point at the key.
pub(super) fn model_key_values(args: &AskArgs) -> Vec<String> {
    if args.kind != QuestionKind::Choice {
        return Vec::new();
    }
    args.options
        .iter()
        .filter(|option| args.answer_key.contains(&option.label))
        .map(|option| option.value.clone())
        .collect()
}

/// Whether a field the learner sees repeats a sealed text: the explanation
/// anywhere, a recall key in the prompt or a label, any key in a tag.
pub(super) fn shown_fields_reveal(args: &AskArgs) -> bool {
    shown_fields_overlap(args).is_some()
}

fn shown_fields_overlap(args: &AskArgs) -> Option<String> {
    if let Some(found) = field_secret_overlap("prompt", &args.prompt, args, true) {
        return Some(found);
    }
    for (index, option) in args.options.iter().enumerate() {
        let label = format!("options[{index}].label");
        let value = format!("options[{index}].value");
        if let Some(found) = field_secret_overlap(
            &label,
            &option.label,
            args,
            args.kind != QuestionKind::Choice,
        )
        .or_else(|| field_secret_overlap(&value, &option.value, args, false))
        {
            return Some(found);
        }
    }
    for (index, concept) in args.concepts.iter().enumerate() {
        if let Some(found) =
            field_secret_overlap(&format!("concepts[{index}]"), concept, args, true)
        {
            return Some(found);
        }
    }
    args.plan_node
        .as_deref()
        .and_then(|node| field_secret_overlap("plan_node", node, args, true))
}

fn field_secret_overlap(
    field: &str,
    text: &str,
    args: &AskArgs,
    check_key: bool,
) -> Option<String> {
    if check_key {
        for (index, key) in args.answer_key.iter().enumerate() {
            if args.kind == QuestionKind::Choice
                && !args.options.iter().any(|option| &option.label == key)
            {
                continue;
            }
            if Sealer::new([key]).holds(text) {
                return Some(format!("{field} overlaps answer_key[{index}]"));
            }
        }
    }
    Sealer::new(args.explanation.iter())
        .holds(text)
        .then(|| format!("{field} overlaps explanation"))
}

/// Whether the posing turn's visible text holds a correct option's label or
/// the recall key, as a whole phrase and ignoring case.
pub fn reveals_key(args: &AskArgs, visible_text: &str) -> bool {
    key_overlap(args, visible_text).is_some()
}

fn key_overlap(args: &AskArgs, visible_text: &str) -> Option<usize> {
    if args.kind == QuestionKind::Goal {
        return None;
    }
    args.answer_key
        .iter()
        .position(|secret| Sealer::new([secret]).holds(visible_text))
}

/// A stored question's key as the learner would recognise it: a choice key
/// as its options' labels, any other key as written.
pub(super) fn shown_key(row: &super::db::QuestionRow) -> Vec<String> {
    let options = row.question.options.as_deref().unwrap_or_default();
    if row.question.kind != QuestionKind::Choice {
        return row.answer_key.clone();
    }
    let labels: Vec<String> = options
        .iter()
        .filter(|option| row.answer_key.contains(&option.value))
        .map(|option| option.label.clone())
        .collect();
    debug_assert_eq!(
        labels.len(),
        row.answer_key.len(),
        "a stored key names options"
    );
    labels
}

/// The display order of a question's options: a Fisher-Yates shuffle seeded
/// by the question id, so every read and every restart shows the same order.
pub fn shuffle(question_id: &str, options: &[ChoiceOption]) -> Vec<ChoiceOption> {
    let mut state = fnv1a(question_id.as_bytes());
    let mut shuffled = options.to_vec();
    for index in (1..shuffled.len()).rev() {
        state = splitmix64(state);
        let pick = (state % (index as u64 + 1)) as usize;
        shuffled.swap(index, pick);
    }
    debug_assert_eq!(shuffled.len(), options.len());
    shuffled
}

/// Options as the learner sees them: each value replaced by its display
/// position ("1", "2", ...), with `key` mapped to match. The model's own
/// values never reach a reader, so no text it writes can name the key by one.
pub fn rekey(shuffled: &[ChoiceOption], key: &[String]) -> (Vec<ChoiceOption>, Vec<String>) {
    let mut shown = Vec::with_capacity(shuffled.len());
    let mut shown_key = Vec::with_capacity(key.len());
    for (index, option) in shuffled.iter().enumerate() {
        let value = (index + 1).to_string();
        if key.contains(&option.value) {
            shown_key.push(value.clone());
        }
        shown.push(ChoiceOption {
            value,
            label: option.label.clone(),
        });
    }
    assert_eq!(
        shown_key.len(),
        key.len(),
        "every key entry names one option"
    );
    debug_assert_eq!(shown.len(), shuffled.len());
    (shown, shown_key)
}

/// The labels of the options a learner chose, for the agent, which knows
/// its options by label once they are re-keyed.
pub fn chosen_labels(options: &[ChoiceOption], choices: &[String]) -> Vec<String> {
    let labels: Vec<String> = options
        .iter()
        .filter(|option| choices.contains(&option.value))
        .map(|option| option.label.clone())
        .collect();
    debug_assert!(
        labels.len() <= choices.len(),
        "choices are distinct options"
    );
    labels
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn splitmix64(state: u64) -> u64 {
    let mut z = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A checked answer: its stated confidence and, for a choice question, the
/// local grade. "Don't know" has neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Graded {
    pub confidence: Option<u8>,
    pub correct: Option<bool>,
}

/// The facts of an open question that grading needs.
pub struct Posed<'a> {
    pub kind: QuestionKind,
    pub multi: bool,
    pub options: &'a [ChoiceOption],
    pub answer_key: &'a [String],
}

impl fmt::Debug for Posed<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Posed")
            .field("kind", &self.kind)
            .field("multi", &self.multi)
            .field("options", &self.options)
            .field("answer_key", &crate::tools::SEALED)
            .finish()
    }
}

/// Validate a learner's answer and grade a choice answer as an exact set.
/// Recall answers go back to the agent to judge.
pub fn grade(posed: &Posed<'_>, answer: &Answer) -> Result<Graded, String> {
    let confidence = check_confidence(answer)?;
    let too_long = |text: &Option<String>| {
        text.as_ref()
            .is_some_and(|t| t.chars().count() > MAX_ANSWER_TEXT)
    };
    if too_long(&answer.text) || too_long(&answer.thinking) {
        return Err(format!(
            "answer text and thinking hold at most {MAX_ANSWER_TEXT} characters"
        ));
    }
    if answer.dont_know {
        if !answer.choices.is_empty() || answer.text.is_some() {
            return Err("a \"don't know\" answer carries no choice or text".to_string());
        }
        return Ok(Graded {
            confidence: None,
            correct: None,
        });
    }
    let has_text = answer.text.as_ref().is_some_and(|t| !t.trim().is_empty());
    match posed.kind {
        QuestionKind::Choice => grade_choice(posed, answer, confidence),
        QuestionKind::Recall if has_text => Ok(Graded {
            confidence,
            correct: None,
        }),
        QuestionKind::Recall => Err("a recall answer needs text, or \"don't know\"".to_string()),
        QuestionKind::Goal if has_text || !answer.choices.is_empty() => Ok(Graded {
            confidence,
            correct: None,
        }),
        QuestionKind::Goal => Err("a goal answer needs text or a choice".to_string()),
    }
}

fn check_confidence(answer: &Answer) -> Result<Option<u8>, String> {
    match (answer.dont_know, answer.confidence) {
        (true, Some(_)) => Err("a \"don't know\" answer carries no confidence".to_string()),
        (true, None) => Ok(None),
        (false, Some(stop)) if CONFIDENCE_STOPS.contains(&stop) => Ok(Some(stop)),
        (false, Some(_)) => Err(format!("confidence is one of {CONFIDENCE_STOPS:?}")),
        (false, None) => Ok(Some(DEFAULT_CONFIDENCE)),
    }
}

fn grade_choice(
    posed: &Posed<'_>,
    answer: &Answer,
    confidence: Option<u8>,
) -> Result<Graded, String> {
    let chosen: BTreeSet<&str> = answer.choices.iter().map(String::as_str).collect();
    if chosen.len() != answer.choices.len() {
        return Err("each choice is given once".to_string());
    }
    if chosen.is_empty() || (!posed.multi && chosen.len() != 1) {
        let wanted = if posed.multi {
            "one or more choices"
        } else {
            "exactly one choice"
        };
        return Err(format!("this question takes {wanted}, or \"don't know\""));
    }
    if !chosen
        .iter()
        .all(|value| posed.options.iter().any(|o| o.value == *value))
    {
        return Err("every choice must be one of the question's option values".to_string());
    }
    let key: BTreeSet<&str> = posed.answer_key.iter().map(String::as_str).collect();
    debug_assert!(!key.is_empty(), "a posed choice question has a key");
    Ok(Graded {
        confidence,
        correct: Some(chosen == key),
    })
}

/// T7: teach-phase checks are collapsed once answered; predictions, probes
/// and goal questions never are.
pub fn collapsed(phase: Phase, kind: QuestionKind, intent: Option<Intent>) -> bool {
    let teaching = matches!(phase, Phase::Plan | Phase::Teach);
    teaching && kind != QuestionKind::Goal && intent.unwrap_or(Intent::Check) == Intent::Check
}

/// The lesson step a question posed in `phase` is, as its evidence says.
pub fn step(phase: Phase, kind: QuestionKind, collapsed: bool) -> QuestionStep {
    match (kind, phase) {
        (QuestionKind::Goal, _) => QuestionStep::Goal,
        (_, Phase::Probe) => QuestionStep::Measure,
        (_, Phase::Practice) => QuestionStep::Practice,
        _ if collapsed => QuestionStep::Check,
        _ => QuestionStep::Predict,
    }
}

/// What an answer is evidence of; goal questions are not about knowledge.
pub fn evidence_kind(phase: Phase, kind: QuestionKind, collapsed: bool) -> Option<EvidenceKind> {
    match (kind, phase) {
        (QuestionKind::Goal, _) => None,
        (_, Phase::Probe) => Some(EvidenceKind::ProbeAnswer),
        (_, Phase::Practice) => None,
        _ if collapsed => Some(EvidenceKind::CheckAnswer),
        _ => Some(EvidenceKind::PredictAnswer),
    }
}
