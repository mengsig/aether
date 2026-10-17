#![cfg(test)]

use aether_client::ClientError;
use aether_core::agent::AgentStatus;
use aether_protocol::Command;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::commands::COMMANDS;
use crate::ui::bottom::popup::fuzzy_match;

/// The fixture's one agent, viewed, with `status`.
fn app_with(status: AgentStatus) -> (App, AgentId) {
    let mut app = crate::testing::app_with_transcript(&[]);
    let id = app.selected.unwrap();
    app.tree.nodes.get_mut(&id).unwrap().agent.status = status;
    (app, id)
}

fn add_child(app: &mut App, parent: AgentId, name: &str) -> AgentId {
    let mut node = app.tree.nodes[&parent].clone();
    let id = AgentId::new();
    node.agent.id = id;
    node.agent.name = name.into();
    node.agent.parent = Some(parent);
    node.children.clear();
    app.tree.nodes.insert(id, node);
    app.tree.nodes.get_mut(&parent).unwrap().children.push(id);
    id
}

fn typed(app: &mut App, text: &str) {
    app.input.insert_str(text);
}

#[test]
fn literal_placeholder_text_is_not_a_paste_even_when_typed_afterwards() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    let big = "x".repeat(1_500);
    typed(&mut app, "[Pasted 1,500 chars] ");
    app.composer_paste(&big);
    assert_eq!(app.input.pastes[0].placeholder, "[Pasted 1,500 chars #2]");
    typed(&mut app, " [Pasted 1,500 chars #2]");
    assert_eq!(
        sent(&app.composer_submit()),
        [(
            id,
            format!("[Pasted 1,500 chars] {big} [Pasted 1,500 chars #2]")
        )]
    );
}

#[test]
fn nested_placeholder_text_in_a_paste_is_expanded_once() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    let suffix = "[Pasted 1,500 chars #2]";
    let first = format!("{}{suffix}", "x".repeat(1_500 - suffix.len()));
    let second = "y".repeat(1_500);
    app.composer_paste(&first);
    app.composer_paste(&second);
    assert_eq!(
        sent(&app.composer_submit()),
        [(id, format!("{first}{second}"))]
    );
}

#[test]
fn shifted_paste_ranges_survive_kill_yank_history_cancel_and_queue_edit() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    let big = "x".repeat(1_500);
    app.composer_paste(&big);
    app.input.cursor = 0;
    typed(&mut app, "prefix ");
    app.input.cursor = app.input.text.len();
    app.composer_edit(EditOp::KillToStart);
    app.composer_edit(EditOp::Yank);
    app.open_popup(PopupKind::History);
    app.close_popup();
    assert_eq!(app.expanded_draft(), format!("prefix {big}"));
    app.transcripts
        .entry(id)
        .or_default()
        .queue
        .push("queued".into());
    app.composer_unqueue();
    assert_eq!(app.expanded_draft(), format!("queued\nprefix {big}"));
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// `(recipient, content)` of every message the effects send.
fn sent(effects: &[Effect]) -> Vec<(AgentId, String)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Request {
                command: Command::SendMessage { to, content, .. },
                ..
            } => Some((*to, content.clone())),
            _ => None,
        })
        .collect()
}

/// The kill ring's texts, oldest first.
fn ring_texts(app: &App) -> Vec<&str> {
    let ring = &app.bottom.composer.kill_ring;
    ring.iter().map(|killed| killed.text.as_str()).collect()
}

#[test]
fn kills_fill_the_ring_and_yank_inserts_the_newest() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    typed(&mut app, "hello brave world");
    app.composer_edit(EditOp::KillWordBack);
    assert_eq!(app.input.text, "hello brave ");
    app.composer_edit(EditOp::Home);
    app.composer_edit(EditOp::KillToEnd);
    assert_eq!(app.input.text, "");
    assert_eq!(ring_texts(&app), ["world", "hello brave "]);
    app.composer_edit(EditOp::Yank);
    assert_eq!(app.input.text, "hello brave ");
    app.composer_edit(EditOp::KillToStart);
    assert_eq!(app.input.text, "");
    assert_eq!(app.bottom.composer.kill_ring.len(), 3);
}

#[test]
fn kill_to_end_at_a_line_end_joins_the_next_line() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    typed(&mut app, "one\ntwo");
    app.input.cursor = 3;
    app.composer_edit(EditOp::KillToEnd);
    assert_eq!(app.input.text, "onetwo");
    assert_eq!(ring_texts(&app), ["\n"]);
}

#[test]
fn empty_kills_do_not_enter_the_ring_and_the_ring_is_bounded() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    app.composer_edit(EditOp::KillWordBack);
    assert!(app.bottom.composer.kill_ring.is_empty());
    for i in 0..KILL_RING_MAX + 5 {
        typed(&mut app, &format!("w{i}"));
        app.composer_edit(EditOp::KillWordBack);
    }
    let ring = ring_texts(&app);
    assert_eq!(ring.len(), KILL_RING_MAX);
    assert_eq!(ring.last().unwrap(), &format!("w{}", KILL_RING_MAX + 4));
}

#[test]
fn the_kill_ring_survives_submit() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    typed(&mut app, "keep this");
    app.composer_edit(EditOp::KillWordBack);
    let effects = app.composer_submit();
    assert_eq!(sent(&effects).len(), 1);
    app.composer_edit(EditOp::Yank);
    assert_eq!(app.input.text, "this");
}

#[test]
fn a_large_paste_becomes_a_placeholder_and_expands_at_submit() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    let big = "x".repeat(2_652);
    typed(&mut app, "see ");
    app.composer_paste(&big);
    assert_eq!(app.input.text, "see [Pasted 2,652 chars]");
    app.composer_paste(&"y".repeat(2_652));
    assert_eq!(
        app.input.text,
        "see [Pasted 2,652 chars][Pasted 2,652 chars #2]"
    );
    let effects = app.composer_submit();
    let expected = format!("see {big}{}", "y".repeat(2_652));
    assert_eq!(sent(&effects), [(id, expected.clone())]);
    assert!(app.input.pastes.is_empty());
    assert_eq!(app.input.history().last(), Some(&expected));
}

