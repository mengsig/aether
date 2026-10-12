//! Wizard steps 2 and 3: switching the listener on, publishing it, and
//! checking that the published address answers.

use std::time::Duration;

use aether_protocol::remote::{RemoteSettings, RemoteStatus};
use anyhow::{Context as _, bail, ensure};

use super::daemon::Daemon;
use super::environment::{Environment, ProbeError};
use super::report::{Mode, Status, Step, StepId, StepState};
use super::runner::CommandRunner;
use super::tailscale::{Node, PortUse, PublishOutcome, Tailscale};
use super::ui::{Ui, Wait};
use super::wizard::{
    Access, Flow, Publication, ServePlan, Wizard, desired_settings, enable_command,
    is_stale_revision,
};

/// Attempts at publishing before the run gives up on a person fixing it.
const PUBLISH_ATTEMPTS: u32 = 3;
/// A new certificate can take half a minute to appear.
const PROBE_ATTEMPTS: u32 = 8;
const PROBE_PAUSE: Duration = Duration::from_secs(4);

impl<R: CommandRunner, D: Daemon, E: Environment, U: Ui> Wizard<'_, R, D, E, U> {
    /// Whether to do something this run offers: a person is asked, `--yes`
    /// says yes, and a check-only run never gets here.
    fn approve(&mut self, question: &str, default: bool) -> anyhow::Result<bool> {
        match self.mode {
            Mode::Interactive => self.ui.confirm(question, default),
            Mode::Yes => Ok(true),
            Mode::Check => Ok(false),
        }
    }

    /// Step 2b: the listener is on, allowlists this address, and is open.
    pub(super) async fn listener_step(
        &mut self,
        current: &RemoteStatus,
        publication: &Publication,
    ) -> anyhow::Result<Flow<RemoteStatus>> {
        let first_time = !current.settings.enabled;
        let push = current.settings.push
            || (first_time
                && self.approve(
                    "Notify your phone when an agent needs you? (alerts carry no agent text)",
                    true,
                )?);
        let wanted = desired_settings(&current.settings, &publication.origin, push);
        if wanted == current.settings {
            return Ok(self.listener_ready(current.clone(), StepState::Done));
        }
        let command = enable_command(&wanted);
        if self.mode == Mode::Check {
            let detail = format!(
                "remote access would be turned on for {}",
                publication.origin
            );
            self.report
                .push(Step::new(StepId::Listener, StepState::Pending, detail).command(command));
            return Ok(Flow::Go(current.clone()));
        }
        let question = format!("Turn on remote access for {}?", publication.origin);
        if self.mode == Mode::Interactive && !self.ui.confirm(&question, true)? {
            let step = Step::new(
                StepId::Listener,
                StepState::Stopped,
                "you chose not to turn on remote access",
            );
            self.report.push(step.fix(vec![command]));
            return Ok(Flow::Stop);
        }
        let saved = self
            .save_settings(current.revision, wanted, &publication.origin)
            .await?;
        Ok(self.listener_ready(saved, StepState::Changed))
    }

    /// Saves `wanted`, once more against the newest settings if somebody
    /// changed them since `revision`.
    async fn save_settings(
        &mut self,
        revision: u64,
        wanted: RemoteSettings,
        origin: &str,
    ) -> anyhow::Result<RemoteStatus> {
        assert!(
            wanted.enabled && wanted.public_origins.iter().any(|o| o == origin),
            "the settings to save must enable the listener for {origin}"
        );
        let push = wanted.push;
        let saved = match self.daemon.configure(wanted, revision).await {
            Err(error) if is_stale_revision(&error) => {
                let latest = self.daemon.remote().await?;
                let wanted =
                    desired_settings(&latest.settings, origin, push || latest.settings.push);
                self.daemon
                    .configure(wanted, latest.revision)
                    .await
                    .context("saving the remote access settings after they changed underneath")?
            }
            other => other?,
        };
        ensure!(
            saved.settings.enabled && saved.settings.public_origins.iter().any(|o| o == origin),
            "the daemon accepted the settings but does not report {origin} as enabled"
        );
        Ok(saved)
    }

    fn listener_ready(&mut self, status: RemoteStatus, state: StepState) -> Flow<RemoteStatus> {
        if let Some(error) = &status.error {
            let detail = format!("the listener could not open: {error}");
            let fix = vec![format!(
                "Something else may be using {}. Set `[remote] address` in config.toml to a free \
                 loopback port, restart the daemon, then run `aether remote setup` again.",
                status.address
            )];
            self.say(&format!("[!!] {detail}"));
            self.report
                .push(Step::new(StepId::Listener, StepState::Blocked, detail).fix(fix));
            return Flow::Stop;
        }
        let Some(listening) = status.listening.clone() else {
            let detail = "remote access is on but the daemon is not listening".to_owned();
            self.say(&format!("[!!] {detail}"));
            self.report
                .push(Step::new(StepId::Listener, StepState::Blocked, detail));
            return Flow::Stop;
        };
        let detail = format!(
            "listening on {listening}, for {}",
            status.settings.public_origins.join(", ")
        );
        self.say(&format!("[ok] {detail}"));
        self.report.push(Step::new(StepId::Listener, state, detail));
        Flow::Go(status)
    }

    /// Step 3: Tailscale publishes the listener on this machine's name.
    pub(super) async fn publish_step(
        &mut self,
        access: &Access,
        publication: &Publication,
    ) -> anyhow::Result<Flow<()>> {
        self.heading("Step 3 of 5: publish it on your tailnet");
        let (Access::Tailnet(node), Some(plan)) = (access, publication.serve) else {
            return Ok(self.proxy_note(publication));
        };
        if !plan.apply {
            return Ok(self.already_published(publication, plan));
        }
        let funnel = self.options.funnel;
        let command = serve_command(plan.port, &publication.target, funnel);
        if self.mode == Mode::Check {
            let detail = format!(
                "Tailscale would publish {} at {}",
                publication.target, publication.origin
            );
            self.report
                .push(Step::new(StepId::Publish, StepState::Pending, detail).command(command));
            return Ok(Flow::Go(()));
        }
        self.explain_publication(publication, funnel);
        if self.mode == Mode::Interactive && !self.consent_to_publish(&command, funnel)? {
            let step = Step::new(
                StepId::Publish,
                StepState::Stopped,
                "you chose not to publish it",
            );
            self.report.push(step.fix(vec![command]));
            return Ok(Flow::Stop);
        }
        self.apply_publication(node, plan, &publication.target)
            .await
    }

    fn proxy_note(&mut self, publication: &Publication) -> Flow<()> {
        let detail = format!(
            "you publish it: point your proxy at {} for {}",
            publication.target, publication.origin
        );
        let fix = vec![
            "The proxy must serve HTTPS, keep the Host header, and pass WebSocket upgrades on /ws."
                .to_owned(),
        ];
        self.say(&format!("[ok] {detail}"));
        for line in &fix {
            self.say(&format!("     {line}"));
        }
        self.report
            .push(Step::new(StepId::Publish, StepState::Skipped, detail).fix(fix));
        Flow::Go(())
    }

    fn already_published(&mut self, publication: &Publication, plan: ServePlan) -> Flow<()> {
        let detail = format!("already published at {}", publication.origin);
        self.say(&format!("[ok] {detail}"));
        if !plan.funnel_exposed {
            self.report
                .push(Step::new(StepId::Publish, StepState::Done, detail));
            return Flow::Go(());
        }
        let warning = format!("{detail}, and OPEN TO THE WHOLE INTERNET by Tailscale Funnel");
        let fix = vec![
            "If you did not mean that: tailscale serve reset, then run `aether remote setup` again.".to_owned(),
        ];
        self.say(&format!("[!!] {warning}"));
        self.say(&format!("     {}", fix[0]));
        self.report
            .push(Step::new(StepId::Publish, StepState::Warning, warning).fix(fix));
        Flow::Go(())
    }

    fn explain_publication(&mut self, publication: &Publication, funnel: bool) {
        self.say(&format!(
            "Aether listens only on this computer ({}). Tailscale will publish it at\n  {}",
            publication.target, publication.origin
        ));
        if funnel {
            self.say(
                "WARNING: you asked for Tailscale Funnel. That puts Aether on the PUBLIC INTERNET: \
                 anyone can reach\nthe sign-in page, not only your devices. Pairing and the device \
                 keys still protect it, but\nit is no longer private. Most people should not do this.",
            );
        } else {
            self.say(
                "That address works only for devices signed in to your tailnet. It is private to you;\n\
                 it is not on the public internet.",
            );
        }
    }

    fn consent_to_publish(&mut self, command: &str, funnel: bool) -> anyhow::Result<bool> {
        if funnel
            && self
                .ui
                .ask("Type 'public' to put Aether on the internet, anything else to stop: ")?
                != "public"
        {
            return Ok(false);
        }
        self.ui.confirm(&format!("Run `{command}` now?"), true)
    }

    async fn apply_publication(
        &mut self,
        node: &Node,
        plan: ServePlan,
        target: &str,
    ) -> anyhow::Result<Flow<()>> {
        let Some(tailscale) = Tailscale::locate(self.runner, &self.platform).await? else {
            bail!("Tailscale was ready a moment ago and is not found now");
        };
        let funnel = self.options.funnel;
        let command = serve_command(plan.port, target, funnel);
        for _ in 0..PUBLISH_ATTEMPTS {
            let (headline, fix) = match tailscale.publish(plan.port, target, funnel).await? {
                PublishOutcome::Published => {
                    return self.confirm_published(&tailscale, node, plan, target).await;
                }
                PublishOutcome::NotPermitted { command: ran } => (
                    format!(
                        "Tailscale will not let this user change what it publishes (ran `{ran}`)"
                    ),
                    vec![
                        "Allow it once, then try again:  sudo tailscale set --operator=$USER"
                            .to_owned(),
                    ],
                ),
                PublishOutcome::NotEnabled { url, command: ran } => (
                    format!(
                        "Tailscale needs a feature turned on for your tailnet first (ran `{ran}`)"
                    ),
                    vec![format!(
                        "Open {} and enable it.",
                        url.unwrap_or_else(|| super::guidance::ADMIN_DNS.to_owned())
                    )],
                ),
                PublishOutcome::Failed {
                    command: ran,
                    complaint,
                } => {
                    let fix = vec![format!(
                        "Run it yourself to see Tailscale's full message: {ran}"
                    )];
                    return Ok(self.publish_blocked(format!("`{ran}` failed: {complaint}"), fix));
                }
            };
            if let Some(flow) = self.after_refusal(headline, fix, &command)? {
                return Ok(flow);
            }
        }
        let fix = vec![format!("Run it yourself to see what is wrong: {command}")];
        Ok(self.publish_blocked(
            "Tailscale still refused after several tries".to_owned(),
            fix,
        ))
    }

    /// Shows why Tailscale refused. `None` means the person fixed it and the
    /// command should be tried again; otherwise the step is over.
    fn after_refusal(
        &mut self,
        headline: String,
        fix: Vec<String>,
        command: &str,
    ) -> anyhow::Result<Option<Flow<()>>> {
        self.say(&format!("[!!] {headline}"));
        for line in &fix {
            self.say(&format!("     {line}"));
        }
        if self.mode != Mode::Interactive {
            return Ok(Some(self.publish_blocked(headline, fix)));
        }
        if self.ui.wait("")? == Wait::Quit {
            let step = Step::new(StepId::Publish, StepState::Stopped, headline).fix(fix);
            self.report.push(step.command(command));
            return Ok(Some(Flow::Stop));
        }
        Ok(None)
    }

    fn publish_blocked(&mut self, detail: String, fix: Vec<String>) -> Flow<()> {
        self.say(&format!("[!!] {detail}"));
        self.report
            .push(Step::new(StepId::Publish, StepState::Blocked, detail).fix(fix));
        Flow::Stop
    }

    /// The command succeeded; read the configuration back to be sure.
    async fn confirm_published(
        &mut self,
        tailscale: &Tailscale<'_, R>,
        node: &Node,
        plan: ServePlan,
        target: &str,
    ) -> anyhow::Result<Flow<()>> {
        let config = tailscale.serve_config().await?;
        let expected = PortUse::Ours {
            funnel: self.options.funnel,
        };
        let actual = config.port_use(&node.dns_name, plan.port, target);
        ensure!(
            actual == expected,
            "Tailscale accepted the command but its configuration now says {actual:?}, not {expected:?}"
        );
        let detail = format!("Tailscale now publishes {target} at port {}", plan.port);
        self.say(&format!("[ok] {detail}"));
        let command = serve_command(plan.port, target, self.options.funnel);
        self.report
            .push(Step::new(StepId::Publish, StepState::Changed, detail).command(command));
        Ok(Flow::Go(()))
    }

    /// Step 3b: the published address answers, so the allowlist, the
    /// listener and the proxy agree.
    pub(super) async fn verify_step(&mut self, origin: &str) -> anyhow::Result<Flow<()>> {
        if self.report.status != Status::Complete {
            let detail = "not checked: earlier steps still have changes to make";
            self.report
                .push(Step::new(StepId::Verify, StepState::Skipped, detail));
            return Ok(Flow::Go(()));
        }
        let mut last = ProbeError::Unreachable("no attempt was made".to_owned());
        for attempt in 0..PROBE_ATTEMPTS {
            match self.env.health(origin).await {
                Ok(health) if health.status == "ok" => return Ok(self.verified(origin)),
                Ok(health) => last = ProbeError::Malformed(format!("status `{}`", health.status)),
                Err(error) => last = error,
            }
            if !matches!(last, ProbeError::Unreachable(_)) || attempt + 1 == PROBE_ATTEMPTS {
                break;
            }
            self.say("     waiting for the address to come up (Tailscale may still be getting its certificate)...");
            self.env.sleep(PROBE_PAUSE).await;
        }
        self.unverified(origin, &last)
    }

    fn verified(&mut self, origin: &str) -> Flow<()> {
        let detail = format!("{origin} answers, so Aether accepts this address");
        self.say(&format!("[ok] {detail}"));
        self.report
            .push(Step::new(StepId::Verify, StepState::Done, detail));
        Flow::Go(())
    }

    fn unverified(&mut self, origin: &str, error: &ProbeError) -> anyhow::Result<Flow<()>> {
        let detail = format!("{origin} did not answer: {error}");
        let fix = probe_advice(origin, error);
        self.say(&format!("[!!] {detail}"));
        for line in &fix {
            self.say(&format!("     {line}"));
        }
        let go_on = self.mode == Mode::Interactive
            && self.ui.confirm(
                "Carry on to the phone anyway? (the check may only fail on this computer)",
                false,
            )?;
        let state = if go_on {
            StepState::Warning
        } else {
            StepState::Blocked
        };
        self.report
            .push(Step::new(StepId::Verify, state, detail).fix(fix));
        Ok(if go_on { Flow::Go(()) } else { Flow::Stop })
    }
}

