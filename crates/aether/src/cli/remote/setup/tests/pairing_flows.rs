//! Step 4: the phone, from the store links to the home screen.

use aether_protocol::remote::PairingState;

use super::super::Options;
use super::super::report::{Mode, Status, StepId, StepState};
use super::fakes::{Fixture, claimed, pairing};
use super::flows::state_of;

/// Answers to the three questions that come before the phone step on a fresh machine.
const BEFORE_PHONE: [&str; 3] = ["y", "y", "y"];

fn with_phone(after: &[&str]) -> Vec<&'static str> {
    let mut answers: Vec<&'static str> = BEFORE_PHONE.to_vec();
    answers.extend(after.iter().map(|a| match *a {
        "" => "",
        "y" => "y",
        "n" => "n",
        "q" => "q",
        other => panic!("unscripted answer {other}"),
    }));
    answers
}

#[tokio::test]
async fn a_phone_is_walked_through_from_the_app_store_to_the_home_screen() {
    let answers = with_phone(&["", "y"]);
    let mut fx = Fixture::fresh()
        .answers(&answers)
        .pairings(vec![pairing(PairingState::Open), claimed("Robin's iPhone")]);
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(report.status, Status::Complete);
    assert_eq!(state_of(&report, StepId::Pairing), StepState::Changed);
    assert_eq!(fx.daemon.confirmations, [true]);
    for expected in [
        "https://apps.apple.com/app/tailscale/id1470499037",
        "https://play.google.com/store/apps/details?id=com.tailscale.ipn",
        "SAME account",
        "amber-otter-canyon",
        "Robin's iPhone",
        "9f2c 41aa 07b3 de55",
        "https://box.tail1234.ts.net/pair#code=7K2M-9QXD",
        "Add to Home Screen",
        "notifications",
    ] {
        assert!(fx.ui.said(expected), "never said `{expected}`");
    }
    assert!(report.next.is_empty(), "{:?}", report.next);
}

#[tokio::test]
async fn words_that_do_not_match_reject_the_device() {
    let answers = with_phone(&["", "n"]);
    let mut fx = Fixture::fresh()
        .answers(&answers)
        .pairings(vec![claimed("Someone Else's Phone")]);
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(fx.daemon.confirmations, [false]);
    assert_eq!(state_of(&report, StepId::Pairing), StepState::Stopped);
    assert_eq!(report.next, ["aether remote setup"]);
}

#[tokio::test]
async fn a_code_nobody_claims_can_be_replaced_with_a_new_one() {
    let answers = with_phone(&["", "y", "y"]);
    let mut fx = Fixture::fresh().answers(&answers).pairings(vec![
        pairing(PairingState::Open),
        pairing(PairingState::Expired),
        pairing(PairingState::Open),
        claimed("Robin's iPhone"),
    ]);
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(fx.daemon.opened, 2);
    assert_eq!(state_of(&report, StepId::Pairing), StepState::Changed);
}

#[tokio::test]
async fn a_code_that_expires_and_is_not_renewed_blocks_with_the_command_to_retry() {
    let answers = with_phone(&["", "n"]);
    let mut fx = Fixture::fresh()
        .answers(&answers)
        .pairings(vec![pairing(PairingState::Expired)]);
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    let step = report.step(StepId::Pairing).expect("step");
    assert_eq!(step.state, StepState::Blocked);
    assert_eq!(step.command.as_deref(), Some("aether remote pair"));
}

#[tokio::test]
async fn waiting_has_a_bound_and_closes_the_pairing_it_opened() {
    let answers = with_phone(&["", "n"]);
    let mut fx = Fixture::fresh()
        .answers(&answers)
        .pairings(vec![pairing(PairingState::Open)]);
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(state_of(&report, StepId::Pairing), StepState::Blocked);
    assert_eq!(fx.daemon.cancelled, ["pair-1"]);
    assert!(fx.env.sleeps.get() >= 600, "it polled for the whole bound");
}

