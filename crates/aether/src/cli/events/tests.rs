#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;
use aether_core::{
    event::{Actor, EventDraft, EventKind},
    id::EventId,
};
use clap::Parser;

fn try_args(flags: &[&str]) -> Result<crate::args::Args, clap::Error> {
    crate::args::Args::try_parse_from(
        ["aether", "events"]
            .into_iter()
            .chain(flags.iter().copied()),
    )
}

fn args(flags: &[&str]) -> EventArgs {
    match try_args(flags).unwrap().command.unwrap() {
        crate::args::Cmd::Events(args) => args,
        _ => panic!("expected events"),
    }
}

fn event(at: &str) -> Event {
    Event::from_draft(
        EventDraft::new(
            Actor::System,
            EventKind::CompactionFailed {
                error: "needle".into(),
                models: vec![],
            },
        ),
        EventId::new(),
        1,
        at.parse().unwrap(),
    )
}

#[test]
fn terminal_defaults_to_fifty_and_pipes_keep_all_history() {
    let defaults = args(&[]);
    assert_eq!(defaults.tail_limit(true), Some(50));
    assert_eq!(defaults.tail_limit(false), None);
    assert_eq!(args(&["--tail", "7"]).tail_limit(false), Some(7));
    assert_eq!(args(&["--tail", "0"]).tail_limit(true), Some(0));
}

#[test]
fn combined_filters_and_since_cutoff() {
    let run = "run_01J00000000000000000000004";
    let task = "tsk_01J00000000000000000000002";
    let args = args(&[
        "--run",
        run,
        "--type",
        "compaction.failed",
        "--since",
        "1h",
        "--task",
        task,
        "--family",
        "compaction",
        "--grep",
        "needle",
    ]);
    assert_eq!(args.run.as_ref().unwrap().to_string(), run);
    assert_eq!(args.task.as_ref().unwrap().to_string(), task);
    assert_eq!(args.types, ["compaction.failed"]);
    assert_eq!(args.families, ["compaction"]);
    let now = "2026-10-16T15:00:00Z".parse().unwrap();
    let cutoff = args.cutoff(now).unwrap();
    assert_eq!(cutoff.unwrap().to_rfc3339(), "2026-10-16T14:00:00+00:00");
    assert!(
        args.matches(&event("2026-10-16T14:00:00Z"), cutoff)
            .unwrap()
    );
    assert!(
        !args
            .matches(&event("2026-10-16T13:59:59Z"), cutoff)
            .unwrap()
    );
}

#[test]
fn dates_disambiguate_days_and_grep_is_literal() {
    let first = event("2026-10-15T23:59:59Z");
    let second = event("2026-10-16T00:00:00Z");
    assert!(event_line(&first).contains("2026-10-15 23:59:59"));
    assert!(event_line(&second).contains("2026-10-16 00:00:00"));
    assert!(args(&["--grep", "needle"]).matches(&first, None).unwrap());
    assert!(!args(&["--grep", ".*"]).matches(&first, None).unwrap());
}

#[test]
fn durations_and_ids_reject_invalid_input() {
    for invalid in ["", "0s", "-1h", "1.5h", "1w", "18446744073709551615d"] {
        assert!(parse_duration(invalid).is_err(), "{invalid}");
    }
    assert_eq!(parse_duration("2h").unwrap().as_secs(), 7200);
    assert!(try_args(&["--run", "bad"]).is_err());
    assert!(try_args(&["--task", "agt_01J5"]).is_err());
    assert!(
        args(&["--since", "18446744073709551615s"])
            .cutoff(aether_core::time::now())
            .is_err()
    );
}
