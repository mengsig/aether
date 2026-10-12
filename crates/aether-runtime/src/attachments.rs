//! Message images: checked at the boundary, stored once per digest, and read
//! back only to hand a model or an engine the picture.
//!
//! Durable state (inbox, events, transcript) carries `ContentBlock::ImageRef`.
//! The user turn always names the stored file in its text, so a model or an
//! engine that cannot take an image still leaves the agent a path to open.

use std::path::{Path, PathBuf};

use aether_core::attachment::{
    AttachmentError, AttachmentRef, ImageAttachment, MAX_IMAGE_BYTES, MAX_IMAGES_PER_MESSAGE,
    MAX_MESSAGE_IMAGE_BYTES, check_image,
};
use aether_core::model::{ContentBlock, Message};
use aether_engine::EngineImage;
use base64::Engine as _;

use crate::{Result, Runtime, RuntimeError};

/// One image that passed every check, with the bytes to store.
#[derive(Debug, PartialEq, Eq)]
pub struct CheckedImage {
    pub reference: AttachmentRef,
    pub bytes: Vec<u8>,
}

/// Decode and check a message's images: count, base64, size, type by magic
/// bytes, and the total. Nothing is stored unless all of them pass.
pub fn check_images(
    images: &[ImageAttachment],
) -> std::result::Result<Vec<CheckedImage>, AttachmentError> {
    if images.len() > MAX_IMAGES_PER_MESSAGE {
        return Err(AttachmentError::TooMany {
            count: images.len(),
        });
    }
    let mut checked = Vec::with_capacity(images.len());
    let mut total = 0usize;
    for (index, image) in images.iter().enumerate() {
        let label = match &image.name {
            Some(name) => format!("{} ({name})", index + 1),
            None => (index + 1).to_string(),
        };
        // Refused on the encoded length, before megabytes are decoded.
        let decoded_len = image.data.len() / 4 * 3;
        if decoded_len > MAX_IMAGE_BYTES + 2 {
            return Err(AttachmentError::TooLarge {
                label,
                size: decoded_len,
            });
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&image.data)
            .map_err(|error| AttachmentError::NotBase64 {
                label: label.clone(),
                reason: error.to_string(),
            })?;
        let reference = check_image(&bytes, &image.media_type, image.name.as_deref(), &label)?;
        total += bytes.len();
        if total > MAX_MESSAGE_IMAGE_BYTES {
            return Err(AttachmentError::TotalTooLarge { size: total });
        }
        checked.push(CheckedImage { reference, bytes });
    }
    debug_assert_eq!(
        checked.len(),
        images.len(),
        "every image is checked or refused"
    );
    Ok(checked)
}

