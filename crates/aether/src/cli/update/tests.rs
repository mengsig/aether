//! What `aether update` decides without a network: versions, asset names,
//! checksums, and which binaries it may replace.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use super::daemon::Daemon;
use super::install::{INSTALL_COMMAND, Installation, RECEIPT_FILE};
use super::release::{
    ReleaseSource, VERSION_ASSET, Version, archive_name, checksum_name, published_digest,
    target_for,
};
use super::{Plan, check_line, confirm, not_confirmed, plan, restart_notice, stop_first};

fn version(text: &str) -> Version {
    Version::parse(text).expect(text)
}

/// A directory holding a stand-in `aether`, optionally with the receipt.
fn installed_in(directory: &Path, receipt: Option<&str>) -> PathBuf {
    std::fs::create_dir_all(directory).expect("install directory");
    let binary = directory.join("aether");
    std::fs::write(&binary, b"old build").expect("binary");
    if let Some(receipt) = receipt {
        std::fs::write(directory.join(RECEIPT_FILE), receipt).expect("receipt");
    }
    binary
}

const RECEIPT: &str = "# Written by Aether's install.sh.\ninstaller=install.sh\n";

#[test]
fn versions_compare_by_number_not_by_text() {
    assert_eq!(version("v1.2.3"), version("1.2.3"));
    assert!(version("1.10.0") > version("1.9.9"));
    assert!(version("2.0.0") > version("1.99.99"));
    assert!(version("0.1.1") < version("0.1.2"));
    assert!(version("1.0.0") > version("0.99.99"));
    assert_eq!(version("v10.20.30").tag(), "v10.20.30");
    assert_eq!(version("v10.20.30").to_string(), "10.20.30");
}

#[test]
fn anything_but_three_plain_numbers_is_not_a_version() {
    for bad in [
        "",
        "v",
        "1.2",
        "1.2.3.4",
        "1.2.x",
        "+1.2.3",
        "1.-2.3",
        "01.2.3",
        "1.2.3-rc1",
        " 1.2.3",
        "v1.2.3\n",
        "vv1.2.3",
        "1..3",
        "99999999999999999999.0.0",
        "v1.2.3/../..",
    ] {
        assert!(Version::parse(bad).is_err(), "`{bad}` must be refused");
    }
}

#[test]
fn only_a_pin_goes_back_to_an_older_release() {
    let running = version("1.2.0");
    assert_eq!(plan(running, version("1.3.0"), false), Plan::Upgrade);
    assert_eq!(plan(running, version("1.3.0"), true), Plan::Upgrade);
    assert_eq!(plan(running, version("1.2.0"), false), Plan::UpToDate);
    assert_eq!(plan(running, version("1.2.0"), true), Plan::UpToDate);
    assert_eq!(plan(running, version("1.1.9"), false), Plan::AheadOfLatest);
    assert_eq!(plan(running, version("1.1.9"), true), Plan::Downgrade);
}

#[test]
fn each_released_platform_has_its_own_asset() {
    for (os, arch, archive) in [
        ("linux", "x86_64", "aether-x86_64-unknown-linux-musl.tar.gz"),
        (
            "linux",
            "aarch64",
            "aether-aarch64-unknown-linux-musl.tar.gz",
        ),
        ("macos", "aarch64", "aether-aarch64-apple-darwin.tar.gz"),
        ("macos", "x86_64", "aether-x86_64-apple-darwin.tar.gz"),
    ] {
        let target = target_for(os, arch).expect("released target");
        assert_eq!(archive_name(target), archive);
        assert_eq!(checksum_name(archive), format!("{archive}.sha256"));
    }
    for (os, arch) in [
        ("windows", "x86_64"),
        ("linux", "riscv64"),
        ("freebsd", "x86_64"),
    ] {
        let refusal = target_for(os, arch).expect_err("no release").to_string();
        assert!(refusal.contains(os) && refusal.contains(arch), "{refusal}");
    }
}

#[test]
fn asset_urls_follow_the_github_release_layout() {
    let github = ReleaseSource::new(super::release::DEFAULT_BASE_URL).expect("default base");
    assert!(github.https_only());
    assert_eq!(
        github.latest_url(VERSION_ASSET),
        "https://github.com/mengsig/aether/releases/latest/download/VERSION"
    );
    assert_eq!(
        github.tagged_url(version("1.2.3"), "aether-aarch64-apple-darwin.tar.gz"),
        "https://github.com/mengsig/aether/releases/download/v1.2.3/aether-aarch64-apple-darwin.tar.gz"
    );
    let mirror = ReleaseSource::new("http://127.0.0.1:8080/mirror/").expect("loopback");
    assert!(!mirror.https_only());
    assert_eq!(
        mirror.latest_url("a"),
        "http://127.0.0.1:8080/mirror/latest/download/a"
    );
}

