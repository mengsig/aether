use super::*;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn hello_and_three_requests_in_one_write_receive_ordered_replies() {
    let (daemon, _config) = daemon().await;
    let daemon = Arc::new(daemon);
    let runtime = daemon.runtime.clone();
    let shutdown = CancellationToken::new();
    let (client, server) = tokio::io::duplex(16 * 1024);
    let serving = tokio::spawn(serve(
        daemon,
        server,
        ConnectionPolicy::default(),
        None,
        None,
        shutdown.clone(),
    ));
    let (read, mut write) = tokio::io::split(client);
    let mut input = FramedRead::new(read, LinesCodec::new());
    let mut request = aether_protocol::encode(&ClientFrame {
        id: 1,
        command: Command::Hello(aether_protocol::Hello::new("pipeline-test", "test")),
    })
    .unwrap();
    for (id, command) in [
        (2, Command::Status),
        (3, Command::Ping),
        (4, Command::Status),
    ] {
        request.push_str(&aether_protocol::encode(&ClientFrame { id, command }).unwrap());
    }
    tokio::time::timeout(Duration::from_secs(3), write.write_all(request.as_bytes()))
        .await
        .expect("write should not block")
        .unwrap();

    let replies = tokio::time::timeout(Duration::from_secs(3), async {
        let mut ids = Vec::new();
        while ids.len() < 4 {
            let line = input.next().await.unwrap().unwrap();
            let frame: ServerFrame = aether_protocol::decode(&line).unwrap();
            match frame {
                ServerFrame::Reply { id, result: Ok(_) } => ids.push(id),
                other => panic!("unexpected pipeline reply: {other:?}"),
            }
        }
        ids
    })
    .await
    .expect("every pipelined request should receive a reply");
    assert_eq!(replies, [1, 2, 3, 4]);

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), serving)
        .await
        .expect("connection should stop")
        .unwrap()
        .unwrap();
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_request_sent_after_the_welcome_still_receives_a_reply() {
    let (daemon, _config) = daemon().await;
    let daemon = Arc::new(daemon);
    let runtime = daemon.runtime.clone();
    let shutdown = CancellationToken::new();
    let (client, server) = tokio::io::duplex(16 * 1024);
    let serving = tokio::spawn(serve(
        daemon,
        server,
        ConnectionPolicy::default(),
        None,
        None,
        shutdown.clone(),
    ));
    let (read, mut write) = tokio::io::split(client);
    let mut input = FramedRead::new(read, LinesCodec::new());
    let hello = aether_protocol::encode(&ClientFrame {
        id: 1,
        command: Command::Hello(aether_protocol::Hello::new("pipeline-test", "test")),
    })
    .unwrap();
    write.write_all(hello.as_bytes()).await.unwrap();
    read_reply(&mut input, 1).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    let status = aether_protocol::encode(&ClientFrame {
        id: 2,
        command: Command::Status,
    })
    .unwrap();
    write.write_all(status.as_bytes()).await.unwrap();
    read_reply(&mut input, 2).await;

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), serving)
        .await
        .expect("connection should stop")
        .unwrap()
        .unwrap();
    runtime.shutdown().await;
}

async fn read_reply<R>(input: &mut FramedRead<R, LinesCodec>, expected_id: u64)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let line = tokio::time::timeout(Duration::from_secs(3), input.next())
        .await
        .expect("reply should arrive")
        .unwrap()
        .unwrap();
    let reply: ServerFrame = aether_protocol::decode(&line).unwrap();
    assert!(matches!(reply, ServerFrame::Reply { id, result: Ok(_) } if id == expected_id));
}
