//! Translate native tool items into Aether content and tool results.

use aether_core::model::ContentBlock;
use serde_json::{Value, json};

/// Cap on file-change diffs kept as tool output.
const MAX_DIFF_CHARS: usize = 4_000;

pub(super) fn str_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

pub(super) fn texts_of(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_str().or_else(|| t["text"].as_str()).map(str::to_owned))
        .collect()
}

/// The tool-use block a thread item corresponds to, when it is a tool.
pub(super) fn tool_use_of(item: &Value) -> Option<ContentBlock> {
    let id = str_of(item, "id");
    let (name, input) = match item["type"].as_str()? {
        "commandExecution" => (
            "bash".to_owned(),
            json!({ "command": item["command"], "cwd": item["cwd"] }),
        ),
        "fileChange" => {
            let changes: Vec<String> = item["changes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| {
                    format!(
                        "{} {}",
                        c["kind"]["type"].as_str().unwrap_or("change"),
                        c["path"].as_str().unwrap_or("?")
                    )
                })
                .collect();
            ("apply_patch".to_owned(), json!({ "changes": changes }))
        }
        "mcpToolCall" => (
            format!(
                "mcp__{}__{}",
                item["server"].as_str().unwrap_or("mcp"),
                item["tool"].as_str().unwrap_or("tool")
            ),
            item["arguments"].clone(),
        ),
        "webSearch" => ("web_search".to_owned(), json!({ "query": item["query"] })),
        _ => return None,
    };
    Some(ContentBlock::ToolUse { id, name, input })
}

/// The result text of a completed tool item: (content, is_error).
pub(super) fn tool_result_of(item: &Value) -> Option<(String, bool)> {
    let status = item["status"].as_str().unwrap_or("");
    match item["type"].as_str()? {
        "commandExecution" => {
            let exit = item["exitCode"].as_i64();
            let output = str_of(item, "aggregatedOutput");
            Some((
                format!(
                    "{output}\n[exit code {}]",
                    exit.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                ),
                exit.is_some_and(|c| c != 0) || status == "failed" || status == "declined",
            ))
        }
        "fileChange" => {
            let mut text = String::new();
            for c in item["changes"].as_array().into_iter().flatten() {
                text.push_str(&format!(
                    "{} {}\n",
                    c["kind"]["type"].as_str().unwrap_or("change"),
                    c["path"].as_str().unwrap_or("?")
                ));
                if let Some(diff) = c["diff"].as_str() {
                    text.push_str(diff);
                    if !diff.ends_with('\n') {
                        text.push('\n');
                    }
                }
            }
            if text.chars().count() > MAX_DIFF_CHARS {
                text = format!("{}…", text.chars().take(MAX_DIFF_CHARS).collect::<String>());
            }
            Some((text, status == "failed" || status == "declined"))
        }
        "mcpToolCall" => {
            let content = if let Some(err) = item["error"]["message"].as_str() {
                err.to_owned()
            } else {
                texts_of(&item["result"]["content"]).join("\n")
            };
            let content = if content.is_empty() {
                item["result"].to_string()
            } else {
                content
            };
            Some((content, crate::codex::mcp_call_failed(item)))
        }
        "webSearch" => {
            let n = item["results"].as_array().map(Vec::len).unwrap_or(0);
            Some((format!("{n} results"), false))
        }
        _ => None,
    }
}
