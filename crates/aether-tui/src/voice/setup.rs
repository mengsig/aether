//! Confirmed local Whisper installation with a pinned source commit and model checksum.
//! Parakeet and Piper use separate private Python environments in their own modules.

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use tokio::process::Command;
use tokio::sync::watch;

mod download;
use download::download_model;

use super::config::{self, Config};
use super::process::{self, Control};

/// Pinning both the tag and the commit it resolves to means a shallow clone
/// is still checked against a hash, even though its own history is not.
const WHISPER_TAG: &str = "v1.9.4";
const WHISPER_COMMIT: &str = "927cfce34f31707e17f2bff35c349632fb9e2c3a";
const WHISPER_REPO: &str = "https://github.com/ggml-org/whisper.cpp.git";

const CLONE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const BUILD_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// A model/backend choice offered by the setup popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    Tiny,
    Base,
    Small,
    /// Parakeet TDT 0.6B v3 via sherpa-onnx; delegates entirely to `sherpa::install`.
    Parakeet,
    /// Local speech output; leaves the active transcription backend unchanged.
    Piper,
}

impl ModelKind {
    pub const ALL: [ModelKind; 5] = [
        Self::Tiny,
        Self::Base,
        Self::Small,
        Self::Parakeet,
        Self::Piper,
    ];

    pub fn speech_output(self) -> bool {
        self == Self::Piper
    }

    pub fn notice(self) -> &'static str {
        match self {
            Self::Tiny | Self::Base | Self::Small => {
                "Downloads whisper.cpp from GitHub and a model from Hugging Face. Commit/model checksum verified. MIT license. Requires Git, CMake, C/C++ tools and FFmpeg; no sudo."
            }
            Self::Parakeet => {
                "Downloads from GitHub and PyPI; model SHA-256 verified. Private Python packages: sherpa-onnx 1.13.8 + NumPy 2.4.3. Model: NVIDIA CC-BY-4.0; engine: Apache-2.0. Needs Python 3.11+ and FFmpeg; ~640 MiB model disk plus runtime."
            }
            Self::Piper => super::piper::NOTICE,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Tiny => "tiny",
            Self::Base => "base",
            Self::Small => "small",
            Self::Parakeet => "parakeet",
            Self::Piper => "piper",
        }
    }

    /// Size/language guidance shown next to each choice in the popup.
    pub fn blurb(self) -> &'static str {
        match self {
            Self::Tiny => "whisper.cpp tiny · ~75 MiB · fastest, least accurate · multilingual",
            Self::Base => "whisper.cpp base · ~142 MiB · recommended default · multilingual",
            Self::Small => "whisper.cpp small · ~466 MiB · slower, most accurate · multilingual",
            Self::Parakeet => {
                "Parakeet TDT 0.6B v3 · ~465 MiB · sherpa-onnx · 25 European languages"
            }
            Self::Piper => "Piper 1.8.0 · LJ Speech US English · ~61 MiB + Python runtime wheels",
        }
    }

    fn stem(self) -> &'static str {
        match self {
            Self::Tiny | Self::Base | Self::Small => self.label(),
            // install() dispatches non-Whisper backends before reaching these helpers.
            Self::Parakeet | Self::Piper => {
                unreachable!("this backend has no whisper.cpp model stem")
            }
        }
    }

    fn sha1_hex(self) -> &'static str {
        match self {
            Self::Tiny => "bd577a113a864445d4c299885e0cb97d4ba92b5f",
            Self::Base => "465707469ff3a37a2b9b8d8f89f2f99de7299dac",
            Self::Small => "55356645c2b361a969dfd0ef2c5a50d530afd8d5",
            Self::Parakeet | Self::Piper => {
                unreachable!("this backend has no whisper.cpp model checksum")
            }
        }
    }

    fn approx_bytes(self) -> u64 {
        match self {
            Self::Tiny => 75 * 1024 * 1024,
            Self::Base => 142 * 1024 * 1024,
            Self::Small => 466 * 1024 * 1024,
            Self::Parakeet | Self::Piper => {
                unreachable!("this backend has no whisper.cpp download size")
            }
        }
    }
}

