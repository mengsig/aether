//! A sent image's transcript line opens the copy the daemon stored, however
//! the image arrived and whichever client shows the transcript (#345).
use super::*;
use crate::testing::render_with;
use crate::ui::transcript::ViewCache;
use crate::viewer::{Documents, Preview, graphics, sent_images, tools};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

/// A decodable PNG whose pixels depend on `seed`.
fn picture(seed: u8) -> Vec<u8> {
    let pixels = image::RgbaImage::from_fn(6, 4, |x, y| {
        image::Rgba([seed.wrapping_add(x as u8 * 40), y as u8 * 60, 200, 255])
    });
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(pixels)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encode a png");
    bytes
}

fn clipboard_image(bytes: &[u8]) -> ImageAttachment {
    ImageAttachment {
        media_type: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        name: None,
    }
}

/// What the daemon does with a sent message's images: store each under its
/// digest in `store`, and deliver the message naming the stored copies.
fn daemon_stores(store: &std::path::Path, images: &[ImageAttachment]) -> Vec<ContentBlock> {
    images
        .iter()
        .map(|image| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&image.data)
                .expect("base64");
            let reference = aether_core::attachment::check_image(
                &bytes,
                &image.media_type,
                image.name.as_deref(),
                "1",
            )
            .expect("the daemon takes the image");
            std::fs::write(store.join(reference.file_name()), &bytes).expect("store");
            ContentBlock::ImageRef(reference)
        })
        .collect()
}

/// An app viewing a ready agent, reading images from `store`.
fn app_with_store(store: &std::path::Path) -> App {
    let (mut app, _) = app_viewing(AgentStatus::Ready);
    app.options.attachment_dir = Some(store.to_owned());
    app.size = (160, 40);
    app.board.open = false;
    app
}

/// Press and release the left button on `hit`.
fn click(app: &mut App, cache: &mut ViewCache, hit: &crate::viewer::Hit) -> Vec<Effect> {
    let mouse = |kind| MouseEvent {
        kind,
        column: hit.area.x + 1,
        row: hit.area.y,
        modifiers: KeyModifiers::NONE,
    };
    app.dispatch_mouse(mouse(MouseEventKind::Down(MouseButton::Left)), cache);
    app.dispatch_mouse(mouse(MouseEventKind::Up(MouseButton::Left)), cache)
}

/// Click the transcript line that reads `line`, and load what it asks for.
fn click_line<'a>(app: &'a mut App, line: &str) -> &'a Result<Preview, String> {
    let mut cache = ViewCache::default();
    let screen = render_with(app, &mut cache, 160, 40);
    assert!(screen.contains(line), "{screen}");
    let hit = cache
        .file_hits
        .iter()
        .find(|hit| hit.reference.label == line)
        .unwrap_or_else(|| panic!("`{line}` is clickable: {:?}", cache.file_hits))
        .clone();
    let effects = click(app, &mut cache, &hit);
    load(app, effects)
}

/// Read the preview `effects` ask for, as the viewer worker does; the viewer shows it.
fn load(app: &mut App, effects: Vec<Effect>) -> &Result<Preview, String> {
    let [Effect::PreviewFile(request)] = &effects[..] else {
        panic!("one preview asked for: {effects:?}");
    };
    assert!(sent_images::is_sent_image(&request.target), "{request:?}");
    let mut documents = Documents::new(tools::Tools::default());
    let result = crate::viewer::read(request, &mut documents).map_err(|e| format!("{e:#}"));
    let id = request.id;
    app.reduce(AppEvent::PreviewReady { id, result });
    assert!(app.viewer.open);
    assert_eq!(app.viewer.pending.as_ref().map(|r| r.id), Some(id));
    app.viewer.preview.as_ref().expect("the preview landed")
}

