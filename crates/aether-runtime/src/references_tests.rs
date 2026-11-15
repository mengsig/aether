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
    let (reference, image) =
        read_reference(&named, None).expect("a PNG is a valid image reference");
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
        read_reference(&request(&path, ReferenceKind::File, None), None).expect("a readable file");
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
    let error = read_reference(&request(&missing, ReferenceKind::Image, None), None).unwrap_err();
    assert!(
        error.contains("gone.png") && error.contains("cannot be read"),
        "{error}"
    );

    let relative = read_reference(
        &request(Path::new("mock.png"), ReferenceKind::Image, None),
        None,
    );
    assert!(relative.unwrap_err().contains("not an absolute path"));

    let folder = read_reference(&request(dir.path(), ReferenceKind::File, None), None).unwrap_err();
    assert!(folder.contains("not a regular file"), "{folder}");

    let text = dir.path().join("notes.txt");
    std::fs::write(&text, "plain").expect("write");
    let error = read_reference(&request(&text, ReferenceKind::Image, None), None).unwrap_err();
    assert!(error.contains("kind `file`"), "{error}");

    let big = dir.path().join("big.png");
    let mut bytes = PNG.to_vec();
    bytes.resize(MAX_IMAGE_BYTES + 1, 0);
    std::fs::write(&big, bytes).expect("write");
    let error = read_reference(&request(&big, ReferenceKind::Image, None), None).unwrap_err();
    assert!(error.contains("too large"), "{error}");
}

fn scoped(root: &Path) -> Scope {
    Scope {
        workspace_root: Some(root.to_path_buf()),
        ..Scope::default()
    }
}

fn image_in(scope: &ReferenceScope, path: &Path) -> std::result::Result<TaskReference, String> {
    read_reference(&request(path, ReferenceKind::Image, None), Some(scope)).map(|(r, _)| r)
}

#[test]
fn a_delegated_reference_must_lie_in_scope_or_on_the_callers_task() {
    let workspace = tempfile::tempdir().expect("workspace");
    let elsewhere = tempfile::tempdir().expect("elsewhere");
    let inside = workspace.path().join("mock.png");
    let outside = elsewhere.path().join("approved.png");
    std::fs::write(&inside, PNG).expect("write");
    std::fs::write(&outside, PNG).expect("write");
    let (scope, _) = delegated_references(&scoped(workspace.path()), &[], false);
    let scope = scope.expect("a scoped caller reads in its scope");

    assert!(image_in(&scope, &inside).is_ok());
    let error = image_in(&scope, &outside).unwrap_err();
    assert!(error.contains("outside your workspace"), "{error}");

    // A symlink inside the workspace still resolves outside it.
    let link = workspace.path().join("link.png");
    std::os::unix::fs::symlink(&outside, &link).expect("symlink");
    let error = image_in(&scope, &link).unwrap_err();
    assert!(error.contains("outside your workspace"), "{error}");

    // One the caller's own task carries may be named again from anywhere.
    let (own, _) =
        read_reference(&request(&outside, ReferenceKind::Image, None), None).expect("own");
    let (scope, inherited) =
        delegated_references(&scoped(workspace.path()), std::slice::from_ref(&own), false);
    let scope = scope.expect("scoped");
    assert_eq!(
        image_in(&scope, &outside).expect("an own reference passes"),
        own
    );
    assert!(inherited.is_empty(), "nothing is inherited unless asked");

    let (unscoped, _) = delegated_references(&Scope::default(), &[], false);
    assert!(unscoped.is_none(), "an unscoped caller is not narrowed");
}

#[test]
fn a_handle_is_placed_by_the_file_it_opened_whatever_the_path_names_later() {
    let workspace = tempfile::tempdir().expect("workspace");
    let elsewhere = tempfile::tempdir().expect("elsewhere");
    let inside = workspace.path().join("mock.png");
    let outside = elsewhere.path().join("secret.png");
    std::fs::write(&inside, PNG).expect("write");
    std::fs::write(&outside, b"\x89PNG\r\n\x1a\nsecret").expect("write");
    let link = workspace.path().join("link.png");
    std::os::unix::fs::symlink(&outside, &link).expect("symlink");

    // The open follows the link outside. The link is then pointed inside,
    // and the inside file itself replaced by a link back out: every name
    // now resolves somewhere else, and the handle still names the secret.
    let file = open_without_blocking(&link).expect("open");
    std::fs::remove_file(&link).expect("unlink");
    std::os::unix::fs::symlink(&inside, &link).expect("relink");
    std::fs::remove_file(&inside).expect("remove inside");
    std::os::unix::fs::symlink(&outside, &inside).expect("swap inside");
    let placed = path_of_opened(&file).expect("the handle's path");
    assert_eq!(placed, std::fs::canonicalize(&outside).expect("canonical"));

    let (scope, _) = delegated_references(&scoped(workspace.path()), &[], false);
    assert!(!scope.expect("scoped").admits(&placed), "{placed:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_file_removed_after_the_open_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("gone.md");
    std::fs::write(&path, "R1").expect("write");
    let file = open_without_blocking(&path).expect("open");
    assert_eq!(
        path_of_opened(&file).expect("still there"),
        std::fs::canonicalize(&path).expect("canonical")
    );
    std::fs::remove_file(&path).expect("remove");
    let error = path_of_opened(&file).unwrap_err();
    assert!(error.to_string().contains("removed"), "{error}");
}

#[test]
fn a_read_stops_one_byte_past_the_limit_and_a_fifo_never_blocks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("grown.md");
    std::fs::write(&path, vec![b'x'; 100]).expect("write");
    let file = open_without_blocking(&path).expect("open");
    assert_eq!(read_at_most(file, 10).expect("read").len(), 11);
    let file = open_without_blocking(&path).expect("open");
    assert_eq!(read_at_most(file, 100).expect("read").len(), 100);

    let fifo = dir.path().join("pipe.md");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo runs");
    assert!(made.success());
    let error = read_reference(&request(&fifo, ReferenceKind::File, None), None).unwrap_err();
    assert!(error.contains("not a regular file"), "{error}");
}

#[test]
fn inheriting_passes_on_the_callers_whole_reference_set_only_when_asked() {
    let workspace = tempfile::tempdir().expect("workspace");
    let path = workspace.path().join("mock.png");
    std::fs::write(&path, PNG).expect("write");
    let (own, _) = read_reference(&request(&path, ReferenceKind::Image, None), None).expect("own");
    let scope = scoped(workspace.path());
    let (_, inherited) = delegated_references(&scope, std::slice::from_ref(&own), true);
    assert_eq!(inherited, vec![own.clone()]);
    let (_, none) = delegated_references(&scope, std::slice::from_ref(&own), false);
    assert!(none.is_empty());
}

#[test]
fn the_delegate_reply_warns_when_the_child_takes_no_images() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mock.png");
    std::fs::write(&path, PNG).expect("write");
    let (image, _) =
        read_reference(&request(&path, ReferenceKind::Image, None), None).expect("image");
    assert_eq!(delegated_reference_note(&[], true), None);
    let seen = delegated_reference_note(std::slice::from_ref(&image), true).expect("a note");
    assert!(seen.contains("attached as pictures"), "{seen}");
    let blind = delegated_reference_note(std::slice::from_ref(&image), false).expect("a note");
    assert!(
        blind.contains("Warning") && blind.contains("paths only"),
        "{blind}"
    );
}
