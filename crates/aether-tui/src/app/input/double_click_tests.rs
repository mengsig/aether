//! Double click selects and copies a line; single click and drag are unchanged.

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::{App, AppEvent, Effect};
use crate::testing::{app_with_transcript, render_with};
use crate::ui::transcript::ViewCache;

fn at(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// A full press and release at `(x, y)` at time `t`.
fn click(app: &mut App, cache: &mut ViewCache, x: u16, y: u16, t: Instant) -> Vec<Effect> {
    app.dispatch_mouse_at(at(MouseEventKind::Down(MouseButton::Left), x, y), cache, t);
    app.dispatch_mouse_at(at(MouseEventKind::Up(MouseButton::Left), x, y), cache, t)
}

fn copied(effects: &[Effect]) -> Option<&str> {
    effects.iter().find_map(|e| match e {
        Effect::Copy(text) => Some(text.as_str()),
        _ => None,
    })
}

fn transcript_app() -> (App, ViewCache) {
    let mut app = app_with_transcript(&["first line here", "second line here"]);
    app.size = (100, 40);
    let mut cache = ViewCache::default();
    render_with(&app, &mut cache, 100, 40);
    (app, cache)
}

/// Row of the transcript whose painted text contains `needle`.
fn row_of(cache: &mut ViewCache, app: &App, needle: &str) -> u16 {
    let screen = render_with(app, cache, 100, 40);
    screen
        .lines()
        .position(|l| l.contains(needle))
        .expect("text on screen") as u16
}

#[test]
fn double_click_copies_the_transcript_row() {
    let (mut app, mut cache) = transcript_app();
    let row = row_of(&mut cache, &app, "second line here");
    let x = cache.regions.transcript.x + 1;
    let t = Instant::now();
    assert!(click(&mut app, &mut cache, x, row, t).is_empty());
    let effects = click(&mut app, &mut cache, x, row, t + Duration::from_millis(100));
    assert_eq!(copied(&effects), Some("• second line here"));
    assert!(cache.mouse.selection.is_some(), "line stays highlighted");
}

#[test]
fn double_click_timing_and_distance() {
    let (mut app, mut cache) = transcript_app();
    let row = row_of(&mut cache, &app, "second line here");
    let x = cache.regions.transcript.x + 1;
    let t = Instant::now();
    click(&mut app, &mut cache, x, row, t);
    // Too slow.
    let slow = click(&mut app, &mut cache, x, row, t + Duration::from_millis(401));
    assert!(copied(&slow).is_none());
    // Two cells away.
    let far = click(
        &mut app,
        &mut cache,
        x + 2,
        row,
        t + Duration::from_millis(500),
    );
    assert!(copied(&far).is_none());
    // One cell away and within the window.
    let near = click(
        &mut app,
        &mut cache,
        x + 3,
        row,
        t + Duration::from_millis(600),
    );
    assert_eq!(copied(&near), Some("• second line here"));
    // A third press right after does not start another double click.
    let third = click(
        &mut app,
        &mut cache,
        x + 3,
        row,
        t + Duration::from_millis(650),
    );
    assert!(copied(&third).is_none());
}

#[test]
fn single_click_and_drag_are_unchanged() {
    let (mut app, mut cache) = transcript_app();
    let row = row_of(&mut cache, &app, "second line here");
    let x = cache.regions.transcript.x;
    let t = Instant::now();
    assert!(click(&mut app, &mut cache, x, row, t).is_empty());
    assert!(cache.mouse.selection.is_none());
    let t = t + Duration::from_secs(2);
    app.dispatch_mouse_at(
        at(MouseEventKind::Down(MouseButton::Left), x, row),
        &mut cache,
        t,
    );
    app.dispatch_mouse_at(
        at(MouseEventKind::Drag(MouseButton::Left), x + 5, row),
        &mut cache,
        t,
    );
    let effects = app.dispatch_mouse_at(
        at(MouseEventKind::Up(MouseButton::Left), x + 5, row),
        &mut cache,
        t,
    );
    assert_eq!(copied(&effects), Some("• seco"));
}

#[test]
fn double_click_copies_the_composer_line_across_wrapped_rows() {
    let mut app = app_with_transcript(&["hello"]);
    app.size = (30, 30);
    let long = "alpha bravo charlie delta echo foxtrot golf hotel";
    app.reduce(AppEvent::Paste(format!("one\n{long}\nlast")));
    let mut cache = ViewCache::default();
    render_with(&app, &mut cache, 30, 30);
    assert!(cache.composer.rows.len() > 3, "the middle line wraps");
    let r = cache.regions.composer;
    let t = Instant::now();
    click(&mut app, &mut cache, r.x + 1, r.y + 1, t);
    let effects = click(
        &mut app,
        &mut cache,
        r.x + 1,
        r.y + 1,
        t + Duration::from_millis(50),
    );
    assert_eq!(copied(&effects), Some(long));

    // A plain line copies on its own.
    let t = t + Duration::from_secs(5);
    click(&mut app, &mut cache, r.x + 1, r.y, t);
    let effects = click(
        &mut app,
        &mut cache,
        r.x + 1,
        r.y,
        t + Duration::from_millis(50),
    );
    assert_eq!(copied(&effects), Some("one"));
}
