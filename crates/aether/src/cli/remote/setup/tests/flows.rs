//! The wizard from nothing to a live address, one branch per test.

use super::super::Options;
use super::super::environment::ProbeError;
use super::super::platform::Platform;
use super::super::report::{Mode, Status, StepId, StepState};
use super::fakes::{
    Fixture, NODE, Scripted, TARGET, healthy, out, protocol_error, remote_status, serve_json,
    stale_revision_error, tailscale_json,
};
use aether_protocol::ErrorCode;
use std::collections::VecDeque;

pub(super) fn yes() -> Options {
    Options {
        yes: true,
        ..Options::default()
    }
}

pub(super) fn state_of(report: &super::super::report::Report, id: StepId) -> StepState {
    report
        .step(id)
        .unwrap_or_else(|| panic!("no {id:?} step in {:?}", report.steps))
        .state
}

#[tokio::test]
async fn a_machine_without_tailscale_is_told_how_to_install_it_and_stops() {
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    assert_eq!(report.status, Status::Blocked);
    let step = report.step(StepId::Tailscale).expect("tailscale step");
    assert_eq!(step.state, StepState::Blocked);
    assert!(
        step.fix
            .iter()
            .any(|line| line.contains("pacman -S tailscale")),
        "{:?}",
        step.fix
    );
    assert!(
        report.step(StepId::Daemon).is_none(),
        "nothing after the blocked step runs"
    );
    assert!(fx.daemon.configures.is_empty());
}

#[tokio::test]
async fn install_instructions_follow_the_platform() {
    for (platform, expected) in [
        (Platform::MacOs, "brew install --cask tailscale"),
        (Platform::Wsl, "https://tailscale.com/download/windows"),
        (
            Platform::Linux(super::super::platform::Distro::Debian),
            "install.sh",
        ),
    ] {
        let mut fx = Fixture::fresh();
        fx.runner = Default::default();
        fx.env.platform = platform;
        let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
        let fix = &report.step(StepId::Tailscale).expect("step").fix;
        assert!(
            fix.iter().any(|line| line.contains(expected)),
            "{expected} not in {fix:?}"
        );
    }
}

#[tokio::test]
async fn a_person_who_installs_tailscale_while_the_wizard_waits_carries_on() {
    let mut fx = Fixture::fresh().answers(&["", "y", "y", "y", "q"]);
    fx.runner = Default::default();
    fx.runner.on("tailscale version", Scripted::NotFound);
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    fx.runner
        .on("tailscale serve status --json", out(0, "{}", ""));
    fx.runner.on(
        "tailscale serve status --json",
        out(0, &serve_json(443, TARGET, false), ""),
    );
    fx.runner.on(
        &format!("tailscale serve --bg --https=443 {TARGET}"),
        out(0, "", ""),
    );
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(state_of(&report, StepId::Tailscale), StepState::Done);
    assert!(fx.ui.said("not installed"));
}

#[tokio::test]
async fn quitting_at_a_tailscale_problem_stops_cleanly() {
    let mut fx = Fixture::fresh().answers(&["q"]);
    fx.runner = Default::default();
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(report.status, Status::Stopped);
    assert_eq!(state_of(&report, StepId::Tailscale), StepState::Stopped);
}

#[tokio::test]
async fn tailscale_that_is_stopped_logged_out_or_unapproved_says_what_to_do() {
    let cases = [
        ("Stopped", "sudo tailscale up"),
        ("NeedsLogin", "https://login.tailscale.com/a/abc123"),
        ("NeedsMachineAuth", "admin/machines"),
    ];
    for (state, expected) in cases {
        let mut fx = Fixture::fresh();
        fx.runner = Default::default();
        fx.runner.on("tailscale version", out(0, "1.102.4", ""));
        fx.runner.on(
            "tailscale status --json",
            out(0, &tailscale_json(state, true, true), ""),
        );
        let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
        let fix = &report.step(StepId::Tailscale).expect("step").fix;
        assert!(
            fix.iter().any(|line| line.contains(expected)),
            "{state}: {fix:?}"
        );
    }
}