#[test]
fn a_small_paste_goes_in_as_text_with_newlines_normalised() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    app.composer_paste("a\r\nb\rc");
    assert_eq!(app.input.text, "a\nb\nc");
    let at_threshold = "z".repeat(PASTE_PLACEHOLDER_CHARS);
    app.composer_paste(&at_threshold);
    assert!(app.input.pastes.is_empty());
}

#[test]
fn a_damaged_placeholder_is_refused_and_the_draft_kept() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    app.composer_paste(&"x".repeat(1_500));
    // No key does this any more; the guard is for whatever else might.
    app.input.backspace();
    typed(&mut app, " ok");
    assert!(app.composer_submit().is_empty());
    assert!(app.composer_queue().is_empty());
    assert_eq!(app.input.text, "[Pasted 1,500 chars ok");
    let flash = app.live_flash().unwrap();
    assert!(
        flash.contains("not sent") && flash.contains("[Pasted 1,500 chars]"),
        "{flash}"
    );

    app.tree.nodes.get_mut(&id).unwrap().agent.status = AgentStatus::Running;
    assert!(app.composer_queue().is_empty());
    assert!(app.transcripts.get(&id).is_none_or(|t| t.queue.is_empty()));
    assert_eq!(app.input.text, "[Pasted 1,500 chars ok");
}

#[test]
fn left_backspace_enter_on_a_placeholder_sends_the_paste_or_nothing_of_it() {
    // The sweep repro: paste, ←, Backspace, Enter sent a broken label.
    let (mut app, id) = app_with(AgentStatus::Ready);
    let big = "x".repeat(1_260);
    app.dispatch_paste(&big);
    app.dispatch_key(key(KeyCode::Left));
    assert_eq!(app.input.cursor, 0, "← steps over the placeholder");
    app.dispatch_key(key(KeyCode::Backspace));
    assert_eq!(app.input.text, "[Pasted 1,260 chars]");
    let effects = app.dispatch_key(key(KeyCode::Enter));
    assert_eq!(sent(&effects), [(id, big)]);
}

#[test]
fn the_cursor_steps_over_a_placeholder_and_typing_lands_beside_it() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    typed(&mut app, "a ");
    app.composer_paste(&"x".repeat(1_500));
    typed(&mut app, " b");
    let (start, end) = (2, 2 + "[Pasted 1,500 chars]".len());
    app.input.cursor = end;
    app.dispatch_key(key(KeyCode::Left));
    assert_eq!(app.input.cursor, start);
    app.dispatch_key(key(KeyCode::Right));
    assert_eq!(app.input.cursor, end);
    app.input.cursor = end + 1;
    app.composer_edit(EditOp::WordLeft);
    assert_eq!(app.input.cursor, start);
    app.composer_edit(EditOp::WordRight);
    assert_eq!(app.input.cursor, end);
    app.dispatch_key(key(KeyCode::Left));
    app.dispatch_key(key(KeyCode::Char('z')));
    assert_eq!(app.input.text, "a z[Pasted 1,500 chars] b");
}

#[test]
fn backspace_and_delete_inside_a_placeholder_remove_it_whole() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    for (op, at) in [
        (EditOp::Backspace, 5),
        (EditOp::Delete, 5),
        (EditOp::Delete, 1),
    ] {
        typed(&mut app, "a");
        app.composer_paste(&"x".repeat(1_500));
        typed(&mut app, "b");
        app.input.cursor = at;
        app.composer_edit(op);
        assert_eq!(app.input.text, "ab", "{op:?} at {at}");
        assert!(app.input.pastes.is_empty());
        app.input.set_text("");
    }
}

#[test]
fn a_word_kill_takes_the_whole_placeholder_and_yank_brings_its_paste_back() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    let big = "x".repeat(1_500);
    typed(&mut app, "see ");
    app.composer_paste(&big);
    app.composer_edit(EditOp::KillWordBack);
    assert_eq!(app.input.text, "see ");
    assert!(app.input.pastes.is_empty());
    app.composer_edit(EditOp::Yank);
    app.composer_edit(EditOp::Yank);
    assert_eq!(
        app.input.text,
        "see [Pasted 1,500 chars][Pasted 1,500 chars #2]"
    );
    let effects = app.composer_submit();
    assert_eq!(sent(&effects), [(id, format!("see {big}{big}"))]);
}

#[test]
fn history_replacing_a_draft_with_a_paste_is_not_a_damaged_paste() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    // Wide enough that the placeholder is one row and ↑ walks history.
    app.size = (120, 40);
    typed(&mut app, "earlier");
    app.composer_submit();
    app.composer_paste(&"x".repeat(1_500));
    app.dispatch_key(key(KeyCode::Up));
    assert_eq!(app.input.text, "earlier");
    let effects = app.dispatch_key(key(KeyCode::Enter));
    assert_eq!(sent(&effects), [(id, "earlier".to_owned())]);

    app.composer_paste(&"y".repeat(1_500));
    app.dispatch_key(ctrl('r'));
    app.dispatch_key(key(KeyCode::Enter));
    assert_eq!(app.input.text, "earlier");
    let effects = app.dispatch_key(key(KeyCode::Enter));
    assert_eq!(sent(&effects), [(id, "earlier".to_owned())]);
}

#[test]
fn backspace_and_delete_remove_a_placeholder_whole() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    typed(&mut app, "a");
    app.composer_paste(&"x".repeat(1_500));
    typed(&mut app, "b");
    app.composer_edit(EditOp::Backspace);
    app.composer_edit(EditOp::Backspace);
    assert_eq!(app.input.text, "a");
    assert!(app.input.pastes.is_empty());

    app.composer_paste(&"y".repeat(1_500));
    app.input.cursor = 1;
    app.composer_edit(EditOp::Delete);
    app.composer_edit(EditOp::Delete);
    assert_eq!(app.input.text, "a");
    app.composer_edit(EditOp::Backspace);
    assert_eq!(app.input.text, "");
    assert!(app.bottom.composer.kill_ring.is_empty());
}

#[test]
fn enter_steers_a_running_agent_and_shows_a_preview() {
    let (mut app, id) = app_with(AgentStatus::Running);
    typed(&mut app, "also update the docs");
    let effects = app.composer_submit();
    assert_eq!(sent(&effects), [(id, "also update the docs".to_owned())]);
    let steering = &app.transcripts[&id].steering;
    assert_eq!(steering.len(), 1);
    assert_eq!(steering[0].text, "also update the docs");
    assert!(app.input.text.is_empty());
}

