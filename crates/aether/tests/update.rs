//! `aether update` as a process, against a fake release on a local server.
//! Nothing here reaches github.com: `AETHER_INSTALL_BASE_URL` names the server.
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

const RUNNING: &str = env!("CARGO_PKG_VERSION");
const RECEIPT: &str = ".aether-receipt";
const INSTALL_COMMAND: &str =
    "curl -fsSL https://raw.githubusercontent.com/mengsig/aether/main/install.sh | sh";
const CARGO_COMMAND: &str = "cargo install --git https://github.com/mengsig/aether --locked aether";
const CARGO_RECORD: &str =
    "[v1]\n\"aether 0.1.1 (path+file:///src/aether/crates/aether)\" = [\"aether\"]\n";
const UPDATE_AVAILABLE: i32 = 10;

/// The archive this machine downloads, spelled out apart from the code under test.
fn archive_name() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "aether-x86_64-unknown-linux-musl.tar.gz",
        ("linux", "aarch64") => "aether-aarch64-unknown-linux-musl.tar.gz",
        ("macos", "aarch64") => "aether-aarch64-apple-darwin.tar.gz",
        ("macos", "x86_64") => "aether-x86_64-apple-darwin.tar.gz",
        other => panic!("no release is built for {other:?}"),
    }
}

type Routes = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// Serves the routes it was given, 404 for the rest, and records each path.
struct Server {
    base: String,
    routes: Routes,
    requested: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("address"));
        let routes = Routes::default();
        let requested = Arc::new(Mutex::new(Vec::new()));
        let (serving, seen) = (routes.clone(), requested.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                answer(stream.expect("connection"), &serving, &seen);
            }
        });
        Self {
            base,
            routes,
            requested,
        }
    }

    fn serve(&self, path: &str, body: impl Into<Vec<u8>>) {
        let replaced = self
            .routes
            .lock()
            .expect("routes")
            .insert(path.to_owned(), body.into());
        assert!(replaced.is_none(), "{path} is already served");
    }

    fn requested(&self) -> Vec<String> {
        self.requested.lock().expect("requested").clone()
    }
}

fn answer(mut stream: TcpStream, routes: &Routes, requested: &Mutex<Vec<String>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut request_line = String::new();
    reader.read_line(&mut request_line).expect("request line");
    let mut header = String::new();
    while reader.read_line(&mut header).expect("header") > 2 {
        header.clear();
    }
    let path = request_line.split(' ').nth(1).expect("path").to_owned();
    requested.lock().expect("requested").push(path.clone());
    let body = routes.lock().expect("routes").get(&path).cloned();
    let (status, body) = match body {
        Some(body) => ("200 OK", body),
        None => ("404 Not Found", b"Not Found".to_vec()),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).expect("head");
    stream.write_all(&body).expect("body");
}

