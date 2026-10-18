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