#[test]
fn enter_on_an_idle_agent_sends_without_a_steer_preview() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    typed(&mut app, "hello");
    let effects = app.composer_submit();
    assert_eq!(sent(&effects), [(id, "hello".to_owned())]);
    assert!(
        app.transcripts
            .get(&id)
            .is_none_or(|t| t.steering.is_empty())
    );
}

#[test]
fn tab_queues_while_running_and_sends_when_idle() {
    let (mut app, id) = app_with(AgentStatus::Running);
    typed(&mut app, "  then add a changelog entry ");
    assert!(app.composer_queue().is_empty());
    assert_eq!(app.transcripts[&id].queue, ["then add a changelog entry"]);
    assert!(app.transcripts[&id].steering.is_empty());
    assert!(app.input.text.is_empty());
    assert_eq!(
        app.input.history().last().map(String::as_str),
        Some("  then add a changelog entry ")
    );

    app.tree.nodes.get_mut(&id).unwrap().agent.status = AgentStatus::Ready;
    typed(&mut app, "now");
    let effects = app.composer_queue();
    assert_eq!(sent(&effects), [(id, "now".to_owned())]);
    assert_eq!(app.transcripts[&id].queue.len(), 1);
}

#[test]
fn tab_with_an_empty_draft_or_a_command_never_queues() {
    let (mut app, id) = app_with(AgentStatus::Running);
    assert!(app.composer_queue().is_empty());
    typed(&mut app, "/clear");
    app.composer_queue();
    assert!(app.input.text.is_empty());
    assert!(app.transcripts.get(&id).is_none_or(|t| t.queue.is_empty()));
}

#[test]
fn shift_left_pulls_the_last_queued_message_back_above_the_draft() {
    let (mut app, id) = app_with(AgentStatus::Running);
    app.transcripts.entry(id).or_default().queue = vec!["first".into(), "second".into()];
    app.composer_unqueue();
    assert_eq!(app.input.text, "second");
    assert_eq!(app.input.cursor, "second".len());
    typed(&mut app, "!");
    app.input.cursor = app.input.text.len();
    app.composer_unqueue();
    assert_eq!(app.input.text, "first\nsecond!");
    assert_eq!(app.input.cursor, "first".len());
    assert!(app.transcripts[&id].queue.is_empty());
    app.composer_unqueue();
    assert_eq!(app.input.text, "first\nsecond!");
}

/// The idempotency key of every queued send in `effects`.
fn queue_keys(effects: &[Effect]) -> Vec<Option<String>> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Request {
                command:
                    Command::SendMessage {
                        idempotency_key, ..
                    },
                then: Then::QueuedSent(_),
            } => Some(idempotency_key.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_finished_turn_sends_the_oldest_queued_message_only() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    app.transcripts.entry(id).or_default().queue = vec!["a".into(), "b".into()];
    let ready = AgentStatusKind::Ready;
    assert_eq!(
        sent(&app.composer_turn_ended(id, ready, 1)),
        [(id, "a".to_owned())]
    );
    // One at a time: the head stays queued until the daemon has it.
    assert_eq!(app.transcripts[&id].queue, ["a", "b"]);
    assert!(app.composer_turn_ended(id, ready, 2).is_empty());
    app.queued_sent(id, None);
    assert_eq!(app.transcripts[&id].queue, ["b"]);
    assert_eq!(
        sent(&app.composer_turn_ended(id, ready, 3)),
        [(id, "b".to_owned())]
    );
    app.queued_sent(id, None);
    assert!(app.composer_turn_ended(id, ready, 4).is_empty());
    assert!(app.composer_turn_ended(AgentId::new(), ready, 5).is_empty());
}

#[test]
fn a_queue_never_drains_to_a_finished_agent() {
    for next in [
        AgentStatusKind::Completed,
        AgentStatusKind::Failed,
        AgentStatusKind::Cancelled,
    ] {
        let (mut app, id) = app_with(AgentStatus::Running);
        app.transcripts.entry(id).or_default().queue = vec!["then run clippy".into()];
        assert!(app.composer_turn_ended(id, next, 7).is_empty(), "{next:?}");
        assert_eq!(app.transcripts[&id].queue, ["then run clippy"]);
        let flash = app.live_flash().unwrap_or_default().to_owned();
        assert!(flash.contains("kept, not sent"), "{flash}");
    }
}

#[test]
fn a_failed_queued_send_puts_nothing_away() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    app.transcripts.entry(id).or_default().queue = vec!["a".into(), "b".into()];
    let effects = app.composer_turn_ended(id, AgentStatusKind::Ready, 9);
    assert_eq!(sent(&effects), [(id, "a".to_owned())]);
    app.queued_sent(id, Some(ClientError::Disconnected));
    assert_eq!(app.transcripts[&id].queue, ["a", "b"]);
    assert!(
        app.live_flash()
            .unwrap_or_default()
            .contains("stays queued")
    );
    // The next turn end tries again.
    let again = app.composer_turn_ended(id, AgentStatusKind::Ready, 10);
    assert_eq!(sent(&again), [(id, "a".to_owned())]);
}

#[test]
fn every_client_keys_a_queued_send_by_the_turn_end_it_answers() {
    let (mut a, id) = app_with(AgentStatus::Ready);
    let mut b = App::new(crate::app::TuiOptions::default());
    for app in [&mut a, &mut b] {
        app.transcripts.entry(id).or_default().queue = vec!["later".into()];
    }
    let from_a = queue_keys(&a.composer_turn_ended(id, AgentStatusKind::Ready, 42));
    let from_b = queue_keys(&b.composer_turn_ended(id, AgentStatusKind::Ready, 42));
    assert_eq!(from_a, from_b, "the daemon must see one submission");
    assert!(from_a[0].as_deref().is_some_and(|key| key.ends_with("-42")));
}

#[test]
fn the_head_being_sent_cannot_be_pulled_back() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    app.transcripts.entry(id).or_default().queue = vec!["a".into()];
    app.composer_turn_ended(id, AgentStatusKind::Ready, 1);
    app.composer_unqueue();
    assert!(app.input.text.is_empty());
    assert_eq!(app.transcripts[&id].queue, ["a"]);
}