/// A stand-in for a released binary: all `aether update` asks of it is `--version`.
fn fake_binary(version: &str) -> String {
    format!("#!/bin/sh\necho \"aether {version}\"\n")
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// The real binary installed in a directory of its own, and a release server.
struct Fixture {
    dir: tempfile::TempDir,
    /// Apart from `dir`: a socket path has to stay short.
    socket_dir: tempfile::TempDir,
    binary: PathBuf,
    server: Server,
}

impl Fixture {
    /// Installed the way `install.sh` leaves it: the binary and its receipt.
    fn installed() -> Self {
        let fixture = Self::without_receipt();
        std::fs::write(fixture.bin_dir().join(RECEIPT), "installer=install.sh\n").expect("receipt");
        fixture
    }

    fn without_receipt() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("update-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .expect("fixture directory");
        let bin = dir.path().join("root").join("bin");
        std::fs::create_dir_all(&bin).expect("install directory");
        let binary = bin.join("aether");
        // A link, not a copy of a large file: an update renames over it.
        std::fs::hard_link(env!("CARGO_BIN_EXE_aether"), &binary).expect("install the binary");
        let socket_dir = tempfile::Builder::new()
            .prefix("upd-")
            .tempdir()
            .expect("socket directory");
        Self {
            dir,
            socket_dir,
            binary,
            server: Server::start(),
        }
    }

    fn bin_dir(&self) -> &Path {
        self.binary.parent().expect("install directory")
    }

    /// Cargo's install record beside `bin`, as `cargo install` leaves it.
    fn record_cargo_install(&self) {
        let cargo_root = self.bin_dir().parent().expect("cargo root");
        std::fs::write(cargo_root.join(".crates.toml"), CARGO_RECORD).expect("cargo record");
    }

    /// Publish `tag`, whose binary says it is `reports`.
    fn publish(&self, tag: &str, reports: &str) {
        self.publish_binary(tag, &fake_binary(reports));
    }

    /// Publish `tag` with `binary`, a script, as its `aether`.
    fn publish_binary(&self, tag: &str, binary: &str) {
        let archive = self.archive(binary);
        let checksum = format!("{}  {}\n", sha256(&archive), archive_name());
        self.publish_assets(tag, archive, checksum);
    }

    fn publish_assets(&self, tag: &str, archive: Vec<u8>, checksum: String) {
        let name = archive_name();
        self.server
            .serve(&format!("/download/{tag}/{name}"), archive);
        self.server
            .serve(&format!("/download/{tag}/{name}.sha256"), checksum);
    }

    /// Publish `tag`'s changelog, the release notes `aether update` shows.
    fn publish_notes(&self, tag: &str, changelog: &str) {
        self.server
            .serve(&format!("/download/{tag}/CHANGELOG.md"), changelog);
    }

    fn latest_is(&self, tag: &str) {
        self.server
            .serve("/latest/download/VERSION", format!("{tag}\n"));
    }

    /// A release archive with the members `scripts/release.sh` packs, made by
    /// the same `tar` that unpacks it.
    fn archive(&self, binary: &str) -> Vec<u8> {
        let stage = tempfile::tempdir_in(self.dir.path()).expect("stage");
        std::fs::write(stage.path().join("aether"), binary).expect("binary");
        std::fs::write(stage.path().join("LICENSE"), "MIT\n").expect("license");
        std::fs::write(stage.path().join("THIRD-PARTY-NOTICES"), "notices\n").expect("notices");
        let archive = stage.path().join("archive.tar.gz");
        let packed = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(stage.path())
            .args(["aether", "LICENSE", "THIRD-PARTY-NOTICES"])
            .status()
            .expect("tar");
        assert!(packed.success(), "tar packs the fake release");
        std::fs::read(&archive).expect("archive")
    }

    fn socket(&self) -> PathBuf {
        self.socket_dir.path().join("d.sock")
    }

    fn update(&self, args: &[&str]) -> Output {
        self.update_from(&self.server.base, args)
    }

    fn update_from(&self, base: &str, args: &[&str]) -> Output {
        self.update_command(base, args)
            .output()
            .expect("aether update")
    }

    /// `aether update` with `answer` on its standard input.
    fn update_answering(&self, args: &[&str], answer: &str) -> Output {
        let mut child = self
            .update_command(&self.server.base, args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("aether update");
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(answer.as_bytes()).expect("answer");
        drop(stdin);
        child.wait_with_output().expect("aether update")
    }

    fn update_command(&self, base: &str, args: &[&str]) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .arg("--socket")
            .arg(self.socket())
            .arg("update")
            .args(args)
            .env("AETHER_INSTALL_BASE_URL", base)
            .env("AETHER_CONFIG_DIR", self.dir.path().join("config"))
            .env("AETHER_DATA_DIR", self.dir.path().join("data"))
            .env("NO_PROXY", "127.0.0.1");
        for proxy in [
            "http_proxy",
            "HTTP_PROXY",
            "https_proxy",
            "HTTPS_PROXY",
            "all_proxy",
            "ALL_PROXY",
        ] {
            command.env_remove(proxy);
        }
        command
    }

    /// What the installed binary says it is now.
    fn installed_version(&self) -> String {
        let output = Command::new(&self.binary)
            .arg("--version")
            .output()
            .expect("installed binary runs");
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn assert_unchanged(&self) {
        assert_eq!(self.installed_version(), format!("aether {RUNNING}"));
        let original = std::fs::metadata(env!("CARGO_BIN_EXE_aether")).expect("built binary");
        let installed = std::fs::metadata(&self.binary).expect("installed binary");
        assert_eq!(installed.len(), original.len(), "still the built binary");
        self.assert_nothing_staged();
    }

    fn assert_nothing_staged(&self) {
        let mut left: Vec<_> = std::fs::read_dir(self.bin_dir())
            .expect("install directory")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("name")
            })
            .filter(|name| name != RECEIPT)
            .collect();
        left.sort();
        assert_eq!(left, ["aether"], "no staging directory is left behind");
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn check_reports_a_newer_release_with_its_own_exit_code_and_changes_nothing() {
    let fixture = Fixture::installed();
    fixture.latest_is("v99.0.0");
    let output = fixture.update(&["--check"]);
    assert_eq!(output.status.code(), Some(UPDATE_AVAILABLE), "{output:?}");
    let said = stdout(&output);
    assert!(said.contains("aether 99.0.0 is available"), "{said}");
    assert!(said.contains(&format!("this is {RUNNING}")), "{said}");
    assert!(
        said.contains("Run `aether update` to install it."),
        "{said}"
    );
    assert_eq!(fixture.server.requested(), ["/latest/download/VERSION"]);
    fixture.assert_unchanged();
}

#[test]
fn check_names_the_command_for_a_binary_update_would_refuse() {
    let copied = Fixture::without_receipt();
    let from_cargo = Fixture::without_receipt();
    from_cargo.record_cargo_install();
    for (fixture, command) in [(copied, INSTALL_COMMAND), (from_cargo, CARGO_COMMAND)] {
        fixture.latest_is("v99.0.0");
        let output = fixture.update(&["--check"]);
        assert_eq!(output.status.code(), Some(UPDATE_AVAILABLE), "{output:?}");
        let said = stdout(&output);
        assert!(said.contains("aether 99.0.0 is available"), "{said}");
        assert!(
            said.contains(&format!(
                "`aether update` leaves this binary alone; install it with `{command}`."
            )),
            "{said}"
        );
        assert!(!said.contains("Run `aether update`"), "{said}");
        assert_eq!(fixture.server.requested(), ["/latest/download/VERSION"]);
        fixture.assert_unchanged();
    }
}

#[test]
fn check_exits_zero_when_nothing_newer_is_published() {
    for (latest, says) in [
        (format!("v{RUNNING}"), "is up to date"),
        (
            "v0.0.0".to_owned(),
            "is newer than the latest release, 0.0.0",
        ),
    ] {
        let fixture = Fixture::without_receipt();
        fixture.latest_is(&latest);
        let output = fixture.update(&["--check"]);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(stdout(&output).contains(says), "{output:?}");
        fixture.assert_unchanged();
    }
}

/// The one JSON object a `--json` run prints: nothing else is on stdout.
fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("stdout is not one JSON object ({error}): {output:?}"))
}

