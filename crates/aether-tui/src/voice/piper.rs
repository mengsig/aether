//! Confirmed, isolated Piper installation. No synthesis or audio devices are used here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::watch;

use super::config::Config;
use super::process::{self, Control};
use super::setup;

pub(super) const NOTICE: &str = "Downloads hash-pinned piper-tts 1.8.0 and CPU runtime wheels from PyPI, plus en_US-ljspeech-medium (~61 MiB) from Hugging Face rhasspy/piper-voices. Piper: GPL-3.0-or-later; LJ Speech dataset: public domain in the US. Needs CPython 3.11–3.14 with venv/pip and FFmpeg; Linux glibc 2.27+ x86_64/aarch64 or macOS 14+ arm64 wheels only. Allow ~500 MiB disk. Private environment; transcription settings preserved. No recording or playback. Installing does not enable speech or open the microphone.";
const CHECK_PYTHON: &str = r#"
import platform, sys, sysconfig
version = sys.version_info[:2]
if sys.implementation.name != 'cpython' or not (3, 11) <= version <= (3, 14) or sysconfig.get_config_var('Py_GIL_DISABLED'):
    sys.exit('Piper setup needs standard CPython 3.11 through 3.14 (not free-threaded)')
machine = platform.machine().lower()
if sys.platform == 'linux':
    libc, release = platform.libc_ver()
    if machine not in ('x86_64', 'aarch64') or libc != 'glibc' or tuple(map(int, release.split('.')[:2])) < (2, 27):
        sys.exit('Piper setup needs Linux glibc 2.27+ on x86_64/aarch64')
elif sys.platform == 'darwin':
    if machine != 'arm64' or int(platform.mac_ver()[0].split('.')[0]) < 14:
        sys.exit('Piper setup needs macOS 14+ on Apple Silicon; Intel Mac wheels are unavailable')
else:
    sys.exit('Piper guided setup supports Linux and Apple Silicon macOS only')
"#;
const REQUIREMENTS: &str = include_str!("piper-requirements.txt");
const TIMEOUT: Duration = Duration::from_secs(15 * 60);
// The ONNX digest is the Git LFS oid at this immutable revision; JSON/card hashes
// were verified from the same revision, never from a mutable download URL.
const VOICE_REVISION: &str = "375a0fe641dea077c2a47b4e9a056d6da521eed3";
const VOICE_NAME: &str = "en_US-ljspeech-medium.onnx";
const ASSETS: [Asset; 3] = [
    Asset {
        name: VOICE_NAME,
        bytes: 63_531_379,
        sha256: "6f52a751e2349abe7a76735eb09dc1875298c77ea2342ffd2fef79ff81b87f22",
    },
    Asset {
        name: "en_US-ljspeech-medium.onnx.json",
        bytes: 4_972,
        sha256: "141d612cc0a95ed7efc1ca936b845c2364967f2e9217c5dbfcf69fc4d6c65860",
    },
    Asset {
        name: "MODEL_CARD",
        bytes: 517,
        sha256: "fbee1529c89d36b3fe76d7e9f3f832dce17f44900a52d76a9bda735654766b4d",
    },
];

struct Asset {
    name: &'static str,
    bytes: u64,
    sha256: &'static str,
}

impl Asset {
    fn url(&self) -> String {
        format!(
            "https://huggingface.co/rhasspy/piper-voices/resolve/{VOICE_REVISION}/en/en_US/ljspeech/medium/{}",
            self.name
        )
    }

    fn verify(&self, bytes: u64, sha256: &str) -> anyhow::Result<()> {
        if bytes != self.bytes || sha256 != self.sha256 {
            bail!(
                "{} size or SHA-256 mismatch; refusing to install",
                self.name
            );
        }
        Ok(())
    }
}