#[test]
fn saving_a_queue_leaves_other_clients_agents_on_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let other = AgentId::new().to_string();
    let mut prefs = crate::prefs::UiPrefs::default();
    prefs.queued.insert(other.clone(), vec!["theirs".into()]);
    prefs.save(dir.path()).expect("seed ui.toml");
    let mut app = crate::testing::app_with_transcript(&[]);
    app.options.prefs_dir = Some(dir.path().to_path_buf());
    let id = app.selected.unwrap();
    app.tree.nodes.get_mut(&id).unwrap().agent.status = AgentStatus::Running;
    typed(&mut app, "mine");
    app.composer_queue();
    let saved = crate::prefs::UiPrefs::load(dir.path()).expect("load");
    assert_eq!(saved.queued[&other], ["theirs"]);
    assert_eq!(saved.queued[&id.to_string()], ["mine"]);
    app.composer_unqueue();
    let saved = crate::prefs::UiPrefs::load(dir.path()).expect("load");
    assert_eq!(saved.queued[&other], ["theirs"]);
    assert!(!saved.queued.contains_key(&id.to_string()));
}

#[test]
fn saved_queues_are_restored_per_agent() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    app.options.queued.insert(id, vec!["later".into()]);
    app.restore_queues();
    app.restore_queues();
    assert_eq!(app.transcripts[&id].queue, ["later"]);
}

#[test]
fn agent_mentions_expand_at_submit_without_rerouting() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    let researcher = add_child(&mut app, id, "researcher");
    typed(
        &mut app,
        "@researcher found it; mail a@researcher and @nobody",
    );
    let effects = app.composer_submit();
    let expected = format!("@researcher ({researcher}) found it; mail a@researcher and @nobody");
    assert_eq!(sent(&effects), [(id, expected)]);
}

#[test]
fn forwarded_slash_commands_keep_their_mentions_as_typed() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    add_child(&mut app, id, "researcher");
    typed(&mut app, "/answer 3 @researcher");
    let effects = app.composer_submit();
    assert_eq!(sent(&effects), [(id, "/answer 3 @researcher".to_owned())]);

    let (mut app, id) = app_with(AgentStatus::Running);
    add_child(&mut app, id, "researcher");
    typed(&mut app, "/answer 3 @researcher");
    assert!(app.composer_queue().is_empty());
    assert_eq!(app.transcripts[&id].queue, ["/answer 3 @researcher"]);

    for status in [AgentStatus::Ready, AgentStatus::Running] {
        let (mut app, id) = app_with(status.clone());
        add_child(&mut app, id, "researcher");
        let prefix = format!("/answer 3 {}", "x".repeat(PASTE_PLACEHOLDER_CHARS));
        app.composer_paste(&prefix);
        assert_eq!(app.input.pastes.len(), 1, "the prefix is a placeholder");
        typed(&mut app, " @researcher");
        let expected = format!("{prefix} @researcher");
        let effects = app.composer_queue();
        let delivered = match status {
            AgentStatus::Running => app.transcripts[&id].queue.clone(),
            _ => sent(&effects).into_iter().map(|(_, text)| text).collect(),
        };
        assert_eq!(delivered, [expected], "{status:?}");
    }
}

#[test]
fn mentions_of_a_shared_name_or_already_expanded_stay_as_typed() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    let first = add_child(&mut app, id, "tester");
    add_child(&mut app, id, "tester");
    let resolve = |name: &str| app.unique_agent(name);
    assert_eq!(expand_mentions("ask @tester", resolve), "ask @tester");
    let done = format!("@x ({first})");
    assert_eq!(expand_mentions(&done, |_| Some(first)), done);
    assert_eq!(
        expand_mentions("@x, hi", |_| Some(first)),
        format!("@x ({first}), hi")
    );
}

#[test]
fn slash_opens_on_an_empty_draft_and_lists_in_registry_order() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    app.open_popup(PopupKind::Slash);
    assert_eq!(app.input.text, "/");
    let popup = app.bottom.popup.as_ref().unwrap();
    let names: Vec<_> = popup::items(popup, &app)
        .into_iter()
        .map(|item| match item {
            PopupItem::Command { name, .. } => name.to_owned(),
            PopupItem::Mode { name, .. } => name,
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    let registry: Vec<_> = COMMANDS
        .iter()
        .map(|c| c.name.to_owned())
        .chain(app.view_modes.modes.keys().cloned())
        .collect();
    assert_eq!(names, registry);
}

#[test]
fn slash_filter_ranks_by_match_class_then_registry_order() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    app.open_popup(PopupKind::Slash);
    for c in "re".chars() {
        assert_eq!(
            app.popup_key(key(KeyCode::Char(c))),
            PopupKeyOutcome::Consumed
        );
    }
    let popup = app.bottom.popup.as_ref().unwrap();
    assert_eq!(popup.query, "re");
    let ranks: Vec<_> = popup::items(popup, &app)
        .into_iter()
        .map(|item| match item {
            PopupItem::Command { name, .. } => (
                fuzzy_match(name, "re").unwrap(),
                COMMANDS.iter().position(|c| c.name == name).unwrap(),
            ),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert!(!ranks.is_empty());
    assert!(ranks.windows(2).all(|w| w[0] <= w[1]), "{ranks:?}");
    let matching = COMMANDS
        .iter()
        .filter(|c| fuzzy_match(c.name, "re").is_some())
        .count();
    assert_eq!(ranks.len(), matching);
}

#[test]
fn a_typed_hidden_alias_leads_with_its_command() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    app.open_popup(PopupKind::Slash);
    for c in "cd".chars() {
        assert_eq!(
            app.popup_key(key(KeyCode::Char(c))),
            PopupKeyOutcome::Consumed
        );
    }
    let popup = app.bottom.popup.as_ref().unwrap();
    let first = popup::items(popup, &app).into_iter().next();
    assert!(
        matches!(
            first,
            Some(PopupItem::Command {
                name: "workspace",
                ..
            })
        ),
        "{first:?}"
    );
}

#[test]
fn fuzzy_match_classes() {
    assert_eq!(fuzzy_match("model", "MODEL"), Some(popup::Match::Exact));
    assert_eq!(fuzzy_match("models", "mod"), Some(popup::Match::Prefix));
    assert_eq!(
        fuzzy_match("approvals", "rov"),
        Some(popup::Match::Substring)
    );
    assert_eq!(fuzzy_match("remember", "rmb"), Some(popup::Match::Fuzzy));
    assert_eq!(fuzzy_match("remember", "bmr"), None);
    assert_eq!(fuzzy_match("x", ""), Some(popup::Match::Prefix));
}

#[test]
fn slash_tab_completes_enter_runs_and_space_closes() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    let first = COMMANDS[0];
    app.open_popup(PopupKind::Slash);
    assert_eq!(app.popup_key(key(KeyCode::Tab)), PopupKeyOutcome::Consumed);
    assert_eq!(app.input.text, format!("/{} ", first.name));
    assert!(app.bottom.popup.is_none());

    app.input.set_text("");
    app.open_popup(PopupKind::Slash);
    let runs_bare = !first.usage.starts_with('<');
    assert_eq!(
        app.popup_key(key(KeyCode::Enter)),
        if runs_bare {
            PopupKeyOutcome::SubmitCommand
        } else {
            PopupKeyOutcome::Consumed
        }
    );
    assert!(app.input.text.starts_with(&format!("/{}", first.name)));

    app.input.set_text("");
    app.open_popup(PopupKind::Slash);
    app.popup_key(key(KeyCode::Char('m')));
    app.popup_key(key(KeyCode::Char(' ')));
    assert!(app.bottom.popup.is_none());
    assert_eq!(app.input.text, "/m ");
}