#[tokio::test]
async fn a_stopped_tailscale_that_is_switched_on_while_waiting_is_rechecked() {
    let mut fx = Fixture::fresh().answers(&["", "y", "y", "y", "q"]);
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Stopped", true, true), ""),
    );
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    fx.runner
        .on("tailscale serve status --json", out(0, "{}", ""));
    fx.runner.on(
        "tailscale serve status --json",
        out(0, &serve_json(443, TARGET, false), ""),
    );
    fx.runner.on(
        &format!("tailscale serve --bg --https=443 {TARGET}"),
        out(0, "", ""),
    );
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(state_of(&report, StepId::Tailscale), StepState::Done);
    assert!(fx.ui.said("switched off"));
}

#[tokio::test]
async fn missing_https_certificates_point_at_the_admin_console() {
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, false), ""),
    );
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let step = report.step(StepId::Tailscale).expect("step");
    assert_eq!(step.state, StepState::Blocked);
    assert!(step.detail.contains("HTTPS certificates"));
    assert!(
        step.fix
            .iter()
            .any(|l| l.contains("https://login.tailscale.com/admin/dns"))
    );
}

#[tokio::test]
async fn missing_magicdns_points_at_the_admin_console() {
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", false, true), ""),
    );
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let step = report.step(StepId::Tailscale).expect("step");
    assert!(step.detail.contains("MagicDNS"));
}

#[tokio::test]
async fn a_tailscale_service_that_is_not_running_reports_what_it_said() {
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(1, "", "failed to connect to local Tailscale service"),
    );
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let step = report.step(StepId::Tailscale).expect("step");
    assert!(
        step.fix
            .iter()
            .any(|l| l.contains("failed to connect to local Tailscale service")),
        "{:?}",
        step.fix
    );
    assert!(
        step.fix
            .iter()
            .any(|l| l.contains("systemctl enable --now tailscaled"))
    );
}

#[tokio::test]
async fn a_fresh_run_enables_publishes_and_verifies_in_order() {
    let mut fx = Fixture::fresh();
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    assert_eq!(state_of(&report, StepId::Tailscale), StepState::Done);
    assert_eq!(state_of(&report, StepId::Listener), StepState::Changed);
    assert_eq!(state_of(&report, StepId::Publish), StepState::Changed);
    assert_eq!(state_of(&report, StepId::Verify), StepState::Done);
    let origin = format!("https://{NODE}");
    assert_eq!(report.origin.as_deref(), Some(origin.as_str()));
    let (settings, revision) = fx.daemon.configures.last().expect("configured");
    assert_eq!(*revision, 0);
    assert!(settings.enabled && settings.pairing && settings.push);
    assert_eq!(settings.public_origins, [origin]);
    assert!(
        fx.runner
            .ran(&format!("tailscale serve --bg --https=443 {TARGET}"))
    );
    assert!(
        !fx.runner
            .ran(&format!("tailscale funnel --bg --https=443 {TARGET}"))
    );
}

#[tokio::test]
async fn rerunning_when_everything_is_set_up_changes_nothing() {
    let mut fx = Fixture::complete();
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    assert_eq!(report.status, Status::Complete);
    assert!(fx.daemon.configures.is_empty(), "no settings were saved");
    assert!(
        !fx.runner.ran_anything_but_probes(),
        "{:?}",
        fx.runner.calls.borrow()
    );
    for id in [
        StepId::Tailscale,
        StepId::Daemon,
        StepId::Listener,
        StepId::Publish,
        StepId::Verify,
        StepId::Pairing,
    ] {
        assert_eq!(state_of(&report, id), StepState::Done, "{id:?}");
    }
    assert_eq!(fx.daemon.opened, 0, "no new pairing is opened");
}

