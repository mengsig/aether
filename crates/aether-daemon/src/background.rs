use std::{future::Future, sync::Arc};

use aether_runtime::{Runtime, supervision};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub(crate) fn isolate(
    tasks: &mut JoinSet<()>,
    runtime: &Arc<Runtime>,
    name: &'static str,
    shutdown: &CancellationToken,
    expected_exit: bool,
    work: impl Future<Output = ()> + Send + 'static,
) {
    let runtime = runtime.clone();
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        supervision::isolate(
            name,
            shutdown,
            expected_exit,
            |detail| runtime.report_problem(format!("background.{name}"), detail),
            work,
        )
        .await;
    });
}

pub(crate) fn github(
    tasks: &mut JoinSet<()>,
    runtime: &Arc<Runtime>,
    shutdown: &CancellationToken,
) {
    let runtime = runtime.clone();
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        supervision::restart(
            "GitHub watcher",
            shutdown.clone(),
            |detail| runtime.report_problem("background.github".into(), detail),
            || runtime.clone().run_github_watcher(shutdown.clone()),
        )
        .await;
    });
}
