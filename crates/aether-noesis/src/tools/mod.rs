//! The Noesis-only harness tools: their names, who may call them, their
//! schemas, the sealing projection and argument healing.
//!
//! Tools exist only for a running service and only for `noesis.*` roles;
//! [`registered_tools`] is the one place that decides.

use serde_json::{Value, json};

use crate::Noesis;
use crate::prompts::Agent;
use crate::teach::agent_for_role;

pub mod args;
mod heal;

pub use args::{ArgError, ToolArgs, parse};

/// What a sealed field reads as everywhere outside `noesis.db`.
pub const SEALED: &str = "<sealed>";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
    Ask,
    ProbeDone,
    Plan,
    Problem,
    Conclude,
    GraphSearch,
    LearnerRead,
}

impl ToolName {
    pub const ALL: [ToolName; 7] = [
        ToolName::Ask,
        ToolName::ProbeDone,
        ToolName::Plan,
        ToolName::Problem,
        ToolName::Conclude,
        ToolName::GraphSearch,
        ToolName::LearnerRead,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ToolName::Ask => "noesis.ask",
            ToolName::ProbeDone => "noesis.probe_done",
            ToolName::Plan => "noesis.plan",
            ToolName::Problem => "noesis.problem",
            ToolName::Conclude => "noesis.conclude",
            ToolName::GraphSearch => "noesis.graph_search",
            ToolName::LearnerRead => "noesis.learner_read",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.as_str() == name)
    }

    /// The input fields sealed until their reveal (CONTRACTS.md §6.2).
    pub fn sealed_fields(self) -> &'static [&'static str] {
        match self {
            ToolName::Ask => &["answer_key", "explanation"],
            ToolName::Problem => &["solution"],
            _ => &[],
        }
    }
}

/// The tools each agent may call.
pub fn tools_for(agent: Agent) -> &'static [ToolName] {
    match agent {
        Agent::Prober => &[
            ToolName::Ask,
            ToolName::ProbeDone,
            ToolName::GraphSearch,
            ToolName::LearnerRead,
        ],
        Agent::Teacher => &[
            ToolName::Ask,
            ToolName::Plan,
            ToolName::GraphSearch,
            ToolName::LearnerRead,
        ],
        Agent::Practitioner => &[
            ToolName::Problem,
            ToolName::Conclude,
            ToolName::GraphSearch,
            ToolName::LearnerRead,
        ],
    }
}

/// The input of a tool call as it may be persisted, streamed or logged:
/// each sealed field reads [`SEALED`]. Input of a sealed tool that is not an
/// object might hold sealed text in any shape, so all of it is sealed.
pub fn seal_input(tool: &str, input: &Value) -> Value {
    let Some(sealed) = ToolName::parse(tool).map(ToolName::sealed_fields) else {
        return input.clone();
    };
    if sealed.is_empty() {
        return input.clone();
    }
    let Value::Object(map) = input else {
        return Value::String(SEALED.to_string());
    };
    let mut projected = map.clone();
    for name in sealed {
        if let Some(value) = projected.get_mut(*name) {
            *value = Value::String(SEALED.to_string());
        }
    }
    debug_assert!(sealed.iter().all(|name| {
        projected
            .get(*name)
            .is_none_or(|value| value.as_str() == Some(SEALED))
    }));
    Value::Object(projected)
}

/// Every sealed text a call carries, read as the parser reads it: the sealed
/// fields, a choice key shown as its options' labels (values are ids the
/// learner sees). `None` when a sealed input is no object: fail closed.
pub fn sealed_texts(tool: &str, input: &Value) -> Option<Vec<String>> {
    let Some(tool) = ToolName::parse(tool) else {
        return Some(Vec::new());
    };
    if tool.sealed_fields().is_empty() {
        return Some(Vec::new());
    }
    let Value::Object(map) = heal::heal_object(input.clone(), args::fields(tool)) else {
        return None;
    };
    let mut texts = Vec::new();
    for name in tool.sealed_fields() {
        let Some(value) = map.get(*name) else {
            continue;
        };
        let choice = map.get("kind").and_then(Value::as_str) == Some("choice");
        if *name == "answer_key" && choice {
            texts.extend(shown_key(&map, value));
        } else {
            collect_texts(value, &mut texts);
        }
    }
    debug_assert!(texts.iter().all(|text| !text.trim().is_empty()));
    Some(texts)
}