#[tokio::test]
async fn serve_already_used_for_something_else_moves_to_the_next_port() {
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    fx.runner.on(
        "tailscale serve status --json",
        out(0, &serve_json(443, "http://127.0.0.1:3000", false), ""),
    );
    fx.runner.on(
        "tailscale serve status --json",
        out(
            0,
            &serde_json::json!({
                "TCP": {"443": {"HTTPS": true}, "8443": {"HTTPS": true}},
                "Web": {
                    format!("{NODE}:443"): {"Handlers": {"/": {"Proxy": "http://127.0.0.1:3000"}}},
                    format!("{NODE}:8443"): {"Handlers": {"/": {"Proxy": TARGET}}},
                },
            })
            .to_string(),
            "",
        ),
    );
    fx.runner.on(
        &format!("tailscale serve --bg --https=8443 {TARGET}"),
        out(0, "", ""),
    );
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let origin = format!("https://{NODE}:8443");
    assert_eq!(report.origin.as_deref(), Some(origin.as_str()));
    assert_eq!(state_of(&report, StepId::Publish), StepState::Changed);
    assert!(
        !fx.runner
            .ran(&format!("tailscale serve --bg --https=443 {TARGET}")),
        "443 is left alone"
    );
    assert_eq!(
        fx.daemon
            .configures
            .last()
            .expect("configured")
            .0
            .public_origins,
        [origin]
    );
}

#[tokio::test]
async fn every_https_port_taken_blocks_with_the_way_out() {
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    let all = serde_json::json!({"Web": {
        format!("{NODE}:443"): {"Handlers": {"/": {"Proxy": "http://127.0.0.1:1"}}},
        format!("{NODE}:8443"): {"Handlers": {"/": {"Proxy": "http://127.0.0.1:2"}}},
        format!("{NODE}:10000"): {"Handlers": {"/": {"Proxy": "http://127.0.0.1:3"}}},
    }});
    fx.runner.on(
        "tailscale serve status --json",
        out(0, &all.to_string(), ""),
    );
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let step = report.step(StepId::Publish).expect("step");
    assert_eq!(step.state, StepState::Blocked);
    assert!(step.fix.iter().any(|l| l.contains("tailscale serve reset")));
    assert!(
        fx.daemon.configures.is_empty(),
        "nothing is enabled for an address that cannot be served"
    );
}

#[tokio::test]
async fn a_daemon_that_is_not_running_is_started_with_yes_and_by_consent_otherwise() {
    let mut fx = Fixture::fresh();
    fx.daemon.reachable = false;
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    assert!(fx.daemon.started);
    assert_eq!(state_of(&report, StepId::Daemon), StepState::Changed);

    let mut fx = Fixture::fresh().answers(&["y", "y", "y", "y", "q"]);
    fx.daemon.reachable = false;
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert!(fx.daemon.started);
    assert_eq!(state_of(&report, StepId::Daemon), StepState::Changed);
}

#[tokio::test]
async fn declining_to_start_the_daemon_stops_and_a_check_never_starts_it() {
    let mut fx = Fixture::fresh().answers(&["n"]);
    fx.daemon.reachable = false;
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert!(!fx.daemon.started);
    assert_eq!(report.status, Status::Stopped);

    let mut fx = Fixture::fresh();
    fx.daemon.reachable = false;
    let report = fx
        .run(&Options::default(), Mode::Check)
        .await
        .expect("runs");
    assert!(!fx.daemon.started);
    assert_eq!(report.status, Status::Pending);
    assert_eq!(
        report
            .step(StepId::Daemon)
            .and_then(|s| s.command.as_deref()),
        Some("aether daemon")
    );
}

#[tokio::test]
async fn a_check_run_changes_nothing_and_lists_what_it_would_do() {
    let mut fx = Fixture::fresh();
    let report = fx
        .run(&Options::default(), Mode::Check)
        .await
        .expect("runs");
    assert_eq!(report.status, Status::Pending);
    assert!(fx.daemon.configures.is_empty());
    assert!(
        !fx.runner
            .ran(&format!("tailscale serve --bg --https=443 {TARGET}"))
    );
    let listener = report.step(StepId::Listener).expect("step");
    assert_eq!(listener.state, StepState::Pending);
    assert!(
        listener
            .command
            .as_deref()
            .is_some_and(|c| c.starts_with("aether remote enable --origin https://"))
    );
    assert_eq!(state_of(&report, StepId::Verify), StepState::Skipped);
    assert_eq!(report.next, ["aether remote setup --yes"]);
}

