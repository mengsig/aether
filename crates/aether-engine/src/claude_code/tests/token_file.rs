//! The long-lived OAuth token file: handed to the child through its
//! environment only, re-read per spawn, and refused when unsafe.

use std::os::unix::fs::{PermissionsExt as _, symlink};

use super::*;

const TOKEN: &str = "sk-ant-oat01-NOTAREALTOKEN0123456789";

fn write_token(path: &Path, contents: &str, mode: u32) {
    std::fs::write(path, contents).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// A fake `claude` that records its argv and token variable, then exits.
fn recording_claude(out: &Path) -> tempfile::TempDir {
    fake_claude(&format!(
        "printf '%s\\n' \"$@\" > {out}/argv\nprintf '%s' \"${{CLAUDE_CODE_OAUTH_TOKEN-unset}}\" > {out}/env\ncat >/dev/null",
        out = out.display()
    ))
}

/// Launch one turn, wait until the stub has written its env file, then stop it.
async fn launch_once(engine: &ClaudeCodeEngine, dir: &Path, out: &Path, hidden: &[&str]) {
    let mut t = turn();
    t.cwd = dir.into();
    t.hidden_env = hidden.iter().map(|name| name.to_string()).collect();
    let cancel = CancellationToken::new();
    let mut run = engine.run(t, cancel.clone()).await.unwrap();
    for _ in 0..300 {
        if out.join("env").exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    cancel.cancel();
    while run.events.recv().await.is_some() {}
}

fn engine_for(dir: &tempfile::TempDir, token_file: Option<&Path>) -> ClaudeCodeEngine {
    let engine = ClaudeCodeEngine::new(dir.path().join("claude").to_string_lossy(), "2.1.265");
    match token_file {
        Some(path) => engine.with_oauth_token_file(path.to_path_buf()),
        None => engine,
    }
}

async fn refusal(path: &Path) -> String {
    let dir = fake_claude("cat >/dev/null");
    let mut t = turn();
    t.cwd = dir.path().into();
    let error = engine_for(&dir, Some(path))
        .run(t, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(error.proves_no_submission(), "{error:?}");
    let message = error.to_string();
    assert!(message.contains(&path.display().to_string()), "{message}");
    assert!(!message.contains(TOKEN), "{message}");
    message
}

#[tokio::test]
async fn token_reaches_the_child_environment_and_never_its_arguments() {
    let (work, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let file = work.path().join("token");
    write_token(&file, &format!("  {TOKEN}\n"), 0o600);
    let dir = recording_claude(out.path());
    launch_once(&engine_for(&dir, Some(&file)), dir.path(), out.path(), &[]).await;
    assert_eq!(
        std::fs::read_to_string(out.path().join("env")).unwrap(),
        TOKEN
    );
    let argv = std::fs::read_to_string(out.path().join("argv")).unwrap();
    assert!(!argv.contains(TOKEN) && !argv.contains("oat01"), "{argv}");
}

#[tokio::test]
async fn token_file_is_read_again_for_every_spawn() {
    let (work, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let file = work.path().join("token");
    let dir = recording_claude(out.path());
    let engine = engine_for(&dir, Some(&file));
    write_token(&file, "first-token-value", 0o600);
    launch_once(&engine, dir.path(), out.path(), &[]).await;
    assert_eq!(
        std::fs::read_to_string(out.path().join("env")).unwrap(),
        "first-token-value"
    );
    std::fs::remove_file(out.path().join("env")).unwrap();
    write_token(&file, "second-token-value", 0o400);
    launch_once(&engine, dir.path(), out.path(), &[]).await;
    assert_eq!(
        std::fs::read_to_string(out.path().join("env")).unwrap(),
        "second-token-value"
    );
}

#[tokio::test]
async fn without_a_token_file_aether_adds_no_token_variable() {
    let out = tempfile::tempdir().unwrap();
    let dir = recording_claude(out.path());
    // A token in the daemon's own environment is the operator's business, not Aether's.
    if std::env::var_os("CLAUDE_CODE_OAUTH_TOKEN").is_some() {
        return;
    }
    launch_once(&engine_for(&dir, None), dir.path(), out.path(), &[]).await;
    assert_eq!(
        std::fs::read_to_string(out.path().join("env")).unwrap(),
        "unset"
    );
}

#[tokio::test]
async fn a_hidden_token_variable_is_not_passed_on() {
    let (work, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let file = work.path().join("token");
    write_token(&file, TOKEN, 0o600);
    let dir = recording_claude(out.path());
    launch_once(
        &engine_for(&dir, Some(&file)),
        dir.path(),
        out.path(),
        &["CLAUDE_CODE_OAUTH_TOKEN"],
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(out.path().join("env")).unwrap(),
        "unset"
    );
}

#[tokio::test]
async fn a_group_readable_token_file_is_refused_without_quoting_it() {
    let work = tempfile::tempdir().unwrap();
    let file = work.path().join("token");
    write_token(&file, TOKEN, 0o644);
    let message = refusal(&file).await;
    assert!(
        message.contains("must have mode 0600 (found 0644)"),
        "{message}"
    );
}

#[tokio::test]
async fn a_symlink_or_directory_is_not_a_token_file() {
    let work = tempfile::tempdir().unwrap();
    let real = work.path().join("real");
    write_token(&real, TOKEN, 0o600);
    let link = work.path().join("link");
    symlink(&real, &link).unwrap();
    refusal(&link).await;
    let message = refusal(work.path()).await;
    assert!(message.contains("regular file"), "{message}");
}

#[tokio::test]
async fn empty_multiline_and_missing_token_files_are_refused() {
    let work = tempfile::tempdir().unwrap();
    let file = work.path().join("token");
    write_token(&file, " \n", 0o600);
    assert!(refusal(&file).await.contains("is empty"));
    write_token(&file, &format!("{TOKEN}\nsecond line"), 0o600);
    assert!(refusal(&file).await.contains("single line"));
    refusal(&work.path().join("absent")).await;
}

#[tokio::test]
async fn a_token_the_child_prints_is_masked_in_its_output_and_error() {
    // No built-in rule knows this shape: only the per-spawn mask hides it.
    const PLAIN: &str = "plain-oauth-value-0123456789";
    let work = tempfile::tempdir().unwrap();
    let file = work.path().join("token");
    write_token(&file, PLAIN, 0o600);
    let dir = fake_claude(concat!(
        "cat >/dev/null &\n",
        "printf '{\"type\":\"assistant\",\"message\":{\"id\":\"m\",\"model\":\"m\",",
        "\"content\":[{\"type\":\"text\",\"text\":\"env %s\"}],\"usage\":{}}}\\n' ",
        "\"$CLAUDE_CODE_OAUTH_TOKEN\"\n",
        "echo \"token=$CLAUDE_CODE_OAUTH_TOKEN\" >&2\n",
        "exit 1",
    ));
    let mut t = turn();
    t.cwd = dir.path().into();
    let mut run = engine_for(&dir, Some(&file))
        .run(t, CancellationToken::new())
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Some(event) = run.events.recv().await {
        events.push(event);
    }
    let seen = format!("{events:?}");
    assert!(!seen.contains(PLAIN), "{seen}");
    let masked = aether_core::redact::KNOWN_SECRET_MASK;
    assert!(seen.contains(&format!("env {masked}")), "stdout: {seen}");
    assert!(seen.contains(&format!("token={masked}")), "stderr: {seen}");
}

#[tokio::test]
async fn a_short_or_quoted_token_is_refused() {
    let work = tempfile::tempdir().unwrap();
    let file = work.path().join("token");
    write_token(&file, "short", 0o600);
    assert!(refusal(&file).await.contains("fewer than 8 characters"));
    write_token(&file, "has \"quote\" inside", 0o600);
    assert!(
        refusal(&file)
            .await
            .contains("characters a token never has")
    );
}