#[test]
fn check_as_json_is_one_object_with_the_same_exit_codes() {
    let nothing = serde_json::Value::Null;
    for (installed, latest, command, code) in [
        (true, "v99.0.0", "aether update".into(), UPDATE_AVAILABLE),
        (false, "v99.0.0", INSTALL_COMMAND.into(), UPDATE_AVAILABLE),
        (true, "v0.0.0", nothing.clone(), 0),
        (false, "v0.0.0", nothing, 0),
    ] {
        let fixture = if installed {
            Fixture::installed()
        } else {
            Fixture::without_receipt()
        };
        fixture.latest_is(latest);
        let output = fixture.update(&["--check", "--json"]);
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        assert_eq!(
            json(&output),
            serde_json::json!({
                "current": RUNNING,
                "latest": latest.trim_start_matches('v'),
                "update_available": code == UPDATE_AVAILABLE,
                "update_command": command,
            })
        );
        fixture.assert_unchanged();
    }
}

/// Whether `aether stop` here can signal a daemon that refuses its build: it
/// needs the socket peer's pidfd, which only Linux 6.5 and newer hand out.
#[cfg(target_os = "linux")]
fn signals_a_refusing_daemon() -> bool {
    let (ours, _theirs) = std::os::unix::net::UnixStream::pair().expect("socket pair");
    nix::sys::socket::getsockopt(&ours, nix::sys::socket::sockopt::PeerPidfd).is_ok()
}

#[cfg(not(target_os = "linux"))]
fn signals_a_refusing_daemon() -> bool {
    false
}

