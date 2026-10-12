//! Wizard step 4: put Aether on the phone. Install Tailscale there, show the
//! pairing QR, wait for the phone, compare the phrase, confirm.

use std::time::Duration;

use aether_client::ClientError;
use aether_protocol::ErrorCode;
use aether_protocol::remote::{
    DeviceIdentity, DeviceRole, Pairing, PairingState, RemoteSettings, RemoteStatus,
};
use anyhow::{Context as _, bail, ensure};

use super::daemon::Daemon;
use super::environment::Environment;
use super::guidance;
use super::report::{Mode, Step, StepId, StepState};
use super::runner::CommandRunner;
use super::ui::{Ui, Wait};
use super::wizard::{Access, Wizard};
use crate::cli::remote::{IPHONE_FIRST, qr};

const POLL: Duration = Duration::from_millis(500);
/// A pairing lives two minutes before and two after a claim; this is a
/// bound on waiting, not the daemon's clock.
const MAX_POLLS: u32 = 600;
const PAIRING_ATTEMPTS: u32 = 3;

#[derive(Debug)]
enum Outcome {
    Enrolled(DeviceIdentity),
    Rejected,
    Expired,
    Interrupted,
}

impl<R: CommandRunner, D: Daemon, E: Environment, U: Ui> Wizard<'_, R, D, E, U> {
    pub(super) async fn pairing_step(
        &mut self,
        access: &Access,
        origin: &str,
        status: &RemoteStatus,
    ) -> anyhow::Result<()> {
        self.heading("Step 4 of 5: your phone");
        if status.devices > 0 && !self.pair_another().await? {
            let detail = format!("{} device(s) already paired", status.devices);
            self.report
                .push(Step::new(StepId::Pairing, StepState::Done, detail));
            return Ok(());
        }
        if status.settings.enabled && !status.settings.pairing {
            let on = RemoteSettings {
                pairing: true,
                ..status.settings.clone()
            };
            let detail = "pairing is switched off on this host";
            let step = Step::new(StepId::Pairing, StepState::Skipped, detail)
                .command(super::wizard::enable_command(&on));
            self.report.push(step);
            return Ok(());
        }
        if self.mode != Mode::Interactive || !self.ui.can_ask() {
            let detail = "pairing needs a person at the terminal";
            let step = Step::new(StepId::Pairing, StepState::Skipped, detail)
                .command("aether remote pair");
            self.report.push(step);
            return Ok(());
        }
        if self.mode == Mode::Interactive && !self.prepare_phone(access)? {
            let step = Step::new(
                StepId::Pairing,
                StepState::Stopped,
                "you stopped before pairing",
            )
            .command("aether remote pair");
            self.report.push(step);
            return Ok(());
        }
        self.pair_with_retries(origin).await
    }

    /// Whether to pair one more device when some are paired already.
    async fn pair_another(&mut self) -> anyhow::Result<bool> {
        let names: Vec<String> = self
            .daemon
            .devices()
            .await?
            .into_iter()
            .map(|d| d.name)
            .collect();
        self.say(&format!("Already paired: {}.", names.join(", ")));
        if self.mode != Mode::Interactive {
            return Ok(false);
        }
        self.ui.confirm("Pair another phone?", false)
    }

    /// Install and sign in; true when the person is ready to scan.
    fn prepare_phone(&mut self, access: &Access) -> anyhow::Result<bool> {
        if matches!(access, Access::Proxy) {
            self.say("On your phone, open a browser; you will scan a code in a moment.");
            return Ok(true);
        }
        self.say(
            "Your phone needs the Tailscale app, signed in to the SAME account as this computer.",
        );
        for (name, url) in [
            ("iPhone / iPad", guidance::IOS_STORE),
            ("Android", guidance::ANDROID_STORE),
        ] {
            self.say(&format!("\n{name}: {url}"));
            self.say(&qr::render(url)?);
        }
        let ready = "Install it, sign in, and switch Tailscale on. ";
        Ok(self.ui.wait(ready)? == Wait::Done)
    }

    async fn pair_with_retries(&mut self, origin: &str) -> anyhow::Result<()> {
        for _ in 0..PAIRING_ATTEMPTS {
            let (state, detail) = match self.pair_once(origin).await? {
                Outcome::Enrolled(device) => {
                    self.say(&format!("[ok] {} is paired.", device.name));
                    self.say("\nLast step, on the phone:");
                    for line in guidance::home_screen_steps() {
                        self.say(&format!("  {line}"));
                    }
                    let detail = format!("paired {} as {:?}", device.name, device.role);
                    self.report
                        .push(Step::new(StepId::Pairing, StepState::Changed, detail));
                    return Ok(());
                }
                Outcome::Rejected => (StepState::Stopped, "you rejected the device".to_owned()),
                Outcome::Interrupted => (StepState::Stopped, "pairing was cancelled".to_owned()),
                Outcome::Expired => {
                    self.say("[!!] The code ran out before the phone finished.");
                    if self.mode == Mode::Interactive
                        && self.ui.confirm("Make a new code?", true)?
                    {
                        continue;
                    }
                    (StepState::Blocked, "the pairing code expired".to_owned())
                }
            };
            let step = Step::new(StepId::Pairing, state, detail).command("aether remote pair");
            self.report.push(step);
            return Ok(());
        }
        let detail = "the phone did not pair after several codes";
        self.report.push(
            Step::new(StepId::Pairing, StepState::Blocked, detail).command("aether remote pair"),
        );
        Ok(())
    }

    /// One pairing: open it, show it, wait for the claim, compare, answer.
    async fn pair_once(&mut self, origin: &str) -> anyhow::Result<Outcome> {
        let pairing = self.daemon.start_pairing(DeviceRole::Operator).await?;
        let (Some(code), Some(url)) = (pairing.code.as_deref(), pairing.url.as_deref()) else {
            bail!("the daemon opened a pairing without a code or address");
        };
        if !url.starts_with(origin) {
            bail!(
                "the pairing address {url} is not under {origin}; the first public origin is not the one just set up"
            );
        }
        self.say(&format!(
            "\nOn your phone, scan this with the camera and open the link. It works once and runs out in two minutes.\n\n{}\n  {url}\n  or type this code at that page: {code}\n\n{IPHONE_FIRST}\n\nWaiting for your phone...",
            qr::render(url)?
        ));
        let claimed = tokio::select! {
            biased;
            claimed = await_claim(&mut *self.daemon, self.env, &pairing.id) => claimed?,
            () = self.env.interrupted() => return self.interrupted(&pairing.id).await,
        };
        self.decide(claimed).await
    }

    async fn interrupted(&mut self, id: &str) -> anyhow::Result<Outcome> {
        self.daemon
            .cancel_pairing(id)
            .await
            .context("cancelling the pairing after Ctrl+C")?;
        self.say("Pairing cancelled.");
        Ok(Outcome::Interrupted)
    }

    async fn decide(&mut self, pairing: Pairing) -> anyhow::Result<Outcome> {
        let Some(candidate) = pairing
            .candidate
            .as_ref()
            .filter(|_| pairing.state == PairingState::Claimed)
        else {
            if pairing.state == PairingState::Open {
                self.daemon
                    .cancel_pairing(&pairing.id)
                    .await
                    .context("closing the pairing that nobody claimed")?;
            }
            return Ok(Outcome::Expired);
        };
        self.say(&format!(
            "\nA device is asking to pair:\n  name:         {}\n  fingerprint:  {}\n  words:        {}\n\nYour phone shows the same three words. If it does not, say no: someone else may have the code.",
            candidate.name,
            short_fingerprint(&candidate.fingerprint),
            candidate.phrase,
        ));
        let accept = self
            .ui
            .confirm("Do the words match, and is that your phone?", false)?;
        let answered = match self.daemon.confirm_pairing(&pairing.id, accept).await {
            Err(error) if is_invalid_transition(&error) => return Ok(Outcome::Expired),
            other => other?,
        };
        match (answered.state, answered.device) {
            (PairingState::Enrolled, Some(device)) => Ok(Outcome::Enrolled(device)),
            (PairingState::Rejected, _) => Ok(Outcome::Rejected),
            (state, _) => bail!("the pairing ended as {state:?} after it was answered"),
        }
    }
}

/// Polls until the pairing is no longer waiting for a claim, or the bound
/// on waiting passes; the last state read is returned either way.
async fn await_claim<D: Daemon, E: Environment>(
    daemon: &mut D,
    env: &E,
    id: &str,
) -> anyhow::Result<Pairing> {
    assert!(!id.is_empty(), "a pairing id is required");
    let mut pairing = daemon.pairing(id).await?;
    for _ in 0..MAX_POLLS {
        if pairing.state != PairingState::Open {
            break;
        }
        env.sleep(POLL).await;
        pairing = daemon.pairing(id).await?;
    }
    ensure!(
        pairing.id == id,
        "the daemon answered for pairing {}, not {id}",
        pairing.id
    );
    Ok(pairing)
}

fn is_invalid_transition(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<ClientError>(),
        Some(ClientError::Protocol(reply)) if reply.code == ErrorCode::InvalidTransition
    )
}

/// The first 16 characters in groups of four, as the web app shows it.
fn short_fingerprint(fingerprint: &str) -> String {
    fingerprint
        .chars()
        .take(16)
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|group| group.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}
