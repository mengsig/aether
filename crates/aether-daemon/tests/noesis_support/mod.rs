#![allow(dead_code, clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! A real daemon for the Noesis guarantee tests: the faux model as its only
//! provider, remote control configured, and every log line it writes kept.

pub mod faux;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};

use crate::phone::Host;
use crate::support::TestDaemon;
pub use faux::FauxModel;

pub const MODEL: &str = "mock/mock-1";
pub const SEALED: &str = "<sealed>";
const WAIT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(25);

/// A daemon and its faux model, with the operator's own local connection.
pub struct Home {
    pub host: Host,
    pub model: FauxModel,
}

impl Home {
    /// A fresh daemon whose `config.toml` adds `extra` to the defaults.
    pub async fn start(extra: &str) -> Home {
        capture_log();
        let model = FauxModel::start().await;
        let config = format!(
            "[model]\ndefault = \"{MODEL}\"\n\n[remote]\naddress = \"127.0.0.1:0\"\nmax_connections = 8\n{extra}"
        );
        let providers = model.providers_toml();
        let daemon = TestDaemon::start_with_files(&[
            ("config.toml", config.as_str()),
            ("providers.toml", providers.as_str()),
        ])
        .await;
        let host = Host::over(daemon).await;
        Home { host, model }
    }

    /// Stop and start the daemon over the same directories and faux model.
    pub async fn restart(self) -> Home {
        let Home { host, model } = self;
        let daemon = host.daemon.restart().await;
        Home {
            host: Host::over(daemon).await,
            model,
        }
    }

    pub async fn ok(&mut self, frame: Value) -> Value {
        self.host.ok(frame).await
    }

    pub async fn err(&mut self, frame: Value) -> Value {
        self.host.err(frame).await
    }

    /// The directory the daemon keeps `aether.db`, and would keep `noesis.db`, in.
    pub fn data_dir(&self) -> PathBuf {
        self.host
            .daemon
            .store_path()
            .parent()
            .expect("a data dir")
            .to_path_buf()
    }

    pub fn noesis_db(&self) -> PathBuf {
        self.data_dir().join("noesis.db")
    }

    pub fn config_toml(&self) -> String {
        std::fs::read_to_string(self.host.daemon.config_dir().join("config.toml"))
            .expect("config.toml")
    }

    pub async fn configure(&mut self, patch: Value) -> Value {
        assert!(patch.is_object(), "a settings patch is an object");
        self.ok(json!({"cmd": "noesis_configure", "patch": patch}))
            .await
    }

    /// Enabled on the faux model, with no disable grace so a disable ends at once.
    pub async fn enable(&mut self) -> Value {
        self.enable_with_grace(0).await
    }

    /// Enabled on the faux model, a disable letting a turn finish for `grace` seconds.
    pub async fn enable_with_grace(&mut self, grace: u32) -> Value {
        let status = self.configure(enable_patch(grace)).await;
        assert_eq!(status["enabled"], true, "{status}");
        assert_eq!(status["disable_policy"]["grace_seconds"], grace, "{status}");
        status
    }

    pub async fn disable(&mut self) -> Value {
        let status = self.configure(json!({"enabled": false})).await;
        assert_eq!(status["enabled"], false, "{status}");
        status
    }

    pub async fn last_seq(&mut self) -> u64 {
        let page = self
            .ok(json!({"cmd": "get_events", "query": {"before": u64::MAX}, "after": 0, "limit": 1}))
            .await;
        let events = page.as_array().expect("an event page is a list");
        assert!(events.len() <= 1, "one event asked for: {page}");
        let seq = events
            .first()
            .map(|event| event["seq"].as_u64().expect("a seq"));
        seq.expect("a running daemon has logged its start")
    }

    pub async fn session(&mut self, id: &str) -> Value {
        self.ok(json!({"cmd": "noesis_get_session", "session": id}))
            .await
    }

    pub async fn create_session(&mut self, mode: &str, goal: &str) -> Value {
        assert!(
            matches!(mode, "teach" | "practice"),
            "a session mode: {mode}"
        );
        let key = format!("create-{}", ulid::Ulid::generate());
        self.ok(json!({"cmd": "noesis_create_session", "goal": goal, "mode": mode, "idempotency_key": key}))
            .await
    }

