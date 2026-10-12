//! The wizard's state machine: Tailscale, the daemon's listener, publishing
//! it, and checking it answers. Pairing a phone is in `pairing.rs`.

use std::time::Duration;

use aether_client::ClientError;
use aether_protocol::remote::{RemoteSettings, RemoteStatus};
use anyhow::{Context as _, bail};

use super::Options;
use super::daemon::Daemon;
use super::environment::Environment;
use super::guidance::{self, Guidance};
use super::platform::Platform;
use super::report::{Mode, Report, Step, StepId, StepState};
use super::runner::CommandRunner;
use super::tailscale::{Node, PortUse, SERVE_PORTS, Tailscale, TailscaleState};
use super::ui::{Ui, Wait};

/// Re-checks of Tailscale after a person says they fixed something.
const MAX_RECHECKS: u32 = 20;
/// Times a still-connecting Tailscale is given a moment before it is a problem.
const CONNECT_WAITS: u32 = 5;
const CONNECT_PAUSE: Duration = Duration::from_secs(2);

/// Whether a step lets the next one run.
#[derive(Debug)]
pub(super) enum Flow<T> {
    Go(T),
    /// The step has recorded why the run ends here.
    Stop,
}

/// How phones will reach the daemon.
#[derive(Debug, Clone)]
pub(super) enum Access {
    Tailnet(Node),
    /// `--proxy-url`: the person's own proxy or overlay.
    Proxy,
}

/// Where the daemon is published and what that takes.
#[derive(Debug, Clone)]
pub(super) struct Publication {
    pub(super) origin: String,
    /// The loopback address `tailscale serve` forwards to.
    pub(super) target: String,
    pub(super) serve: Option<ServePlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ServePlan {
    pub(super) port: u16,
    /// A `tailscale serve` or `funnel` command has to run.
    pub(super) apply: bool,
    /// The port is already open to the whole internet.
    pub(super) funnel_exposed: bool,
}

pub(super) struct Wizard<'a, R, D, E, U> {
    pub(super) options: &'a Options,
    pub(super) mode: Mode,
    pub(super) proxy_origin: Option<String>,
    pub(super) runner: &'a R,
    pub(super) daemon: &'a mut D,
    pub(super) env: &'a E,
    pub(super) ui: &'a mut U,
    pub(super) platform: Platform,
    pub(super) report: Report,
}

impl<'a, R: CommandRunner, D: Daemon, E: Environment, U: Ui> Wizard<'a, R, D, E, U> {
    pub(super) fn new(
        options: &'a Options,
        mode: Mode,
        proxy_origin: Option<String>,
        runner: &'a R,
        daemon: &'a mut D,
        env: &'a E,
        ui: &'a mut U,
    ) -> Self {
        let platform = env.platform();
        assert_eq!(
            proxy_origin.is_some(),
            options.proxy_url.is_some(),
            "the proxy origin is the validated --proxy-url"
        );
        let report = Report::new(mode, platform.label());
        Self {
            options,
            mode,
            proxy_origin,
            runner,
            daemon,
            env,
            ui,
            platform,
            report,
        }
    }

    pub(super) fn say(&mut self, text: &str) {
        self.ui.say(text);
    }

    pub(super) fn heading(&mut self, text: &str) {
        self.ui.say(&format!("\n== {text}"));
    }

    pub(super) async fn run(mut self) -> anyhow::Result<Report> {
        self.say("Aether remote access: use your agents from your phone.");
        self.steps().await?;
        self.finish();
        Ok(self.report)
    }

    async fn steps(&mut self) -> anyhow::Result<()> {
        let Flow::Go(access) = self.access().await? else {
            return Ok(());
        };
        let Flow::Go(status) = self.daemon_step().await? else {
            return Ok(());
        };
        let Flow::Go(publication) = self.plan(&access, &status).await? else {
            return Ok(());
        };
        self.report.origin = Some(publication.origin.clone());
        let Flow::Go(status) = self.listener_step(&status, &publication).await? else {
            return Ok(());
        };
        let Flow::Go(()) = self.publish_step(&access, &publication).await? else {
            return Ok(());
        };
        let Flow::Go(()) = self.verify_step(&publication.origin).await? else {
            return Ok(());
        };
        self.pairing_step(&access, &publication.origin, &status)
            .await
    }

    async fn access(&mut self) -> anyhow::Result<Flow<Access>> {
        if self.proxy_origin.is_some() {
            let detail =
                "skipped: you gave --proxy-url, so your own proxy or overlay carries the traffic";
            self.report
                .push(Step::new(StepId::Tailscale, StepState::Skipped, detail));
            return Ok(Flow::Go(Access::Proxy));
        }
        Ok(match self.tailscale_step().await? {
            Flow::Go(node) => Flow::Go(Access::Tailnet(node)),
            Flow::Stop => Flow::Stop,
        })
    }