/// What to try for a published address that did not answer.
fn probe_advice(origin: &str, error: &ProbeError) -> Vec<String> {
    match error {
        ProbeError::Unreachable(reason) if reason.contains("certificate") => vec![
            "Something on this computer answered that name with a certificate that is not Tailscale's.".to_owned(),
            "Another program listening on the same port here does that. It affects this computer only, not your phone.".to_owned(),
            format!("From your phone, once Tailscale is on, open {origin}/api/health. It should say \"status\":\"ok\"."),
        ],
        ProbeError::Unreachable(_) => vec![
            "Wait a minute and run `aether remote setup` again: the first certificate can be slow.".to_owned(),
            "Check that this computer can look up tailnet names (Tailscale's DNS may be switched off here).".to_owned(),
            format!("From your phone, once Tailscale is on, open {origin}/api/health. It should say \"status\":\"ok\"."),
        ],
        ProbeError::Status(403) => vec![
            "Aether refused the name. Run `aether remote status` and check that the address is listed.".to_owned(),
        ],
        ProbeError::Status(_) | ProbeError::Malformed(_) => vec![
            "Something answered, but not Aether. Is another program on the same port, or is the proxy pointed elsewhere?".to_owned(),
            "See what is published: tailscale serve status".to_owned(),
        ],
    }
}

/// The command that publishes `target` on HTTPS `port`.
pub(super) fn serve_command(port: u16, target: &str, funnel: bool) -> String {
    let verb = if funnel { "funnel" } else { "serve" };
    format!("tailscale {verb} --bg --https={port} {target}")
}
