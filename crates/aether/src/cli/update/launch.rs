//! The check `aether` makes on launch: is there a newer release, and does the
//! person want it now. It runs beside startup, is bounded, remembers its
//! answer for a few hours and says nothing when it cannot learn one.

use std::future::Future;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, anyhow, ensure};
use serde::{Deserialize, Serialize};

use super::release::ReleaseSource;
use super::{Installation, UPDATE_COMMAND, UpdateOptions, Version, fetch, latest_version};

/// The state file's name, kept in the data directory.
pub const STATE_FILE: &str = "update-check.json";
/// A cached answer this young is used without asking the network.
const FRESH_FOR_SECS: u64 = 6 * 60 * 60;
/// The whole request for the latest version.
const FETCH_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a stopping daemon has to release its socket.
const STOP_WAIT: Duration = Duration::from_secs(10);
const STOP_POLL: Duration = Duration::from_millis(100);
/// Set on the restart after an update so a stale answer cannot offer it twice.
const RESTARTED_VARIABLE: &str = "AETHER_UPDATE_RESTARTED";
/// Set by whatever manages this Aether, such as Archon, which then owns updates.
const MANAGED_VARIABLE: &str = "AETHER_MANAGED_BY";

/// What the last check learned and what the person turned down.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
struct State {
    checked_at: u64,
    latest: Option<String>,
    skipped: Option<String>,
}

impl State {
    /// A missing or unreadable file is no cache: it only saves a request.
    fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    fn save(&self, path: &Path) -> anyhow::Result<()> {
        let text = serde_json::to_string(self).context("could not encode the update state")?;
        let staged = path.with_extension("json.tmp");
        std::fs::write(&staged, text)
            .with_context(|| format!("could not write {}", staged.display()))?;
        std::fs::rename(&staged, path)
            .with_context(|| format!("could not replace {}", path.display()))
    }

    fn cached_latest(&self, now: u64) -> Option<Version> {
        if now.saturating_sub(self.checked_at) >= FRESH_FOR_SECS {
            return None;
        }
        Version::parse(self.latest.as_deref()?).ok()
    }