#[test]
fn enter_on_a_command_that_needs_an_argument_completes_it() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    let Some(spec) = COMMANDS.iter().find(|c| c.usage.starts_with('<')) else {
        return;
    };
    app.open_popup(PopupKind::Slash);
    for c in spec.name.chars() {
        app.popup_key(key(KeyCode::Char(c)));
    }
    assert_eq!(
        app.popup_key(key(KeyCode::Enter)),
        PopupKeyOutcome::Consumed
    );
    assert_eq!(app.input.text, format!("/{} ", spec.name));
}

#[test]
fn slash_popup_navigation_esc_and_backspace() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    app.open_popup(PopupKind::Slash);
    assert_eq!(app.popup_key(key(KeyCode::Up)), PopupKeyOutcome::Consumed);
    assert_eq!(app.bottom.popup.as_ref().unwrap().cursor, 0);
    assert_eq!(app.popup_key(key(KeyCode::Down)), PopupKeyOutcome::Consumed);
    assert_eq!(app.bottom.popup.as_ref().unwrap().cursor, 1);
    assert_eq!(app.popup_key(key(KeyCode::Esc)), PopupKeyOutcome::Consumed);
    assert!(app.bottom.popup.is_none());
    assert_eq!(app.input.text, "/");

    app.open_popup(PopupKind::Slash);
    assert_eq!(app.input.text, "/");
    assert_eq!(
        app.popup_key(key(KeyCode::Backspace)),
        PopupKeyOutcome::Consumed
    );
    assert!(app.bottom.popup.is_none());
    assert_eq!(
        app.popup_key(key(KeyCode::Char('x'))),
        PopupKeyOutcome::Pass
    );
}

#[test]
fn slash_does_not_open_mid_draft() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    typed(&mut app, "hello");
    app.open_popup(PopupKind::Slash);
    assert!(app.bottom.popup.is_none());
    assert_eq!(app.input.text, "hello");
}

#[test]
fn agent_mentions_list_others_children_first_and_insert_the_name() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    add_child(&mut app, id, "researcher");
    add_child(&mut app, id, "tester");
    typed(&mut app, "ask ");
    app.open_popup(PopupKind::Mention(MentionTab::Agents));
    assert_eq!(app.input.text, "ask @");
    let popup = app.bottom.popup.as_ref().unwrap();
    let names: Vec<_> = popup::items(popup, &app)
        .into_iter()
        .map(|item| match item {
            PopupItem::Agent { name, .. } => name,
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(names, ["researcher", "tester"]);

    for c in "tst".chars() {
        app.popup_key(key(KeyCode::Char(c)));
    }
    assert_eq!(
        app.popup_key(key(KeyCode::Enter)),
        PopupKeyOutcome::Consumed
    );
    assert_eq!(app.input.text, "ask @tester ");
    assert!(app.bottom.popup.is_none());
}

#[test]
fn mention_tabs_switch_with_left_and_right() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    app.open_popup(PopupKind::Mention(MentionTab::Agents));
    assert_eq!(app.popup_key(key(KeyCode::Left)), PopupKeyOutcome::Consumed);
    assert_eq!(
        app.bottom.popup.as_ref().unwrap().kind,
        PopupKind::Mention(MentionTab::Files)
    );
    assert_eq!(
        app.popup_key(key(KeyCode::Right)),
        PopupKeyOutcome::Consumed
    );
    assert_eq!(
        app.bottom.popup.as_ref().unwrap().kind,
        PopupKind::Mention(MentionTab::Agents)
    );
}

#[test]
fn file_mentions_list_the_workspace_and_insert_a_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src/ui")).unwrap();
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    for file in [
        "src/ui/composer.rs",
        "src/main.rs",
        "README.md",
        "target/debug/x",
        ".git/HEAD",
    ] {
        std::fs::write(root.join(file), "").unwrap();
    }
    let (mut app, id) = app_with(AgentStatus::Ready);
    app.tree
        .nodes
        .get_mut(&id)
        .unwrap()
        .agent
        .scope
        .workspace_root = Some(root.to_path_buf());
    app.open_popup(PopupKind::Mention(MentionTab::Files));
    let popup = app.bottom.popup.as_ref().unwrap();
    assert_eq!(
        popup.files,
        ["README.md", "src/main.rs", "src/ui/composer.rs"]
    );
    for c in "comp".chars() {
        app.popup_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.popup_key(key(KeyCode::Tab)), PopupKeyOutcome::Consumed);
    assert_eq!(app.input.text, "src/ui/composer.rs ");
}

