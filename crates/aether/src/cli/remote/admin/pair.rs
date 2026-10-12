//! `aether remote pair`: show the code, wait for a device to claim it, and
//! have the operator say whether that device is theirs.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use aether_client::Client;
use aether_protocol::remote::{DeviceRole, Pairing, PairingCandidate, PairingState};
use anyhow::{Context as _, bail};

use super::role_label;
use crate::cli::remote::{IPHONE_FIRST, qr, requests};
use crate::cli::{connect, sane};

/// How often the daemon is asked whether the code was claimed.
const POLL: Duration = Duration::from_millis(500);

/// Hex characters of a fingerprint shown per group, and groups per line.
const FINGERPRINT_GROUP: usize = 4;
const FINGERPRINT_GROUPS_PER_LINE: usize = 8;

pub(in crate::cli::remote) async fn pair(socket: PathBuf, role: DeviceRole) -> anyhow::Result<()> {
    let client = connect(&socket).await?;
    let opened = requests::start_pairing(&client, role).await?;
    print!("{}", invitation(&opened)?);
    let settled = tokio::select! {
        settled = settle(&client, &opened.id) => settled?,
        interrupted = tokio::signal::ctrl_c() => {
            interrupted.context("could not wait for Ctrl+C")?;
            requests::cancel_pairing(&client, &opened.id).await?
        }
    };
    println!("{}", outcome(&settled)?);
    Ok(())
}

/// Wait for the claim, show who claimed, and confirm or reject as answered.
/// Returns the pairing in whatever state ended the wait.
async fn settle(client: &Client, id: &str) -> anyhow::Result<Pairing> {
    let claimed = loop {
        let current = requests::get_pairing(client, id).await?;
        if current.state != PairingState::Open {
            break current;
        }
        tokio::time::sleep(POLL).await;
    };
    if claimed.state != PairingState::Claimed {
        return Ok(claimed);
    }
    let candidate = claimed
        .candidate
        .as_ref()
        .context("the daemon reported a claim without the device that made it")?;
    print!("{}", introduction(candidate, claimed.role));
    let accept = ask("Enrol this device? [y/N] ").await?;
    requests::confirm_pairing(client, id, accept).await
}

/// What the operator is shown to start with: the code, and the URL as a QR.
fn invitation(opened: &Pairing) -> anyhow::Result<String> {
    let code = opened.code.as_deref();
    let code = code.context("the daemon opened a pairing without a code")?;
    let url = opened.url.as_deref();
    let url = url.context("the daemon opened a pairing without a URL")?;
    let mut out = format!("\nPairing code: {}\n\n", sane(code));
    out.push_str(&qr::render(url)?);
    out.push_str(&format!("\nOr open: {}\n", sane(url)));
    if is_loopback_url(url) {
        out.push_str(
            "This address opens on this machine only. To pair a phone, first run\n\
             `aether remote enable --origin https://<an address the phone reaches>`.\n",
        );
    }
    out.push_str(&format!("\n{IPHONE_FIRST}\n"));
    out.push_str("\nWaiting for a device to claim the code. Ctrl+C cancels.\n");
    Ok(out)
}

fn is_loopback_url(url: &str) -> bool {
    let host = url.strip_prefix("http://").unwrap_or_default();
    ["127.0.0.1", "localhost", "[::1]"]
        .iter()
        .any(|loopback| host.starts_with(loopback))
}

/// The claimant as the operator must judge it. The name is the device's own
/// word; the phrase and fingerprint are what tie this claim to that screen.
fn introduction(candidate: &PairingCandidate, role: DeviceRole) -> String {
    let fingerprint = grouped(&sane(&candidate.fingerprint));
    let fingerprint = fingerprint.replace('\n', "\n               ");
    format!(
        "\nA device claimed the code:\n  name         {}\n  role         {}\n  phrase       \
         {}\n  fingerprint  {fingerprint}\nEnrol it only if the device shows the same phrase.\n",
        sane(&candidate.name),
        role_label(role),
        sane(&candidate.phrase),
    )
}

/// A fingerprint in groups of four, eight groups to a line.
fn grouped(fingerprint: &str) -> String {
    let characters: Vec<char> = fingerprint.chars().collect();
    let groups: Vec<String> = characters
        .chunks(FINGERPRINT_GROUP)
        .map(|group| group.iter().collect())
        .collect();
    let lines: Vec<String> = groups
        .chunks(FINGERPRINT_GROUPS_PER_LINE)
        .map(|line| line.join(" "))
        .collect();
    lines.join("\n")
}

/// Put `question` to the operator. Anything but yes, including no answer at
/// all, declines: enrolling is the choice that must be made on purpose.
async fn ask(question: &str) -> anyhow::Result<bool> {
    print!("{question}");
    std::io::stdout()
        .flush()
        .context("could not show the question")?;
    let (answered, answer) = tokio::sync::oneshot::channel();
    // A blocking terminal read cannot be cancelled, so it gets a thread of
    // its own: Ctrl+C then ends the command without waiting for a key.
    std::thread::spawn(move || {
        let mut line = String::new();
        let read = std::io::stdin().read_line(&mut line).map(|_| line);
        // Nobody is listening only once the command was interrupted.
        let _ = answered.send(read);
    });
    let line = answer.await.context("the terminal reader stopped")?;
    Ok(is_yes(&line.context("could not read the answer")?))
}

fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// The line that ends the command, or the error when no decision was made.
fn outcome(settled: &Pairing) -> anyhow::Result<String> {
    let name = settled
        .candidate
        .as_ref()
        .map(|candidate| sane(&candidate.name));
    let name = name.unwrap_or_else(|| "the device".to_owned());
    match settled.state {
        PairingState::Enrolled => {
            let device = settled.device.as_ref();
            let device = device.context("the daemon enrolled a device without naming it")?;
            let role = role_label(device.role);
            Ok(format!(
                "paired {} as {role} ({})",
                sane(&device.name),
                device.id
            ))
        }
        PairingState::Rejected => Ok(format!("rejected {name}; nothing was enrolled")),
        PairingState::Expired => {
            bail!("the pairing expired before it was confirmed; run `aether remote pair` again")
        }
        PairingState::Cancelled => bail!("the pairing was cancelled; nothing was enrolled"),
        PairingState::Open | PairingState::Claimed => {
            bail!("the pairing was left undecided; run `aether remote pair` again")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairing(state: &str, extra: serde_json::Value) -> Pairing {
        let mut pairing = serde_json::json!({
            "id": "pairing-1", "state": state, "role": "observer",
            "expires_at": "2026-10-12T00:02:00Z",
        });
        for (key, value) in extra.as_object().expect("an object") {
            pairing[key] = value.clone();
        }
        serde_json::from_value(pairing).expect("a pairing")
    }

    fn candidate() -> serde_json::Value {
        serde_json::json!({
            "name": "Kim's \u{1b}[2Jphone",
            "fingerprint": "cbac3e5256fa5af7dd44fefe167382334a0bdedfcdf5ed1cd87077beead38bc8",
            "phrase": "amber-otter-violin",
            "claimed_at": "2026-10-12T00:00:30Z",
        })
    }

    #[test]
    fn the_invitation_shows_the_code_the_url_and_a_qr() {
        let url = "https://box.tail1234.ts.net/pair#code=7K2M-9QXD";
        let opened = pairing("open", serde_json::json!({"code": "7K2M-9QXD", "url": url}));
        let shown = invitation(&opened).expect("an invitation");
        assert!(shown.contains("Pairing code: 7K2M-9QXD"), "{shown}");
        assert!(shown.contains(&format!("Or open: {url}")), "{shown}");
        assert!(shown.contains(&qr::render(url).expect("a QR")), "{shown}");
        assert!(!shown.contains("this machine only"), "{shown}");

        let local = "http://127.0.0.1:8377/pair#code=7K2M-9QXD";
        let opened = pairing(
            "open",
            serde_json::json!({"code": "7K2M-9QXD", "url": local}),
        );
        let shown = invitation(&opened).expect("an invitation");
        assert!(shown.contains("this machine only"), "{shown}");
        assert!(invitation(&pairing("open", serde_json::json!({}))).is_err());
    }

    #[test]
    fn the_claimant_is_shown_with_its_phrase_and_whole_fingerprint() {
        let claimed = pairing("claimed", serde_json::json!({"candidate": candidate()}));
        let candidate = claimed.candidate.as_ref().expect("a candidate");
        let shown = introduction(candidate, claimed.role);
        assert!(
            shown.contains("name         Kim's [2Jphone\n"),
            "no escape survives: {shown:?}"
        );
        assert!(shown.contains("role         observer\n"), "{shown}");
        assert!(
            shown.contains("phrase       amber-otter-violin\n"),
            "{shown}"
        );
        assert!(
            shown.contains(
                "  fingerprint  cbac 3e52 56fa 5af7 dd44 fefe 1673 8233\n               \
                 4a0b dedf cdf5 ed1c d870 77be ead3 8bc8\n"
            ),
            "{shown}"
        );
        assert_eq!(grouped("abcdef"), "abcd ef");
        assert_eq!(grouped(""), "");
    }

    #[test]
    fn only_a_plain_yes_enrols() {
        for yes in ["y\n", "Y\n", " yes \r\n", "YES"] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "\n", "n\n", "no\n", "yep\n", "y y\n"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }

    #[test]
    fn the_last_line_says_what_happened_and_an_undecided_pairing_fails() {
        let device = serde_json::json!({
            "id": "dev_01M4PH1G605RXFBHJAHG90SGEV", "name": "phone", "role": "observer",
        });
        let extra = serde_json::json!({"candidate": candidate(), "device": device});
        let enrolled = outcome(&pairing("enrolled", extra)).expect("enrolled");
        assert_eq!(
            enrolled,
            "paired phone as observer (dev_01M4PH1G605RXFBHJAHG90SGEV)"
        );
        let extra = serde_json::json!({"candidate": candidate()});
        let rejected = outcome(&pairing("rejected", extra)).expect("a decision");
        assert_eq!(rejected, "rejected Kim's [2Jphone; nothing was enrolled");
        for undecided in ["expired", "cancelled", "open", "claimed"] {
            let failed = outcome(&pairing(undecided, serde_json::json!({})));
            assert!(failed.is_err(), "{undecided}");
        }
        assert!(outcome(&pairing("enrolled", serde_json::json!({}))).is_err());
    }
}
