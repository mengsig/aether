#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::path::Path;
use std::time::Duration;

use aether_protocol::PROTOCOL_VERSION;
use support::{RawConn, TestDaemon};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
#[tokio::test]
async fn retargeting_a_socket_parent_alias_preserves_foreign_entries_on_shutdown() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = tempfile::tempdir().unwrap();
    let shared = root.path().join("shared");
    let original = root.path().join("original");
    let foreign = root.path().join("foreign");
    for dir in [&shared, &original, &foreign] {
        std::fs::create_dir(dir).unwrap();
    }
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
    let alias = shared.join("link");
    symlink(&original, &alias).unwrap();
    let socket = original.join("aether.sock");
    let pid = aether_protocol::paths::pid_path_for(&socket);
    let foreign_socket = foreign.join("aether.sock");
    let foreign_pid = aether_protocol::paths::pid_path_for(&foreign_socket);
    let listener = std::os::unix::net::UnixListener::bind(&foreign_socket).unwrap();
    std::fs::write(&foreign_pid, b"foreign pid evidence").unwrap();
    let config = aether_daemon::DaemonConfig {
        socket: alias.join("aether.sock"),
        store_path: root.path().join("aether.db"),
        config_dir: Some(root.path().join("config")),
        workspace_root: Some(root.path().to_path_buf()),
        max_concurrent_turns: None,
        yolo: false,
        tcp: None,
        allow_remote: false,
    };
    let shutdown = CancellationToken::new();
    let serving = tokio::spawn(aether_daemon::run(config, shutdown.clone()));
    let answered = serves(&socket, &serving).await;
    std::fs::remove_file(&alias).unwrap();
    symlink(&foreign, &alias).unwrap();
    shutdown.cancel();
    let ended = tokio::time::timeout(Duration::from_secs(30), serving)
        .await
        .unwrap()
        .unwrap();
    assert!(answered, "daemon never answered: {ended:?}");
    ended.unwrap();
    assert!(
        foreign_socket.exists(),
        "foreign socket must survive cleanup"
    );
    assert_eq!(std::fs::read(foreign_pid).unwrap(), b"foreign pid evidence");
    std::os::unix::net::UnixStream::connect(&foreign_socket).unwrap();
    assert!(
        !socket.exists(),
        "the daemon's pinned socket must be removed"
    );
    assert!(!pid.exists(), "the daemon's pinned PID must be removed");
    drop(listener);
}

