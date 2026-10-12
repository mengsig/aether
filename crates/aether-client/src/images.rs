//! Turning an image on this machine into a message attachment. It is checked
//! here the way the daemon checks it, so a file the daemon would refuse is
//! refused before megabytes cross the connection.

use std::path::{Path, PathBuf};

use aether_core::attachment::{
    AttachmentError, ImageAttachment, MAX_IMAGE_BYTES, check_image, sniff_media_type,
};
use base64::Engine as _;

/// Why a file could not be attached.
#[derive(Debug, thiserror::Error)]
pub enum ImageFileError {
    #[error("could not read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} is not a file", path.display())]
    NotAFile { path: PathBuf },
    #[error(transparent)]
    Refused(#[from] AttachmentError),
}

/// Image bytes as an attachment. `name` is the file name it was read from,
/// when there is one; it also names the image in a refusal.
pub fn image_attachment(
    bytes: &[u8],
    name: Option<&str>,
) -> Result<ImageAttachment, AttachmentError> {
    let label = name.unwrap_or("from the clipboard");
    let Some(media_type) = sniff_media_type(bytes) else {
        return Err(AttachmentError::UnsupportedType {
            label: label.to_owned(),
        });
    };
    let checked = check_image(bytes, media_type, name, label)?;
    debug_assert_eq!(checked.size_bytes, bytes.len() as u64);
    Ok(ImageAttachment {
        media_type: checked.media_type,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        name: checked.name,
    })
}

/// Read an image file into an attachment. A file over the size limit is
/// refused on its length, before it is read.
pub fn read_image_file(path: &Path) -> Result<ImageAttachment, ImageFileError> {
    let read_failed = |source| ImageFileError::Read {
        path: path.to_owned(),
        source,
    };
    let metadata = std::fs::metadata(path).map_err(read_failed)?;
    if !metadata.is_file() {
        return Err(ImageFileError::NotAFile {
            path: path.to_owned(),
        });
    }
    let name = path.file_name().map(|name| name.to_string_lossy());
    if metadata.len() > MAX_IMAGE_BYTES as u64 {
        return Err(AttachmentError::TooLarge {
            label: name.as_deref().unwrap_or("file").to_owned(),
            size: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
        }
        .into());
    }
    let bytes = std::fs::read(path).map_err(read_failed)?;
    Ok(image_attachment(&bytes, name.as_deref())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\npixels";

    #[test]
    fn an_image_file_becomes_an_attachment_named_after_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("shot.png");
        std::fs::write(&path, PNG).expect("write");
        let image = read_image_file(&path).expect("attached");
        assert_eq!(image.media_type, "image/png");
        assert_eq!(image.name.as_deref(), Some("shot.png"));
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&image.data)
                .expect("base64"),
            PNG
        );
    }

    #[test]
    fn a_file_that_is_not_an_image_is_refused_by_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "plain text").expect("write");
        let error = read_image_file(&path).expect_err("refused");
        assert!(
            matches!(
                error,
                ImageFileError::Refused(AttachmentError::UnsupportedType { .. })
            ),
            "{error:?}"
        );
        assert!(error.to_string().contains("notes.txt"), "{error}");
    }

    #[test]
    fn a_missing_file_a_directory_and_an_oversized_file_are_each_named() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = read_image_file(&dir.path().join("gone.png")).expect_err("missing");
        assert!(
            matches!(missing, ImageFileError::Read { .. }),
            "{missing:?}"
        );
        assert!(missing.to_string().contains("gone.png"), "{missing}");

        let directory = read_image_file(dir.path()).expect_err("a directory");
        assert!(matches!(directory, ImageFileError::NotAFile { .. }));

        let path = dir.path().join("huge.png");
        let file = std::fs::File::create(&path).expect("create");
        file.set_len(MAX_IMAGE_BYTES as u64 + 1)
            .expect("sparse file");
        let huge = read_image_file(&path).expect_err("too large");
        assert!(
            matches!(
                huge,
                ImageFileError::Refused(AttachmentError::TooLarge { .. })
            ),
            "{huge:?}"
        );
    }

    #[test]
    fn clipboard_bytes_need_no_name() {
        let image = image_attachment(PNG, None).expect("attached");
        assert_eq!(image.name, None);
        let error = image_attachment(b"not an image", None).expect_err("refused");
        assert!(error.to_string().contains("from the clipboard"), "{error}");
    }
}
