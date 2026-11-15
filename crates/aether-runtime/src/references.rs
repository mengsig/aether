//! Task references: checked once at creation, stored like message images,
//! and re-attached to every turn so compaction never drops them.

use std::path::{Path, PathBuf};

use aether_core::attachment::{AttachmentId, MAX_IMAGE_BYTES, check_image, sniff_media_type};
use aether_core::reference::{
    MAX_REFERENCE_FILE_BYTES, MAX_REFERENCE_IMAGES, MAX_REFERENCES, ReferenceKind,
    ReferenceRequest, TaskReference, check_budget, clean_label, merge, same_reference,
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

/// Where a delegating agent may take its child's references from: inside
/// its scope's roots, or a path its own task already carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceScope {
    /// Canonical workspace root and allowed paths; one that does not
    /// resolve holds nothing, so it is left out.
    roots: Vec<PathBuf>,
    /// Canonical paths of the delegating agent's own task references.
    own: Vec<PathBuf>,
}

impl ReferenceScope {
    fn admits(&self, path: &Path) -> bool {
        self.own.iter().any(|own| own == path)
            || self.roots.iter().any(|root| path.starts_with(root))
    }
}

/// Read one named reference and check it: an absolute, readable regular
/// file within its kind's size limit, in `scope` when there is one, and for
/// an image, a supported type. The file is opened once; its type, path,
/// scope and size are all taken from the open handle, so a path swapped
/// meanwhile cannot change what is read or where it is said to lie.
fn read_reference(
    request: &ReferenceRequest,
    scope: Option<&ReferenceScope>,
) -> std::result::Result<(TaskReference, Option<CheckedImage>), String> {
    let shown = request.path.display();
    if !request.path.is_absolute() {
        return Err(format!("reference {shown} is not an absolute path"));
    }
    let unreadable = |error: std::io::Error| format!("reference {shown} cannot be read: {error}");
    let file = open_without_blocking(&request.path).map_err(unreadable)?;
    let opened = file.metadata().map_err(unreadable)?;
    if !opened.is_file() {
        return Err(format!("reference {shown} is not a regular file"));
    }
    let path = path_of_opened(&file).map_err(unreadable)?;
    if scope.is_some_and(|scope| !scope.admits(&path)) {
        return Err(format!(
            "reference {shown} is outside your workspace and allowed paths, and not one of \
             your task's references"
        ));
    }
    let limit = match request.kind {
        ReferenceKind::Image => MAX_IMAGE_BYTES as u64,
        ReferenceKind::File => MAX_REFERENCE_FILE_BYTES,
    };
    let bytes = read_at_most(file, limit).map_err(unreadable)?;
    if bytes.len() as u64 > limit {
        return Err(format!(
            "reference {shown} is too large: more than {limit} bytes for a {} reference",
            request.kind.as_str()
        ));
    }
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
    debug_assert!(reference.path.is_absolute());
    debug_assert_eq!(
        reference.image.is_some(),
        request.kind == ReferenceKind::Image
    );
    Ok((reference, image))
}

/// Open for reading without waiting: a FIFO or device named as a reference
/// opens at once and is then refused as not a regular file.
fn open_without_blocking(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(path)
}

/// The path the kernel holds for the open file itself. Nothing re-resolves
/// the requested name, so a path swapped after the open cannot vouch for it.
#[cfg(target_os = "linux")]
fn path_of_opened(file: &std::fs::File) -> std::io::Result<PathBuf> {
    use std::os::fd::AsRawFd as _;
    let path = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
    check_opened_path(path)
}

#[cfg(target_vendor = "apple")]
fn path_of_opened(file: &std::fs::File) -> std::io::Result<PathBuf> {
    let mut path = PathBuf::new();
    nix::fcntl::fcntl(file, nix::fcntl::FcntlArg::F_GETPATH(&mut path))?;
    check_opened_path(path)
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn path_of_opened(_file: &std::fs::File) -> std::io::Result<PathBuf> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "this platform cannot name an open file's path",
    ))
}

/// A handle's path must be absolute; Linux marks one whose file was
/// removed since, and such a file is no longer where it was named.
fn check_opened_path(path: PathBuf) -> std::io::Result<PathBuf> {
    let removed = path.to_string_lossy().ends_with(" (deleted)");
    if !path.is_absolute() || removed {
        return Err(std::io::Error::other(
            "it was moved or removed while it was being opened",
        ));
    }
    Ok(path)
}

/// At most `limit + 1` bytes from `file`, so a file that grew past its
/// limit after it was opened is seen as too large rather than read whole.
fn read_at_most(file: std::fs::File, limit: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    debug_assert!(bytes.len() as u64 <= limit + 1);
    Ok(bytes)
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
    /// when any reference is unreadable, outside `scope` or over a budget.
    pub(crate) fn resolve_references(
        &self,
        requests: &[ReferenceRequest],
        inherited: Vec<TaskReference>,
        scope: Option<&ReferenceScope>,
    ) -> Result<ResolvedReferences> {
        // Bounded before anything is read; the budget is then checked as each
        // one is read, so at most one image past it is ever held.
        if requests.len() > MAX_REFERENCES {
            return Err(RuntimeError::Invalid(format!(
                "too many references: {} named, at most {MAX_REFERENCES} per task",
                requests.len()
            )));
        }
        let mut named: Vec<TaskReference> = Vec::with_capacity(requests.len());
        let mut images = Vec::new();
        for request in requests {
            let (reference, image) =
                read_reference(request, scope).map_err(RuntimeError::Invalid)?;
            if named.iter().any(|held| same_reference(held, &reference)) {
                continue;
            }
            named.push(reference);
            // The merged set holds every distinct named one, so this refuses
            // nothing the whole set would take.
            check_budget(&named).map_err(RuntimeError::Invalid)?;
            images.extend(image);
        }
        debug_assert!(images.len() <= MAX_REFERENCE_IMAGES);
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

/// What a delegating agent passes its child: the scope its named references
/// are read in (none for an agent with no workspace root or allowed paths),
/// plus with `inherit` its own task's whole reference set.
pub(crate) fn delegated_references(
    scope: &aether_core::agent::Scope,
    own: &[TaskReference],
    inherit: bool,
) -> (Option<ReferenceScope>, Vec<TaskReference>) {
    let mut named_roots: Vec<&Path> = scope.allowed_paths.iter().map(PathBuf::as_path).collect();
    named_roots.extend(scope.workspace_root.as_deref());
    let reference_scope = (!named_roots.is_empty()).then(|| ReferenceScope {
        roots: named_roots
            .iter()
            .filter_map(|root| std::fs::canonicalize(root).ok())
            .collect(),
        own: own.iter().map(|reference| reference.path.clone()).collect(),
    });
    let inherited = if inherit { own.to_vec() } else { Vec::new() };
    debug_assert!(inherit || inherited.is_empty());
    debug_assert!(
        reference_scope
            .as_ref()
            .is_none_or(|scope| scope.roots.len() <= named_roots.len())
    );
    (reference_scope, inherited)
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

#[cfg(test)]
#[path = "references_tests.rs"]
mod tests;