    async fn tailscale_state(&self) -> anyhow::Result<TailscaleState> {
        match Tailscale::locate(self.runner, &self.platform).await? {
            Some(tailscale) => tailscale.state().await,
            None => Ok(TailscaleState::NotInstalled),
        }
    }

    /// Step 1: Tailscale installed, running, signed in, with MagicDNS and
    /// HTTPS certificates; each missing piece explained, then re-checked.
    async fn tailscale_step(&mut self) -> anyhow::Result<Flow<Node>> {
        self.heading("Step 1 of 5: Tailscale, a private network for your devices");
        self.say(guidance::tailscale_intro());
        let (mut rechecks, mut connect_waits) = (0, 0);
        loop {
            let state = self.tailscale_state().await?;
            if let TailscaleState::Ready(node) = state {
                let detail = format!("connected as {}", node.dns_name);
                self.say(&format!("[ok] {detail}"));
                self.report
                    .push(Step::new(StepId::Tailscale, StepState::Done, detail));
                return Ok(Flow::Go(node));
            }
            if state == TailscaleState::Connecting && connect_waits < CONNECT_WAITS {
                connect_waits += 1;
                self.env.sleep(CONNECT_PAUSE).await;
                continue;
            }
            rechecks += 1;
            let guidance = guidance::for_state(&state, &self.platform);
            if rechecks > MAX_RECHECKS {
                self.show(&guidance);
                self.block_on(&guidance);
                return Ok(Flow::Stop);
            }
            if self.stuck_on(&guidance)? {
                return Ok(Flow::Stop);
            }
        }
    }

    fn show(&mut self, guidance: &Guidance) {
        self.say(&format!("[!!] {}", guidance.headline));
        for step in &guidance.steps {
            self.say(&format!("     {step}"));
        }
    }

    /// Records that Tailscale is still not ready and what to do about it.
    fn block_on(&mut self, guidance: &Guidance) {
        let step = Step::new(
            StepId::Tailscale,
            StepState::Blocked,
            guidance.headline.clone(),
        );
        self.report.push(step.fix(guidance.steps.clone()));
    }

    /// Shows what is in the way. True when the run cannot go on: a person is
    /// not here to fix it, or chose to stop.
    fn stuck_on(&mut self, guidance: &Guidance) -> anyhow::Result<bool> {
        self.show(guidance);
        let fix = guidance.steps.clone();
        if self.mode != Mode::Interactive {
            self.block_on(guidance);
            return Ok(true);
        }
        if self.ui.wait("")? == Wait::Quit {
            let step = Step::new(
                StepId::Tailscale,
                StepState::Stopped,
                guidance.headline.clone(),
            );
            self.report.push(step.fix(fix));
            return Ok(true);
        }
        Ok(false)
    }

    /// Step 2a: a daemon is running and its remote settings are read.
    async fn daemon_step(&mut self) -> anyhow::Result<Flow<RemoteStatus>> {
        self.heading("Step 2 of 5: the Aether daemon");
        let mut started = false;
        if !self.daemon.connect().await? {
            self.say(
                "The Aether daemon, the background program that runs your agents, is not running.",
            );
            match self.mode {
                Mode::Check => {
                    let step = Step::new(
                        StepId::Daemon,
                        StepState::Pending,
                        "the daemon is not running",
                    )
                    .command("aether daemon");
                    self.report.push(step);
                    return Ok(Flow::Stop);
                }
                Mode::Interactive if !self.ui.confirm("Start it now?", true)? => {
                    let step = Step::new(
                        StepId::Daemon,
                        StepState::Stopped,
                        "you chose not to start the daemon",
                    )
                    .fix(vec!["aether daemon".to_owned()]);
                    self.report.push(step);
                    return Ok(Flow::Stop);
                }
                Mode::Interactive | Mode::Yes => {}
            }
            self.daemon.start().await.context("starting the daemon")?;
            started = true;
        }
        let status = self.daemon.remote().await?;
        let (state, detail) = if started {
            (StepState::Changed, "started the daemon")
        } else {
            (StepState::Done, "the daemon is running")
        };
        self.say(&format!("[ok] {detail}"));
        self.report.push(Step::new(StepId::Daemon, state, detail));
        Ok(Flow::Go(status))
    }

