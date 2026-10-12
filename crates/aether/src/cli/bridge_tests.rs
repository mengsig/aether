#![allow(clippy::expect_used)]
use super::*;
use tokio::io::AsyncReadExt as _;

#[tokio::test]
async fn pump_preserves_bytes_and_drains_reply_after_input_half_close() {
    let (bridge, mut daemon) = tokio::net::UnixStream::pair().expect("pair");
    let (mut writer, input) = tokio::io::duplex(4096);
    let (output, mut reader) = tokio::io::duplex(4096);
    let bytes: Vec<u8> = (0..=255).cycle().take(3072).collect();
    let sent = bytes.clone();
    let host = tokio::spawn(async move {
        let mut received = Vec::new();
        daemon
            .read_to_end(&mut received)
            .await
            .expect("request bytes");
        assert_eq!(received, sent);
        daemon.write_all(&received).await.expect("response");
        daemon.shutdown().await.expect("half-close");
    });
    writer.write_all(&bytes).await.expect("stdin");
    writer.shutdown().await.expect("stdin EOF");
    let pumping = tokio::spawn(pump(bridge, input, output));
    let mut echoed = Vec::new();
    reader.read_to_end(&mut echoed).await.expect("stdout");
    assert_eq!(echoed, bytes);
    assert!(pumping.await.expect("pump task").is_ok());
    host.await.expect("host");
}

#[tokio::test]
async fn daemon_eof_finishes_even_when_stdin_stays_open() {
    let (socket, daemon) = tokio::net::UnixStream::pair().expect("pair");
    let (_writer, input) = tokio::io::duplex(64);
    drop(daemon);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        pump(socket, input, tokio::io::sink()),
    )
    .await;
    assert!(result.expect("EOF finishes").is_ok());
    assert!(
        bridge(PathBuf::from("target/no-such-remote.sock"), false)
            .await
            .is_err()
    );
}
