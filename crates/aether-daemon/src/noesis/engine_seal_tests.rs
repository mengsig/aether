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
fn a_sealing_calls_key_holding_a_secret_seals_its_input_whole() {
    let live = Sealer::new(["SOLUTION-X"]);
    let own = json!({"statement": "Solve", "mode": "recall", "solution": "OWN-Y", "OWN-Y": 1});
    let named = json!({"statement": "Solve", "mode": "recall", "solution": "z", "SOLUTION-X": 1});
    let plain = json!({"statement": "Solve", "mode": "recall", "solution": "w"});
    let response = reply(vec![
        call("c1", ToolName::Problem, own),
        call("c2", ToolName::Problem, named),
        call("c3", ToolName::Problem, plain.clone()),
    ]);
    let kept_inputs = inputs(&kept(&response, &live));
    let sealed = json!(tools::SEALED);
    assert_eq!(kept_inputs[..2], [sealed.clone(), sealed]);
    assert_eq!(
        kept_inputs[2], plain,
        "the runtime seals a plain pose's solution"
    );
}

fn ids_and_names(content: &[ContentBlock]) -> Vec<(String, String)> {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, .. } => Some((id.clone(), name.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_call_id_or_name_holding_a_secret_is_renamed_and_its_result_follows() {
    let live = Sealer::new(["SOLUTION-X"]);
    let ask = json!({"kind": "recall", "prompt": "Name the code", "answer_key": "OWN-KEY"});
    let response = reply(vec![
        ContentBlock::ToolUse {
            id: "c1".into(),
            name: "OWN-KEY".into(),
            input: json!({}),
        },
        call("id-SOLUTION-X", ToolName::Ask, ask),
        call("c3", ToolName::GraphSearch, json!({"query": "cells"})),
    ]);
    let content = kept(&response, &live);
    let ask_wire = wire_name(ToolName::Ask.as_str());
    let search_wire = wire_name(ToolName::GraphSearch.as_str());
    let kept_calls = ids_and_names(&content);
    let renamed = sealed_id("id-SOLUTION-X");
    assert_eq!(kept_calls[0], ("c1".to_string(), SEALED_TOOL.to_string()));
    assert_eq!(kept_calls[1], (renamed.clone(), ask_wire));
    assert_eq!(kept_calls[2], ("c3".to_string(), search_wire));
    assert!(!live.holds(&renamed) && !renamed.contains("OWN-KEY"));
    let history = [Message {
        role: MessageRole::Assistant,
        content,
    }];
    let (calls, _) = last_calls(&history);
    assert_eq!(recorded_id(&calls, "id-SOLUTION-X".into()), renamed);
    assert_eq!(recorded_id(&calls, "c3".into()), "c3");
    assert_eq!(open_asks(&calls, &[]), [renamed]);
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
    let kept_ids: Vec<String> = ids_and_names(&content)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        kept_ids,
        [sealed_id("c1"), sealed_id("c2")],
        "unread ids are renamed too"
    );
}
