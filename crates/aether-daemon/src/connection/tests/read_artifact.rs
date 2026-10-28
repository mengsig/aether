//! `ReadArtifact` caps what one reply carries so it fits the client's frame.
use crate::connection::dispatch::{
    MAX_ARTIFACT_READ_BYTES, masked_artifact_slice, read_artifact_slice,
};
use aether_core::id::ArtifactId;
use aether_store::Store as _;

fn stored(bytes: &[u8]) -> (aether_store::SqliteStore, ArtifactId, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = aether_store::SqliteStore::open(dir.path().join("aether.db")).expect("store");
    let id = ArtifactId::new();
    store.write_artifact_bytes(id, bytes).expect("write");
    (store, id, dir)
}

#[test]
fn a_read_past_the_cap_returns_the_cap_and_says_it_was_cut() {
    let (store, id, _dir) = stored(&[b'a'; 100]);
    let (slice, truncated) = read_artifact_slice(&store, id, usize::MAX, 10).expect("read");
    assert_eq!(slice.len(), 10);
    assert!(truncated);
}

#[test]
fn a_payload_exactly_at_the_cap_is_not_truncated() {
    let (store, id, _dir) = stored(&[b'a'; 10]);
    let (slice, truncated) = read_artifact_slice(&store, id, 10, 10).expect("read");
    assert_eq!(slice.len(), 10);
    assert!(!truncated);
}

#[test]
fn a_cut_inside_a_character_backs_off_to_its_start() {
    let (store, id, _dir) = stored("aé€".as_bytes());
    let (slice, truncated) = read_artifact_slice(&store, id, 3, 100).expect("read");
    assert_eq!(slice, "aé".as_bytes());
    assert!(truncated);
}

#[test]
fn the_default_cap_leaves_the_base64_reply_under_the_client_frame_limit() {
    let encoded = MAX_ARTIFACT_READ_BYTES.div_ceil(3) * 4;
    assert!(encoded < 64 * 1024 * 1024 - 1024 * 1024, "{encoded}");
}

/// Masking an 8-byte secret with a 23-byte label nearly triples a slice; the
/// reply must still fit the cap it was asked for, and say it was cut.
#[test]
fn a_masked_slice_that_grows_past_the_cap_is_cut_and_marked_truncated() {
    const SHORT: &str = "s3cr3t-8";
    let redactor = aether_tools::Redactor::with_known_secrets([SHORT]).expect("compiles");
    let (store, id, _dir) = stored(SHORT.repeat(100).as_bytes());
    let (slice, truncated) =
        masked_artifact_slice(&store, id, usize::MAX, 800, &redactor).expect("read");
    assert!(truncated, "the masked reply was cut");
    assert!(slice.len() <= 800, "{}", slice.len());
    let text = String::from_utf8(slice).expect("text");
    assert!(text.starts_with("[REDACTED:known_secret]") && !text.contains(SHORT));
    let (whole, cut) =
        masked_artifact_slice(&store, id, usize::MAX, 4096, &redactor).expect("read");
    assert!(!cut && whole.len() == 2300, "{} {cut}", whole.len());
}

/// A cap the caller picks never cuts a secret so that its first part shows:
/// a reply ends before the secret, or takes it whole and masked.
#[test]
fn no_cap_shows_the_start_of_a_known_secret() {
    const KNOWN: &str = "fleet-token-0123456789abcdef";
    let redactor = aether_tools::Redactor::with_known_secrets([KNOWN]).expect("compiles");
    let text = format!("head {KNOWN} tail");
    let (store, id, _dir) = stored(text.as_bytes());
    for cap in 1..text.len() {
        let (slice, truncated) =
            masked_artifact_slice(&store, id, cap, MAX_ARTIFACT_READ_BYTES, &redactor)
                .expect("read");
        let shown = String::from_utf8(slice).expect("text");
        assert!(truncated, "cap {cap}: the reply says it was cut");
        assert!(shown.len() <= cap, "cap {cap}: {shown:?}");
        assert!(
            !shown.contains("fleet-to"),
            "cap {cap}: part of the secret shows in {shown:?}"
        );
    }
}

/// A published artifact is stored as sent; the reply to any connection, an
/// observer's included, carries the daemon's known secrets masked.
#[tokio::test]
async fn a_reply_masks_the_daemons_known_secrets() {
    use crate::authz::AuthContext;
    use crate::connection::dispatch::dispatch_version;
    use aether_core::event::Actor;
    use aether_protocol::{Command, Reply};
    use base64::Engine as _;
    const KNOWN: &str = "fleet-token-0123456789abcdef";
    let store: std::sync::Arc<dyn aether_store::Store> =
        std::sync::Arc::new(aether_store::SqliteStore::open_in_memory().expect("store"));
    let (daemon, _config_dir) = super::daemon_with_config(store.clone(), |config| {
        config.redactor = aether_tools::Redactor::with_known_secrets([KNOWN]).expect("compiles");
    })
    .await;
    let (agent, _) = daemon
        .runtime
        .create_agent(
            aether_runtime::CreateAgentSpec {
                role: "root".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
    let artifact = daemon
        .runtime
        .publish_artifact(
            aether_runtime::artifacts::ArtifactPublication {
                kind: aether_core::artifact::ArtifactKind::File,
                name: "deploy.log".into(),
                summary: "evidence".into(),
                content_type: Some("text/plain".into()),
                bytes: format!("token {KNOWN}\n\u{ff}").into_bytes(),
                agent: agent.id,
                task: None,
            },
            Actor::user(),
        )
        .await
        .expect("publish");
    let raw = store
        .read_artifact_bytes(artifact.id)
        .expect("read")
        .expect("bytes");
    assert!(
        String::from_utf8_lossy(&raw).contains(KNOWN),
        "stored as sent"
    );
    for auth in [AuthContext::Token, AuthContext::Local] {
        let command = Command::ReadArtifact {
            id: artifact.id,
            max_bytes: 1024,
        };
        let Ok(Reply::ArtifactBytes { base64, .. }) =
            dispatch_version(&daemon, command, Actor::user(), &auth).await
        else {
            panic!("an artifact reply for {auth:?}");
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(base64)
            .expect("base64");
        assert_eq!(
            String::from_utf8(bytes).expect("text"),
            "token [REDACTED:known_secret]\n\u{ff}",
            "{auth:?}"
        );
    }
    daemon.runtime.shutdown().await;
}