#[tokio::test]
async fn a_listener_that_cannot_open_blocks_with_the_daemons_reason() {
    let mut fx = Fixture::fresh();
    fx.daemon.listen_error = Some("address already in use".to_owned());
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let step = report.step(StepId::Listener).expect("step");
    assert_eq!(step.state, StepState::Blocked);
    assert!(step.detail.contains("address already in use"));
    assert!(report.step(StepId::Publish).is_none());
}

#[tokio::test]
async fn a_listener_bound_off_loopback_is_never_published() {
    let mut fx = Fixture::fresh();
    fx.daemon.status.address = "0.0.0.0:8377".to_owned();
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    assert_eq!(state_of(&report, StepId::Listener), StepState::Blocked);
    assert!(!fx.runner.ran_anything_but_probes());
}

#[tokio::test]
async fn settings_changed_underneath_are_read_again_and_saved() {
    let mut fx = Fixture::fresh();
    fx.daemon.stale_once = true;
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    assert_eq!(state_of(&report, StepId::Listener), StepState::Changed);
    assert_eq!(
        fx.daemon.configures.last().expect("saved").1,
        1,
        "saved against the newer revision"
    );
}

#[tokio::test]
async fn other_origins_are_kept_and_the_new_one_leads() {
    let mut fx = Fixture::fresh();
    fx.daemon.status = remote_status(true, &["https://old.example.com"]);
    fx.daemon.status.settings.push = true;
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    assert_eq!(report.status, Status::Complete);
    let origins = &fx.daemon.configures.last().expect("saved").0.public_origins;
    assert_eq!(
        origins,
        &[
            format!("https://{NODE}"),
            "https://old.example.com".to_owned()
        ]
    );
}

#[tokio::test]
async fn a_rerun_never_turns_notifications_on_by_itself() {
    let mut fx = Fixture::fresh();
    fx.daemon.status = remote_status(true, &[&format!("https://{NODE}")]);
    fx.daemon.status.settings.push = false;
    fx.daemon.status.devices = 1;
    fx.daemon.devices = vec![super::fakes::device("phone")];
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    assert!(
        fx.daemon.configures.is_empty(),
        "push stays as the person left it"
    );
    assert_eq!(state_of(&report, StepId::Listener), StepState::Done);
}

#[tokio::test]
async fn declining_notifications_the_first_time_leaves_them_off() {
    let mut fx = Fixture::fresh().answers(&["n", "y", "y", "q"]);
    fx.run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert!(!fx.daemon.configures.last().expect("saved").0.push);
}

#[tokio::test]
async fn tailscale_refusing_a_non_operator_says_how_to_allow_it() {
    let mut fx = Fixture::fresh();
    let command = format!("tailscale serve --bg --https=443 {TARGET}");
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    fx.runner
        .on("tailscale serve status --json", out(0, "{}", ""));
    fx.runner.on(
        &command,
        out(
            1,
            "",
            "Access denied: serve config denied\nUse 'sudo tailscale set --operator=$USER'",
        ),
    );
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let step = report.step(StepId::Publish).expect("step");
    assert_eq!(step.state, StepState::Blocked);
    assert!(
        step.fix
            .iter()
            .any(|l| l.contains("sudo tailscale set --operator=$USER"))
    );
    assert!(
        step.detail.contains(&command),
        "what was run is shown: {}",
        step.detail
    );
}

#[tokio::test]
async fn after_the_operator_fix_the_same_command_is_tried_again() {
    let command = format!("tailscale serve --bg --https=443 {TARGET}");
    let mut fx = Fixture::fresh().answers(&["y", "y", "y", "", "q"]);
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    fx.runner
        .on("tailscale serve status --json", out(0, "{}", ""));
    fx.runner.on(
        "tailscale serve status --json",
        out(0, &serve_json(443, TARGET, false), ""),
    );
    fx.runner.on(&command, out(1, "", "Access denied"));
    fx.runner.on(&command, out(0, "", ""));
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(state_of(&report, StepId::Publish), StepState::Changed);
    assert_eq!(
        fx.runner
            .calls
            .borrow()
            .iter()
            .filter(|c| **c == command)
            .count(),
        2
    );
}

