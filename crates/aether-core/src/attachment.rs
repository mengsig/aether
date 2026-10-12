//! Images a person sends with a message.
//!
//! The bytes travel base64-encoded in the command. The daemon stores each
//! distinct image once, under its digest, and everything durable (inbox,
//! events, transcript) carries an [`AttachmentRef`] instead of the bytes.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

/// Largest single image, in bytes (3.75 MiB). The model APIs cap an image at
/// 5 MB measured on its base64 form, which is a third larger than the file.
pub const MAX_IMAGE_BYTES: usize = 3_932_160;

/// Most images one message may carry.
pub const MAX_IMAGES_PER_MESSAGE: usize = 10;

/// Most image bytes one message may carry in total (10 MiB), so its encoded
/// form stays inside one protocol frame and one engine input line.
pub const MAX_MESSAGE_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// Most images one model request may carry, history included. Past 20, the
/// model APIs hold every image of a request to a smaller pixel size.
pub const MAX_REQUEST_IMAGES: usize = 20;

/// Most image bytes one model request may carry, history included (15 MiB,
/// 20 MiB as base64): well inside the 32 MB the model APIs take per request.
pub const MAX_REQUEST_IMAGE_BYTES: usize = 15 * 1024 * 1024;

// The newest message is always sent whole.
const _: () = assert!(
    MAX_REQUEST_IMAGES >= MAX_IMAGES_PER_MESSAGE
        && MAX_REQUEST_IMAGE_BYTES >= MAX_MESSAGE_IMAGE_BYTES
);

/// Longest file name kept with an image; a longer one is cut.
pub const MAX_ATTACHMENT_NAME_CHARS: usize = 120;

/// The image types a message may carry, with the extension each is stored
/// under.
pub const IMAGE_MEDIA_TYPES: [(&str, &str); 4] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/gif", "gif"),
    ("image/webp", "webp"),
];

