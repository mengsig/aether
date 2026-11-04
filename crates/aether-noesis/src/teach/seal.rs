//! The seal boundary. A [`Sealer`] owns one set of secrets (recall keys,
//! practice solutions, explanations, correct option values and labels), and
//! every generic projection the loop publishes goes through it.

use std::fmt;

use serde_json::{Value, json};

use super::engine::Step;
use super::records::{NoesisEvent, PlanTree};
use super::service::{CallRecord, StepRecord};
use crate::tools::{self, SEALED};

/// What a step's visible text reads as once it gave away a sealed answer.
pub const WITHHELD: &str = "<withheld: this message gave away a sealed answer>";

/// What a refusal reads as once its reason would repeat a sealed text.
pub const REFUSAL_WITHHELD: &str =
    "this call was refused; its reason is withheld because it repeats sealed text";

/// A set of secrets and the one matching rule every projection uses: a
/// secret matches as a whole phrase, ignoring case and whitespace runs, and a
/// number still matches beside a unit or punctuation ("42kg", "42.").
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Sealer {
    secrets: Vec<String>,
}

/// Debug shows only how many secrets there are, never one of them.
impl fmt::Debug for Sealer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sealer")
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

impl Sealer {
    pub fn new<S: AsRef<str>>(secrets: impl IntoIterator<Item = S>) -> Self {
        let mut normalised: Vec<String> = Vec::new();
        for secret in secrets {
            let secret = matchable(normalise(secret.as_ref()));
            if !secret.is_empty() && !normalised.contains(&secret) {
                normalised.push(secret);
            }
        }
        debug_assert!(normalised.iter().all(|s| s.trim() == s && !s.is_empty()));
        debug_assert!(normalised.iter().all(|s| {
            let alnum = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
            !s.chars().any(char::is_alphanumeric)
                || (alnum(s.chars().next()) && alnum(s.chars().next_back()))
        }));
        Self {
            secrets: normalised,
        }
    }

    /// Whether `text` holds any secret.
    pub fn holds(&self, text: &str) -> bool {
        if self.secrets.is_empty() {
            return false;
        }
        let haystack = normalise(text);
        self.secrets
            .iter()
            .any(|secret| contains_phrase(&haystack, secret))
    }

    /// `text` itself, or [`SEALED`] when it holds a secret.
    pub fn text(&self, text: &str) -> String {
        if self.holds(text) {
            SEALED.to_string()
        } else {
            text.to_string()
        }
    }

    /// Replace `value` whole by [`SEALED`] when any part of it holds a secret.
    /// Masking only the matching parts would show where a secret sits, which
    /// among visible alternatives (a choice's options) names it.
    pub fn value(&self, value: &mut Value) {
        if self.reveals(value) {
            *value = Value::String(SEALED.into());
        }
        debug_assert!(!self.reveals(value) || value.as_str() == Some(SEALED));
    }

    /// A refusal quotes the model's own names and ids, so one that holds a
    /// secret is replaced whole by [`REFUSAL_WITHHELD`].
    pub fn refusal(&self, content: &mut Value) {
        debug_assert!(
            content.get("error").is_some(),
            "only refusals are sealed whole"
        );
        if self.reveals(content) {
            *content = json!({ "error": REFUSAL_WITHHELD });
        }
        debug_assert!(content.is_object());
    }

    /// A step's visible text as it may be published.
    pub fn step_text(&self, text: &str) -> String {
        if self.holds(text) {
            WITHHELD.to_string()
        } else {
            text.to_string()
        }
    }

    /// A tool call as it may be published: a sealing tool's input withheld
    /// whole, every other part withheld where it holds a secret.
    pub fn call(&self, call: &mut CallRecord) {
        call.input = tools::seal_input(&call.name, &call.input);
        self.value(&mut call.input);
        call.name = self.text(&call.name);
        if call.is_error {
            self.refusal(&mut call.result);
        } else {
            self.value(&mut call.result);
        }
        debug_assert!(!self.reveals(&call.input) || call.input.as_str() == Some(SEALED));
        debug_assert!(call.name == SEALED || !self.holds(&call.name));
    }

