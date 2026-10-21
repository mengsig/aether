#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;

#[test]
fn fatal_result_records_context_and_success_preserves_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("aether.db");
    record_result(&store, Ok(())).unwrap();
    assert!(crash::read(&store).unwrap().is_none());
    let error = anyhow::anyhow!("disk unavailable").context("daemon stopped");
    let returned = record_result(&store, Err(error)).unwrap_err();
    assert_eq!(format!("{returned:#}"), "daemon stopped: disk unavailable");
    let evidence = crash::read(&store).unwrap().unwrap();
    assert_eq!(evidence.kind, CrashKind::FatalError);
    assert_eq!(evidence.message, "daemon stopped: disk unavailable");
    assert!(
        evidence
            .location
            .as_ref()
            .unwrap()
            .contains("crash/tests.rs:")
    );
    record_result(&store, Ok(())).unwrap();
    assert_eq!(crash::read(&store).unwrap().unwrap(), evidence);
}

#[test]
fn failure_to_persist_preserves_the_original_error() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("aether.db");
    std::fs::create_dir(crash::path_for(&store)).unwrap();
    let error = record_result(&store, Err(anyhow::anyhow!("fatal cause"))).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot persist daemon fatal error")
    );
    assert_eq!(error.root_cause().to_string(), "fatal cause");
}

#[test]
fn panic_hook_records_an_unwind_without_changing_the_parent_test_hook() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("aether.db");
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash::tests::panic_child", "--nocapture"])
        .env("AETHER_CRASH_TEST_STORE", &store)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    assert!(String::from_utf8_lossy(&child.stderr).contains("panic evidence regression"));
    let record = crash::read(&store).unwrap().unwrap();
    assert_eq!(record.kind, CrashKind::Panic);
    assert_eq!(record.message, "panic evidence regression");
    assert!(record.location.unwrap().contains("crash/tests.rs:"));
    assert!(!record.acknowledged);
}

#[test]
fn panic_child() {
    let Some(store) = std::env::var_os("AETHER_CRASH_TEST_STORE") else {
        return;
    };
    let store = PathBuf::from(store);
    install_hook(store.clone());
    install_hook(store.clone());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let joined = runtime.block_on(async { tokio::spawn(async { panic!("task panic") }).await });
    let task_record = crash::read(&store).unwrap().unwrap();
    assert!(
        record_result(
            &store,
            Err(anyhow::Error::new(joined.unwrap_err()).context("join failed"))
        )
        .is_err()
    );
    assert_eq!(crash::read(&store).unwrap().unwrap(), task_record);
    assert!(record_result(&store, Err(anyhow::anyhow!("unrelated fatal error"))).is_err());
    assert_eq!(
        crash::read(&store).unwrap().unwrap().kind,
        CrashKind::FatalError
    );
    let unwind = std::panic::catch_unwind(|| panic!("panic evidence regression"));
    assert!(unwind.is_err());
}
