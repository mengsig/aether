//! `aether doctor`: is this installation able to run agents, and if not,
//! what has to change.
//!
//! It runs in the client, not the daemon, because the moment a person needs
//! it most is the moment the daemon will not start. Everything it looks at
//! is passed in, so each check is a function of its input and has a test.

use std::path::{Path, PathBuf};

mod agent_temp;
mod providers;
pub use agent_temp::check_agent_temp;
pub use providers::{
    PROBE_TIMEOUT, Unreachable, check_providers, local_servers, unreachable_servers,
};

use aether_core::disk::{DiskFloor, LowDisk};
use aether_protocol::DaemonStatus;
use aether_protocol::remote::RemoteStatus;
use serde::{Deserialize, Serialize};

/// Free space under which the data directory is called out.
const DISK_WARN_BYTES: u64 = 1 << 30; // 1 GiB
const DISK_FAIL_BYTES: u64 = 100 << 20; // 100 MiB

/// How bad a check's answer is. Ordered: the report's verdict is its worst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Ok,
    /// Works, but something will bite later.
    Warn,
    /// Will not work until it is fixed.
    Fail,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }

    /// `aether doctor`'s exit code: 0 all well, 1 something to watch, 2
    /// something broken, so a script can branch on it.
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Ok => 0,
            Self::Warn => 1,
            Self::Fail => 2,
        }
    }
}

/// One thing looked at, what was found, and what to do about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub level: Level,
    pub detail: String,
    /// What would fix it. Absent when nothing needs fixing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Check {
    fn ok(name: &str, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            level: Level::Ok,
            detail: detail.into(),
            fix: None,
        }
    }

    fn warn(name: &str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            level: Level::Warn,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }

    fn fail(name: &str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            level: Level::Fail,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }
}

impl Check {
    /// Credentials are the daemon's answer to give; without one the check
    /// says it could not look rather than reporting no providers.
    pub fn no_daemon_providers(command: &str) -> Self {
        Self::warn(
            "providers",
            "not checked: no daemon to ask",
            format!("start one with `{command}`, then run this again"),
        )
    }

    /// The daemon answered for credentials but not with its providers'
    /// addresses, so whether its local servers listen is unknown.
    pub fn unprobed_providers(why: &str, command: &str) -> Self {
        Self::warn(
            "providers",
            format!("not checked: the daemon did not list its providers: {why}"),
            format!("`{command} providers` shows what it says"),
        )
    }
}

/// Every check, and the worst level among them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub level: Level,
    pub checks: Vec<Check>,
}

impl Report {
    pub fn new(checks: Vec<Check>) -> Self {
        let level = checks.iter().map(|c| c.level).max().unwrap_or(Level::Ok);
        Self { level, checks }
    }
}

/// The paths this installation uses. Passed in so a test can point the
/// checks at a directory it made.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub socket: PathBuf,
}

impl Paths {
    /// The store under `data_dir`, named the one way the rest of the system
    /// names it.
    fn store_path(&self) -> PathBuf {
        self.data_dir.join(aether_protocol::paths::STORE_FILE_NAME)
    }
}

