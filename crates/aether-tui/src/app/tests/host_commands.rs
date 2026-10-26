//! `--host-commands`: listed like built-ins, sent to the agent as typed.
use super::composer::sent_messages;
use super::*;
use crate::commands::{COMMANDS, HostCommands};
use crate::ui::bottom::popup::{self, PopupItem, PopupKind};

fn archon() -> HostCommands {
    HostCommands::parse(
        r#"{"version":1,"host":"archon","commands":[
            {"name":"answer","args":"<n> <text>","about":"answer question n"},
            {"name":"questions","about":"list open questions"}
        ]}"#,
    )
    .unwrap()
}

pub(super) fn app_with_host() -> (App, AgentId) {
    let (mut app, id) = app_with_agent();
    app.options.host_commands = archon();
    app.flash = None;
    (app, id)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.reduce(key(KeyCode::Char(c)));
    }
}

fn slash_items(app: &App) -> Vec<PopupItem> {
    popup::items(app.bottom.popup.as_ref().expect("slash popup"), app)
}

#[test]
fn the_slash_popup_lists_host_commands_after_the_built_ins_marked_with_the_host() {
    let (mut app, _) = app_with_host();
    app.open_popup(PopupKind::Slash);
    let items = slash_items(&app);
    let hosts: Vec<_> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match item {
            PopupItem::HostCommand { name, usage, about } => {
                Some((i, name.as_str(), usage.as_str(), about.as_str()))
            }
            _ => None,
        })
        .collect();
    let dot = app.theme.glyphs().dot;
    assert_eq!(
        hosts,
        [
            (
                COMMANDS.len(),
                "answer",
                "<n> <text>",
                format!("answer question n {dot} archon").as_str()
            ),
            (
                COMMANDS.len() + 1,
                "questions",
                "",
                format!("list open questions {dot} archon").as_str()
            ),
        ]
    );
}

#[test]
fn a_welcome_puts_host_commands_first_in_the_slash_popup() {
    let (mut app, _) = app_with_host();
    app.options.host_commands = HostCommands::parse(
        r#"{"version":1,"host":"archon","welcome":{"title":"Archon"},"commands":[{"name":"answer"},{"name":"questions"}]}"#,
    )
    .unwrap();
    app.open_popup(PopupKind::Slash);
    let items = slash_items(&app);
    assert!(matches!(&items[0], PopupItem::HostCommand { name, .. } if name == "answer"));
    assert!(matches!(&items[1], PopupItem::HostCommand { name, .. } if name == "questions"));
    assert!(matches!(&items[2], PopupItem::Command { .. }));
}

#[test]
fn the_slash_popup_filters_host_commands_like_built_ins() {
    let (mut app, _) = app_with_host();
    type_text(&mut app, "/quest");
    assert_eq!(app.bottom.popup.as_ref().unwrap().query, "quest");
    let first = slash_items(&app).into_iter().next();
    assert!(
        matches!(&first, Some(PopupItem::HostCommand { name, .. }) if name == "questions"),
        "{first:?}"
    );
    type_text(&mut app, "zz");
    assert!(
        !slash_items(&app)
            .iter()
            .any(|item| matches!(item, PopupItem::HostCommand { .. }))
    );
}

#[test]
fn choosing_a_host_command_that_takes_arguments_completes_it_then_sends_it_verbatim() {
    let (mut app, id) = app_with_host();
    type_text(&mut app, "/answ");
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(sent_messages(&effects).is_empty(), "{effects:?}");
    assert_eq!(app.input.text, "/answer ");
    type_text(&mut app, "3 yes");
    let effects = app.reduce(key(KeyCode::Enter));
    assert_eq!(sent_messages(&effects), [(id, "/answer 3 yes".to_owned())]);
    assert!(app.input.text.is_empty());
}

#[test]
fn choosing_a_host_command_without_arguments_sends_it_at_once() {
    let (mut app, id) = app_with_host();
    type_text(&mut app, "/questions");
    let effects = app.reduce(key(KeyCode::Enter));
    assert_eq!(sent_messages(&effects), [(id, "/questions".to_owned())]);
    assert_eq!(app.live_flash(), None, "a host command is no typo");
}

#[test]
fn a_typed_host_command_reaches_the_agent_as_typed_and_never_runs_locally() {
    let (mut app, id) = app_with_host();
    app.input.set_text("/Answer 1  Yes, ship it");
    let effects = app.reduce(key(KeyCode::Enter));
    assert_eq!(
        sent_messages(&effects),
        [(id, "/Answer 1  Yes, ship it".to_owned())]
    );
    assert_eq!(app.live_flash(), None);
}

#[test]
fn a_host_command_with_no_live_agent_says_where_it_would_go() {
    let mut app = app_with_status(TuiOptions {
        host_commands: archon(),
        ..TuiOptions::default()
    });
    app.input.set_text("/answer 3 yes");
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(sent_messages(&effects).is_empty(), "{effects:?}");
    let flash = app.live_flash().unwrap();
    assert!(
        flash.contains("/answer is for archon; view a live agent"),
        "{flash}"
    );
}

#[test]
fn a_host_commands_warning_shows_once_at_start() {
    let refused = HostCommands::parse(r#"{"version":1,"commands":[{"name":"model"}]}"#).unwrap();
    let app = app_with_status(TuiOptions {
        host_commands: refused,
        ..TuiOptions::default()
    });
    let flash = app.live_flash().unwrap();
    assert!(
        flash.contains("/model refused: shadows a built-in"),
        "{flash}"
    );
}

#[test]
fn a_host_command_named_like_a_view_mode_is_refused_at_start_and_the_mode_still_runs() {
    let host =
        HostCommands::parse(r#"{"version":1,"commands":[{"name":"focus"},{"name":"answer"}]}"#)
            .unwrap();
    let options = TuiOptions {
        host_commands: host,
        ..TuiOptions::default()
    };
    assert!(options.view_modes.modes.contains_key("focus"));
    let app = app_with_status(options);
    let names: Vec<_> = app
        .options
        .host_commands
        .commands
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["answer"]);
    let flash = app.live_flash().unwrap();
    assert!(
        flash.contains("/focus refused: shadows a view mode"),
        "{flash}"
    );
    assert!(matches!(
        app.parse_input("/focus"),
        InputAction::Command(SlashCommand::ViewMode(Some(_)))
    ));
}
