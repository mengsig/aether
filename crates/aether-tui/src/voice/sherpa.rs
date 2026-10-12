use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::watch;

use super::config::{self, Config, required};
use super::process::{self, Control};
use super::setup::{config_snapshot, lock_setup, parse_config};

const ADAPTER: &str = include_str!("sherpa_adapter.py");
const MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2";
const MODEL_SHA256: &str = "5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf";
const MODEL_BYTES: u64 = 487_170_055;
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
pub(super) const MODEL_FILES: [&str; 4] = [
    "encoder.int8.onnx",
    "decoder.int8.onnx",
    "joiner.int8.onnx",
    "tokens.txt",
];

pub(super) async fn transcribe(
    config: &Config,
    audio: &Path,
    control: &mut watch::Receiver<Control>,
) -> anyhow::Result<String> {
    config.recording()?;
    let mut command = Command::new(required(&config.sherpa_python, "sherpa_python")?);
    command
        .args(["-I", "-c", ADAPTER, "transcribe"])
        .arg(required(&config.sherpa_model_dir, "sherpa_model_dir")?)
        .arg(audio)
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1");
    let bytes = process::run(
        command,
        &[],
        super::backend::TEXT_LIMIT,
        Duration::from_secs(120),
        control,
        false,
    )
    .await?;
    super::backend::transcript(bytes)
}

fn find_tool(name: &str) -> anyhow::Result<PathBuf> {
    let path = std::env::var_os("PATH").context("PATH is unset")?;
    for dir in std::env::split_paths(&path).filter(|dir| dir.is_absolute()) {
        let candidate = dir.join(name);
        if let Ok(meta) = candidate.metadata() {
            use std::os::unix::fs::PermissionsExt;
            if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                return Ok(candidate);
            }
        }
    }
    bail!("install {name} through your system package manager, then retry /voice setup")
}

fn cancelled(control: &watch::Receiver<Control>) -> anyhow::Result<()> {
    if *control.borrow() != Control::Run {
        bail!("voice setup cancelled");
    }
    Ok(())
}

