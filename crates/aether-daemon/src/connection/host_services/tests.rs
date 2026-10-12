//! The five commands as the connection hands them over, and the checks every
//! one of them makes on what a client sent.

use std::path::PathBuf;

use aether_protocol::remote::{PathCompletionKind, PathCompletions};
use aether_protocol::{Command, ErrorCode, ErrorReply, Reply};

use super::test_support::{commit_file, daemon, init_repo};
use super::*;

fn complete(kind: PathCompletionKind, prefix: &str, workspace: Option<PathBuf>) -> Command {
    Command::CompletePath {
        kind,
        prefix: prefix.into(),
        workspace,
        limit: None,
    }
}

async fn refused(command: Command) -> ErrorReply {
    let (daemon, _config) = daemon().await;
    let error = dispatch(&daemon, command).await.expect_err("refused");
    daemon.runtime.shutdown().await;
    error
}

#[tokio::test]
async fn each_command_answers_with_its_own_reply() {
    let (daemon, _config) = daemon().await;
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");
    let prefix = format!("{}/", dir.path().display());

    let replies = [
        dispatch(&daemon, Command::GetAttention).await,
        dispatch(
            &daemon,
            complete(PathCompletionKind::Directory, &prefix, None),
        )
        .await,
        dispatch(&daemon, Command::ListWorkspaces { limit: None }).await,
        dispatch(
            &daemon,
            Command::ValidateWorkspace {
                path: prefix.clone(),
            },
        )
        .await,
        dispatch(&daemon, Command::GetGitSummary { workspace: repo }).await,
    ]
    .map(|reply| reply.expect("reply"));

    assert!(matches!(replies[0], Reply::Attention(_)));
    let Reply::PathCompletions(PathCompletions { entries, .. }) = &replies[1] else {
        panic!("wrong reply: {:?}", replies[1]);
    };
    assert_eq!(entries.len(), 1);
    assert!(matches!(replies[2], Reply::Workspaces(_)));
    assert!(matches!(replies[3], Reply::WorkspaceValidation(_)));
    assert!(matches!(replies[4], Reply::GitSummary(_)));
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_command_that_is_not_a_host_service_is_an_internal_error() {
    let error = refused(Command::Status).await;

    assert_eq!(error.code, ErrorCode::Internal);
}

#[tokio::test]
async fn nul_and_control_characters_are_refused_in_every_path_field() {
    for bad in ["/tm\0p", "/tmp/a\nb", "/tmp/\u{7f}", "~/\u{9b}", "/tmp/\r"] {
        let commands = [
            complete(PathCompletionKind::Directory, bad, None),
            complete(PathCompletionKind::WorkspaceFile, bad, Some("/tmp".into())),
            complete(PathCompletionKind::WorkspaceFile, "", Some(bad.into())),
            Command::ValidateWorkspace { path: bad.into() },
            Command::GetGitSummary {
                workspace: bad.into(),
            },
        ];
        for command in commands {
            let name = command.name();
            let error = refused(command).await;
            assert_eq!(error.code, ErrorCode::InvalidArgument, "{name} {bad:?}");
        }
    }
}

#[tokio::test]
async fn over_long_paths_are_refused_at_the_limit_and_accepted_just_under_it() {
    let long = format!("/{}", "a".repeat(MAX_PATH_BYTES));
    let at_limit = format!("/{}", "a".repeat(MAX_PATH_BYTES - 1));
    assert_eq!(at_limit.len(), MAX_PATH_BYTES);

    for command in [
        complete(PathCompletionKind::Directory, &long, None),
        Command::ValidateWorkspace { path: long.clone() },
        Command::GetGitSummary {
            workspace: long.clone().into(),
        },
    ] {
        assert_eq!(refused(command).await.code, ErrorCode::InvalidArgument);
    }
    let (daemon, _config) = daemon().await;
    let reply = dispatch(
        &daemon,
        complete(PathCompletionKind::Directory, &at_limit, None),
    )
    .await;
    assert!(matches!(reply, Ok(Reply::PathCompletions(_))), "{reply:?}");
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn completion_needs_a_workspace_for_files_and_an_absolute_one() {
    for workspace in [
        None,
        Some(PathBuf::from("relative")),
        Some(PathBuf::from("")),
    ] {
        let error = refused(complete(PathCompletionKind::WorkspaceFile, "", workspace)).await;
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }
}

#[tokio::test]
async fn a_zero_limit_is_refused_and_a_huge_one_is_capped() {
    let error = refused(Command::CompletePath {
        kind: PathCompletionKind::Directory,
        prefix: "/".into(),
        workspace: None,
        limit: Some(0),
    })
    .await;
    assert_eq!(error.code, ErrorCode::InvalidArgument);

    assert_eq!(effective_limit(None, 50, 200).unwrap(), 50);
    assert_eq!(effective_limit(Some(7), 50, 200).unwrap(), 7);
    assert_eq!(effective_limit(Some(u32::MAX), 50, 200).unwrap(), 200);
}

#[test]
fn text_checks_name_the_field_that_failed() {
    let error = check_text("prefix", "a\0b").expect_err("NUL");
    assert!(error.message.contains("`prefix`"), "{}", error.message);
    assert!(check_text("prefix", "é/ü/日本語 with spaces").is_ok());
    assert!(check_absolute("workspace", std::path::Path::new("/a/b")).is_ok());
    assert!(check_absolute("workspace", std::path::Path::new("a/b")).is_err());
}

#[test]
fn a_path_that_is_not_utf8_is_refused() {
    use std::os::unix::ffi::OsStrExt as _;
    let path = std::path::Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff"));

    let error = check_absolute("workspace", path).expect_err("not UTF-8");

    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn work_that_outlives_its_time_is_reported_and_keeps_its_slot() {
    static SLOTS: Semaphore = Semaphore::const_new(1);
    let (release, hold) = std::sync::mpsc::channel::<()>();

    let slow = run_bounded(&SLOTS, Duration::from_millis(50), "test", move || {
        let _ = hold.recv();
    })
    .await;

    assert_eq!(slow.expect_err("timed out").code, ErrorCode::Unavailable);
    let busy = run_bounded(&SLOTS, Duration::from_millis(50), "test", || ()).await;
    assert_eq!(busy.expect_err("no slot").code, ErrorCode::Unavailable);
    assert_eq!(
        SLOTS.available_permits(),
        0,
        "the stuck job still holds its slot"
    );
    drop(release);
    for _ in 0..200 {
        if SLOTS.available_permits() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        SLOTS.available_permits(),
        1,
        "the slot returns with the job"
    );
    assert_eq!(
        run_bounded(&SLOTS, Duration::from_secs(5), "test", || 7)
            .await
            .unwrap(),
        7
    );
}
