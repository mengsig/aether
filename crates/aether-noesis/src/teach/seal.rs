//! The seal boundary. A [`Sealer`] owns one set of secrets (recall keys,
//! practice solutions, explanations, correct option values and labels).
//!
//! It guards a teaching promise, not a security boundary: an honest learner
//! never sees an exercise's answer before trying it. One check,
//! [`reply_leaks`], runs over everything a model reply would keep before any
//! of it is published or persisted; a reply that fails it is discarded.

use std::fmt;

use serde_json::{Value, json};

use super::engine::{EngineError, Step, ToolCall};
use crate::budget::Spend;
use crate::tools::{self, ToolName};

/// What a refusal reads as once its reason would repeat a sealed text.
pub const REFUSAL_WITHHELD: &str =
    "this call was refused; its reason is withheld because it repeats sealed text";

/// What the agent is told before it writes a discarded reply again. It
/// names no secret, so it is safe to send as it stands.
pub const SEAL_REMINDER: &str = "Your previous reply was discarded before anyone saw it: it \
    gave away a sealed answer (a question's answer key or explanation, or a problem's \
    solution) before the learner tried. Write this step again without stating, hinting at \
    or repeating any of them.";

/// The step the learner sees when a reply gave a sealed answer away twice.
pub const NEUTRAL_REPLY: &str = "Let's try that differently. Send any reply to carry on.";