/// Refused before any asset is fetched, with the daemon's own stop command
/// and the same update to run after it. Returns the whole error.
fn assert_stop_first(fixture: &Fixture, output: &Output, rerun: &str) -> String {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error = stderr(output);
    let socket = fixture.socket();
    let aether = format!("aether --socket '{}'", socket.display());
    assert!(
        error.contains(&format!(
            "aether was not updated: the daemon on {} has to be stopped first",
            socket.display()
        )),
        "{error}"
    );
    assert!(
        error.contains(&format!(
            "Run `{aether} stop`, then `{aether} {rerun}` again"
        )),
        "{error}"
    );
    assert!(
        fixture
            .server
            .requested()
            .iter()
            .all(|path| path == "/latest/download/VERSION"),
        "no asset is fetched"
    );
    fixture.assert_unchanged();
    error
}

#[test]
fn update_as_json_reports_what_was_installed_and_what_to_restart() {
    let fixture = Fixture::installed();
    fixture.latest_is("v99.0.0");
    fixture.publish("v99.0.0", "99.0.0");
    let _daemon = std::os::unix::net::UnixListener::bind(fixture.socket()).expect("daemon socket");
    let output = fixture.update(&["--json"]);
    if !signals_a_refusing_daemon() {
        let error = assert_stop_first(&fixture, &output, "update");
        let message = json(&output)["error"]["message"].to_string();
        assert!(message.contains("has to be stopped first"), "{message}");
        assert!(error.contains("aether was not updated"), "{error}");
        return;
    }
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(fixture.installed_version(), "aether 99.0.0");
    let aether = format!("aether --socket '{}'", fixture.socket().display());
    let binary = std::fs::canonicalize(&fixture.binary).expect("installed binary");
    assert_eq!(
        json(&output),
        serde_json::json!({
            "current": RUNNING,
            "version": "99.0.0",
            "action": "upgraded",
            "binary": binary,
            "daemon_running": true,
            "restart": [format!("{aether} stop"), aether],
            "release_notes": {
                "published": false,
                "url": format!("{}/tag/v99.0.0", fixture.server.base),
            },
        })
    );
}

