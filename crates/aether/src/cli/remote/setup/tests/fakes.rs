//! Fakes for every seam of the wizard, and the fixture that wires them.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use aether_client::ClientError;
use aether_protocol::remote::{
    Device, DeviceId, DeviceIdentity, DeviceRole, Health, Pairing, PairingCandidate, PairingState,
    RemoteSettings, RemoteStatus,
};
use aether_protocol::{ErrorCode, ErrorReply};
use serde_json::json;

use super::super::daemon::Daemon;
use super::super::environment::{Environment, ProbeError};
use super::super::platform::{Distro, Platform};
use super::super::report::{Mode, Report};
use super::super::runner::{CommandRunner, Output, RunError, display};
use super::super::ui::Ui;
use super::super::wizard::Wizard;
use super::super::{Options, origin_of};

pub(super) const NODE: &str = "box.tail1234.ts.net";
pub(super) const ADDRESS: &str = "127.0.0.1:8377";
pub(super) const TARGET: &str = "http://127.0.0.1:8377";

pub(super) enum Scripted {
    Out(Output),
    NotFound,
    TimedOut,
    /// Killed at the timeout after printing this much.
    TimedOutSaying(Output),
}

pub(super) fn out(code: i32, stdout: &str, stderr: &str) -> Scripted {
    Scripted::Out(Output {
        code: Some(code),
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
    })
}

/// Answers by full command line; the last answer for a line repeats.
#[derive(Default)]
pub(super) struct FakeRunner {
    answers: RefCell<HashMap<String, VecDeque<Scripted>>>,
    pub(super) calls: RefCell<Vec<String>>,
}

impl FakeRunner {
    pub(super) fn on(&self, line: &str, answer: Scripted) {
        self.answers
            .borrow_mut()
            .entry(line.to_owned())
            .or_default()
            .push_back(answer);
    }

    pub(super) fn ran(&self, line: &str) -> bool {
        self.calls.borrow().iter().any(|call| call == line)
    }

    pub(super) fn ran_anything_but_probes(&self) -> bool {
        self.calls
            .borrow()
            .iter()
            .any(|c| !c.ends_with(" version") && !c.contains(" status"))
    }
}

impl CommandRunner for FakeRunner {
    async fn run(
        &self,
        program: &str,
        args: &[&str],
        _timeout: Duration,
    ) -> Result<Output, RunError> {
        let line = display(program, args);
        self.calls.borrow_mut().push(line.clone());
        let mut answers = self.answers.borrow_mut();
        let Some(queue) = answers.get_mut(&line) else {
            return Err(RunError::NotFound);
        };
        let answer = if queue.len() > 1 {
            queue.pop_front()
        } else {
            None
        };
        let answer = match &answer {
            Some(answer) => answer,
            None => queue.front().expect("a scripted answer"),
        };
        match answer {
            Scripted::Out(output) => Ok(output.clone()),
            Scripted::NotFound => Err(RunError::NotFound),
            Scripted::TimedOut => Err(RunError::TimedOut {
                limit: Duration::from_secs(5),
                partial: Output::default(),
            }),
            Scripted::TimedOutSaying(partial) => Err(RunError::TimedOut {
                limit: Duration::from_secs(60),
                partial: partial.clone(),
            }),
        }
    }
}

pub(super) fn stale_revision_error() -> anyhow::Error {
    let message = aether_protocol::remote::stale_revision_message(1, 0);
    anyhow::Error::new(ClientError::Protocol(ErrorReply::new(
        ErrorCode::InvalidArgument,
        message,
    )))
}

pub(super) fn protocol_error(code: ErrorCode) -> anyhow::Error {
    anyhow::Error::new(ClientError::Protocol(ErrorReply::new(code, "scripted")))
}

#[derive(Default)]
pub(super) struct FakeDaemon {
    pub(super) reachable: bool,
    pub(super) started: bool,
    pub(super) status: RemoteStatus,
    /// The listener cannot open: `error` is reported after configuring.
    pub(super) listen_error: Option<String>,
    pub(super) stale_once: bool,
    /// The daemon refuses the next configure with this `invalid_argument`.
    pub(super) refusal: Option<String>,
    pub(super) configures: Vec<(RemoteSettings, u64)>,
    pub(super) devices: Vec<Device>,
    pub(super) opened: u32,
    /// What `get_pairing` returns, in order; the last repeats.
    pub(super) pairing_states: VecDeque<Pairing>,
    pub(super) confirm_error: bool,
    pub(super) confirmations: Vec<bool>,
    pub(super) cancelled: Vec<String>,
}

pub(super) fn remote_status(enabled: bool, origins: &[&str]) -> RemoteStatus {
    RemoteStatus {
        revision: if enabled { 3 } else { 0 },
        settings: RemoteSettings {
            enabled,
            pairing: enabled,
            public_origins: origins.iter().map(|o| (*o).to_owned()).collect(),
            push: false,
        },
        address: ADDRESS.to_owned(),
        listening: enabled.then(|| ADDRESS.to_owned()),
        ..RemoteStatus::default()
    }
}

