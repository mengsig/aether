#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Transient feedback is one flash in the hint row (SPEC §1.1): one at a
//! time, gone when its time is up, and never repeated for the same outage.

use aether_tui::app::{App, AppEvent, TuiOptions};

fn connected_app() -> App {
    let mut app = App::new(TuiOptions::default());
    app.reduce(AppEvent::Connected);
    app
}

fn fail(app: &mut App, action: &str) {
    app.reduce(AppEvent::EffectFailed {
        action: action.into(),
        error: "denied".into(),
    });
}

/// Pretend the flash's time is up, as the clock would after a few seconds.
fn outlive_flash(app: &mut App) {
    let until = app.flash.as_ref().expect("a flash is showing").until;
    app.now = until;
}

#[test]
fn a_new_flash_replaces_the_one_showing() {
    let mut app = connected_app();
    fail(&mut app, "copy");
    assert!(
        app.live_flash().is_some_and(|f| f.contains("copy failed")),
        "{:?}",
        app.flash
    );
    fail(&mut app, "mouse capture");
    let shown = app.live_flash().expect("the second flash shows");
    assert!(shown.contains("mouse capture failed"), "{shown:?}");
    assert!(
        !shown.contains("copy"),
        "only one flash at a time: {shown:?}"
    );
}

#[test]
fn a_flash_expires_by_the_clock_and_a_tick_clears_it() {
    let mut app = App::new(TuiOptions::default());
    assert_eq!(app.next_tick_in(), None, "nothing to wake for yet");
    fail(&mut app, "copy");
    assert!(
        app.next_tick_in().is_some(),
        "a showing flash must book the tick that clears it"
    );

    outlive_flash(&mut app);
    assert_eq!(app.live_flash(), None, "an expired flash is not shown");

    // A tick reads the wall clock, so age the flash rather than wait for it.
    app.flash.as_mut().unwrap().until = aether_core::time::now();
    app.dirty = false;
    app.reduce(AppEvent::Tick { frames: 1 });
    assert_eq!(app.flash, None, "the tick past its deadline drops it");
    assert!(app.dirty, "dropping the flash must repaint the hint row");
    assert_eq!(app.next_tick_in(), None, "nothing left to wake for");
}

#[test]
fn a_long_error_stays_longer_than_a_short_note() {
    let mut app = App::new(TuiOptions::default());
    let start = app.now;
    fail(&mut app, "copy");
    let short = app.flash.as_ref().unwrap().until - start;
    fail(&mut app, &"a very long action name ".repeat(10));
    let long = app.flash.as_ref().unwrap().until - start;
    assert!(long > short, "{long} should outlast {short}");
    assert!(
        long <= chrono::Duration::seconds(6),
        "even a long flash goes away: {long}"
    );
}

/// The runner fires one `AppEvent::Disconnected` per failed in-flight
/// reply. One outage must flash once, not once per reply.
#[test]
fn connection_lost_flashes_once_per_outage() {
    let mut app = connected_app();
    app.reduce(AppEvent::Disconnected { because: None });
    assert!(
        app.live_flash()
            .is_some_and(|f| f.contains("connection lost")),
        "{:?}",
        app.flash
    );

    outlive_flash(&mut app);
    app.reduce(AppEvent::Disconnected { because: None });
    app.reduce(AppEvent::Disconnected { because: None });
    assert_eq!(
        app.live_flash(),
        None,
        "later failed replies of the same outage must not flash again"
    );
}

/// Reconnecting ends the outage the dedupe was guarding; a fresh
/// disconnection afterwards must report itself again, with its reason.
#[test]
fn connection_lost_flashes_again_after_reconnecting() {
    let mut app = connected_app();
    app.reduce(AppEvent::Disconnected { because: None });
    outlive_flash(&mut app);

    app.reduce(AppEvent::Connected);
    app.reduce(AppEvent::Disconnected {
        because: Some("idle timeout".into()),
    });
    let shown = app.live_flash().expect("a new outage flashes");
    assert!(
        shown.contains("connection lost") && shown.contains("idle timeout"),
        "{shown:?}"
    );
}

#[test]
fn daemon_status_keeps_the_disk_hold_until_a_successful_reply_clears_it() {
    use aether_core::disk::LowDisk;
    use aether_protocol::Reply;
    use aether_tui::app::Then;
    use aether_tui::testing::populated_app;

    let mut app = populated_app();
    let mut status = app.status.clone().expect("status");
    let disk = LowDisk {
        path: "/isolated/store".into(),
        available_bytes: 1 << 30,
        floor_bytes: 5 << 30,
        since: app.now,
    };
    status.low_disk = Some(disk.clone());
    app.background_dirty = false;
    app.reduce(AppEvent::Reply {
        then: Then::Status,
        result: Ok(Reply::Status(status.clone())),
    });
    assert!(app.background_dirty);
    assert_eq!(app.status.as_ref().unwrap().low_disk.as_ref(), Some(&disk));
    app.background_dirty = false;
    app.reduce(AppEvent::Reply {
        then: Then::Status,
        result: Ok(Reply::Status(status.clone())),
    });
    assert!(!app.background_dirty, "unchanged polls need no repaint");
    app.reduce(AppEvent::Reply {
        then: Then::Status,
        result: Err(aether_client::ClientError::Disconnected),
    });
    assert_eq!(app.status.as_ref().unwrap().low_disk.as_ref(), Some(&disk));
    status.low_disk = None;
    app.background_dirty = false;
    app.reduce(AppEvent::Reply {
        then: Then::Status,
        result: Ok(Reply::Status(status)),
    });
    assert!(app.background_dirty);
    assert!(app.status.as_ref().unwrap().low_disk.is_none());
}

#[test]
fn a_stale_status_keeps_the_disk_hold_a_fresh_one_changes() {
    use aether_core::disk::LowDisk;
    use aether_protocol::{Reply, StatusStale};
    use aether_tui::app::Then;
    use aether_tui::testing::populated_app;

    let mut app = populated_app();
    let status = app.status.clone().expect("status");
    let disk = LowDisk {
        path: "/isolated/store".into(),
        available_bytes: 1 << 30,
        floor_bytes: 5 << 30,
        since: app.now,
    };
    let send = |app: &mut aether_tui::app::App, mut next: aether_protocol::DaemonStatus| {
        next.stale = None;
        app.reduce(AppEvent::Reply {
            then: Then::Status,
            result: Ok(Reply::Status(next)),
        });
    };
    let mut held = status.clone();
    held.low_disk = Some(disk.clone());
    send(&mut app, held);

    for read_at in [None, Some(app.now)] {
        let mut stale = status.clone();
        stale.stale = Some(StatusStale {
            read_at,
            waited_ms: 3_000,
        });
        app.reduce(AppEvent::Reply {
            then: Then::Status,
            result: Ok(Reply::Status(stale)),
        });
        let shown = app.status.as_ref().expect("status");
        assert!(shown.stale.is_some(), "the status is marked stale");
        assert_eq!(shown.low_disk.as_ref(), Some(&disk), "{read_at:?}");
    }

    send(&mut app, status.clone());
    assert!(
        app.status.as_ref().unwrap().low_disk.is_none(),
        "fresh frees"
    );

    let mut stale = status;
    stale.low_disk = Some(disk);
    stale.stale = Some(StatusStale {
        read_at: Some(app.now),
        waited_ms: 3_000,
    });
    app.reduce(AppEvent::Reply {
        then: Then::Status,
        result: Ok(Reply::Status(stale)),
    });
    assert!(
        app.status.as_ref().unwrap().low_disk.is_none(),
        "stale holds nothing new"
    );
}
