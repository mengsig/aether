//! `ReadArtifact` caps what one reply carries so it fits the client's frame.
use crate::connection::dispatch::{MAX_ARTIFACT_READ_BYTES, read_artifact_slice};
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