/// The half-block rows `bytes` paint in the pane `preview` was drawn for.
fn assert_shows(preview: &Result<Preview, String>, bytes: &[u8]) {
    let Ok(Preview::Image { rows, path, .. }) = preview else {
        panic!("an image preview: {preview:?}");
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let original = dir.path().join("original.png");
    std::fs::write(&original, bytes).expect("write");
    let decoded = graphics::load(&original).expect("decode the original");
    let width = rows.iter().map(|row| row.width()).max().unwrap_or(0);
    let expected = graphics::render_half_blocks(&decoded, width as u16, rows.len() as u16);
    assert_eq!(*rows, expected, "the stored pixels, not another file's");
    assert_eq!(std::fs::read(path).expect("the stored file"), bytes);
}

#[test]
fn a_pasted_image_opens_from_its_transcript_line() {
    let store = tempfile::tempdir().expect("store");
    let mut app = app_with_store(store.path());
    let bytes = picture(1);
    attach(&mut app, clipboard_image(&bytes));
    let (_, _, images) = sent(&app.reduce(key(KeyCode::Enter)));
    push_user_content(&mut app, daemon_stores(store.path(), &images));

    let line = format!("[image: image/png, {} B]", bytes.len());
    assert_shows(click_line(&mut app, &line), &bytes);
    let screen = render_to(&app, 160, 40);
    assert!(screen.contains("Sent image"), "{screen}");
    assert!(!screen.contains("File not found"), "{screen}");
}

#[test]
fn an_attached_file_opens_after_it_was_deleted_not_a_workspace_namesake() {
    let store = tempfile::tempdir().expect("store");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut app = app_with_store(store.path());
    app.options.cwd = Some(workspace.path().to_owned());
    std::fs::write(workspace.path().join("shot.png"), picture(9)).expect("namesake");
    let (_dir, path) = png_file("shot.png");
    let bytes = picture(2);
    std::fs::write(&path, &bytes).expect("write");

    assert_eq!(
        app.run_image_command(ImageArg::File(path.display().to_string())),
        vec![Effect::AttachImage(ImageSource::File(path.clone()))]
    );
    let read = aether_client::images::read_image_file(&path).map_err(|e| e.to_string());
    app.reduce(AppEvent::ImageRead(read));
    let (_, _, images) = sent(&app.reduce(key(KeyCode::Enter)));
    std::fs::remove_file(&path).expect("the file goes away");
    push_user_content(&mut app, daemon_stores(store.path(), &images));

    let line = format!("[image: shot.png, {} B]", bytes.len());
    assert_shows(click_line(&mut app, &line), &bytes);
}

#[test]
fn another_client_opens_the_image_until_it_is_no_longer_stored() {
    let store = tempfile::tempdir().expect("store");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut sender = app_with_store(store.path());
    let bytes = picture(3);
    attach(&mut sender, clipboard_image(&bytes));
    let (_, _, images) = sent(&sender.reduce(key(KeyCode::Enter)));
    let delivered = daemon_stores(store.path(), &images);
    drop(sender);

    // A restarted TUI, or a second one, sees the message as the daemon replays it.
    let mut viewer = app_with_store(store.path());
    viewer.options.cwd = Some(workspace.path().to_owned());
    push_user_content(&mut viewer, delivered.clone());
    let line = format!("[image: image/png, {} B]", bytes.len());
    assert_shows(click_line(&mut viewer, &line), &bytes);

    // The file picker, the keyboard's way in, lists the line and opens it too.
    viewer.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::ALT,
    )));
    let listed: Vec<_> = viewer
        .viewer
        .picker
        .iter()
        .map(sent_images::picker_name)
        .collect();
    assert_eq!(listed, [line.as_str()]);
    let effects = viewer.reduce(key(KeyCode::Enter));
    assert_shows(load(&mut viewer, effects), &bytes);

    let ContentBlock::ImageRef(stored) = &delivered[0] else {
        panic!("a stored image");
    };
    std::fs::remove_file(store.path().join(stored.file_name())).expect("pruned");
    let Some(message) = click_line(&mut viewer, &line).as_ref().err().cloned() else {
        panic!("a missing image is an error");
    };
    assert_eq!(message, sent_images::NOT_STORED);
    let screen = render_to(&viewer, 160, 40);
    assert!(screen.contains(sent_images::NOT_STORED), "{screen}");
    assert!(!screen.contains("ask the agent"), "{screen}");
}