/// A missing local tool, with a human next step. Setup never runs a
/// package manager itself; it only names what is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prerequisite {
    pub name: &'static str,
    pub hint: &'static str,
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    use std::os::unix::fs::PermissionsExt;
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}

/// Resolve `bin` on `path_env` without touching global state, so tests can
/// point this at a fake toolchain instead of the real `PATH`.
pub(super) fn find_in_path(path_env: Option<&OsStr>, bin: &str) -> Option<PathBuf> {
    debug_assert!(!bin.is_empty(), "tool name must not be empty");
    let path_env = path_env?;
    std::env::split_paths(path_env)
        .filter(|dir| dir.is_absolute())
        .find_map(|dir| {
            let candidate = dir.join(bin);
            is_executable(&candidate).then_some(candidate)
        })
}

fn missing_prerequisites_in(path_env: Option<&OsStr>) -> Vec<Prerequisite> {
    let mut missing = Vec::new();
    for (bin, name, hint) in [
        ("git", "git", "install git through your package manager"),
        (
            "cmake",
            "cmake",
            "install cmake through your package manager",
        ),
        (
            "ffmpeg",
            "ffmpeg",
            "install ffmpeg with Pulse (Linux) or AVFoundation (macOS) support; see docs/voice.md",
        ),
    ] {
        if find_in_path(path_env, bin).is_none() {
            missing.push(Prerequisite { name, hint });
        }
    }
    let has_compiler = ["cc", "gcc", "clang"]
        .into_iter()
        .any(|bin| find_in_path(path_env, bin).is_some())
        && ["c++", "g++", "clang++"]
            .into_iter()
            .any(|bin| find_in_path(path_env, bin).is_some());
    if !has_compiler {
        missing.push(Prerequisite {
            name: "a C/C++ compiler",
            hint: "install a compiler (e.g. build-essential, gcc or clang)",
        });
    }
    let has_builder = ["make", "ninja"]
        .into_iter()
        .any(|bin| find_in_path(path_env, bin).is_some());
    if !has_builder {
        missing.push(Prerequisite {
            name: "make or ninja",
            hint: "install make (or ninja) through your package manager",
        });
    }
    missing
}

/// What the popup shows before it will offer to build the Whisper backend.
/// Parakeet and Piper require Python/FFmpeg rather than a C++ build toolchain.
pub fn missing_prerequisites(model: ModelKind) -> Vec<Prerequisite> {
    prerequisites_for(model, std::env::var_os("PATH").as_deref())
}

fn prerequisites_for(model: ModelKind, path: Option<&OsStr>) -> Vec<Prerequisite> {
    if !matches!(model, ModelKind::Parakeet | ModelKind::Piper) {
        return missing_prerequisites_in(path);
    }
    [
        ("python3", "install Python 3.11+ with venv/pip"),
        ("ffmpeg", "install FFmpeg with local audio device support"),
    ]
    .into_iter()
    .filter(|(name, _)| find_in_path(path, name).is_none())
    .map(|(name, hint)| Prerequisite { name, hint })
    .collect()
}

pub(super) fn check_cancel(control: &watch::Receiver<Control>) -> anyhow::Result<()> {
    if *control.borrow() != Control::Run {
        bail!("voice setup cancelled");
    }
    Ok(())
}

fn prerequisite_message(missing: &[Prerequisite]) -> String {
    assert!(!missing.is_empty(), "caller must only report actual gaps");
    let mut message = "missing prerequisites:".to_owned();
    for item in missing {
        message.push_str(&format!("\n  - {}: {}", item.name, item.hint));
    }
    message
}

