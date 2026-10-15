use super::*;

async fn serve_proposal_miss(listener: UnixListener, code: ErrorCode) {
    let (stream, _) = listener.accept().await.unwrap();
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    for index in 0..3 {
        let line = next_request(&mut lines).await.unwrap();
        let frame: ClientFrame = serde_json::from_str(&line).unwrap();
        let result = match frame.command {
            Command::Hello(_) if index == 0 => Ok(Reply::Welcome(Welcome {
                protocol_version: aether_protocol::PROTOCOL_VERSION,
                daemon_version: "stub".into(),
                instance_id: "stub".into(),
                schema_version: 1,
                pid: std::process::id(),
                features: vec![aether_protocol::features::MEMORY_PROPOSALS.into()],
                daemon_protocol_version: None,
                min_protocol_version: None,
                idle_timeout_secs: None,
                device: None,
                submission_horizon: None,
            })),
            Command::Memories { query } => {
                assert_eq!(query.id.unwrap().to_string(), MEMORY);
                assert_eq!(query.proposed, index == 2);
                if query.proposed {
                    Err(ErrorReply::new(code, "proposal query refused"))
                } else {
                    Ok(Reply::Memories(vec![]))
                }
            }
            command => panic!("unexpected request {index}: {command:?}"),
        };
        write_frame(
            &mut write,
            ServerFrame::Reply {
                id: frame.id,
                result,
            },
        )
        .await;
    }
}

#[tokio::test]
async fn memory_show_reports_not_found_when_proposal_retry_is_forbidden() {
    for code in [ErrorCode::Forbidden, ErrorCode::Internal] {
        let root = tempfile::tempdir().unwrap();
        let listener = UnixListener::bind(root.path().join("a.sock")).unwrap();
        let server = tokio::spawn(serve_proposal_miss(listener, code));
        let mut command =
            process::Command::from(isolated_command(root.path(), &["memory", "show", MEMORY]));
        command.kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(30), command.output())
            .await
            .unwrap()
            .unwrap();
        server.await.unwrap();
        assert_eq!(output.status.code(), Some(1));
        let message = stderr(&output);
        if code == ErrorCode::Forbidden {
            assert!(
                message.contains(&format!("no memory {MEMORY}")),
                "{message}"
            );
            assert!(!message.contains("proposal query refused"), "{message}");
        } else {
            assert!(message.contains("proposal query refused"), "{message}");
        }
    }
}
