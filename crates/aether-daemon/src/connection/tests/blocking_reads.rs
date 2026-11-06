use super::attachments::run;
use super::*;
use crate::authz::AuthContext;
use aether_core::id::{AgentId, RunId};
use aether_protocol::Command;

/// Heavy reads must leave the async worker: with the only blocking thread
/// held, each one parks off-worker, and `Ping` still answers.
#[test]
fn heavy_reads_wait_off_the_async_worker_and_ping_still_answers() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (daemon, _config) = daemon().await;
        // Dropping `release` on an assertion failure also frees the thread.
        let (release, held) = std::sync::mpsc::channel::<()>();
        let hold = tokio::task::spawn_blocking(move || {
            let _ = held.recv();
        });
        let reads = [
            Command::ListAgents {
                query: Default::default(),
            },
            Command::ListRuns { limit: 5 },
            Command::GetRunReport { run: RunId::new() },
            Command::Replay {
                agent: AgentId::new(),
            },
            Command::GetTree {
                role_prefix: None,
                root: None,
                run: None,
                include_archived: false,
            },
        ];
        for read in reads {
            let name = format!("{read:?}");
            let pending = run(&daemon, &AuthContext::Local, read);
            tokio::pin!(pending);
            assert!(
                futures::poll!(&mut pending).is_pending(),
                "{name} ran inline on the async worker"
            );
            let pong = run(&daemon, &AuthContext::Local, Command::Ping).await;
            assert!(matches!(pong, Ok(aether_protocol::Reply::Ok)), "{name}");
        }
        drop(release);
        hold.await.unwrap();
        daemon.runtime.shutdown().await;
    });
}

/// `status` while its read cannot run, as on a host too loaded to schedule
/// it: answered within its patience and marked stale, never a timeout. Before
/// the shared read it waited for the thread, and here that never comes.
#[test]
fn status_answers_stale_while_its_read_cannot_run() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (daemon, _config) = daemon().await;
        let (release, held) = std::sync::mpsc::channel::<()>();
        let hold = tokio::task::spawn_blocking(move || {
            let _ = held.recv();
        });
        let bound = crate::status_reads::PATIENCE * 3;
        let status =
            tokio::time::timeout(bound, run(&daemon, &AuthContext::Local, Command::Status));
        let Ok(Ok(aether_protocol::Reply::Status(status))) = status.await else {
            panic!("status did not answer within {bound:?} while its read was held");
        };
        let stale = status.stale.as_ref().expect("marked stale");
        assert_eq!(stale.read_at, None, "nothing was read yet");
        assert_eq!(status.agents_live, 0);
        assert!(
            status.problems[0].contains("unknown"),
            "{:?}",
            status.problems
        );
        assert!(
            status.max_concurrent_turns.is_some_and(|turns| turns > 0),
            "the configured limits are there without a read"
        );

        drop(release);
        hold.await.unwrap();
        let fresh = async {
            loop {
                let reply = run(&daemon, &AuthContext::Local, Command::Status).await;
                let Ok(aether_protocol::Reply::Status(status)) = reply else {
                    panic!("status failed: {reply:?}");
                };
                // A loaded host can run a section past its deadline; wait for a whole read.
                if status.stale.is_none() && status.unavailable.is_empty() {
                    return status;
                }
            }
        };
        let status = tokio::time::timeout(Duration::from_secs(60), fresh)
            .await
            .expect("a fresh status once the read can run");
        assert!(
            status.problems.iter().all(|p| !p.contains("status")),
            "{:?}",
            status.problems
        );
        daemon.runtime.shutdown().await;
    });
}