/// A set of secrets and the one matching rule every projection uses: a
/// secret matches as a whole phrase, ignoring case and whitespace runs, and a
/// number still matches beside a unit or punctuation ("42kg", "42.").
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Sealer {
    secrets: Vec<String>,
    /// The active problem's solution, among `secrets`: a reply that
    /// concludes the problem may show it.
    solution: Option<String>,
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
        let mut sealer = Self::default();
        sealer.extend(secrets);
        sealer
    }

    /// A session's live seal: `held`, every secret it holds unrevealed, of
    /// which `solution` is the active problem's.
    pub fn live<S: AsRef<str>>(held: impl IntoIterator<Item = S>, solution: Option<&str>) -> Self {
        let mut sealer = Self::new(held);
        sealer.solution = solution
            .map(|solution| matchable(normalise(solution)))
            .filter(|solution| !solution.is_empty());
        if let Some(solution) = sealer.solution.clone() {
            sealer.extend([solution]);
        }
        debug_assert!(sealer.solution.iter().all(|s| sealer.secrets.contains(s)));
        sealer
    }

    /// This seal for a reply that concludes the active problem, which
    /// reveals its solution by design.
    fn concluding(&self) -> Self {
        let mut sealer = self.clone();
        if let Some(solution) = sealer.solution.take() {
            sealer.secrets.retain(|secret| *secret != solution);
        }
        debug_assert!(sealer.solution.is_none());
        sealer
    }

    fn extend<S: AsRef<str>>(&mut self, secrets: impl IntoIterator<Item = S>) {
        for secret in secrets {
            let secret = matchable(normalise(secret.as_ref()));
            if !secret.is_empty() && !self.secrets.contains(&secret) {
                self.secrets.push(secret);
            }
        }
        debug_assert!(self.secrets.iter().all(|s| s.trim() == s && !s.is_empty()));
        debug_assert!(self.secrets.iter().all(|s| {
            let alnum = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
            !s.chars().any(char::is_alphanumeric)
                || (alnum(s.chars().next()) && alnum(s.chars().next_back()))
        }));
    }

    /// This seal joined by `secrets`.
    fn joined<S: AsRef<str>>(&self, secrets: impl IntoIterator<Item = S>) -> Self {
        let mut sealer = self.clone();
        sealer.extend(secrets);
        debug_assert!(sealer.secrets.len() >= self.secrets.len());
        sealer
    }

    /// Whether this seals nothing, so text may be shown as it is written.
    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
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

    /// Replace `value` whole by [`tools::SEALED`] when any part of it holds a
    /// secret. Masking only the matching parts would show where a secret
    /// sits, which among visible alternatives (a choice's options) names it.
    pub fn value(&self, value: &mut Value) {
        if self.reveals(value) {
            *value = Value::String(tools::SEALED.into());
        }
        debug_assert!(!self.reveals(value) || value.as_str() == Some(tools::SEALED));
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

    /// Whether any scalar or key in `value` holds a secret; the parser reads a
    /// number, boolean or null as its text and text holding JSON as that JSON,
    /// so this does too.
    pub fn reveals(&self, value: &Value) -> bool {
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

/// Whether a tool's input carries sealed fields: a question or a problem.
fn is_pose(tool: &str) -> bool {
    ToolName::parse(tool).is_some_and(|tool| !tool.sealed_fields().is_empty())
}

/// Whether `step`, a model reply, would give a sealed answer away if it were
/// kept. This is the one seal check. It reads all the reply keeps: its text,
/// run together and block by block, each call's name unless it is a Noesis
/// tool's own (the crate's fixed text, which says nothing of a key), and
/// every key and scalar of each input but a pose's sealed fields, a pose's
/// input read healed as it is parsed. It checks them against `live` and
/// every sealed text the reply's own calls carry. Call ids are the engine's
/// own, never the model's text.
///
/// A pose's own view necessarily shows its options, which name its key, so
/// it is checked without its own secrets: posing refuses a view that gives
/// its key away. A pose whose input is unreadable is refused, so its
/// secrets never become live. A reply that concludes the active problem may
/// show its solution. A failed attempt is neither shown nor kept.
pub fn reply_leaks(live: &Sealer, step: &Step) -> bool {
    let concludes = step
        .calls
        .iter()
        .any(|call| call.name == ToolName::Conclude.as_str());
    let live = if concludes {
        live.concluding()
    } else {
        live.clone()
    };
    let own = |skip: Option<usize>| {
        let calls = step.calls.iter().enumerate();
        let carried = calls.filter(move |(index, _)| Some(*index) != skip);
        carried.flat_map(|(_, call)| call_secrets(call))
    };
    debug_assert!(
        step.text_blocks.is_empty() || step.text_blocks.concat() == step.text,
        "the blocks are the text, block by block"
    );
    let all = live.joined(own(None));
    let blocks = step.text_blocks.join("\n");
    if all.holds(&step.text) || all.holds(&blocks) {
        return true;
    }
    for (index, call) in step.calls.iter().enumerate() {
        let shown = tools::visible_texts(&call.name, &call.input);
        let leaks = if is_pose(&call.name) {
            let others = live.joined(own(Some(index)));
            shown.iter().any(|text| others.holds(text))
        } else {
            shown.iter().any(|text| all.holds(text))
        };
        let named = ToolName::parse(&call.name).is_none() && all.holds(&call.name);
        if leaks || named {
            return true;
        }
    }
    debug_assert!(all.secrets.len() >= live.secrets.len());
    false
}

/// The replies one model step took: the step kept, the raw reply it came
/// from (`None` for [`neutral_step`]) and every raw reply discarded.
#[derive(Debug)]
pub struct Replies<R> {
    pub step: Step,
    pub kept: Option<R>,
    pub discarded: Vec<R>,
}

/// Make one step's reply with `attempt` and keep the first that
/// [`reply_leaks`] passes. A discarded reply is made once more, with
/// [`SEAL_REMINDER`]; a second one that leaks becomes [`neutral_step`].
pub fn kept_reply<R>(
    live: &Sealer,
    mut attempt: impl FnMut(Option<&'static str>) -> Result<(Step, R), EngineError>,
) -> Result<Replies<R>, EngineError> {
    let mut discarded = Vec::new();
    let mut spent = Spend::default();
    for reminder in [None, Some(SEAL_REMINDER)] {
        let (mut step, raw) = attempt(reminder)?;
        if !reply_leaks(live, &step) {
            step.usage += spent;
            return Ok(Replies {
                step,
                kept: Some(raw),
                discarded,
            });
        }
        spent += step.usage;
        discarded.push(raw);
    }
    debug_assert_eq!(discarded.len(), 2, "both attempts were discarded");
    Ok(Replies {
        step: neutral_step(spent),
        kept: None,
        discarded,
    })
}

/// The step that stands for two discarded replies: [`NEUTRAL_REPLY`], no
/// calls, and what the discarded replies spent.
pub fn neutral_step(spent: Spend) -> Step {
    let step = Step {
        text: NEUTRAL_REPLY.to_string(),
        text_blocks: vec![NEUTRAL_REPLY.to_string()],
        calls: Vec::new(),
        usage: spent,
    };
    debug_assert!(!reply_leaks(&Sealer::default(), &step));
    step
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
pub(crate) fn embedded_json(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if !(trimmed.starts_with('[') || trimmed.starts_with('{')) {
        return None;
    }
    let parsed: Value = serde_json::from_str(trimmed).ok()?;
    (parsed.is_object() || parsed.is_array()).then_some(parsed)
}

/// The sealed texts `call` carries; none when it poses nothing, or when its
/// input is unreadable, which posing refuses.
pub(super) fn call_secrets(call: &ToolCall) -> Vec<String> {
    let secrets = tools::sealed_texts(&call.name, &call.input).unwrap_or_default();
    debug_assert!(secrets.iter().all(|secret| !secret.trim().is_empty()));
    debug_assert!(secrets.is_empty() || is_pose(&call.name));
    secrets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::SEALED;

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

    fn call(id: &str, name: &str, input: Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            input,
        }
    }

    fn reply(text: &str, calls: Vec<ToolCall>) -> Step {
        Step {
            text: text.into(),
            calls,
            ..Step::default()
        }
    }

    fn ask(prompt: &str, key: &str) -> ToolCall {
        let input = json!({"kind": "recall", "prompt": prompt, "answer_key": key});
        call("call_ask", "noesis.ask", input)
    }

    #[test]
    fn a_reply_leaks_a_live_secret_from_anything_it_would_keep() {
        let live = sealer(&["mitochondria"]);
        let search = |input: Value| reply("", vec![call("c", "noesis.graph_search", input)]);
        let leaks = [
            reply("It is the Mitochondria.", vec![]),
            search(json!({"query": "the mitochondria"})),
            search(json!({"mitochondria": 1})),
            search(json!({"query": r#"{"q": "\u006ditochondria"}"#})),
            reply("", vec![call("c", "mitochondria_tool", json!({}))]),
            reply("", vec![ask("Name the mitochondria's role", "energy")]),
        ];
        for step in &leaks {
            assert!(reply_leaks(&live, step), "{step:?}");
        }
        let split = Step {
            text: "x =4".into(),
            text_blocks: vec!["x =".into(), "4".into()],
            ..Step::default()
        };
        assert!(
            reply_leaks(&sealer(&["x = 4"]), &split),
            "read block by block"
        );
        for (secret, input) in [
            ("true", json!({"flag": true})),
            ("null", json!({"q": null})),
        ] {
            assert!(reply_leaks(&sealer(&[secret]), &search(input)), "{secret}");
        }
        let kept = reply(
            "Cells have parts.",
            vec![call(
                "call_mitochondria",
                "noesis.graph_search",
                json!({"query": "cells"}),
            )],
        );
        assert!(!reply_leaks(&live, &kept), "ids are the engine's own");
        // A Noesis tool's name is the crate's text, the same whatever the key.
        assert!(!reply_leaks(
            &sealer(&["ask"]),
            &reply("", vec![ask("Capital?", "Paris")])
        ));
        let graph = call("c", "noesis.graph_search", json!({"query": "cells"}));
        assert!(!reply_leaks(
            &sealer(&["graph", "search"]),
            &reply("", vec![graph])
        ));
    }

    #[test]
    fn a_stringified_pose_is_read_healed_against_live_and_sibling_secrets() {
        let stringified = |prompt: &str, key: &str| {
            let input = json!({"kind": "recall", "prompt": prompt, "answer_key": key});
            call("call_s", "noesis.ask", Value::String(input.to_string()))
        };
        let shows_paris = stringified("Is it Paris?", "Rome");
        let sibling = reply(
            "",
            vec![shows_paris.clone(), ask("Capital of France?", "Paris")],
        );
        assert!(
            reply_leaks(&Sealer::default(), &sibling),
            "another call's key"
        );
        let alone = reply("", vec![shows_paris]);
        assert!(reply_leaks(&sealer(&["Paris"]), &alone), "a live key");
        assert!(
            !reply_leaks(&Sealer::default(), &alone),
            "its own key stays sealed"
        );
        let shown = tools::visible_texts("noesis.ask", &stringified("Is it Paris?", "Rome").input);
        assert!(shown.iter().any(|text| text == "Is it Paris?"), "{shown:?}");
        assert!(!shown.iter().any(|text| text == "Rome"), "{shown:?}");
    }

    #[test]
    fn a_pose_shows_its_kept_input_as_well_as_its_healed_one() {
        let input = json!({"kind": "recall", "prompt": "Capital?", "answer_key": "Paris",
            "multi": "yes", "hint": null});
        let pose = reply("", vec![call("call_p", "noesis.ask", input.clone())]);
        assert!(
            reply_leaks(&sealer(&["yes"]), &pose),
            "healed to true, kept as text"
        );
        assert!(
            reply_leaks(&sealer(&["hint"]), &pose),
            "dropped when healed, kept"
        );
        assert!(!reply_leaks(&sealer(&["Lyon"]), &pose));
        let shown = tools::visible_texts("noesis.ask", &input);
        assert!(
            shown.iter().any(|text| text == "true"),
            "the healed flag: {shown:?}"
        );
        assert!(!shown.iter().any(|text| text == "Paris"), "{shown:?}");
    }

    #[test]
    fn a_reply_leaks_the_secrets_its_own_calls_introduce() {
        let none = Sealer::default();
        let posed = ask("Capital of France?", "Paris");
        assert!(reply_leaks(
            &none,
            &reply("Hint: it is Paris.", vec![posed.clone()])
        ));
        let search = call("c", "noesis.graph_search", json!({"query": "paris"}));
        assert!(reply_leaks(&none, &reply("", vec![search, posed.clone()])));
        assert!(!reply_leaks(&none, &reply("One question:", vec![posed])));
        let options = json!([{"value": "a", "label": "Paris"}, {"value": "b", "label": "Lyon"}]);
        let choice = json!({"kind": "choice", "prompt": "Capital?", "options": options, "answer_key": ["a"]});
        let choice = call("call_choice", "noesis.ask", choice);
        assert!(
            !reply_leaks(&none, &reply("", vec![choice.clone()])),
            "a pose's own options name its key"
        );
        assert!(reply_leaks(&sealer(&["Lyon"]), &reply("", vec![choice])));
    }

    /// Replies made one by one from `script`, with the reminder each was told.
    fn attempts(
        script: Vec<Step>,
    ) -> (
        Result<Replies<usize>, EngineError>,
        Vec<Option<&'static str>>,
    ) {
        let mut script = script.into_iter();
        let mut told = Vec::new();
        let replies = kept_reply(&sealer(&["Paris"]), |reminder| {
            told.push(reminder);
            let step = script.next().expect("a scripted reply");
            Ok((step, told.len()))
        });
        (replies, told)
    }

    #[test]
    fn a_leaking_reply_is_discarded_and_made_again_once_with_a_reminder() {
        let spent = |usd| Spend { usd, tokens: 10 };
        let leak = Step {
            usage: spent(0.25),
            ..reply("It is Paris.", vec![])
        };
        let clean = Step {
            usage: spent(0.5),
            ..reply("Try again.", vec![])
        };
        let (replies, told) = attempts(vec![clean.clone()]);
        let replies = replies.unwrap();
        assert_eq!(
            (replies.step, replies.kept, told),
            (clean.clone(), Some(1), vec![None])
        );
        assert!(replies.discarded.is_empty());

        let (replies, told) = attempts(vec![leak.clone(), clean]);
        let replies = replies.unwrap();
        assert_eq!(told, vec![None, Some(SEAL_REMINDER)]);
        assert_eq!((replies.kept, replies.discarded), (Some(2), vec![1]));
        assert_eq!(replies.step.text, "Try again.");
        assert_eq!(
            replies.step.usage,
            Spend {
                usd: 0.75,
                tokens: 20
            },
            "the discarded one is spent too"
        );

        let (replies, _) = attempts(vec![leak.clone(), leak]);
        let replies = replies.unwrap();
        assert_eq!((replies.kept, replies.discarded), (None, vec![1, 2]));
        assert_eq!(
            replies.step,
            neutral_step(Spend {
                usd: 0.5,
                tokens: 20
            })
        );
        assert!(!SEAL_REMINDER.contains("Paris") && replies.step.calls.is_empty());
    }

    #[test]
    fn a_failed_reply_fails_the_step_and_keeps_nothing() {
        let failed = kept_reply(&Sealer::default(), |_| -> Result<(Step, ()), EngineError> {
            Err(EngineError::Failed("broke".into()))
        });
        assert!(matches!(failed, Err(EngineError::Failed(_))));
    }

    #[test]
    fn a_session_history_holds_no_live_secret_and_reveals_it_once_answered() {
        use super::super::faux::FauxEngine;
        use super::super::records::Answer;
        use super::super::{Teaching, testkit};

        let (_dir, noesis) = testkit::fixture();
        let mut t = Teaching::new(&noesis, FauxEngine::new());
        let id = testkit::teacher_session(&mut t);
        let mut ask = testkit::recall("Name the rule for f(g(x))", "KEY-HISTORY-CHAIN");
        ask["explanation"] = Value::from("EXPLAIN-HISTORY-OUTER");
        assert!(
            !testkit::call(&mut t, &id, testkit::ASK, ask, "")
                .result
                .is_error
        );
        let live = Sealer::new(t.live_secrets(&id).unwrap());
        assert!(live.holds("KEY-HISTORY-CHAIN") && live.holds("EXPLAIN-HISTORY-OUTER"));
        let open = serde_json::to_value(t.get_session(&id).unwrap()).unwrap();
        assert!(!live.reveals(&open), "{open}");

        let question = t.get_session(&id).unwrap().questions[0].question.id.clone();
        let answer = Answer {
            text: Some("the chain rule".into()),
            ..Answer::default()
        };
        t.answer(&id, &question, &answer).unwrap();
        let history = serde_json::to_value(t.get_session(&id).unwrap().questions).unwrap();
        assert!(
            live.reveals(&history),
            "the reveal carries the key once answered"
        );
        assert!(t.live_secrets(&id).unwrap().is_empty());
    }
}
