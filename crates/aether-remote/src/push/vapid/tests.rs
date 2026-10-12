use std::time::Duration;

use super::*;
use crate::push::testing::verify_authorization;

/// RFC 8292 section 2.4: a token and the key it verifies under.
const RFC8292_HEADER: &str = "vapid t=eyJ0eXAiOiJKV1QiLCJhbGciOiJFUzI1NiJ9.eyJhdWQiOiJodHRwczovL3B1c2guZXhhbXBsZS5uZXQiLCJleHAiOjE0NTM1MjM3NjgsInN1YiI6Im1haWx0bzpwdXNoQGV4YW1wbGUuY29tIn0.i3CYb7t4xfxCDquptFOepC9GAu_HLGkMlMuCGSK2rpiUfnK9ojFwDXb1JrErtmysazNjjvW2L9OkSSHzvoD1oA, k=BA1Hxzyi1RUM1b5wjxsn7nGxAszw2u61m164i3MrAIxHF6YK5h4SDYic-dRuU_RCPCfA5aq9ojSwk5Y2EmClBPs";

fn key_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("push-vapid.key")
}

#[test]
fn the_rfc8292_example_verifies() {
    let token = verify_authorization(RFC8292_HEADER).expect("the published token");
    assert_eq!(
        token.header,
        serde_json::json!({"typ": "JWT", "alg": "ES256"})
    );
    assert_eq!(
        token.claims,
        serde_json::json!({
            "aud": "https://push.example.net",
            "exp": 1_453_523_768,
            "sub": "mailto:push@example.com",
        })
    );
    let forged = RFC8292_HEADER.replace("eyJhdWQi", "eyJhdWRp");
    verify_authorization(&forged).expect_err("a changed claim breaks the signature");
}

#[test]
fn a_token_names_its_audience_expiry_and_subject() {
    let dir = tempfile::tempdir().expect("a directory");
    let key = VapidKey::load_or_create(&key_path(&dir)).expect("a new key");
    let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let header = key
        .authorization(
            "https://fcm.googleapis.com",
            Some("mailto:ops@example.com"),
            now,
        )
        .expect("signing");

    let token = verify_authorization(&header).expect("our own token verifies");
    assert_eq!(token.key, key.public_key());
    assert_eq!(
        token.header,
        serde_json::json!({"typ": "JWT", "alg": "ES256"})
    );
    assert_eq!(token.claims["aud"], "https://fcm.googleapis.com");
    assert_eq!(token.claims["sub"], "mailto:ops@example.com");
    let expiry = token.claims["exp"].as_u64().expect("a numeric expiry");
    assert_eq!(expiry, 1_800_000_000 + TOKEN_LIFETIME.as_secs());
    assert!(
        TOKEN_LIFETIME <= Duration::from_secs(24 * 3600),
        "RFC 8292 allows a day"
    );

    let anonymous = key
        .authorization("https://fcm.googleapis.com", None, now)
        .expect("signing");
    let token = verify_authorization(&anonymous).expect("verifies");
    assert!(token.claims.get("sub").is_none(), "no subject is invented");
}

#[test]
fn the_key_is_created_once_and_read_back() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = key_path(&dir);
    assert!(VapidKey::load(&path).expect("nothing yet").is_none());

    let created = VapidKey::load_or_create(&path).expect("a new key");
    let point = URL_SAFE_NO_PAD
        .decode(created.public_key())
        .expect("base64url");
    assert_eq!(point.len(), PUBLIC_KEY_BYTES);
    assert_eq!(point[0], 0x04);

    let again = VapidKey::load_or_create(&path).expect("the same key");
    let loaded = VapidKey::load(&path).expect("readable").expect("present");
    assert_eq!(again.public_key(), created.public_key());
    assert_eq!(loaded.public_key(), created.public_key());

    let names: Vec<_> = std::fs::read_dir(dir.path())
        .expect("listing")
        .map(|entry| entry.expect("an entry").file_name())
        .collect();
    assert_eq!(names, ["push-vapid.key"], "no staging file is left behind");
}

#[cfg(unix)]
#[test]
fn the_key_file_is_private_and_an_exposed_one_is_refused() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().expect("a directory");
    let path = key_path(&dir);
    VapidKey::load_or_create(&path).expect("a new key");
    let mode = std::fs::metadata(&path)
        .expect("metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    assert!(matches!(VapidKey::load(&path), Err(VapidError::Exposed(_))));
    assert!(matches!(
        VapidKey::load_or_create(&path),
        Err(VapidError::Exposed(_))
    ));

    let link = dir.path().join("link.key");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    std::os::unix::fs::symlink(&path, &link).expect("symlink");
    assert!(matches!(VapidKey::load(&link), Err(VapidError::Exposed(_))));
}

#[cfg(unix)]
#[test]
fn a_file_that_is_not_a_key_is_refused_and_never_replaced() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().expect("a directory");
    let path = key_path(&dir);
    std::fs::write(&path, b"not a key").expect("writing");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");

    assert!(matches!(
        VapidKey::load_or_create(&path),
        Err(VapidError::Malformed(_))
    ));
    assert_eq!(std::fs::read(&path).expect("reading"), b"not a key");
}

#[test]
fn the_private_key_is_never_printed() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = key_path(&dir);
    let key = VapidKey::load_or_create(&path).expect("a new key");
    let pkcs8 = std::fs::read(&path).expect("the key file");
    let printed = format!("{key:?}");
    assert!(printed.contains(key.public_key()), "{printed}");
    assert!(!printed.contains(&URL_SAFE_NO_PAD.encode(&pkcs8)));
    assert!(!printed.contains(&hex::encode(&pkcs8)));
    // The private scalar is the 32 bytes after the PKCS#8 and SEC1 prefixes.
    let scalar = &pkcs8[36..68];
    assert!(!printed.contains(&URL_SAFE_NO_PAD.encode(scalar)));
    assert!(!printed.contains(&hex::encode(scalar)));
}