#[test]
fn a_stored_file_that_is_not_the_image_is_refused() {
    let store = tempfile::tempdir().expect("store");
    let mut app = app_with_store(store.path());
    let bytes = picture(4);
    let delivered = daemon_stores(store.path(), &[clipboard_image(&bytes)]);
    let ContentBlock::ImageRef(stored) = &delivered[0] else {
        panic!("a stored image");
    };
    std::fs::write(store.path().join(stored.file_name()), picture(5)).expect("swap");
    push_user_content(&mut app, delivered);
    let line = format!("[image: image/png, {} B]", bytes.len());
    let Err(message) = click_line(&mut app, &line) else {
        panic!("another image's bytes are not shown");
    };
    assert!(message.starts_with(sent_images::NOT_STORED), "{message}");
}

#[test]
fn a_remote_client_says_where_the_image_is_and_reads_nothing() {
    let store = tempfile::tempdir().expect("store");
    let mut app = app_with_store(store.path());
    let delivered = daemon_stores(store.path(), &[clipboard_image(&picture(6))]);
    push_user_content(&mut app, delivered);
    let remote = aether_client::SshRemote::new("box", "user@example.com").expect("a remote");
    app.options.endpoint = Some(aether_client::Endpoint::Ssh(remote));
    assert!(app.options.is_remote());
    let reference = sent_images::cell_references(
        &app.transcripts[&app.selected.expect("viewed")]
            .cells
            .last()
            .expect("the message")
            .model,
    )
    .pop()
    .expect("the image line");
    assert!(app.open_reference(reference).is_empty());
    assert!(!app.viewer.open);
    let flash = app.live_flash().expect("said why");
    assert!(flash.contains("remote host"), "{flash}");
}

#[test]
fn images_with_one_summary_get_lines_of_their_own() {
    let image = |seed: u8| AttachmentRef {
        id: AttachmentId::of(&[seed]),
        media_type: "image/png".into(),
        size_bytes: 1_300_000,
        name: None,
    };
    let images = vec![image(1), image(2)];
    let model = crate::app::transcript::CellModel::User {
        from: crate::app::transcript::UserFrom::Operator,
        text: crate::app::with_image_lines(
            "see image/png",
            images.iter().map(AttachmentRef::summary),
        ),
        images: images.clone(),
    };
    let references = sent_images::cell_references(&model);
    let labels: Vec<_> = references.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "image/png",
            "[image: image/png, 1.2 MB]",
            "[image 2: image/png, 1.2 MB]"
        ]
    );
    assert_eq!(
        references[0].target, "image/png",
        "the words keep their file links"
    );
    for (reference, image) in references[1..].iter().zip(&images) {
        assert!(sent_images::is_sent_image(&reference.target));
        assert!(reference.target.ends_with(&image.file_name()));
    }
}

/// Send the file `name` holding `bytes` with `words`, as the daemon stores it.
fn send_file(app: &mut App, store: &std::path::Path, name: &str, words: &str, bytes: &[u8]) {
    let (_dir, path) = png_file(name);
    std::fs::write(&path, bytes).expect("write");
    app.run_image_command(ImageArg::File(path.display().to_string()));
    let read = aether_client::images::read_image_file(&path).map_err(|e| e.to_string());
    app.reduce(AppEvent::ImageRead(read));
    app.reduce(AppEvent::Paste(words.into()));
    let (_, text, images) = sent(&app.reduce(key(KeyCode::Enter)));
    assert_eq!((text.as_str(), images.len()), (words, 1));
    let mut content = vec![ContentBlock::text(words)];
    content.extend(daemon_stores(store, &images));
    push_user_content(app, content);
}

