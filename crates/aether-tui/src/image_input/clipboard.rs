//! Local clipboard helpers, with bounded output and time to finish.

use std::process::Stdio;
use std::time::Duration;

use aether_client::images::image_attachment;
use aether_core::attachment::{
    AttachmentError, IMAGE_MEDIA_TYPES, ImageAttachment, MAX_IMAGE_BYTES,
};
use tokio::io::AsyncReadExt;

use super::ClipboardContent;

const HELPER_TIMEOUT: Duration = Duration::from_secs(3);
const NO_IMAGE: &str = "the clipboard holds no image (PNG, JPEG, GIF or WebP)";
const NO_CONTENT: &str = "the clipboard holds no text or supported image";
const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;
const MACOS_IMAGE_TYPES: [&str; 4] = ["PNGf", "JPEG", "GIFf", "WebP"];
const URI_LIST: &str = "text/uri-list";

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
    let bytes = clipboard_bytes(tool, tool.program())
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
    // Browsers and chat apps offer text beside a copied image: the image wins,
    // and the terminal's own paste (Ctrl+Shift+V) still gives the text.
    if let Some(kind) = offered_type(tool, &listing) {
        match image_bytes(tool, program, kind).await {
            Ok(bytes) => {
                return image_attachment(&bytes, None)
                    .map(ClipboardContent::Image)
                    .map_err(|e| HelperError::Broken(e.to_string()));
            }
            Err(HelperError::Refused) => {}
            Err(error) => return Err(error),
        }
    }
    if let Some(kind) = offered_text_type(tool, &listing) {
        return read_text(tool, program, pbpaste, kind).await;
    }
    if tool != ClipboardTool::Osascript && offers(&listing, URI_LIST) {
        return copied_files(tool, program).await;
    }
    if tool == ClipboardTool::WlPaste {
        return Err(HelperError::Refused);
    }
    read_text(tool, program, pbpaste, "").await
}

fn offers(listing: &str, media_type: &str) -> bool {
    listing.lines().any(|line| line.trim() == media_type)
}

/// Files copied in a file manager, one `file://` URL a line. One image file
/// becomes that image once `paste_with` reads the path; anything else is text.
async fn copied_files(tool: ClipboardTool, program: &str) -> Result<ClipboardContent, HelperError> {
    debug_assert!(
        tool != ClipboardTool::Osascript,
        "macOS offers no text/uri-list"
    );
    let args: &[&str] = match tool {
        ClipboardTool::WlPaste => &["--no-newline", "--type", URI_LIST],
        ClipboardTool::Xclip => &["-selection", "clipboard", "-o", "-t", URI_LIST],
        ClipboardTool::Osascript => return Err(HelperError::Refused),
    };
    let listed = utf8_output(program, args).await?;
    // RFC 2483: CRLF-separated URLs, `#` lines are comments.
    let urls: Vec<&str> = listed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    if urls.is_empty() {
        return Err(HelperError::Refused);
    }
    debug_assert!(urls.iter().all(|url| !url.contains('\n')));
    Ok(ClipboardContent::Text(urls.join("\n")))
}

async fn read_text(
    tool: ClipboardTool,
    program: &str,
    pbpaste: &str,
    kind: &str,
) -> Result<ClipboardContent, HelperError> {
    debug_assert!(!program.is_empty());
    debug_assert!(tool != ClipboardTool::WlPaste || !kind.is_empty());
    match tool {
        ClipboardTool::WlPaste => text_content(program, &["--no-newline", "--type", kind]).await,
        ClipboardTool::Xclip => text_content(program, &["-selection", "clipboard", "-o"]).await,
        ClipboardTool::Osascript => text_content(pbpaste, &[]).await,
    }
}

fn offered_text_type(tool: ClipboardTool, listing: &str) -> Option<&str> {
    if tool == ClipboardTool::Osascript {
        return ["utf8", "utxt", "TEXT"]
            .into_iter()
            .find(|class| listing.contains(&format!("«class {class}»")));
    }
    listing.lines().map(str::trim).find(|kind| {
        *kind == "text/plain"
            || kind.starts_with("text/plain;charset=")
            || matches!(*kind, "UTF8_STRING" | "TEXT" | "STRING")
    })
}

async fn text_content(program: &str, args: &[&str]) -> Result<ClipboardContent, HelperError> {
    utf8_output(program, args).await.map(ClipboardContent::Text)
}