    pub fn step(&self, step: &mut StepRecord) {
        step.text = self.step_text(&step.text);
        for call in &mut step.calls {
            self.call(call);
        }
        debug_assert!(step.text == WITHHELD || !self.holds(&step.text));
    }

    /// An event as it may be published. A pose's own view is not sealed here:
    /// it necessarily shows its options, and posing refuses a view that
    /// would give its key away. A conclusion reveals its solution by design.
    pub fn event(&self, event: &mut NoesisEvent) {
        match event {
            NoesisEvent::PlanUpdated { plan, .. } => self.plan(plan),
            NoesisEvent::SessionCreated { session } => {
                if let Some(plan) = session.plan.as_mut() {
                    self.plan(plan);
                }
            }
            NoesisEvent::PhaseChanged { .. }
            | NoesisEvent::QuestionPosed { .. }
            | NoesisEvent::AnswerRecorded { .. }
            | NoesisEvent::ProblemPosed { .. }
            | NoesisEvent::PracticeConcluded { .. } => {}
        }
    }

    /// A plan as it may be published: a title holding a secret is sealed and
    /// a node id holding one is renamed everywhere it appears.
    pub fn plan(&self, plan: &mut PlanTree) {
        let node_count = plan.nodes.len();
        for (index, node) in plan.nodes.iter_mut().enumerate() {
            node.title = self.text(&node.title);
            if !self.holds(&node.id) {
                continue;
            }
            let renamed = format!("sealed-{}", index + 1);
            for edge in &mut plan.edges {
                for end in [&mut edge.from, &mut edge.to] {
                    if *end == node.id {
                        *end = renamed.clone();
                    }
                }
            }
            node.id = renamed;
        }
        debug_assert_eq!(plan.nodes.len(), node_count);
        debug_assert!(
            plan.edges
                .iter()
                .all(|e| !self.holds(&e.from) && !self.holds(&e.to))
        );
    }

    /// Whether any scalar or key in `value` holds a secret; the parser reads a
    /// number, boolean or null as its text and text holding JSON as that JSON,
    /// so this does too.
    fn reveals(&self, value: &Value) -> bool {
        match value {
            Value::String(text) => {
                self.holds(text) || embedded_json(text).is_some_and(|v| self.reveals(&v))
            }
            Value::Array(items) => items.iter().any(|item| self.reveals(item)),
            Value::Object(map) => map
                .iter()
                .any(|(key, item)| self.holds(key) || self.reveals(item)),
            scalar => self.holds(&scalar.to_string()),
        }
    }
}

/// Lowercase with every whitespace run collapsed to one space, trimmed.
fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.extend(word.chars().flat_map(char::to_lowercase));
    }
    out
}

/// A normalised secret without its edge punctuation, so "x = 4." still
/// matches "x = 4"; one with no letter or digit stays literal.
fn matchable(secret: String) -> String {
    let trimmed = secret.trim_matches(|c: char| !c.is_alphanumeric());
    if trimmed.is_empty() {
        return secret;
    }
    debug_assert!(secret.contains(trimmed));
    trimmed.to_string()
}

/// Whether `phrase` stands in `haystack` as a whole phrase. A neighbour
/// rejects a match only when it continues the phrase's edge run of letters
/// or digits: "42" stands in "42kg" but not in "420".
fn contains_phrase(haystack: &str, phrase: &str) -> bool {
    if phrase.is_empty() {
        return false;
    }
    let class = |c: char| (c.is_alphabetic(), c.is_numeric());
    let continues = |edge: Option<char>, next: Option<char>| match (edge, next) {
        (Some(edge), Some(next)) => edge.is_alphanumeric() && class(edge) == class(next),
        _ => false,
    };
    let first_of_phrase = phrase.chars().next();
    let last_of_phrase = phrase.chars().next_back();
    // Matches may overlap ("had had" in "chad had had"), so a rejected match
    // resumes one character later rather than after the match.
    let mut from = 0;
    while let Some(found) = haystack[from..].find(phrase) {
        let start = from + found;
        let before = haystack[..start].chars().next_back();
        let after = haystack[start + phrase.len()..].chars().next();
        if !continues(first_of_phrase, before) && !continues(last_of_phrase, after) {
            return true;
        }
        let first = haystack[start..].chars().next();
        debug_assert!(
            first.is_some(),
            "a match of a non-empty phrase has a first character"
        );
        from = start + first.map_or(1, char::len_utf8);
    }
    false
}

