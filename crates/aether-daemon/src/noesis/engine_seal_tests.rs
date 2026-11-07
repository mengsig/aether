//! A reply's blocks as the transcript keeps them: sealed against the
//! secrets live while it was written and those its own calls carry.

use serde_json::json;

use super::*;

fn text(text: &str) -> ContentBlock {
    ContentBlock::Text {
        text: text.to_string(),
    }
}

fn call(id: &str, tool: ToolName, input: Value) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: wire_name(tool.as_str()),
        input,
    }
}

fn reply(content: Vec<ContentBlock>) -> ModelResponse {
    ModelResponse {
        content,
        ..ModelResponse::default()
    }
}

fn kept(response: &ModelResponse, live: &Sealer) -> Vec<ContentBlock> {
    transcript_content(response, live.with_step(&step_of(response)).as_ref())
}

fn inputs(content: &[ContentBlock]) -> Vec<Value> {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { input, .. } => Some(input.clone()),
            _ => None,
        })
        .collect()
}

fn texts(content: &[ContentBlock]) -> Vec<String> {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_live_secret_split_across_text_blocks_is_withheld_whole() {
    let live = Sealer::new(["SECRET-ANSWER"]);
    let response = reply(vec![text("Psst: SECRET-"), text("ANSWER")]);
    let content = kept(&response, &live);
    assert_eq!(
        texts(&content),
        [WITHHELD],
        "one note stands for both blocks"
    );
    let spaced = reply(vec![text("Psst: SECRET-ANSWER is"), text("close")]);
    assert_eq!(texts(&kept(&spaced, &live)), [WITHHELD]);
    let plain = reply(vec![text("Try"), text("again.")]);
    assert_eq!(texts(&kept(&plain, &live)), ["Try", "again."]);
}

#[test]
fn a_tool_input_holding_a_live_or_its_own_replys_secret_is_sealed() {
    let live = Sealer::new(["SOLUTION-X"]);
    let ask = json!({"kind": "recall", "prompt": "Name the code", "answer_key": "OWN-KEY"});
    let refused = json!({"statement": "Again: SOLUTION-X", "mode": "recall", "solution": "other"});
    let response = reply(vec![
        text("Look:"),
        call("c1", ToolName::GraphSearch, json!({"query": "SOLUTION-X"})),
        call(
            "c2",
            ToolName::LearnerRead,
            json!({"concepts": ["OWN-KEY"]}),
        ),
        call("c3", ToolName::Ask, ask.clone()),
        call("c4", ToolName::Problem, refused),
        call("c5", ToolName::GraphSearch, json!({"query": "cells"})),
    ]);
    let content = kept(&response, &live);
    let sealed = json!(tools::SEALED);
    assert_eq!(texts(&content), ["Look:"]);
    let kept_inputs = inputs(&content);
    assert_eq!(kept_inputs[..2], [sealed.clone(), sealed.clone()]);
    assert_eq!(kept_inputs[2], ask, "the runtime seals a pose's own fields");
    assert_eq!(kept_inputs[3]["statement"], sealed, "{:?}", kept_inputs[3]);
    assert_eq!(kept_inputs[3]["mode"], "recall");
    assert_eq!(kept_inputs[4], json!({"query": "cells"}));
}

#[test]
fn a_reply_whose_secrets_cannot_be_read_is_kept_sealed_whole() {
    let response = reply(vec![
        text("Hello there."),
        call("c1", ToolName::Ask, json!("not an object")),
        call("c2", ToolName::GraphSearch, json!({"query": "cells"})),
    ]);
    let content = kept(&response, &Sealer::default());
    assert_eq!(texts(&content), [WITHHELD]);
    let sealed = json!(tools::SEALED);
    assert_eq!(inputs(&content), [sealed.clone(), sealed]);
}
