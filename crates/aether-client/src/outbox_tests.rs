#![allow(clippy::expect_used)]
use super::*;
use aether_core::id::AgentId;

const HORIZON: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV:7";

fn send(content: &str, key: &str) -> Command {
    Command::SendMessage {
        to: AgentId::new(),
        content: content.into(),
        subtree: false,
        attachments: Vec::new(),
        idempotency_key: Some(key.into()),
    }
}

async fn outbox() -> (Outbox, tempfile::TempDir) {
    std::fs::create_dir_all("../../target/outbox-tests").expect("directory");
    let directory = tempfile::tempdir_in("../../target/outbox-tests").expect("fixture");
    let outbox = Outbox::open(directory.path().into()).await.expect("outbox");
    (outbox, directory)
}

#[tokio::test]
async fn pending_fields_keys_and_horizons_survive_reopening_and_are_retired_after_ack() {
    let (outbox, directory) = outbox().await;
    let command = send("exact body", "stable-key");
    let claim = outbox
        .record(&command, Some(HORIZON))
        .await
        .expect("record");
    let reopened = Outbox::open(directory.path().into()).await.expect("reopen");
    let pending = reopened.pending().await.expect("pending").entries;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].command, command);
    assert_eq!(pending[0].key(), "stable-key");
    assert_eq!(pending[0].horizon.as_deref(), Some(HORIZON));
    assert_eq!(pending[0].unconfirmed, None);
    reopened.acknowledge(&claim).await.expect("acknowledge");
    assert!(reopened.pending().await.expect("empty").entries.is_empty());
}

#[tokio::test]
async fn outbox_refuses_conflicting_key_fields_and_unkeyed_operations() {
    let (outbox, _directory) = outbox().await;
    let mut command = send("one", "stable-key");
    outbox.record(&command, None).await.expect("record");
    if let Command::SendMessage { content, .. } = &mut command {
        *content = "two".into();
    }
    assert!(matches!(
        outbox.record(&command, None).await,
        Err(ClientError::InvalidUsage(_))
    ));
    assert!(outbox.record(&Command::Ping, None).await.is_err());
}

/// However old an entry is, it is listed: age is the host's to judge, and an
/// entry that stopped the listing would stop the connection with it.
#[tokio::test]
async fn an_operation_older_than_the_hosts_window_is_still_listed() {
    let (outbox, _directory) = outbox().await;
    let command = send("uncertain", "old-key");
    let claim = outbox.record(&command, None).await.expect("record");
    let file = std::fs::File::open(outbox.path(&claim.key)).expect("recorded operation");
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 24 * 60 * 60);
    file.set_times(std::fs::FileTimes::new().set_modified(old))
        .expect("age operation");
    let pending = outbox.pending().await.expect("listed, not refused").entries;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].command, command);
}

#[tokio::test]
async fn retrying_an_unresolved_key_waits_for_its_resolution() {
    let (outbox, _directory) = outbox().await;
    let command = send("uncertain", "unresolved-key");
    outbox
        .record(&command, Some(HORIZON))
        .await
        .expect("first send");
    assert!(matches!(
        outbox
            .record(&command, Some("01ARZ3NDEKTSV4RRFFQ69G5FAV:99"))
            .await,
        Err(ClientError::DeliveryUnconfirmed(_))
    ));
    let pending = outbox.pending().await.expect("retained").entries;
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].horizon.as_deref(),
        Some(HORIZON),
        "the horizon of the first send is the one that counts"
    );
}

/// Marking an entry unconfirmed changes nothing else in it.
#[tokio::test]
async fn an_unconfirmed_mark_keeps_the_command_and_the_first_horizon() {
    let (outbox, _directory) = outbox().await;
    let command = send("uncertain", "marked-key");
    let claim = outbox
        .record(&command, Some(HORIZON))
        .await
        .expect("record");
    let before = outbox.pending().await.expect("pending").entries.remove(0);
    outbox
        .mark_unconfirmed(&claim, &before, "the host could not say")
        .await
        .expect("mark");
    let after = outbox.pending().await.expect("pending").entries.remove(0);
    assert_eq!(after.unconfirmed.as_deref(), Some("the host could not say"));
    assert_eq!(
        Unresolved {
            unconfirmed: None,
            ..after
        },
        before
    );
}

/// An entry written before horizons is the bare command. It still loads, with
/// no horizon, so the host is asked and can only answer from a receipt.
#[tokio::test]
async fn an_entry_from_before_horizons_loads_without_one() {
    let (outbox, _directory) = outbox().await;
    let command = send("legacy", "legacy-key");
    let bare = serde_json::to_vec(&command).expect("encode");
    std::fs::write(outbox.path("legacy-key"), bare).expect("an older client's entry");
    let pending = outbox.pending().await.expect("pending").entries;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].command, command);
    assert_eq!(pending[0].horizon, None);
}

#[tokio::test]
async fn an_entry_under_the_wrong_name_is_listed_as_unreadable_by_name() {
    let (outbox, _directory) = outbox().await;
    let command = send("misfiled", "real-key");
    let bare = serde_json::to_vec(&command).expect("encode");
    std::fs::write(outbox.path("another-key"), bare).expect("misfiled entry");
    let pending = outbox
        .pending()
        .await
        .expect("a bad file does not fail the listing");
    assert!(pending.entries.is_empty());
    assert_eq!(pending.unreadable.len(), 1);
    let bad = &pending.unreadable[0];
    assert_eq!(bad.path, outbox.path("another-key"));
    assert!(bad.reason.starts_with("unreadable:"), "{}", bad.reason);
    assert!(
        bad.reason.contains("does not match its key"),
        "{}",
        bad.reason
    );
    assert!(bad.reason.contains(&bad.path.display().to_string()));
}

