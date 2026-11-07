//! The result of a task its agent closed in prose. A model that writes its
//! `complete_task` input as its answer, instead of calling the tool, still
//! hands its parent the structured result it wrote.

use aether_core::id::ArtifactId;
use aether_core::task::TaskResult;
use aether_tools::builtin::harness::MAX_STRUCTURED_BYTES;
use serde_json::{Map, Value};

const TOOL_NAME: &str = "complete_task";
/// The name an engine shows the tool under, through Aether's MCP server.
const MCP_TOOL_NAME: &str = "mcp__aether__complete_task";

/// `text` as the summary. An answer that is nothing but one JSON object, bare
/// or fenced, also gives that object as the structured result; written as the
/// tool's own input (`summary` and `structured`), it gives those instead.
pub(super) fn prose_result(text: String) -> TaskResult {
    debug_assert!(!text.trim().is_empty(), "a blank reply is never a result");
    let Some(object) = written_object(&text) else {
        return TaskResult::summary(text);
    };
    let (summary, artifacts, structured) = match tool_input(&object) {
        Some(input) => (
            input.summary.unwrap_or(text),
            input.artifacts,
            input.structured,
        ),
        None => (text, Vec::new(), object),
    };
    if Value::Object(structured.clone()).to_string().len() > MAX_STRUCTURED_BYTES {
        return TaskResult::summary(summary);
    }
    debug_assert!(!summary.trim().is_empty(), "the summary is never blank");
    TaskResult {
        summary,
        artifacts,
        structured: Some(Value::Object(structured)),
    }
}

/// The JSON object the whole answer is: bare, in one code fence, after the
/// tool's name, or as `{"complete_task": {...}}`, under either of its names.
fn written_object(text: &str) -> Option<Map<String, Value>> {
    let body = unfenced(text.trim());
    let body = [MCP_TOOL_NAME, TOOL_NAME]
        .iter()
        .find_map(|name| body.strip_prefix(name))
        .map_or(body, str::trim_start);
    if !body.starts_with('{') {
        return None;
    }
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(body) else {
        return None;
    };
    let wrapped = [TOOL_NAME, MCP_TOOL_NAME]
        .iter()
        .find_map(|name| match object.get(*name) {
            Some(Value::Object(inner)) if object.len() == 1 => Some(inner.clone()),
            _ => None,
        });
    Some(wrapped.unwrap_or(object))
}

/// The inside of one code fence around the whole of `text`, else `text`.
fn unfenced(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let Some(inner) = rest.strip_suffix("```") else {
        return text;
    };
    // The fence's first line names the language, if anything.
    let Some((_, body)) = inner.split_once('\n') else {
        return text;
    };
    if body.contains("```") {
        return text;
    }
    body.trim()
}

struct ToolInput {
    summary: Option<String>,
    artifacts: Vec<ArtifactId>,
    structured: Map<String, Value>,
}

/// An object shaped as `complete_task`'s input: no other key than `summary`,
/// `artifacts` and `structured`; `structured` an object, `summary` text and
/// `artifacts` valid artifact ids if present.
fn tool_input(object: &Map<String, Value>) -> Option<ToolInput> {
    if !object
        .keys()
        .all(|key| matches!(key.as_str(), "summary" | "artifacts" | "structured"))
    {
        return None;
    }
    let Some(Value::Object(structured)) = object.get("structured") else {
        return None;
    };
    let summary = match object.get("summary") {
        None => None,
        Some(Value::String(text)) if !text.trim().is_empty() => Some(text.clone()),
        Some(_) => return None,
    };
    let artifacts = match object.get("artifacts") {
        None => Vec::new(),
        Some(Value::Array(ids)) => ids
            .iter()
            .map(|id| id.as_str()?.parse().ok())
            .collect::<Option<_>>()?,
        Some(_) => return None,
    };
    Some(ToolInput {
        summary,
        artifacts,
        structured: structured.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::prose_result;
    use serde_json::json;

    #[test]
    fn plain_prose_stays_a_summary_alone() {
        for text in [
            "Fixed the boundary and committed it.",
            "Verdict: pass. {\"verdict\": \"pass\"} is what I would send.",
            "[1, 2, 3]",
            "{not json at all}",
        ] {
            let result = prose_result(text.into());
            assert_eq!(result.summary, text);
            assert_eq!(result.structured, None, "{text}");
        }
    }

    #[test]
    fn an_answer_that_is_one_json_object_gives_it_as_the_structured_result() {
        let verdict = json!({"verdict": "pass", "head": "abc", "findings": []});
        let bare = verdict.to_string();
        for text in [
            bare.clone(),
            format!("```json\n{bare}\n```"),
            format!("```\n{bare}\n```"),
            format!("complete_task {bare}"),
            format!("mcp__aether__complete_task {bare}"),
            json!({"complete_task": verdict}).to_string(),
            json!({"mcp__aether__complete_task": verdict}).to_string(),
            format!("  \n{}\n", serde_json::to_string_pretty(&verdict).unwrap()),
        ] {
            let result = prose_result(text.clone());
            assert_eq!(result.structured.as_ref(), Some(&verdict), "{text}");
            assert_eq!(result.summary, text, "the answer is kept as written");
        }
    }

    #[test]
    fn the_tool_input_written_out_gives_its_summary_and_structured() {
        let structured = json!({"verdict": "fail", "findings": [{"blocking": true}]});
        let input = json!({"summary": "One blocking finding.", "structured": structured});
        for text in [
            input.to_string(),
            json!({"complete_task": input}).to_string(),
        ] {
            let result = prose_result(text.clone());
            assert_eq!(result.summary, "One blocking finding.", "{text}");
            assert_eq!(result.structured.as_ref(), Some(&structured), "{text}");
        }
        // Only `structured`: the answer itself stays the summary.
        let text = json!({"structured": structured}).to_string();
        let result = prose_result(text.clone());
        assert_eq!(result.summary, text);
        assert_eq!(result.structured.as_ref(), Some(&structured));
    }

    #[test]
    fn the_tool_input_written_out_keeps_its_artifacts() {
        let art = aether_core::id::ArtifactId::new();
        let input = json!({
            "summary": "done", "artifacts": [art.to_string()],
            "structured": {"verdict": "pass"},
        });
        let result = prose_result(input.to_string());
        assert_eq!(result.summary, "done");
        assert_eq!(result.artifacts, vec![art]);
        assert_eq!(result.structured, Some(json!({"verdict": "pass"})));
        let bad = json!({"artifacts": ["nope"], "structured": {"a": 1}});
        assert_eq!(prose_result(bad.to_string()).structured, Some(bad));
    }

    #[test]
    fn an_object_not_shaped_as_the_tool_input_is_the_structured_result_whole() {
        let object = json!({"summary": "done", "structured": "not an object"});
        let result = prose_result(object.to_string());
        assert_eq!(result.structured.as_ref(), Some(&object));
        let object = json!({"summary": 3, "structured": {"a": 1}});
        assert_eq!(prose_result(object.to_string()).structured, Some(object));
    }

    #[test]
    fn an_oversized_object_is_left_in_the_summary() {
        let huge = json!({"blob": "x".repeat(super::MAX_STRUCTURED_BYTES)}).to_string();
        let result = prose_result(huge.clone());
        assert_eq!(result.structured, None);
        assert_eq!(result.summary, huge);
    }
}
