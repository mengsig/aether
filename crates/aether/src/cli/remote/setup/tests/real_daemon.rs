//! The wizard against a real daemon: its saved settings are accepted, and its
//! pairing enrols a phone that claims the code over HTTP.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use aether_client::Client;
use aether_protocol::remote::{DeviceRole, Health, RemoteSettings};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::super::Options;
use super::super::daemon::SocketDaemon;
use super::super::environment::{Environment, ProbeError};
use super::super::platform::{Distro, Platform};
use super::super::report::{Mode, Report, Status, StepId, StepState};
use super::super::ui::Ui;
use super::super::wizard::Wizard;
use super::fakes::{FakeRunner, healthy};
use super::flows::state_of;
use crate::cli::remote::requests;

/// A P-256 public key as a browser exports it, base64url.
const PUBLIC_KEY: &str =
    "BMgwFKrH7TvvMAEg7GPE-KUu5a2OgrV55YaK-5umZW8nLS5GYqYPuBEEPsHRxvA1O8ZG6UR3D9hr4P_hjgxbma4";
const ORIGIN: &str = "https://aether.example.com";
const PHONE: &str = "Wizard Phone";
/// Where the wizard prints the code a person would type.
const CODE_MARKER: &str = "or type this code at that page: ";

struct Host {
    root: tempfile::TempDir,
    shutdown: CancellationToken,
    serving: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Host {
    async fn start() -> Host {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path();
        std::fs::create_dir_all(dir.join("workspace")).expect("workspace");
        std::fs::create_dir_all(dir.join("config")).expect("config dir");
        let remote = "[remote]\naddress = \"127.0.0.1:0\"\n";
        std::fs::write(dir.join("config/config.toml"), remote).expect("config");
        let config = aether_daemon::DaemonConfig {
            socket: dir.join("a.sock"),
            store_path: dir.join("aether.db"),
            config_dir: Some(dir.join("config")),
            workspace_root: Some(dir.join("workspace")),
            max_concurrent_turns: None,
            yolo: false,
            tcp: None,
            allow_remote: false,
        };
        let shutdown = CancellationToken::new();
        let serving = tokio::spawn(aether_daemon::run(config, shutdown.clone()));
        let host = Host {
            root,
            shutdown,
            serving,
        };
        host.client().await;
        host
    }

    fn socket(&self) -> PathBuf {
        self.root.path().join("a.sock")
    }

    /// A connection, waiting for the daemon to answer if it only just started.
    async fn client(&self) -> Client {
        for _ in 0..400 {
            if let Ok(client) = Client::connect(self.socket()).await {
                return client;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("the daemon never answered on {}", self.socket().display());
    }

    async fn stop(self) {
        self.shutdown.cancel();
        let stopped = tokio::time::timeout(Duration::from_secs(10), self.serving).await;
        stopped
            .expect("the daemon stops")
            .expect("joins")
            .expect("cleanly");
    }
}

/// Everything the wizard said, shared with the phone that reads the code.
type Transcript = Rc<RefCell<Vec<String>>>;

/// A terminal that answers from a script and keeps what it was told.
struct Scripted {
    answers: VecDeque<&'static str>,
    said: Transcript,
}

impl Ui for Scripted {
    fn can_ask(&self) -> bool {
        true
    }

    fn say(&mut self, text: &str) {
        self.said.borrow_mut().push(text.to_owned());
    }

    fn ask(&mut self, prompt: &str) -> anyhow::Result<String> {
        let answer = self.answers.pop_front();
        let answer = answer.ok_or_else(|| anyhow::anyhow!("no answer scripted for `{prompt}`"))?;
        Ok(answer.to_owned())
    }
}

/// The world around the wizard, with a phone in it: the first wait after
/// the code is shown is when the phone claims it.
struct WithPhone {
    socket: PathBuf,
    said: Transcript,
    /// What the daemon answered the claim with.
    claimed: RefCell<Option<Value>>,
}

impl WithPhone {
    fn shown_code(&self) -> Option<String> {
        let said = self.said.borrow();
        let line = said.iter().find(|text| text.contains(CODE_MARKER))?;
        let (_, after) = line.split_once(CODE_MARKER)?;
        after.split_whitespace().next().map(str::to_owned)
    }

    async fn claim(&self, code: &str) -> Value {
        let client = Client::connect(&self.socket).await.expect("connect");
        let status = requests::status(&client).await.expect("status");
        let listening = status.listening.expect("the wizard opened the listener");
        let claim = json!({"code": code, "public_key": PUBLIC_KEY, "device_name": PHONE});
        let answer = reqwest::Client::new()
            .post(format!("http://{listening}/api/pair/claim"))
            .header("Origin", format!("http://{listening}"))
            .json(&claim)
            .send()
            .await
            .expect("the listener answers");
        assert!(answer.status().is_success(), "{}", answer.status());
        answer.json().await.expect("a JSON answer")
    }
}

impl Environment for WithPhone {
    fn platform(&self) -> Platform {
        Platform::Linux(Distro::Arch)
    }

    async fn sleep(&self, _duration: Duration) {
        let waiting = self.claimed.borrow().is_none();
        if let (true, Some(code)) = (waiting, self.shown_code()) {
            let claimed = self.claim(&code).await;
            self.claimed.replace(Some(claimed));
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    async fn health(&self, origin: &str) -> Result<Health, ProbeError> {
        assert_eq!(origin, ORIGIN, "the wizard probes the address it set up");
        healthy()
    }

    async fn interrupted(&self) {
        std::future::pending::<()>().await;
    }
}

struct Run {
    report: Report,
    said: Vec<String>,
    claimed: Option<Value>,
}

/// One `aether remote setup --proxy-url` against `host`, answered by `answers`.
async fn set_up(host: &Host, mode: Mode, answers: &[&'static str]) -> Run {
    let options = Options {
        yes: mode == Mode::Yes,
        proxy_url: Some(ORIGIN.to_owned()),
        ..Options::default()
    };
    let said = Transcript::default();
    let env = WithPhone {
        socket: host.socket(),
        said: said.clone(),
        claimed: RefCell::new(None),
    };
    let mut ui = Scripted {
        answers: answers.iter().copied().collect(),
        said: said.clone(),
    };
    let runner = FakeRunner::default();
    let mut daemon = SocketDaemon::new(host.socket());
    let proxy = Some(ORIGIN.to_owned());
    let wizard = Wizard::new(&options, mode, proxy, &runner, &mut daemon, &env, &mut ui);
    let report = wizard.run().await.expect("the wizard runs");
    assert!(ui.answers.is_empty(), "unasked: {:?}", ui.answers);
    let said = said.borrow().clone();
    Run {
        report,
        said,
        claimed: env.claimed.into_inner(),
    }
}

#[tokio::test]
async fn the_wizard_enables_and_pairs_against_a_real_daemon() {
    let host = Host::start().await;
    // Notifications, turning remote access on, and that the words match.
    let first = set_up(&host, Mode::Interactive, &["y", "y", "y"]).await;
    assert_eq!(first.report.status, Status::Complete, "{:?}", first.said);
    assert_eq!(
        state_of(&first.report, StepId::Listener),
        StepState::Changed
    );
    assert_eq!(state_of(&first.report, StepId::Pairing), StepState::Changed);
    let claimed = first.claimed.expect("the phone claimed the code");
    let phrase = claimed["phrase"].as_str().expect("a phrase");
    let said = first.said.join("\n");
    assert!(said.contains(phrase), "the words the phone shows: {said}");
    assert!(said.contains(&format!("{ORIGIN}/pair#code=")), "{said}");

    let client = host.client().await;
    let status = requests::status(&client).await.expect("status");
    let wanted = RemoteSettings {
        enabled: true,
        pairing: true,
        public_origins: vec![ORIGIN.to_owned()],
        push: true,
    };
    assert_eq!(status.settings, wanted);
    assert_eq!((status.revision, status.devices), (1, 1));
    assert!(status.listening.is_some() && status.error.is_none());
    let devices = requests::devices(&client, false).await.expect("devices");
    assert_eq!(devices.len(), 1, "{devices:?}");
    assert_eq!(
        (devices[0].name.as_str(), devices[0].role),
        (PHONE, DeviceRole::Operator)
    );
    let key = host.root.path().join("aether.push-vapid-key");
    assert!(key.is_file(), "turning push on makes the VAPID key");

    // A second run finds everything in place and changes nothing.
    let again = set_up(&host, Mode::Yes, &[]).await;
    assert_eq!(again.report.status, Status::Complete, "{:?}", again.said);
    assert_eq!(state_of(&again.report, StepId::Listener), StepState::Done);
    assert_eq!(state_of(&again.report, StepId::Pairing), StepState::Done);
    assert!(again.claimed.is_none(), "no second pairing was opened");
    let after = requests::status(&client).await.expect("status");
    assert_eq!((after.revision, after.devices), (1, 1));
    drop(client);
    host.stop().await;
}
