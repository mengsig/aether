//! Typed arguments of the Noesis tools, read from healed JSON. Problems name
//! fields and what they accept, never a value, so a refusal persisted in a
//! transcript cannot carry a sealed answer key or solution.

use std::fmt;

use serde_json::{Map, Value};

use super::ToolName;
use super::heal::{Field, Kind, field, heal_object};
use crate::teach::records::{Intent, MissClass, Outcome, PracticeMode, QuestionKind};

/// Arguments the tool could not accept, every problem listed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{tool}: {}", problems.join("; "))]
pub struct ArgError {
    pub tool: &'static str,
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionArg {
    pub value: String,
    pub label: String,
}

/// `noesis.ask`. The answer key and explanation are sealed: Debug hides them.
#[derive(Clone, PartialEq, Eq)]
pub struct AskArgs {
    pub kind: QuestionKind,
    pub prompt: String,
    pub options: Vec<OptionArg>,
    pub answer_key: Vec<String>,
    pub explanation: Option<String>,
    pub plan_node: Option<String>,
    pub concepts: Vec<String>,
    pub multi: Option<bool>,
    pub intent: Option<Intent>,
}

impl fmt::Debug for AskArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AskArgs")
            .field("kind", &self.kind)
            .field("prompt", &self.prompt)
            .field("options", &self.options)
            .field("answer_key", &super::SEALED)
            .field("explanation", &super::SEALED)
            .field("plan_node", &self.plan_node)
            .field("concepts", &self.concepts)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StrandArg {
    pub strand: String,
    pub floor: String,
    pub ceiling: String,
    pub misconceptions: Vec<String>,
    pub start: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeDoneArgs {
    pub strands: Vec<StrandArg>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlanNodeArg {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanArgs {
    pub nodes: Vec<PlanNodeArg>,
    pub edges: Vec<crate::teach::records::PlanEdge>,
}

/// `noesis.problem`. The solution is sealed: Debug hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct ProblemArgs {
    pub statement: String,
    pub mode: PracticeMode,
    pub targets: Vec<String>,
    pub solution: String,
}

impl fmt::Debug for ProblemArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProblemArgs")
            .field("statement", &self.statement)
            .field("mode", &self.mode)
            .field("targets", &self.targets)
            .field("solution", &super::SEALED)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConcludeArgs {
    pub outcome: Outcome,
    pub rung: Option<u8>,
    pub miss_class: Option<MissClass>,
    pub retest: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphSearchArgs {
    pub query: String,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnerReadArgs {
    pub concepts: Vec<String>,
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolArgs {
    Ask(AskArgs),
    ProbeDone(ProbeDoneArgs),
    Plan(PlanArgs),
    Problem(ProblemArgs),
    Conclude(ConcludeArgs),
    GraphSearch(GraphSearchArgs),
    LearnerRead(LearnerReadArgs),
}

pub const GRAPH_SEARCH_LIMIT: std::ops::RangeInclusive<u32> = 1..=50;
const GRAPH_SEARCH_DEFAULT: u32 = 10;

const OPTION_FIELDS: &[Field] = &[field("value", Kind::Text), field("label", Kind::Text)];
const STRAND_FIELDS: &[Field] = &[
    field("strand", Kind::Text),
    field("floor", Kind::Text),
    field("ceiling", Kind::Text),
    field("misconceptions", Kind::Texts),
    field("start", Kind::Text),
];
const NODE_FIELDS: &[Field] = &[field("id", Kind::Text), field("title", Kind::Text)];
const EDGE_FIELDS: &[Field] = &[field("from", Kind::Text), field("to", Kind::Text)];

const ASK_FIELDS: &[Field] = &[
    field("kind", Kind::Enum(QuestionKind::NAMES)),
    field("prompt", Kind::Text),
    field("options", Kind::Objects(OPTION_FIELDS)),
    field("answer_key", Kind::Texts),
    field("explanation", Kind::Text),
    field("plan_node", Kind::Text),
    field("concepts", Kind::Texts),
    field("multi", Kind::Bool),
    field("intent", Kind::Enum(Intent::NAMES)),
];
const PROBE_DONE_FIELDS: &[Field] = &[
    field("strands", Kind::Objects(STRAND_FIELDS)),
    field("summary", Kind::Text),
];
const PLAN_FIELDS: &[Field] = &[
    field("nodes", Kind::Objects(NODE_FIELDS)),
    field("edges", Kind::Objects(EDGE_FIELDS)),
];
const PROBLEM_FIELDS: &[Field] = &[
    field("statement", Kind::Text),
    field("mode", Kind::Enum(PracticeMode::NAMES)),
    field("targets", Kind::Texts),
    field("solution", Kind::Text),
];
const CONCLUDE_FIELDS: &[Field] = &[
    field("outcome", Kind::Enum(Outcome::NAMES)),
    field("rung", Kind::Rung),
    field("miss_class", Kind::Enum(MissClass::NAMES)),
    field("retest", Kind::Bool),
];
const GRAPH_SEARCH_FIELDS: &[Field] = &[field("query", Kind::Text), field("limit", Kind::Int)];
const LEARNER_READ_FIELDS: &[Field] = &[field("concepts", Kind::Texts), field("query", Kind::Text)];

/// The fields of each tool, for healing and for refusing unknown names.
pub(crate) fn fields(tool: ToolName) -> &'static [Field] {
    match tool {
        ToolName::Ask => ASK_FIELDS,
        ToolName::ProbeDone => PROBE_DONE_FIELDS,
        ToolName::Plan => PLAN_FIELDS,
        ToolName::Problem => PROBLEM_FIELDS,
        ToolName::Conclude => CONCLUDE_FIELDS,
        ToolName::GraphSearch => GRAPH_SEARCH_FIELDS,
        ToolName::LearnerRead => LEARNER_READ_FIELDS,
    }
}

/// Heal and read one tool call's input.
pub fn parse(tool: ToolName, input: Value) -> Result<ToolArgs, ArgError> {
    let healed = heal_object(input, fields(tool));
    let mut reader = Reader::new(tool, healed);
    let args = match tool {
        ToolName::Ask => ToolArgs::Ask(read_ask(&mut reader)),
        ToolName::ProbeDone => ToolArgs::ProbeDone(read_probe_done(&mut reader)),
        ToolName::Plan => ToolArgs::Plan(read_plan(&mut reader)),
        ToolName::Problem => ToolArgs::Problem(ProblemArgs {
            statement: reader.text("statement"),
            mode: reader.enumeration(
                "mode",
                PracticeMode::parse,
                PracticeMode::NAMES,
                PracticeMode::Recall,
            ),
            targets: reader.texts("targets"),
            solution: reader.text("solution"),
        }),
        ToolName::Conclude => ToolArgs::Conclude(ConcludeArgs {
            outcome: reader.enumeration(
                "outcome",
                Outcome::parse,
                Outcome::NAMES,
                Outcome::Dismissed,
            ),
            rung: reader.opt_rung("rung"),
            miss_class: reader.opt_enumeration("miss_class", MissClass::parse, MissClass::NAMES),
            retest: reader.boolean("retest"),
        }),
        ToolName::GraphSearch => ToolArgs::GraphSearch(GraphSearchArgs {
            query: reader.text("query"),
            limit: reader.limit("limit"),
        }),
        ToolName::LearnerRead => ToolArgs::LearnerRead(LearnerReadArgs {
            concepts: reader.texts("concepts"),
            query: reader.opt_text("query"),
        }),
    };
    reader.finish().map(|()| args)
}

fn read_ask(reader: &mut Reader) -> AskArgs {
    let options = reader.objects("options", |entry| {
        // A bare text option is its own value and label.
        if let Value::String(text) = entry {
            let text = text.trim().to_string();
            return Ok(OptionArg {
                value: text.clone(),
                label: text,
            });
        }
        let value = entry_text(entry, "value")?;
        let label = entry_text(entry, "label").unwrap_or_else(|_| value.clone());
        Ok(OptionArg { value, label })
    });
    AskArgs {
        kind: reader.enumeration(
            "kind",
            QuestionKind::parse,
            QuestionKind::NAMES,
            QuestionKind::Goal,
        ),
        prompt: reader.text("prompt"),
        options,
        answer_key: reader.texts("answer_key"),
        explanation: reader.opt_text("explanation"),
        plan_node: reader.opt_text("plan_node"),
        concepts: reader.texts("concepts"),
        multi: reader.opt_boolean("multi"),
        intent: reader.opt_enumeration("intent", Intent::parse, Intent::NAMES),
    }
}

fn read_probe_done(reader: &mut Reader) -> ProbeDoneArgs {
    let strands = reader.objects("strands", |entry| {
        Ok(StrandArg {
            strand: entry_text(entry, "strand")?,
            floor: entry_text(entry, "floor")?,
            ceiling: entry_text(entry, "ceiling")?,
            misconceptions: entry_texts(entry, "misconceptions")?,
            start: entry_text(entry, "start")?,
        })
    });
    ProbeDoneArgs {
        strands,
        summary: reader.opt_text("summary"),
    }
}

fn read_plan(reader: &mut Reader) -> PlanArgs {
    let nodes = reader.objects("nodes", |entry| {
        Ok(PlanNodeArg {
            id: entry_text(entry, "id")?,
            title: entry_text(entry, "title")?,
        })
    });
    let edges = reader.objects("edges", |entry| {
        Ok(crate::teach::records::PlanEdge {
            from: entry_text(entry, "from")?,
            to: entry_text(entry, "to")?,
        })
    });
    PlanArgs { nodes, edges }
}

fn entry_text(entry: &Value, key: &str) -> Result<String, String> {
    match entry.get(key) {
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(text.trim().to_string()),
        Some(Value::String(_)) => Err(format!("`{key}` is empty")),
        Some(_) => Err(format!("`{key}` must be text")),
        None => Err(format!("`{key}` is missing")),
    }
}

fn entry_texts(entry: &Value, key: &str) -> Result<Vec<String>, String> {
    match entry.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(text) => Ok(text.trim().to_string()),
                _ => Err(format!("`{key}` must be a list of texts")),
            })
            .collect(),
        Some(_) => Err(format!("`{key}` must be a list of texts")),
    }
}

/// Reads fields off one argument object, collecting every problem.
struct Reader {
    tool: &'static str,
    object: Map<String, Value>,
    problems: Vec<String>,
}

impl Reader {
    fn new(tool: ToolName, value: Value) -> Self {
        let name = tool.as_str();
        let Value::Object(object) = value else {
            let problems = vec!["the arguments must be a JSON object".to_string()];
            return Self {
                tool: name,
                object: Map::new(),
                problems,
            };
        };
        let known = fields(tool);
        let mut problems: Vec<String> = object
            .keys()
            .filter(|key| !known.iter().any(|field| field.name == key.as_str()))
            .map(|key| format!("unknown field `{}`", printable(key)))
            .collect();
        problems.sort();
        Self {
            tool: name,
            object,
            problems,
        }
    }

    fn problem(&mut self, text: String) {
        self.problems.push(text);
    }

    fn opt_text(&mut self, key: &str) -> Option<String> {
        match self.object.get(key)? {
            Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
            Value::String(_) => None,
            _ => {
                self.problem(format!("`{key}` must be text"));
                None
            }
        }
    }

    fn text(&mut self, key: &str) -> String {
        if !self.object.contains_key(key) {
            self.problem(format!("`{key}` is required"));
            return String::new();
        }
        let present = self.problems.len();
        let text = self.opt_text(key);
        if text.is_none() && self.problems.len() == present {
            self.problem(format!("`{key}` is empty"));
        }
        text.unwrap_or_default()
    }

    fn texts(&mut self, key: &str) -> Vec<String> {
        let Some(value) = self.object.get(key) else {
            return Vec::new();
        };
        let entry = Value::Object(Map::from_iter([(key.to_string(), value.clone())]));
        entry_texts(&entry, key).unwrap_or_else(|problem| {
            self.problem(problem);
            Vec::new()
        })
    }

    fn opt_enumeration<T>(
        &mut self,
        key: &str,
        parse: fn(&str) -> Option<T>,
        names: &[&str],
    ) -> Option<T> {
        let value = self.object.get(key)?;
        let parsed = value.as_str().and_then(parse);
        if parsed.is_none() {
            self.problem(format!("`{key}` must be one of {}", names.join(", ")));
        }
        parsed
    }

    /// A required enum; a missing or bad value is reported and yields
    /// `fallback`, so reading can go on and collect every problem.
    fn enumeration<T: Copy>(
        &mut self,
        key: &str,
        parse: fn(&str) -> Option<T>,
        names: &[&str],
        fallback: T,
    ) -> T {
        debug_assert!(!names.is_empty() && names.iter().all(|name| parse(name).is_some()));
        if !self.object.contains_key(key) {
            self.problem(format!("`{key}` is required: one of {}", names.join(", ")));
        }
        self.opt_enumeration(key, parse, names).unwrap_or(fallback)
    }

    fn opt_boolean(&mut self, key: &str) -> Option<bool> {
        let value = self.object.get(key)?;
        let parsed = value.as_bool();
        if parsed.is_none() {
            self.problem(format!("`{key}` must be true or false"));
        }
        parsed
    }

    fn boolean(&mut self, key: &str) -> bool {
        if !self.object.contains_key(key) {
            self.problem(format!("`{key}` is required: true or false"));
        }
        self.opt_boolean(key).unwrap_or(false)
    }

    fn opt_rung(&mut self, key: &str) -> Option<u8> {
        let value = self.object.get(key)?;
        let rung = value.as_u64().filter(|rung| *rung <= 5);
        if rung.is_none() {
            self.problem(format!("`{key}` must be a hint rung from 0 (R0) to 5 (R5)"));
        }
        rung.map(|rung| rung as u8)
    }

    fn limit(&mut self, key: &str) -> u32 {
        let Some(value) = self.object.get(key) else {
            return GRAPH_SEARCH_DEFAULT;
        };
        let limit = value.as_u64().and_then(|limit| u32::try_from(limit).ok());
        match limit.filter(|limit| GRAPH_SEARCH_LIMIT.contains(limit)) {
            Some(limit) => limit,
            None => {
                let (low, high) = (GRAPH_SEARCH_LIMIT.start(), GRAPH_SEARCH_LIMIT.end());
                self.problem(format!(
                    "`{key}` must be a whole number from {low} to {high}"
                ));
                GRAPH_SEARCH_DEFAULT
            }
        }
    }

    /// A list of objects, each read by `read`; problems carry the index.
    fn objects<T>(&mut self, key: &str, read: impl Fn(&Value) -> Result<T, String>) -> Vec<T> {
        let Some(value) = self.object.get(key) else {
            return Vec::new();
        };
        let Value::Array(items) = value else {
            self.problem(format!("`{key}` must be a list"));
            return Vec::new();
        };
        let mut read_items = Vec::with_capacity(items.len());
        let mut problems = Vec::new();
        for (index, item) in items.iter().enumerate() {
            match read(item) {
                Ok(parsed) => read_items.push(parsed),
                Err(problem) => problems.push(format!("`{key}[{index}]`: {problem}")),
            }
        }
        self.problems.extend(problems);
        read_items
    }

    fn finish(self) -> Result<(), ArgError> {
        if self.problems.is_empty() {
            return Ok(());
        }
        Err(ArgError {
            tool: self.tool,
            problems: self.problems,
        })
    }
}

/// An unknown field name is the model's text, so it is shortened and kept
/// to a safe alphabet before it is echoed into a refusal.
fn printable(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(40)
        .collect()
}