pub(super) fn publish_install(
    install: tempfile::TempDir,
    publish: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<PathBuf> {
    // Disarm temporary cleanup before publishing paths that other processes can read.
    let path = install.keep();
    if let Err(cause) = publish() {
        return match std::fs::remove_dir_all(&path) {
            Ok(()) => Err(cause),
            Err(cleanup) => Err(cause.context(format!(
                "also failed to remove incomplete install {}: {cleanup}",
                path.display()
            ))),
        };
    }
    Ok(path)
}

/// Mutual exclusion between Whisper, Parakeet and Piper setup runs:
/// installers lock the same path before touching voice.toml
/// or their managed directories.
pub(super) struct SetupLock(std::fs::File);

impl Drop for SetupLock {
    fn drop(&mut self) {
        // Closing alone leaves flock held if a concurrent fork inherited the descriptor.
        if let Err(error) = self.0.unlock() {
            tracing::error!(%error, "release voice setup lock");
        }
    }
}

pub(super) fn lock_setup(prefs: &Path) -> anyhow::Result<SetupLock> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(prefs.join(".voice-setup.lock"))
        .context("open voice setup lock")?;
    if !file.metadata()?.is_file() {
        bail!("voice setup lock must be a regular file");
    }
    file.try_lock()
        .context("another voice setup is running; retry after it finishes")?;
    Ok(SetupLock(file))
}

/// The exact bytes of voice.toml right now, or `None` if it is genuinely
/// absent. A dangling symlink or a non-regular-file path is a hard error,
/// never silently treated as "nothing to preserve".
pub(super) fn config_snapshot(prefs: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let path = prefs.join(config::FILE);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
        Ok(meta) if meta.is_file() => config::read_small(&path, 16_384).map(Some),
        Ok(meta) => Err(not_a_regular_file(&path, meta.file_type().is_symlink())),
    }
}

/// Setup replaces voice.toml atomically, which would silently turn a link
/// into a file; the refusal names the path and the way out.
fn not_a_regular_file(path: &Path, link: bool) -> anyhow::Error {
    let path = path.display();
    if link {
        return anyhow::anyhow!(
            "{path} is a link, and setup only rewrites a regular file: replace the link with a copy of the file it points to, then run the install again"
        );
    }
    anyhow::anyhow!(
        "{path} is not a regular file, and setup only rewrites a regular file: move it out of the way, then run the install again"
    )
}

pub(super) fn parse_config(snapshot: Option<&[u8]>) -> anyhow::Result<toml::Table> {
    match snapshot {
        Some(bytes) => {
            toml::from_str(std::str::from_utf8(bytes).context("voice.toml must be UTF-8")?)
                .context("invalid voice.toml; fix it before running setup")
        }
        None => Ok(toml::Table::new()),
    }
}

/// Merges the new Whisper paths into `table`, validates the result through
/// the real strict `Config`, then writes it atomically - refusing if
/// voice.toml changed under us since `snapshot` was taken.
fn save_config(
    prefs: &Path,
    snapshot: Option<&[u8]>,
    mut table: toml::Table,
    ffmpeg: &Path,
    whisper_cli: &Path,
    model_file: &Path,
) -> anyhow::Result<()> {
    for (key, path) in [("whisper", whisper_cli), ("whisper_model", model_file)] {
        table.insert(
            key.into(),
            toml::Value::String(path.to_str().context("voice paths must be UTF-8")?.into()),
        );
    }
    if !table.contains_key("ffmpeg") {
        table.insert(
            "ffmpeg".into(),
            toml::Value::String(ffmpeg.to_str().context("ffmpeg path must be UTF-8")?.into()),
        );
    }
    table.insert("stt_backend".into(), "whisper_cpp".into());
    if !table.contains_key("language") {
        table.insert("language".into(), "auto".into());
    }
    let body = toml::to_string_pretty(&table).context("serialize voice.toml")?;
    if body.len() > 16_384 {
        bail!("updated voice.toml exceeds its 16KiB limit");
    }
    toml::from_str::<Config>(&body)
        .context("validate merged voice.toml")?
        .recording()
        .context("validate the installed Whisper configuration")?;
    persist_config(prefs, snapshot, &body)
}