/// What `program` prints, as non-empty UTF-8 text of at most 16 MiB.
async fn utf8_output(program: &str, args: &[&str]) -> Result<String, HelperError> {
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
    Ok(text)
}

fn list_args(tool: ClipboardTool) -> &'static [&'static str] {
    match tool {
        ClipboardTool::WlPaste => &["--list-types"],
        ClipboardTool::Xclip => &["-selection", "clipboard", "-t", "TARGETS", "-o"],
        ClipboardTool::Osascript => &["-e", "clipboard info"],
    }
}

async fn clipboard_bytes(tool: ClipboardTool, program: &str) -> Result<Vec<u8>, HelperError> {
    debug_assert!(!program.is_empty());
    let offered = run_helper(program, list_args(tool), 64 * 1024).await?;
    let media_type =
        offered_type(tool, &String::from_utf8_lossy(&offered)).ok_or(HelperError::Refused)?;
    debug_assert!(!media_type.is_empty());
    image_bytes(tool, program, media_type).await
}

async fn image_bytes(
    tool: ClipboardTool,
    program: &str,
    media_type: &str,
) -> Result<Vec<u8>, HelperError> {
    debug_assert!(!program.is_empty());
    debug_assert!(match tool {
        ClipboardTool::Osascript => MACOS_IMAGE_TYPES.contains(&media_type),
        _ => IMAGE_MEDIA_TYPES
            .iter()
            .any(|(known, _)| *known == media_type),
    });
    // One byte over the limit is enough for the size check to refuse it.
    let limit = MAX_IMAGE_BYTES + 1;
    let read: &[&str] = match tool {
        ClipboardTool::Osascript => {
            return macos_image_bytes(program, media_type).await;
        }
        ClipboardTool::WlPaste => &["--type"],
        ClipboardTool::Xclip => &["-selection", "clipboard", "-o", "-t"],
    };
    let args: Vec<&str> = read.iter().copied().chain([media_type]).collect();
    run_helper(program, &args, limit).await
}

async fn macos_image_bytes(program: &str, class: &str) -> Result<Vec<u8>, HelperError> {
    debug_assert!(!program.is_empty());
    debug_assert!(MACOS_IMAGE_TYPES.contains(&class));
    let script = format!("get the clipboard as «class {class}»");
    let limit = (MAX_IMAGE_BYTES + 1) * 2 + 64;
    let printed = run_helper(program, &["-e", &script], limit).await?;
    if printed.len() == limit {
        return Err(HelperError::Broken(
            AttachmentError::TooLarge {
                label: "from the clipboard".into(),
                size: MAX_IMAGE_BYTES + 1,
            }
            .to_string(),
        ));
    }
    hex_payload(&String::from_utf8_lossy(&printed), class).ok_or(HelperError::Refused)
}

fn offered_type(tool: ClipboardTool, listing: &str) -> Option<&'static str> {
    if tool == ClipboardTool::Osascript {
        return MACOS_IMAGE_TYPES
            .into_iter()
            .find(|class| listing.contains(&format!("«class {class}»")));
    }
    offered_image_type(listing)
}

/// The first supported image type among the types a clipboard offers, one per line.
fn offered_image_type(listing: &str) -> Option<&'static str> {
    IMAGE_MEDIA_TYPES
        .iter()
        .map(|(media_type, _)| *media_type)
        .find(|media_type| offers(listing, media_type))
}

/// The bytes `osascript` printed as `«data PNGf89504E47…»`.
fn hex_payload(printed: &str, class: &str) -> Option<Vec<u8>> {
    debug_assert!(MACOS_IMAGE_TYPES.contains(&class));
    debug_assert_eq!(class.len(), 4);
    let open = format!("«data {class}");
    let start = printed.find(&open)? + open.len();
    let hex = &printed[start..];
    let hex = &hex[..hex.find('»')?];
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
    // The PTY paste test puts a fake `wl-paste` first on PATH and says so.
    #[cfg(test)]
    if matches!(program, "wl-paste" | "xclip" | "osascript" | "pbpaste")
        && std::env::var_os("AETHER_TEST_FAKE_CLIPBOARD").is_none()
    {
        return Err(HelperError::Broken(
            "real clipboard helpers are disabled in tests".into(),
        ));
    }
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
