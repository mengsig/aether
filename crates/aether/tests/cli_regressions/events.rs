use super::*;

#[tokio::test]
async fn events_tail_pages_backwards_and_prints_chronologically() {
    let output = cli(
        &["events", "--agent", AGENT, "--tail", "1005", "--json"],
        Stub {
            event_count: Some(2012),
            ..Default::default()
        },
    )
    .await;
    assert!(output.status.success(), "{}", stderr(&output));
    let seqs: Vec<_> = stdout(&output)
        .lines()
        .map(|line| serde_json::from_str::<Event>(line).unwrap().seq)
        .collect();
    assert_eq!(seqs, (1008..=2012).collect::<Vec<_>>());
}

#[tokio::test]
async fn events_tail_respects_after_and_local_filters() {
    for (extra, expected) in [
        (vec!["--after", "2010"], vec![2011, 2012]),
        (vec!["--grep", "failed"], vec![2010, 2011, 2012]),
        (vec!["--grep", "absent"], vec![]),
        (vec!["--since", "1s"], vec![]),
        (vec!["--after", "3000"], vec![]),
        (vec!["--tail", "0"], vec![]),
    ] {
        let mut args = vec!["events", "--agent", AGENT, "--json"];
        if !extra.contains(&"--tail") {
            args.extend(["--tail", "3"]);
        }
        args.extend(extra);
        let output = cli(
            &args,
            Stub {
                event_count: Some(2012),
                ..Default::default()
            },
        )
        .await;
        assert!(output.status.success(), "{}", stderr(&output));
        let seqs: Vec<_> = stdout(&output)
            .lines()
            .map(|line| serde_json::from_str::<Event>(line).unwrap().seq)
            .collect();
        assert_eq!(seqs, expected);
    }
}

#[tokio::test]
async fn events_follow_starts_after_tail_snapshot_and_keeps_filters() {
    for (tail, expected) in [("2", vec![9, 10, 12]), ("0", vec![12])] {
        let output = tokio::time::timeout(
            Duration::from_secs(5),
            cli(
                &[
                    "events",
                    "--agent",
                    AGENT,
                    "--tail",
                    tail,
                    "--type",
                    "compaction.failed",
                    "--grep",
                    r#""error":"failed"#,
                    "--json",
                    "-f",
                ],
                Stub {
                    event_count: Some(10),
                    event_follow: true,
                    ..Default::default()
                },
            ),
        )
        .await
        .expect("follow should finish when the stub closes");
        assert!(output.status.success(), "{}", stderr(&output));
        let seqs: Vec<_> = stdout(&output)
            .lines()
            .map(|line| serde_json::from_str::<Event>(line).unwrap().seq)
            .collect();
        assert_eq!(seqs, expected);
    }
}

#[tokio::test]
async fn events_tail_rejects_a_page_that_does_not_retreat() {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        cli(
            &["events", "--agent", AGENT, "--tail", "1001"],
            Stub {
                event_count: Some(2012),
                stale_events: true,
                ..Default::default()
            },
        ),
    )
    .await
    .expect("stale reverse pages must fail promptly");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("event page did not retreat before"));
}
