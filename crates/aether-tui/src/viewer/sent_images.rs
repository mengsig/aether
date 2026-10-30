//! Images sent with a message: their transcript lines open the copy the
//! daemon stored under the image's digest, never a file in the workspace.

use std::io::Read;
use std::path::{Path, PathBuf};

use aether_core::attachment::{AttachmentId, IMAGE_MEDIA_TYPES, MAX_IMAGE_BYTES};
use anyhow::{Context, Result, bail, ensure};

use super::references::{Reference, parse};
use super::{Preview, ReadRequest, files, graphics, load};
use crate::app::transcript::CellModel;
use crate::ui::cells::ChatCell;
use crate::ui::cells::text::clean;

/// Marks a sent image's reference. `references::parse` refuses any scheme
/// but `file:`, so no link in a message can carry it.
const SCHEME: &str = "aether-image:";

/// Shown when the daemon no longer holds an image a transcript names.
pub const NOT_STORED: &str = "This image is no longer stored.";

/// Whether `target` names a sent image rather than a file.
pub fn is_sent_image(target: &str) -> bool {
    target.starts_with(SCHEME)
}

/// What the file picker lists for `reference`: a sent image by its line.
pub fn picker_name(reference: &Reference) -> &str {
    if is_sent_image(&reference.target) {
        return &reference.label;
    }
    &reference.target
}

/// The references a cell offers. A user cell's image lines open the stored
/// image; only the words around them are parsed for files.
pub fn cell_references(model: &CellModel) -> Vec<Reference> {
    let CellModel::User { text, images, .. } = model else {
        return model.copy_source().map(parse).unwrap_or_default();
    };
    if images.is_empty() {
        return parse(text);
    }
    let (words, lines) = crate::app::split_image_lines(text, images);
    let mut references = parse(&words);
    // Labelled as `ui::cells::user` draws the line, so the click finds it.
    references.extend(images.iter().zip(lines).map(|(image, line)| Reference {
        target: format!("{SCHEME}{}", image.file_name()),
        label: clean(&line),
    }));
    debug_assert!(references.iter().all(|r| !r.target.is_empty()));
    references
}

/// The stored file and digest a sent image's target names, when well formed.
fn stored_file(target: &str) -> Option<(&str, AttachmentId)> {
    let file = target.strip_prefix(SCHEME)?;
    let (digest, extension) = file.split_once('.')?;
    let known = IMAGE_MEDIA_TYPES
        .iter()
        .any(|(_, known)| *known == extension);
    let digest = digest.parse::<AttachmentId>().ok()?;
    known.then_some((file, digest))
}

/// Preview the daemon's stored copy of a sent image, checked against its digest.
pub(super) fn preview(request: &ReadRequest) -> Result<Preview> {
    ensure!(is_sent_image(&request.target), "not a sent image");
    let Some((file, digest)) = stored_file(&request.target) else {
        bail!("a sent image names no stored file");
    };
    let Some(dir) = request.stored_images.as_deref() else {
        bail!(NOT_STORED);
    };
    let (path, bytes) = read_stored(&dir.join(file))?;
    ensure!(
        AttachmentId::of(&bytes) == digest,
        "{NOT_STORED} The stored copy does not match it."
    );
    let image = graphics::decode(&bytes).context("decode the stored image")?;
    load::picture(path, &image, request, None)
}

/// The stored file's resolved path and bytes; a missing one is said plainly.
fn read_stored(path: &Path) -> Result<(PathBuf, Vec<u8>)> {
    let path = match path.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => bail!(NOT_STORED),
        Err(error) => {
            return Err(error).with_context(|| format!("resolve {}", path.display()));
        }
    };
    let file = files::open_canonical(&path).context("open the stored image")?;
    let limit = MAX_IMAGE_BYTES as u64;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .context("read the stored image")?;
    ensure!(!bytes.is_empty(), "{NOT_STORED} The stored copy is empty.");
    ensure!(
        bytes.len() as u64 <= limit,
        "the stored image is larger than a sent image can be"
    );
    Ok((path, bytes))
}
