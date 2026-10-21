//! Making a transcript acceptable to every provider: strict role
//! alternation, no dangling tool calls, a user turn to respond to.

use std::collections::BTreeSet;

use aether_core::model::{ContentBlock, Message, MessageRole, ToolResultBlock};

/// Repair a message list so providers accept it:
/// - consecutive same-role messages are merged;
/// - tool results without a preceding tool call are dropped;
/// - tool calls without a following result get an error result;
/// - the list starts with a user message.
pub fn sanitize(messages: Vec<Message>) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    let mut open_calls: BTreeSet<String> = BTreeSet::new();

    for mut message in messages {
        message
            .content
            .retain(|b| !matches!(b, ContentBlock::Text { text } if text.trim().is_empty()));
        if message.role == MessageRole::User {
            message.content.retain(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => open_calls.remove(tool_use_id),
                _ => true,
            });
        }
        if message.content.is_empty() {
            continue;
        }
        if message.role == MessageRole::Assistant {
            if !open_calls.is_empty() {
                out.push(interrupted_results(&open_calls));
                open_calls.clear();
            }
            for block in &message.content {
                if let ContentBlock::ToolUse { id, .. } = block {
                    open_calls.insert(id.clone());
                }
            }
        }
        match out.last_mut() {
            Some(last) if last.role == message.role => last.content.extend(message.content),
            _ => out.push(message),
        }
    }
    if !open_calls.is_empty() {
        out.push(interrupted_results(&open_calls));
    }
    if out
        .first()
        .is_some_and(|m| m.role == MessageRole::Assistant)
    {
        out.insert(0, Message::user_text("(session resumed)"));
    }
    out
}

fn interrupted_results(ids: &BTreeSet<String>) -> Message {
    Message::user_blocks(
        ids.iter()
            .map(|id| ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: vec![ToolResultBlock::Text {
                    text: "Interrupted before this tool ran (the daemon restarted). Call it again if still needed; for a wait or sleep, issue one blocking call rather than polling.".into(),
                }],
                is_error: true,
            })
            .collect(),
    )
}

/// Remove the oldest message and any assistant messages that then lead, so
/// the remaining tail starts with a user turn. Returns how many were removed.
/// Never empties the list.
pub fn drop_oldest_exchange(messages: &mut Vec<Message>) -> usize {
    if messages.len() <= 1 {
        return 0;
    }
    let mut removed = 0;
    messages.remove(0);
    removed += 1;
    while messages.len() > 1 && messages[0].role != MessageRole::User {
        messages.remove(0);
        removed += 1;
    }
    removed
}

/// Make sure the model has a user turn to answer. On a brand-new transcript
/// the turn states the objective; later it is a plain nudge. Returns true
/// when a message was appended (the caller persists it so the transcript
/// stays faithful).
pub fn ensure_ends_with_user(messages: &mut Vec<Message>, objective: Option<&str>) -> bool {
    match messages.last() {
        Some(m) if m.role == MessageRole::User => false,
        None => {
            let text = match objective.map(str::trim).filter(|o| !o.is_empty()) {
                Some(o) => format!("Your task:\n{o}"),
                None => "Begin.".to_owned(),
            };
            messages.push(Message::user_text(text));
            true
        }
        Some(_) => {
            messages.push(Message::user_text("Continue with your task."));
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_use(id: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.into(),
            name: "t".into(),
            input: json!({}),
        }
    }

    #[test]
    fn merges_consecutive_roles_and_prepends_user() {
        let msgs = vec![
            Message::assistant_text("a"),
            Message::assistant_text("b"),
            Message::user_text("c"),
            Message::user_text("d"),
        ];
        let out = sanitize(msgs);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].text(), "(session resumed)");
        assert_eq!(out[1].content.len(), 2);
        assert_eq!(out[2].content.len(), 2);
    }

    #[test]
    fn dangling_tool_use_gets_error_result() {
        let msgs = vec![
            Message::user_text("go"),
            Message::assistant_blocks(vec![tool_use("c1")]),
        ];
        let out = sanitize(msgs);
        assert_eq!(out.len(), 3);
        assert!(matches!(
            &out[2].content[0],
            ContentBlock::ToolResult { tool_use_id, is_error: true, .. } if tool_use_id == "c1"
        ));
    }

    #[test]
    fn orphan_tool_results_are_dropped() {
        let msgs = vec![
            Message::user_blocks(vec![ContentBlock::tool_result_text("nope", "x", false)]),
            Message::user_text("hello"),
        ];
        let out = sanitize(msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text(), "hello");
    }

    #[test]
    fn matched_tool_results_are_kept() {
        let msgs = vec![
            Message::user_text("go"),
            Message::assistant_blocks(vec![tool_use("c1"), tool_use("c2")]),
            Message::user_blocks(vec![
                ContentBlock::tool_result_text("c1", "r1", false),
                ContentBlock::tool_result_text("c2", "r2", false),
            ]),
            Message::assistant_text("done"),
        ];
        let out = sanitize(msgs.clone());
        assert_eq!(out, msgs);
        let mut out = out;
        assert!(ensure_ends_with_user(&mut out, Some("ship it")));
        assert_eq!(out.len(), 5);
        assert_eq!(out[4].text(), "Continue with your task.");
        assert!(!ensure_ends_with_user(&mut out, None));
        let mut fresh = Vec::new();
        assert!(ensure_ends_with_user(&mut fresh, Some("ship it")));
        assert_eq!(fresh[0].text(), "Your task:\nship it");
    }

    #[test]
    fn drop_oldest_exchange_leaves_a_user_led_tail() {
        let mut msgs = vec![
            Message::user_text("u1"),
            Message::assistant_text("a1"),
            Message::assistant_text("a1b"),
            Message::user_text("u2"),
            Message::assistant_text("a2"),
        ];
        assert_eq!(drop_oldest_exchange(&mut msgs), 3);
        assert_eq!(msgs[0].text(), "u2");
        assert_eq!(msgs.len(), 2);
        // Never empties: dropping from a two-message tail leaves the last one.
        assert_eq!(drop_oldest_exchange(&mut msgs), 1);
        assert_eq!(msgs.len(), 1);
        assert_eq!(drop_oldest_exchange(&mut msgs), 0);
        // Sanitizing a user-led tail never grows it.
        let tail = vec![Message::user_text("u"), Message::assistant_text("a")];
        assert_eq!(sanitize(tail.clone()).len(), tail.len());
    }
}
