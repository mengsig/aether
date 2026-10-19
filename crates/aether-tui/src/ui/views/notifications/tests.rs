//! The notification center view: what its keys ask the app to do, and how
//! the list, the detail and the footer indicator paint.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use aether_core::event::Actor;
use aether_core::id::AgentId;
use aether_core::notification::{
    Notification, NotificationAction, NotificationKind, NotificationPost,
};
use aether_core::time::now;
use aether_protocol::Command;

use super::NotificationsView;
use crate::app::{App, Effect, Then};
use crate::testing::populated_app;
use crate::ui::bottom::{BottomView, ViewAction, ViewOutcome};
use crate::ui::tests_support::screen_text;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn press(view: &mut NotificationsView, app: &App, code: KeyCode) -> ViewOutcome {
    view.handle_key(key(code), app)
}

fn post(
    kind: NotificationKind,
    title: &str,
    actions: &[&str],
    free_text: bool,
) -> NotificationPost {
    NotificationPost {
        kind,
        title: title.into(),
        body: "CI is **green** on `next`.".into(),
        key: Some(title.to_lowercase()),
        link: None,
        actions: actions
            .iter()
            .enumerate()
            .map(|(i, label)| NotificationAction {
                id: (i + 1).to_string(),
                label: (*label).into(),
            })
            .collect(),
        free_text,
    }
}

/// `populated_app` serving a question from Archon, a blocker from its
/// agent, and a resolved outcome, oldest last.
fn center_app() -> (App, AgentId) {
    let mut app = populated_app();
    let agent = *app.tree.nodes.keys().next().expect("one agent");
    app.notifications.set_served(true);
    let at = |age: i64| now() - chrono::Duration::seconds(age);
    let question = Notification::new(
        "archon".into(),
        post(
            NotificationKind::Question,
            "Merge #231 into next?",
            &["Merge", "Hold"],
            true,
        ),
        at(120),
    );
    let blocker = Notification::new(
        agent.to_string(),
        post(NotificationKind::Blocker, "Need a GitHub token", &[], false),
        at(30),
    );
    let mut outcome = Notification::new(
        "archon".into(),
        post(NotificationKind::Outcome, "Shipped #229", &[], false),
        at(3_600),
    );
    outcome.resolve(now());
    for n in [question, blocker, outcome] {
        app.notifications.apply(n);
    }
    (app, agent)
}

fn effects(outcome: ViewOutcome) -> Vec<Effect> {
    let ViewOutcome::Act(actions) = outcome else {
        panic!("expected Act, got {outcome:?}");
    };
    actions
        .into_iter()
        .map(|action| match action {
            ViewAction::Effect(effect) => *effect,
            other => panic!("expected an effect, got {other:?}"),
        })
        .collect()
}

fn flashed(outcome: ViewOutcome) -> String {
    match outcome {
        ViewOutcome::Act(actions) => match actions.as_slice() {
            [ViewAction::Flash(text)] => text.clone(),
            other => panic!("expected one flash, got {other:?}"),
        },
        other => panic!("expected a flash, got {other:?}"),
    }
}

fn draw(view: &NotificationsView, app: &App, width: u16) -> String {
    let height = view.desired_height(width, app).clamp(1, 40);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| view.render(frame, frame.area(), app, &app.theme))
        .expect("draw");
    screen_text(&terminal)
}

#[test]
fn the_list_groups_by_state_newest_first_and_marks_what_waits() {
    let (app, _) = center_app();
    let view = NotificationsView::default();
    let screen = draw(&view, &app, 100);
    let at = |text: &str| {
        screen
            .find(text)
            .unwrap_or_else(|| panic!("{text}:\n{screen}"))
    };
    assert!(
        screen.contains("Notifications  2 unread · 2 waiting on you"),
        "{screen}"
    );
    assert!(at("Unread") < at("Need a GitHub token"));
    assert!(at("Need a GitHub token") < at("Merge #231"), "newest first");
    assert!(at("Merge #231") < at("Resolved") && at("Resolved") < at("Shipped #229"));
    assert!(
        screen.contains("› ◆ Need a GitHub token"),
        "cursor on the newest:\n{screen}"
    );
    assert!(
        screen.contains("blocker · assistant"),
        "an agent by name:\n{screen}"
    );
    assert!(screen.contains("question · archon"), "{screen}");
    assert!(
        !screen.contains("◆ Shipped"),
        "a settled one waits on nobody"
    );
}

#[test]
fn opening_marks_read_and_the_detail_shows_body_and_choices() {
    let (app, _) = center_app();
    let mut view = NotificationsView::default();
    press(&mut view, &app, KeyCode::Down);
    let opened = effects(press(&mut view, &app, KeyCode::Enter));
    let question = app.notifications.grouped()[1].clone();
    assert!(matches!(
        opened.as_slice(),
        [Effect::Request { command: Command::MarkNotificationRead { id }, then: Then::NotificationChanged }]
            if *id == question.id
    ));
    let screen = draw(&view, &app, 100);
    for text in [
        "Question  from archon",
        "Merge #231 into next?",
        "CI is green",
        "1. Merge",
        "2. Hold",
        "write an answer",
    ] {
        assert!(screen.contains(text), "{text}:\n{screen}");
    }
    let ascii = {
        let mut app = app;
        app.theme = crate::ui::theme::tokens::Theme::resolve(crate::ui::theme::tokens::ThemeSpec {
            ascii: true,
            ..Default::default()
        });
        draw(&view, &app, 80)
    };
    assert!(ascii.is_ascii(), "{ascii}");
}

