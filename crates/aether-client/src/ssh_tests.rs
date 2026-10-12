#![allow(clippy::expect_used)]
use super::*;
fn fake_ssh() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ssh-hold")
}

async fn reaped(diagnostics: Arc<Mutex<String>>) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if diagnostics
                .lock()
                .expect("diagnostics")
                .contains("SSH exited")
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("child reaped");
    assert!(
        diagnostics
            .lock()
            .expect("diagnostics")
            .contains("SSH exited")
    );
}

#[tokio::test]
async fn dropping_last_client_cancels_and_reaps_carrier() {
    let program = fake_ssh();
    let remote = SshRemote::new("box", "fake-host").expect("profile");
    let client = spawn_client(&remote, program.as_os_str()).expect("spawn");
    let diagnostics = client
        .carrier
        .as_ref()
        .expect("carrier")
        .diagnostics
        .clone();
    let cloned = client.clone();
    drop(client);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !diagnostics
            .lock()
            .expect("diagnostics")
            .contains("SSH exited")
    );
    drop(cloned);
    reaped(diagnostics).await;
}

#[tokio::test]
async fn cancelling_a_connection_future_reaps_the_child() {
    let program = fake_ssh();
    let remote = SshRemote::new("box", "fake-host").expect("profile");
    let client = spawn_client(&remote, program.as_os_str()).expect("spawn");
    let diagnostics = client
        .carrier
        .as_ref()
        .expect("carrier")
        .diagnostics
        .clone();
    let task = tokio::spawn(async move {
        let _held_client = client;
        std::future::pending::<()>().await;
    });
    task.abort();
    assert!(task.await.expect_err("cancelled").is_cancelled());
    reaped(diagnostics).await;
}