#[test]
fn nothing_to_do_and_a_refusal_are_json_too() {
    let fixture = Fixture::installed();
    fixture.latest_is(&format!("v{RUNNING}"));
    let output = fixture.update(&["--json"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        json(&output),
        serde_json::json!({"current": RUNNING, "version": RUNNING, "action": "none"})
    );

    let foreign = Fixture::without_receipt();
    let output = foreign.update(&["--json"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let message = json(&output)["error"]["message"].to_string();
    assert!(message.contains("was not installed by"), "{message}");
    assert!(stderr(&output).contains(INSTALL_COMMAND), "{output:?}");
    assert!(foreign.server.requested().is_empty());
    foreign.assert_unchanged();
}

#[test]
fn update_installs_the_latest_release_over_the_running_binary() {
    let fixture = Fixture::installed();
    fixture.latest_is("v99.0.0");
    fixture.publish("v99.0.0", "99.0.0");
    let output = fixture.update(&[]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(fixture.installed_version(), "aether 99.0.0");
    let said = stdout(&output);
    assert!(
        said.contains(&format!("Updating aether {RUNNING} to 99.0.0")),
        "{said}"
    );
    assert!(said.contains("Installed aether 99.0.0"), "{said}");
    assert!(
        said.contains("No daemon is running: the next `aether --socket"),
        "{said}"
    );
    assert!(
        said.contains("99.0.0 publishes no release notes for `aether update`"),
        "{said}"
    );
    let name = archive_name();
    assert_eq!(
        fixture.server.requested(),
        [
            "/latest/download/VERSION".to_owned(),
            "/download/v99.0.0/CHANGELOG.md".to_owned(),
            format!("/download/v99.0.0/{name}.sha256"),
            format!("/download/v99.0.0/{name}"),
        ]
    );
    fixture.assert_nothing_staged();
    let built = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("--version")
        .output()
        .expect("built binary");
    assert_eq!(
        stdout(&built).trim(),
        format!("aether {RUNNING}"),
        "the old file was renamed over, never written into"
    );
}

/// The staged binary locks a directory beside itself, so the swap works and
/// the staging directory cannot be emptied afterwards.
#[test]
fn staging_that_cannot_be_removed_is_a_warning_on_an_update_that_happened() {
    let fixture = Fixture::installed();
    fixture.latest_is("v99.0.0");
    let locks_staging = "#!/bin/sh\nstuck=\"$(dirname \"$0\")/stuck\"\n\
        [ -d \"$stuck\" ] || { mkdir \"$stuck\" && : >\"$stuck/file\" && chmod 000 \"$stuck\"; }\n\
        echo \"aether 99.0.0\"\n";
    fixture.publish_binary("v99.0.0", locks_staging);
    let output = fixture.update(&[]);
    let staged: Vec<PathBuf> = std::fs::read_dir(fixture.bin_dir())
        .expect("install directory")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.join("stuck").is_dir())
        .collect();
    for directory in &staged {
        let unlocked = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(directory.join("stuck"), unlocked).expect("unlock");
    }
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(fixture.installed_version(), "aether 99.0.0");
    assert!(
        stdout(&output).contains("Installed aether 99.0.0"),
        "{output:?}"
    );
    // Root empties a locked directory, so there nothing is left to warn about.
    if staged.is_empty() {
        assert_eq!(stderr(&output), "");
        return;
    }
    let warning = stderr(&output);
    assert!(
        warning.starts_with("Warning: aether was updated, but"),
        "{warning}"
    );
    assert!(
        warning.contains(&staged[0].display().to_string()),
        "{warning}"
    );
    assert!(!warning.contains("Error:"), "{warning}");
}

#[test]
fn a_running_daemon_is_named_with_the_commands_that_restart_it() {
    let fixture = Fixture::installed();
    fixture.latest_is("v99.0.0");
    fixture.publish("v99.0.0", "99.0.0");
    let _daemon = std::os::unix::net::UnixListener::bind(fixture.socket()).expect("daemon socket");
    let output = fixture.update(&[]);
    if !signals_a_refusing_daemon() {
        assert_stop_first(&fixture, &output, "update");
        return;
    }
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(fixture.installed_version(), "aether 99.0.0");
    let said = stdout(&output);
    let aether = format!("aether --socket '{}'", fixture.socket().display());
    assert!(
        said.contains(&format!("The running daemon is still aether {RUNNING}")),
        "{said}"
    );
    assert!(said.contains(&format!("run `{aether} stop`")), "{said}");
    assert!(
        said.contains(&format!("then `{aether}` to start 99.0.0")),
        "{said}"
    );
}

#[test]
fn a_pinned_update_with_a_running_daemon_names_the_same_pin_to_run_again() {
    let fixture = Fixture::installed();
    fixture.publish("v0.0.0", "0.0.0");
    let _daemon = std::os::unix::net::UnixListener::bind(fixture.socket()).expect("daemon socket");
    let output = fixture.update(&["--version", "0.0.0"]);
    if !signals_a_refusing_daemon() {
        assert_stop_first(&fixture, &output, "update --version v0.0.0");
        return;
    }
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(fixture.installed_version(), "aether 0.0.0");
    assert!(
        stdout(&output).contains(&format!("The running daemon is still aether {RUNNING}")),
        "{output:?}"
    );
}

/// The refusal on every platform: a socket this user cannot reach is never
/// taken for "no daemon", and it cannot be pinned either.
#[test]
fn a_daemon_that_cannot_be_confirmed_stoppable_stops_the_update_before_any_download() {
    let fixture = Fixture::installed();
    fixture.latest_is("v99.0.0");
    fixture.publish("v99.0.0", "99.0.0");
    let _daemon = std::os::unix::net::UnixListener::bind(fixture.socket()).expect("daemon socket");
    let socket_dir = fixture.socket_dir.path();
    std::fs::set_permissions(socket_dir, std::fs::Permissions::from_mode(0o000)).expect("lock");
    let output = fixture.update(&[]);
    std::fs::set_permissions(socket_dir, std::fs::Permissions::from_mode(0o700)).expect("unlock");
    // Root reaches every socket, so there the update goes the platform's usual way.
    if nix::unistd::geteuid().is_root() {
        return;
    }
    let error = assert_stop_first(&fixture, &output, "update");
    assert!(error.contains("its socket could not be reached"), "{error}");
    assert_eq!(fixture.server.requested(), ["/latest/download/VERSION"]);
}

#[test]
fn a_running_daemon_is_no_obstacle_when_there_is_nothing_to_install() {
    let fixture = Fixture::installed();
    fixture.latest_is(&format!("v{RUNNING}"));
    let _daemon = std::os::unix::net::UnixListener::bind(fixture.socket()).expect("daemon socket");
    let output = fixture.update(&[]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        stdout(&output).contains("is already installed; nothing to do"),
        "{output:?}"
    );
    fixture.assert_unchanged();
}

#[test]
fn a_checksum_mismatch_is_refused_and_the_old_binary_stays() {
    let fixture = Fixture::installed();
    fixture.latest_is("v99.0.0");
    let wrong = format!("{}  {}\n", sha256(b"another archive"), archive_name());
    let archive = fixture.archive(&fake_binary("99.0.0"));
    fixture.publish_assets("v99.0.0", archive, wrong);
    let output = fixture.update(&[]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error = stderr(&output);
    assert!(error.contains("checksum mismatch"), "{error}");
    assert!(error.contains("aether was not updated"), "{error}");
    fixture.assert_unchanged();
}

#[test]
fn a_binary_that_reports_another_version_is_never_installed() {
    let fixture = Fixture::installed();
    fixture.latest_is("v99.0.0");
    fixture.publish("v99.0.0", "98.0.0");
    let output = fixture.update(&[]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error = stderr(&output);
    assert!(
        error.contains("reports `aether 98.0.0`, not aether 99.0.0"),
        "{error}"
    );
    fixture.assert_unchanged();
}

#[test]
fn a_pinned_older_release_is_installed_and_called_a_downgrade() {
    let fixture = Fixture::installed();
    fixture.publish("v0.0.0", "0.0.0");
    let output = fixture.update(&["--version", "v0.0.0"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let said = stdout(&output);
    assert!(
        said.contains(&format!("Downgrading aether {RUNNING} to 0.0.0")),
        "{said}"
    );
    assert_eq!(fixture.installed_version(), "aether 0.0.0");
    assert!(
        !fixture
            .server
            .requested()
            .contains(&"/latest/download/VERSION".to_owned()),
        "a pin never asks which release is the latest"
    );
}

#[test]
fn a_pinned_newer_release_is_installed_without_the_v() {
    let fixture = Fixture::installed();
    fixture.publish("v99.1.2", "99.1.2");
    let output = fixture.update(&["--version", "99.1.2"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(fixture.installed_version(), "aether 99.1.2");
}

#[test]
fn the_installed_version_is_not_downloaded_again() {
    for args in [&["--version", RUNNING][..], &[][..]] {
        let fixture = Fixture::installed();
        fixture.latest_is(&format!("v{RUNNING}"));
        let output = fixture.update(args);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(
            stdout(&output).contains("is already installed; nothing to do"),
            "{output:?}"
        );
        assert!(
            fixture
                .server
                .requested()
                .iter()
                .all(|path| path == "/latest/download/VERSION"),
            "no asset is fetched"
        );
        fixture.assert_unchanged();
    }
}

#[test]
fn a_release_older_than_this_build_is_not_installed_unasked() {
    let fixture = Fixture::installed();
    fixture.latest_is("v0.0.0");
    fixture.publish("v0.0.0", "0.0.0");
    let output = fixture.update(&[]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        stdout(&output).contains("`aether update --version v0.0.0` installs that release"),
        "{output:?}"
    );
    fixture.assert_unchanged();
}

#[test]
fn a_binary_the_installer_did_not_install_is_left_alone_without_a_request() {
    let fixture = Fixture::without_receipt();
    fixture.latest_is("v99.0.0");
    fixture.publish("v99.0.0", "99.0.0");
    let output = fixture.update(&[]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error = stderr(&output);
    assert!(
        error.contains("was not installed by Aether's install.sh"),
        "{error}"
    );
    assert!(error.contains(INSTALL_COMMAND), "{error}");
    assert_eq!(fixture.server.requested(), Vec::<String>::new());
    fixture.assert_unchanged();
}

#[test]
fn a_cargo_installed_binary_is_left_alone_and_told_the_cargo_command() {
    // A receipt beside it changes nothing: cargo's record is read first.
    for fixture in [Fixture::without_receipt(), Fixture::installed()] {
        fixture.record_cargo_install();
        fixture.latest_is("v99.0.0");
        fixture.publish("v99.0.0", "99.0.0");
        let output = fixture.update(&[]);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let error = stderr(&output);
        assert!(error.contains("was installed by cargo"), "{error}");
        assert!(error.contains(CARGO_COMMAND), "{error}");
        assert_eq!(fixture.server.requested(), Vec::<String>::new());
        fixture.assert_unchanged();
    }
}

#[test]
fn an_install_directory_that_is_not_writable_is_refused() {
    let fixture = Fixture::installed();
    fixture.latest_is("v99.0.0");
    fixture.publish("v99.0.0", "99.0.0");
    let read_only = std::fs::Permissions::from_mode(0o555);
    std::fs::set_permissions(fixture.bin_dir(), read_only).expect("read-only");
    let output = fixture.update(&[]);
    std::fs::set_permissions(fixture.bin_dir(), std::fs::Permissions::from_mode(0o755))
        .expect("writable again");
    // Root may write anywhere, so there the update simply succeeds.
    if stdout(&output).contains("Installed aether 99.0.0") {
        return;
    }
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error = stderr(&output);
    assert!(error.contains("is not writable by you"), "{error}");
    assert!(error.contains(INSTALL_COMMAND), "{error}");
    assert_eq!(fixture.server.requested(), Vec::<String>::new());
    fixture.assert_unchanged();
}

#[test]
fn a_release_that_does_not_exist_is_an_error_and_changes_nothing() {
    let fixture = Fixture::installed();
    let output = fixture.update(&["--version", "v77.7.7"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error = stderr(&output);
    assert!(error.contains("404 Not Found"), "{error}");
    assert!(error.contains("/download/v77.7.7/"), "{error}");
    fixture.assert_unchanged();
}

#[test]
fn what_the_server_says_is_validated_before_it_is_used() {
    let cases: [(&[u8], &str); 3] = [
        (b"<html>rate limited</html>", "does not name a release"),
        (b"v1.2.3-rc1\n", "does not name a release"),
        (&[b'9'; 8192], "more than the 4096 allowed"),
    ];
    for (version_asset, says) in cases {
        let fixture = Fixture::installed();
        fixture
            .server
            .serve("/latest/download/VERSION", version_asset);
        for args in [&["--check"][..], &[][..]] {
            let output = fixture.update(args);
            assert_eq!(output.status.code(), Some(1), "{output:?}");
            assert!(stderr(&output).contains(says), "{output:?}");
        }
        fixture.assert_unchanged();
    }
}

#[test]
fn an_unreachable_server_is_an_error_and_changes_nothing() {
    let fixture = Fixture::installed();
    let closed = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", closed.local_addr().expect("address"));
    drop(closed);
    for args in [&["--check"][..], &[][..]] {
        let output = fixture.update_from(&base, args);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(
            stderr(&output).contains("could not learn the latest release"),
            "{output:?}"
        );
    }
    fixture.assert_unchanged();
}

#[test]
fn a_download_base_that_is_not_https_or_local_is_refused() {
    let fixture = Fixture::installed();
    for base in ["http://example.com/releases", "ftp://127.0.0.1/releases"] {
        let output = fixture.update_from(base, &["--check"]);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(
            stderr(&output).contains("AETHER_INSTALL_BASE_URL is not usable"),
            "{output:?}"
        );
    }
}

#[test]
fn usage_errors_and_help_follow_the_documented_exit_codes() {
    let fixture = Fixture::installed();
    let conflict = fixture.update(&["--check", "--version", "v1.0.0"]);
    assert_eq!(conflict.status.code(), Some(2), "{conflict:?}");
    for (tag, says) in [
        ("1.2", "expected the form v1.2.3"),
        ("v1.0.0-rc1", "`v1.0.0-rc1` is not a release version"),
    ] {
        let bad_version = fixture.update(&["--version", tag]);
        assert_eq!(bad_version.status.code(), Some(2), "{bad_version:?}");
        let error = stderr(&bad_version);
        assert!(error.contains(says), "{error}");
        assert!(error.contains("--version <TAG>"), "{error}");
    }
    let help = fixture.update(&["--help"]);
    assert_eq!(help.status.code(), Some(0), "{help:?}");
    let help = stdout(&help);
    assert!(help.contains("10 = a newer release exists"), "{help}");
    assert!(help.contains("AETHER_INSTALL_BASE_URL"), "{help}");
    assert_eq!(fixture.server.requested(), Vec::<String>::new());
}

/// A changelog whose newest release migrates the store one way, and whose
/// release before it only changes a default.
const MIGRATING_CHANGELOG: &str = "# Changelog

## 99.0.0

Agents can now pause.

### Before updating

- **Store schema 99.** The migration is one way: an older binary refuses
  the migrated store.

## 98.0.0

Cleanup.

### Before updating

- **Storage cleanup is on by default.** Logs go after fourteen days.
";

fn migrating_release() -> Fixture {
    let fixture = Fixture::installed();
    fixture.latest_is("v99.0.0");
    fixture.publish("v99.0.0", "99.0.0");
    fixture.publish_notes("v99.0.0", MIGRATING_CHANGELOG);
    fixture
}

#[test]
fn a_one_way_migration_is_shown_and_installed_only_when_confirmed() {
    for answer in ["", "n\n"] {
        let fixture = migrating_release();
        let output = fixture.update_answering(&[], answer);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let said = stdout(&output);
        assert!(
            said.contains("What 99.0.0 brings:\n\nAgents can now pause."),
            "{said}"
        );
        assert!(said.contains("Before updating to 98.0.0:"), "{said}");
        assert!(said.contains("Before updating to 99.0.0:"), "{said}");
        assert!(said.contains("**Store schema 99.**"), "{said}");
        let error = stderr(&output);
        assert!(error.contains("migrates the store one way"), "{error}");
        assert!(error.contains("[y/N]"), "{error}");
        assert!(error.contains("aether was not updated"), "{error}");
        assert!(error.contains("or add --yes"), "{error}");
        assert_eq!(
            fixture.server.requested(),
            ["/latest/download/VERSION", "/download/v99.0.0/CHANGELOG.md"],
            "nothing is downloaded before the answer"
        );
        fixture.assert_unchanged();
    }

    let fixture = migrating_release();
    let output = fixture.update_answering(&[], "y\n");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(stderr(&output).contains("[y/N]"), "{output:?}");
    assert_eq!(fixture.installed_version(), "aether 99.0.0");
}

#[test]
fn yes_installs_a_one_way_migration_without_asking() {
    let fixture = migrating_release();
    let output = fixture.update(&["--yes"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        stdout(&output).contains("Before updating to 99.0.0:"),
        "{output:?}"
    );
    assert!(!stderr(&output).contains("[y/N]"), "{output:?}");
    assert_eq!(fixture.installed_version(), "aether 99.0.0");

    let fixture = migrating_release();
    let output = fixture.update(&["--yes", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let notes = &json(&output)["release_notes"];
    assert_eq!(notes["published"], true, "{notes}");
    assert_eq!(notes["one_way_migration"], true, "{notes}");
    assert_eq!(notes["summary"], "Agents can now pause.", "{notes}");
    assert_eq!(notes["before_updating"][1]["version"], "99.0.0", "{notes}");
    assert!(stderr(&output).is_empty(), "{output:?}");
}

#[test]
fn json_without_yes_asks_on_stderr_and_keeps_stdout_one_object() {
    let fixture = migrating_release();
    let output = fixture.update(&["--json"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let message = json(&output)["error"]["message"].to_string();
    assert!(message.contains("migrates the store one way"), "{message}");
    let error = stderr(&output);
    assert!(error.contains("Before updating to 99.0.0:"), "{error}");
    assert!(error.contains("[y/N]"), "{error}");
    fixture.assert_unchanged();
}

#[test]
fn notes_without_a_migration_are_shown_and_the_update_goes_ahead() {
    let fixture = Fixture::installed();
    fixture.latest_is("v99.0.0");
    fixture.publish("v99.0.0", "99.0.0");
    fixture.publish_notes(
        "v99.0.0",
        "## 99.0.0\n\nFixes only.\n\n### Fixed\n\n- A fix.\n",
    );
    let output = fixture.update(&[]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let said = stdout(&output);
    assert!(
        said.contains("What 99.0.0 brings:\n\nFixes only."),
        "{said}"
    );
    assert!(!said.contains("Before updating"), "{said}");
    assert!(!stderr(&output).contains("[y/N]"), "{output:?}");
    assert_eq!(fixture.installed_version(), "aether 99.0.0");
}

#[test]
fn notes_that_do_not_describe_the_release_stop_the_update() {
    let fixture = Fixture::installed();
    fixture.latest_is("v99.0.0");
    fixture.publish("v99.0.0", "99.0.0");
    fixture.publish_notes("v99.0.0", "## 98.0.0\n\nAn older release.\n");
    let output = fixture.update(&["--yes"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error = stderr(&output);
    assert!(error.contains("no section for 99.0.0"), "{error}");
    assert!(error.contains("aether was not updated"), "{error}");
    fixture.assert_unchanged();
}