#[cfg(unix)]
#[tokio::test]
async fn a_short_socket_alias_serves_a_target_longer_than_a_unix_address() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir_in("/tmp").unwrap();
    let original = root.path().join("a".repeat(200));
    std::fs::create_dir(&original).unwrap();
    let alias = root.path().join("link");
    symlink(&original, &alias).unwrap();
    let socket = alias.join("s");
    let target = original.join("s");
    assert!(std::os::unix::net::SocketAddr::from_pathname(&socket).is_ok());
    assert!(std::os::unix::net::SocketAddr::from_pathname(&target).is_err());
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    let config = aether_daemon::DaemonConfig {
        socket: socket.clone(),
        store_path: root.path().join("aether.db"),
        config_dir: Some(root.path().join("config")),
        workspace_root: Some(root.path().to_path_buf()),
        max_concurrent_turns: None,
        yolo: false,
        tcp: None,
        allow_remote: false,
    };
    let shutdown = CancellationToken::new();
    let serving = tokio::spawn(aether_daemon::run(config, shutdown.clone()));
    let answered = serves(&socket, &serving).await;
    // Cleanup must still use the canonical target after this public address disappears.
    std::fs::remove_file(alias).unwrap();
    shutdown.cancel();
    let ended = tokio::time::timeout(Duration::from_secs(30), serving)
        .await
        .unwrap()
        .unwrap();
    assert!(answered, "valid configured address must serve: {ended:?}");
    ended.unwrap();
    assert!(!target.exists());
    assert!(!aether_protocol::paths::pid_path_for(&target).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_preserves_replacements_for_published_socket_and_pid() {
    use std::os::unix::fs::symlink;

    for replace_with_symlink in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("aether.sock");
        let pid = aether_protocol::paths::pid_path_for(&socket);
        let saved_socket = root.path().join("saved.sock");
        let saved_pid = root.path().join("saved.pid");
        let config = aether_daemon::DaemonConfig {
            socket: socket.clone(),
            store_path: root.path().join("aether.db"),
            config_dir: Some(root.path().join("config")),
            workspace_root: Some(root.path().to_path_buf()),
            max_concurrent_turns: None,
            yolo: false,
            tcp: None,
            allow_remote: false,
        };
        let shutdown = CancellationToken::new();
        let serving = tokio::spawn(aether_daemon::run(config, shutdown.clone()));
        let answered = serves(&socket, &serving).await;
        assert!(answered, "daemon must serve before replacing its entries");
        std::fs::rename(&socket, &saved_socket).unwrap();
        std::fs::rename(&pid, &saved_pid).unwrap();
        let listener = if replace_with_symlink {
            symlink(&saved_socket, &socket).unwrap();
            symlink(&saved_pid, &pid).unwrap();
            None
        } else {
            std::fs::write(&pid, b"replacement pid evidence").unwrap();
            Some(std::os::unix::net::UnixListener::bind(&socket).unwrap())
        };
        shutdown.cancel();
        let error = tokio::time::timeout(Duration::from_secs(30), serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("was replaced"), "{error:#}");
        if replace_with_symlink {
            assert_eq!(std::fs::read_link(&socket).unwrap(), saved_socket);
            assert_eq!(std::fs::read_link(&pid).unwrap(), saved_pid);
        } else {
            std::os::unix::net::UnixStream::connect(&socket).unwrap();
            assert_eq!(std::fs::read(&pid).unwrap(), b"replacement pid evidence");
        }
        drop(listener);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn rejecting_a_pid_file_preserves_its_entry_without_starting_the_runtime() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("aether.sock");
    let pid = aether_protocol::paths::pid_path_for(&socket);
    let victim = root.path().join("keep");
    std::fs::write(&victim, b"keep bytes").unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::hard_link(&victim, &pid).unwrap();
    let config = aether_daemon::DaemonConfig {
        socket: socket.clone(),
        store_path: root.path().join("aether.db"),
        config_dir: Some(root.path().join("config")),
        workspace_root: Some(root.path().join("workspace")),
        max_concurrent_turns: None,
        yolo: false,
        tcp: None,
        allow_remote: false,
    };
    let error = tokio::time::timeout(
        Duration::from_secs(30),
        aether_daemon::run(config.clone(), CancellationToken::new()),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(
        error.to_string().contains("multiple hard links"),
        "{error:#}"
    );
    assert!(
        !socket.exists(),
        "preflight must reject before publishing a socket"
    );
    assert!(
        !config.store_path.exists(),
        "preflight must precede store recovery"
    );
    assert!(pid.exists(), "the rejected PID entry must be preserved");
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep bytes");
    assert_eq!(
        std::fs::metadata(&victim).unwrap().permissions().mode() & 0o777,
        0o644
    );
    std::fs::remove_file(pid).unwrap();
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    tokio::time::timeout(
        Duration::from_secs(30),
        aether_daemon::run(config, shutdown),
    )
    .await
    .unwrap()
    .unwrap();
}

/// Whether `daemon` comes up and answers on `socket`. Generous, because a
/// daemon opens its store, migrates and recovers before it binds, and the
/// machine running this may be busy doing that for several others.
async fn serves(socket: &Path, daemon: &JoinHandle<anyhow::Result<()>>) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        if daemon.is_finished() {
            return false;
        }
        if let Ok(mut conn) = RawConn::connect(socket).await
            && conn.hello(PROTOCOL_VERSION).await.is_ok()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

/// What a client's auto-start does when it cannot reach the daemon: start
/// another on the same store and socket. It must refuse, not take the socket.
#[tokio::test]
async fn a_second_daemon_for_a_served_store_refuses_and_the_first_keeps_serving() {
    let daemon = TestDaemon::start().await;
    let root = daemon.socket.parent().expect("socket dir").to_path_buf();
    let second = aether_daemon::DaemonConfig {
        socket: daemon.socket.clone(),
        store_path: daemon.store_path(),
        config_dir: Some(root.join("config")),
        workspace_root: Some(root.join("workspace")),
        max_concurrent_turns: None,
        yolo: false,
        tcp: None,
        allow_remote: false,
    };

    let started = aether_daemon::run(second, CancellationToken::new());
    let refused = tokio::time::timeout(Duration::from_secs(10), started)
        .await
        .expect("the second daemon must refuse at once, not serve")
        .expect_err("the store already has a daemon");
    assert_eq!(
        refused.to_string(),
        format!(
            "another aether daemon is already running for {}",
            daemon.store_path().display()
        )
    );

    let mut conn = RawConn::connect(&daemon.socket)
        .await
        .expect("the first daemon's socket is still in place");
    conn.hello(PROTOCOL_VERSION)
        .await
        .expect("and the first daemon still answers on it");
    daemon.stop().await;
}

/// A daemon for another store asked to serve the same socket: the store lock
/// cannot stop it, so the socket's own lock must, before it probes anything.
#[tokio::test]
async fn a_daemon_for_another_store_cannot_take_a_held_socket() {
    let daemon = TestDaemon::start().await;
    let root = daemon.socket.parent().expect("socket dir").to_path_buf();
    let other_store = root.join("other").join("aether.db");
    let second = aether_daemon::DaemonConfig {
        socket: daemon.socket.clone(),
        store_path: other_store.clone(),
        config_dir: Some(root.join("config")),
        workspace_root: Some(root.join("workspace")),
        max_concurrent_turns: None,
        yolo: false,
        tcp: None,
        allow_remote: false,
    };

    let started = aether_daemon::run(second, CancellationToken::new());
    let refused = tokio::time::timeout(Duration::from_secs(10), started)
        .await
        .expect("the second daemon must refuse at once, not serve")
        .expect_err("the socket already has a daemon");
    assert_eq!(
        refused.to_string(),
        format!(
            "another aether daemon is already using the socket {}",
            daemon.socket.display()
        )
    );
    assert!(
        !other_store.exists(),
        "the refused daemon must not have created its store"
    );

    let mut conn = RawConn::connect(&daemon.socket)
        .await
        .expect("the first daemon's socket is still in place");
    conn.hello(PROTOCOL_VERSION)
        .await
        .expect("and the first daemon still answers on it");
    daemon.stop().await;
}

/// A daemon killed outright leaves its socket file behind. The next start
/// must see nothing answers there and replace it, or no restart succeeds.
#[tokio::test]
async fn a_socket_left_by_a_dead_daemon_is_replaced() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let socket = root.join("aetherd.sock");
    drop(std::os::unix::net::UnixListener::bind(&socket).expect("the dead daemon's socket"));
    assert!(socket.exists(), "a dropped listener leaves its file behind");
    let config = aether_daemon::DaemonConfig {
        socket: socket.clone(),
        store_path: root.join("aether.db"),
        config_dir: Some(root.join("config")),
        workspace_root: Some(root.join("workspace")),
        max_concurrent_turns: None,
        yolo: false,
        tcp: None,
        allow_remote: false,
    };
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");

    let shutdown = CancellationToken::new();
    let serving = tokio::spawn(aether_daemon::run(config, shutdown.clone()));
    let answered = serves(&socket, &serving).await;
    shutdown.cancel();
    let ended = tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .expect("the daemon stops on shutdown")
        .expect("the daemon task");
    assert!(answered, "the restarted daemon never answered: {ended:?}");
    ended.expect("the restarted daemon served and shut down cleanly");
}

/// Two stores misconfigured onto one socket, started at the same instant.
/// The store lock cannot separate them: each would probe the socket, find
/// nothing answering and bind, and the second could unlink what the first
/// had just bound. Exactly one must serve, and the other must say so
/// without touching the socket.
#[tokio::test]
async fn two_daemons_for_different_stores_cannot_both_take_one_socket() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let socket = root.join("aetherd.sock");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let store_of = |name: &str| root.join(name).join("aether.db");
    let config = |name: &str| aether_daemon::DaemonConfig {
        socket: socket.clone(),
        store_path: store_of(name),
        config_dir: Some(root.join("config")),
        workspace_root: Some(root.join("workspace")),
        max_concurrent_turns: None,
        yolo: false,
        tcp: None,
        allow_remote: false,
    };

    let shutdown = CancellationToken::new();
    let mut one = tokio::spawn(aether_daemon::run(config("one"), shutdown.clone()));
    let mut two = tokio::spawn(aether_daemon::run(config("two"), shutdown.clone()));
    // Only the loser can finish while both are wanted, so the first task to
    // end is the one that lost the socket. Under a timeout, because two
    // daemons that both serve would leave this waiting for ever.
    let first_to_end = async move {
        tokio::select! {
            ended = &mut one => (ended, two),
            ended = &mut two => (ended, one),
        }
    };
    let (ended, winner) = tokio::time::timeout(Duration::from_secs(30), first_to_end)
        .await
        .expect("one of the two must refuse the socket rather than both serving it");
    let refused = ended
        .expect("the refused daemon's task")
        .expect_err("one of the two must refuse the socket");
    assert_eq!(
        refused.to_string(),
        format!(
            "another aether daemon is already using the socket {}",
            socket.display()
        )
    );
    let answered = serves(&socket, &winner).await;
    shutdown.cancel();
    let ended = tokio::time::timeout(Duration::from_secs(10), winner)
        .await
        .expect("the winner stops on shutdown")
        .expect("the winner's task");
    assert!(answered, "no daemon serves the socket either: {ended:?}");
    ended.expect("the winner served and shut down cleanly");
    // Checked once the winner has served: only then is its own store certain
    // to exist, and the refused one is still nowhere.
    assert_eq!(
        [store_of("one"), store_of("two")]
            .iter()
            .filter(|store| store.exists())
            .count(),
        1,
        "the refused daemon must not have opened its store"
    );
}

