//! References a task carries: approved images and files its owner must see.
//!
//! A creator names them by path; the daemon checks each once, stores an image
//! under its digest like a message image, and records a [`TaskReference`] on
//! the task. Every turn re-attaches the set, so compaction never reduces an
//! approved mockup to a marker.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::attachment::{AttachmentRef, MAX_IMAGE_BYTES, MAX_MESSAGE_IMAGE_BYTES, human_size};

/// Most references one task may carry, images and files together.
pub const MAX_REFERENCES: usize = 16;

/// Most image references one task may carry. Leaves room in a model request
/// (`MAX_REQUEST_IMAGES`) for the images of the conversation itself.
pub const MAX_REFERENCE_IMAGES: usize = 8;

/// Most image bytes one task's references may carry in total (6 MiB).
pub const MAX_REFERENCE_IMAGE_BYTES: u64 = 6 * 1024 * 1024;

/// Largest file reference (16 MiB): a reference is something to read whole.
pub const MAX_REFERENCE_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Longest label kept with a reference; a longer one is cut.
pub const MAX_REFERENCE_LABEL_CHARS: usize = 120;

const _: () = assert!(
    MAX_REFERENCE_IMAGES <= MAX_REFERENCES
        && MAX_REFERENCE_IMAGE_BYTES <= MAX_MESSAGE_IMAGE_BYTES as u64
        && MAX_REFERENCE_IMAGE_BYTES >= MAX_IMAGE_BYTES as u64
);

/// Whether a reference is delivered as a picture or named as a file to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    /// PNG, JPEG, GIF or WebP: attached as image content to a vision model.
    Image,
    /// Any readable file: named by path and digest for the owner to open.
    File,
}

impl ReferenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::File => "file",
        }
    }
}

/// One reference as a creator names it: `{path, kind, label}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReferenceRequest {
    /// An absolute path the daemon can read. A delegating agent's relative
    /// path resolves against its working directory.
    pub path: PathBuf,
    pub kind: ReferenceKind,
    /// What the reference is, e.g. `approved mockup: settings page, dark`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// One checked reference, as the task stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskReference {
    pub kind: ReferenceKind,
    /// The creator's label, else the file name.
    pub label: String,
    /// The canonical path the reference was read from.
    pub path: PathBuf,
    pub size_bytes: u64,
    /// SHA-256 of the bytes as they were checked, in lowercase hex.
    pub sha256: String,
    /// For an image: the stored copy delivered to the model, which a later
    /// edit of `path` cannot change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<AttachmentRef>,
}

impl TaskReference {
    /// One line naming the reference for a prompt:
    /// `[image] approved mockup: /work/mock.png (1.2 MB, sha256 ab12cd34ef56)`.
    pub fn line(&self) -> String {
        format!(
            "[{}] {}: {} ({}, sha256 {})",
            self.kind.as_str(),
            self.label,
            self.path.display(),
            human_size(self.size_bytes),
            &self.sha256[..self.sha256.len().min(12)]
        )
    }
}

/// A label safe to print on one prompt line, cut to a fixed length.
pub fn clean_label(label: &str) -> Option<String> {
    let cleaned: String = crate::text::strip_controls(label)
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .take(MAX_REFERENCE_LABEL_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

/// Check a whole reference set against the count and image budgets. The
/// error names the budget, so a creator can see which one it crossed.
pub fn check_budget(references: &[TaskReference]) -> Result<(), String> {
    if references.len() > MAX_REFERENCES {
        return Err(format!(
            "too many references: {} named, at most {MAX_REFERENCES} per task",
            references.len()
        ));
    }
    let images: Vec<&TaskReference> = references
        .iter()
        .filter(|reference| reference.kind == ReferenceKind::Image)
        .collect();
    if images.len() > MAX_REFERENCE_IMAGES {
        return Err(format!(
            "too many image references: {} named, at most {MAX_REFERENCE_IMAGES} per task",
            images.len()
        ));
    }
    let bytes: u64 = images.iter().map(|image| image.size_bytes).sum();
    if bytes > MAX_REFERENCE_IMAGE_BYTES {
        return Err(format!(
            "image references are too large together: {}, at most {} per task",
            human_size(bytes),
            human_size(MAX_REFERENCE_IMAGE_BYTES)
        ));
    }
    debug_assert!(images.iter().all(|image| image.image.is_some()));
    Ok(())
}

/// Whether two references name the same thing: one path, bytes and kind. A
/// file and an image of the same bytes differ, as only the image is shown.
pub fn same_reference(a: &TaskReference, b: &TaskReference) -> bool {
    a.path == b.path && a.sha256 == b.sha256 && a.kind == b.kind
}

/// `extra` appended to `base`, skipping any `base` already holds: inheriting
/// a reference and naming it again is one reference.
pub fn merge(mut base: Vec<TaskReference>, extra: Vec<TaskReference>) -> Vec<TaskReference> {
    for reference in extra {
        if !base.iter().any(|held| same_reference(held, &reference)) {
            base.push(reference);
        }
    }
    base
}

#[cfg(test)]
#[path = "reference_tests.rs"]
mod tests;
