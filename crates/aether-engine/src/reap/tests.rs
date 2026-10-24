#![allow(clippy::unwrap_used)]

use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt as _;

use aether_core::id::AgentId;

use super::linux::{carries, cgroup_members, is_live, tagged};
use super::reap_agent;

const UID: u32 = 4242;

/// One fake `/proc/<pid>` with a `stat` in `state` and the given environment.
fn fake_process(root: &Path, pid: u32, state: &str, environment: &[&str]) {
    let dir = root.join(pid.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("stat"),
        format!("{pid} (a (b) c) {state} 1 {pid} 0\n"),
    )
    .unwrap();
    std::fs::write(dir.join("environ"), environment.join("\0")).unwrap();
}

fn own_uid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

#[test]
fn selects_live_processes_carrying_the_exact_tag() {
    let root = tempfile::tempdir().unwrap();
    let tag = "AETHER_ENGINE_AGENT=agt_A";
    fake_process(root.path(), 10, "S", &["HOME=/home/user", tag]);
    fake_process(root.path(), 11, "R", &[tag]);
    fake_process(root.path(), 12, "S", &["AETHER_ENGINE_AGENT=agt_AB"]);
    fake_process(root.path(), 13, "Z", &[tag]);
    fake_process(root.path(), 14, "S", &["X=AETHER_ENGINE_AGENT=agt_A"]);
    fake_process(root.path(), 15, "S", &[tag]);
    fake_process(root.path(), 1, "S", &[tag]);
    std::fs::create_dir(root.path().join("self")).unwrap();
    std::fs::write(root.path().join("uptime"), "1 1").unwrap();

    assert_eq!(tagged(root.path(), tag, 15, own_uid()), vec![10, 11]);
}

#[test]
fn never_selects_another_users_process() {
    let root = tempfile::tempdir().unwrap();
    let tag = "AETHER_ENGINE_AGENT=agt_A";
    fake_process(root.path(), 10, "S", &[tag]);
    assert!(carries(root.path(), 10, tag, own_uid()));
    assert!(!carries(root.path(), 10, tag, UID));
    assert!(tagged(root.path(), tag, 0, UID).is_empty());
}

#[test]
fn a_process_without_a_readable_stat_or_environment_is_not_selected() {
    let root = tempfile::tempdir().unwrap();
    let tag = "AETHER_ENGINE_AGENT=agt_A";
    fake_process(root.path(), 10, "S", &[tag]);
    std::fs::remove_file(root.path().join("10/stat")).unwrap();
    fake_process(root.path(), 11, "S", &[tag]);
    std::fs::remove_file(root.path().join("11/environ")).unwrap();
    assert!(!is_live(root.path(), 10));
    assert!(tagged(root.path(), tag, 0, own_uid()).is_empty());
}

#[test]
fn cgroup_members_are_the_live_listed_pids() {
    let root = tempfile::tempdir().unwrap();
    fake_process(root.path(), 20, "S", &[]);
    fake_process(root.path(), 21, "Z", &[]);
    fake_process(root.path(), 22, "S", &[]);
    let procs = root.path().join("cgroup.procs");
    std::fs::write(&procs, "20\n21\n22\n23\n").unwrap();

    assert_eq!(cgroup_members(root.path(), &procs, 22), vec![20]);
    let gone = root.path().join("gone/cgroup.procs");
    assert!(cgroup_members(root.path(), &gone, 0).is_empty());
}

/// Start `script` as an engine of `agent` is - spawned, then moved into its
/// scope before it forks - and return the pid it prints.
async fn start_detached(agent: AgentId, script: &str) -> u32 {
    let mut command = crate::scope::command("sh", Some(agent));
    command
        .args(["-c", &format!("read go; {script}")])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    command.as_std_mut().process_group(0);
    let mut child = command.spawn().unwrap();
    crate::scope::adopt(child.id(), Some(agent)).await;
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"go\n").await.unwrap();
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