/// The key as the learner would recognise it: an entry naming an option (by
/// its trimmed value, as the parser compares) is that option's label.
fn shown_key(map: &serde_json::Map<String, Value>, key: &Value) -> Vec<String> {
    let mut entries = Vec::new();
    collect_texts(key, &mut entries);
    let options = map.get("options").and_then(Value::as_array);
    let labels: Vec<(&str, &str)> = options
        .into_iter()
        .flatten()
        .filter_map(|o| Some((o.get("value")?.as_str()?.trim(), o.get("label")?.as_str()?)))
        .collect();
    let count = entries.len();
    let shown: Vec<String> = entries
        .into_iter()
        .map(|entry| {
            let label = labels.iter().find(|(value, _)| *value == entry.trim());
            match label {
                Some((_, label)) if !label.trim().is_empty() => label.to_string(),
                _ => entry,
            }
        })
        .collect();
    debug_assert_eq!(shown.len(), count, "each key entry is shown once");
    shown
}

fn collect_texts(value: &Value, texts: &mut Vec<String>) {
    match value {
        Value::String(text) if !text.trim().is_empty() => texts.push(text.clone()),
        Value::Number(number) => texts.push(number.to_string()),
        Value::Array(items) => items.iter().for_each(|item| collect_texts(item, texts)),
        Value::Object(map) => map.values().for_each(|item| collect_texts(item, texts)),
        _ => {}
    }
}

/// A tool as offered to a model.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// The tools to register for an agent with `role`: none unless Noesis is
/// running and the role is a `noesis.*` role.
pub fn registered_tools(noesis: Option<&Noesis>, role: &str) -> Vec<ToolDef> {
    let (Some(_running), Some(agent)) = (noesis, agent_for_role(role)) else {
        return Vec::new();
    };
    definitions(agent)
}

/// The definitions of every tool `agent` may call.
pub fn definitions(agent: Agent) -> Vec<ToolDef> {
    let tools = tools_for(agent);
    let defs: Vec<ToolDef> = tools.iter().map(|tool| definition(*tool)).collect();
    debug_assert_eq!(defs.len(), tools.len());
    defs
}

fn definition(tool: ToolName) -> ToolDef {
    let (description, input_schema) = match tool {
        ToolName::Ask => (
            "Pose one structured question. A successful pose ends your turn; the answer arrives as this tool's result.",
            ask_schema(),
        ),
        ToolName::ProbeDone => (
            "End the probe with the per-strand assessment and hand the session to the Teacher. Call once.",
            probe_done_schema(),
        ),
        ToolName::Plan => (
            "Set the whole prerequisite plan as a DAG; each call replaces the previous plan.",
            plan_schema(),
        ),
        ToolName::Problem => (
            "Pose one practice problem with its committed, hidden solution.",
            problem_schema(),
        ),
        ToolName::Conclude => (
            "Close the active problem with its outcome; this reveals the solution to the learner.",
            conclude_schema(),
        ),
        ToolName::GraphSearch => (
            "Search the knowledge graph of true statements.",
            object(
                json!({"query": text(), "limit": {"type": "integer", "minimum": 1, "maximum": 50}}),
                &["query"],
            ),
        ),
        ToolName::LearnerRead => (
            "Read what the learner model holds about concepts.",
            object(json!({"concepts": texts(), "query": text()}), &[]),
        ),
    };
    ToolDef {
        name: tool.as_str(),
        description,
        input_schema,
    }
}

fn text() -> Value {
    json!({"type": "string"})
}

fn texts() -> Value {
    json!({"type": "array", "items": {"type": "string"}})
}