pub(super) fn persist_config(
    prefs: &Path,
    snapshot: Option<&[u8]>,
    body: &str,
) -> anyhow::Result<()> {
    if body.len() > 16_384 {
        bail!("updated voice.toml exceeds its 16KiB limit");
    }
    let mut temp = tempfile::NamedTempFile::new_in(prefs).context("create a temp config file")?;
    temp.write_all(body.as_bytes())
        .context("write voice.toml")?;
    temp.as_file().sync_all().context("flush voice.toml")?;
    if config_snapshot(prefs)?.as_deref() != snapshot {
        bail!("voice.toml changed while installing; existing changes preserved, retry setup");
    }
    temp.persist(prefs.join(config::FILE))
        .context("save voice.toml")?;
    Ok(())
}

fn log_tail(path: &Path, limit: usize) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let start = bytes.len().saturating_sub(limit);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

async fn run_step(
    command: Command,
    log_path: &Path,
    timeout: Duration,
    control: &mut watch::Receiver<Control>,
) -> anyhow::Result<()> {
    let log = std::fs::File::create(log_path)
        .with_context(|| format!("create {}", log_path.display()))?;
    match process::run_logged(command, &log, timeout, control).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let tail = log_tail(log_path, 2_000);
            if tail.trim().is_empty() {
                Err(error)
            } else {
                Err(error.context(format!("last output:\n{tail}")))
            }
        }
    }
}

/// A clone that resolves to a different commit than [`WHISPER_COMMIT`]
/// means the tag moved or the mirror lied; refuse to build it.
async fn verify_commit(
    git: &Path,
    repo_dir: &Path,
    control: &mut watch::Receiver<Control>,
) -> anyhow::Result<()> {
    let mut rev_parse = Command::new(git);
    rev_parse
        .arg("-C")
        .arg(repo_dir)
        .args(["rev-parse", "HEAD"]);
    let output = process::run(rev_parse, &[], 256, Duration::from_secs(10), control, false)
        .await
        .context("read the cloned commit")?;
    let head = String::from_utf8(output).context("git rev-parse produced non-UTF-8 output")?;
    let head = head.trim();
    assert!(
        WHISPER_COMMIT.len() == 40,
        "pinned commit must be a full SHA-1"
    );
    if head != WHISPER_COMMIT {
        bail!(
            "whisper.cpp {WHISPER_TAG} resolved to commit {head}, not the pinned {WHISPER_COMMIT}; refusing to build an unverified checkout"
        );
    }
    Ok(())
}

/// Clone the pinned tag into `repo_dir` (which must not exist yet - git
/// requires an empty target), verify its commit, then configure and build.
/// Returns the built `whisper-cli` path.
async fn install_backend(
    git: &Path,
    cmake: &Path,
    repo_dir: &Path,
    logs_dir: &Path,
    control: &mut watch::Receiver<Control>,
    progress: &mut impl FnMut(String),
) -> anyhow::Result<PathBuf> {
    check_cancel(control)?;
    progress(format!("cloning whisper.cpp {WHISPER_TAG}"));
    let mut clone = Command::new(git);
    clone
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "clone",
            "--branch",
            WHISPER_TAG,
            "--depth",
            "1",
            "--no-tags",
            WHISPER_REPO,
        ])
        .arg(repo_dir);
    run_step(clone, &logs_dir.join("clone.log"), CLONE_TIMEOUT, control)
        .await
        .context("clone whisper.cpp")?;

    check_cancel(control)?;
    verify_commit(git, repo_dir, control).await?;

    check_cancel(control)?;
    progress("configuring the build (cmake)".into());
    let mut configure = Command::new(cmake);
    configure
        .arg("-B")
        .arg(repo_dir.join("build"))
        .arg("-S")
        .arg(repo_dir)
        .args(["-DCMAKE_BUILD_TYPE=Release", "-DBUILD_SHARED_LIBS=OFF"]);
    if find_in_path(std::env::var_os("PATH").as_deref(), "make").is_none() {
        configure.args(["-G", "Ninja"]);
    }
    run_step(
        configure,
        &logs_dir.join("configure.log"),
        CONFIGURE_TIMEOUT,
        control,
    )
    .await
    .context("configure the whisper.cpp build")?;

    check_cancel(control)?;
    progress("building whisper-cli (this can take a few minutes)".into());
    let mut build = Command::new(cmake);
    build
        .arg("--build")
        .arg(repo_dir.join("build"))
        .args(["--config", "Release", "-j2"]);
    run_step(build, &logs_dir.join("build.log"), BUILD_TIMEOUT, control)
        .await
        .context("build whisper.cpp")?;

    let cli = repo_dir.join("build/bin/whisper-cli");
    if !cli.is_file() {
        bail!(
            "the build finished but whisper-cli was not produced at {}",
            cli.display()
        );
    }
    Ok(cli)
}