/// Write one checked image under its digest. An image already stored is left
/// alone: the name is the content, so the bytes cannot differ.
fn store_image(dir: &Path, image: &CheckedImage) -> std::io::Result<()> {
    let path = dir.join(image.reference.file_name());
    match std::fs::metadata(&path) {
        Ok(existing) if existing.is_file() && existing.len() == image.reference.size_bytes => {
            return Ok(());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    aether_core::private_file::write(&path, &image.bytes)
}

/// Create the attachment directory, readable by its owner only.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Refuse a relative attachment directory. An engine is handed a stored
/// image's path and opens it from its agent's workspace, not the daemon's.
pub(crate) fn check_attachment_dir(dir: Option<&Path>) -> Result<()> {
    match dir {
        Some(dir) if !dir.is_absolute() => Err(RuntimeError::Invalid(format!(
            "the attachment directory {} is relative; an engine opens a stored image from its \
             agent's workspace, so the directory must be absolute",
            dir.display()
        ))),
        _ => Ok(()),
    }
}

impl Runtime {
    /// Where this daemon stores message images, when it stores any.
    pub fn attachment_dir(&self) -> Option<&Path> {
        self.config.attachment_dir.as_deref()
    }

    /// The file a stored image lives in. It may have been removed since.
    pub fn attachment_path(&self, image: &AttachmentRef) -> Option<PathBuf> {
        self.attachment_dir().map(|dir| dir.join(image.file_name()))
    }

    /// Check a message's images and store each once. Returns the references
    /// the message carries instead of the bytes; refuses the whole message
    /// when any image is not acceptable.
    pub fn store_attachments(&self, images: &[ImageAttachment]) -> Result<Vec<AttachmentRef>> {
        if images.is_empty() {
            return Ok(Vec::new());
        }
        let checked =
            check_images(images).map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        let Some(dir) = self.attachment_dir() else {
            return Err(RuntimeError::Invalid(
                "this daemon has no attachment directory, so a message cannot carry images".into(),
            ));
        };
        let failed = |error: std::io::Error| {
            RuntimeError::Internal(format!(
                "could not store an image under {}: {error}",
                dir.display()
            ))
        };
        create_private_dir(dir).map_err(failed)?;
        for image in &checked {
            store_image(dir, image).map_err(failed)?;
        }
        Ok(checked.into_iter().map(|image| image.reference).collect())
    }

    /// The stored images of one user message as an engine takes them: files.
    /// An image whose file is gone is left out; the text still names it.
    pub(crate) fn engine_images(&self, message: &Message) -> Vec<EngineImage> {
        let mut images = Vec::new();
        for block in &message.content {
            let ContentBlock::ImageRef(image) = block else {
                continue;
            };
            match self.attachment_path(image).filter(|path| path.is_file()) {
                Some(path) => images.push(EngineImage {
                    path,
                    media_type: image.media_type.clone(),
                }),
                None => tracing::warn!(
                    image = %image.id,
                    "a message's image is no longer stored; the engine gets its text only"
                ),
            }
        }
        images
    }

    /// Replace every stored-image reference in a model request with the image
    /// itself. A model without vision, or an image whose file is gone, gets a
    /// line of text in its place: the turn must not fail on an old message.
    pub(crate) fn inline_images(&self, messages: &mut [Message], vision: bool) {
        for message in messages {
            for block in &mut message.content {
                let ContentBlock::ImageRef(image) = block else {
                    continue;
                };
                *block = self.inlined(image, vision);
            }
        }
    }

    fn inlined(&self, image: &AttachmentRef, vision: bool) -> ContentBlock {
        if !vision {
            return ContentBlock::text(format!(
                "{} (this model takes no images; open the stored file named above)",
                image.label()
            ));
        }
        let read = self
            .attachment_path(image)
            .ok_or_else(|| "this daemon has no attachment directory".to_owned())
            .and_then(|path| std::fs::read(path).map_err(|error| error.to_string()));
        match read {
            Ok(bytes) => ContentBlock::Image {
                media_type: image.media_type.clone(),
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            },
            Err(error) => {
                tracing::warn!(image = %image.id, %error, "a message's image could not be read");
                ContentBlock::text(format!("{} (no longer stored: {error})", image.label()))
            }
        }
    }
}

/// A message's content: its text, then one reference per stored image.
/// An image sent without words carries no empty text block.
pub fn message_content(text: String, images: Vec<AttachmentRef>) -> Vec<ContentBlock> {
    let mut content = Vec::with_capacity(1 + images.len());
    if !text.is_empty() || images.is_empty() {
        content.push(ContentBlock::text(text));
    }
    content.extend(images.into_iter().map(ContentBlock::ImageRef));
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::attachment::AttachmentId;

    pub(crate) const PNG: &[u8] = b"\x89PNG\r\n\x1a\npixels";

    fn attachment(media_type: &str, bytes: &[u8], name: Option<&str>) -> ImageAttachment {
        ImageAttachment {
            media_type: media_type.into(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
            name: name.map(str::to_owned),
        }
    }

    #[test]
    fn a_valid_image_is_checked_into_its_reference_and_bytes() {
        let checked =
            check_images(&[attachment("image/png", PNG, Some("shot.png"))]).expect("a PNG passes");
        assert_eq!(checked.len(), 1);
        assert_eq!(checked[0].bytes, PNG);
        assert_eq!(checked[0].reference.id, AttachmentId::of(PNG));
        assert_eq!(checked[0].reference.name.as_deref(), Some("shot.png"));
    }

    #[test]
    fn a_wrong_type_is_refused_by_name() {
        let pdf = attachment("image/png", b"%PDF-1.7 not an image", Some("doc.pdf"));
        let error = check_images(&[pdf]).unwrap_err();
        assert!(matches!(error, AttachmentError::UnsupportedType { .. }));
        assert!(error.to_string().contains("1 (doc.pdf)"), "{error}");
        let lying = attachment("image/jpeg", PNG, None);
        assert!(matches!(
            check_images(&[lying]).unwrap_err(),
            AttachmentError::TypeMismatch {
                actual: "image/png",
                ..
            }
        ));
    }

    #[test]
    fn an_oversized_image_is_refused_before_and_after_decoding() {
        let mut big = PNG.to_vec();
        big.resize(MAX_IMAGE_BYTES + 1, 0);
        let error = check_images(&[attachment("image/png", &big, None)]).unwrap_err();
        assert!(matches!(error, AttachmentError::TooLarge { .. }), "{error}");
        // Far past the cap, the encoded length alone refuses it.
        let huge = ImageAttachment {
            media_type: "image/png".into(),
            data: "A".repeat(MAX_IMAGE_BYTES * 2),
            name: None,
        };
        assert!(matches!(
            check_images(&[huge]).unwrap_err(),
            AttachmentError::TooLarge { .. }
        ));
    }

    #[test]
    fn too_many_images_and_too_many_bytes_are_refused() {
        let one = attachment("image/png", PNG, None);
        let many = vec![one.clone(); MAX_IMAGES_PER_MESSAGE + 1];
        assert_eq!(
            check_images(&many).unwrap_err(),
            AttachmentError::TooMany {
                count: MAX_IMAGES_PER_MESSAGE + 1
            }
        );
        assert!(check_images(&vec![one; MAX_IMAGES_PER_MESSAGE]).is_ok());

        let mut large = PNG.to_vec();
        large.resize(MAX_IMAGE_BYTES, 0);
        let fits = MAX_MESSAGE_IMAGE_BYTES / MAX_IMAGE_BYTES;
        let over = vec![attachment("image/png", &large, None); fits + 1];
        assert!(matches!(
            check_images(&over).unwrap_err(),
            AttachmentError::TotalTooLarge { .. }
        ));
    }

    #[test]
    fn bad_base64_is_refused_with_the_image_named() {
        let broken = ImageAttachment {
            media_type: "image/png".into(),
            data: "not base64!".into(),
            name: Some("x.png".into()),
        };
        let error = check_images(&[broken]).unwrap_err();
        assert!(matches!(error, AttachmentError::NotBase64 { .. }));
        assert!(error.to_string().contains("x.png"), "{error}");
    }

    #[test]
    fn a_relative_attachment_directory_is_refused() {
        let error = check_attachment_dir(Some(Path::new("data/aether.attachments"))).unwrap_err();
        assert!(matches!(error, RuntimeError::Invalid(_)), "{error}");
        assert!(error.to_string().contains("data/aether.attachments"));
        assert!(check_attachment_dir(Some(Path::new("/var/lib/aether.attachments"))).is_ok());
        assert!(check_attachment_dir(None).is_ok());
    }

    #[test]
    fn storing_twice_keeps_one_private_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let checked = check_images(&[attachment("image/png", PNG, None)]).expect("checked");
        store_image(dir.path(), &checked[0]).expect("first store");
        store_image(dir.path(), &checked[0]).expect("second store");
        let files: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .map(|entry| entry.expect("entry").path())
            .collect();
        assert_eq!(
            files,
            vec![dir.path().join(checked[0].reference.file_name())]
        );
        assert_eq!(std::fs::read(&files[0]).expect("read"), PNG);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&files[0])
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn an_image_sent_without_words_has_no_empty_text_block() {
        let checked = check_images(&[attachment("image/png", PNG, None)]).expect("checked");
        let reference = checked[0].reference.clone();
        let alone = message_content(String::new(), vec![reference.clone()]);
        assert_eq!(alone, vec![ContentBlock::ImageRef(reference.clone())]);
        let with_words = message_content("look".into(), vec![reference.clone()]);
        assert_eq!(
            with_words,
            vec![
                ContentBlock::text("look"),
                ContentBlock::ImageRef(reference)
            ]
        );
        // A message with neither is what it always was: one text block.
        assert_eq!(
            message_content(String::new(), Vec::new()),
            vec![ContentBlock::text("")]
        );
    }
}