async fn download_inner(url: &str, destination: &Path, asset: &Asset) -> anyhow::Result<()> {
    let mut url = url.to_owned();
    for hop in 0..=5 {
        if !url.starts_with("https://") {
            bail!("Piper downloads require HTTPS");
        }
        let target = aether_net::vet(&url, aether_net::AddressPolicy::PublicOnly).await?;
        let client = aether_net::pinned_client(&target, TIMEOUT, "aether-voice-setup")?;
        let mut response = client.get(target.url.clone()).send().await?;
        if response.status().is_redirection() {
            if hop == 5 {
                bail!("too many Piper download redirects");
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .context("Piper redirect missing Location")?
                .to_str()?;
            url = target.url.join(location)?.to_string();
            continue;
        }
        response.error_for_status_ref()?;
        if response
            .content_length()
            .is_some_and(|len| len > asset.bytes)
        {
            bail!("Piper download exceeds its size limit");
        }
        let mut file = tokio::fs::File::create(destination).await?;
        let mut hash = Sha256::new();
        let mut total = 0_u64;
        while let Some(chunk) = response.chunk().await? {
            total += chunk.len() as u64;
            if total > asset.bytes {
                bail!("Piper download exceeds its size limit");
            }
            hash.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        asset.verify(total, &hex::encode(hash.finalize()))?;
        return Ok(());
    }
    bail!("Piper download redirect limit exceeded")
}

async fn download(
    dir: &Path,
    asset: &Asset,
    control: &mut watch::Receiver<Control>,
) -> anyhow::Result<()> {
    setup::check_cancel(control)?;
    let url = asset.url();
    let destination = dir.join(asset.name);
    tokio::select! {
        biased;
        _ = control.changed() => bail!("voice setup cancelled"),
        result = tokio::time::timeout(TIMEOUT, download_inner(&url, &destination, asset)) => {
            result.context("Piper download timed out")?
        }
    }
}

fn merged_config(mut table: toml::Table, ffmpeg: &Path) -> anyhow::Result<toml::Table> {
    if !table.contains_key("ffmpeg") {
        table.insert(
            "ffmpeg".into(),
            ffmpeg.to_str().context("ffmpeg path must be UTF-8")?.into(),
        );
    }
    // Reject invalid/unknown settings before any download rather than losing them.
    let _: Config = toml::Value::Table(table.clone())
        .try_into()
        .context("invalid voice.toml; fix it before running setup")?;
    Ok(table)
}

fn save_config(
    prefs: &Path,
    snapshot: Option<&[u8]>,
    mut table: toml::Table,
    piper: &Path,
    model: &Path,
) -> anyhow::Result<()> {
    for (key, path) in [("piper", piper), ("piper_model", model)] {
        table.insert(
            key.into(),
            path.to_str().context("voice paths must be UTF-8")?.into(),
        );
    }
    let body = toml::to_string_pretty(&table).context("serialize voice.toml")?;
    toml::from_str::<Config>(&body)?
        .speech()
        .context("validate installed Piper configuration")?;
    setup::persist_config(prefs, snapshot, &body)
}

fn python_command(python: &Path, root: &Path) -> Command {
    let mut command = Command::new(python);
    command
        .arg("-I")
        .env("TMPDIR", root)
        .env("PIP_CONFIG_FILE", "/dev/null")
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1");
    command
}

fn pip_command(python: &Path, root: &Path, requirements: &Path) -> Command {
    let mut command = python_command(python, root);
    command
        .args([
            "-m",
            "pip",
            "--isolated",
            "--disable-pip-version-check",
            "install",
            "--quiet",
            "--no-input",
            "--no-cache-dir",
            "--only-binary=:all:",
            "--require-hashes",
            "--index-url",
            "https://pypi.org/simple",
            "--timeout",
            "30",
            "--retries",
            "2",
            "--requirement",
        ])
        .arg(requirements);
    command
}

async fn run(
    command: Command,
    timeout: Duration,
    control: &mut watch::Receiver<Control>,
    stage: &str,
) -> anyhow::Result<()> {
    setup::check_cancel(control)?;
    process::run(command, &[], 65_536, timeout, control, false)
        .await
        .with_context(|| {
            format!("{stage}; requires Python 3.11+ venv/pip and compatible CPU wheels")
        })?;
    Ok(())
}

async fn install_runtime(
    python: &Path,
    root: &Path,
    control: &mut watch::Receiver<Control>,
    progress: &mut impl FnMut(String),
) -> anyhow::Result<PathBuf> {
    let mut command = python_command(python, root);
    command.args(["-c", CHECK_PYTHON]);
    run(
        command,
        Duration::from_secs(10),
        control,
        "check Python version",
    )
    .await?;
    let env = root.join("env");
    progress("creating a private Python environment (Piper)".into());
    let mut command = python_command(python, root);
    command.args(["-m", "venv"]).arg(&env);
    run(
        command,
        TIMEOUT,
        control,
        "create private Python environment",
    )
    .await?;
    let interpreter = env.join("bin/python");
    let requirements = root.join("requirements.txt");
    std::fs::write(&requirements, REQUIREMENTS)?;
    progress("installing hash-verified Piper 1.8.0 and CPU runtime wheels from PyPI".into());
    run(
        pip_command(&interpreter, root, &requirements),
        TIMEOUT,
        control,
        "install hash-pinned Piper runtime",
    )
    .await?;
    Ok(interpreter)
}

fn write_launcher(root: &Path, python: &Path) -> anyhow::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    // Keep Python isolated at runtime too; quote paths without interpolating user text.
    let python = python
        .to_str()
        .context("Python path must be UTF-8")?
        .replace('\'', "'\\''");
    let launcher = root.join("piper");
    std::fs::write(
        &launcher,
        format!("#!/bin/sh\nexec '{python}' -I -m piper \"$@\"\n"),
    )?;
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700))?;
    Ok(launcher)
}

