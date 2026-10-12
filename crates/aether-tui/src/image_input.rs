//! Where a composer image comes from: a file named by a paste or by `/image`,
//! or the system clipboard through a local helper program.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use aether_client::images::{image_attachment, read_image_file};
use aether_core::attachment::{IMAGE_MEDIA_TYPES, ImageAttachment, MAX_IMAGE_BYTES};
use tokio::io::AsyncReadExt;

/// What `Effect::AttachImage` reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    Clipboard,
    File(PathBuf),
}

/// File extensions a pasted path must end in to be read as an image.
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];
const HELPER_TIMEOUT: Duration = Duration::from_secs(3);
const NO_IMAGE: &str = "the clipboard holds no image (PNG, JPEG, GIF or WebP)";
/// `osascript` prints the PNG as hex between these.
const MACOS_PNG_OPEN: &str = "«data PNGf";
const MACOS_PNG_CLOSE: &str = "»";

/// Read `source` into an attachment; the error is one line for the status row.
pub async fn read(source: ImageSource) -> Result<ImageAttachment, String> {
    match source {
        ImageSource::Clipboard => clipboard_image().await,
        ImageSource::File(path) => tokio::task::spawn_blocking(move || read_image_file(&path))
            .await
            .map_err(|e| format!("reading the image did not finish: {e}"))?
            .map_err(|e| e.to_string()),
    }
}

/// The image file a paste names, when the paste is exactly one path to one.
/// A terminal may quote, escape or `file://` a dropped file; each is tried.
pub fn pasted_image_path(text: &str, home: Option<&Path>) -> Option<PathBuf> {
    let text = text.trim();
    if text.is_empty() || text.contains('\n') {
        return None;
    }
    path_spellings(text)
        .into_iter()
        .filter_map(|spelling| crate::workspace::resolve(&spelling, home, None))
        .find(|path| has_image_extension(path) && path.is_file())
}

fn has_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// `text` as typed, then with quotes, backslash escapes or a `file://` URL undone.
fn path_spellings(text: &str) -> Vec<String> {
    let mut spellings = vec![text.to_owned()];
    for quote in ['\'', '"'] {
        let inner = text
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote));
        spellings.extend(inner.map(str::to_owned));
    }
    if text.contains('\\') {
        spellings.push(unescaped(text));
    }
    spellings.extend(text.strip_prefix("file://").and_then(percent_decoded));
    spellings
}

/// Shell escaping undone: each backslash yields the character after it.
fn unescaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            c => out.push(c),
        }
    }
    out
}

/// `%20` and its kin decoded; `None` for a broken escape or non-UTF-8 bytes.
fn percent_decoded(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&first, tail)) = rest.split_first() {
        if first != b'%' {
            bytes.push(first);
            rest = tail;
            continue;
        }
        let hex = std::str::from_utf8(tail.get(..2)?).ok()?;
        bytes.push(u8::from_str_radix(hex, 16).ok()?);
        rest = &tail[2..];
    }
    String::from_utf8(bytes).ok()
}

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

async fn clipboard_image() -> Result<ImageAttachment, String> {
    let tool = clipboard_tool(
        cfg!(target_os = "macos"),
        std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some(),
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        std::env::var_os("DISPLAY").is_some(),
    )?;
    let bytes = clipboard_bytes(tool).await.map_err(|error| match error {
        HelperError::Missing => tool.missing_hint(),
        HelperError::Refused => NO_IMAGE.to_owned(),
        HelperError::Broken(reason) => reason,
    })?;
    image_attachment(&bytes, None).map_err(|e| e.to_string())
}