/// `config.toml` and `providers.toml` load, and every MCP server in them is
/// usable. A config that does not parse stops the daemon starting, so it is
/// a failure rather than a warning.
/// Absolute paths - config, socket, store, disk - are in nearly every line,
/// and doctor output is what people paste into a bug report, so they carry a
/// home directory and a username with them. They stay: "the store is at a
/// path" diagnoses nothing, and half of what doctor is for is telling someone
/// which file it actually read. What must never travel is content, and none
/// does: no config source line, no credential, no environment variable.
///
/// A parse error's location without the source line it quotes.
///
/// A `toml` error renders the offending line under a caret, and doctor output
/// is what people paste into a bug report. `config.toml` is designed to hold
/// credential *names* rather than values, but a file that does not parse is
/// exactly one someone has been editing by hand, so the line, column and
/// reason travel and the file's own text does not.
fn without_source_span(error: &str) -> String {
    error
        .lines()
        .map(str::trim_end)
        .filter(|line| {
            let rest = line.trim_start().trim_start_matches(char::is_numeric);
            !rest.trim_start().starts_with('|')
        })
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

pub fn check_credentials_dir(paths: &Paths) -> Check {
    use aether_model::auth::store::{credentials_dir_fix, validate_credentials_dir};
    match validate_credentials_dir(&paths.config_dir) {
        Ok(()) => Check::ok("credentials", "no unsafe credential directory found"),
        Err(error) => Check::warn(
            "credentials",
            error.to_string(),
            format!(
                "for a real directory you own, run `{}`; replace a symlink with a private directory",
                credentials_dir_fix(&paths.config_dir)
            ),
        ),
    }
}

pub fn check_config(paths: &Paths) -> Check {
    if !paths.config_dir.exists() {
        return Check::ok(
            "config",
            format!(
                "{} does not exist yet; the daemon writes templates on first start",
                paths.config_dir.display()
            ),
        );
    }
    // `load_config` already refuses a server that names no transport or an
    // unusable url, so a bad one is a parse failure here.
    let config = match aether_model::config::load_config(&paths.config_dir) {
        Ok(config) => config,
        Err(e) => {
            return Check::fail(
                "config",
                without_source_span(&e.to_string()),
                "fix the file, or move it aside and let the daemon write a fresh template",
            );
        }
    };
    if let Err(e) = config.mcp.check_names_are_distinct() {
        return Check::fail("config", e, "rename one of the servers in config.toml");
    }
    // `load_config` reads config.toml alone, so a providers.toml that does
    // not parse passed this check clean while the docstring said otherwise.
    let mut registry = aether_model::Registry::builtin();
    let overrides =
        aether_model::registry::load_providers_file(&paths.config_dir.join("providers.toml"))
            .and_then(|file| file.map_or(Ok(()), |file| registry.apply_overrides(file)));
    if let Err(e) = overrides {
        return Check::fail(
            "config",
            without_source_span(&e.to_string()),
            "fix providers.toml, or move it aside to fall back to the built-in providers",
        );
    }
    let retired = aether_model::models::retired_model_settings(&registry, &config.model);
    if !retired.is_empty() {
        return Check::fail(
            "config",
            retired.join("; "),
            "name the successor in config.toml's [model] section",
        );
    }
    Check::ok(
        "config",
        format!(
            "{} · {} mcp server(s) configured",
            paths.config_dir.display(),
            config.mcp.servers.len()
        ),
    )
}

/// What this binary found on the daemon socket. A daemon that answered but
/// refused the handshake is neither of the other two: reported as silence it
/// would send the operator after a daemon that is in fact running.
#[derive(Debug)]
pub enum SocketState<'a> {
    /// A daemon answered and reported its status.
    Answering(&'a DaemonStatus),
    /// A daemon answered and refused this build's protocol, in its words.
    Refused(&'a str),
    /// Nothing answered.
    Silent,
}

/// Is a daemon answering, and is it the one this binary would talk to.
pub fn check_socket(paths: &Paths, state: SocketState<'_>, command: &str) -> Check {
    match state {
        SocketState::Answering(s) => Check::ok(
            "daemon",
            format!(
                "aetherd {} (pid {}) on {}",
                s.version,
                s.pid,
                paths.socket.display()
            ),
        ),
        // A failure, not a warning: nothing this binary asks for will work,
        // and `aether` will not replace the daemon that holds the socket.
        SocketState::Refused(why) => Check::fail(
            "daemon",
            format!(
                "a daemon on {} refused this build: {why}",
                paths.socket.display()
            ),
            format!("the daemon is from another build; run `{command} stop`, then start it again"),
        ),
        SocketState::Silent if paths.socket.exists() => Check::warn(
            "daemon",
            format!("{} exists but nothing answers", paths.socket.display()),
            format!("the daemon stopped without cleaning up; `{command}` starts a new one"),
        ),
        SocketState::Silent => Check::warn(
            "daemon",
            format!("no daemon at {}", paths.socket.display()),
            format!("start one with `{command}` or `{command} daemon`"),
        ),
    }
}

/// The store opens, passes SQLite's own integrity check, and is a schema
/// this binary understands.
pub fn check_store(paths: &Paths) -> Check {
    // `paths::store_path` owns the name. Built here, a filename that moved
    // would leave this reporting "does not exist yet" at level `ok` - a
    // false pass on the check most likely to matter.
    let path = paths.store_path();
    match std::fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Err(error) = check_creatable(&path) {
                return Check::fail(
                    "store",
                    format!("cannot create {}: {error:#}", path.display()),
                    "check the data directory's permissions and destination",
                );
            }
            return Check::ok(
                "store",
                format!(
                    "{} does not exist yet; the daemon creates it",
                    path.display()
                ),
            );
        }
        Err(error) => {
            return Check::fail(
                "store",
                format!("cannot inspect {}: {error}", path.display()),
                "check the data directory's permissions",
            );
        }
        Ok(_) => {}
    }
    let conn = match rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(conn) => conn,
        Err(e) => {
            return Check::fail(
                "store",
                format!("{}: {e}", path.display()),
                "check the file is readable and not on a full or read-only disk",
            );
        }
    };
    let integrity: Result<String, _> =
        conn.query_row("PRAGMA quick_check(1)", [], |row| row.get(0));
    match integrity {
        Ok(answer) if answer == "ok" => {}
        Ok(answer) => {
            return Check::fail(
                "store",
                format!("{}: {answer}", path.display()),
                "restore the store from a backup; agents cannot run on a corrupt log",
            );
        }
        Err(e) => {
            return Check::fail(
                "store",
                format!("{}: {e}", path.display()),
                "restore the store from a backup",
            );
        }
    }
    let found: Option<u32> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| v.parse().ok());
    let supported = aether_store::SCHEMA_VERSION;
    match found {
        Some(v) if v == supported => Check::ok(
            "store",
            format!("{} · schema {v} · integrity ok", path.display()),
        ),
        Some(v) if v > supported => Check::fail(
            "store",
            format!("schema {v}, this build reads {supported}"),
            "a newer aether wrote this store; upgrade the binary rather than downgrading the store",
        ),
        Some(v) => Check::ok(
            "store",
            format!("schema {v} · migrates to {supported} on next start"),
        ),
        None => Check::fail(
            "store",
            format!("{} carries no schema version", path.display()),
            "the file is not an aether store; move it aside",
        ),
    }
}