#[test]
fn a_download_base_is_https_or_this_machine() {
    for good in [
        "https://mirror.example.com/aether",
        "http://127.0.0.1:9000",
        "http://localhost:9000/releases",
        "http://[::1]:9000",
    ] {
        ReleaseSource::new(good).expect(good);
    }
    for bad in [
        "http://example.com/releases",
        "http://127.0.0.1.example.com",
        "ftp://example.com/releases",
        "https://user:secret@example.com/releases",
        "https://example.com/releases?token=1",
        "https://example.com/releases#latest",
        "not a url",
        "",
    ] {
        assert!(ReleaseSource::new(bad).is_err(), "`{bad}` must be refused");
    }
}

#[test]
fn a_published_checksum_must_be_a_digest_for_this_archive() {
    let digest = "0123456789abcdef".repeat(4);
    let archive = "aether-x86_64-unknown-linux-musl.tar.gz";
    for file in [
        format!("{digest}  {archive}\n"),
        format!("{digest} *{archive}\n"),
        format!("{}  {archive}\n", digest.to_uppercase()),
        format!("{digest}\n"),
    ] {
        assert_eq!(published_digest(&file, archive).expect(&file), digest);
    }
    for file in [
        String::new(),
        format!("{}  {archive}\n", &digest[..63]),
        format!("{}  {archive}\n", digest.replace('0', "g")),
        format!("{digest}  aether-aarch64-apple-darwin.tar.gz\n"),
        "<html>Not Found</html>".to_owned(),
    ] {
        assert!(
            published_digest(&file, archive).is_err(),
            "`{file}` must be refused"
        );
    }
}

#[test]
fn a_binary_with_the_installers_receipt_in_a_writable_directory_is_replaceable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let binary = installed_in(&dir.path().join("bin"), Some(RECEIPT));
    let installation = Installation::at(&binary).expect("replaceable");
    assert_eq!(
        installation.binary(),
        std::fs::canonicalize(&binary).expect("canonical")
    );
}

#[test]
fn a_symlink_is_judged_by_the_directory_the_binary_really_lives_in() {
    let dir = tempfile::tempdir().expect("tempdir");
    let real = installed_in(&dir.path().join("opt"), None);
    let linked_from = dir.path().join("bin");
    std::fs::create_dir(&linked_from).expect("bin");
    std::fs::write(linked_from.join(RECEIPT_FILE), RECEIPT).expect("receipt beside the link");
    let link = linked_from.join("aether");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    let refusal = Installation::at(&link).expect_err("no receipt beside the real file");
    assert!(refusal.to_string().contains(INSTALL_COMMAND), "{refusal}");
}

#[test]
fn a_binary_without_a_valid_receipt_is_refused_with_the_installer_command() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, receipt) in [
        ("none", None),
        ("empty", Some("")),
        ("other", Some("installer=homebrew\n")),
    ] {
        let binary = installed_in(&dir.path().join(name), receipt);
        let refusal = Installation::at(&binary).expect_err("not ours").to_string();
        assert!(refusal.contains(INSTALL_COMMAND), "{refusal}");
        assert!(refusal.contains("was not installed by"), "{refusal}");
        assert_eq!(std::fs::read(&binary).expect("binary"), b"old build");
    }
}

#[test]
fn a_cargo_installed_binary_is_refused_with_the_cargo_command() {
    let dir = tempfile::tempdir().expect("tempdir");
    let binary = installed_in(&dir.path().join("bin"), None);
    std::fs::write(
        dir.path().join(".crates.toml"),
        "[v1]\n\"aether 0.1.1 (path+file:///src/aether/crates/aether)\" = [\"aether\"]\n",
    )
    .expect("cargo record");
    let refusal = Installation::at(&binary).expect_err("cargo's").to_string();
    assert!(refusal.contains("installed by cargo"), "{refusal}");
    assert!(
        refusal.contains("cargo install --git https://github.com/mengsig/aether --locked aether"),
        "{refusal}"
    );
    assert!(!refusal.contains(INSTALL_COMMAND), "{refusal}");
}