/// One image as a client sends it with a `send_message` command.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ImageAttachment {
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`. The daemon
    /// checks it against the bytes.
    pub media_type: String,
    /// The image bytes, base64 (standard alphabet, padded).
    pub data: String,
    /// The file name the image had, when the client knows one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ImageAttachment {
    /// Its size once decoded, read off the base64 length.
    pub fn size_bytes(&self) -> u64 {
        let symbols = self.data.trim_end_matches('=').len() as u64;
        symbols * 3 / 4
    }

    /// The image as its stored reference will name it: `shot.png, 1.2 MB`.
    pub fn summary(&self) -> String {
        let name = self.name.as_deref().and_then(clean_name);
        let name = name.as_deref().unwrap_or(&self.media_type);
        format!("{name}, {}", human_size(self.size_bytes()))
    }
}

// The payload is megabytes of base64; a debug print names it instead.
impl fmt::Debug for ImageAttachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageAttachment")
            .field("media_type", &self.media_type)
            .field("data", &format_args!("<{} base64 chars>", self.data.len()))
            .field("name", &self.name)
            .finish()
    }
}

/// Why an image was refused. The daemon answers with this text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttachmentError {
    #[error("too many images: {count} attached, at most {MAX_IMAGES_PER_MESSAGE} per message")]
    TooMany { count: usize },
    #[error("image {label} is not valid base64: {reason}")]
    NotBase64 { label: String, reason: String },
    #[error("image {label} is empty")]
    Empty { label: String },
    #[error(
        "image {label} is too large: {size} bytes, at most {MAX_IMAGE_BYTES} bytes ({limit}) per image",
        limit = human_size(MAX_IMAGE_BYTES as u64)
    )]
    TooLarge { label: String, size: usize },
    #[error(
        "images are too large together: {size} bytes, at most {MAX_MESSAGE_IMAGE_BYTES} bytes ({limit}) per message",
        limit = human_size(MAX_MESSAGE_IMAGE_BYTES as u64)
    )]
    TotalTooLarge { size: usize },
    #[error("image {label} is not a supported type: its bytes are not PNG, JPEG, GIF or WebP")]
    UnsupportedType { label: String },
    #[error("image {label} is declared as {declared} but its bytes are {actual}")]
    TypeMismatch {
        label: String,
        declared: String,
        actual: &'static str,
    },
}

/// The SHA-256 of an image's bytes, as 64 lowercase hex characters. It names
/// the stored file, so nothing else ever parses as one.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AttachmentId(String);

impl AttachmentId {
    /// The id of exactly these bytes.
    pub fn of(bytes: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(bytes)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AttachmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for AttachmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for AttachmentId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let well_formed = s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !well_formed {
            return Err(format!(
                "expected an attachment id of 64 lowercase hex characters, got `{s}`"
            ));
        }
        Ok(Self(s.to_owned()))
    }
}

impl Serialize for AttachmentId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AttachmentId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl schemars::JsonSchema for AttachmentId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "AttachmentId".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": "^[0-9a-f]{64}$",
            "description": "The SHA-256 of the stored bytes, in lowercase hex.",
        })
    }
}

/// A stored image, as inboxes, events and transcripts carry it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AttachmentRef {
    pub id: AttachmentId,
    pub media_type: String,
    pub size_bytes: u64,
    /// The file name the image was sent under, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl AttachmentRef {
    /// The file this image is stored as inside the daemon's attachment
    /// directory: its digest plus the extension of its type.
    pub fn file_name(&self) -> String {
        match extension_of(&self.media_type) {
            Some(extension) => format!("{}.{extension}", self.id),
            None => self.id.to_string(),
        }
    }

    /// What a person calls this image: its file name, else its type.
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.media_type)
    }

    /// The image as a person names it: `shot.png, 1.2 MB`.
    pub fn summary(&self) -> String {
        format!("{}, {}", self.display_name(), human_size(self.size_bytes))
    }

    /// The one-line stand-in shown wherever the image itself cannot be:
    /// `[image: shot.png, 1.2 MB]`.
    pub fn label(&self) -> String {
        format!("[image: {}]", self.summary())
    }
}

/// The image type these bytes are, read from their leading magic bytes.
pub fn sniff_media_type(bytes: &[u8]) -> Option<&'static str> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.starts_with(PNG) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// The extension a supported image type is stored under.
pub fn extension_of(media_type: &str) -> Option<&'static str> {
    IMAGE_MEDIA_TYPES
        .iter()
        .find(|(known, _)| *known == media_type)
        .map(|(_, extension)| *extension)
}

/// Check one decoded image against the type and size rules and name what it
/// is stored as. `label` identifies the image in the error.
pub fn check_image(
    bytes: &[u8],
    declared: &str,
    name: Option<&str>,
    label: &str,
) -> Result<AttachmentRef, AttachmentError> {
    let label = || label.to_owned();
    if bytes.is_empty() {
        return Err(AttachmentError::Empty { label: label() });
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(AttachmentError::TooLarge {
            label: label(),
            size: bytes.len(),
        });
    }
    let Some(actual) = sniff_media_type(bytes) else {
        return Err(AttachmentError::UnsupportedType { label: label() });
    };
    if declared != actual {
        return Err(AttachmentError::TypeMismatch {
            label: label(),
            declared: declared.to_owned(),
            actual,
        });
    }
    Ok(AttachmentRef {
        id: AttachmentId::of(bytes),
        media_type: actual.to_owned(),
        size_bytes: bytes.len() as u64,
        name: name.and_then(clean_name),
    })
}

/// A file name safe to print in a prompt and a terminal: the last path
/// component, on one line, without control characters, cut to a fixed length.
pub fn clean_name(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = crate::text::strip_controls(base)
        .chars()
        .filter(|c| *c != '\n')
        .take(MAX_ATTACHMENT_NAME_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

/// A byte count as a person reads it: `812 B`, `34.1 KB`, `1.2 MB`.
pub fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if bytes < KB {
        return format!("{bytes} B");
    }
    if bytes < MB {
        return format!("{:.1} KB", bytes as f64 / KB as f64);
    }
    format!("{:.1} MB", bytes as f64 / MB as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest file each sniffed type accepts: its magic bytes.
    pub(crate) fn sample(media_type: &str) -> Vec<u8> {
        match media_type {
            "image/png" => b"\x89PNG\r\n\x1a\nrest".to_vec(),
            "image/jpeg" => vec![0xff, 0xd8, 0xff, 0xe0, 0x00],
            "image/gif" => b"GIF89a-rest".to_vec(),
            "image/webp" => b"RIFF\x04\x00\x00\x00WEBPVP8 ".to_vec(),
            other => panic!("no sample for {other}"),
        }
    }

    #[test]
    fn each_supported_type_is_recognised_by_its_bytes() {
        for (media_type, _) in IMAGE_MEDIA_TYPES {
            assert_eq!(sniff_media_type(&sample(media_type)), Some(media_type));
        }
        assert_eq!(sniff_media_type(b"%PDF-1.7"), None);
        assert_eq!(sniff_media_type(b"RIFF\x04\x00\x00\x00WAVEfmt "), None);
        assert_eq!(sniff_media_type(b""), None);
    }

    #[test]
    fn a_checked_image_is_named_by_its_digest_and_type() {
        let bytes = sample("image/png");
        let image = check_image(&bytes, "image/png", Some("/home/user/shot.png"), "1")
            .expect("a PNG is accepted");
        assert_eq!(image.id, AttachmentId::of(&bytes));
        assert_eq!(image.size_bytes, bytes.len() as u64);
        assert_eq!(image.name.as_deref(), Some("shot.png"));
        assert_eq!(image.file_name(), format!("{}.png", image.id));
        assert_eq!(
            image.label(),
            format!("[image: shot.png, {} B]", bytes.len())
        );
    }

    #[test]
    fn the_declared_type_must_match_the_bytes() {
        let error = check_image(&sample("image/gif"), "image/png", None, "2").unwrap_err();
        assert_eq!(
            error,
            AttachmentError::TypeMismatch {
                label: "2".into(),
                declared: "image/png".into(),
                actual: "image/gif",
            }
        );
        let error = check_image(b"plain text", "image/png", None, "notes.txt").unwrap_err();
        assert_eq!(
            error,
            AttachmentError::UnsupportedType {
                label: "notes.txt".into()
            }
        );
        assert!(error.to_string().contains("not PNG, JPEG, GIF or WebP"));
    }

    #[test]
    fn an_image_at_the_limit_passes_and_one_byte_more_does_not() {
        let mut bytes = sample("image/png");
        bytes.resize(MAX_IMAGE_BYTES, 0);
        assert!(check_image(&bytes, "image/png", None, "1").is_ok());
        bytes.push(0);
        let error = check_image(&bytes, "image/png", None, "big.png").unwrap_err();
        assert_eq!(
            error,
            AttachmentError::TooLarge {
                label: "big.png".into(),
                size: MAX_IMAGE_BYTES + 1
            }
        );
        assert!(error.to_string().contains("3.8 MB"), "{error}");
        assert_eq!(
            check_image(b"", "image/png", None, "1").unwrap_err(),
            AttachmentError::Empty { label: "1".into() }
        );
    }

    #[test]
    fn an_attachment_id_is_only_ever_a_digest() {
        let id = AttachmentId::of(b"x");
        assert_eq!(id.as_str().parse::<AttachmentId>(), Ok(id.clone()));
        for hostile in ["../../etc/passwd", "", "ABCDEF", &"g".repeat(64)] {
            assert!(hostile.parse::<AttachmentId>().is_err(), "{hostile}");
        }
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(
            serde_json::from_str::<AttachmentId>(&json).expect("round trip"),
            id
        );
        assert!(serde_json::from_str::<AttachmentId>("\"../x\"").is_err());
    }

    #[test]
    fn a_name_is_one_printable_path_component() {
        assert_eq!(clean_name("C:\\shots\\a b.png").as_deref(), Some("a b.png"));
        assert_eq!(
            clean_name("evil\u{1b}[2J\nname.png").as_deref(),
            Some("evil[2Jname.png")
        );
        assert_eq!(clean_name("dir/"), None);
        assert_eq!(clean_name("  "), None);
        let long = "n".repeat(MAX_ATTACHMENT_NAME_CHARS + 30);
        assert_eq!(
            clean_name(&long).map(|n| n.chars().count()),
            Some(MAX_ATTACHMENT_NAME_CHARS)
        );
    }

    #[test]
    fn sizes_read_in_the_unit_a_person_expects() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(34_918), "34.1 KB");
        assert_eq!(human_size(1_258_291), "1.2 MB");
    }

    /// "pixels" in base64 at each padding: 6, 5 and 4 bytes.
    #[test]
    fn an_unsent_image_reads_like_its_stored_reference() {
        let image = |data: &str, name: Option<&str>| ImageAttachment {
            media_type: "image/png".into(),
            data: data.into(),
            name: name.map(str::to_owned),
        };
        assert_eq!(image("cGl4ZWxz", None).size_bytes(), 6);
        assert_eq!(image("cGl4ZWw=", None).size_bytes(), 5);
        assert_eq!(image("cGl4ZQ==", None).size_bytes(), 4);
        assert_eq!(image("", None).size_bytes(), 0);
        assert_eq!(
            image("cGl4ZWxz", Some("/home/user/shot.png")).summary(),
            "shot.png, 6 B"
        );
        assert_eq!(image("cGl4ZWxz", None).summary(), "image/png, 6 B");
    }
}