async fn wait_gone(pid: u32) -> bool {
    for _ in 0..100 {
        if !is_live(Path::new("/proc"), pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

fn still_tagged(agent: AgentId) -> Vec<u32> {
    let tag = format!("AETHER_ENGINE_AGENT={agent}");
    tagged(Path::new("/proc"), &tag, std::process::id(), own_uid())
}

fn scopes_available() -> bool {
    std::process::Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "--collect", "--", "true"])
        .status()
        .is_ok_and(|status| status.success())
}

#[tokio::test]
async fn reaps_a_detached_descendant_once_its_agent_ends() {
    let agent = AgentId::new();
    let pid = start_detached(
        agent,
        "setsid sleep 37 </dev/null >/dev/null 2>&1 & echo $!",
    )
    .await;
    assert!(is_live(Path::new("/proc"), pid), "the detached child runs");

    let report = reap_agent(agent, Duration::from_secs(5), || true).await;

    assert!(report.terminated.contains(&pid), "{report:?}");
    assert!(
        report.killed.is_empty(),
        "sleep honours SIGTERM: {report:?}"
    );
    assert!(wait_gone(pid).await, "the detached child is gone");
}

#[tokio::test]
async fn kills_a_process_that_outlives_the_grace() {
    let agent = AgentId::new();
    // An ignored signal stays ignored across exec.
    let pid = start_detached(
        agent,
        "setsid sh -c 'trap \"\" TERM; exec sleep 38' </dev/null >/dev/null 2>&1 & echo $!",
    )
    .await;
    // Let the trap take effect before the reap.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let report = reap_agent(agent, Duration::from_millis(300), || true).await;

    assert!(report.killed.contains(&pid), "{report:?}");
    assert!(report.survived.is_empty(), "{report:?}");
    assert!(wait_gone(pid).await, "the stubborn child is killed");
}

#[tokio::test]
async fn a_zero_grace_still_sends_sigterm_first() {
    let agent = AgentId::new();
    let pid = start_detached(
        agent,
        "setsid sh -c 'trap \"\" TERM; exec sleep 45' </dev/null >/dev/null 2>&1 & echo $!",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let report = reap_agent(agent, Duration::ZERO, || true).await;

    assert!(report.terminated.contains(&pid), "{report:?}");
    assert!(report.killed.contains(&pid), "{report:?}");
    assert!(wait_gone(pid).await, "the stubborn child is killed");
}

#[tokio::test]
async fn a_child_forked_on_sigterm_is_reaped_too() {
    let agent = AgentId::new();
    let pid = start_detached(
        agent,
        "setsid sh -c 'trap \"setsid sleep 44 </dev/null >/dev/null 2>&1 & exit 0\" TERM; \
         while :; do sleep 0.1; done' </dev/null >/dev/null 2>&1 & echo $!",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let report = reap_agent(agent, Duration::from_secs(5), || true).await;

    assert!(report.terminated.contains(&pid), "{report:?}");
    assert!(report.survived.is_empty(), "{report:?}");
    assert!(
        still_tagged(agent).is_empty(),
        "the forked child is gone too: {report:?}"
    );
}

#[tokio::test]
async fn an_agent_without_processes_reaps_nothing() {
    let report = reap_agent(AgentId::new(), Duration::from_secs(5), || true).await;
    assert!(report.is_empty(), "{report:?}");
}

#[tokio::test]
async fn the_scope_reaps_a_descendant_that_cleared_its_environment() {
    if !scopes_available() {
        eprintln!("skipped: no systemd user scopes here");
        return;
    }
    let agent = AgentId::new();
    let pid = start_detached(
        agent,
        "setsid env -i sleep 39 </dev/null >/dev/null 2>&1 & echo $!",
    )
    .await;
    assert_eq!(crate::scope::agent_units(agent).await.len(), 1);

    let report = reap_agent(agent, Duration::from_secs(5), || true).await;

    assert!(report.terminated.contains(&pid), "{report:?}");
    assert!(wait_gone(pid).await, "the untagged child is gone");
}

#[tokio::test]
async fn a_restarted_agent_keeps_its_next_engine() {
    let agent = AgentId::new();
    let stubborn = start_detached(
        agent,
        "setsid sh -c 'trap \"\" TERM; exec sleep 40' </dev/null >/dev/null 2>&1 & echo $!",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let ended = Arc::new(AtomicBool::new(true));
    let still_ended = ended.clone();
    let reap = tokio::spawn(reap_agent(agent, Duration::from_secs(2), move || {
        still_ended.load(Ordering::SeqCst)
    }));
    tokio::time::sleep(Duration::from_millis(500)).await;
    // The agent is restarted, and its next engine carries the same tag.
    ended.store(false, Ordering::SeqCst);
    let next = start_detached(
        agent,
        "setsid sleep 41 </dev/null >/dev/null 2>&1 & echo $!",
    )
    .await;

    let report = reap.await.unwrap();

    assert!(report.terminated.contains(&stubborn), "{report:?}");
    assert!(report.killed.is_empty(), "{report:?}");
    assert!(is_live(Path::new("/proc"), next), "the next engine runs on");
    let cleanup = reap_agent(agent, Duration::from_millis(300), || true).await;
    assert!(cleanup.terminated.contains(&next), "{cleanup:?}");
    assert!(cleanup.killed.contains(&stubborn), "{cleanup:?}");
}