#[test]
fn a_file_named_in_the_words_still_opens_the_image_from_its_line() {
    let store = tempfile::tempdir().expect("store");
    let mut app = app_with_store(store.path());
    let name = format!("{}.png", "a".repeat(90));
    let bytes = picture(7);
    send_file(
        &mut app,
        store.path(),
        &name,
        &format!("see {name}"),
        &bytes,
    );

    let mut cache = ViewCache::default();
    render_with(&app, &mut cache, 160, 40);
    let line = format!("[image: {name}, {} B]", bytes.len());
    let image = cache
        .file_hits
        .iter()
        .find(|h| h.reference.label == line)
        .expect("the image line");
    // The longer workspace link sits inside the image line; the image wins it.
    let inside = cache
        .file_hits
        .iter()
        .find(|h| h.reference.target == name && h.area.y == image.area.y)
        .expect("the name is a link inside the image line")
        .clone();
    assert!(inside.reference.target.len() > image.reference.target.len());
    let effects = click(&mut app, &mut cache, &inside);
    assert_shows(load(&mut app, effects), &bytes);
}

#[test]
fn an_image_line_is_drawn_and_clicked_as_written() {
    let store = tempfile::tempdir().expect("store");
    let mut app = app_with_store(store.path());
    let bytes = picture(8);
    send_file(
        &mut app,
        store.path(),
        "shot`one`.png",
        "look `here`",
        &bytes,
    );
    let screen = render_to(&app, 160, 40);
    assert!(
        screen.contains("look here"),
        "the words keep inline code: {screen}"
    );
    let line = format!("[image: shot`one`.png, {} B]", bytes.len());
    assert_shows(click_line(&mut app, &line), &bytes);
}

#[test]
fn words_quoting_an_image_line_keep_their_file_link() {
    let store = tempfile::tempdir().expect("store");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut app = app_with_store(store.path());
    app.options.cwd = Some(workspace.path().to_owned());
    std::fs::write(workspace.path().join("shot.png"), picture(9)).expect("namesake");
    let bytes = picture(10);
    let line = format!("[image: shot.png, {} B]", bytes.len());
    send_file(
        &mut app,
        store.path(),
        "shot.png",
        &format!("see {line}"),
        &bytes,
    );

    let mut cache = ViewCache::default();
    render_with(&app, &mut cache, 160, 40);
    let images: Vec<_> = cache
        .file_hits
        .iter()
        .filter(|h| sent_images::is_sent_image(&h.reference.target))
        .collect();
    assert_eq!(images.len(), 1, "only the drawn image line: {images:?}");
    let quoted = cache
        .file_hits
        .iter()
        .filter(|h| h.reference.target == "shot.png")
        .min_by_key(|h| h.area.y)
        .expect("the file named in the words")
        .clone();
    assert!(quoted.area.y < images[0].area.y);
    let effects = click(&mut app, &mut cache, &quoted);
    let [Effect::PreviewFile(request)] = &effects[..] else {
        panic!("one preview asked for: {effects:?}");
    };
    assert_eq!(request.target, "shot.png");
    assert_shows(click_line(&mut app, &line), &bytes);
}

#[test]
fn names_differing_only_in_case_open_their_own_images() {
    let store = tempfile::tempdir().expect("store");
    let mut app = app_with_store(store.path());
    let first = picture(20);
    let second = (21..=255)
        .map(picture)
        .find(|other| other.len() == first.len() && *other != first)
        .expect("another picture of the same size");
    for (name, bytes) in [("shot.png", &first), ("SHOT.png", &second)] {
        let (_dir, path) = png_file(name);
        std::fs::write(&path, bytes).expect("write");
        app.run_image_command(ImageArg::File(path.display().to_string()));
        let read = aether_client::images::read_image_file(&path).map_err(|e| e.to_string());
        app.reduce(AppEvent::ImageRead(read));
    }
    let (_, _, images) = sent(&app.reduce(key(KeyCode::Enter)));
    push_user_content(&mut app, daemon_stores(store.path(), &images));

    let size = first.len();
    assert_shows(
        click_line(&mut app, &format!("[image: shot.png, {size} B]")),
        &first,
    );
    let line = format!("[image 2: SHOT.png, {size} B]");
    assert_shows(click_line(&mut app, &line), &second);
}

#[test]
fn a_name_quoting_another_image_line_opens_its_own_image() {
    quoting_line_opens_its_own_image("b.png", "b.png");
    // Kelvin signs fold to `k`: the inner line is longer in bytes, not as matched.
    quoting_line_opens_its_own_image(&"\u{212A}".repeat(30), &"k".repeat(30));
}