/// The whole guided Whisper install: check local tools, clone/build/verify
/// whisper.cpp, download/verify a model, then write voice.toml. Never
/// touches the daemon connection, the microphone, or an `enabled` flag.
/// Parakeet and Piper delegate to their respective installers.
pub(crate) async fn install(
    prefs_dir: PathBuf,
    model: ModelKind,
    control: &mut watch::Receiver<Control>,
    mut progress: impl FnMut(String),
) -> anyhow::Result<String> {
    if model == ModelKind::Parakeet {
        return super::sherpa::install(prefs_dir, control, progress).await;
    }
    if model == ModelKind::Piper {
        return super::piper::install(prefs_dir, control, progress).await;
    }
    check_cancel(control)?;
    progress("checking prerequisites".into());
    let path_env = std::env::var_os("PATH");
    let missing = missing_prerequisites_in(path_env.as_deref());
    if !missing.is_empty() {
        bail!(prerequisite_message(&missing));
    }
    let git = find_in_path(path_env.as_deref(), "git").context("git disappeared from PATH")?;
    let cmake =
        find_in_path(path_env.as_deref(), "cmake").context("cmake disappeared from PATH")?;
    let ffmpeg =
        find_in_path(path_env.as_deref(), "ffmpeg").context("ffmpeg disappeared from PATH")?;

    aether_model::config::create_config_dir(&prefs_dir)
        .with_context(|| format!("create {}", prefs_dir.display()))?;
    let prefs_dir = std::fs::canonicalize(&prefs_dir).context("resolve the config directory")?;
    let _lock = lock_setup(&prefs_dir)?;
    let snapshot = config_snapshot(&prefs_dir)?;
    let table = parse_config(snapshot.as_deref())?;

    let voice_root = prefs_dir.join("voice");
    tokio::fs::create_dir_all(&voice_root)
        .await
        .with_context(|| format!("create {}", voice_root.display()))?;

    // Built directly at its final path (never moved), so a shared-library
    // rpath baked into whisper-cli, if any, stays valid.
    let holder = tempfile::Builder::new()
        .prefix("whisper.cpp-")
        .tempdir_in(&voice_root)
        .context("create an install directory")?;
    let logs_dir = holder.path().to_owned();
    let repo_dir = logs_dir.join("src");

    let whisper_cli =
        install_backend(&git, &cmake, &repo_dir, &logs_dir, control, &mut progress).await?;
    assert!(
        whisper_cli.starts_with(&repo_dir),
        "the built binary must live in the install directory"
    );

    check_cancel(control)?;
    progress(format!(
        "downloading the {} model (~{} MiB)",
        model.label(),
        model.approx_bytes() / (1024 * 1024)
    ));
    let staged_model = logs_dir.join(format!("ggml-{}.bin", model.stem()));
    download_model(model, &staged_model, control).await?;

    check_cancel(control)?;
    progress("saving configuration; voice remains off".into());
    let installed = publish_install(holder, || {
        save_config(
            &prefs_dir,
            snapshot.as_deref(),
            table,
            &ffmpeg,
            &whisper_cli,
            &staged_model,
        )
    })?;
    assert!(
        whisper_cli.starts_with(&installed),
        "keep() must preserve the built path"
    );

    Ok(format!(
        "installed whisper.cpp {WHISPER_TAG} ({} model) at {}; voice.toml updated. \
         Voice stays off until you run /voice check and press F4.",
        model.label(),
        installed.display(),
    ))
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