/// Room to keep writing events, snapshots and artifacts. `floor` is the
/// free space new turns wait for (`[scheduler] min_free_disk_mib`), `None`
/// when they never wait; `held` is a running daemon's word that they wait now.
pub fn check_disk(paths: &Paths, floor: Option<DiskFloor>, held: Option<&LowDisk>) -> Check {
    // The data directory may not exist yet; its nearest existing ancestor is
    // on the same filesystem, which is what the number is about.
    let mut probe: &Path = &paths.data_dir;
    while !probe.exists() {
        match probe.parent() {
            Some(parent) => probe = parent,
            None => {
                return Check::warn(
                    "disk",
                    format!("{} has no reachable parent", paths.data_dir.display()),
                    "point AETHER_DATA_DIR somewhere that exists",
                );
            }
        }
    }
    let measured =
        fs4::available_space(probe).and_then(|free| Ok((free, fs4::total_space(probe)?)));
    match measured {
        Ok((free, total)) => judge_disk(probe, free, floor.map(|f| f.bytes(total)), held),
        Err(e) => Check::warn(
            "disk",
            format!("free space under {} is unknown: {e}", probe.display()),
            "check the filesystem is mounted and readable",
        ),
    }
}

/// [`check_disk`] once measured: `free` bytes under `probe`, against a floor
/// already worked out for its filesystem.
fn judge_disk(probe: &Path, free: u64, floor: Option<u64>, held: Option<&LowDisk>) -> Check {
    let human = format!("{:.1} GiB free under {}", gib(free), probe.display());
    if let Some(low) = held {
        return Check::fail(
            "disk",
            format!(
                "{human} · new turns wait: below the {:.1} GiB floor since {}",
                gib(low.floor_bytes),
                low.since.to_rfc3339()
            ),
            format!(
                "free space on the filesystem holding {}; turns start again with {:.1} GiB free",
                low.path,
                gib(low.floor_bytes.saturating_add(low.floor_bytes / 10))
            ),
        );
    }
    if free < DISK_FAIL_BYTES {
        return Check::fail(
            "disk",
            human,
            "free space, or move AETHER_DATA_DIR: a full disk loses the event a turn just wrote",
        );
    }
    match floor {
        Some(floor) if free < floor => Check::warn(
            "disk",
            format!("{human} · below the {:.1} GiB floor", gib(floor)),
            "free space: a daemon holds new turns until there is more",
        ),
        _ if free < DISK_WARN_BYTES => {
            Check::warn("disk", human, "artifacts and snapshots will fill this soon")
        }
        Some(floor) => Check::ok(
            "disk",
            format!("{human} · new turns wait below {:.1} GiB", gib(floor)),
        ),
        None => Check::ok("disk", human),
    }
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1u64 << 30) as f64
}

