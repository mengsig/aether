//! Funnel and mounts never meet: setup refuses either Funnel while the daemon mounts anything.

use aether_protocol::remote::{MountKind, RemoteMount};

use super::super::Options;
use super::super::report::{Mode, Status, StepId, StepState};
use super::fakes::{FakeRunner, Fixture, TARGET, out, serve_json, tailscale_json};
use super::flows::{state_of, yes};

fn archon_mount() -> RemoteMount {
    RemoteMount {
        prefix: "/archon".into(),
        kind: MountKind::Archon,
        target: "http://127.0.0.1:8378".into(),
    }
}

fn mounting(mut fx: Fixture) -> Fixture {
    fx.daemon.status.settings.mounts = Some(vec![archon_mount()]);
    fx
}

#[tokio::test]
async fn funnel_is_not_set_up_while_a_mount_is_present() {
    let funnel = Options {
        yes: true,
        funnel: true,
        ..Options::default()
    };
    let mut fx = mounting(Fixture::fresh());
    let report = fx.run(&funnel, Mode::Yes).await.expect("runs");
    assert_eq!(report.status, Status::Blocked);
    let step = report.step(StepId::Publish).expect("a publish step");
    assert_eq!(step.state, StepState::Blocked);
    assert!(step.detail.contains("/archon"), "{}", step.detail);
    assert!(
        step.fix
            .iter()
            .any(|fix| fix.contains("aether remote mount remove /archon"))
    );
    assert!(
        !fx.runner.ran_anything_but_probes(),
        "nothing was published"
    );
    assert!(
        fx.daemon.configures.is_empty(),
        "the listener was not changed"
    );
}

#[tokio::test]
async fn an_open_funnel_is_not_kept_while_a_mount_is_present() {
    let mut fx = mounting(Fixture::complete());
    fx.runner = FakeRunner::default();
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
    assert_eq!(state_of(&report, StepId::Publish), StepState::Blocked);
    let step = report.step(StepId::Publish).expect("a publish step");
    assert!(
        step.fix
            .iter()
            .any(|fix| fix.contains("box.tail1234.ts.net:443"))
    );
    assert!(!fx.runner.ran_anything_but_probes());
    assert!(fx.daemon.configures.is_empty());
}

#[tokio::test]
async fn a_private_setup_with_a_mount_goes_ahead() {
    let mut fx = mounting(Fixture::fresh());
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    assert_ne!(state_of(&report, StepId::Publish), StepState::Blocked);
    let (asked, _) = fx
        .daemon
        .configures
        .first()
        .expect("the listener was configured");
    assert_eq!(
        asked.mounts, None,
        "setup keeps the saved mounts as they are"
    );
}

#[tokio::test]
async fn a_funnel_on_another_port_is_not_kept_while_a_mount_is_present() {
    let mut fx = mounting(Fixture::complete());
    fx.runner = FakeRunner::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &tailscale_json("Running", true, true), ""),
    );
    let private_443 = serve_json(443, TARGET, false);
    let mut both: serde_json::Value = serde_json::from_str(&private_443).expect("json");
    both["AllowFunnel"] = serde_json::json!({"box.tail1234.ts.net:8443": true});
    fx.runner.on(
        "tailscale serve status --json",
        out(0, &both.to_string(), ""),
    );
    let report = fx.run(&yes(), Mode::Yes).await.expect("runs");
    assert_eq!(state_of(&report, StepId::Publish), StepState::Blocked);
    let step = report.step(StepId::Publish).expect("a publish step");
    assert!(
        step.fix.iter().any(|fix| fix.contains(":8443")),
        "{:?}",
        step.fix
    );
    assert!(fx.daemon.configures.is_empty());
}

#[tokio::test]
async fn a_mount_saved_while_setup_runs_stops_the_funnel_before_it_opens() {
    let funnel = Options {
        yes: true,
        funnel: true,
        ..Options::default()
    };
    let command = format!("tailscale funnel --bg --https=443 {TARGET}");
    let mut fx = Fixture::fresh();
    fx.runner.on(&command, out(0, "", ""));
    fx.daemon.stale_once = true;
    fx.daemon.mount_on_stale = Some(archon_mount());
    let report = fx.run(&funnel, Mode::Yes).await.expect("runs");
    assert_eq!(
        fx.daemon.configures.len(),
        1,
        "saved once, after the stale read"
    );
    assert_eq!(state_of(&report, StepId::Publish), StepState::Blocked);
    let step = report.step(StepId::Publish).expect("a publish step");
    assert!(step.detail.contains("/archon"), "{}", step.detail);
    assert!(!fx.runner.ran(&command), "Funnel was never turned on");
}