pub(super) fn pairing(state: PairingState) -> Pairing {
    Pairing {
        id: "pair-1".to_owned(),
        state,
        role: DeviceRole::Operator,
        expires_at: aether_core::time::now(),
        code: None,
        url: None,
        candidate: None,
        device: None,
    }
}

pub(super) fn claimed(name: &str) -> Pairing {
    Pairing {
        candidate: Some(PairingCandidate {
            name: name.to_owned(),
            fingerprint: "9f2c41aa07b3de5512345678".to_owned(),
            phrase: "amber-otter-canyon".to_owned(),
            claimed_at: aether_core::time::now(),
        }),
        ..pairing(PairingState::Claimed)
    }
}

pub(super) fn enrolled(name: &str) -> Pairing {
    Pairing {
        device: Some(DeviceIdentity {
            id: DeviceId::new(),
            name: name.to_owned(),
            role: DeviceRole::Operator,
        }),
        ..pairing(PairingState::Enrolled)
    }
}

pub(super) fn device(name: &str) -> Device {
    Device {
        id: DeviceId::new(),
        name: name.to_owned(),
        role: DeviceRole::Operator,
        fingerprint: None,
        created_at: aether_core::time::now(),
        last_seen_at: None,
        current: false,
        revoked_at: None,
        connections: 0,
        push: false,
    }
}

impl FakeDaemon {
    pub(super) fn running(status: RemoteStatus) -> Self {
        Self {
            reachable: true,
            status,
            ..Self::default()
        }
    }

    fn next_pairing(&mut self) -> Pairing {
        if self.pairing_states.len() > 1 {
            return self.pairing_states.pop_front().expect("a scripted pairing");
        }
        self.pairing_states
            .front()
            .cloned()
            .expect("a scripted pairing")
    }
}

impl Daemon for FakeDaemon {
    async fn connect(&mut self) -> anyhow::Result<bool> {
        Ok(self.reachable)
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        self.reachable = true;
        self.started = true;
        Ok(())
    }

    async fn remote(&mut self) -> anyhow::Result<RemoteStatus> {
        Ok(self.status.clone())
    }

    async fn configure(
        &mut self,
        settings: RemoteSettings,
        expected_revision: u64,
    ) -> anyhow::Result<RemoteStatus> {
        if let Some(message) = self.refusal.take() {
            return Err(anyhow::Error::new(ClientError::Protocol(ErrorReply::new(
                ErrorCode::InvalidArgument,
                message,
            ))));
        }
        if std::mem::take(&mut self.stale_once) {
            self.status.revision += 1;
            return Err(stale_revision_error());
        }
        assert_eq!(
            expected_revision, self.status.revision,
            "a stale revision reached the daemon"
        );
        self.configures.push((settings.clone(), expected_revision));
        self.status.revision += 1;
        self.status.listening =
            (settings.enabled && self.listen_error.is_none()).then(|| ADDRESS.to_owned());
        self.status.error = self.listen_error.clone();
        self.status.settings = settings;
        Ok(self.status.clone())
    }

    async fn start_pairing(&mut self, _role: DeviceRole) -> anyhow::Result<Pairing> {
        self.opened += 1;
        let origin = self
            .status
            .settings
            .public_origins
            .first()
            .cloned()
            .unwrap_or_default();
        Ok(Pairing {
            code: Some("7K2M-9QXD".to_owned()),
            url: Some(format!("{origin}/pair#code=7K2M-9QXD")),
            ..pairing(PairingState::Open)
        })
    }

    async fn pairing(&mut self, _id: &str) -> anyhow::Result<Pairing> {
        Ok(self.next_pairing())
    }

    async fn confirm_pairing(&mut self, _id: &str, accept: bool) -> anyhow::Result<Pairing> {
        self.confirmations.push(accept);
        if self.confirm_error {
            return Err(protocol_error(ErrorCode::InvalidTransition));
        }
        Ok(if accept {
            enrolled("Robin's iPhone")
        } else {
            pairing(PairingState::Rejected)
        })
    }

    async fn cancel_pairing(&mut self, id: &str) -> anyhow::Result<Pairing> {
        self.cancelled.push(id.to_owned());
        Ok(pairing(PairingState::Cancelled))
    }

    async fn devices(&mut self) -> anyhow::Result<Vec<Device>> {
        Ok(self.devices.clone())
    }
}

pub(super) struct FakeEnv {
    pub(super) platform: Platform,
    pub(super) health: RefCell<VecDeque<Result<Health, ProbeError>>>,
    pub(super) sleeps: Cell<u32>,
    pub(super) interrupt: bool,
}

pub(super) fn healthy() -> Result<Health, ProbeError> {
    Ok(Health {
        status: "ok".to_owned(),
        protocol_version: 18,
        min_protocol_version: 17,
        pairing_open: false,
    })
}

impl Environment for FakeEnv {
    fn platform(&self) -> Platform {
        self.platform.clone()
    }

    async fn sleep(&self, _duration: Duration) {
        self.sleeps.set(self.sleeps.get() + 1);
        tokio::task::yield_now().await;
    }