/// Send an image named after `quoted`'s line, then one named `inner`; a click on
/// the quoted line inside the first opens the first, and the second's own line the second.
fn quoting_line_opens_its_own_image(inner: &str, quoted: &str) {
    let store = tempfile::tempdir().expect("store");
    let mut app = app_with_store(store.path());
    let (outer_bytes, inner_bytes) = (picture(30), picture(31));
    let size = inner_bytes.len();
    let name = format!("outer [image: {quoted}.png, {size} B].png");
    let inner_name = format!("{inner}.png");
    for (name, bytes) in [(&name, &outer_bytes), (&inner_name, &inner_bytes)] {
        let (_dir, path) = png_file(name);
        std::fs::write(&path, bytes).expect("write");
        app.run_image_command(ImageArg::File(path.display().to_string()));
        let read = aether_client::images::read_image_file(&path).map_err(|e| e.to_string());
        app.reduce(AppEvent::ImageRead(read));
    }
    let (_, _, images) = sent(&app.reduce(key(KeyCode::Enter)));
    push_user_content(&mut app, daemon_stores(store.path(), &images));

    let mut cache = ViewCache::default();
    render_with(&app, &mut cache, 160, 40);
    let line = format!("[image: {name}, {} B]", outer_bytes.len());
    let inner_line = format!("[image: {inner_name}, {size} B]");
    let hits_of = |label: &str| -> Vec<crate::viewer::Hit> {
        let mut hits: Vec<_> = cache
            .file_hits
            .iter()
            .filter(|h| h.reference.label == label)
            .cloned()
            .collect();
        hits.sort_by_key(|h| h.area.y);
        hits
    };
    let own = hits_of(&line).first().expect("its line").clone();
    let inner_hits = hits_of(&inner_line);
    // Inside the first line, where both images' labels match.
    let inside = inner_hits
        .iter()
        .find(|h| h.area.y == own.area.y)
        .expect("the quoted line matches")
        .clone();
    let below = inner_hits.last().expect("the second image's line").clone();
    assert!(below.area.y > own.area.y, "{inner_hits:?}");
    let effects = click(&mut app, &mut cache, &inside);
    assert_shows(load(&mut app, effects), &outer_bytes);
    let effects = click(&mut app, &mut cache, &below);
    assert_shows(load(&mut app, effects), &inner_bytes);
}

#[test]
fn a_name_spelling_another_images_reference_opens_its_own_image() {
    let store = tempfile::tempdir().expect("store");
    let mut app = app_with_store(store.path());
    let (spelled, other) = (picture(40), picture(41));
    let digest = aether_core::attachment::AttachmentId::of(&other);
    let name = format!("aether-image:{digest}.png");
    let long = format!("{}.png", "b".repeat(110));
    for (name, bytes) in [(&name, &spelled), (&long, &other)] {
        let (_dir, path) = png_file(name);
        std::fs::write(&path, bytes).expect("write");
        app.run_image_command(ImageArg::File(path.display().to_string()));
        let read = aether_client::images::read_image_file(&path).map_err(|e| e.to_string());
        app.reduce(AppEvent::ImageRead(read));
    }
    let (_, _, images) = sent(&app.reduce(key(KeyCode::Enter)));
    push_user_content(&mut app, daemon_stores(store.path(), &images));

    let mut cache = ViewCache::default();
    render_with(&app, &mut cache, 160, 40);
    let line = format!("[image: {name}, {} B]", spelled.len());
    let mut on_name = cache
        .file_hits
        .iter()
        .find(|h| h.reference.label == line)
        .expect("its line")
        .clone();
    // Press on the name, which spells the other image's target; its label is longer.
    on_name.area.x += "[image: ".len() as u16 + 10;
    assert!(
        on_name.area.width > 30,
        "the line is on one row: {on_name:?}"
    );
    let effects = click(&mut app, &mut cache, &on_name);
    assert_shows(load(&mut app, effects), &spelled);
}
