use super::*;
use crate::background::BackgroundProcesses;
use crate::testing::{Observed, ctx_in, wait_for};
use aether_core::id::AgentId;
use std::time::Duration;

async fn ready(path: &Path) {
    wait_for("background child ready", || {
        if std::fs::read_to_string(path).is_ok_and(|s| !s.is_empty()) {
            Observed::Held
        } else {
            Observed::Pending("no marker".into())
        }
    })
    .await;
}

fn alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| !s.contains(") Z "))
}

async fn dead(pid: i32) {
    wait_for("orphan group killed", || {
        if alive(pid) {
            Observed::Pending("still alive".into())
        } else {
            Observed::Held
        }
    })
    .await;
}

#[tokio::test]
async fn recovery_after_sigkill_reaps_only_this_store() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let registry = BackgroundProcesses::new();
    registry
        .recover_host_jobs(&other.path().join("store"))
        .unwrap();
    let agent = AgentId::new();
    let untouched = registry
        .start(
            agent,
            "echo $$ > pid.tmp && mv pid.tmp pid; sleep 60",
            &ctx_in(other.path()),
        )
        .unwrap();
    ready(&other.path().join("pid")).await;
    let other_pid = std::fs::read_to_string(other.path().join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    let mut fixture = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "background::journal::tests::crash_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("AETHER_BACKGROUND_TEST_DIR", dir.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    ready(&dir.path().join("ready")).await;
    let pid = std::fs::read_to_string(dir.path().join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    fixture.kill().await.unwrap();
    assert!(alive(pid), "fixture must leave a live orphan");
    let recovered = BackgroundProcesses::new();
    recovered
        .recover_host_jobs(&dir.path().join("store"))
        .unwrap();
    dead(pid).await;
    assert!(alive(other_pid), "another store's job must remain alive");
    registry.stop(agent, untouched.id).await.unwrap();
}

#[tokio::test]
#[ignore = "subprocess fixture for recovery_after_sigkill_reaps_only_this_store"]
async fn crash_fixture() {
    let dir = PathBuf::from(std::env::var_os("AETHER_BACKGROUND_TEST_DIR").unwrap());
    let registry = BackgroundProcesses::new();
    registry.recover_host_jobs(&dir.join("store")).unwrap();
    registry
        .start(
            AgentId::new(),
            "echo $$ > pid.tmp && mv pid.tmp pid; sleep 60",
            &ctx_in(&dir),
        )
        .unwrap();
    ready(&dir.join("pid")).await;
    std::fs::write(dir.join("ready"), "ready").unwrap();
    tokio::time::sleep(Duration::from_secs(60)).await;
    panic!("fixture should have been killed");
}

#[tokio::test]
async fn recovery_rejects_reused_pid_wrong_uid_and_previous_boot() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = tokio::process::Command::new("sleep")
        .arg("60")
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    let journal = Journal::recover(dir.path().join("journal")).unwrap();
    for mismatch in ["start", "uid", "boot"] {
        let (registration, durability) = journal.register(ProcessId::new(), pid).unwrap();
        durability.sync().unwrap();
        let mut record: Record =
            serde_json::from_slice(&std::fs::read(&registration.0).unwrap()).unwrap();
        match mismatch {
            "start" => record.start_time += 1,
            "uid" => record.uid = record.uid.wrapping_add(1),
            _ => record.boot_id.push_str("-previous"),
        }
        std::fs::write(&registration.0, serde_json::to_vec(&record).unwrap()).unwrap();
        Journal::recover(journal.directory.clone()).unwrap();
        assert!(alive(pid as i32), "mismatched {mismatch} must not kill");
        assert!(!registration.0.exists());
    }
    child.kill().await.unwrap();
}