    async fn health(&self, _origin: &str) -> Result<Health, ProbeError> {
        let mut queue = self.health.borrow_mut();
        if queue.len() > 1 {
            queue.pop_front().expect("scripted")
        } else {
            queue.front().cloned().expect("scripted")
        }
    }

    async fn interrupted(&self) {
        if !self.interrupt {
            std::future::pending::<()>().await;
        }
    }
}

pub(super) struct FakeUi {
    pub(super) asking: bool,
    pub(super) answers: VecDeque<String>,
    pub(super) said: Vec<String>,
    pub(super) asked: Vec<String>,
}

impl FakeUi {
    pub(super) fn said(&self, needle: &str) -> bool {
        self.said.iter().any(|line| line.contains(needle))
    }
}

impl Ui for FakeUi {
    fn can_ask(&self) -> bool {
        self.asking
    }

    fn say(&mut self, text: &str) {
        self.said.push(text.to_owned());
    }

    fn ask(&mut self, prompt: &str) -> anyhow::Result<String> {
        self.asked.push(prompt.to_owned());
        self.answers
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("no answer scripted for `{prompt}`"))
    }
}

pub(super) struct Fixture {
    pub(super) runner: FakeRunner,
    pub(super) daemon: FakeDaemon,
    pub(super) env: FakeEnv,
    pub(super) ui: FakeUi,
}

/// `tailscale status --json` for a machine in `state`, with the features on.
pub(super) fn tailscale_json(state: &str, magic_dns: bool, certs: bool) -> String {
    json!({
        "BackendState": state,
        "AuthURL": if state == "NeedsLogin" { "https://login.tailscale.com/a/abc123" } else { "" },
        "Self": {"DNSName": format!("{NODE}.")},
        "CurrentTailnet": {"Name": "someone@example.com", "MagicDNSEnabled": magic_dns},
        "CertDomains": if certs { vec![NODE] } else { vec![] },
    })
    .to_string()
}

pub(super) fn serve_json(port: u16, target: &str, funnel: bool) -> String {
    let key = format!("{NODE}:{port}");
    let mut config = json!({
        "TCP": {port.to_string(): {"HTTPS": true}},
        "Web": {key.clone(): {"Handlers": {"/": {"Proxy": target}}}},
    });
    if funnel {
        config["AllowFunnel"] = json!({key: true});
    }
    config.to_string()
}

impl Fixture {
    /// Tailscale installed and ready, nothing served; the daemon running
    /// with remote access off; a healthy address; a phone that never asks.
    pub(super) fn fresh() -> Self {
        let runner = FakeRunner::default();
        runner.on("tailscale version", out(0, "1.102.4", ""));
        runner.on(
            "tailscale status --json",
            out(0, &tailscale_json("Running", true, true), ""),
        );
        runner.on("tailscale serve status --json", out(0, "{}", ""));
        runner.on(
            "tailscale serve status --json",
            out(0, &serve_json(443, TARGET, false), ""),
        );
        runner.on(
            &format!("tailscale serve --bg --https=443 {TARGET}"),
            out(0, "", ""),
        );
        Self {
            runner,
            daemon: FakeDaemon::running(remote_status(false, &[])),
            env: FakeEnv {
                platform: Platform::Linux(Distro::Arch),
                health: RefCell::new(VecDeque::from([healthy()])),
                sleeps: Cell::new(0),
                interrupt: false,
            },
            ui: FakeUi {
                asking: false,
                answers: VecDeque::new(),
                said: Vec::new(),
                asked: Vec::new(),
            },
        }
    }

    /// A person at the terminal who will give `answers` in order.
    pub(super) fn answers(mut self, answers: &[&str]) -> Self {
        self.ui.asking = true;
        self.ui.answers = answers.iter().map(|a| (*a).to_owned()).collect();
        self
    }

    /// What `get_pairing` reports, in order; the last state repeats.
    pub(super) fn pairings(mut self, states: Vec<Pairing>) -> Self {
        self.daemon.pairing_states = states.into();
        self
    }

    /// Everything already set up: serving, enabled, one device paired.
    pub(super) fn complete() -> Self {
        let mut fx = Self::fresh();
        fx.runner = FakeRunner::default();
        fx.runner.on("tailscale version", out(0, "1.102.4", ""));
        fx.runner.on(
            "tailscale status --json",
            out(0, &tailscale_json("Running", true, true), ""),
        );
        fx.runner.on(
            "tailscale serve status --json",
            out(0, &serve_json(443, TARGET, false), ""),
        );
        let origin = format!("https://{NODE}");
        fx.daemon = FakeDaemon::running(remote_status(true, &[&origin]));
        fx.daemon.status.settings.push = true;
        fx.daemon.status.devices = 1;
        fx.daemon.devices = vec![device("Robin's iPhone")];
        fx
    }

    pub(super) async fn run(&mut self, options: &Options, mode: Mode) -> anyhow::Result<Report> {
        let proxy = options.proxy_url.as_deref().map(origin_of).transpose()?;
        Wizard::new(
            options,
            mode,
            proxy,
            &self.runner,
            &mut self.daemon,
            &self.env,
            &mut self.ui,
        )
        .run()
        .await
    }
}