/// The JSON object or array `text` holds, as the argument healer parses it;
/// its escapes (`\u03c0`) decode, so a secret can hide in them. Each nested
/// level is strictly shorter text, so the recursion ends.
fn embedded_json(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if !(trimmed.starts_with('[') || trimmed.starts_with('{')) {
        return None;
    }
    let parsed: Value = serde_json::from_str(trimmed).ok()?;
    (parsed.is_object() || parsed.is_array()).then_some(parsed)
}

/// The sealed texts the calls of a step carry, and whether any sealing
/// call's input was unreadable (its secrets unknown, so the step fails closed).
pub(super) fn step_secrets(step: &Step) -> (Vec<String>, bool) {
    let mut secrets = Vec::new();
    let mut unreadable = false;
    for call in &step.calls {
        match tools::sealed_texts(&call.name, &call.input) {
            Some(found) => secrets.extend(found),
            None => unreadable = true,
        }
    }
    debug_assert!(secrets.iter().all(|secret| !secret.trim().is_empty()));
    (secrets, unreadable)
}

#[cfg(test)]
mod tests {
    use super::super::records::{NodeStatus, PlanEdge, PlanNode};
    use super::*;

    fn sealer(secrets: &[&str]) -> Sealer {
        Sealer::new(secrets.iter().copied())
    }

    #[test]
    fn phrases_match_whole_words_ignoring_case_and_whitespace() {
        let paris = sealer(&["Paris"]);
        assert!(paris.holds("It is PARIS, I think"));
        assert!(!paris.holds("comparison"));
        assert!(sealer(&["x  =\t4"]).holds("so X = 4."));
        assert!(sealer(&["x = 4"]).holds("so x\n=   4"));
        assert!(!sealer(&["x = 4"]).holds("x = 45"));
        assert!(sealer(&["had had"]).holds("chad had had enough"));
        assert!(!sealer(&["had"]).holds("chad hadhad"));
        assert!(sealer(&["é"]).holds("ééa é"));
        assert!(!sealer(&["", "  "]).holds("anything"));
        assert_eq!(sealer(&[]), Sealer::default());
        assert!(!Sealer::default().holds("anything"));
    }

    #[test]
    fn a_number_matches_beside_a_unit_or_punctuation_but_not_inside_a_number() {
        let answer = sealer(&["42"]);
        for shown in ["42", "42kg", "42 kg", "42.", "(42)", "it is 42!"] {
            assert!(answer.holds(shown), "{shown}");
        }
        for hidden in ["420", "1420", "convert 42000 grams", "4.2"] {
            assert!(!answer.holds(hidden), "{hidden}");
        }
        assert!(sealer(&["3.5"]).holds("about 3.5"));
        assert!(sealer(&["4"]).holds("so 4x it is"));
    }

    #[test]
    fn a_secret_with_edge_punctuation_matches_without_it() {
        assert!(sealer(&["x = 4."]).holds("so x = 4"));
        assert!(sealer(&["Paris."]).holds("it is Paris"));
        assert!(sealer(&["42."]).holds("it is 42 kg"));
        assert!(sealer(&["\"(Paris)\""]).holds("PARIS"));
        assert!(!sealer(&["x = 4."]).holds("x = 45"));
        assert!(sealer(&["?!"]).holds("what ?! no"));
        assert!(!sealer(&["?!"]).holds("what? no"));
    }