#[tokio::test]
async fn recovery_kills_a_group_whose_leader_was_reaped() {
    let dir = tempfile::tempdir().unwrap();
    let id = ProcessId::new();
    let mut child = tokio::process::Command::new("bash")
        .env(JOB_ENV, id.to_string())
        .args([
            "-c",
            "sleep 60 & echo $! > descendant.tmp && mv descendant.tmp descendant; while [ ! -e exit ]; do sleep 0.01; done",
        ])
        .current_dir(dir.path())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let journal = Journal::recover(dir.path().join("journal")).unwrap();
    let (registration, durability) = journal.register(id, child.id().unwrap()).unwrap();
    durability.sync().unwrap();
    ready(&dir.path().join("descendant")).await;
    let pid = std::fs::read_to_string(dir.path().join("descendant"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    std::fs::write(dir.path().join("exit"), "exit").unwrap();
    child.wait().await.unwrap();
    assert!(alive(pid));
    Journal::recover(journal.directory).unwrap();
    dead(pid).await;
    assert!(!registration.0.exists());
}

#[test]
fn malformed_record_refuses_recovery_and_remains_visible() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::recover(dir.path().join("journal")).unwrap();
    let path = journal.directory.join("bad.json");
    std::fs::write(&path, b"incomplete").unwrap();
    let error = Journal::recover(journal.directory).unwrap_err();
    assert!(error.to_string().contains(&path.display().to_string()));
    assert!(path.exists());
}

#[tokio::test]
async fn recovery_does_not_kill_a_leaderless_group_without_the_job_token() {
    let dir = tempfile::tempdir().unwrap();
    let id = ProcessId::new();
    let mut child = tokio::process::Command::new("bash")
        .env(JOB_ENV, id.to_string())
        .args(["-c", "env -u AETHER_BACKGROUND_JOB sleep 60 & echo $! > descendant.tmp && mv descendant.tmp descendant; while [ ! -e exit ]; do sleep 0.01; done"])
        .current_dir(dir.path()).process_group(0).kill_on_drop(true).spawn().unwrap();
    let pgid = child.id().unwrap() as i32;
    let journal = Journal::recover(dir.path().join("journal")).unwrap();
    let (registration, durability) = journal.register(id, pgid as u32).unwrap();
    durability.sync().unwrap();
    ready(&dir.path().join("descendant")).await;
    let pid = std::fs::read_to_string(dir.path().join("descendant"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_for("descendant environment cleared", || {
        let cleared = std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|environment| {
            !environment
                .split(|b| *b == 0)
                .any(|entry| entry.starts_with(format!("{JOB_ENV}=").as_bytes()))
        });
        if cleared {
            Observed::Held
        } else {
            Observed::Pending("job token still present".into())
        }
    })
    .await;
    std::fs::write(dir.path().join("exit"), "exit").unwrap();
    child.wait().await.unwrap();
    Journal::recover(journal.directory).unwrap();
    let survived = alive(pid);
    killpg(Pid::from_raw(pgid), Signal::SIGKILL).unwrap();
    assert!(
        survived,
        "a same-user group without the token is not proven ours"
    );
    assert!(!registration.0.exists());
}

#[test]
fn unreadable_proc_entries_preserve_the_record_without_failing_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::recover(dir.path().join("journal")).unwrap();
    let record = Record {
        pgid: 12345,
        start_time: 1,
        boot_id: journal.boot_id.clone(),
        uid: geteuid().as_raw(),
        job: Some(ProcessId::new()),
    };
    let path = journal.directory.join("owned.json");
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let proc_root = dir.path().join("proc");
    // A directory in place of stat is unreadable even when tests run as root.
    std::fs::create_dir_all(proc_root.join("12345/stat")).unwrap();
    journal.recover_record(&path, &proc_root).unwrap();
    assert!(path.exists(), "an inconclusive check must remain retryable");
}

#[test]
fn recovery_clears_the_record_when_a_job_process_exits_mid_scan() {
    use std::os::unix::fs::PermissionsExt;
    // Root ignores the mode that makes the confirming open fail.
    if geteuid().is_root() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::recover(dir.path().join("journal")).unwrap();
    let job = ProcessId::new();
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let record = Record {
        pgid: child.id() as i32,
        start_time: 1,
        boot_id: journal.boot_id.clone(),
        uid: geteuid().as_raw(),
        job: Some(job),
    };
    let path = journal.directory.join(format!("{job}.json"));
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let proc_root = dir.path().join("proc");
    let environ = proc_root.join(child.id().to_string()).join("environ");
    std::fs::create_dir_all(environ.parent().unwrap()).unwrap();
    nix::unistd::mkfifo(&environ, nix::sys::stat::Mode::S_IRWXU).unwrap();
    // Before Linux 7.2 a process's environ turns root-owned once exit drops its
    // memory, so the confirming read fails with EACCES; the FIFO replays that.
    let writer = std::thread::spawn({
        let environ = environ.clone();
        move || {
            let mut fifo = std::fs::OpenOptions::new()
                .write(true)
                .open(&environ)
                .unwrap();
            fifo.write_all(format!("{JOB_ENV}={job}\0").as_bytes())
                .unwrap();
            std::fs::set_permissions(&environ, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
    });
    journal.recover_record(&path, &proc_root).unwrap();
    writer.join().unwrap();
    let survived = child.try_wait().unwrap().is_none();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(survived, "an unconfirmed process must not be signalled");
    assert!(
        !path.exists(),
        "a process gone mid-scan must not retain the record"
    );
}

#[test]
fn recovery_removes_unpublished_temporary_records() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::recover(dir.path().join("journal")).unwrap();
    let path = journal.directory.join(".tmp-crashed");
    std::fs::write(&path, "unfinished").unwrap();
    Journal::recover(journal.directory).unwrap();
    assert!(!path.exists());
}

#[test]
fn recovery_removes_empty_records_published_before_their_fsync() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::recover(dir.path().join("journal")).unwrap();
    let path = journal.directory.join(format!("{}.json", ProcessId::new()));
    std::fs::write(&path, b"").unwrap();
    Journal::recover(journal.directory).unwrap();
    assert!(!path.exists());
}

#[test]
fn proc_reads_classify_disappearance_and_allow_non_utf8_command_names() {
    assert!(disappeared(&io::Error::from_raw_os_error(nix::libc::ESRCH)));
    assert!(disappeared(&io::Error::from(io::ErrorKind::NotFound)));
    assert!(!disappeared(&io::Error::from(
        io::ErrorKind::PermissionDenied
    )));
    let mut fields = vec!["0"; 20];
    fields[0] = "S";
    fields[2] = "123";
    fields[19] = "456";
    let mut stat = b"123 (bad\xff) ".to_vec();
    stat.extend_from_slice(fields.join(" ").as_bytes());
    let identity = identity(&stat).unwrap();
    assert_eq!(identity.group, 123);
    assert_eq!(identity.start_time, 456);
    assert!(identity.running());
}

#[tokio::test]
async fn stop_kills_setsid_descendants_without_touching_other_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let registry = BackgroundProcesses::new();
    registry
        .recover_host_jobs(&dir.path().join("store"))
        .unwrap();
    let ctx = ctx_in(dir.path());
    let escaped = registry.start(ctx.agent_id, "setsid bash -c 'echo $$ > escaped.tmp; mv escaped.tmp escaped; exec sleep 60' </dev/null >/dev/null 2>&1 & wait", &ctx).unwrap();
    let other = registry.start(ctx.agent_id, "sleep 60", &ctx).unwrap();
    ready(&dir.path().join("escaped")).await;
    let pid = std::fs::read_to_string(dir.path().join("escaped"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(pid));
    registry.stop(ctx.agent_id, escaped.id).await.unwrap();
    dead(pid).await;
    assert!(
        registry
            .read(ctx.agent_id, other.id)
            .unwrap()
            .status
            .running
    );
    registry.stop_all().await;
}

#[tokio::test]
async fn recovery_kills_setsid_descendants_after_the_original_group_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let id = ProcessId::new();
    let mut child = tokio::process::Command::new("bash")
        .env(JOB_ENV, id.to_string())
        .args(["-c", "setsid bash -c 'echo $$ > escaped.tmp; mv escaped.tmp escaped; exec sleep 60' </dev/null >/dev/null 2>&1 & while [ ! -e exit ]; do sleep 0.01; done"])
        .current_dir(dir.path()).process_group(0).kill_on_drop(true).spawn().unwrap();
    let journal = Journal::recover(dir.path().join("journal")).unwrap();
    let (registration, durability) = journal.register(id, child.id().unwrap()).unwrap();
    durability.sync().unwrap();
    ready(&dir.path().join("escaped")).await;
    let pid = std::fs::read_to_string(dir.path().join("escaped"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    std::fs::write(dir.path().join("exit"), b"exit").unwrap();
    child.wait().await.unwrap();
    assert!(alive(pid));
    Journal::recover(journal.directory).unwrap();
    dead(pid).await;
    assert!(!registration.0.exists());
}
