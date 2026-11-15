//! Task references: checked once at creation, stored like message images,
//! and re-attached to every turn so compaction never drops them.

use std::path::{Path, PathBuf};

use aether_core::attachment::{AttachmentId, MAX_IMAGE_BYTES, check_image, sniff_media_type};
use aether_core::reference::{
    MAX_REFERENCE_FILE_BYTES, ReferenceKind, ReferenceRequest, TaskReference, check_budget,
    clean_label, merge,
};
use aether_core::task::Task;
use aether_engine::EngineImage;

use crate::attachments::CheckedImage;
use crate::{Result, Runtime, RuntimeError};

/// A task's references, checked, with the image bytes still to be stored.
#[derive(Debug, Default)]
pub(crate) struct ResolvedReferences {
    pub references: Vec<TaskReference>,
    pub images: Vec<CheckedImage>,
}

/// Read one named reference and check it: an absolute, readable regular
/// file within its kind's size limit, and for an image, a supported type.
fn read_reference(
    request: &ReferenceRequest,
) -> std::result::Result<(TaskReference, Option<CheckedImage>), String> {
    let shown = request.path.display();
    if !request.path.is_absolute() {
        return Err(format!("reference {shown} is not an absolute path"));
    }
    let path = std::fs::canonicalize(&request.path)
        .map_err(|error| format!("reference {shown} cannot be read: {error}"))?;
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("reference {shown} cannot be read: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("reference {shown} is not a regular file"));
    }
    let limit = match request.kind {
        ReferenceKind::Image => MAX_IMAGE_BYTES as u64,
        ReferenceKind::File => MAX_REFERENCE_FILE_BYTES,
    };
    if metadata.len() > limit {
        return Err(format!(
            "reference {shown} is too large: {} bytes, at most {limit} for a {} reference",
            metadata.len(),
            request.kind.as_str()
        ));
    }
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("reference {shown} cannot be read: {error}"))?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let label = request
        .label
        .as_deref()
        .and_then(clean_label)
        .or_else(|| file_name.as_deref().and_then(clean_label))
        .unwrap_or_else(|| request.kind.as_str().to_owned());
    let size_bytes = bytes.len() as u64;
    let sha256 = AttachmentId::of(&bytes).to_string();
    let image = match request.kind {
        ReferenceKind::File => None,
        ReferenceKind::Image => Some(checked_image(bytes, file_name.as_deref(), &shown)?),
    };
    let reference = TaskReference {
        kind: request.kind,
        label,
        path,
        size_bytes,
        sha256,
        image: image.as_ref().map(|image| image.reference.clone()),
    };
    debug_assert!(size_bytes <= limit, "the size was checked before reading");
    debug_assert_eq!(
        reference.image.is_some(),
        request.kind == ReferenceKind::Image
    );
    Ok((reference, image))
}

fn checked_image(
    bytes: Vec<u8>,
    name: Option<&str>,
    shown: &std::path::Display<'_>,
) -> std::result::Result<CheckedImage, String> {
    let Some(media_type) = sniff_media_type(&bytes) else {
        return Err(format!(
            "reference {shown} is not a PNG, JPEG, GIF or WebP image; name it with kind `file`"
        ));
    };
    let reference = check_image(&bytes, media_type, name, &shown.to_string())
        .map_err(|error| format!("reference {error}"))?;
    Ok(CheckedImage { reference, bytes })
}

impl Runtime {
    /// Check `requests` and add them to `inherited`, refusing the whole set
    /// when any reference is unreadable or the set crosses a budget.
    pub(crate) fn resolve_references(
        &self,
        requests: &[ReferenceRequest],
        inherited: Vec<TaskReference>,
    ) -> Result<ResolvedReferences> {
        let mut named = Vec::with_capacity(requests.len());
        let mut images = Vec::new();
        for request in requests {
            let (reference, image) = read_reference(request).map_err(RuntimeError::Invalid)?;
            named.push(reference);
            images.extend(image);
        }
        let references = merge(inherited, named);
        check_budget(&references).map_err(RuntimeError::Invalid)?;
        if !images.is_empty() && self.attachment_dir().is_none() {
            return Err(RuntimeError::Invalid(
                "this daemon has no attachment directory, so a task cannot carry image references"
                    .into(),
            ));
        }
        debug_assert!(images.iter().all(|image| !image.bytes.is_empty()));
        Ok(ResolvedReferences { references, images })
    }