/// Called only after setup confirmation. The existing setup lock covers all backends.
pub(super) async fn install(
    prefs_dir: PathBuf,
    control: &mut watch::Receiver<Control>,
    progress: impl FnMut(String),
) -> anyhow::Result<String> {
    setup::check_cancel(control)?;
    let path = std::env::var_os("PATH");
    let python = setup::find_in_path(path.as_deref(), "python3")
        .context("install Python 3.11+ with venv/pip, then retry /voice setup")?;
    let ffmpeg = setup::find_in_path(path.as_deref(), "ffmpeg")
        .context("install FFmpeg, then retry /voice setup")?;
    install_with_tools(prefs_dir, &python, &ffmpeg, control, progress).await
}

async fn install_with_tools(
    prefs_dir: PathBuf,
    python: &Path,
    ffmpeg: &Path,
    control: &mut watch::Receiver<Control>,
    mut progress: impl FnMut(String),
) -> anyhow::Result<String> {
    setup::check_cancel(control)?;
    aether_model::config::create_config_dir(&prefs_dir)?;
    let prefs_dir = std::fs::canonicalize(prefs_dir)?;
    let _lock = setup::lock_setup(&prefs_dir)?;
    let snapshot = setup::config_snapshot(&prefs_dir)?;
    let table = merged_config(setup::parse_config(snapshot.as_deref())?, ffmpeg)?;
    let root = prefs_dir.join("voice");
    std::fs::create_dir_all(&root)?;
    let install = tempfile::Builder::new().prefix("piper-").tempdir_in(root)?;
    let interpreter = install_runtime(python, install.path(), control, &mut progress).await?;
    progress("downloading and verifying LJ Speech voice, JSON and model card (~61 MiB)".into());
    for asset in &ASSETS {
        download(install.path(), asset, control).await?;
    }
    let model = install.path().join(VOICE_NAME);
    progress("checking the local voice loads (no synthesis or audio)".into());
    let mut command = python_command(&interpreter, install.path());
    command.args(["-c", "import sys; from piper.voice import PiperVoice; PiperVoice.load(sys.argv[1], use_cuda=False)"]).arg(&model);
    run(
        command,
        Duration::from_secs(60),
        control,
        "load verified Piper voice",
    )
    .await?;
    let launcher = write_launcher(install.path(), &interpreter)?;
    std::fs::write(
        install.path().join("INSTALL-NOTICE.txt"),
        format!(
            "{NOTICE}\nhttps://pypi.org/project/piper-tts/1.8.0/\nhttps://keithito.com/LJ-Speech-Dataset/\nVoice/model card source: {}\nRuntime licenses are included in env; pinned wheel hashes are in requirements.txt.\n",
            ASSETS[2].url(),
        ),
    )?;
    setup::check_cancel(control)?;
    progress("saving Piper paths; transcription and speech enablement unchanged".into());
    let installed = setup::publish_install(install, || {
        save_config(&prefs_dir, snapshot.as_deref(), table, &launcher, &model)
    })?;
    Ok(format!(
        "Piper 1.8.0 and LJ Speech installed at {}. Transcription settings preserved; speech remains separately opt-in. No audio was recorded or played.",
        installed.display(),
    ))
}

#[cfg(test)]
#[path = "piper_tests.rs"]
mod tests;
