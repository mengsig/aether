//! Funnel and mounts never meet: setup refuses either Funnel while the daemon mounts anything.

use aether_protocol::remote::{MountKind, RemoteMount};

use super::super::Options;
use super::super::report::{Mode, Status, StepId, StepState};
use super::fakes::{FakeRunner, Fixture, TARGET, out, serve_json, tailscale_json};
use super::flows::{state_of, yes};

fn mounting(mut fx: Fixture) -> Fixture {
    fx.daemon.status.settings.mounts = Some(vec![RemoteMount {
        prefix: "/archon".into(),
        kind: MountKind::Archon,
        target: "http://127.0.0.1:8378".into(),
    }]);
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
            .any(|fix| fix.contains("tailscale funnel --https=443 off"))
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