    fn is_skipped(&self, version: Version) -> bool {
        self.skipped.as_deref() == Some(version.to_string().as_str())
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The release worth offering: newer than `running` and not skipped. A fresh
/// cache answers without `fetch`; a failed `fetch` is no answer.
async fn newer_release<F, Fut>(
    running: Version,
    state_path: &Path,
    now: u64,
    timeout: Duration,
    fetch: F,
) -> Option<Version>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<Version>>,
{
    let mut state = State::load(state_path);
    let latest = match state.cached_latest(now) {
        Some(latest) => latest,
        None => {
            let latest = match tokio::time::timeout(timeout, fetch()).await {
                Ok(Ok(latest)) => latest,
                Ok(Err(error)) => {
                    tracing::debug!("update check failed: {error:#}");
                    return None;
                }
                Err(_) => {
                    tracing::debug!("update check timed out");
                    return None;
                }
            };
            state.checked_at = now;
            state.latest = Some(latest.to_string());
            if let Err(error) = state.save(state_path) {
                tracing::debug!("could not remember the update check: {error:#}");
            }
            latest
        }
    };
    (latest > running && !state.is_skipped(latest)).then_some(latest)
}

/// A check running beside startup.
pub struct LaunchCheck {
    task: tokio::task::JoinHandle<Option<Version>>,
    state_path: PathBuf,
}

impl LaunchCheck {
    /// Begin the check, unless the config turns it off or something else
    /// manages updates. Never fails: a check that cannot start is no check.
    pub fn start(config_dir: &Path, state_dir: &Path) -> Option<Self> {
        let off = |why: &str| {
            tracing::debug!("no update check on launch: {why}");
            None
        };
        if env_set(MANAGED_VARIABLE) {
            return off("another tool manages updates");
        }
        if env_set(RESTARTED_VARIABLE) {
            return off("this launch follows an update");
        }
        match aether_model::config::load_config(config_dir) {
            Ok(config) if !config.update.check => return off("[update] check is false"),
            Ok(_) => {}
            Err(error) => return off(&format!("config.toml is unreadable: {error}")),
        }
        let running = Version::parse(env!("CARGO_PKG_VERSION")).ok()?;
        let source = ReleaseSource::from_env().ok()?;
        let client = fetch::client(&source).ok()?;
        let state_path = state_dir.join(STATE_FILE);
        let path = state_path.clone();
        let task = tokio::spawn(async move {
            let latest = || latest_version(&client, &source);
            newer_release(running, &path, now_secs(), FETCH_TIMEOUT, latest).await
        });
        Some(Self { task, state_path })
    }

    /// The newer release, if the check has already found one. Startup never
    /// waits: an answer that is late is cached for the next launch.
    async fn newer(self) -> Option<Version> {
        if !self.task.is_finished() {
            return None;
        }
        match self.task.await {
            Ok(found) => found,
            Err(error) => {
                tracing::debug!("update check task failed: {error}");
                None
            }
        }
    }
}

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Yes,
    No,
    Skip,
}

/// Ask on `prompt`; an empty line agrees, and so does nothing but `y`. No
/// answer at all (a closed input) is a no: nothing updates without a yes.
fn ask(
    answer: &mut impl BufRead,
    prompt: &mut impl Write,
    latest: Version,
) -> anyhow::Result<Answer> {
    write!(
        prompt,
        "Aether {latest} is available. Update and restart now? [Y/n] (s = skip {latest}) "
    )
    .and_then(|()| prompt.flush())
    .context("could not ask about the update")?;
    let mut line = String::new();
    let read = answer
        .read_line(&mut line)
        .context("could not read the answer about the update")?;
    if read == 0 {
        return Ok(Answer::No);
    }
    Ok(match line.trim().to_ascii_lowercase().as_str() {
        "" | "y" | "yes" => Answer::Yes,
        "s" | "skip" => Answer::Skip,
        _ => Answer::No,
    })
}

/// Remember that `version` was turned down, so it is not offered again.
fn skip(state_path: &Path, version: Version) -> anyhow::Result<()> {
    let mut state = State::load(state_path);
    state.skipped = Some(version.to_string());
    state.save(state_path)
}

/// Offer the newer release the check found, if any, and on a yes update,
/// stop the daemon and start the new build with the same arguments. Returns
/// only when the launch carries on with this build.
pub async fn offer(
    check: LaunchCheck,
    endpoint: aether_client::Endpoint,
    explicit_socket: bool,
) -> anyhow::Result<()> {
    let state_path = check.state_path.clone();
    let Some(latest) = check.newer().await else {
        return Ok(());
    };
    let Some(socket) = endpoint.local_path().map(Path::to_path_buf) else {
        return Ok(());
    };
    let command = super::update_command()?;
    if command != UPDATE_COMMAND {
        eprintln!("Aether {latest} is available; install it with `{command}`.");
        return Ok(());
    }
    let answer = ask(&mut std::io::stdin().lock(), &mut std::io::stderr(), latest)?;
    match answer {
        Answer::No => Ok(()),
        Answer::Skip => {
            match skip(&state_path, latest) {
                Ok(()) => eprintln!("Skipping {latest}; the next release will be offered."),
                Err(error) => eprintln!("Could not remember skipping {latest}: {error:#}"),
            }
            Ok(())
        }
        Answer::Yes => update_and_restart(endpoint, socket, explicit_socket).await,
    }
}

/// Stop the daemon with this (still compatible) build, update with nothing
/// holding the socket, then start the new build. A failed update restarts
/// the current build, since its daemon is gone.
async fn update_and_restart(
    endpoint: aether_client::Endpoint,
    socket: PathBuf,
    explicit_socket: bool,
) -> anyhow::Result<()> {
    let binary = Installation::of_running_binary()?.binary().to_path_buf();
    if let Err(error) = stop_daemon(endpoint, &socket).await {
        // The daemon may be gone or going, so this client cannot be trusted: start afresh.
        eprintln!("Not updated: {error:#}\nRestarting the current build.");
        return restart_as(&binary);
    }
    let options = UpdateOptions {
        check: false,
        version: None,
        json: false,
        yes: false,
    };
    if let Err(error) = super::run_update(socket, explicit_socket, options).await {
        eprintln!("Not updated: {error:#}\nStarting the current build.");
    }
    restart_as(&binary)
}

/// Whether a daemon still listens on `socket` or holds its lock: it keeps
/// the lock through its own cleanup, after the listener is gone.
async fn stopping(socket: &Path) -> anyhow::Result<bool> {
    if super::daemon::probe(socket).await.is_running() {
        return Ok(true);
    }
    aether_daemon::socket_lock_is_held(socket).context("could not check the daemon's lock")
}

/// Ask the daemon to stop and wait, boundedly, until nothing listens.
async fn stop_daemon(endpoint: aether_client::Endpoint, socket: &Path) -> anyhow::Result<()> {
    super::super::stop_endpoint(endpoint, false)
        .await
        .context("could not stop the daemon")?;
    let deadline = tokio::time::Instant::now() + STOP_WAIT;
    while stopping(socket).await? {
        ensure!(
            tokio::time::Instant::now() < deadline,
            "the daemon on {} did not stop within {}s",
            socket.display(),
            STOP_WAIT.as_secs()
        );
        tokio::time::sleep(STOP_POLL).await;
    }
    Ok(())
}

/// Replace this process with `binary` run with the arguments this one got.
fn restart_as(binary: &Path) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt as _;