#[tokio::test]
async fn a_tailnet_feature_that_is_off_shows_the_page_that_turns_it_on() {
    let command = format!("tailscale serve --bg --https=443 {TARGET}");
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    fx.runner
        .on("tailscale serve status --json", out(0, "{}", ""));
    fx.runner.on(&command, out(1, "Serve is not enabled on your tailnet.\nTo enable, visit:\n\n\thttps://login.tailscale.com/f/serve?node=n123\n", ""));
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let fix = &report.step(StepId::Publish).expect("step").fix;
    assert!(
        fix.iter()
            .any(|l| l.contains("https://login.tailscale.com/f/serve?node=n123")),
        "{fix:?}"
    );
}

#[tokio::test]
async fn a_publish_that_waits_for_the_tailnet_admin_still_shows_the_enable_page() {
    let command = format!("tailscale serve --bg --https=443 {TARGET}");
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    fx.runner
        .on("tailscale serve status --json", out(0, "{}", ""));
    let printed = "Serve is not enabled on your tailnet.\nTo enable, visit:\n\n\thttps://login.tailscale.com/f/serve?node=n123\n";
    fx.runner
        .on(&command, Scripted::TimedOutSaying(partial(printed)));
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let step = report.step(StepId::Publish).expect("step");
    assert_eq!(step.state, StepState::Blocked);
    assert!(
        step.fix
            .iter()
            .any(|l| l.contains("https://login.tailscale.com/f/serve?node=n123")),
        "{:?}",
        step.fix
    );
}

#[tokio::test]
async fn a_publish_that_times_out_saying_nothing_known_is_an_error_with_what_it_said() {
    let command = format!("tailscale serve --bg --https=443 {TARGET}");
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    fx.runner
        .on("tailscale serve status --json", out(0, "{}", ""));
    fx.runner.on(
        &command,
        Scripted::TimedOutSaying(partial("Waiting for the certificate...")),
    );
    let error = fx.run(&yes(), Mode::Yes).await.expect_err("timed out");
    let text = format!("{error:#}");
    assert!(
        text.contains("did not finish within 60s") && text.contains("Waiting for the certificate"),
        "{text}"
    );
}

fn partial(stdout: &str) -> super::super::runner::Output {
    super::super::runner::Output {
        code: None,
        stdout: stdout.to_owned(),
        stderr: String::new(),
    }
}

#[tokio::test]
async fn an_unexplained_tailscale_failure_shows_the_command_and_its_message() {
    let command = format!("tailscale serve --bg --https=443 {TARGET}");
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    fx.runner
        .on("tailscale serve status --json", out(0, "{}", ""));
    fx.runner
        .on(&command, out(2, "", "flag provided but not defined: -bg"));
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let step = report.step(StepId::Publish).expect("step");
    assert!(
        step.detail.contains(&command) && step.detail.contains("flag provided but not defined")
    );
    assert!(step.fix.iter().any(|l| l.contains(&command)));
}

#[tokio::test]
async fn an_address_that_never_answers_blocks_but_a_person_may_carry_on() {
    let down = || Err(ProbeError::Unreachable("dns error".to_owned()));
    let mut fx = Fixture::fresh();
    fx.env.health = std::cell::RefCell::new(VecDeque::from([down()]));
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    assert_eq!(state_of(&report, StepId::Verify), StepState::Blocked);
    assert!(fx.env.sleeps.get() >= 1, "it waited for the certificate");
    assert!(report.step(StepId::Pairing).is_none());

    let mut fx = Fixture::fresh().answers(&["y", "y", "y", "y", "q"]);
    fx.env.health = std::cell::RefCell::new(VecDeque::from([down()]));
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(state_of(&report, StepId::Verify), StepState::Warning);
    assert!(report.step(StepId::Pairing).is_some());
}

#[tokio::test]
async fn a_foreign_certificate_is_explained_as_a_program_on_this_computer() {
    let reason = "invalid peer certificate: UnknownIssuer";
    let mut fx = Fixture::fresh();
    fx.env.health = std::cell::RefCell::new(VecDeque::from([Err(ProbeError::Unreachable(
        reason.to_owned(),
    ))]));
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let step = report.step(StepId::Verify).expect("verified");
    assert!(step.detail.contains(reason), "{}", step.detail);
    assert!(
        step.fix.iter().any(|l| l.contains("not your phone")),
        "{:?}",
        step.fix
    );
}