    /// The task's image references as an engine takes them: stored files.
    pub(crate) fn reference_engine_images(&self, task: Option<&Task>) -> Vec<EngineImage> {
        let mut images = Vec::new();
        for image in reference_images(task) {
            match self.attachment_path(image).filter(|path| path.is_file()) {
                Some(path) => images.push(EngineImage {
                    path,
                    media_type: image.media_type.clone(),
                }),
                None => tracing::warn!(
                    image = %image.id,
                    "a task's reference image is no longer stored; the engine gets its path only"
                ),
            }
        }
        images
    }
}

fn reference_images(
    task: Option<&Task>,
) -> impl Iterator<Item = &aether_core::attachment::AttachmentRef> {
    task.into_iter()
        .flat_map(|task| &task.references)
        .filter_map(|reference| reference.image.as_ref())
}

/// What a delegating agent may attach to its child: paths inside its own
/// scope, or references its own task already carries, plus with `inherit`
/// its task's whole set. The error names the first path it may not pass on.
pub(crate) fn delegated_references(
    scope: &aether_core::agent::Scope,
    own: &[TaskReference],
    requests: Vec<ReferenceRequest>,
    inherit: bool,
) -> std::result::Result<(Vec<ReferenceRequest>, Vec<TaskReference>), String> {
    let mut roots: Vec<&Path> = scope.allowed_paths.iter().map(PathBuf::as_path).collect();
    roots.extend(scope.workspace_root.as_deref());
    for request in &requests {
        let shown = request.path.display();
        let path = std::fs::canonicalize(&request.path)
            .map_err(|error| format!("reference {shown} cannot be read: {error}"))?;
        let own_reference = own.iter().any(|reference| reference.path == path);
        if !roots.is_empty() && !own_reference && !within_any(&path, &roots) {
            return Err(format!(
                "reference {shown} is outside your workspace and allowed paths, and not one of \
                 your task's references"
            ));
        }
    }
    let inherited = if inherit { own.to_vec() } else { Vec::new() };
    debug_assert!(inherit || inherited.is_empty());
    Ok((requests, inherited))
}

/// The delegate reply's account of what the child's task carries, and why
/// a child that takes no images still gets their paths.
pub(crate) fn delegated_reference_note(
    references: &[TaskReference],
    vision: bool,
) -> Option<String> {
    if references.is_empty() {
        return None;
    }
    let images = references
        .iter()
        .filter(|reference| reference.kind == ReferenceKind::Image)
        .count();
    let delivery = match (images, vision) {
        (0, _) => String::new(),
        (_, true) => format!(
            " {images} image(s) are attached as pictures before it starts and after each compaction."
        ),
        (_, false) => format!(
            " Warning: the child's model takes no images, so its {images} image reference(s) reach it \
             as paths only; delegate to a vision model if it must see them."
        ),
    };
    Some(format!(
        " The child's task carries {} reference(s).{delivery}",
        references.len()
    ))
}

/// Whether `path` lies inside one of `roots`, compared canonically so a
/// symlink cannot carry a reference out of the delegating agent's scope.
pub(crate) fn within_any(path: &Path, roots: &[&Path]) -> bool {
    // A root that does not resolve holds nothing, so it cannot admit a path.
    roots
        .iter()
        .any(|root| std::fs::canonicalize(root).is_ok_and(|root| path.starts_with(root)))
}

#[cfg(test)]
#[path = "references_tests.rs"]
mod tests;