    ensure!(
        binary.is_file(),
        "{} is gone after the update",
        binary.display()
    );
    let error = std::process::Command::new(binary)
        .args(std::env::args_os().skip(1))
        .env(RESTARTED_VARIABLE, "1")
        .exec();
    Err(anyhow!(error).context(format!("could not restart {}", binary.display())))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).expect("version")
    }

    fn state_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(STATE_FILE);
        (dir, path)
    }

    async fn found(path: &Path, now: u64, latest: &'static str) -> Option<Version> {
        let fetch = || async move { Ok(version(latest)) };
        newer_release(version("1.2.1"), path, now, FETCH_TIMEOUT, fetch).await
    }

    #[tokio::test]
    async fn a_newer_release_is_offered_and_remembered() {
        let (_dir, path) = state_path();
        assert_eq!(found(&path, 1000, "1.3.0").await, Some(version("1.3.0")));
        let state = State::load(&path);
        assert_eq!(state.latest.as_deref(), Some("1.3.0"));
        assert_eq!(state.checked_at, 1000);
    }

    #[tokio::test]
    async fn the_same_or_an_older_release_is_not_offered() {
        let (_dir, path) = state_path();
        assert_eq!(found(&path, 1000, "1.2.1").await, None);
        assert_eq!(found(&path, 1000 + FRESH_FOR_SECS, "1.1.0").await, None);
    }

    #[tokio::test]
    async fn a_fresh_cache_makes_no_request_and_a_stale_one_does() {
        let (_dir, path) = state_path();
        found(&path, 1000, "1.3.0").await;
        let no_request = || async { Err(anyhow!("must not ask the network")) };
        let cached = newer_release(
            version("1.2.1"),
            &path,
            1000 + 60,
            FETCH_TIMEOUT,
            no_request,
        )
        .await;
        assert_eq!(cached, Some(version("1.3.0")));
        assert_eq!(
            found(&path, 1000 + FRESH_FOR_SECS, "1.4.0").await,
            Some(version("1.4.0"))
        );
    }

    #[tokio::test]
    async fn offline_is_silent_and_leaves_no_cache() {
        let (_dir, path) = state_path();
        let offline = || async { Err(anyhow!("network is unreachable")) };
        assert_eq!(
            newer_release(version("1.2.1"), &path, 1, FETCH_TIMEOUT, offline).await,
            None
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn a_request_that_hangs_is_given_up_on() {
        let (_dir, path) = state_path();
        let hangs = || std::future::pending::<anyhow::Result<Version>>();
        assert_eq!(
            newer_release(version("1.2.1"), &path, 1, Duration::from_millis(50), hangs).await,
            None
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn a_skipped_version_is_not_offered_but_the_next_one_is() {
        let (_dir, path) = state_path();
        skip(&path, version("1.3.0")).expect("skip");
        assert_eq!(found(&path, 1000, "1.3.0").await, None);
        assert_eq!(
            found(&path, 1000 + FRESH_FOR_SECS, "1.3.1").await,
            Some(version("1.3.1"))
        );
    }

    #[test]
    fn an_unreadable_state_file_is_no_cache() {
        let (_dir, path) = state_path();
        std::fs::write(&path, "{not json").expect("write");
        assert_eq!(State::load(&path), State::default());
    }

    fn answered(input: &str) -> Answer {
        let mut prompt = Vec::new();
        let answer = ask(&mut input.as_bytes(), &mut prompt, version("1.3.0")).expect("ask");
        let shown = String::from_utf8(prompt).expect("utf8");
        assert!(shown.contains("Aether 1.3.0 is available. Update and restart now? [Y/n]"));
        answer
    }

    #[test]
    fn only_a_yes_or_an_empty_line_updates() {
        assert_eq!(answered("\n"), Answer::Yes);
        assert_eq!(answered("Y\n"), Answer::Yes);
        assert_eq!(answered("yes\n"), Answer::Yes);
        assert_eq!(answered("n\n"), Answer::No);
        assert_eq!(answered("what\n"), Answer::No);
        assert_eq!(answered(""), Answer::No, "a closed input is not a yes");
        assert_eq!(answered("s\n"), Answer::Skip);
    }

    #[tokio::test]
    async fn startup_does_not_wait_for_an_unfinished_check() {
        let task = tokio::spawn(std::future::pending::<Option<Version>>());
        let check = LaunchCheck {
            task,
            state_path: PathBuf::new(),
        };
        let started = std::time::Instant::now();
        assert_eq!(check.newer().await, None);
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn config_off_and_managed_installs_start_no_check() {
        let config = tempfile::tempdir().expect("tempdir");
        let state = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            config.path().join("config.toml"),
            "[update]\ncheck = false\n",
        )
        .expect("write");
        assert!(LaunchCheck::start(config.path(), state.path()).is_none());
        assert!(!state.path().join(STATE_FILE).exists());
    }
}
