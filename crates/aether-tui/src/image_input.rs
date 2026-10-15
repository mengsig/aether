//! Where a composer image comes from: a file named by a paste or by `/image`,
//! or the system clipboard through a local helper program.

use std::path::{Path, PathBuf};

use aether_client::images::read_image_file;
use aether_core::attachment::ImageAttachment;

pub(crate) mod clipboard;
pub(crate) use clipboard::paste as paste_clipboard;

/// What `Effect::AttachImage` reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    Clipboard,
    File(PathBuf),
}

/// A clipboard paste, including a text path that was read off the UI loop.
#[derive(Debug)]
pub enum ClipboardContent {
    Image(ImageAttachment),
    Text(String),
    ImagePath {
        text: String,
        image: Result<ImageAttachment, String>,
    },
}

pub(crate) fn pasted_content(text: String) -> ClipboardContent {
    let home = crate::workspace::home_dir();
    let Some(path) = pasted_image_path(&text, home.as_deref()) else {
        return ClipboardContent::Text(text);
    };
    debug_assert!(!text.trim().is_empty());
    debug_assert!(has_image_extension(&path));
    let image = read_image_file(&path).map_err(|e| e.to_string());
    ClipboardContent::ImagePath { text, image }
}

/// File extensions a pasted path must end in to be read as an image.
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// Read `source` into an attachment; the error is one line for the status row.
pub async fn read(source: ImageSource) -> Result<ImageAttachment, String> {
    match source {
        ImageSource::Clipboard => clipboard::image().await,
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