/// One engine CLI's detection outcome. Registration and
/// usability are not the same axis any more: an adapter can be usable in
/// full, usable with a known gap (e.g. a version floor verified only for one
/// code path), or not usable at all. Modelled as its own type rather than
/// smuggled through `Result`'s two states, so doctor renders "usable, with
/// limits" distinctly from "not usable".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineStatus {
    /// Detected, nothing known to be missing.
    Ready(String),
    /// Detected and registered, but a specific capability is known not to
    /// work; `limitation` is the adapter's own wording for it, so doctor
    /// never reconstructs it.
    Limited { adapter: String, limitation: String },
    /// Did not register at all - not installed, too old for a floor that
    /// *is* a registration gate, unparsable output, or a missing capability.
    Unavailable(String),
}

/// The engine CLIs on this machine, and whether they are signed in.
///
/// `found` is one entry per engine: its status when detection ran, naming
/// the program, what was detected, the requirement and the remedy
/// rather than a bare "missing" - see [`EngineStatus`].
pub fn check_engines(found: &[(String, EngineStatus)], command: &str) -> Check {
    if found.is_empty() {
        return Check::warn(
            "engines",
            "no engine CLIs found",
            "install Claude Code or Codex to run subscription-backed agents; API providers do not need them",
        );
    }
    let mut usable = Vec::new();
    let mut limited = false;
    let mut missing = Vec::new();
    for (name, status) in found {
        match status {
            EngineStatus::Ready(label) => usable.push(label.clone()),
            EngineStatus::Limited {
                adapter,
                limitation,
            } => {
                limited = true;
                usable.push(format!("{adapter} (usable, with limits: {limitation})"));
            }
            EngineStatus::Unavailable(reason) => missing.push(format!("{name}: {reason}")),
        }
    }
    if usable.is_empty() {
        return Check::warn(
            "engines",
            format!("not usable: {}", missing.join(" · ")),
            format!(
                "install the CLI and put it on PATH, then sign in (`claude login`, `codex login`); `{command} auth` says which it is"
            ),
        );
    }
    let mut check = Check::ok("engines", usable.join(" · "));
    if !missing.is_empty() {
        check.level = Level::Warn;
        check.detail = format!(
            "{} · not usable: {}",
            usable.join(" · "),
            missing.join(" · ")
        );
        check.fix = Some(
            "install the missing CLI and put it on PATH, then sign in (`claude login`, `codex login`)"
                .into(),
        );
    } else if limited {
        check.level = Level::Warn;
        check.fix = Some("resolve the requirements named above (CLI version or missing dependencies) to drop the limitation".into());
    }
    check
}

/// Inspect the nearest existing ancestor without creating a probe file.
/// Access is checked by the OS (including ACLs), not inferred from mode bits.
fn check_creatable(path: &Path) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let mut ancestor = path;
    loop {
        let probe = if ancestor.as_os_str().is_empty() {
            Path::new(".")
        } else {
            ancestor
        };
        match std::fs::metadata(probe) {
            Ok(metadata) => {
                anyhow::ensure!(metadata.is_dir(), "{} is not a directory", probe.display());
                rustix::fs::accessat(
                    rustix::fs::CWD,
                    probe,
                    rustix::fs::Access::WRITE_OK | rustix::fs::Access::EXEC_OK,
                    rustix::fs::AtFlags::EACCESS,
                )
                .with_context(|| format!("write/search access to {}", probe.display()))?;
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // A dangling symlink is not a future directory the daemon can create.
                match std::fs::symlink_metadata(probe) {
                    Ok(_) => anyhow::bail!("{} is a dangling symlink", probe.display()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).with_context(|| format!("inspect {}", probe.display()));
                    }
                }
            }
            Err(error) => {
                return Err(error).with_context(|| format!("inspect {}", probe.display()));
            }
        }
        ancestor = ancestor
            .parent()
            .ok_or_else(|| anyhow::anyhow!("{} has no reachable ancestor", path.display()))?;
    }
}

