//! What `--json` prints, pinned.

use serde_json::{Value, json};

use super::super::Options;
use super::super::report::Mode;
use super::fakes::{Fixture, out};

fn parsed(report: &super::super::report::Report) -> Value {
    serde_json::from_str(&report.to_json().expect("serializes")).expect("valid JSON")
}

#[tokio::test]
async fn a_check_of_a_machine_that_is_fully_set_up() {
    let mut fx = Fixture::complete();
    let report = fx
        .run(&Options::default(), Mode::Check)
        .await
        .expect("runs");
    assert_eq!(
        parsed(&report),
        json!({
            "status": "complete",
            "mode": "check",
            "platform": "linux-arch",
            "origin": "https://box.tail1234.ts.net",
            "steps": [
                {"id": "tailscale", "state": "done", "detail": "connected as box.tail1234.ts.net"},
                {"id": "daemon", "state": "done", "detail": "the daemon is running"},
                {"id": "listener", "state": "done",
                 "detail": "listening on 127.0.0.1:8377, for https://box.tail1234.ts.net"},
                {"id": "publish", "state": "done", "detail": "already published at https://box.tail1234.ts.net"},
                {"id": "verify", "state": "done",
                 "detail": "https://box.tail1234.ts.net answers, so Aether accepts this address"},
                {"id": "pairing", "state": "done", "detail": "1 device(s) already paired"},
            ],
            "next": [],
        })
    );
}

#[tokio::test]
async fn a_check_of_a_fresh_machine_lists_the_commands_it_would_run() {
    let mut fx = Fixture::fresh();
    let report = fx
        .run(&Options::default(), Mode::Check)
        .await
        .expect("runs");
    assert_eq!(
        parsed(&report),
        json!({
            "status": "pending",
            "mode": "check",
            "platform": "linux-arch",
            "origin": "https://box.tail1234.ts.net",
            "steps": [
                {"id": "tailscale", "state": "done", "detail": "connected as box.tail1234.ts.net"},
                {"id": "daemon", "state": "done", "detail": "the daemon is running"},
                {"id": "listener", "state": "pending",
                 "detail": "remote access would be turned on for https://box.tail1234.ts.net",
                 "command": "aether remote enable --origin https://box.tail1234.ts.net --no-push"},
                {"id": "publish", "state": "pending",
                 "detail": "Tailscale would publish http://127.0.0.1:8377 at https://box.tail1234.ts.net",
                 "command": "tailscale serve --bg --https=443 http://127.0.0.1:8377"},
                {"id": "verify", "state": "skipped",
                 "detail": "not checked: earlier steps still have changes to make"},
                {"id": "pairing", "state": "skipped",
                 "detail": "pairing needs a person at the terminal", "command": "aether remote pair"},
            ],
            "next": ["aether remote setup --yes"],
        })
    );
}

#[tokio::test]
async fn a_blocked_run_carries_the_instructions_to_unblock_it() {
    let mut fx = Fixture::fresh();
    fx.runner = Default::default();
    fx.runner.on("tailscale version", out(0, "1.102.4", ""));
    fx.runner.on(
        "tailscale status --json",
        out(0, &super::fakes::tailscale_json("Running", true, false), ""),
    );
    let options = Options {
        yes: true,
        ..Options::default()
    };
    let report = fx.run(&options, Mode::Yes).await.expect("runs");
    let value = parsed(&report);
    assert_eq!(value["status"], "blocked");
    assert_eq!(value["mode"], "yes");
    assert_eq!(value["next"], json!(["aether remote setup"]));
    let step = &value["steps"][0];
    assert_eq!(step["id"], "tailscale");
    assert_eq!(step["state"], "blocked");
    assert_eq!(
        step["detail"],
        "HTTPS certificates are off for your tailnet. Phones only run Aether over HTTPS."
    );
    assert_eq!(step["fix"][0], "Open https://login.tailscale.com/admin/dns");
}

#[tokio::test]
async fn the_report_never_contains_the_pairing_code_or_the_tailnet_name() {
    let mut fx = Fixture::fresh();
    let options = Options {
        yes: true,
        ..Options::default()
    };
    let report = fx.run(&options, Mode::Yes).await.expect("runs");
    let text = report.to_json().expect("serializes");
    assert!(!text.contains("7K2M"), "{text}");
    assert!(!text.contains("someone@example.com"), "{text}");
}