/// Something is listening on the socket that does not hold its lock: a
/// daemon from an older build, or another program. The probe still catches
/// it, and it keeps its socket.
#[tokio::test]
async fn a_socket_a_foreign_listener_holds_is_left_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let socket = root.join("aetherd.sock");
    let foreign = std::os::unix::net::UnixListener::bind(&socket).expect("the foreign listener");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let config = aether_daemon::DaemonConfig {
        socket: socket.clone(),
        store_path: root.join("aether.db"),
        config_dir: Some(root.join("config")),
        workspace_root: Some(root.join("workspace")),
        max_concurrent_turns: None,
        yolo: false,
        tcp: None,
        allow_remote: false,
    };

    let started = aether_daemon::run(config, CancellationToken::new());
    let refused = tokio::time::timeout(Duration::from_secs(10), started)
        .await
        .expect("the daemon must refuse at once, not serve")
        .expect_err("the socket is taken");
    assert_eq!(
        refused.to_string(),
        format!(
            "an aether daemon is already listening on {}",
            socket.display()
        )
    );
    std::os::unix::net::UnixStream::connect(&socket).expect("the foreign listener is still bound");
    drop(foreign);
}

#[tokio::test]
async fn starting_a_daemon_preserves_an_existing_socket_parent() {
    use std::os::unix::fs::PermissionsExt;

    for home_mode in [0o750, 0o755] {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(home_mode)).unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir(&data).unwrap();
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir(data.join("logs")).unwrap();
        let socket = dir.path().join("aetherd.sock");
        let config = aether_daemon::DaemonConfig {
            socket: socket.clone(),
            store_path: data.join("aether.db"),
            config_dir: Some(dir.path().join("config")),
            workspace_root: Some(dir.path().to_path_buf()),
            max_concurrent_turns: None,
            yolo: false,
            tcp: None,
            allow_remote: false,
        };
        let shutdown = CancellationToken::new();
        let serving = tokio::spawn(aether_daemon::run(config, shutdown.clone()));
        let answered = serves(&socket, &serving).await;
        let parent_mode = std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
        let socket_mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
        let data_mode = std::fs::metadata(&data).unwrap().permissions().mode() & 0o777;
        let file_modes: Vec<_> = ["aether.db", "aether.db-wal", "aether.db-shm"]
            .iter()
            .map(|name| {
                std::fs::metadata(data.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777
            })
            .collect();
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(answered);
        assert_eq!(parent_mode, home_mode);
        assert_eq!(data_mode, 0o755);
        assert_eq!(socket_mode, 0o600);
        assert_eq!(file_modes, [0o600; 3]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn simultaneous_starts_leave_one_daemon_serving() {
    let root = tempfile::tempdir().unwrap();
    let config = aether_daemon::DaemonConfig {
        socket: root.path().join("aether.sock"),
        store_path: root.path().join("aether.db"),
        config_dir: Some(root.path().join("config")),
        workspace_root: Some(root.path().to_path_buf()),
        max_concurrent_turns: None,
        yolo: false,
        tcp: None,
        allow_remote: false,
    };
    let shutdown = CancellationToken::new();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
    let start = |config: aether_daemon::DaemonConfig| {
        let barrier = barrier.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            aether_daemon::run(config, shutdown).await
        })
    };
    let mut first = start(config.clone());
    let mut second = start(config.clone());
    barrier.wait().await;
    let (refused, serving) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::select! {
            result = &mut first => (result, second),
            result = &mut second => (result, first),
        }
    })
    .await
    .expect("one concurrent start must refuse");
    let error = refused.unwrap().unwrap_err();
    assert!(
        error.to_string().contains("another aether daemon"),
        "{error:#}"
    );
    let answered = serves(&config.socket, &serving).await;
    shutdown.cancel();
    let ended = tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .unwrap()
        .unwrap();
    assert!(answered, "the winning daemon must serve: {ended:?}");
    ended.unwrap();
}
