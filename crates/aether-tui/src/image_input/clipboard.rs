//! Local clipboard helpers, with bounded output and time to finish.

use std::process::Stdio;
use std::time::Duration;

use aether_client::images::image_attachment;
use aether_core::attachment::{IMAGE_MEDIA_TYPES, ImageAttachment, MAX_IMAGE_BYTES};
use tokio::io::AsyncReadExt;

use super::ClipboardContent;

const HELPER_TIMEOUT: Duration = Duration::from_secs(3);
const NO_IMAGE: &str = "the clipboard holds no image (PNG, JPEG, GIF or WebP)";
const NO_CONTENT: &str = "the clipboard holds no text or supported image";
const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;
/// `osascript` prints the PNG as hex between these.
const MACOS_PNG_OPEN: &str = "«data PNGf";
const MACOS_PNG_CLOSE: &str = "»";

/// The program that reads this session's clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardTool {
    WlPaste,
    Xclip,
    Osascript,
}

impl ClipboardTool {
    fn program(self) -> &'static str {
        match self {
            Self::WlPaste => "wl-paste",
            Self::Xclip => "xclip",
            Self::Osascript => "osascript",
        }
    }

    /// What to say when the program is not installed.
    fn missing_hint(self) -> String {
        let package = match self {
            Self::WlPaste => "wl-clipboard",
            Self::Xclip => "xclip",
            Self::Osascript => "osascript",
        };
        format!("install {package} to paste images, or use /image <path>")
    }
}

/// Which tool reads the clipboard here, or why none can.
pub(crate) fn clipboard_tool(
    macos: bool,
    ssh: bool,
    wayland: bool,
    display: bool,
) -> Result<ClipboardTool, String> {
    if ssh {
        return Err(
            "the clipboard is on the other end of this SSH session: copy the file here and use /image <path>"
                .into(),
        );
    }
    if macos {
        return Ok(ClipboardTool::Osascript);
    }
    if wayland {
        return Ok(ClipboardTool::WlPaste);
    }
    if display {
        return Ok(ClipboardTool::Xclip);
    }
    Err("no display to read a clipboard from: use /image <path>".into())
}

/// Why a helper gave no bytes.
#[derive(Debug, PartialEq, Eq)]
enum HelperError {
    Missing,
    /// It ran and said no: an empty clipboard, or nothing of the type asked for.
    Refused,
    Broken(String),
}

pub(super) async fn image() -> Result<ImageAttachment, String> {
    let tool = session_tool()?;
    let bytes = clipboard_bytes(tool)
        .await
        .map_err(|error| helper_reason(tool, error, NO_IMAGE))?;
    image_attachment(&bytes, None).map_err(|e| e.to_string())
}

fn session_tool() -> Result<ClipboardTool, String> {
    clipboard_tool(
        cfg!(target_os = "macos"),
        std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some(),
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        std::env::var_os("DISPLAY").is_some(),
    )
}

fn helper_reason(tool: ClipboardTool, error: HelperError, refused: &str) -> String {
    match error {
        HelperError::Missing => tool.missing_hint(),
        HelperError::Refused => refused.to_owned(),
        HelperError::Broken(reason) => reason,
    }
}

pub(crate) async fn paste() -> Result<ClipboardContent, String> {
    let tool = session_tool()?;
    paste_with(tool, tool.program(), "pbpaste").await
}

pub(crate) async fn paste_with(
    tool: ClipboardTool,
    program: &str,
    pbpaste: &str,
) -> Result<ClipboardContent, String> {
    let content = paste_bytes(tool, program, pbpaste)
        .await
        .map_err(|error| helper_reason(tool, error, NO_CONTENT))?;
    match content {
        ClipboardContent::Text(text) => {
            tokio::task::spawn_blocking(move || super::pasted_content(text))
                .await
                .map_err(|e| format!("reading the pasted path did not finish: {e}"))
        }
        content => Ok(content),
    }
}

async fn paste_bytes(
    tool: ClipboardTool,
    program: &str,
    pbpaste: &str,
) -> Result<ClipboardContent, HelperError> {
    debug_assert!(!program.is_empty());
    debug_assert!(tool != ClipboardTool::Osascript || !pbpaste.is_empty());
    let offered = run_helper(program, list_args(tool), 64 * 1024).await?;
    let listing = String::from_utf8_lossy(&offered);
    let media_type = offered_image_type(&listing);
    let mac_image = tool == ClipboardTool::Osascript
        && ["PNGf", "JPEG", "GIFf", "WebP"]
            .iter()
            .any(|class| listing.contains(&format!("«class {class}»")));
    if media_type.is_some() || mac_image {
        let bytes = image_bytes(tool, program, media_type).await?;
        return image_attachment(&bytes, None)
            .map(ClipboardContent::Image)
            .map_err(|e| HelperError::Broken(e.to_string()));
    }
    let (program, args): (&str, &[&str]) = match tool {
        ClipboardTool::WlPaste => {
            let media_type = ["text/plain;charset=utf-8", "text/plain"]
                .into_iter()
                .find(|kind| listing.lines().any(|line| line.trim() == *kind))
                .ok_or(HelperError::Refused)?;
            return text_content(program, &["--no-newline", "--type", media_type]).await;
        }
        ClipboardTool::Xclip => (program, &["-selection", "clipboard", "-o"]),
        ClipboardTool::Osascript => (pbpaste, &[]),
    };
    text_content(program, args).await
}