#[tokio::test]
async fn undecodable_oversized_and_non_file_entries_are_listed_beside_good_ones() {
    let (outbox, _directory) = outbox().await;
    outbox
        .record(&send("fine", "good-key"), None)
        .await
        .expect("record");
    std::fs::write(outbox.path("garbage"), b"{ not json").expect("undecodable");
    let oversized = std::fs::File::create(outbox.path("huge")).expect("oversized");
    oversized.set_len(MAX_ENTRY_BYTES + 1).expect("sparse");
    std::fs::create_dir(outbox.path("directory")).expect("not a file");
    let pending = outbox.pending().await.expect("listed");
    assert_eq!(pending.entries.len(), 1);
    assert_eq!(pending.unreadable.len(), 3);
    for bad in &pending.unreadable {
        assert!(bad.reason.starts_with("unreadable:"), "{}", bad.reason);
        assert!(bad.reason.contains(&bad.path.display().to_string()));
    }
    let garbage = &pending
        .unreadable
        .iter()
        .find(|bad| bad.path == outbox.path("garbage"));
    assert!(
        garbage
            .expect("garbage")
            .reason
            .contains("cannot be decoded")
    );
}

#[tokio::test]
async fn discarding_an_unreadable_entry_removes_it_and_is_logged() {
    let (outbox, directory) = outbox().await;
    std::fs::write(outbox.path("garbage"), b"{ not json").expect("undecodable");
    let bad = outbox.pending().await.expect("listed").unreadable.remove(0);
    outbox.discard_unreadable(&bad).await.expect("discard");
    assert!(!bad.path.exists());
    assert!(
        outbox
            .pending()
            .await
            .expect("listed")
            .unreadable
            .is_empty()
    );
    let log = std::fs::read_to_string(directory.path().join("settled.jsonl")).expect("log");
    assert!(
        log.contains("\"discarded\"") && log.contains(&bad.key),
        "{log}"
    );
}

#[tokio::test]
async fn discarding_a_directory_entry_removes_an_empty_one_and_logs_only_a_removal() {
    let (outbox, directory) = outbox().await;
    let log = directory.path().join("settled.jsonl");
    std::fs::create_dir(outbox.path("folder")).expect("a directory named like an entry");
    std::fs::write(outbox.path("folder").join("kept"), b"x").expect("a file inside");
    let bad = outbox.pending().await.expect("listed").unreadable.remove(0);
    let refused = outbox
        .discard_unreadable(&bad)
        .await
        .expect_err("not empty");
    let named = bad.path.display().to_string();
    assert!(refused.to_string().contains(&named), "{refused}");
    assert!(
        bad.path.exists() && !log.exists(),
        "nothing is logged for a refusal"
    );
    std::fs::remove_file(bad.path.join("kept")).expect("emptied");
    outbox.discard_unreadable(&bad).await.expect("discard");
    assert!(!bad.path.exists());
    let log = std::fs::read_to_string(log).expect("log");
    assert_eq!(log.matches("\"discarded\"").count(), 1, "{log}");
}

#[tokio::test]
async fn an_entry_a_live_send_holds_is_not_claimed_by_a_second_adopter() {
    let (outbox, directory) = outbox().await;
    let in_flight = outbox
        .record(&send("in flight", "held-key"), Some(HORIZON))
        .await
        .expect("record");
    let adopter = Outbox::open(directory.path().into()).await.expect("second");
    assert!(matches!(
        adopter.claim("held-key").await.expect("claim"),
        Claimed::Busy
    ));
    outbox.acknowledge(&in_flight).await.expect("answered");
    drop(in_flight);
    assert!(matches!(
        adopter.claim("held-key").await.expect("claim"),
        Claimed::Gone
    ));
}

#[tokio::test]
async fn an_entry_whose_send_ended_can_be_claimed_and_is_exclusive_while_held() {
    let (outbox, _directory) = outbox().await;
    drop(
        outbox
            .record(&send("lost", "free-key"), Some(HORIZON))
            .await
            .expect("record"),
    );
    let Claimed::Held(claim, entry) = outbox.claim("free-key").await.expect("claim") else {
        panic!("a released entry is free");
    };
    assert_eq!(entry.key(), "free-key");
    assert!(matches!(
        outbox.claim("free-key").await.expect("claim"),
        Claimed::Busy
    ));
    outbox.acknowledge(&claim).await.expect("acknowledge");
}

#[tokio::test]
async fn a_full_outbox_refuses_new_sends_and_keeps_listing() {
    let (outbox, _directory) = outbox().await;
    for index in 0..MAX_UNRESOLVED {
        let entry = outbox.path(&format!("filler-{index}"));
        std::fs::write(entry, b"{}").expect("filler");
    }
    let refused = outbox.record(&send("one more", "over"), None).await;
    assert!(
        matches!(refused, Err(ClientError::DeliveryUnconfirmed(_))),
        "{refused:?}"
    );
    assert!(!outbox.path("over").exists());
}
