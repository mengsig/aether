use super::*;

#[cfg(unix)]
#[tokio::test]
async fn auth_reply_keeps_cli_and_ollama_ready_with_an_unsafe_credential_directory() {
    use std::os::unix::fs::PermissionsExt;

    let (mut daemon, config) = daemon().await;
    std::fs::write(
        config.path().join("providers.toml"),
        r#"
[providers.claude-code.auth.cli]
program = "/bin/sh"
status = ["-c", "printf '{\n  \"loggedIn\": true\n}\n'"]
login = ["-c", "true"]
logged_in_markers = ['"loggedIn": true']
label = "test"
[providers.codex.auth.cli]
program = "/bin/sh"
status = ["-c", "printf 'Logged in'"]
login = ["-c", "true"]
logged_in_markers = ["Logged in"]
label = "test"
"#,
    )
    .expect("provider overrides");
    std::fs::set_permissions(config.path(), std::fs::Permissions::from_mode(0o775))
        .expect("shared config");
    daemon.models = Arc::new(
        aether_model::Models::load(Some(config.path().to_owned()))
            .expect("models")
            .with_env(|_| None),
    );
    let Reply::Auth(rows) = dispatch(&daemon, Command::AuthStatus, Actor::user())
        .await
        .expect("auth status")
    else {
        panic!("auth reply");
    };
    for id in ["claude-code", "codex", "ollama"] {
        let row = rows
            .iter()
            .find(|row| row.provider == id)
            .expect("provider");
        assert!(row.configured, "{row:?}");
        assert!(row.detail.is_none(), "{row:?}");
    }
    let api = rows.iter().find(|row| row.provider == "anthropic").unwrap();
    assert!(!api.configured);
    let detail = api.detail.as_deref().expect("store failure");
    assert!(detail.contains("group- or world-writable"), "{detail}");
    assert!(detail.contains("chmod 700"), "{detail}");
    assert!(!config.path().join("credentials.json.lock").exists());
    daemon.runtime.shutdown().await;
}