#[tokio::test]
async fn confirming_after_the_window_closed_is_an_expiry_not_a_crash() {
    let answers = with_phone(&["", "y", "n"]);
    let mut fx = Fixture::fresh()
        .answers(&answers)
        .pairings(vec![claimed("Robin's iPhone")]);
    fx.daemon.confirm_error = true;
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(state_of(&report, StepId::Pairing), StepState::Blocked);
}

#[tokio::test]
async fn ctrl_c_while_waiting_cancels_the_pairing() {
    let answers = with_phone(&[""]);
    let mut fx = Fixture::fresh()
        .answers(&answers)
        .pairings(vec![pairing(PairingState::Open)]);
    fx.env.interrupt = true;
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(fx.daemon.cancelled, ["pair-1"]);
    assert_eq!(state_of(&report, StepId::Pairing), StepState::Stopped);
    assert!(fx.ui.said("Pairing cancelled"));
}

#[tokio::test]
async fn quitting_before_scanning_opens_no_pairing() {
    let answers = with_phone(&["q"]);
    let mut fx = Fixture::fresh().answers(&answers);
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(fx.daemon.opened, 0);
    assert_eq!(report.status, Status::Stopped);
}

#[tokio::test]
async fn yes_leaves_pairing_pending_even_with_a_person_present() {
    let mut fx = Fixture::fresh()
        .answers(&["y"])
        .pairings(vec![claimed("Robin's iPhone")]);
    let options = Options {
        yes: true,
        ..Options::default()
    };
    let report = fx.run(&options, Mode::Yes).await.expect("runs");
    assert_eq!(state_of(&report, StepId::Pairing), StepState::Skipped);
    assert_eq!(fx.daemon.opened, 0, "no pairing was opened");
    assert!(
        fx.ui.asked.is_empty(),
        "nothing was asked: {:?}",
        fx.ui.asked
    );
    assert_eq!(report.next, ["aether remote pair"]);
}

#[tokio::test]
async fn without_a_person_pairing_is_left_as_the_next_command() {
    let mut fx = Fixture::fresh();
    let options = Options {
        yes: true,
        ..Options::default()
    };
    let report = fx.run(&options, Mode::Yes).await.expect("runs");
    assert_eq!(state_of(&report, StepId::Pairing), StepState::Skipped);
    assert_eq!(fx.daemon.opened, 0);
    assert_eq!(report.status, Status::Complete);
    assert_eq!(report.next, ["aether remote pair"]);
}

#[tokio::test]
async fn an_already_paired_phone_is_not_paired_again_unless_asked() {
    let mut fx = Fixture::complete().answers(&["n"]);
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(state_of(&report, StepId::Pairing), StepState::Done);
    assert!(fx.ui.said("Already paired: Robin's iPhone"));
    assert_eq!(fx.daemon.opened, 0);
}

#[tokio::test]
async fn pairing_another_device_works_after_saying_yes() {
    let mut fx = Fixture::complete()
        .answers(&["y", "", "y"])
        .pairings(vec![claimed("Second Phone")]);
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(state_of(&report, StepId::Pairing), StepState::Changed);
}

#[tokio::test]
async fn behind_a_proxy_there_is_no_tailscale_app_to_install() {
    let options = Options {
        proxy_url: Some("https://aether.example.com".to_owned()),
        ..Options::default()
    };
    let answers = with_phone(&["y"]);
    let mut fx = Fixture::fresh()
        .answers(&answers)
        .pairings(vec![claimed("Robin's iPhone")]);
    let report = fx.run(&options, Mode::Interactive).await.expect("runs");
    assert_eq!(state_of(&report, StepId::Pairing), StepState::Changed);
    assert!(!fx.ui.said("Tailscale app"));
    assert!(fx.ui.said("https://aether.example.com/pair#code=7K2M-9QXD"));
}
