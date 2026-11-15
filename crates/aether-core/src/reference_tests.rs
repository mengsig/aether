use super::*;
use crate::attachment::AttachmentId;

fn image(name: &str, size_bytes: u64) -> TaskReference {
    let id = AttachmentId::of(name.as_bytes());
    TaskReference {
        kind: ReferenceKind::Image,
        label: name.into(),
        path: PathBuf::from(format!("/work/{name}")),
        size_bytes,
        sha256: id.to_string(),
        image: Some(AttachmentRef {
            id,
            media_type: "image/png".into(),
            size_bytes,
            name: Some(name.into()),
        }),
    }
}

fn file(name: &str) -> TaskReference {
    TaskReference {
        kind: ReferenceKind::File,
        label: name.into(),
        path: PathBuf::from(format!("/work/{name}")),
        size_bytes: 10,
        sha256: AttachmentId::of(name.as_bytes()).to_string(),
        image: None,
    }
}

#[test]
fn a_request_reads_the_documented_wire_shape() {
    let request: ReferenceRequest = serde_json::from_value(serde_json::json!({
        "path": "/work/mock.png", "kind": "image", "label": "approved mockup"
    }))
    .expect("the documented shape parses");
    assert_eq!(request.kind, ReferenceKind::Image);
    assert_eq!(request.label.as_deref(), Some("approved mockup"));
    let unknown = serde_json::from_value::<ReferenceRequest>(serde_json::json!({
        "path": "/work/mock.png", "kind": "video"
    }));
    assert!(unknown.is_err(), "only image and file are kinds");
    let extra = serde_json::from_value::<ReferenceRequest>(serde_json::json!({
        "path": "/work/a", "kind": "file", "url": "x"
    }));
    assert!(extra.is_err(), "an unknown field is refused, not dropped");
}

#[test]
fn the_budget_counts_references_images_and_image_bytes() {
    let files: Vec<_> = (0..MAX_REFERENCES)
        .map(|i| file(&format!("{i}.md")))
        .collect();
    assert!(check_budget(&files).is_ok());
    let mut over = files.clone();
    over.push(file("one-more.md"));
    let error = check_budget(&over).unwrap_err();
    assert!(error.contains("too many references"), "{error}");

    let images: Vec<_> = (0..=MAX_REFERENCE_IMAGES)
        .map(|i| image(&format!("{i}.png"), 1))
        .collect();
    let error = check_budget(&images).unwrap_err();
    assert!(error.contains("too many image references"), "{error}");

    let half = MAX_REFERENCE_IMAGE_BYTES / 2;
    assert!(check_budget(&[image("a.png", half), image("b.png", half)]).is_ok());
    let error = check_budget(&[image("a.png", half), image("b.png", half + 1)]).unwrap_err();
    assert!(error.contains("too large together"), "{error}");
}

#[test]
fn merging_keeps_one_copy_of_a_reference_named_twice() {
    let base = vec![image("mock.png", 5), file("spec.md")];
    let merged = merge(base.clone(), vec![image("mock.png", 5), file("notes.md")]);
    let labels: Vec<_> = merged.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["mock.png", "spec.md", "notes.md"]);
    // Same path, other bytes: a changed file is a different reference.
    let mut changed = file("spec.md");
    changed.sha256 = AttachmentId::of(b"other").to_string();
    assert_eq!(merge(base, vec![changed]).len(), 3);
}

#[test]
fn a_label_is_one_printable_line_and_a_line_names_the_digest() {
    assert_eq!(
        clean_label("mock\nup\u{1b}[2J").as_deref(),
        Some("mock up[2J")
    );
    assert_eq!(clean_label("   "), None);
    let long = "l".repeat(MAX_REFERENCE_LABEL_CHARS + 5);
    assert_eq!(
        clean_label(&long).map(|l| l.chars().count()),
        Some(MAX_REFERENCE_LABEL_CHARS)
    );
    let line = image("mock.png", 2048).line();
    assert!(
        line.starts_with("[image] mock.png: /work/mock.png (2.0 KB, sha256 "),
        "{line}"
    );
}