#[test]
fn an_unreadable_workspace_is_flashed() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    let missing = std::env::temp_dir().join(format!("aether-missing-{id}"));
    app.tree
        .nodes
        .get_mut(&id)
        .unwrap()
        .agent
        .scope
        .workspace_root = Some(missing);
    app.open_popup(PopupKind::Mention(MentionTab::Files));
    assert!(app.bottom.popup.as_ref().unwrap().files.is_empty());
    assert!(
        app.live_flash()
            .unwrap()
            .starts_with("can't list workspace files")
    );
}

#[test]
fn mention_tokens_need_a_boundary_and_no_whitespace() {
    assert_eq!(mention_start("@ab", 3), Some(0));
    assert_eq!(mention_start("hi @ab", 6), Some(3));
    assert_eq!(mention_start("a@ab", 4), None);
    assert_eq!(mention_start("@ab cd", 6), None);
    assert_eq!(slash_query("/mo"), Some("mo"));
    assert_eq!(slash_query("/model x"), None);
    assert_eq!(slash_query("mo"), None);
}

fn history_app() -> App {
    let (mut app, _) = app_with(AgentStatus::Ready);
    app.input.history = vec!["fix tests".into(), "add docs".into(), "fix lint".into()];
    typed(&mut app, "wip");
    app
}

#[test]
fn history_search_previews_matches_and_esc_restores_the_draft() {
    let mut app = history_app();
    app.open_popup(PopupKind::History);
    assert_eq!(app.input.text, "fix lint");
    for c in "fix".chars() {
        assert_eq!(
            app.popup_key(key(KeyCode::Char(c))),
            PopupKeyOutcome::Consumed
        );
    }
    assert_eq!(app.input.text, "fix lint");
    assert_eq!(app.popup_key(ctrl('r')), PopupKeyOutcome::Consumed);
    assert_eq!(app.input.text, "fix tests");
    assert_eq!(app.popup_key(ctrl('r')), PopupKeyOutcome::Consumed);
    assert_eq!(app.input.text, "fix tests");
    assert_eq!(
        app.popup_key(key(KeyCode::Char('z'))),
        PopupKeyOutcome::Consumed
    );
    assert_eq!(app.input.text, "wip");
    assert_eq!(app.popup_key(key(KeyCode::Esc)), PopupKeyOutcome::Consumed);
    assert_eq!(app.input.text, "wip");
    assert!(app.bottom.popup.is_none());
}

#[test]
fn history_search_enter_accepts_without_sending() {
    let mut app = history_app();
    app.open_popup(PopupKind::History);
    app.popup_key(key(KeyCode::Char('d')));
    assert_eq!(
        app.popup_key(key(KeyCode::Enter)),
        PopupKeyOutcome::Consumed
    );
    assert_eq!(app.input.text, "add docs");
    assert!(app.bottom.popup.is_none());
}

#[test]
fn history_search_ctrl_c_and_close_popup_restore_too() {
    let mut app = history_app();
    app.open_popup(PopupKind::History);
    assert_eq!(app.popup_key(ctrl('c')), PopupKeyOutcome::Consumed);
    assert_eq!(app.input.text, "wip");
    app.open_popup(PopupKind::History);
    app.close_popup();
    assert_eq!(app.input.text, "wip");
}

#[test]
fn shortcuts_and_find_close_popups_and_each_other() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    app.open_popup(PopupKind::Slash);
    app.toggle_shortcuts();
    assert!(app.bottom.shortcuts && app.bottom.popup.is_none());
    app.open_find();
    assert!(!app.bottom.shortcuts && app.bottom.find.is_some());
    app.toggle_shortcuts();
    assert!(app.bottom.find.is_none());
    app.toggle_shortcuts();
    assert!(!app.bottom.shortcuts);
}

#[test]
fn find_keys_edit_the_query_step_and_close() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    app.open_find();
    assert_eq!(
        app.popup_key(key(KeyCode::Char('x'))),
        PopupKeyOutcome::Consumed
    );
    assert_eq!(app.bottom.find.as_ref().unwrap().query, "x");
    assert_eq!(
        app.popup_key(key(KeyCode::Backspace)),
        PopupKeyOutcome::Consumed
    );
    assert_eq!(app.bottom.find.as_ref().unwrap().query, "");
    assert_eq!(
        app.popup_key(key(KeyCode::Enter)),
        PopupKeyOutcome::Consumed
    );
    assert_eq!(app.popup_key(key(KeyCode::F(2))), PopupKeyOutcome::Pass);
    assert_eq!(app.popup_key(key(KeyCode::Esc)), PopupKeyOutcome::Consumed);
    assert!(app.bottom.find.is_none());
    assert_eq!(app.popup_key(key(KeyCode::Esc)), PopupKeyOutcome::Pass);
}

#[test]
fn a_find_step_reveals_the_current_match() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    app.bottom.find = Some(FindBar {
        query: "x".into(),
        matches: vec![
            crate::app::transcript::CellId(3),
            crate::app::transcript::CellId(7),
        ],
        current: Some(0),
        ..FindBar::default()
    });
    app.popup_key(key(KeyCode::Down));
    let last = app.scroll_log.since(0).last().copied().unwrap();
    assert_eq!(last.agent, id);
    assert_eq!(
        last.intent,
        ScrollIntent::Reveal(crate::app::transcript::CellId(7))
    );
    app.popup_key(key(KeyCode::Down));
    let last = app.scroll_log.since(0).last().copied().unwrap();
    assert_eq!(
        last.intent,
        ScrollIntent::Reveal(crate::app::transcript::CellId(3))
    );
}

/// `ls` with 20 files: the compact preview shows only the first 2 and last 3.
fn push_listing(app: &mut App, id: AgentId) -> crate::app::transcript::CellId {
    use crate::app::transcript::{CallStatus, CellModel};
    let output = (1..=20)
        .map(|i| format!("ux-file-{i:02}.txt"))
        .collect::<Vec<_>>()
        .join("\n");
    app.transcripts.get_mut(&id).unwrap().push_for_test(
        aether_core::time::now(),
        CellModel::Exec {
            call: None,
            command: "ls".into(),
            status: CallStatus::Succeeded,
            output,
            exit: Some(0),
            duration_ms: Some(10),
            is_search: false,
            artifacts: Vec::new(),
        },
    )
}