    /// Decides the public address and, for Tailscale, the port to serve on.
    async fn plan(
        &mut self,
        access: &Access,
        status: &RemoteStatus,
    ) -> anyhow::Result<Flow<Publication>> {
        let node = match access {
            Access::Proxy => return Ok(Flow::Go(self.proxy_publication(status))),
            Access::Tailnet(node) => node,
        };
        let Some(target) = loopback_target(&status.address) else {
            let detail = format!(
                "the listener address {} is not on this computer's loopback, so it cannot be published safely",
                status.address
            );
            let fix = vec![
                "Set `[remote] address` in config.toml to 127.0.0.1:8377 and restart the daemon."
                    .to_owned(),
            ];
            self.say(&format!("[!!] {detail}"));
            self.report
                .push(Step::new(StepId::Listener, StepState::Blocked, detail).fix(fix));
            return Ok(Flow::Stop);
        };
        let Some(tailscale) = Tailscale::locate(self.runner, &self.platform).await? else {
            bail!("Tailscale was ready a moment ago and is not found now");
        };
        let config = tailscale.serve_config().await?;
        let (mut free, mut unreadable) = (None, None);
        for port in SERVE_PORTS {
            match config.port_use(&node.dns_name, port, &target) {
                PortUse::Ours { funnel } => {
                    let plan = ServePlan {
                        port,
                        apply: self.options.funnel && !funnel,
                        funnel_exposed: funnel,
                    };
                    return Ok(Flow::Go(publication(node, target, plan)));
                }
                PortUse::Free if free.is_none() => free = Some(port),
                PortUse::Unknown(what) => unreadable = Some(what),
                PortUse::Free | PortUse::Taken(_) => {}
            }
        }
        let Some(port) = free else {
            return Ok(match unreadable {
                Some(what) => self.serve_unreadable(&what),
                None => self.ports_taken(),
            });
        };
        let plan = ServePlan {
            port,
            apply: true,
            funnel_exposed: false,
        };
        Ok(Flow::Go(publication(node, target, plan)))
    }

    fn proxy_publication(&self, status: &RemoteStatus) -> Publication {
        Publication {
            origin: self.proxy_origin.clone().unwrap_or_default(),
            target: format!("http://{}", status.address),
            serve: None,
        }
    }

    /// No port is known to be free, and `what` of the status could not be read.
    fn serve_unreadable<T>(&mut self, what: &str) -> Flow<T> {
        let detail = format!(
            "could not tell which HTTPS ports Tailscale is already using: `tailscale serve \
             status --json` has parts this version of Aether does not understand ({what})"
        );
        let fix = vec![
            "See what: tailscale serve status".to_owned(),
            "If nothing there needs to stay, clear it: tailscale serve reset".to_owned(),
            "Then run `aether remote setup` again.".to_owned(),
        ];
        self.say(&format!("[!!] {detail}"));
        self.report
            .push(Step::new(StepId::Publish, StepState::Blocked, detail).fix(fix));
        Flow::Stop
    }

    fn ports_taken<T>(&mut self) -> Flow<T> {
        let detail = "Tailscale already publishes something else on every HTTPS port it offers (443, 8443 and 10000)";
        let fix = vec![
            "See what: tailscale serve status".to_owned(),
            "Clear it (this removes everything Tailscale serves from this computer): tailscale serve reset".to_owned(),
            "Then run `aether remote setup` again.".to_owned(),
        ];
        self.say(&format!("[!!] {detail}"));
        self.report
            .push(Step::new(StepId::Publish, StepState::Blocked, detail).fix(fix));
        Flow::Stop
    }
}

fn publication(node: &Node, target: String, plan: ServePlan) -> Publication {
    let origin = if plan.port == 443 {
        format!("https://{}", node.dns_name)
    } else {
        format!("https://{}:{}", node.dns_name, plan.port)
    };
    Publication {
        origin,
        target,
        serve: Some(plan),
    }
}

/// `http://addr` when `addr` is a loopback socket address.
pub(super) fn loopback_target(addr: &str) -> Option<String> {
    let parsed: std::net::SocketAddr = addr.parse().ok()?;
    parsed
        .ip()
        .is_loopback()
        .then(|| format!("http://{parsed}"))
}

/// A stale revision: somebody changed the settings between our read and write.
/// Any other refusal, `invalid_argument` included, is not one.
pub(super) fn is_stale_revision(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<ClientError>(),
        Some(ClientError::Protocol(reply)) if aether_protocol::remote::is_stale_revision(reply)
    )
}

/// The settings that make remote access work, keeping everything else.
/// Pairing is switched on only the first time; a re-run leaves it as it is.
pub(super) fn desired_settings(
    current: &RemoteSettings,
    origin: &str,
    push: bool,
) -> RemoteSettings {
    let mut origins = current.public_origins.clone();
    origins.retain(|existing| existing != origin);
    origins.insert(0, origin.to_owned());
    RemoteSettings {
        enabled: true,
        pairing: current.pairing || !current.enabled,
        public_origins: origins,
        push,
    }
}

/// `aether remote enable` with the same effect as `settings`.
pub(super) fn enable_command(settings: &RemoteSettings) -> String {
    let mut command = String::from("aether remote enable");
    for origin in &settings.public_origins {
        command.push_str(" --origin ");
        command.push_str(origin);
    }
    command.push_str(if settings.push {
        " --push"
    } else {
        " --no-push"
    });
    if !settings.pairing {
        command.push_str(" --no-pairing");
    }
    command
}