#[test]
fn a_digit_or_enter_answers_with_the_action_id() {
    let (app, _) = center_app();
    let question = app.notifications.grouped()[1].clone();
    let mut view = NotificationsView::default();
    press(&mut view, &app, KeyCode::Down);
    press(&mut view, &app, KeyCode::Enter);
    let answer = |effects: Vec<Effect>| match effects.as_slice() {
        [
            Effect::Request {
                command: Command::AnswerNotification { id, action, text },
                then: Then::NotificationAnswered,
            },
        ] => {
            assert_eq!(*id, question.id);
            (action.clone(), text.clone())
        }
        other => panic!("expected an answer, got {other:?}"),
    };
    let by_digit = answer(effects(press(&mut view, &app, KeyCode::Char('2'))));
    assert_eq!(by_digit, (Some("2".into()), None));
    press(&mut view, &app, KeyCode::Up);
    let by_enter = answer(effects(press(&mut view, &app, KeyCode::Enter)));
    assert_eq!(by_enter, (Some("1".into()), None));
    assert!(matches!(
        press(&mut view, &app, KeyCode::Char('9')),
        ViewOutcome::Consumed
    ));
}

#[test]
fn the_operator_can_write_an_answer_of_their_own() {
    let (app, _) = center_app();
    let mut view = NotificationsView::default();
    press(&mut view, &app, KeyCode::Down);
    press(&mut view, &app, KeyCode::Enter);
    press(&mut view, &app, KeyCode::Char('a'));
    let empty = flashed(press(&mut view, &app, KeyCode::Enter));
    assert!(empty.contains("type an answer"), "{empty}");
    for c in "hold x".chars() {
        press(&mut view, &app, KeyCode::Char(c));
    }
    view.handle_paste(" until Monday");
    let screen = draw(&view, &app, 100);
    assert!(screen.contains("hold x until Monday"), "{screen}");
    let sent = effects(press(&mut view, &app, KeyCode::Enter));
    assert!(matches!(
        sent.as_slice(),
        [Effect::Request { command: Command::AnswerNotification { action: None, text: Some(text), .. }, .. }]
            if text == "hold x until Monday"
    ));
}

#[test]
fn read_resolve_and_jump_act_on_the_highlighted_one() {
    let (app, agent) = center_app();
    let blocker = app.notifications.grouped()[0].clone();
    let mut view = NotificationsView::default();
    let read = effects(press(&mut view, &app, KeyCode::Char('r')));
    assert!(
        matches!(read.as_slice(), [Effect::Request { command: Command::MarkNotificationRead { id }, .. }] if *id == blocker.id)
    );
    let resolved = effects(press(&mut view, &app, KeyCode::Char('x')));
    assert!(matches!(
        resolved.as_slice(),
        [Effect::Request { command: Command::ResolveNotification { id: Some(id), source: None, key: None }, .. }]
            if *id == blocker.id
    ));
    let ViewOutcome::Close(actions) = press(&mut view, &app, KeyCode::Char('o')) else {
        panic!("o closes the center");
    };
    assert!(matches!(actions.as_slice(), [ViewAction::ViewAgent(id)] if *id == agent));
    press(&mut view, &app, KeyCode::Down);
    let service = flashed(press(&mut view, &app, KeyCode::Char('o')));
    assert!(service.contains("posted by a service"), "{service}");
    press(&mut view, &app, KeyCode::End);
    assert!(flashed(press(&mut view, &app, KeyCode::Char('x'))).contains("already resolved"));
}

#[test]
fn a_settled_notification_shows_its_answer_and_takes_none() {
    let (mut app, _) = center_app();
    let mut question = app.notifications.grouped()[1].clone();
    question
        .answer(
            Some("2".into()),
            Some("after the release".into()),
            Actor::user(),
            now(),
        )
        .expect("answerable");
    app.notifications.apply(question);
    let mut view = NotificationsView::default();
    press(&mut view, &app, KeyCode::End);
    press(&mut view, &app, KeyCode::Up);
    press(&mut view, &app, KeyCode::Enter);
    let screen = draw(&view, &app, 100);
    assert!(
        screen.contains("Answered Hold \"after the release\""),
        "{screen}"
    );
    assert!(!screen.contains("write an answer"), "{screen}");
    assert!(matches!(
        press(&mut view, &app, KeyCode::Char('1')),
        ViewOutcome::Consumed
    ));
    assert!(matches!(
        press(&mut view, &app, KeyCode::Esc),
        ViewOutcome::Consumed
    ));
    assert!(matches!(
        press(&mut view, &app, KeyCode::Esc),
        ViewOutcome::Close(_)
    ));
}

#[test]
fn the_footer_counts_unread_bottom_right_and_questions_stand_out() {
    let (mut app, _) = center_app();
    let screen = crate::testing::render_to(&app, 120, 40);
    let footer = screen.lines().last().expect("a footer row");
    assert!(
        footer.trim_end().ends_with("◆ 2 waiting · 2 unread"),
        "{footer:?}"
    );
    let narrow = crate::testing::render_to(&app, 40, 20);
    assert!(
        narrow.lines().last().is_some_and(|row| row.contains("◆ 2")),
        "{narrow}"
    );
    for n in app.notifications.items().to_vec() {
        let mut read = n;
        read.mark_read(now());
        app.notifications.apply(read);
    }
    let screen = crate::testing::render_to(&app, 120, 40);
    assert!(
        !screen.lines().last().unwrap_or_default().contains("unread"),
        "{screen}"
    );
}