    #[test]
    fn a_value_holding_a_secret_anywhere_is_withheld_whole() {
        let cases = [
            (
                json!({"a": "fine", "b": ["fine", {"c": "so X = 4."}]}),
                "x = 4",
            ),
            (json!({"query": [42], "other": 7}), "42"),
            (json!({"flag": true}), "true"),
            (json!({"query": null}), "null"),
            (json!({"query": "vocabulary", "sealed": true}), "sealed"),
            (json!({"opt1": "Paris"}), "opt1"),
        ];
        for (mut value, secret) in cases {
            sealer(&[secret]).value(&mut value);
            assert_eq!(value, json!(SEALED), "{secret}");
        }
        let escaped = Value::String(r#"{"query":"\u03c0"}"#.into());
        let nested = json!({"nodes": r#"[{"title": "[\"\\u03c0\"]"}]"#});
        for mut value in [escaped, nested] {
            sealer(&["π"]).value(&mut value);
            assert_eq!(value, json!(SEALED));
        }
        let mut kept = json!({"query": "x = 5", "n": 4});
        sealer(&["x = 4"]).value(&mut kept);
        assert_eq!(kept, json!({"query": "x = 5", "n": 4}));
    }

    #[test]
    fn a_refusal_holding_a_secret_is_withheld_whole() {
        let secrets = sealer(&["mitochondria"]);
        let mut refusal = json!({"error": "node `mitochondria`: duplicate id"});
        secrets.refusal(&mut refusal);
        assert_eq!(refusal, json!({"error": REFUSAL_WITHHELD}));
        let mut kept = json!({"error": "node `cell-wall`: duplicate id"});
        secrets.refusal(&mut kept);
        assert_eq!(kept, json!({"error": "node `cell-wall`: duplicate id"}));
    }

    #[test]
    fn a_step_and_its_calls_are_sealed_together() {
        let secrets = sealer(&["Paris", "opt1"]);
        let mut step = StepRecord {
            text: "The answer is paris.".into(),
            calls: vec![
                CallRecord {
                    id: "call_1".into(),
                    name: "noesis.ask".into(),
                    input: json!({"prompt": "Capital?"}),
                    result: json!({"question": "q_1"}),
                    is_error: false,
                },
                CallRecord {
                    id: "call_2".into(),
                    name: "noesis.graph_search".into(),
                    input: json!({"query": "opt1"}),
                    result: json!({"error": "no `Paris`"}),
                    is_error: true,
                },
            ],
        };
        secrets.step(&mut step);
        assert_eq!(step.text, WITHHELD);
        assert_eq!(step.calls[0].input, json!(SEALED), "a sealing tool's input");
        assert_eq!(step.calls[0].result, json!({"question": "q_1"}));
        assert_eq!(step.calls[1].input, json!(SEALED));
        assert_eq!(step.calls[1].result, json!({"error": REFUSAL_WITHHELD}));
        assert_eq!(step.calls[1].id, "call_2", "public ids are opaque");
        assert!(
            !format!("{step:?}{secrets:?}")
                .to_lowercase()
                .contains("paris")
        );
    }

    #[test]
    fn a_plan_holding_a_secret_is_sealed_in_titles_ids_and_edges() {
        let node = |id: &str, title: &str| PlanNode {
            id: id.into(),
            title: title.into(),
            status: NodeStatus::Upcoming,
        };
        let mut plan = PlanTree {
            revision: 1,
            nodes: vec![
                node("intro", "Mitochondria basics"),
                node("mitochondria", "Two"),
            ],
            edges: vec![PlanEdge {
                from: "intro".into(),
                to: "mitochondria".into(),
            }],
        };
        sealer(&["mitochondria"]).plan(&mut plan);
        assert_eq!(plan.nodes[0].title, SEALED);
        assert_eq!(plan.nodes[0].id, "intro");
        assert_eq!(plan.nodes[1].id, "sealed-2");
        assert_eq!(plan.edges[0].to, "sealed-2");
        assert!(!format!("{plan:?}").to_lowercase().contains("mitochondria"));
    }
}