#[tokio::test]
async fn an_address_that_comes_up_on_a_later_attempt_is_verified() {
    let mut fx = Fixture::fresh();
    fx.env.health = std::cell::RefCell::new(VecDeque::from([
        Err(ProbeError::Unreachable("still starting".to_owned())),
        Err(ProbeError::Unreachable("still starting".to_owned())),
        healthy(),
    ]));
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    assert_eq!(state_of(&report, StepId::Verify), StepState::Done);
    assert_eq!(fx.env.sleeps.get(), 2);
}

#[tokio::test]
async fn a_forbidden_host_is_not_retried_and_points_at_the_allowlist() {
    let mut fx = Fixture::fresh();
    fx.env.health = std::cell::RefCell::new(VecDeque::from([Err(ProbeError::Status(403))]));
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let step = report.step(StepId::Verify).expect("step");
    assert_eq!(step.state, StepState::Blocked);
    assert_eq!(fx.env.sleeps.get(), 0);
    assert!(step.fix.iter().any(|l| l.contains("aether remote status")));
}

#[tokio::test]
async fn a_proxy_url_skips_tailscale_entirely() {
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    let options = Options {
        yes: true,
        proxy_url: Some("https://aether.example.com".to_owned()),
        ..Options::default()
    };
    let report = fx.run(&options, Mode::Yes).await.expect("runs");
    assert_eq!(report.status, Status::Complete);
    assert!(
        fx.runner.calls.borrow().is_empty(),
        "tailscale is never run"
    );
    assert_eq!(state_of(&report, StepId::Tailscale), StepState::Skipped);
    assert_eq!(state_of(&report, StepId::Publish), StepState::Skipped);
    assert_eq!(report.origin.as_deref(), Some("https://aether.example.com"));
    assert_eq!(
        fx.daemon.configures.last().expect("saved").0.public_origins,
        ["https://aether.example.com"]
    );
    assert!(
        report
            .step(StepId::Publish)
            .expect("step")
            .detail
            .contains(TARGET)
    );
}

