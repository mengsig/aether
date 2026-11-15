use super::*;
use aether_core::agent::Scope;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\npixels";

fn request(path: &Path, kind: ReferenceKind, label: Option<&str>) -> ReferenceRequest {
    ReferenceRequest {
        path: path.to_path_buf(),
        kind,
        label: label.map(str::to_owned),
    }
}

#[test]
fn an_image_reference_is_read_once_with_its_canonical_path_and_digest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mock.png");
    std::fs::write(&path, PNG).expect("write");
    let named = request(&path, ReferenceKind::Image, Some("approved\nmockup"));
    let (reference, image) = read_reference(&named).expect("a PNG is a valid image reference");
    assert_eq!(
        reference.path,
        std::fs::canonicalize(&path).expect("canonical")
    );
    assert_eq!(reference.label, "approved mockup");
    assert_eq!(reference.sha256, AttachmentId::of(PNG).to_string());
    assert_eq!(reference.size_bytes, PNG.len() as u64);
    let image = image.expect("an image carries bytes to store");
    assert_eq!(image.bytes, PNG);
    assert_eq!(reference.image.as_ref(), Some(&image.reference));
    assert_eq!(image.reference.media_type, "image/png");
}

#[test]
fn a_file_reference_is_named_by_its_file_and_stores_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("intent.md");
    std::fs::write(&path, "R1: ISO labels").expect("write");
    let (reference, image) =
        read_reference(&request(&path, ReferenceKind::File, None)).expect("a readable file");
    assert_eq!(reference.label, "intent.md");
    assert!(reference.image.is_none() && image.is_none());
    assert!(
        reference.line().starts_with("[file] intent.md: "),
        "{}",
        reference.line()
    );
}

#[test]
fn an_unusable_reference_is_refused_by_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("gone.png");
    let error = read_reference(&request(&missing, ReferenceKind::Image, None)).unwrap_err();
    assert!(
        error.contains("gone.png") && error.contains("cannot be read"),
        "{error}"
    );

    let relative = read_reference(&request(Path::new("mock.png"), ReferenceKind::Image, None));
    assert!(relative.unwrap_err().contains("not an absolute path"));

    let folder = read_reference(&request(dir.path(), ReferenceKind::File, None)).unwrap_err();
    assert!(folder.contains("not a regular file"), "{folder}");

    let text = dir.path().join("notes.txt");
    std::fs::write(&text, "plain").expect("write");
    let error = read_reference(&request(&text, ReferenceKind::Image, None)).unwrap_err();
    assert!(error.contains("kind `file`"), "{error}");

    let big = dir.path().join("big.png");
    let mut bytes = PNG.to_vec();
    bytes.resize(MAX_IMAGE_BYTES + 1, 0);
    std::fs::write(&big, bytes).expect("write");
    let error = read_reference(&request(&big, ReferenceKind::Image, None)).unwrap_err();
    assert!(error.contains("too large"), "{error}");
}

fn scoped(root: &Path) -> Scope {
    Scope {
        workspace_root: Some(root.to_path_buf()),
        ..Scope::default()
    }
}

#[test]
fn a_delegated_reference_must_lie_in_scope_or_on_the_callers_task() {
    let workspace = tempfile::tempdir().expect("workspace");
    let elsewhere = tempfile::tempdir().expect("elsewhere");
    let inside = workspace.path().join("mock.png");
    let outside = elsewhere.path().join("approved.png");
    std::fs::write(&inside, PNG).expect("write");
    std::fs::write(&outside, PNG).expect("write");
    let scope = scoped(workspace.path());

    let ok = delegated_references(
        &scope,
        &[],
        vec![request(&inside, ReferenceKind::Image, None)],
        false,
    );
    assert!(ok.is_ok(), "{ok:?}");

    let error = delegated_references(
        &scope,
        &[],
        vec![request(&outside, ReferenceKind::Image, None)],
        false,
    )
    .unwrap_err();
    assert!(error.contains("outside your workspace"), "{error}");

    // A symlink inside the workspace still resolves outside it.
    let link = workspace.path().join("link.png");
    std::os::unix::fs::symlink(&outside, &link).expect("symlink");
    let error = delegated_references(
        &scope,
        &[],
        vec![request(&link, ReferenceKind::Image, None)],
        false,
    )
    .unwrap_err();
    assert!(error.contains("outside your workspace"), "{error}");

    // One the caller's own task carries may be named again from anywhere.
    let (own, _) = read_reference(&request(&outside, ReferenceKind::Image, None)).expect("own");
    let (named, inherited) = delegated_references(
        &scope,
        std::slice::from_ref(&own),
        vec![request(&outside, ReferenceKind::Image, None)],
        false,
    )
    .expect("an own reference passes");
    assert_eq!(named.len(), 1);
    assert!(inherited.is_empty(), "nothing is inherited unless asked");
}

#[test]
fn inheriting_passes_on_the_callers_whole_reference_set_only_when_asked() {
    let workspace = tempfile::tempdir().expect("workspace");
    let path = workspace.path().join("mock.png");
    std::fs::write(&path, PNG).expect("write");
    let (own, _) = read_reference(&request(&path, ReferenceKind::Image, None)).expect("own");
    let scope = scoped(workspace.path());
    let (_, inherited) =
        delegated_references(&scope, std::slice::from_ref(&own), Vec::new(), true).expect("ok");
    assert_eq!(inherited, vec![own.clone()]);
    let (_, none) =
        delegated_references(&scope, std::slice::from_ref(&own), Vec::new(), false).expect("ok");
    assert!(none.is_empty());
}

#[test]
fn the_delegate_reply_warns_when_the_child_takes_no_images() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mock.png");
    std::fs::write(&path, PNG).expect("write");
    let (image, _) = read_reference(&request(&path, ReferenceKind::Image, None)).expect("image");
    assert_eq!(delegated_reference_note(&[], true), None);
    let seen = delegated_reference_note(std::slice::from_ref(&image), true).expect("a note");
    assert!(seen.contains("attached as pictures"), "{seen}");
    let blind = delegated_reference_note(std::slice::from_ref(&image), false).expect("a note");
    assert!(
        blind.contains("Warning") && blind.contains("paths only"),
        "{blind}"
    );
}
