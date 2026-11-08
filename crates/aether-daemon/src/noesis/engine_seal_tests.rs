//! A reply as the seal check reads it and as the transcript keeps it once
//! it passed: whole, but for its thinking.

use aether_noesis::teach::reply_leaks;
use serde_json::{Value, json};

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

#[test]
fn a_live_secret_split_across_text_blocks_is_read_both_ways() {
    let live = Sealer::new(["x = 4"]);
    let run_together = reply(vec![text("so x "), text("= 4.")]);
    let block_by_block = reply(vec![text("so x ="), text("4.")]);
    for response in [&run_together, &block_by_block] {
        assert!(
            reply_leaks(&live, &step_of(response)),
            "{:?}",
            response.content
        );
    }
    let apart = reply(vec![text("x is"), text("unknown")]);
    assert!(!reply_leaks(&live, &step_of(&apart)));
}

#[test]
fn a_reply_s_calls_are_read_by_their_noesis_names() {
    let ask = json!({"kind": "recall", "prompt": "Capital?", "answer_key": "Paris"});
    let search = json!({"query": "Paris"});
    let response = reply(vec![
        call("c1", ToolName::GraphSearch, search),
        call("c2", ToolName::Ask, ask),
    ]);
    let step = step_of(&response);
    let names: Vec<&str> = step.calls.iter().map(|call| call.name.as_str()).collect();
    assert_eq!(names, ["noesis.graph_search", "noesis.ask"]);
    assert!(
        reply_leaks(&Sealer::default(), &step),
        "the search names the key its own reply poses"
    );
}

#[test]
fn a_kept_reply_drops_only_its_thinking() {
    let ask = json!({"kind": "recall", "prompt": "Capital?", "answer_key": "Paris"});
    let thinking = ContentBlock::Thinking {
        text: "the key is Paris".into(),
        signature: None,
        redacted: false,
    };
    let response = reply(vec![
        thinking,
        text("One question:"),
        call("c1", ToolName::Ask, ask),
    ]);
    let kept = kept_content(&response);
    assert_eq!(kept, response.content[1..].to_vec());
    assert_eq!(text_content(NEUTRAL), vec![text(NEUTRAL)]);
}

const NEUTRAL: &str = aether_noesis::teach::NEUTRAL_REPLY;

#[test]
fn a_note_joins_the_last_user_message_so_roles_alternate() {
    let mut messages = vec![Message::user_text("Teach me")];
    with_note(&mut messages, "Again, please.");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content.len(), 2);
    messages.push(Message::assistant_text("Sure."));
    with_note(&mut messages, "Once more.");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[2], Message::user_text("Once more."));
}
