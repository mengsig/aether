use std::path::PathBuf;
use std::time::Duration;

use aether_core::provider::ProviderUsage;
#[cfg(target_os = "linux")]
use nix::errno::Errno;
#[cfg(target_os = "linux")]
use nix::sys::wait::{WaitPidFlag, waitpid};
#[cfg(target_os = "linux")]
use nix::unistd::Pid;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::read_with_timeout;
use crate::{CodexAppEngine, Engine};

const SERVER: &str = r#"#!/bin/sh
set -eu
dir=${0%/*}
printf '%s\n' "$$" > "$dir/pid"
printf '%s\n' "$@" > "$dir/args"
printf '%s\n' "${HOME-unset}" > "$dir/home"
trap '' TERM
sleep 60 &
printf '%s\n' "$!" > "$dir/descendant"
if [ '@MODE@' = stall-init ]; then
    touch "$dir/ready"
    wait
fi
IFS= read -r message
printf '%s\n' "$message" >> "$dir/requests"
printf '%s\n' '{"id":1,"result":{}}'
IFS= read -r message
printf '%s\n' "$message" >> "$dir/requests"
IFS= read -r message
printf '%s\n' "$message" >> "$dir/requests"
touch "$dir/ready"
if [ '@MODE@' = stall-read ]; then wait; fi
if [ '@MODE@' = exit-with-child ]; then exit 0; fi
printf '%s\n' '{"method":"account/updated","params":{}}'
printf '%s\n' '@REPLY@'
while IFS= read -r message; do
    printf '%s\n' "$message" >> "$dir/requests"
done
wait
"#;

struct Fixture {
    dir: tempfile::TempDir,
    program: PathBuf,
}

impl Fixture {
    fn new(mode: &str, reply: &str) -> Self {
        assert!(!reply.contains('\''), "fixture JSON is shell-quoted");
        assert!(["reply", "stall-init", "stall-read", "exit-with-child"].contains(&mode));
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("codex");
        let script = SERVER.replace("@MODE@", mode).replace("@REPLY@", reply);
        crate::test_script::write(&program, &script);
        Self { dir, program }
    }

    fn engine(&self) -> CodexAppEngine {
        CodexAppEngine::new(self.program.to_str().unwrap(), "test")
    }

    fn requests(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join("requests"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    async fn ready(&self) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !self.dir.path().join("ready").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fake server started");
        assert!(self.dir.path().join("pid").exists());
        assert!(self.dir.path().join("descendant").exists());
    }

    #[cfg(target_os = "linux")]
    fn pid(&self, file: &str) -> Pid {
        let raw = std::fs::read_to_string(self.dir.path().join(file))
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        assert!(raw > 1);
        Pid::from_raw(raw)
    }

    #[cfg(target_os = "linux")]
    async fn assert_cleaned(&self) {
        let leader = self.pid("pid");
        let descendant = self.pid("descendant");
        tokio::time::timeout(Duration::from_secs(3), async {
            while PathBuf::from(format!("/proc/{leader}")).exists() || process_running(descendant) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("leader reaped and descendant stopped");
        assert_eq!(
            waitpid(leader, Some(WaitPidFlag::WNOHANG)),
            Err(Errno::ECHILD)
        );
        assert!(!process_running(descendant), "group child survived cleanup");
    }
}

#[cfg(target_os = "linux")]
fn process_running(pid: Pid) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => {
            let (_, rest) = stat.rsplit_once(')').expect("proc stat command");
            !rest.trim_start().starts_with(['Z', 'X'])
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.raw_os_error() == Some(Errno::ESRCH as i32) =>
        {
            false
        }
        Err(error) => panic!("reading process state: {error}"),
    }
}

fn quota_reply() -> Value {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/codex_rate_limits.json"
    ))
    .unwrap();
    json!({"id": 2, "result": fixture["params"]})
}

#[tokio::test]
async fn reads_account_windows_without_a_turn_and_preserves_reset_times() {
    let fixture = Fixture::new("reply", &quota_reply().to_string());
    let payload = fixture
        .engine()
        .read_usage(&["HOME".into()], CancellationToken::new())
        .await
        .unwrap()
        .unwrap();
    let report = ProviderUsage::from_engine_payload(&payload, aether_core::time::now()).unwrap();
    assert_eq!(report.windows.len(), 2);
    for (window, reset) in report.windows.iter().zip([2_000_000_000, 2_000_500_000]) {
        assert_eq!(window.quota.resets_at.unwrap().timestamp(), reset);
    }
    assert_eq!(report.windows[0].quota.used_percent, 42);
    assert_eq!(payload, quota_reply()["result"]["rateLimits"]);
    let requests = fixture.requests();
    let methods: Vec<_> = requests
        .iter()
        .map(|request| request["method"].as_str().unwrap())
        .collect();
    assert_eq!(
        methods,
        ["initialize", "initialized", "account/rateLimits/read"]
    );
    assert_eq!(requests[0]["params"]["clientInfo"]["name"], "aether");
    assert_eq!(requests[2]["params"], json!({}));
    assert_eq!(
        std::fs::read_to_string(fixture.dir.path().join("home")).unwrap(),
        "unset\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.dir.path().join("args")).unwrap(),
        "app-server\n"
    );
    #[cfg(target_os = "linux")]
    fixture.assert_cleaned().await;
}

#[tokio::test]
async fn reports_rpc_errors_and_rejects_malformed_responses() {
    let cases = [
        (
            r#"{"id":2,"error":{"code":-32000,"message":"not authenticated"}}"#,
            "not authenticated",
        ),
        ("not JSON", "invalid RPC JSON"),
        (r#"{"id":2,"result":{}}"#, "no rateLimits object"),
        (
            r#"{"id":2,"result":{"rateLimits":{}}}"#,
            "no usable quota windows",
        ),
        (r#"{"id":3,"result":{}}"#, "unexpected RPC"),
    ];
    for (reply, expected) in cases {
        let fixture = Fixture::new("reply", reply);
        let error = fixture
            .engine()
            .read_usage(&[], CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert_eq!(fixture.requests().len(), 3);
        #[cfg(target_os = "linux")]
        fixture.assert_cleaned().await;
    }
}

#[tokio::test]
async fn timeout_bounds_initialization_rpc_and_inherited_pipes() {
    for mode in ["stall-init", "stall-read", "exit-with-child"] {
        let fixture = Fixture::new(mode, "{}");
        let started = tokio::time::Instant::now();
        let error = tokio::time::timeout(
            Duration::from_secs(3),
            read_with_timeout(
                fixture.program.to_str().unwrap(),
                &[],
                CancellationToken::new(),
                Duration::from_secs(1),
            ),
        )
        .await
        .expect("whole read bounded")
        .unwrap_err();
        assert!(error.to_string().contains("timed out"), "{mode}: {error}");
        assert!(started.elapsed() < Duration::from_secs(3));
        #[cfg(target_os = "linux")]
        fixture.assert_cleaned().await;
    }
}

#[tokio::test]
async fn cancellation_waits_for_cleanup() {
    let fixture = Fixture::new("stall-read", "{}");
    let cancel = CancellationToken::new();
    let engine = fixture.engine();
    let mut read = Box::pin(engine.read_usage(&[], cancel.clone()));
    tokio::select! {
        result = &mut read => panic!("read finished before cancellation: {result:?}"),
        _ = fixture.ready() => {}
    }
    cancel.cancel();
    let error = tokio::time::timeout(Duration::from_secs(3), read)
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert_eq!(fixture.requests().len(), 3);
    #[cfg(target_os = "linux")]
    fixture.assert_cleaned().await;
}

#[tokio::test]
async fn dropping_a_read_cancels_and_reaps_its_server() {
    let fixture = Fixture::new("stall-read", "{}");
    let cancel = CancellationToken::new();
    let engine = fixture.engine();
    let mut read = Box::pin(engine.read_usage(&[], cancel.clone()));
    tokio::select! {
        result = &mut read => panic!("read finished before drop: {result:?}"),
        _ = fixture.ready() => {}
    }
    drop(read);
    assert!(
        !cancel.is_cancelled(),
        "dropping a read must not cancel its caller"
    );
    assert_eq!(fixture.requests().len(), 3);
    #[cfg(target_os = "linux")]
    fixture.assert_cleaned().await;
}

#[tokio::test]
async fn precancelled_reads_do_not_spawn() {
    let fixture = Fixture::new("stall-init", "{}");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = fixture.engine().read_usage(&[], cancel).await.unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert!(!fixture.dir.path().join("pid").exists());
}
