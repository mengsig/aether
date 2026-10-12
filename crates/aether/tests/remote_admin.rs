#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! `aether remote` against a real daemon: the binary enables the listener,
//! pairs a device that claims over HTTP, and administers it.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::TcpStream;
use tokio::process::{Child, ChildStdout, Command};
use tokio_util::sync::CancellationToken;

/// A P-256 public key as a browser exports it, base64url. Nothing here
/// signs with it: enrolling is all the command line has to do.
const PUBLIC_KEY: &str =
    "BMgwFKrH7TvvMAEg7GPE-KUu5a2OgrV55YaK-5umZW8nLS5GYqYPuBEEPsHRxvA1O8ZG6UR3D9hr4P_hjgxbma4";

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
        host.await_answering().await;
        host
    }

    fn socket(&self) -> PathBuf {
        self.root.path().join("a.sock")
    }

    async fn await_answering(&self) {
        for _ in 0..400 {
            if aether_client::Client::connect(self.socket()).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("the daemon never answered on {}", self.socket().display());
    }

    fn command(&self, args: &[&str]) -> Command {
        let root: &Path = self.root.path();
        let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
        command
            .args(["remote"])
            .args(args)
            .arg("--socket")
            .arg(self.socket())
            .env("XDG_RUNTIME_DIR", root.join("runtime"))
            .env("AETHER_CONFIG_DIR", root.join("config"))
            .env("AETHER_DATA_DIR", root.join("data"))
            .env_remove("AETHER_SOCKET")
            .env_remove("AETHER_REMOTE")
            .env_remove("AETHER_TCP_ADDR")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        command
    }

    /// Run to the end; standard output if it succeeded, standard error if not.
    async fn run(&self, args: &[&str]) -> Result<String, String> {
        let running = self.command(args).output();
        let output = tokio::time::timeout(Duration::from_secs(60), running).await;
        let output = output.expect("the command ends").expect("the command runs");
        if output.status.success() {
            return Ok(String::from_utf8(output.stdout).expect("UTF-8"));
        }
        Err(String::from_utf8(output.stderr).expect("UTF-8"))
    }

    async fn json(&self, args: &[&str]) -> Value {
        let printed = self
            .run(args)
            .await
            .unwrap_or_else(|error| panic!("{args:?}: {error}"));
        serde_json::from_str(&printed).unwrap_or_else(|_| panic!("{args:?} printed {printed}"))
    }

    /// Start `aether remote pair`, and claim the code it prints as `name`.
    async fn pair_claimed_by(&self, name: &str, listening: &str) -> Pairing {
        let mut command = self.command(&["pair", "--role", "observer"]);
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut child = command.spawn().expect("spawn");
        let stdout = child.stdout.take().expect("piped");
        let mut pairing = Pairing {
            child,
            lines: BufReader::new(stdout).lines(),
            seen: String::new(),
        };
        let invited = pairing.read_until("Pairing code: ").await;
        let code = invited
            .rsplit("Pairing code: ")
            .next()
            .expect("a code")
            .trim();
        let claim = json!({"code": code, "public_key": PUBLIC_KEY, "device_name": name});
        let claimed = post(listening, "/api/pair/claim", &claim).await;
        assert!(claimed["phrase"].is_string(), "{claimed}");
        let shown = pairing.read_until("Enrol it only if").await;
        let phrase = claimed["phrase"].as_str().expect("a phrase");
        assert!(shown.contains(&format!("phrase       {phrase}")), "{shown}");
        assert!(shown.contains(&format!("name         {name}")), "{shown}");
        pairing
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

/// A running `aether remote pair`.
struct Pairing {
    child: Child,
    lines: Lines<BufReader<ChildStdout>>,
    seen: String,
}

impl Pairing {
    /// Everything printed so far, once a line containing `marker` arrives.
    async fn read_until(&mut self, marker: &str) -> String {
        loop {
            let next = tokio::time::timeout(Duration::from_secs(30), self.lines.next_line()).await;
            let line = next.expect("output in time").expect("readable");
            let line = line.unwrap_or_else(|| panic!("ended before {marker:?}: {}", self.seen));
            self.seen.push_str(&line);
            self.seen.push('\n');
            if line.contains(marker) {
                return self.seen.clone();
            }
        }
    }

    /// Answer the question, or close the terminal with `None`; the rest of
    /// the output and whether the command succeeded.
    async fn answer(mut self, answer: Option<&str>) -> (String, bool) {
        let mut stdin = self.child.stdin.take().expect("piped");
        if let Some(answer) = answer {
            stdin.write_all(answer.as_bytes()).await.expect("answer");
        }
        drop(stdin);
        let mut rest = String::new();
        while let Some(line) = self.lines.next_line().await.expect("readable") {
            rest.push_str(&line);
            rest.push('\n');
        }
        let ended = tokio::time::timeout(Duration::from_secs(30), self.child.wait()).await;
        (rest, ended.expect("ends").expect("waits").success())
    }
}

/// `POST` JSON as the page the listener serves would; the JSON answered.
async fn post(listening: &str, path: &str, body: &Value) -> Value {
    let body = body.to_string();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {listening}\r\nOrigin: http://{listening}\r\nContent-Type: \
         application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(listening).await.expect("connect");
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.expect("read");
    let (_, body) = answer.split_once("\r\n\r\n").expect("a body");
    serde_json::from_str(body).unwrap_or_else(|_| panic!("not JSON: {answer}"))
}

/// Only a yes enrols: a no and a closed terminal both reject. Returns the
/// id of the one device that was accepted.
async fn pair_three_and_accept_one(host: &Host, listening: &str) -> String {
    let declined = host.pair_claimed_by("Stranger", listening).await;
    let (said, succeeded) = declined.answer(Some("n\n")).await;
    assert!(
        succeeded && said.contains("rejected Stranger; nothing was enrolled"),
        "{said}"
    );
    let unanswered = host.pair_claimed_by("Nobody home", listening).await;
    let (said, _) = unanswered.answer(None).await;
    assert!(
        said.contains("rejected Nobody home"),
        "no answer is no: {said}"
    );
    assert_eq!(host.json(&["devices", "--json"]).await, json!([]));

    let accepted = host.pair_claimed_by("Kim's phone", listening).await;
    let (said, succeeded) = accepted.answer(Some("y\n")).await;
    assert!(
        succeeded && said.contains("paired Kim's phone as observer (dev_"),
        "{said}"
    );
    let devices = host.json(&["devices", "--json"]).await;
    let device = &devices[0];
    assert_eq!(
        (&device["name"], &device["role"]),
        (&json!("Kim's phone"), &json!("observer"))
    );
    device["id"].as_str().expect("an id").to_owned()
}

/// Rename by name, revoke by name and again by id, and list what is left.
async fn rename_then_revoke(host: &Host, id: &str) {
    let renamed = host
        .run(&["rename", "Kim's phone", "Kim's tablet"])
        .await
        .expect("renamed");
    assert_eq!(renamed, format!("renamed {id} to Kim's tablet\n"));
    let missing = host
        .run(&["revoke", "Kim's phone"])
        .await
        .expect_err("the old name");
    assert!(
        missing.contains("no paired device is named `Kim's phone`"),
        "{missing}"
    );
    let revoked = host
        .json(&["revoke", "Kim's tablet", "--reason", "lost", "--json"])
        .await;
    assert!(
        revoked["revoked_at"].is_string() && revoked["id"] == id,
        "{revoked}"
    );
    let again = host
        .run(&["revoke", id])
        .await
        .expect("revoking by id is idempotent");
    assert_eq!(again, format!("revoked Kim's tablet ({id})\n"));
    let listed = host.run(&["devices"]).await.expect("devices");
    assert!(listed.starts_with("no paired devices"), "{listed}");
    let all = host.run(&["devices", "--all"]).await.expect("devices");
    assert!(all.contains(id) && all.contains("revoked "), "{all}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_command_line_enables_pairs_and_administers() {
    let host = Host::start().await;
    let before = host.json(&["status", "--json"]).await;
    assert_eq!(before["status"]["settings"]["enabled"], false, "{before}");
    assert_eq!(before["devices"], json!([]));
    let refused = host.run(&["pair"]).await.expect_err("nothing is listening");
    assert!(refused.contains("aether remote enable"), "{refused}");

    let enabled = host.json(&["enable", "--json"]).await;
    let listening = enabled["listening"]
        .as_str()
        .expect("a bound address")
        .to_owned();
    let settings = json!({"enabled": true, "pairing": true, "public_origins": [], "push": false});
    assert_eq!(enabled["settings"], settings, "{enabled}");
    let text = host.run(&["status"]).await.expect("status");
    assert!(
        text.contains(&format!("enabled, listening on {listening}")),
        "{text}"
    );
    assert_eq!(
        host.run(&["devices"])
            .await
            .expect("devices")
            .lines()
            .count(),
        1
    );

    let id = pair_three_and_accept_one(&host, &listening).await;
    rename_then_revoke(&host, &id).await;

    let disabled = host.run(&["disable"]).await.expect("disable");
    assert_eq!(disabled, "remote control disabled\n");
    assert!(
        TcpStream::connect(&listening).await.is_err(),
        "the port is released"
    );
    host.stop().await;
}