async fn clipboard_bytes(tool: ClipboardTool) -> Result<Vec<u8>, HelperError> {
    let program = tool.program();
    // One byte over the limit is enough for the size check to refuse it.
    let limit = MAX_IMAGE_BYTES + 1;
    let (list, read): (&[&str], &[&str]) = match tool {
        ClipboardTool::Osascript => {
            let script = ["-e", "get the clipboard as «class PNGf»"];
            let printed = run_helper(program, &script, limit * 2 + 64).await?;
            return hex_payload(&String::from_utf8_lossy(&printed)).ok_or(HelperError::Refused);
        }
        ClipboardTool::WlPaste => (&["--list-types"], &["--type"]),
        ClipboardTool::Xclip => (
            &["-selection", "clipboard", "-t", "TARGETS", "-o"],
            &["-selection", "clipboard", "-o", "-t"],
        ),
    };
    let offered = run_helper(program, list, 64 * 1024).await?;
    let media_type =
        offered_image_type(&String::from_utf8_lossy(&offered)).ok_or(HelperError::Refused)?;
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
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\npixels";

    #[test]
    fn a_paste_is_an_image_path_however_the_terminal_spelled_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("Screenshot from today.PNG");
        std::fs::write(&path, PNG).expect("write");
        let plain = path.to_str().expect("utf-8").to_owned();
        let spellings = [
            plain.clone(),
            format!("  {plain}\n"),
            format!("'{plain}'"),
            format!("\"{plain}\""),
            plain.replace(' ', "\\ "),
            format!("file://{}", plain.replace(' ', "%20")),
        ];
        for spelling in spellings {
            assert_eq!(
                pasted_image_path(&spelling, None).as_deref(),
                Some(path.as_path()),
                "{spelling:?}"
            );
        }
    }

    #[test]
    fn a_home_relative_paste_is_read_from_home() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(home.path().join("Pictures")).expect("mkdir");
        let path = home.path().join("Pictures/shot.png");
        std::fs::write(&path, PNG).expect("write");
        assert_eq!(
            pasted_image_path("~/Pictures/shot.png", Some(home.path())),
            Some(path)
        );
        assert_eq!(pasted_image_path("~/Pictures/shot.png", None), None);
    }

    #[test]
    fn text_that_is_not_one_existing_image_path_stays_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let image = dir.path().join("shot.png");
        let notes = dir.path().join("notes.txt");
        std::fs::write(&image, PNG).expect("write");
        std::fs::write(&notes, "words").expect("write");
        let image = image.to_str().expect("utf-8");
        let missing = dir.path().join("gone.png");
        let stays_text = [
            String::new(),
            "look at this".to_owned(),
            "shot.png".to_owned(),
            notes.to_str().expect("utf-8").to_owned(),
            missing.to_str().expect("utf-8").to_owned(),
            dir.path().to_str().expect("utf-8").to_owned(),
            format!("{image}\n{image}"),
            format!("see {image}"),
            "file://%zz/shot.png".to_owned(),
        ];
        for text in stays_text {
            assert_eq!(pasted_image_path(&text, None), None, "{text:?}");
        }
    }

    #[test]
    fn the_clipboard_tool_follows_the_session() {
        assert_eq!(
            clipboard_tool(false, false, true, true),
            Ok(ClipboardTool::WlPaste)
        );
        assert_eq!(
            clipboard_tool(false, false, false, true),
            Ok(ClipboardTool::Xclip)
        );
        assert_eq!(
            clipboard_tool(true, false, false, false),
            Ok(ClipboardTool::Osascript)
        );
        for (macos, ssh, wayland, display) in [
            (false, true, true, true),
            (true, true, false, false),
            (false, false, false, false),
        ] {
            let hint = clipboard_tool(macos, ssh, wayland, display).expect_err("no tool");
            assert!(hint.contains("/image <path>"), "{hint}");
            assert!(!hint.contains('\n'), "one line: {hint}");
        }
        for tool in [ClipboardTool::WlPaste, ClipboardTool::Xclip] {
            let hint = tool.missing_hint();
            assert!(hint.starts_with("install "), "{hint}");
            assert!(hint.contains("/image <path>"), "{hint}");
        }
    }

    #[test]
    fn the_offered_type_is_the_first_one_supported() {
        let listing = "TARGETS\ntext/plain\nimage/webp\nimage/png\n";
        assert_eq!(offered_image_type(listing), Some("image/png"));
        assert_eq!(offered_image_type("image/jpeg\r\n"), Some("image/jpeg"));
        assert_eq!(offered_image_type("text/plain\nimage/bmp\n"), None);
        assert_eq!(offered_image_type(""), None);
    }

    #[test]
    fn the_macos_hex_dump_decodes_to_its_bytes() {
        assert_eq!(
            hex_payload("«data PNGf89504E470D0A1A0A»\n"),
            Some(b"\x89PNG\r\n\x1a\n".to_vec())
        );
        for broken in [
            "",
            "«data PNGf»",
            "«data PNGf895»",
            "«data PNGfzz»",
            "«data PNGf89",
        ] {
            assert_eq!(hex_payload(broken), None, "{broken:?}");
        }
    }

    #[tokio::test]
    async fn a_helper_gives_its_bytes_a_refusal_or_its_absence() {
        let printed = run_helper("sh", &["-c", "printf pixels"], 64).await;
        assert_eq!(printed, Ok(b"pixels".to_vec()));
        let refused = run_helper("sh", &["-c", "printf nothing; exit 1"], 64).await;
        assert_eq!(refused, Err(HelperError::Refused));
        let missing = run_helper("aether-no-such-clipboard-helper", &[], 64).await;
        assert_eq!(missing, Err(HelperError::Missing));
        // Output past the limit is cut there, and the helper is not waited on.
        let cut = run_helper("sh", &["-c", "printf 0123456789; exec sleep 30"], 4).await;
        assert_eq!(cut, Ok(b"0123".to_vec()));
    }

    #[tokio::test]
    async fn a_file_source_reads_the_file_and_names_what_is_wrong() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("shot.png");
        std::fs::write(&path, PNG).expect("write");
        let image = read(ImageSource::File(path)).await.expect("attached");
        assert_eq!(image.name.as_deref(), Some("shot.png"));
        assert_eq!(image.size_bytes(), PNG.len() as u64);
        let missing = read(ImageSource::File(dir.path().join("gone.png"))).await;
        let reason = missing.expect_err("no such file");
        assert!(reason.contains("gone.png"), "{reason}");
    }
}
