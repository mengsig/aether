//! `aether update`: replace this binary with a released one.
//!
//! It downloads the assets `install.sh` downloads and checks the same
//! SHA-256. Requests are made only because the person ran the command: there
//! is no background check and nothing is reported anywhere.

mod daemon;
mod fetch;
mod install;
mod release;

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests;

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail, ensure};
use daemon::Daemon;
use install::{Installation, Refusal};
pub use release::Version;
use release::{Archive, ReleaseSource};

use super::command_hint;

/// Exit code of `aether update --check` when a newer release exists.
pub const UPDATE_AVAILABLE_EXIT_CODE: i32 = 10;
const UPDATE_COMMAND: &str = "aether update";

/// What `aether update` was asked for.
#[derive(Debug)]
pub struct UpdateOptions {
    /// Only report whether a newer release exists.
    pub check: bool,
    /// Install this release instead of the newest one.
    pub version: Option<Version>,
    /// Print the outcome as one object, with no progress lines.
    pub json: bool,
}

/// What installing `wanted` over `running` amounts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    UpToDate,
    /// The newest release is older than this build; only a pin goes back.
    AheadOfLatest,
    Upgrade,
    Downgrade,
}

impl Plan {
    fn replaces_the_binary(self) -> bool {
        matches!(self, Self::Upgrade | Self::Downgrade)
    }

    /// The word `--json` reports for it.
    fn action(self) -> &'static str {
        match self {
            Self::UpToDate | Self::AheadOfLatest => "none",
            Self::Upgrade => "upgraded",
            Self::Downgrade => "downgraded",
        }
    }
}

fn plan(running: Version, wanted: Version, pinned: bool) -> Plan {
    match wanted.cmp(&running) {
        Ordering::Equal => Plan::UpToDate,
        Ordering::Greater => Plan::Upgrade,
        Ordering::Less if pinned => Plan::Downgrade,
        Ordering::Less => Plan::AheadOfLatest,
    }
}

/// Run `aether update` and return its exit code.
pub async fn update(
    socket: PathBuf,
    explicit_socket: bool,
    options: UpdateOptions,
) -> anyhow::Result<i32> {
    let running = Version::parse(env!("CARGO_PKG_VERSION"))
        .context("this build's own version is not a release version")?;
    let pinned = options.version;
    ensure!(
        !(options.check && pinned.is_some()),
        "--check reports on the newest release and takes no --version"
    );
    let source = ReleaseSource::from_env()?;
    let client = fetch::client(&source)?;
    if options.check {
        let latest = latest_version(&client, &source).await?;
        return report_check(running, latest, options.json);
    }
    // Refused before any request: a binary that is not ours to replace needs no download.
    let installation = Installation::of_running_binary()?;
    let wanted = match pinned {
        Some(version) => version,
        None => latest_version(&client, &source).await?,
    };
    let plan = plan(running, wanted, pinned.is_some());
    if !plan.replaces_the_binary() {
        let outcome = outcome(plan, running, wanted);
        super::acted(options.json, &announcement(plan, running, wanted), outcome)?;
        return Ok(0);
    }
    let aether = command_hint(&socket, explicit_socket);
    // Asked while this is still the build the daemon accepts, so it can still be stopped.
    let daemon_before = daemon::probe(&socket).await;
    if let Daemon::AskOnly(reason) = &daemon_before {
        bail!(stop_first(&socket, &aether, pinned, reason));
    }
    let target = release::target_for(std::env::consts::OS, std::env::consts::ARCH)?;
    let archive = source.archive(wanted, target);
    if !options.json {
        println!("{}", announcement(plan, running, wanted));
        println!("Downloading {}", archive.url);
    }
    if let Some(left_behind) = replace_binary(&client, &installation, wanted, &archive).await? {
        eprintln!("Warning: {}", super::sane(&left_behind));
    }
    let installed = Installed {
        outcome: outcome(plan, running, wanted),
        binary: installation.binary(),
        daemon_before,
        daemon: daemon::probe(&socket).await,
        pid_file: aether_protocol::paths::pid_path_for(&socket),
        aether,
        old: running,
        new: wanted,
    };
    installed.print(options.json)?;
    Ok(0)
}

