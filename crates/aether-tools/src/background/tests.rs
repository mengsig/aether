use super::*;
use crate::testing::{Observed, WAIT_CEILING, ctx_in, wait_for};

/// Poll until `f` holds, or fail naming `what`. The conditions here are
/// single booleans, so the label is what a failure has to report.
async fn wait_until(what: &str, f: impl Fn() -> bool) {
    wait_for(what, || {
        if f() {
            Observed::Held
        } else {
            Observed::Pending("not yet".into())
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_process_has_all_of_its_output() {
    // Output still in the pipe when the shell exits belongs to the
    // process. Reporting it finished before the pipes drain loses the
    // tail: a poll sees "no new output" and reap_settled may drop the
    // handle that held it.
    let dir = tempfile::tempdir().unwrap();
    let reg = BackgroundProcesses::new();
    let agent = AgentId::new();
    let started = reg
        .start(agent, "(printf tail) & wait", &ctx_in(dir.path()))
        .unwrap();
    let mut seen = String::new();
    let deadline = std::time::Instant::now() + WAIT_CEILING;
    loop {
        let out = reg.read(agent, started.id).unwrap();
        seen.push_str(&out.text);
        if !out.status.running {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "never finished");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        seen.contains("tail"),
        "a process reported finished must have all of its output: {seen:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_process_streams_output_and_is_read_once() {
    let dir = tempfile::tempdir().unwrap();
    let reg = BackgroundProcesses::new();
    let agent = AgentId::new();
    let started = reg
        .start(agent, "echo one; sleep 0.4; echo two", &ctx_in(dir.path()))
        .unwrap();
    assert!(started.running);
    // `read` consumes, so a poll that discards what it saw can swallow
    // the line the next loop is waiting for. Accumulate instead.
    let mut seen = String::new();
    let deadline = std::time::Instant::now() + WAIT_CEILING;
    let mut after_first = String::new();
    loop {
        let out = reg.read(agent, started.id).unwrap();
        if seen.contains("one") {
            after_first.push_str(&out.text);
        }
        seen.push_str(&out.text);
        if seen.contains("one") && seen.contains("two") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no second line: {seen:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !after_first.contains("one"),
        "output already read must not repeat: {after_first:?}"
    );
    wait_until("the process to finish", || {
        !reg.read(agent, started.id).unwrap().status.running
    })
    .await;
    assert_eq!(
        reg.read(agent, started.id).unwrap().status.exit_code,
        Some(0)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_an_agent_kills_its_processes_and_leaves_others_alone() {
    let dir = tempfile::tempdir().unwrap();
    let reg = BackgroundProcesses::new();
    let mine = AgentId::new();
    let other = AgentId::new();
    let marker = dir.path().join("pid");
    let a = reg
        .start(
            mine,
            &format!(
                "echo $$ > {}.tmp && mv {}.tmp {}; sleep 60",
                marker.display(),
                marker.display(),
                marker.display()
            ),
            &ctx_in(dir.path()),
        )
        .unwrap();
    let b = reg.start(other, "sleep 60", &ctx_in(dir.path())).unwrap();
    wait_until("the process to report its pid", || {
        read_pid(&marker).is_some()
    })
    .await;
    let pid = read_pid(&marker).expect("the reported pid remains readable");

    assert_eq!(reg.stop_agent(mine).await, 1);
    wait_until("the stopped process to die", || !alive(pid)).await;
    assert!(reg.read(mine, a.id).is_err(), "a stopped process is gone");
    assert_eq!(
        reg.list(other).unwrap().len(),
        1,
        "another agent is untouched"
    );
    reg.stop(other, b.id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_cannot_reach_another_agents_process() {
    let dir = tempfile::tempdir().unwrap();
    let reg = BackgroundProcesses::new();
    let mine = AgentId::new();
    let stranger = AgentId::new();
    let p = reg.start(mine, "sleep 60", &ctx_in(dir.path())).unwrap();
    assert!(reg.read(stranger, p.id).is_err());
    assert!(reg.stop(stranger, p.id).await.is_err());
    assert!(reg.list(stranger).unwrap().is_empty());
    reg.stop(mine, p.id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_process_that_exited_is_stopped_without_signalling_its_group() {
    let dir = tempfile::tempdir().unwrap();
    let reg = BackgroundProcesses::new();
    let agent = AgentId::new();
    let p = reg.start(agent, "echo done", &ctx_in(dir.path())).unwrap();
    wait_until("the process to finish", || {
        !reg.read(agent, p.id).unwrap().status.running
    })
    .await;
    // Its group is already reaped, so stopping it is bookkeeping only.
    let status = reg.stop(agent, p.id).await.unwrap();
    assert_eq!(status.exit_code, Some(0));
    assert!(reg.read(agent, p.id).is_err(), "the handle is forgotten");
    assert!(reg.list(agent).unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn output_beyond_one_megabyte_is_preserved_and_paged() {
    let dir = tempfile::tempdir().unwrap();
    let reg = BackgroundProcesses::new();
    let agent = AgentId::new();
    let bytes = (1 << 20) + (256 << 10);
    let p = reg
        .start(
            agent,
            &format!("head -c {bytes} /dev/zero | tr '\\0' 'a'"),
            &ctx_in(dir.path()),
        )
        .unwrap();
    wait_until("the process to finish", || {
        !reg.list(agent).unwrap()[0].running
    })
    .await;
    let mut seen = String::new();
    loop {
        let out = reg.read(agent, p.id).unwrap();
        assert_eq!(out.dropped_bytes, 0);
        assert!(
            out.status.output_error.is_none(),
            "{:?}",
            out.status.output_error
        );
        assert!(out.text.len() <= crate::capture::PAGE_BYTES);
        if out.text.is_empty() {
            break;
        }
        seen.push_str(&out.text);
    }
    assert_eq!(seen, "a".repeat(bytes));
    assert_eq!(std::fs::read_to_string(&p.output_path).unwrap(), seen);
    reg.stop(agent, p.id).await.unwrap();
    assert!(p.output_path.exists());
    reg.stop_agent(agent).await;
    assert!(!p.output_path.parent().unwrap().exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_is_capped_and_a_settled_process_frees_its_slot() {
    let dir = tempfile::tempdir().unwrap();
    let reg = BackgroundProcesses::new();
    let agent = AgentId::new();
    let mut held = Vec::new();
    for _ in 0..MAX_PER_AGENT {
        held.push(reg.start(agent, "sleep 60", &ctx_in(dir.path())).unwrap());
    }
    let refused = reg
        .start(agent, "sleep 60", &ctx_in(dir.path()))
        .expect_err("the cap must hold");
    assert!(
        refused.to_string().contains(&MAX_PER_AGENT.to_string()),
        "the refusal must say what the limit is: {refused}"
    );
    // Another agent has its own budget.
    let other = AgentId::new();
    reg.start(other, "sleep 60", &ctx_in(dir.path()))
        .expect("a different agent is unaffected");

    reg.stop(agent, held[0].id).await.unwrap();
    let replacement = reg
        .start(agent, "echo freed", &ctx_in(dir.path()))
        .expect("a stopped process frees its slot");
    // A process that exited and was read frees its slot on the next start.
    wait_until("the replacement process to finish", || {
        !reg.read(agent, replacement.id).unwrap().status.running
    })
    .await;
    reg.read(agent, replacement.id).unwrap();
    reg.stop(agent, held[1].id).await.unwrap();
    assert_eq!(
        reg.list(agent).unwrap().len(),
        MAX_PER_AGENT - 1,
        "the settled process was reaped, the running ones were not"
    );
    reg.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_everything_leaves_no_process_behind() {
    let dir = tempfile::tempdir().unwrap();
    let reg = BackgroundProcesses::new();
    let markers: Vec<_> = (0..2).map(|i| dir.path().join(format!("pid{i}"))).collect();
    for (i, marker) in markers.iter().enumerate() {
        reg.start(
            AgentId::new(),
            &format!(
                "echo $$ > {}.tmp && mv {}.tmp {}; sleep 60",
                marker.display(),
                marker.display(),
                marker.display()
            ),
            &ctx_in(dir.path()),
        )
        .unwrap_or_else(|e| panic!("start {i}: {e}"));
    }
    for marker in &markers {
        wait_until("the process to report its pid", || {
            read_pid(marker).is_some()
        })
        .await;
    }
    let pids: Vec<i32> = markers
        .iter()
        .map(|m| read_pid(m).expect("the reported pid remains readable"))
        .collect();
    assert_eq!(reg.stop_all().await, 2);
    for pid in pids {
        wait_until("the stopped process to die", || !alive(pid)).await;
    }
}

fn alive(pid: i32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
        && std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|s| !s.contains(") Z "))
            .unwrap_or(false)
}

fn read_pid(path: &std::path::Path) -> Option<i32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn leader_exit_kills_descendants_and_preserves_written_output() {
    let dir = tempfile::tempdir().unwrap();
    let reg = BackgroundProcesses::new();
    let agent = AgentId::new();
    let p = reg.start(agent,
        "bash -c 'trap \"\" TERM; echo $BASHPID > descendant.tmp && mv descendant.tmp descendant; while :; do sleep 1; done' & while [ ! -s descendant ]; do sleep 0.01; done; printf written",
        &ctx_in(dir.path())).unwrap();
    wait_until("leader exit", || !reg.list(agent).unwrap()[0].running).await;
    let pid = std::fs::read_to_string(dir.path().join("descendant"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_until("descendant termination", || !alive(pid)).await;
    assert_eq!(reg.read(agent, p.id).unwrap().text, "written");
    reg.reap_settled(agent).unwrap();
    assert!(!p.output_path.parent().unwrap().exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_escalates_after_leader_exits_on_term() {
    let dir = tempfile::tempdir().unwrap();
    let reg = BackgroundProcesses::new();
    let agent = AgentId::new();
    let p = reg.start(agent,
        "bash -c 'trap \"\" TERM; echo $BASHPID > descendant.tmp && mv descendant.tmp descendant; while :; do sleep 1; done' & wait",
        &ctx_in(dir.path())).unwrap();
    let marker = dir.path().join("descendant");
    wait_until("descendant ready", || {
        std::fs::read_to_string(&marker).is_ok_and(|s| !s.is_empty())
    })
    .await;
    let pid = std::fs::read_to_string(marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    reg.stop(agent, p.id).await.unwrap();
    wait_until("TERM-resistant descendant termination", || !alive(pid)).await;
    assert!(p.output_path.exists());
    reg.stop_agent(agent).await;
    assert!(!p.output_path.parent().unwrap().exists());
}

#[tokio::test]
async fn stopped_logs_remain_readable_until_eviction_or_agent_end() {
    let dir = tempfile::tempdir().unwrap();
    let reg = BackgroundProcesses::new();
    let agent = AgentId::new();
    let mut paths = Vec::new();
    for _ in 0..=MAX_PER_AGENT {
        let p = reg
            .start(agent, "printf retained", &ctx_in(dir.path()))
            .unwrap();
        wait_until("completed output", || !reg.list(agent).unwrap()[0].running).await;
        reg.stop(agent, p.id).await.unwrap();
        assert_eq!(std::fs::read_to_string(&p.output_path).unwrap(), "retained");
        paths.push(p.output_path);
    }
    assert!(!paths[0].parent().unwrap().exists());
    assert!(paths[1..].iter().all(|p| p.exists()));
    reg.stop_agent(agent).await;
    assert!(paths.iter().all(|p| !p.parent().unwrap().exists()));
}

#[tokio::test]
async fn oversized_background_output_reports_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let reg = BackgroundProcesses::new();
    let agent = AgentId::new();
    let p = reg
        .start(agent, "head -c 9437184 /dev/zero", &ctx_in(dir.path()))
        .unwrap();
    wait_until("capped process finished", || {
        !reg.list(agent).unwrap()[0].running
    })
    .await;
    let output = reg.read(agent, p.id).unwrap();
    assert!(output.dropped_bytes > 1024 * 1024);
    assert_eq!(output.status.dropped_bytes, output.dropped_bytes);
    assert!(output.status.output_error.is_none());
    assert_eq!(
        std::fs::metadata(p.output_path).unwrap().len(),
        8 * 1024 * 1024
    );
    reg.stop_agent(agent).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn stop_skips_the_grace_when_only_the_unreaped_leader_remains() {
    let mut child = tokio::process::Command::new("sleep")
        .arg("60")
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = child.id().unwrap() as i32;
    child.start_kill().unwrap();
    wait_until("leader exit without reaping", || !alive(pid)).await;
    tokio::time::timeout(
        STOP_GRACE / 2,
        signal_until_gone(&mut child, Some(Pid::from_raw(pid)), None),
    )
    .await
    .expect("an empty live group must not wait out the grace")
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn background_and_terminal_processes_get_the_agent_temp_dir() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ctx_in(dir.path());
    let root = dir.path().join("t");
    aether_core::agent_temp::prepare_root(&root, &ctx.instance_id).unwrap();
    let leaf = aether_core::agent_temp::leaf_path(&root, ctx.agent_id);
    ctx.temp_dir = Some(leaf.clone());
    let reg = BackgroundProcesses::new();
    let expected = format!("{0}|{0}|{0}", leaf.display());
    for terminal in [false, true] {
        let started = reg
            .start_with_terminal(
                ctx.agent_id,
                r#"printf '%s|%s|%s' "$TMPDIR" "$TMP" "$TEMP""#,
                &ctx,
                terminal,
            )
            .unwrap();
        let mut seen = String::new();
        let deadline = std::time::Instant::now() + WAIT_CEILING;
        loop {
            let out = reg.read(ctx.agent_id, started.id).unwrap();
            seen.push_str(&out.text);
            if !out.status.running {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "never finished");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(seen.contains(&expected), "terminal={terminal}: {seen:?}");
    }
    assert!(leaf.is_dir());
}