#[test]
fn cargos_record_wins_over_a_receipt_in_the_same_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let binary = installed_in(&dir.path().join("bin"), Some(RECEIPT));
    std::fs::write(
        dir.path().join(".crates.toml"),
        "[v1]\n\"aether 0.1.1 (git+https://github.com/mengsig/aether#0f32d39e)\" = [\"aether\"]\n",
    )
    .expect("cargo record");
    let refusal = Installation::at(&binary).expect_err("cargo's").to_string();
    assert!(refusal.contains("installed by cargo"), "{refusal}");
    assert!(!refusal.contains(INSTALL_COMMAND), "{refusal}");
    assert_eq!(std::fs::read(&binary).expect("binary"), b"old build");
}

#[test]
fn a_directory_that_cannot_be_written_is_refused_before_anything_is_tried() {
    if nix::unistd::geteuid().is_root() {
        eprintln!("root may write anywhere: nothing to refuse");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let directory = dir.path().join("bin");
    let binary = installed_in(&directory, Some(RECEIPT));
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o555))
        .expect("read-only");
    let refusal = Installation::at(&binary)
        .map(|_| ())
        .map_err(|e| e.to_string());
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))
        .expect("writable again");
    let refusal = refusal.expect_err("read-only directory");
    assert!(refusal.contains("is not writable by you"), "{refusal}");
    assert!(refusal.contains(INSTALL_COMMAND), "{refusal}");
}

#[test]
fn replacing_renames_over_the_binary_and_leaves_the_running_file_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let binary = installed_in(&dir.path().join("bin"), Some(RECEIPT));
    // A hard link stands in for a process still running the old file.
    let running = dir.path().join("running");
    std::fs::hard_link(&binary, &running).expect("hard link");
    let installation = Installation::at(&binary).expect("replaceable");

    let elsewhere = dir.path().join("aether");
    std::fs::write(&elsewhere, b"new build").expect("stray file");
    let refusal = installation
        .replace_with(&elsewhere)
        .expect_err("not staged");
    assert!(refusal.to_string().contains("is not staged beside"));
    assert_eq!(std::fs::read(&binary).expect("binary"), b"old build");

    let staging = installation.staging().expect("staging");
    let staged = staging.path().join("aether");
    std::fs::write(&staged, b"new build").expect("staged");
    installation.replace_with(&staged).expect("replaced");
    assert_eq!(std::fs::read(&binary).expect("binary"), b"new build");
    assert_eq!(std::fs::read(&running).expect("running"), b"old build");
    staging.close().expect("staging removed");
    let mut left: Vec<_> = std::fs::read_dir(dir.path().join("bin"))
        .expect("install directory")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    left.sort();
    assert_eq!(left, [RECEIPT_FILE, "aether"]);
}

#[test]
fn the_restart_notice_names_the_commands_for_the_selected_daemon() {
    let (old, new) = (version("1.0.0"), version("1.1.0"));
    let aether = "aether --socket '/tmp/d.sock'";
    let pid_file = Path::new("/tmp/d.pid");
    let stoppable = Daemon::Signallable;
    let running = restart_notice(&stoppable, &stoppable, aether, pid_file, old, new);
    assert!(
        running.contains("`aether --socket '/tmp/d.sock' stop`"),
        "{running}"
    );
    assert!(running.contains("still aether 1.0.0"), "{running}");
    assert!(running.contains("start 1.1.0"), "{running}");
    assert!(!running.contains("/tmp/d.pid"), "{running}");
    for before in [Daemon::Absent, Daemon::Signallable] {
        let idle = restart_notice(&before, &Daemon::Absent, "aether", pid_file, old, new);
        assert_eq!(
            idle,
            "No daemon is running: the next `aether` starts 1.1.0; on a headless host (no TUI), run `aether daemon`."
        );
    }
    let started = restart_notice(&Daemon::Absent, &stoppable, aether, pid_file, old, new);
    assert_eq!(
        started, running,
        "a daemon that can be signalled is told the same either way"
    );
    assert!(running.contains(&format!("{aether} stop && {aether} daemon")));
    assert!(!running.contains('\n'));
}

