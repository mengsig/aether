//! Fixtures shared by the host service tests: a daemon, real git
//! repositories and fake `git` programs.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use crate::Daemon;

pub(super) async fn daemon() -> (Daemon, tempfile::TempDir) {
    let config_dir = tempfile::tempdir().expect("config dir");
    let store: Arc<dyn aether_store::Store> =
        Arc::new(aether_store::SqliteStore::open_in_memory().expect("store"));
    let models = Arc::new(
        aether_model::Models::load(Some(config_dir.path().to_path_buf())).expect("models"),
    );
    let runtime = aether_runtime::Runtime::start_with_engines(
        aether_runtime::RuntimeConfig {
            holder: "test".into(),
            default_model: Some(aether_core::model::ModelRef::new(
                "anthropic",
                "claude-sonnet-5-5",
            )),
            ..Default::default()
        },
        store,
        models.clone(),
        Arc::new(aether_tools::ToolRegistry::with_builtins()),
        None,
        Default::default(),
    )
    .await
    .expect("runtime");
    let daemon = Daemon {
        runtime,
        models,
        instance_id: "inst".into(),
        version: "test".into(),
        push: None,
        remote: Default::default(),
    };
    (daemon, config_dir)
}

/// Run git in `dir` as a person with no configuration would, returning stdout.
pub(super) fn run_git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "protocol.file.allow=always",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8")
}

/// An empty repository on `main` in a new directory below `parent`.
pub(super) fn init_repo(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    std::fs::create_dir_all(&dir).expect("repo dir");
    run_git(&dir, &["init", "-q", "-b", "main"]);
    dir.canonicalize().expect("canonical repo")
}

/// Write `name` and commit it.
pub(super) fn commit_file(dir: &Path, name: &str, contents: &str) {
    std::fs::write(dir.join(name), contents).expect("write");
    run_git(dir, &["add", name]);
    run_git(dir, &["commit", "-q", "-m", name]);
}

/// An executable shell script, standing in for a program. A child writes
/// it: a write handle of ours, inherited by a concurrent fork, makes exec fail.
pub(super) fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::io::Write as _;
    let path = dir.join(name);
    let mut writer = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(&path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("sh runs");
    let mut stdin = writer.stdin.take().expect("piped stdin");
    write!(stdin, "#!/bin/sh\n{body}\n").expect("script");
    drop(stdin);
    assert!(writer.wait().expect("sh ends").success(), "script written");
    path
}

/// Permission bits mean nothing to root, so tests that need a refusal skip.
pub(super) fn running_as_root() -> bool {
    rustix::process::geteuid().is_root()
}