/// Where the daemon's log is written now, so nobody reads a file it has
/// stopped writing to.
pub fn check_logs(log: &Path) -> Check {
    match crate::logging::check_log_file(log).and_then(|meta| meta.modified()) {
        Ok(at) => Check::ok(
            "logs",
            format!(
                "daemon writes {} · last at {}",
                log.display(),
                aether_core::time::Timestamp::from(at).to_rfc3339()
            ),
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match check_creatable(log) {
            Ok(()) => Check::ok("logs", format!("daemon will write {}", log.display())),
            Err(error) => Check::fail(
                "logs",
                format!("cannot create {}: {error:#}", log.display()),
                "check the data directory's permissions and destination",
            ),
        },
        Err(e) => Check::warn(
            "logs",
            format!("cannot use {}: {e}", log.display()),
            "check the log destination and data directory's permissions",
        ),
    }
}

/// Configured MCP servers, and whether the daemon reached them.
pub fn check_mcp(
    configured: usize,
    health: &[aether_core::mcp::McpServerHealth],
    command: &str,
) -> Check {
    use aether_core::mcp::McpServerState;
    if configured == 0 && health.is_empty() {
        return Check::ok("mcp", "none configured");
    }
    if health.is_empty() {
        return Check::warn(
            "mcp",
            format!("{configured} configured; no daemon to say whether they answer"),
            format!("start the daemon and run `{command} mcp list`"),
        );
    }
    let failed: Vec<String> = health
        .iter()
        .filter(|h| h.state == McpServerState::Failed)
        .map(|h| h.name.clone())
        .collect();
    let unknown: Vec<_> = health
        .iter()
        .filter(|h| h.state == McpServerState::Unknown)
        .map(|h| h.name.as_str())
        .collect();
    let ready = health
        .iter()
        .filter(|h| h.state == McpServerState::Ready)
        .count();
    let mut details = vec![if ready == health.len() {
        format!("{ready} server(s) connected")
    } else {
        format!("{ready} of {} server(s) connected", health.len())
    }];
    if configured != health.len() {
        details.push(format!(
            "config/daemon differ: {configured} locally configured, {} reported by daemon",
            health.len()
        ));
    }
    if ready == health.len() {
        return Check::ok("mcp", details.join(" · "));
    }
    if !failed.is_empty() {
        details.push(format!("not answering: {}", failed.join(", ")));
    }
    if !unknown.is_empty() {
        details.push(format!("not contacted yet: {}", unknown.join(", ")));
    }
    Check::warn(
        "mcp",
        details.join(" · "),
        format!(
            "`{command} mcp refresh <server>` retries; their tools are withheld until one does"
        ),
    )
}

/// Remote access: whether the phone listener is open, reachable by name, and
/// has someone to talk to. `None` is a daemon that could not say.
pub fn check_remote(status: Option<&RemoteStatus>, command: &str) -> Check {
    let Some(status) = status else {
        return Check::ok("remote", "not checked: no daemon answered");
    };
    if !status.settings.enabled {
        return Check::ok(
            "remote",
            format!("off; `{command} remote setup` turns on phone access"),
        );
    }
    if let Some(error) = &status.error {
        return Check::fail(
            "remote",
            format!("enabled, but the listener is not open: {error}"),
            format!(
                "fix that, then `{command} remote status`; `{command} remote setup` re-checks every step"
            ),
        );
    }
    let Some(listening) = &status.listening else {
        return Check::warn(
            "remote",
            "enabled, but the listener is not open yet",
            format!("`{command} remote status` shows when it is"),
        );
    };
    let origins = &status.settings.public_origins;
    if origins.is_empty() {
        return Check::warn(
            "remote",
            format!(
                "listening on {listening}, but no public address is allowed, so phones are refused"
            ),
            format!("`{command} remote setup` publishes one"),
        );
    }
    let detail = format!(
        "listening on {listening} · {} · {} device(s) paired",
        origins.join(", "),
        status.devices
    );
    if status.devices == 0 {
        return Check::warn(
            "remote",
            detail,
            format!("`{command} remote pair` adds a phone"),
        );
    }
    Check::ok("remote", detail)
}

#[cfg(test)]
mod tests;