fn names(values: &[&str]) -> Value {
    json!({"type": "string", "enum": values})
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn ask_schema() -> Value {
    use crate::teach::records::{Intent, QuestionKind};
    let option = object(
        json!({"value": text(), "label": text()}),
        &["value", "label"],
    );
    object(
        json!({
            "kind": names(QuestionKind::NAMES),
            "prompt": text(),
            "options": {"type": "array", "items": option, "minItems": 2},
            "answer_key": {"anyOf": [text(), texts()]},
            "explanation": text(),
            "plan_node": text(),
            "concepts": texts(),
            "multi": {"type": "boolean"},
            "intent": names(Intent::NAMES),
        }),
        &["kind", "prompt"],
    )
}

fn probe_done_schema() -> Value {
    let strand = object(
        json!({"strand": text(), "floor": text(), "ceiling": text(), "misconceptions": texts(), "start": text()}),
        &["strand", "floor", "ceiling", "start"],
    );
    object(
        json!({"strands": {"type": "array", "items": strand, "minItems": 1}, "summary": text()}),
        &["strands"],
    )
}

fn plan_schema() -> Value {
    let node = object(json!({"id": text(), "title": text()}), &["id", "title"]);
    let edge = object(json!({"from": text(), "to": text()}), &["from", "to"]);
    object(
        json!({"nodes": {"type": "array", "items": node, "minItems": 1}, "edges": {"type": "array", "items": edge}}),
        &["nodes"],
    )
}

fn problem_schema() -> Value {
    use crate::teach::records::PracticeMode;
    object(
        json!({"statement": text(), "mode": names(PracticeMode::NAMES), "targets": texts(), "solution": text()}),
        &["statement", "mode", "solution"],
    )
}

fn conclude_schema() -> Value {
    use crate::teach::records::{MissClass, Outcome};
    object(
        json!({
            "outcome": names(Outcome::NAMES),
            "rung": {"type": "integer", "minimum": 0, "maximum": 5},
            "miss_class": names(MissClass::NAMES),
            "retest": {"type": "boolean"},
        }),
        &["outcome", "retest"],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_name_round_trips_and_only_ask_and_problem_seal() {
        for tool in ToolName::ALL {
            assert_eq!(ToolName::parse(tool.as_str()), Some(tool));
            assert!(tool.as_str().starts_with("noesis."));
            let sealing = matches!(tool, ToolName::Ask | ToolName::Problem);
            assert_eq!(!tool.sealed_fields().is_empty(), sealing, "{tool:?}");
        }
        assert_eq!(ToolName::parse("ask"), None);
    }

    #[test]
    fn sealing_replaces_sealed_fields_and_whole_malformed_inputs() {
        let input = json!({"kind": "recall", "prompt": "Name it", "answer_key": "mitochondria", "explanation": "because"});
        let sealed = seal_input("noesis.ask", &input);
        assert_eq!(
            sealed,
            json!({"kind": "recall", "prompt": "Name it", "answer_key": SEALED, "explanation": SEALED})
        );
        let text_input = Value::String(r#"{"solution": "x = 4"}"#.into());
        assert_eq!(seal_input("noesis.problem", &text_input), json!(SEALED));
        let plan = json!({"nodes": []});
        assert_eq!(seal_input("noesis.plan", &plan), plan);
        assert_eq!(seal_input("shell", &input), input);
    }

    #[test]
    fn sealed_texts_show_keyed_labels_by_trimmed_value_and_keep_every_other_text() {
        let input = json!({"kind": "choice", "prompt": "?", "answer_key": ["a"], "explanation": "b",
            "options": [{"value": " a ", "label": "Paris"}, {"value": "b", "label": "Rome"}]});
        assert_eq!(
            sealed_texts("noesis.ask", &input),
            Some(vec!["Paris".into(), "b".into()])
        );
        let stringified = Value::String(r#"{"solution": "x = 4"}"#.into());
        assert_eq!(
            sealed_texts("noesis.problem", &stringified),
            Some(vec!["x = 4".into()])
        );
        assert_eq!(sealed_texts("noesis.problem", &json!("x = 4")), None);
        assert_eq!(
            sealed_texts("noesis.plan", &json!("anything")),
            Some(vec![])
        );
    }

    #[test]
    fn schemas_list_exactly_the_fields_the_parser_reads() {
        for tool in ToolName::ALL {
            let def = definition(tool);
            let properties = def.input_schema["properties"].as_object().unwrap();
            let mut schema: Vec<&str> = properties.keys().map(String::as_str).collect();
            let mut parsed: Vec<&str> = args::fields(tool).iter().map(|f| f.name).collect();
            schema.sort_unstable();
            parsed.sort_unstable();
            assert_eq!(schema, parsed, "{tool:?}");
        }
    }

    #[test]
    fn each_agent_gets_only_its_own_tools() {
        let names = |agent| -> Vec<&str> { definitions(agent).iter().map(|d| d.name).collect() };
        assert!(names(Agent::Prober).contains(&"noesis.probe_done"));
        assert!(!names(Agent::Teacher).contains(&"noesis.probe_done"));
        assert!(names(Agent::Teacher).contains(&"noesis.plan"));
        assert!(!names(Agent::Practitioner).contains(&"noesis.ask"));
        assert!(names(Agent::Practitioner).contains(&"noesis.conclude"));
    }
}