#[test]
fn find_reaches_tool_output_the_preview_hides_and_turns_details_on() {
    let (mut app, id) = app_with(AgentStatus::Ready);
    let cell = push_listing(&mut app, id);
    app.open_find();
    for c in "ux-file-20".chars() {
        app.popup_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.bottom.find.as_ref().unwrap().matches, [cell]);
    assert!(!app.details, "a hit the preview shows needs no details");

    app.popup_key(ctrl('u'));
    for c in "ux-file-03".chars() {
        app.popup_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.bottom.find.as_ref().unwrap().matches, [cell]);
    assert!(app.details, "the hit is only in details");
    let flash = app.live_flash().unwrap_or_default().to_owned();
    assert!(flash.contains("details on"), "{flash}");
    let last = app.scroll_log.since(0).last().copied().unwrap();
    assert_eq!(last.intent, ScrollIntent::Reveal(cell));
}

/// The sweep's repro: a reply finishing while find is open joins its matches unasked.
#[test]
fn an_open_find_picks_up_new_replies_and_keeps_its_place() {
    use aether_core::model::MessageRole;
    let (mut app, id) = app_with(AgentStatus::Running);
    crate::testing::push_message(&mut app, id, MessageRole::Assistant, "count: 199");
    crate::testing::push_message(&mut app, id, MessageRole::Assistant, "and again 199");
    app.open_find();
    for c in "199".chars() {
        app.popup_key(key(KeyCode::Char(c)));
    }
    app.popup_key(key(KeyCode::Down));
    let bar = app.bottom.find.as_ref().unwrap();
    let (current, matches) = (bar.matches[1], bar.matches.len());
    assert_eq!(matches, 2);
    let reveals = app.scroll_log.since(0).count();

    crate::testing::push_message(&mut app, id, MessageRole::Assistant, "1 … 199, done");
    crate::testing::push_message(&mut app, id, MessageRole::Assistant, "no hit here");
    let bar = app.bottom.find.as_ref().unwrap();
    assert_eq!(bar.matches.len(), 3, "the finished reply joins the matches");
    assert_eq!(bar.current.map(|i| bar.matches[i]), Some(current));
    assert_eq!(
        app.scroll_log.since(0).count(),
        reveals,
        "a passive refresh never scrolls"
    );
}

/// Streaming and other agents' events leave the viewed cells alone, so find skips its walk.
#[test]
fn an_open_find_rescans_only_when_the_viewed_cells_change() {
    use aether_core::model::MessageRole;
    use aether_protocol::Delta;
    let (mut app, id) = app_with(AgentStatus::Running);
    let other = add_child(&mut app, id, "reviewer");
    crate::testing::push_message(&mut app, id, MessageRole::Assistant, "count: 199");
    app.open_find();
    for c in "199".chars() {
        app.popup_key(key(KeyCode::Char(c)));
    }
    let scans = find::scans(app.bottom.find.as_ref().unwrap());

    let invocation = aether_core::id::InvocationId::new();
    for agent in [id, other] {
        app.reduce(crate::app::AppEvent::Frame(
            crate::app::SubscriptionFrame::Delta {
                agent,
                invocation,
                delta: Delta::Text { text: "199".into() },
            },
        ));
    }
    crate::testing::push_message(&mut app, other, MessageRole::Assistant, "199 elsewhere");
    assert_eq!(find::scans(app.bottom.find.as_ref().unwrap()), scans);

    crate::testing::push_message(&mut app, id, MessageRole::Assistant, "199 again");
    let bar = app.bottom.find.as_ref().unwrap();
    assert_eq!(find::scans(bar), scans + 1);
    assert_eq!(bar.matches.len(), 2);
}