/// Why nothing was installed: where a refusing daemon cannot be signalled,
/// replacing the binary would remove the only build that can still stop it.
fn stop_first(socket: &Path, aether: &str, pinned: Option<Version>, reason: &str) -> String {
    let pin = pinned.map_or_else(String::new, |version| {
        format!(" --version {}", version.tag())
    });
    format!(
        "aether was not updated: the daemon on {} has to be stopped first, because here a \
         build with another protocol version cannot stop it ({reason}). Run `{aether} stop`, \
         then `{aether} update{pin}` again; agents resume when a daemon next starts.",
        socket.display()
    )
}

/// What is about to happen, or why nothing will.
fn announcement(plan: Plan, running: Version, wanted: Version) -> String {
    match plan {
        Plan::UpToDate => format!("aether {running} is already installed; nothing to do."),
        Plan::AheadOfLatest => format!(
            "aether {running} is newer than the latest release, {wanted}; nothing to do. \
             `aether update --version {}` installs that release.",
            wanted.tag()
        ),
        Plan::Upgrade => format!("Updating aether {running} to {wanted}."),
        Plan::Downgrade => format!(
            "Downgrading aether {running} to {wanted}, an older release. A store that \
             {running} has migrated may be refused by {wanted} (docs/migration-backups.md)."
        ),
    }
}

/// The `--json` fields every outcome of an update has.
fn outcome(plan: Plan, running: Version, wanted: Version) -> serde_json::Value {
    serde_json::json!({
        "current": running.to_string(),
        "version": wanted.to_string(),
        "action": plan.action(),
    })
}

/// Say whether a newer release exists, and return the exit code that says it.
fn report_check(running: Version, latest: Version, json: bool) -> anyhow::Result<i32> {
    let available = plan(running, latest, false) == Plan::Upgrade;
    let command = if available {
        Some(update_command()?)
    } else {
        None
    };
    let value = serde_json::json!({
        "current": running.to_string(),
        "latest": latest.to_string(),
        "update_available": available,
        "update_command": command,
    });
    super::acted(json, &check_line(running, latest, command), value)?;
    Ok(if available {
        UPDATE_AVAILABLE_EXIT_CODE
    } else {
        0
    })
}

/// How this binary gets a newer release: `aether update`, or the command
/// that updates a binary `aether update` refuses. Makes no request.
fn update_command() -> anyhow::Result<&'static str> {
    let refused = match Installation::of_running_binary() {
        Ok(_) => return Ok(UPDATE_COMMAND),
        Err(refused) => refused,
    };
    let refusal = refused
        .downcast::<Refusal>()
        .context("could not tell how this binary was installed")?;
    Ok(refusal.command)
}

/// What `--check` says; `command` installs the newer release, when there is one.
fn check_line(running: Version, latest: Version, command: Option<&str>) -> String {
    let Some(command) = command else {
        return match latest.cmp(&running) {
            Ordering::Less => {
                format!("aether {running} is newer than the latest release, {latest}.")
            }
            Ordering::Equal | Ordering::Greater => format!("aether {running} is up to date."),
        };
    };
    let available = format!("aether {latest} is available; this is {running}.");
    if command == UPDATE_COMMAND {
        return format!("{available} Run `{command}` to install it.");
    }
    format!("{available} `{UPDATE_COMMAND}` leaves this binary alone; install it with `{command}`.")
}

/// The newest published release, read from its `VERSION` asset.
async fn latest_version(
    client: &reqwest::Client,
    source: &ReleaseSource,
) -> anyhow::Result<Version> {
    let url = source.latest_url(release::VERSION_ASSET);
    let text = fetch::text(client, &url)
        .await
        .context("could not learn the latest release")?;
    Version::parse(text.trim()).with_context(|| format!("{url} does not name a release"))
}