#[tokio::test]
async fn funnel_needs_the_flag_and_the_word_public_and_says_it_is_public() {
    let funnel = Options {
        funnel: true,
        ..Options::default()
    };
    let command = format!("tailscale funnel --bg --https=443 {TARGET}");
    let mut fx = Fixture::fresh();
    fx.runner.on(&command, out(0, "", ""));
    fx.ui.answers = ["n", "y", "wrong"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let report = fx.run(&funnel, Mode::Interactive).await.expect("runs");
    assert_eq!(state_of(&report, StepId::Publish), StepState::Stopped);
    assert!(!fx.runner.ran(&command), "a wrong word runs nothing");
    assert!(fx.ui.said("PUBLIC INTERNET"));
}

#[tokio::test]
async fn funnel_with_yes_runs_the_funnel_command_not_serve() {
    let funnel = Options {
        yes: true,
        funnel: true,
        ..Options::default()
    };
    let command = format!("tailscale funnel --bg --https=443 {TARGET}");
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    fx.runner
        .on("tailscale serve status --json", out(0, "{}", ""));
    fx.runner.on(
        "tailscale serve status --json",
        out(0, &serve_json(443, TARGET, true), ""),
    );
    fx.runner.on(&command, out(0, "", ""));
    let report = fx.run(&funnel, Mode::Yes).await.expect("runs");
    assert!(fx.runner.ran(&command));
    assert!(
        !fx.runner
            .ran(&format!("tailscale serve --bg --https=443 {TARGET}"))
    );
    assert_eq!(state_of(&report, StepId::Publish), StepState::Changed);
}

#[tokio::test]
async fn funnel_already_open_without_the_flag_is_a_loud_warning() {
    let mut fx = Fixture::complete();
    fx.runner = super::fakes::FakeRunner::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    fx.runner.on(
        "tailscale serve status --json",
        out(0, &serve_json(443, TARGET, true), ""),
    );
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let step = report.step(StepId::Publish).expect("step");
    assert_eq!(step.state, StepState::Warning);
    assert!(step.detail.contains("OPEN TO THE WHOLE INTERNET"));
    assert!(!fx.runner.ran_anything_but_probes());
}

#[tokio::test]
async fn an_unreadable_serve_status_is_an_error_naming_the_command() {
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    fx.runner
        .on("tailscale serve status --json", out(0, "not json", ""));
    let error = fx.run(&yes(), Mode::Yes).await.expect_err("fails");
    assert!(
        format!("{error:#}").contains("tailscale serve status --json"),
        "{error:#}"
    );
}

#[tokio::test]
async fn a_tailscale_command_that_hangs_is_reported_with_what_was_run() {
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on("tailscale status --json", Scripted::TimedOut);
    let error = fx.run(&yes(), Mode::Yes).await.expect_err("fails");
    let text = format!("{error:#}");
    assert!(
        text.contains("tailscale status --json") && text.contains("did not finish"),
        "{text}"
    );
}

#[test]
fn a_stale_revision_is_recognised_and_other_failures_are_not() {
    use super::super::wizard::is_stale_revision;
    assert!(is_stale_revision(&stale_revision_error()));
    assert!(!is_stale_revision(&protocol_error(
        ErrorCode::InvalidArgument
    )));
    assert!(!is_stale_revision(&protocol_error(ErrorCode::Forbidden)));
    assert!(!is_stale_revision(&anyhow::anyhow!("other")));
}

#[tokio::test]
async fn a_refusal_that_is_not_a_stale_revision_is_reported_as_it_was_said() {
    let mut fx = Fixture::fresh();
    fx.daemon.refusal = Some("public origin `http://x` must use https".into());
    let error = fx.run(&yes(), Mode::Yes).await.expect_err("refused");
    let text = format!("{error:#}");
    assert!(text.contains("must use https"), "{text}");
    assert!(!text.contains("changed underneath"), "{text}");
    assert!(
        fx.daemon.configures.is_empty(),
        "no second attempt was made"
    );
}

#[tokio::test]
async fn giving_up_on_tailscale_after_many_rechecks_is_a_blocked_step_not_a_clean_finish() {
    let mut fx = Fixture::fresh().answers(&[""; 40]);
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Stopped", true, true), ""),
    );
    let report = fx
        .run(&Options::default(), Mode::Interactive)
        .await
        .expect("runs");
    assert_eq!(state_of(&report, StepId::Tailscale), StepState::Blocked);
    assert_ne!(report.status, Status::Complete, "{:?}", report.steps);
    assert!(
        !report.step(StepId::Tailscale).expect("step").fix.is_empty(),
        "it says what to do"
    );
}

#[tokio::test]
async fn a_rerun_keeps_pairing_switched_off_and_says_how_to_turn_it_on() {
    let mut fx = Fixture::complete();
    fx.daemon.status.settings.pairing = false;
    fx.daemon.status.settings.public_origins.clear();
    fx.daemon.devices.clear();
    fx.daemon.status.devices = 0;
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let (saved, _) = fx.daemon.configures.last().expect("origin saved");
    assert!(!saved.pairing, "the setting the operator chose stands");
    let step = report.step(StepId::Pairing).expect("pairing step");
    assert_eq!(step.state, StepState::Skipped);
    assert!(
        step.command
            .as_deref()
            .is_some_and(|c| !c.contains("--no-pairing")),
        "{step:?}"
    );
}

#[tokio::test]
async fn the_first_run_switches_pairing_on() {
    let mut fx = Fixture::fresh();
    fx.run(&yes(), Mode::Yes).await.expect("runs");
    assert!(fx.daemon.configures.last().expect("saved").0.pairing);
}

#[tokio::test]
async fn an_unreadable_serve_status_blocks_instead_of_guessing_a_free_port() {
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    fx.runner.on(
        "tailscale serve status --json",
        out(0, r#"{"Mystery":{"x":1}}"#, ""),
    );
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    let step = report.step(StepId::Publish).expect("step");
    assert_eq!(step.state, StepState::Blocked);
    assert!(step.detail.contains("could not tell"), "{}", step.detail);
    assert!(
        !fx.runner.ran_anything_but_probes(),
        "nothing was published"
    );
}