    /// Poll `id`'s session until `ready` holds for it.
    pub async fn session_until(
        &mut self,
        id: &str,
        what: &str,
        ready: impl Fn(&Value) -> bool,
    ) -> Value {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            let session = self.session(id).await;
            if ready(&session) {
                return session;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{what} never happened: {session}"
            );
            tokio::time::sleep(POLL).await;
        }
    }

    /// Wait until the faux model has been asked `count` times.
    pub async fn requests_reach(&self, count: usize) -> Vec<Value> {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            let requests = self.model.requests();
            if requests.len() >= count {
                return requests;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the model was asked {} times, not {count}",
                requests.len()
            );
            tokio::time::sleep(POLL).await;
        }
    }
}

/// The patch that turns Noesis on over the faux model.
pub fn enable_patch(grace: u32) -> Value {
    json!({
        "enabled": true,
        "models": {"strong": MODEL, "cheap": MODEL},
        "billing": {"allow_api": true},
        "disable": {"grace_seconds": grace},
    })
}

/// Wait until this process no longer holds `path` open; how long that took.
pub async fn await_closed(path: &Path, within: Duration) -> Duration {
    let started = std::time::Instant::now();
    while holds_open(path) {
        assert!(
            started.elapsed() < within,
            "{} is still open after {within:?}",
            path.display()
        );
        tokio::time::sleep(POLL).await;
    }
    started.elapsed()
}

/// Whether this process (the daemon runs in it) holds `path`, or a file
/// beside it that starts with its name (a SQLite sidecar), open.
pub fn holds_open(path: &Path) -> bool {
    let dir = path.parent().expect("a file in a directory");
    let dir = std::fs::canonicalize(dir).expect("the data dir exists");
    let prefix = dir.join(path.file_name().expect("a file name"));
    let prefix = prefix.to_string_lossy();
    let open = open_files();
    assert!(!open.is_empty(), "a running test process has files open");
    open.iter()
        .any(|target| target.to_string_lossy().starts_with(&*prefix))
}

#[cfg(target_os = "linux")]
fn open_files() -> Vec<PathBuf> {
    let fds = std::fs::read_dir("/proc/self/fd").expect("this process's fds");
    // An fd closed between listing and reading is no longer open.
    fds.filter_map(Result::ok)
        .filter_map(|fd| std::fs::read_link(fd.path()).ok())
        .collect()
}

/// No procfs here (macOS): `lsof` lists the names this process holds open.
#[cfg(not(target_os = "linux"))]
fn open_files() -> Vec<PathBuf> {
    let pid = std::process::id().to_string();
    let listed = std::process::Command::new("lsof")
        .args(["-n", "-P", "-Fn", "-p", pid.as_str()])
        .output()
        .expect("lsof lists this process's open files");
    assert!(
        listed.status.success(),
        "lsof failed: {}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let names = String::from_utf8_lossy(&listed.stdout).into_owned();
    names
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .map(PathBuf::from)
        .collect()
}

/// `noesis.db` is neither on disk nor open.
pub fn assert_no_noesis_db(home: &Home, when: &str) {
    let db = home.noesis_db();
    assert!(!db.exists(), "noesis.db was created {when}");
    assert!(!holds_open(&db), "noesis.db is open {when}");
}

/// The error is the one every Noesis refusal while disabled carries.
pub fn assert_noesis_disabled(error: &Value, what: &str) {
    assert_eq!(error["code"], "unavailable", "{what}: {error}");
    assert_eq!(error["reason"], "noesis_disabled", "{what}: {error}");
}

/// The data of an answer frame, or the error it carried.
pub fn answer_of(answer: &Value) -> Result<Value, Value> {
    match answer["result"].get("Ok") {
        Some(ok) => Ok(ok["data"].clone()),
        None => Err(answer["result"]["Err"].clone()),
    }
}

/// Every event and delta the daemon logs lands here, for the whole test binary.
static LOG: OnceLock<Mutex<Vec<u8>>> = OnceLock::new();

struct LogWriter;

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let log = LOG
            .get()
            .expect("the log is captured before anything writes");
        log.lock().expect("log lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Route every `tracing` record at every level to [`LOG`]; a superset of
/// what `aetherd`'s own log file keeps.
fn capture_log() {
    if LOG.set(Mutex::new(Vec::new())).is_err() {
        return;
    }
    let installed = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(|| LogWriter)
        .try_init();
    installed.expect("no other subscriber is installed in this test binary");
}

/// The daemon log so far.
pub fn log_text() -> String {
    let log = LOG.get().expect("the log is captured");
    let bytes = log.lock().expect("log lock").clone();
    String::from_utf8_lossy(&bytes).into_owned()
}