/// Keys that edit or send the draft when the composer has focus.
fn draft_keys() -> Vec<KeyEvent> {
    let mut keys: Vec<KeyEvent> = "wkyaej".chars().map(ctrl).collect();
    keys.extend(
        [
            KeyCode::Home,
            KeyCode::End,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Delete,
            KeyCode::Tab,
        ]
        .map(key),
    );
    keys.push(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT));
    keys.push(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    keys.push(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    keys
}

#[test]
fn editing_keys_in_the_find_bar_never_touch_the_draft() {
    let (mut app, _) = app_with(AgentStatus::Running);
    typed(&mut app, "UNSENT IMPORTANT DRAFT");
    app.dispatch_key(ctrl('f'));
    for c in "hello".chars() {
        app.dispatch_key(key(KeyCode::Char(c)));
    }
    assert!(app.dispatch_key(ctrl('u')).is_empty());
    assert_eq!(
        app.bottom.find.as_ref().unwrap().query,
        "",
        "ctrl+u clears the query"
    );
    for chord in draft_keys() {
        let effects = app.dispatch_key(chord);
        assert!(effects.is_empty(), "{chord:?}: {effects:?}");
        assert_eq!(app.input.text, "UNSENT IMPORTANT DRAFT", "{chord:?}");
    }
    app.dispatch_key(key(KeyCode::Esc));
    assert!(app.bottom.find.is_none());
    assert_eq!(app.input.text, "UNSENT IMPORTANT DRAFT");
    assert!(app.bottom.composer.kill_ring.is_empty());
}

#[test]
fn editing_keys_in_history_search_never_edit_the_preview() {
    let mut app = history_app();
    app.dispatch_key(ctrl('r'));
    for c in "add".chars() {
        app.dispatch_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.input.text, "add docs");
    app.dispatch_key(ctrl('u'));
    assert_eq!(
        app.bottom.popup.as_ref().unwrap().query,
        "",
        "ctrl+u clears the query"
    );
    assert_eq!(app.input.text, "fix lint", "the newest entry again");
    for c in "docs".chars() {
        app.dispatch_key(key(KeyCode::Char(c)));
    }
    // Enter, shifted or not, accepts the entry.
    for chord in draft_keys()
        .into_iter()
        .filter(|k| k.code != KeyCode::Enter)
    {
        let effects = app.dispatch_key(chord);
        assert!(effects.is_empty(), "{chord:?}: {effects:?}");
        assert_eq!(app.input.text, "add docs", "{chord:?}");
    }
    app.dispatch_key(key(KeyCode::Enter));
    assert!(app.bottom.popup.is_none());
    assert_eq!(app.input.text, "add docs", "accepted as recalled");
}

#[test]
fn a_paste_into_history_search_extends_the_query_like_typing() {
    let mut app = history_app();
    app.dispatch_key(ctrl('r'));
    assert_eq!(app.input.text, "fix lint");
    app.dispatch_paste("do\r\ncs");
    assert_eq!(app.bottom.popup.as_ref().unwrap().query, "do  cs");
    app.dispatch_key(ctrl('u'));
    app.dispatch_paste("docs");
    assert_eq!(app.bottom.popup.as_ref().unwrap().query, "docs");
    assert_eq!(
        app.input.text, "add docs",
        "the recalled entry is not edited"
    );
    app.dispatch_key(key(KeyCode::Esc));
    assert_eq!(app.input.text, "wip");
}

#[test]
fn the_wheel_scrolls_an_overflowing_help_panel() {
    use crossterm::event::{MouseEvent, MouseEventKind};
    let (mut app, _) = app_with(AgentStatus::Ready);
    app.size = (80, 20);
    app.board.open = false;
    app.dispatch_key(key(KeyCode::Char('?')));
    let mut cache = crate::ui::transcript::ViewCache::default();
    crate::testing::render_with(&app, &mut cache, 80, 20);
    let panel = cache.regions.view;
    let wheel = |kind| MouseEvent {
        kind,
        column: panel.x + 1,
        row: panel.y + 1,
        modifiers: KeyModifiers::NONE,
    };
    app.dispatch_mouse(wheel(MouseEventKind::ScrollDown), &mut cache);
    assert_eq!(app.bottom.shortcuts_scroll, 3);
    app.dispatch_mouse(wheel(MouseEventKind::ScrollUp), &mut cache);
    assert_eq!(app.bottom.shortcuts_scroll, 0);
    assert_eq!(
        app.scroll_log.since(0).count(),
        0,
        "the transcript stayed put"
    );
}

#[test]
fn arrows_and_pages_scroll_an_overflowing_help_panel_not_the_transcript() {
    let (mut app, _) = app_with(AgentStatus::Ready);
    // One column of keys on a short screen: several pages of them.
    app.size = (70, 20);
    app.board.open = false;
    app.dispatch_key(key(KeyCode::Char('?')));
    assert!(app.bottom.shortcuts);
    let composer = crate::ui::composer_area(&app);
    let (page, max) = crate::ui::bottom::shortcuts::scroll_range(
        composer,
        &app.theme,
        &app.keys,
        app.help_modes(),
        &app.options.host_commands,
    );
    assert!(max > page, "the fixture must overflow by more than a page");
    let draft = app.input.text.clone();

    app.dispatch_key(key(KeyCode::Down));
    assert_eq!(app.bottom.shortcuts_scroll, 1);
    app.dispatch_key(key(KeyCode::PageDown));
    assert_eq!(app.bottom.shortcuts_scroll, 1 + page);
    for _ in 0..max {
        app.dispatch_key(key(KeyCode::PageDown));
    }
    assert_eq!(app.bottom.shortcuts_scroll, max);
    app.dispatch_key(key(KeyCode::PageUp));
    app.dispatch_key(key(KeyCode::Up));
    assert_eq!(app.bottom.shortcuts_scroll, max - page - 1);
    assert_eq!(
        app.scroll_log.since(0).count(),
        0,
        "the transcript stayed put"
    );
    assert_eq!(app.input.text, draft, "↑ did not walk history");

    // Reopening starts from the top.
    app.dispatch_key(key(KeyCode::Char('?')));
    app.dispatch_key(key(KeyCode::Char('?')));
    assert!(app.bottom.shortcuts);
    assert_eq!(app.bottom.shortcuts_scroll, 0);
}

#[test]
fn find_counts_and_steps_individual_occurrences() {
    let mut app =
        crate::testing::app_with_transcript(&["MONOSPACE monospace", "monospace monospace"]);
    app.open_find();
    for ch in "monospace".chars() {
        app.popup_key(key(KeyCode::Char(ch)));
    }
    let matches = app.bottom.find.as_ref().unwrap().matches.clone();
    assert_eq!(matches.len(), 4);
    assert_eq!(matches[0], matches[1]);
    assert_eq!(matches[2], matches[3]);
    for index in [1, 2, 3, 0] {
        app.popup_key(key(KeyCode::Enter));
        assert_eq!(app.bottom.find.as_ref().unwrap().current, Some(index));
    }
    app.popup_key(key(KeyCode::Up));
    assert_eq!(app.bottom.find.as_ref().unwrap().current, Some(3));
    app.track_find();
    assert_eq!(app.bottom.find.as_ref().unwrap().current, Some(3));
}

#[test]
fn find_counts_matches_across_indexing_soft_wraps() {
    let message = format!(
        "{}match phrase {}match phrase",
        "x ".repeat(495),
        "x ".repeat(495)
    );
    let mut app = crate::testing::app_with_transcript(&[&message]);
    app.open_find();
    for ch in "match phrase".chars() {
        app.popup_key(key(KeyCode::Char(ch)));
    }
    assert_eq!(app.bottom.find.as_ref().unwrap().matches.len(), 2);
}

#[test]
fn duplicate_agent_mentions_show_the_board_name_and_keep_the_selected_id() {
    let (mut app, root) = app_with(AgentStatus::Ready);
    let first = add_child(&mut app, root, "worker");
    let second = add_child(&mut app, root, "worker");
    let created = app.tree.nodes[&first].agent.created_at;
    app.tree.nodes.get_mut(&second).unwrap().agent.created_at =
        created + chrono::Duration::seconds(1);
    app.open_popup(PopupKind::Mention(MentionTab::Agents));
    let items = popup::items(app.bottom.popup.as_ref().unwrap(), &app);
    assert!(items.iter().any(|item| matches!(item, PopupItem::Agent { id, name, .. } if *id == second && name == "worker 2")));
    app.bottom.popup.as_mut().unwrap().cursor = items
        .iter()
        .position(|item| matches!(item, PopupItem::Agent { id, .. } if *id == second))
        .unwrap();
    app.popup_key(key(KeyCode::Enter));
    assert_eq!(app.input.text, format!("@worker 2 ({second}) "));
    assert_eq!(app.expanded_draft(), app.input.text);
}