fn save_config(
    prefs: &Path,
    snapshot: Option<&[u8]>,
    mut table: toml::Table,
    ffmpeg: &Path,
    python: &Path,
    model_dir: &Path,
) -> anyhow::Result<()> {
    for (key, path) in [("sherpa_python", python), ("sherpa_model_dir", model_dir)] {
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
    table.insert("stt_backend".into(), "parakeet".into());
    table.insert("language".into(), "auto".into());
    let body = toml::to_string_pretty(&table)?;
    if body.len() > 16_384 {
        bail!("updated voice.toml exceeds its 16KiB limit");
    }
    toml::from_str::<Config>(&body)?.recording()?;
    let mut temp = tempfile::NamedTempFile::new_in(prefs)?;
    temp.write_all(body.as_bytes())?;
    temp.as_file().sync_all()?;
    if config_snapshot(prefs)?.as_deref() != snapshot {
        bail!("voice.toml changed during setup; existing changes preserved, retry setup");
    }
    temp.persist(prefs.join(config::FILE))
        .context("save voice.toml")?;
    Ok(())
}

async fn download_inner(
    url: &str,
    destination: &Path,
    cap: u64,
    digest: &str,
) -> anyhow::Result<()> {
    let mut url = url.to_owned();
    for hop in 0..=5 {
        if !url.starts_with("https://") {
            bail!("model downloads require HTTPS");
        }
        let target = aether_net::vet(&url, aether_net::AddressPolicy::PublicOnly).await?;
        let client = aether_net::pinned_client(&target, INSTALL_TIMEOUT, "aether-voice-setup")?;
        let mut response = client.get(target.url.clone()).send().await?;
        if response.status().is_redirection() {
            if hop == 5 {
                bail!("too many model download redirects");
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .context("model redirect missing Location")?
                .to_str()?;
            url = target.url.join(location)?.to_string();
            continue;
        }
        response.error_for_status_ref()?;
        if response.content_length().is_some_and(|len| len > cap) {
            bail!("model download exceeds its size limit");
        }
        let mut file = tokio::fs::File::create(destination).await?;
        let mut hash = Sha256::new();
        let mut total = 0_u64;
        while let Some(chunk) = response.chunk().await? {
            total += chunk.len() as u64;
            if total > cap {
                bail!("model download exceeds its size limit");
            }
            hash.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        if hex::encode(hash.finalize()) != digest {
            bail!("Parakeet archive SHA-256 mismatch; refusing to install");
        }
        return Ok(());
    }
    bail!("model download redirect limit exceeded")
}

async fn download(
    destination: &Path,
    control: &mut watch::Receiver<Control>,
) -> anyhow::Result<()> {
    cancelled(control)?;
    tokio::select! {
        biased;
        _ = control.changed() => bail!("voice setup cancelled"),
        result = tokio::time::timeout(INSTALL_TIMEOUT, download_inner(MODEL_URL, destination, MODEL_BYTES, MODEL_SHA256)) => {
            result.context("Parakeet download timed out")?
        }
    }
}

async fn run(
    command: Command,
    control: &mut watch::Receiver<Control>,
    stage: &str,
) -> anyhow::Result<()> {
    process::run(command, &[], 65_536, INSTALL_TIMEOUT, control, false)
        .await
        .with_context(|| {
            format!("{stage}; check Python venv/pip availability and platform wheel support")
        })?;
    Ok(())
}

pub(super) async fn install(
    prefs_dir: PathBuf,
    control: &mut watch::Receiver<Control>,
    mut progress: impl FnMut(String),
) -> anyhow::Result<String> {
    cancelled(control)?;
    let python = find_tool("python3")?;
    let ffmpeg = find_tool("ffmpeg")?;
    aether_model::config::create_config_dir(&prefs_dir)?;
    let prefs_dir = std::fs::canonicalize(prefs_dir)?;
    let _lock = lock_setup(&prefs_dir)?;
    let snapshot = config_snapshot(&prefs_dir)?;
    let table = parse_config(snapshot.as_deref())?;
    let root = prefs_dir.join("voice");
    std::fs::create_dir_all(&root)?;
    let install = tempfile::Builder::new()
        .prefix("parakeet-")
        .tempdir_in(&root)?;
    let venv = install.path().join("env");
    let interpreter = venv.join("bin/python");
    progress("creating a private Python environment (Parakeet)".into());
    let mut command = Command::new(python);
    command
        .args(["-I", "-m", "venv"])
        .arg(&venv)
        .env("TMPDIR", install.path());
    run(command, control, "create private Python environment").await?;

    progress("installing sherpa-onnx 1.13.8 and NumPy wheels from PyPI".into());
    let mut command = Command::new(&interpreter);
    command
        .args([
            "-I",
            "-m",
            "pip",
            "--isolated",
            "--disable-pip-version-check",
            "install",
            "--quiet",
            "--no-cache-dir",
            "--only-binary=:all:",
            "--index-url",
            "https://pypi.org/simple",
            "sherpa-onnx==1.13.8",
            "numpy==2.4.3",
        ])
        .env("PIP_CONFIG_FILE", "/dev/null")
        .env("TMPDIR", install.path());
    run(command, control, "install pinned CPU runtime wheels").await?;

    progress("downloading Parakeet TDT 0.6B v3 int8 (~465 MiB)".into());
    let archive = install.path().join("model.tar.bz2");
    download(&archive, control).await?;
    progress("verifying and unpacking Parakeet model files".into());
    let models = install.path().join("models");
    let mut command = Command::new(&interpreter);
    command
        .args(["-I", "-c", ADAPTER, "unpack"])
        .arg(&archive)
        .arg(&models);
    run(command, control, "unpack verified model archive").await?;
    tokio::fs::remove_file(archive).await?;
    std::fs::write(
        install.path().join("MODEL-NOTICE.txt"),
        "NVIDIA Parakeet TDT 0.6B v3, CC-BY-4.0.\nhttps://huggingface.co/nvidia/parakeet-tdt-0.6b-v3\nhttps://creativecommons.org/licenses/by/4.0/\nONNX int8 conversion by the sherpa-onnx project.\nArchive SHA-256: 5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf\nsherpa-onnx 1.13.8 (Apache-2.0), NumPy 2.4.3 (see packaged licenses).\n",
    )?;
    cancelled(control)?;
    progress("saving Parakeet configuration; microphone remains off".into());
    let installed = super::setup::publish_install(install, || {
        save_config(
            &prefs_dir,
            snapshot.as_deref(),
            table,
            &ffmpeg,
            &interpreter,
            &models,
        )
    })?;
    Ok(format!(
        "Parakeet installed at {}. Press F4 to dictate; review before sending. Spoken questions remain separately opt-in.",
        installed.display()
    ))
}

#[cfg(test)]
#[path = "sherpa_tests.rs"]
mod tests;