/// Download, verify and install `version`. Until the final rename succeeds,
/// every failure leaves the installed binary as it was. After it, the update
/// has happened: staging that cannot be removed is returned as a warning.
async fn replace_binary(
    client: &reqwest::Client,
    installation: &Installation,
    version: Version,
    archive: &Archive,
) -> anyhow::Result<Option<String>> {
    let staging = installation.staging()?;
    stage_and_swap(client, installation, version, archive, staging.path())
        .await
        .with_context(|| {
            format!(
                "aether was not updated; {} is unchanged",
                installation.binary().display()
            )
        })?;
    let left_behind = staging.path().to_path_buf();
    Ok(staging.close().err().map(|error| {
        format!(
            "aether was updated, but its staging directory {} could not be removed: {error}",
            left_behind.display()
        )
    }))
}

async fn stage_and_swap(
    client: &reqwest::Client,
    installation: &Installation,
    version: Version,
    archive: &Archive,
    staging: &Path,
) -> anyhow::Result<()> {
    let published = fetch::text(client, &archive.checksum_url).await?;
    let expected = release::published_digest(&published, &archive.name)?;
    let archive_path = staging.join(&archive.name);
    let actual = fetch::file(client, &archive.url, &archive_path).await?;
    ensure!(
        actual == expected,
        "checksum mismatch for {}: expected {expected}, got {actual}",
        archive.name
    );
    let unpacked = install::unpack(&archive_path, staging).await?;
    install::check_runs_as(&unpacked, version).await?;
    installation.replace_with(&unpacked)
}

/// A finished update, said as two lines or as one `--json` object.
struct Installed<'a> {
    outcome: serde_json::Value,
    binary: &'a Path,
    /// What held the socket before anything was downloaded.
    daemon_before: Daemon,
    /// What holds the socket now that the binary is replaced.
    daemon: Daemon,
    pid_file: PathBuf,
    /// How this daemon's `aether` is typed, `--socket` included.
    aether: String,
    old: Version,
    new: Version,
}

impl Installed<'_> {
    fn print(mut self, json: bool) -> anyhow::Result<()> {
        if !json {
            println!(
                "Installed aether {} at {}.",
                self.new,
                self.binary.display()
            );
            let notice = restart_notice(
                &self.daemon_before,
                &self.daemon,
                &self.aether,
                &self.pid_file,
                self.old,
                self.new,
            );
            println!("{}", super::sane(&notice));
            return Ok(());
        }
        let restart = if self.daemon.is_running() {
            vec![format!("{} stop", self.aether), self.aether.clone()]
        } else {
            Vec::new()
        };
        self.outcome["binary"] = serde_json::json!(self.binary.display().to_string());
        self.outcome["daemon_running"] = serde_json::json!(self.daemon.is_running());
        self.outcome["restart"] = serde_json::json!(restart);
        println!("{}", serde_json::to_string_pretty(&self.outcome)?);
        Ok(())
    }
}

/// The one thing left to do after the binary changed: a running daemon is
/// still the old build, and serves only its own protocol version and older.
/// `before` is what held the socket when the update began, `now` what holds it.
fn restart_notice(
    before: &Daemon,
    now: &Daemon,
    aether: &str,
    pid_file: &Path,
    old: Version,
    new: Version,
) -> String {
    let by_hand = format!(
        "run `{aether} stop`, then `{aether}`. If `stop` cannot signal it, end the process \
         whose ID is in {} first.",
        pid_file.display()
    );
    match (before, now) {
        (_, Daemon::Absent) => format!("No daemon is running: the next `{aether}` starts {new}."),
        (_, Daemon::Signallable) => format!(
            "The running daemon is still aether {old}: run `{aether} stop`, then `{aether}` to \
             start {new} (agents resume when it starts again)."
        ),
        (Daemon::Absent, Daemon::AskOnly(_)) => {
            format!("A daemon started during the update and is not aether {new}: {by_hand}")
        }
        // A daemon was there before the update, so nothing says this one is new.
        (_, Daemon::AskOnly(reason)) => format!(
            "The daemon could not be checked again after the update ({reason}) and is not \
             aether {new}: {by_hand}"
        ),
    }
}