#[test]
fn a_daemon_that_cannot_be_signalled_is_never_told_that_stop_is_enough() {
    let (old, new) = (version("1.0.0"), version("1.1.0"));
    let ask_only = Daemon::AskOnly("no pidfd".to_owned());
    let pid_file = Path::new("/run/aether/aetherd.pid");
    let late = restart_notice(&Daemon::Absent, &ask_only, "aether", pid_file, old, new);
    assert!(late.contains("started during the update"), "{late}");
    let unchecked = restart_notice(
        &Daemon::Signallable,
        &ask_only,
        "aether",
        pid_file,
        old,
        new,
    );
    assert!(
        unchecked.contains("could not be checked again after the update (no pidfd)"),
        "{unchecked}"
    );
    assert!(
        !unchecked.contains("started during the update"),
        "a daemon found before the update did not start during it: {unchecked}"
    );
    for notice in [&late, &unchecked] {
        assert!(notice.contains("is not aether 1.1.0"), "{notice}");
        assert!(
            notice.contains("run `aether stop`, then `aether`."),
            "{notice}"
        );
        assert!(notice.contains("aether stop && aether daemon"), "{notice}");
        assert!(notice.contains("If `stop` cannot signal it"), "{notice}");
        assert!(notice.contains("/run/aether/aetherd.pid"), "{notice}");
        assert!(!notice.contains('\n'), "{notice}");
    }
}

#[test]
fn stopping_first_names_the_socket_the_reason_and_the_same_update_again() {
    let socket = Path::new("/tmp/d.sock");
    let aether = "aether --socket '/tmp/d.sock'";
    let reason = "this platform cannot pin the socket peer";
    let latest = stop_first(socket, aether, None, reason);
    assert!(latest.starts_with("aether was not updated: the daemon on /tmp/d.sock"));
    assert!(latest.contains(reason), "{latest}");
    assert!(
        latest.contains(
            "Run `aether --socket '/tmp/d.sock' stop`, then `aether --socket '/tmp/d.sock' update` again"
        ),
        "{latest}"
    );
    let pinned = stop_first(socket, "aether", Some(version("1.2.3")), reason);
    assert!(
        pinned.contains("Run `aether stop`, then `aether update --version v1.2.3` again"),
        "{pinned}"
    );
    assert!(!latest.contains('\n') && !pinned.contains('\n'));
}

#[test]
fn check_names_the_command_that_really_installs_the_newer_release() {
    let (running, newer) = (version("1.0.0"), version("1.1.0"));
    assert_eq!(
        check_line(running, newer, Some("aether update")),
        "aether 1.1.0 is available; this is 1.0.0. Run `aether update` to install it."
    );
    let refused = check_line(running, newer, Some("cargo install aether"));
    assert_eq!(
        refused,
        "aether 1.1.0 is available; this is 1.0.0. `aether update` leaves this binary alone; \
         install it with `cargo install aether`."
    );
    assert_eq!(
        check_line(running, running, None),
        "aether 1.0.0 is up to date."
    );
    assert_eq!(
        check_line(newer, running, None),
        "aether 1.1.0 is newer than the latest release, 1.0.0."
    );
}

#[test]
fn only_a_yes_confirms_a_one_way_migration_and_no_answer_is_a_no() {
    for (answer, agreed) in [
        ("y\n", true),
        ("YES\n", true),
        (" yes \n", true),
        ("n\n", false),
        ("\n", false),
        ("", false),
        ("yep\n", false),
    ] {
        let mut prompt = Vec::new();
        let said = confirm(&mut answer.as_bytes(), &mut prompt, version("1.2.0")).expect("asked");
        assert_eq!(said, agreed, "{answer:?}");
        let prompt = String::from_utf8(prompt).expect("utf-8");
        assert!(
            prompt.contains("Updating to aether 1.2.0 migrates the store one way"),
            "{prompt}"
        );
        assert!(prompt.ends_with("[y/N] "), "{prompt}");
    }
}

#[test]
fn an_unconfirmed_migration_names_the_same_update_again_and_yes() {
    let latest = not_confirmed("aether", version("1.2.0"), None);
    assert!(latest.contains("aether was not updated"), "{latest}");
    assert!(
        latest.contains("run `aether update` and answer y, or add --yes"),
        "{latest}"
    );
    let pinned = not_confirmed(
        "aether --socket /s",
        version("1.2.0"),
        Some(version("1.2.0")),
    );
    assert!(
        pinned.contains("`aether --socket /s update --version v1.2.0`"),
        "{pinned}"
    );
}