async fn text_content(program: &str, args: &[&str]) -> Result<ClipboardContent, HelperError> {
    let bytes = run_helper(program, args, MAX_TEXT_BYTES + 1).await?;
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(HelperError::Broken("clipboard text exceeds 16 MiB".into()));
    }
    let text = String::from_utf8(bytes)
        .map_err(|e| HelperError::Broken(format!("clipboard text is not UTF-8: {e}")))?;
    if text.is_empty() {
        return Err(HelperError::Refused);
    }
    debug_assert!(text.len() <= MAX_TEXT_BYTES);
    debug_assert!(!text.is_empty());
    Ok(ClipboardContent::Text(text))
}

fn list_args(tool: ClipboardTool) -> &'static [&'static str] {
    match tool {
        ClipboardTool::WlPaste => &["--list-types"],
        ClipboardTool::Xclip => &["-selection", "clipboard", "-t", "TARGETS", "-o"],
        ClipboardTool::Osascript => &["-e", "clipboard info"],
    }
}

async fn clipboard_bytes(tool: ClipboardTool) -> Result<Vec<u8>, HelperError> {
    let program = tool.program();
    if tool == ClipboardTool::Osascript {
        return image_bytes(tool, program, None).await;
    }
    let offered = run_helper(program, list_args(tool), 64 * 1024).await?;
    let media_type =
        offered_image_type(&String::from_utf8_lossy(&offered)).ok_or(HelperError::Refused)?;
    image_bytes(tool, program, Some(media_type)).await
}

async fn image_bytes(
    tool: ClipboardTool,
    program: &str,
    media_type: Option<&str>,
) -> Result<Vec<u8>, HelperError> {
    debug_assert!(tool == ClipboardTool::Osascript || media_type.is_some());
    debug_assert!(
        media_type.is_none_or(|kind| IMAGE_MEDIA_TYPES.iter().any(|(known, _)| *known == kind))
    );
    // One byte over the limit is enough for the size check to refuse it.
    let limit = MAX_IMAGE_BYTES + 1;
    let read: &[&str] = match tool {
        ClipboardTool::Osascript => {
            let script = ["-e", "get the clipboard as «class PNGf»"];
            let printed = run_helper(program, &script, limit * 2 + 64).await?;
            return hex_payload(&String::from_utf8_lossy(&printed)).ok_or(HelperError::Refused);
        }
        ClipboardTool::WlPaste => &["--type"],
        ClipboardTool::Xclip => &["-selection", "clipboard", "-o", "-t"],
    };
    let media_type = media_type.ok_or(HelperError::Refused)?;
    let args: Vec<&str> = read.iter().copied().chain([media_type]).collect();
    run_helper(program, &args, limit).await
}

/// The first supported image type among the types a clipboard offers, one per line.
fn offered_image_type(listing: &str) -> Option<&'static str> {
    IMAGE_MEDIA_TYPES
        .iter()
        .map(|(media_type, _)| *media_type)
        .find(|media_type| listing.lines().any(|line| line.trim() == *media_type))
}

/// The bytes `osascript` printed as `«data PNGf89504E47…»`.
fn hex_payload(printed: &str) -> Option<Vec<u8>> {
    let start = printed.find(MACOS_PNG_OPEN)? + MACOS_PNG_OPEN.len();
    let hex = &printed[start..];
    let hex = &hex[..hex.find(MACOS_PNG_CLOSE)?];
    if hex.is_empty() || !hex.len().is_multiple_of(2) || !hex.is_ascii() {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).ok())
        .collect()
}

/// Run `program` and take at most `limit` bytes of what it prints.
async fn run_helper(program: &str, args: &[&str], limit: usize) -> Result<Vec<u8>, HelperError> {
    debug_assert!(!program.is_empty());
    debug_assert!(limit > 0);
    let broken = |reason: String| HelperError::Broken(format!("{program}: {reason}"));
    let mut child = match tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(HelperError::Missing),
        Err(e) => return Err(broken(e.to_string())),
    };
    let Some(stdout) = child.stdout.take() else {
        return Err(broken("no output to read".into()));
    };
    let printed = async {
        let mut bytes = Vec::new();
        let mut bounded = stdout.take(limit as u64);
        bounded
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| broken(e.to_string()))?;
        // At the limit the helper may still be writing: it is dropped, not awaited.
        if bytes.len() == limit {
            return Ok(bytes);
        }
        let status = child.wait().await.map_err(|e| broken(e.to_string()))?;
        if status.success() {
            Ok(bytes)
        } else {
            Err(HelperError::Refused)
        }
    };
    match tokio::time::timeout(HELPER_TIMEOUT, printed).await {
        Ok(result) => result,
        Err(_) => Err(broken("timed out".into())),
    }
}

#[cfg(test)]
pub(crate) mod tests;
